use std::collections::{BTreeSet, HashMap};

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::TimeDelta;
use tokio::task::{Id, JoinSet};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::api::Served;
use crate::briefs::Briefs;
use crate::clock::Clock;
use crate::domain::{
    Item, MergeMethod, Number, Snapshot, Timestamp, UpdateMethod, UpdateRule, label,
};
use crate::holds::Holds;
use crate::reconcile::{
    Action, DoneRun, FailedMerge, LiveRun, Memory, Policy, Subject, Work, WorkKind, Workflows,
    hold_note, reconcile,
};
use crate::run::{self, Shell, WorkflowEnd};
use crate::runs::{self, Recorded, RunOf, Runs, Watched};
use crate::runtime::{Runtime, UsageLimit};
use crate::spec_glob::SpecGlob;
use crate::status::{Polls, View};
use crate::store::{CloseReason, IssueRead, Store};
use crate::webhook::{self, Pending};
use crate::workflow::{self, LastRuns, Workflow, WorkflowFiles, WorkflowName};

#[derive(Clone, Debug)]
pub struct FactoryConfig {
    pub spec_glob: SpecGlob,
    pub spec_lookback: TimeDelta,
    pub max_runs: usize,
    pub ci_grace: TimeDelta,
    pub ci_timeout: TimeDelta,
    pub ci_attempts: u32,
    pub result_retries: u32,
    pub session_timeout: std::time::Duration,
    /// `None` follows the repository's settings.
    pub update_method: Option<UpdateMethod>,
    pub auto_merge: bool,
    /// `None` follows the repository's settings.
    pub merge_method: Option<MergeMethod>,
    pub require_label: Option<String>,
    pub briefs: Briefs,
    pub work_dir: PathBuf,
    pub schedule_workflows: bool,
    pub waits_out_usage_limit: bool,
}

enum Running {
    Work(LiveRun),
    Workflow,
}

struct LiveEntry {
    running: Running,
    watched: Watched,
    cancel: CancellationToken,
}

pub struct Factory {
    shell: Arc<Shell>,
    runs: JoinSet<()>,
    refusals: tokio::sync::mpsc::UnboundedReceiver<UsageLimit>,
    live: HashMap<Id, LiveEntry>,
    stopping: CancellationToken,
    schedule_workflows: bool,
    last_runs: LastRuns,
    workflow_files: WorkflowFiles,
    memory: Memory,
    holds_record: HoldsRecord,
    kept: Option<Kept>,
    /// The cuts into tickets that ended since the kept snapshot was taken.
    ended: Vec<LiveRun>,
    /// The items Linewright's actions and runs changed since the kept
    /// snapshot last read them.
    written: BTreeSet<Item>,
}

enum HoldsRecord {
    /// There is no record yet: the next poll takes the holds from what its
    /// snapshot shows, so that a Linewright that kept none does not hold
    /// pull requests again.
    Missing,
    /// `saved` is `None` until first written, and a save that failed leaves
    /// it behind the holds remembered. The pull requests in `settling` were
    /// ready when the record began, before GitHub had worked out their merge
    /// state: they are taken from the snapshot too, once it has.
    Kept {
        saved: Option<Holds>,
        settling: BTreeSet<Number>,
    },
}

#[derive(Clone)]
struct Kept {
    snapshot: Arc<Snapshot>,
    taken_at: Timestamp,
    read_at: HashMap<Item, Timestamp>,
}

impl Kept {
    /// The run takes feedback since this as having arrived after.
    fn as_of(&self, work: &Work) -> Timestamp {
        let read_at = self.read_at.get(&work.starts_from());
        read_at.copied().unwrap_or(self.taken_at)
    }
}

impl Factory {
    pub async fn start(
        store: Arc<dyn Store>,
        runtime: Arc<dyn Runtime>,
        clock: Arc<dyn Clock>,
        config: FactoryConfig,
    ) -> Result<Factory> {
        let identity = store.identity().await.context("checking the token")?;
        let mut settings = store
            .settings()
            .await
            .context("reading the repository's settings")?;
        if let Some(method) = config.update_method {
            settings.update_rule = UpdateRule::Always(method);
        }
        if let Some(method) = config.merge_method {
            settings.merge_method = method;
        }
        if config.auto_merge {
            settings.check_auto_merge(config.update_method)?;
        }
        store
            .ensure_labels(&label::ALL)
            .await
            .context("creating labels")?;
        info!(
            account = %identity.login,
            default_branch = %settings.default_branch,
            update_rule = ?settings.update_rule,
            auto_merge = config.auto_merge,
            merge_method = %settings.merge_method,
            "started"
        );
        let last_runs = if config.schedule_workflows {
            LastRuns::load(&config.work_dir)
        } else {
            LastRuns::default()
        };
        let (report_refusal, refusals) = tokio::sync::mpsc::unbounded_channel();
        let runs = Runs::new(&config.work_dir);
        let held = Holds::load(&config.work_dir);
        let holds_record = match &held {
            Some(saved) => HoldsRecord::Kept {
                saved: Some(saved.clone()),
                settling: BTreeSet::new(),
            },
            None => HoldsRecord::Missing,
        };
        let shell = Shell {
            policy: Policy {
                linewright: identity.login.clone(),
                spec_glob: config.spec_glob,
                spec_lookback: config.spec_lookback,
                max_runs: config.max_runs,
                ci_grace: config.ci_grace,
                ci_timeout: config.ci_timeout,
                ci_attempts: config.ci_attempts,
                auto_merge: config.auto_merge.then_some(settings.merge_method),
                update_rule: settings.update_rule,
                allowed_merge_methods: settings.allowed_merge_methods.clone(),
                require_label: config.require_label,
            },
            store,
            runtime,
            clock,
            briefs: config.briefs,
            identity,
            settings,
            result_retries: config.result_retries,
            session_timeout: config.session_timeout,
            runs: runs.clone(),
            work_dir: config.work_dir,
            waits_out_usage_limit: config.waits_out_usage_limit,
            report_refusal,
        };
        Ok(Factory {
            shell: Arc::new(shell),
            runs: JoinSet::new(),
            refusals,
            live: HashMap::new(),
            stopping: CancellationToken::new(),
            schedule_workflows: config.schedule_workflows,
            last_runs,
            workflow_files: WorkflowFiles::default(),
            memory: Memory {
                done_runs: done_runs(&runs),
                held: held.unwrap_or_default(),
                ..Memory::default()
            },
            holds_record,
            kept: None,
            ended: vec![],
            written: BTreeSet::new(),
        })
    }

    /// Cancelling it lets the action in progress finish, so no action is left
    /// half done.
    pub fn stopping(&self) -> CancellationToken {
        self.stopping.clone()
    }

    pub async fn cycle(&mut self) -> Result<Vec<Action>> {
        // Reap first: a run that ends during the snapshot stays live for this
        // poll, so the snapshot cannot miss what it wrote.
        self.reap();
        let shell = &self.shell;
        let now = shell.clock.now();
        let snapshot = shell
            .store
            .snapshot(now - shell.policy.spec_lookback)
            .await
            .context("taking a snapshot")?;
        self.ended.clear();
        self.written.clear();
        let kept = Kept {
            snapshot: Arc::new(snapshot),
            taken_at: now,
            read_at: HashMap::new(),
        };
        let defined = self.workflows().await;
        Ok(self.reconcile(kept, defined, now).await)
    }

    /// The items Linewright changed are read before `subjects` are resolved:
    /// what a commit or a push names is found in the kept snapshot only once
    /// it holds them, such as a pull request a run opened or pushed to.
    pub async fn refresh(
        &mut self,
        subjects: impl IntoIterator<Item: Into<webhook::Subject>>,
    ) -> Result<Vec<Action>> {
        let Some(mut kept) = self.kept.clone() else {
            return self.cycle().await;
        };
        // Reap first, as a poll does: a run that ends during the reads stays
        // live for this refresh.
        self.reap();
        let shell = &self.shell;
        let now = shell.clock.now();
        let merged_since = now - shell.policy.spec_lookback;
        let snapshot = Arc::make_mut(&mut kept.snapshot);
        let mut read = read_all(snapshot, shell, self.written.clone(), merged_since).await?;
        let named: BTreeSet<Item> = subjects
            .into_iter()
            .flat_map(|subject| subject.into().items(snapshot))
            .filter(|item| !read.contains(item))
            .collect();
        if read.is_empty() && named.is_empty() {
            return Ok(vec![]);
        }
        read.extend(read_all(snapshot, shell, named, merged_since).await?);
        let others = closed_by_pulls_read(snapshot, &read);
        read.extend(read_all(snapshot, shell, others, merged_since).await?);
        kept.read_at
            .extend(read.into_iter().map(|item| (item, now)));
        self.written.clear();
        Ok(self.reconcile(kept, vec![], now).await)
    }

    /// A refresh never races a poll: one still running when the poll falls
    /// due finishes first.
    pub async fn serve(
        &mut self,
        poll_interval: Duration,
        pending: &Pending,
        served: &tokio::sync::watch::Sender<Option<Served>>,
    ) {
        let stopping = self.stopping.clone();
        while !stopping.is_cancelled() {
            // The poll reads what deliveries named before it as well.
            pending.take();
            if let Err(e) = self.cycle().await {
                error!(error = format!("{e:#}"), "poll failed");
            }
            let next_poll = Instant::now() + poll_interval;
            let next_poll_at = self.shell.clock.now() + poll_interval;
            served.send_replace(self.served(next_poll_at));
            loop {
                tokio::select! {
                    () = tokio::time::sleep_until(next_poll) => break,
                    () = stopping.cancelled() => break,
                    subjects = pending.next() => {
                        if let Err(e) = self.refresh(subjects).await {
                            error!(
                                error = format!("{e:#}"),
                                "acting on deliveries failed; the next poll catches up"
                            );
                        }
                    }
                    // Taken as they come, as the next poll would take them,
                    // so the view counts the runs in flight and the usage
                    // limit as they are.
                    Some(result) = self.runs.join_next_with_id(), if !self.runs.is_empty() => {
                        self.finished(result);
                    }
                    Some(limit) = self.refusals.recv() => self.refused(limit),
                }
                served.send_replace(self.served(next_poll_at));
            }
        }
    }

    fn served(&self, next_poll: Timestamp) -> Option<Served> {
        let runs = self.live.values().map(|entry| entry.watched.clone());
        Some(Served::new(
            self.view(next_poll)?,
            self.shell.store.clone(),
            runs.collect(),
        ))
    }

    fn view(&self, next_poll: Timestamp) -> Option<View> {
        let kept = self.kept.as_ref()?;
        let live = self.live_runs();
        let in_flight = self
            .live
            .values()
            .map(|entry| entry.watched.read())
            .collect();
        Some(View {
            snapshot: kept.snapshot.clone(),
            read_at: kept.read_at.clone(),
            policy: self.shell.policy.clone(),
            polls: Polls {
                last: kept.taken_at,
                next: next_poll,
            },
            memory: self.memory.clone(),
            live,
            in_flight,
            workflows: self.workflow_files.clone(),
            last_runs: self.last_runs.clone(),
        })
    }

    /// A cut that ended since the snapshot was taken counts as in flight: the
    /// snapshot may miss what it wrote.
    fn live_runs(&self) -> Vec<LiveRun> {
        let in_flight = self.live.values().filter_map(|entry| match entry.running {
            Running::Work(run) => Some(run),
            Running::Workflow => None,
        });
        self.ended.iter().copied().chain(in_flight).collect()
    }

    fn workflows_in_flight(&self) -> Vec<WorkflowName> {
        self.live
            .values()
            .filter_map(|entry| match &entry.watched.id.of {
                RunOf::Workflow { workflow } => Some(workflow.clone()),
                RunOf::Work { .. } => None,
            })
            .collect()
    }

    async fn reconcile(
        &mut self,
        kept: Kept,
        defined: Vec<Workflow>,
        now: Timestamp,
    ) -> Vec<Action> {
        let kept = self.kept.insert(kept).clone();
        let snapshot = &kept.snapshot;
        if let Some(resets_at) = self.memory.lift_usage_limit(now) {
            info!(%resets_at, "usage limit reset; sessions start again");
        }
        let live = self.live_runs();
        let workflows = Workflows {
            defined,
            last_runs: self.last_runs.clone(),
            in_flight: self.workflows_in_flight(),
        };
        self.take_shown_holds(snapshot);
        self.memory.retain_open(snapshot);
        self.record_holds();
        let actions = reconcile(
            snapshot,
            &live,
            &self.memory,
            &workflows,
            now,
            &self.shell.policy,
        );
        for action in &actions {
            if self.stopping.is_cancelled() {
                break;
            }
            self.written.extend(written(action, snapshot));
            if let Err(e) = self.execute(action, &kept).await {
                error!(action = ?action, error = format!("{e:#}"), "action failed");
            }
        }
        actions
    }

    pub async fn run_workflow(&self, name: &WorkflowName) -> Result<()> {
        let shell = &self.shell;
        let branch = &shell.settings.default_branch;
        let path = name.path();
        let text = shell
            .store
            .read_file(branch, &path)
            .await
            .with_context(|| format!("reading {path}"))?
            .with_context(|| format!("there is no workflow `{name}`: `{branch}` has no {path}"))?;
        let workflow =
            workflow::parse(name, &text).with_context(|| format!("{path} does not parse"))?;
        run_and_log(
            shell,
            &workflow,
            begin_workflow(shell, name)?,
            self.stopping.clone(),
        )
        .await
        .with_context(|| format!("the run of the workflow `{name}` failed"))
    }

    async fn workflows(&mut self) -> Vec<Workflow> {
        if !self.schedule_workflows {
            return vec![];
        }
        let branch = &self.shell.settings.default_branch;
        self.workflow_files = match workflow::read(self.shell.store.as_ref(), branch).await {
            Ok(files) => files,
            Err(e) => {
                error!(error = format!("{e:#}"), "listing the workflows failed");
                WorkflowFiles::default()
            }
        };
        for file in &self.workflow_files.unparsed {
            warn!(
                path = file.path,
                error = file.reason,
                "skipping a workflow file"
            );
        }
        self.workflow_files.parsed.clone()
    }

    pub fn is_idle(&self) -> bool {
        self.runs.is_empty()
    }

    pub async fn next_finished(&mut self) {
        if let Some(result) = self.runs.join_next_with_id().await {
            self.finished(result);
        }
    }

    pub async fn settle(&mut self) {
        while !self.is_idle() {
            self.next_finished().await;
        }
    }

    pub async fn stop(mut self) {
        for entry in self.live.values() {
            entry.cancel.cancel();
        }
        self.settle().await;
    }

    fn reap(&mut self) {
        while let Some(result) = self.runs.try_join_next_with_id() {
            self.finished(result);
        }
        while let Ok(limit) = self.refusals.try_recv() {
            self.refused(limit);
        }
    }

    fn take_shown_holds(&mut self, snapshot: &Snapshot) {
        let linewright = &self.shell.policy.linewright;
        let held = &mut self.memory.held;
        match &mut self.holds_record {
            HoldsRecord::Missing => {
                self.holds_record = HoldsRecord::Kept {
                    saved: None,
                    settling: held.take_shown(snapshot, linewright, |_| true),
                };
            }
            HoldsRecord::Kept { settling, .. } if !settling.is_empty() => {
                *settling = held.take_shown(snapshot, linewright, |pr| settling.contains(&pr));
            }
            HoldsRecord::Kept { .. } => {}
        }
    }

    fn record_holds(&mut self) {
        let HoldsRecord::Kept { saved, .. } = &mut self.holds_record else {
            return;
        };
        if saved.as_ref() == Some(&self.memory.held) {
            return;
        }
        match self.memory.held.save(&self.shell.work_dir) {
            Ok(()) => *saved = Some(self.memory.held.clone()),
            Err(e) => warn!(
                error = format!("{e:#}"),
                "recording the holds for review failed"
            ),
        }
    }

    fn refused(&mut self, UsageLimit { resets_at }: UsageLimit) {
        if self.memory.hold_until(resets_at) {
            info!(%resets_at, "usage limit in force; no session starts until it resets");
        }
    }

    fn finished(&mut self, result: Result<(Id, ()), tokio::task::JoinError>) {
        let id = match result {
            Ok((id, ())) => id,
            Err(e) => {
                error!(error = %e, "a run panicked");
                e.id()
            }
        };
        let Some(LiveEntry {
            running: Running::Work(run),
            watched,
            ..
        }) = self.live.remove(&id)
        else {
            return;
        };
        let done = self.shell.runs.read_run(&watched.id);
        self.memory
            .done_runs
            .extend(done.as_ref().and_then(Recorded::done_on_pull));
        match run.subject {
            // Reading the issue again reads what the run wrote: the issue,
            // and the pull requests that close it.
            Subject::Issue(issue) => {
                self.written.insert(Item::Issue(issue));
            }
            Subject::Pull(pr) => {
                self.written.insert(Item::Pull(pr));
            }
            // A cut writes issues the kept snapshot may lack, such as a
            // tracking issue it left part way.
            Subject::SpecChange(_) => self.ended.push(run),
        }
    }

    async fn execute(&mut self, action: &Action, kept: &Kept) -> Result<()> {
        let store = &self.shell.store;
        let snapshot = &kept.snapshot;
        match action {
            Action::Start(work) => {
                let shell = &self.shell;
                let running = shell.runs.begin(
                    RunOf::Work {
                        item: work.item(),
                        stage: work.kind(),
                    },
                    shell.clock.now(),
                )?;
                let id = running.id().to_string();
                let watched = running.watched();
                let cancel = CancellationToken::new();
                let reworked = match work.subject() {
                    Subject::Pull(pr) => self.memory.last_done(pr, WorkKind::Rework),
                    Subject::Issue(_) | Subject::SpecChange(_) => None,
                };
                let handle = self.runs.spawn(run::run(
                    shell.clone(),
                    work.clone(),
                    snapshot.clone(),
                    kept.as_of(work),
                    reworked,
                    running,
                    cancel.clone(),
                ));
                info!(run = id, subject = ?work.subject(), kind = ?work.kind(), "run started");
                self.live.insert(
                    handle.id(),
                    LiveEntry {
                        running: Running::Work(LiveRun {
                            subject: work.subject(),
                            kind: work.kind(),
                        }),
                        watched,
                        cancel,
                    },
                );
            }
            Action::Cancel { subject, reason } => {
                for entry in self.live.values() {
                    if matches!(&entry.running, Running::Work(run) if run.subject == *subject) {
                        info!(subject = ?subject, reason, "cancelling run");
                        entry.cancel.cancel();
                    }
                }
            }
            Action::Claim { issue } => {
                info!(issue = issue.0, "claiming");
                store.assign(*issue).await?;
            }
            Action::ClaimPull { pr } => {
                info!(pr = pr.0, "claiming the pull request");
                store.assign(*pr).await?;
            }
            Action::Relabel { issue, remove, add } => {
                info!(issue = issue.0, remove, add, "relabeling");
                store.remove_label(*issue, remove).await?;
                store.add_label(*issue, add).await?;
            }
            Action::HandBack { item, reason } => {
                info!(%item, reason, "handing back");
                run::hand_back(&self.shell, *item, reason, None).await?;
            }
            Action::MarkReady { pr } => {
                info!(pr = pr.0, "marking ready for review");
                let has_ci = snapshot.pull(*pr).is_some_and(|p| !p.checks.is_empty());
                let note = if has_ci {
                    "CI is green: ready for review."
                } else {
                    "No CI reported: ready for review."
                };
                ready_for_review(store.as_ref(), *pr, true, note).await?;
            }
            Action::Merge {
                pr,
                head,
                draft,
                method,
            } => {
                info!(pr = pr.0, method = ?method, "merging");
                if *draft {
                    store.mark_ready(*pr).await?;
                }
                if let Err(e) = store.merge(*pr, head, *method).await {
                    let reason = format!("{e:#}");
                    warn!(pr = pr.0, reason, "merge failed");
                    self.memory.failed_merges.push(FailedMerge {
                        pr: *pr,
                        reason,
                        at: self.shell.clock.now(),
                    });
                }
            }
            Action::RememberRebase(rebase) => {
                self.memory.tried_rebases.push(rebase.clone());
            }
            Action::MarkReadyForMergeState { pr } => {
                info!(pr = pr.0, "marking ready for its merge state");
                store.mark_ready(*pr).await?;
            }
            Action::HoldForReview {
                pr,
                draft,
                requires,
            } => {
                info!(
                    pr = pr.0,
                    ?requires,
                    "holding for review: GitHub blocks the merge"
                );
                ready_for_review(store.as_ref(), *pr, *draft, &hold_note(requires)).await?;
                self.memory.held.hold(*pr, requires);
                self.record_holds();
            }
            Action::CloseTracking { issue } => {
                info!(issue = issue.0, "closing tracking issue");
                store.comment(*issue, "All tickets are closed.").await?;
                store.close_issue(*issue, CloseReason::Completed).await?;
            }
            Action::DiscardTickets { tracking, tickets } => {
                warn!(
                    tracking = tracking.0,
                    "discarding an interrupted decomposition"
                );
                let note = "Cutting the spec change into tickets stopped part way; this set is discarded and the spec change is cut again.";
                for issue in tickets.iter().chain([tracking]) {
                    store.comment(*issue, note).await?;
                    store.close_issue(*issue, CloseReason::NotPlanned).await?;
                }
            }
            Action::DeleteBranch { branch } => {
                info!(
                    branch = branch.as_str(),
                    "deleting the branch of a closed ticket"
                );
                store.delete_branch(branch.as_str()).await?;
                // A refresh before the next poll decides on the kept
                // snapshot, which must not hold the branch any more.
                if let Some(latest) = &mut self.kept {
                    Arc::make_mut(&mut latest.snapshot)
                        .ticket_branches
                        .retain(|b| b.name != *branch);
                }
            }
            Action::RecordLastRuns(last_runs) => {
                self.last_runs = last_runs.clone();
                last_runs.save(&self.shell.work_dir)?;
            }
            Action::RunWorkflow(workflow) => {
                let running = begin_workflow(&self.shell, &workflow.name)?;
                let watched = running.watched();
                let cancel = CancellationToken::new();
                let shell = self.shell.clone();
                let handle = self.runs.spawn({
                    let (workflow, cancel) = (workflow.clone(), cancel.clone());
                    // A run that fails is logged; there is nothing to hand back.
                    async move {
                        let _ = run_and_log(&shell, &workflow, running, cancel).await;
                    }
                });
                self.live.insert(
                    handle.id(),
                    LiveEntry {
                        running: Running::Workflow,
                        watched,
                        cancel,
                    },
                );
            }
        }
        Ok(())
    }
}

fn done_runs(runs: &Runs) -> Vec<DoneRun> {
    let recorded = runs.read().unwrap_or_else(|e| {
        warn!(error = format!("{e:#}"), "reading the runs' records failed");
        vec![]
    });
    recorded.iter().filter_map(Recorded::done_on_pull).collect()
}

fn begin_workflow(shell: &Shell, name: &WorkflowName) -> Result<runs::Running> {
    shell
        .runs
        .begin(
            RunOf::Workflow {
                workflow: name.clone(),
            },
            shell.clock.now(),
        )
        .with_context(|| format!("recording the run of the workflow `{name}`"))
}

async fn run_and_log(
    shell: &Shell,
    workflow: &Workflow,
    running: runs::Running,
    cancel: CancellationToken,
) -> Result<()> {
    let name = &workflow.name;
    info!(workflow = %name, run = %running.id(), "workflow run started");
    match run::workflow(shell, workflow, running, cancel).await {
        Ok(WorkflowEnd::Finished(result)) => {
            info!(workflow = %name, summary = result.summary, "workflow run finished");
            Ok(())
        }
        Ok(WorkflowEnd::UsageLimit(limit)) => {
            warn!(
                workflow = %name,
                resets_at = %limit.resets_at,
                "workflow run ended for the usage limit"
            );
            Ok(())
        }
        Err(e) => {
            error!(workflow = %name, error = format!("{e:#}"), "workflow run failed");
            Err(e)
        }
    }
}

async fn ready_for_review(store: &dyn Store, pr: Number, draft: bool, note: &str) -> Result<()> {
    if draft {
        store.mark_ready(pr).await?;
    }
    store.comment(pr, note).await
}

/// Also the issues closed while open: a pull request frees an issue when it
/// ends or stops closing it.
fn closed_by_pulls_read(snapshot: &Snapshot, read: &BTreeSet<Item>) -> BTreeSet<Item> {
    let mut others = BTreeSet::new();
    for item in read {
        let Item::Pull(pr) = *item else { continue };
        let Some(pull) = snapshot.pull(pr) else {
            continue;
        };
        others.extend(
            snapshot
                .issues_closing(pr, &pull.closes)
                .into_iter()
                .map(Item::Issue)
                .filter(|other| !read.contains(other)),
        );
    }
    others
}

async fn read_all(
    snapshot: &mut Snapshot,
    shell: &Shell,
    items: impl IntoIterator<Item = Item>,
    merged_since: Timestamp,
) -> Result<BTreeSet<Item>> {
    let mut read = BTreeSet::new();
    for item in items {
        let read_with = read_into(snapshot, shell, item, merged_since).await;
        read.extend(read_with.with_context(|| format!("reading {item} again"))?);
    }
    Ok(read)
}

/// Returns the items read: besides `item`, an issue's pull requests, the
/// ended ones a snapshot may hold only for the issues it holds, and the
/// issues someone else's pull request closes or closed while open.
async fn read_into(
    snapshot: &mut Snapshot,
    shell: &Shell,
    item: Item,
    merged_since: Timestamp,
) -> Result<Vec<Item>> {
    let store = &shell.store;
    match item {
        Item::Issue(number) => read_issue_into(snapshot, shell, number).await,
        Item::Pull(number) => {
            let mut closing = BTreeSet::new();
            match store.linewright_pull(number).await? {
                Some(pull) => snapshot.put_pull(pull),
                None => {
                    snapshot.remove_pull(number);
                    let closes = store.open_pull_closes(number).await?;
                    closing.extend(snapshot.issues_closing(number, &closes));
                }
            }
            match store.merged_pull(number, merged_since).await? {
                Some(merged) => snapshot.put_merged(merged),
                None => snapshot.remove_merged(number),
            }
            let mut read = vec![item];
            for issue in closing {
                read.extend(read_issue_into(snapshot, shell, issue).await?);
            }
            Ok(read)
        }
    }
}

async fn read_issue_into(
    snapshot: &mut Snapshot,
    shell: &Shell,
    number: Number,
) -> Result<Vec<Item>> {
    let store = &shell.store;
    let mut read = vec![Item::Issue(number)];
    let left = match store.issue(number).await? {
        IssueRead::Held { issue, pulls } => {
            snapshot.relink(number, issue.open);
            snapshot.put_issue(*issue);
            for pull in pulls {
                read.push(Item::Pull(pull.number));
                snapshot.put_pull(pull);
            }
            false
        }
        IssueRead::LeftOut { open } => {
            snapshot.relink(number, open);
            true
        }
        IssueRead::Gone => {
            snapshot.unlink(number);
            true
        }
    };
    if left {
        snapshot.remove_issue(number);
        for pr in snapshot.ended_pulls_closing(number) {
            read.push(Item::Pull(pr));
            match store.linewright_pull(pr).await? {
                Some(pull) => snapshot.put_pull(pull),
                None => snapshot.remove_pull(pr),
            }
        }
    }
    Ok(read)
}

/// The items `action` changes in the store, beyond what the runs it starts
/// write.
fn written(action: &Action, snapshot: &Snapshot) -> Vec<Item> {
    match action {
        Action::Claim { issue }
        | Action::Relabel { issue, .. }
        | Action::CloseTracking { issue } => vec![Item::Issue(*issue)],
        Action::DiscardTickets { tracking, tickets } => tickets
            .iter()
            .chain([tracking])
            .map(|issue| Item::Issue(*issue))
            .collect(),
        Action::Merge { pr, .. } => snapshot
            .pull(*pr)
            .into_iter()
            .flat_map(|p| p.closes.iter().map(|issue| Item::Issue(*issue)))
            .chain([Item::Pull(*pr)])
            .collect(),
        Action::ClaimPull { pr }
        | Action::MarkReady { pr }
        | Action::MarkReadyForMergeState { pr }
        | Action::HoldForReview { pr, .. } => vec![Item::Pull(*pr)],
        Action::HandBack { item, .. } => vec![*item],
        Action::Start(_)
        | Action::Cancel { .. }
        | Action::RememberRebase(_)
        | Action::DeleteBranch { .. }
        | Action::RecordLastRuns(_)
        | Action::RunWorkflow(_) => vec![],
    }
}
