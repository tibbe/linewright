//! The manager: each poll takes a snapshot of the store, asks the reconcile
//! step what to do, and does it, starting runs as tasks. Between polls it
//! holds the runs in flight and keeps the snapshot, which a refresh reads the
//! items a delivery names into again before it reconciles
//! (`docs/specs/architecture.md`).

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

use crate::briefs::Briefs;
use crate::clock::Clock;
use crate::domain::{
    Item, MergeMethod, Number, Snapshot, Timestamp, UpdateMethod, UpdateRule, label,
};
use crate::marker::{Stage, marked};
use crate::reconcile::{
    Action, FailedMerge, LiveRun, Memory, Policy, Subject, Work, Workflows, hold_note, reconcile,
};
use crate::run::{self, Shell};
use crate::runtime::Runtime;
use crate::spec_glob::SpecGlob;
use crate::store::{CloseReason, IssueRead, Store};
use crate::webhook::Pending;
use crate::workflow::{self, LastRuns, Workflow, WorkflowName};

/// The settings a factory runs with.
#[derive(Clone, Debug)]
pub struct FactoryConfig {
    pub spec_glob: SpecGlob,
    pub spec_lookback: TimeDelta,
    pub max_workers: usize,
    pub ci_grace: TimeDelta,
    pub ci_timeout: TimeDelta,
    pub ci_attempts: u32,
    pub result_retries: u32,
    pub session_timeout: std::time::Duration,
    /// `None` follows the repository's settings.
    pub update_method: Option<UpdateMethod>,
    /// Merge green pull requests instead of leaving them for review.
    pub auto_merge: bool,
    /// `None` follows the repository's settings.
    pub merge_method: Option<MergeMethod>,
    /// Start work only on issues with this label, which is never created.
    pub require_label: Option<String>,
    pub briefs: Briefs,
    /// Where clones, session records and the workflows' last runs go.
    pub work_dir: PathBuf,
    /// Whether each poll also starts the workflows that are due. Replay does
    /// not: workflows are not captured or replayed.
    pub schedule_workflows: bool,
}

/// What a run in flight works on.
enum Running {
    Work(LiveRun),
    Workflow(WorkflowName),
}

struct LiveEntry {
    running: Running,
    cancel: CancellationToken,
}

pub struct Factory {
    shell: Arc<Shell>,
    runs: JoinSet<()>,
    live: HashMap<Id, LiveEntry>,
    stopping: CancellationToken,
    schedule_workflows: bool,
    last_runs: LastRuns,
    /// What it remembers between polls, of pull requests still open.
    memory: Memory,
    /// The last poll's snapshot, with what refreshes read into it since:
    /// `None` before the first poll.
    kept: Option<Kept>,
    /// The cuts into tickets that ended since the kept snapshot was taken.
    ended: Vec<LiveRun>,
    /// The items Linewright's actions and runs changed since the kept
    /// snapshot last read them.
    written: BTreeSet<Item>,
}

/// The snapshot kept between polls, with what refreshes read into it.
#[derive(Clone)]
struct Kept {
    snapshot: Arc<Snapshot>,
    /// When the poll took the snapshot.
    taken_at: Timestamp,
    /// When refreshes read the items they read into it.
    read_at: HashMap<Item, Timestamp>,
}

impl Kept {
    /// When what `work` starts from was read. The run takes feedback since
    /// as having arrived after.
    fn as_of(&self, work: &Work) -> Timestamp {
        let read_at = self.read_at.get(&work.starts_from());
        read_at.copied().unwrap_or(self.taken_at)
    }
}

impl Factory {
    /// Checks the store can be worked (the token can push and, with
    /// auto-merge, the repository allows the merge method and `update-branch`
    /// keeps branches mergeable with it), reads the repository's settings
    /// and creates missing labels.
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
            machine_user = %identity.login,
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
        let shell = Shell {
            policy: Policy {
                machine_user: identity.login.clone(),
                spec_glob: config.spec_glob,
                spec_lookback: config.spec_lookback,
                max_workers: config.max_workers,
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
            work_dir: config.work_dir,
        };
        Ok(Factory {
            shell: Arc::new(shell),
            runs: JoinSet::new(),
            live: HashMap::new(),
            stopping: CancellationToken::new(),
            schedule_workflows: config.schedule_workflows,
            last_runs,
            memory: Memory::default(),
            kept: None,
            ended: vec![],
            written: BTreeSet::new(),
        })
    }

    /// Cancelled to stop: a poll or refresh then takes no more actions,
    /// though the one in progress finishes, so no action is left half done.
    pub fn stopping(&self) -> CancellationToken {
        self.stopping.clone()
    }

    /// One poll: reconcile a fresh snapshot with the runs in flight and act
    /// on the decisions, and keep the snapshot. Returns the decisions.
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

    /// One refresh: read `items` again, with those Linewright changed since
    /// they were last read, into the kept snapshot, reconcile it with the
    /// runs in flight and act on the decisions, without taking a snapshot.
    /// Workflows wait for the next poll. Before the first poll there is
    /// nothing kept, and it polls instead. Returns the decisions.
    pub async fn refresh(&mut self, items: impl IntoIterator<Item = Item>) -> Result<Vec<Action>> {
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
        let items: BTreeSet<Item> = items
            .into_iter()
            .chain(self.written.iter().copied())
            .collect();
        for item in items {
            let read = read_into(snapshot, shell, item, merged_since)
                .await
                .with_context(|| format!("reading {item} again"))?;
            kept.read_at
                .extend(read.into_iter().map(|read| (read, now)));
        }
        self.written.clear();
        Ok(self.reconcile(kept, vec![], now).await)
    }

    /// Polls every `poll_interval` and, between polls, refreshes the items
    /// deliveries add to `pending`, until stopping. A refresh never races a
    /// poll: one still running when the poll falls due finishes first. A
    /// failed poll or refresh is logged and left for the next poll to catch
    /// up on (`docs/specs/product.md`, "Webhooks").
    pub async fn serve(&mut self, poll_interval: Duration, pending: &Pending) {
        let stopping = self.stopping.clone();
        while !stopping.is_cancelled() {
            // The poll reads what deliveries named before it as well.
            pending.take();
            if let Err(e) = self.cycle().await {
                error!(error = format!("{e:#}"), "poll failed");
            }
            let next_poll = Instant::now() + poll_interval;
            loop {
                tokio::select! {
                    () = tokio::time::sleep_until(next_poll) => break,
                    () = stopping.cancelled() => break,
                    items = pending.next() => {
                        if let Err(e) = self.refresh(items).await {
                            error!(
                                error = format!("{e:#}"),
                                "acting on deliveries failed; the next poll catches up"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Keeps `kept`, reconciles its snapshot with the runs in flight and the
    /// workflows `defined`, and acts on the decisions. Returns the decisions.
    async fn reconcile(
        &mut self,
        kept: Kept,
        defined: Vec<Workflow>,
        now: Timestamp,
    ) -> Vec<Action> {
        let kept = self.kept.insert(kept).clone();
        let snapshot = &kept.snapshot;
        // A cut that ended since the snapshot was taken counts as in flight:
        // the snapshot may miss what it wrote.
        let mut live: Vec<LiveRun> = self.ended.clone();
        let mut in_flight: Vec<WorkflowName> = vec![];
        for entry in self.live.values() {
            match &entry.running {
                Running::Work(run) => live.push(*run),
                Running::Workflow(name) => in_flight.push(name.clone()),
            }
        }
        let workflows = Workflows {
            defined,
            last_runs: self.last_runs.clone(),
            in_flight,
        };
        self.memory.retain_open(snapshot);
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

    /// Runs the workflow `name` on the default branch once, now, and files
    /// the issues its result returns. Fails when there is no such workflow,
    /// its file does not parse or the run fails.
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
        run_and_log(shell, &workflow, self.stopping.clone())
            .await
            .with_context(|| format!("the run of the workflow `{name}` failed"))
    }

    /// The workflows on the default branch whose files parse, when polls
    /// schedule them. A file that does not parse is logged and skipped.
    async fn workflows(&self) -> Vec<Workflow> {
        if !self.schedule_workflows {
            return vec![];
        }
        let branch = &self.shell.settings.default_branch;
        match workflow::read(self.shell.store.as_ref(), branch).await {
            Ok(files) => {
                for file in files.unparsed {
                    warn!(
                        path = file.path,
                        error = file.reason,
                        "skipping a workflow file"
                    );
                }
                files.parsed
            }
            Err(e) => {
                error!(error = format!("{e:#}"), "listing the workflows failed");
                vec![]
            }
        }
    }

    pub fn is_idle(&self) -> bool {
        self.runs.is_empty()
    }

    /// Waits for one run in flight to end.
    pub async fn next_finished(&mut self) {
        if let Some(result) = self.runs.join_next_with_id().await {
            self.finished(result);
        }
    }

    /// Waits for every run in flight to end.
    pub async fn settle(&mut self) {
        while !self.is_idle() {
            self.next_finished().await;
        }
    }

    /// Cancels every run in flight and waits for them to stop.
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
            ..
        }) = self.live.remove(&id)
        else {
            return;
        };
        match run.subject {
            // Reading the issue again reads what the run wrote: the issue,
            // and the pull requests that close it.
            Subject::Issue(issue) => {
                self.written.insert(Item::Issue(issue));
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
                let cancel = CancellationToken::new();
                let handle = self.runs.spawn(run::run(
                    self.shell.clone(),
                    work.clone(),
                    snapshot.clone(),
                    kept.as_of(work),
                    cancel.clone(),
                ));
                info!(subject = ?work.subject(), kind = ?work.kind(), "run started");
                self.live.insert(
                    handle.id(),
                    LiveEntry {
                        running: Running::Work(LiveRun {
                            subject: work.subject(),
                            kind: work.kind(),
                        }),
                        cancel,
                    },
                );
            }
            Action::Cancel { subject, reason } => {
                // A run that ended since the snapshot is cancelled too, and
                // there is nothing left to stop.
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
            Action::Relabel { issue, remove, add } => {
                info!(issue = issue.0, remove, add, "relabeling");
                store.remove_label(*issue, remove).await?;
                store.add_label(*issue, add).await?;
            }
            Action::HandBack {
                issue,
                stage,
                reason,
            } => {
                info!(issue = issue.0, stage = %stage, reason, "handing back");
                run::hand_back(&self.shell, *issue, *stage, reason, None).await?;
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
                    self.memory
                        .failed_merges
                        .push(FailedMerge { pr: *pr, reason });
                }
            }
            Action::RememberRebase(rebase) => {
                self.memory.tried_rebases.push(rebase.clone());
            }
            Action::ForgetFailedMerges { pr } => {
                self.memory.failed_merges.retain(|f| f.pr != *pr);
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
            }
            Action::CloseTracking { issue } => {
                info!(issue = issue.0, "closing tracking issue");
                store
                    .comment(*issue, &marked(Stage::Tickets, "All tickets are closed."))
                    .await?;
                store.close_issue(*issue, CloseReason::Completed).await?;
            }
            Action::DiscardTickets { tracking, tickets } => {
                warn!(
                    tracking = tracking.0,
                    "discarding an interrupted decomposition"
                );
                let note = marked(
                    Stage::Tickets,
                    "Cutting the spec change into tickets stopped part way; this set is discarded and the spec change is cut again.",
                );
                for issue in tickets.iter().chain([tracking]) {
                    store.comment(*issue, &note).await?;
                    store.close_issue(*issue, CloseReason::NotPlanned).await?;
                }
            }
            Action::RecordLastRuns(last_runs) => {
                self.last_runs = last_runs.clone();
                last_runs.save(&self.shell.work_dir)?;
            }
            Action::RunWorkflow(workflow) => {
                let cancel = CancellationToken::new();
                let shell = self.shell.clone();
                let handle = self.runs.spawn({
                    let (workflow, cancel) = (workflow.clone(), cancel.clone());
                    // A run that fails is logged; there is nothing to hand back.
                    async move {
                        let _ = run_and_log(&shell, &workflow, cancel).await;
                    }
                });
                self.live.insert(
                    handle.id(),
                    LiveEntry {
                        running: Running::Workflow(workflow.name.clone()),
                        cancel,
                    },
                );
            }
        }
        Ok(())
    }
}

/// Runs `workflow` once, files the issues its result returns and logs how
/// it went: by hand or on its schedule.
async fn run_and_log(shell: &Shell, workflow: &Workflow, cancel: CancellationToken) -> Result<()> {
    let name = &workflow.name;
    info!(workflow = %name, "workflow run started");
    match run::workflow(shell, workflow, cancel).await {
        Ok(result) => {
            info!(workflow = %name, summary = result.summary, "workflow run finished");
            Ok(())
        }
        Err(e) => {
            error!(workflow = %name, error = format!("{e:#}"), "workflow run failed");
            Err(e)
        }
    }
}

/// Hands the pull request to people: marks it ready for review if `draft`
/// and comments `note`.
async fn ready_for_review(store: &dyn Store, pr: Number, draft: bool, note: &str) -> Result<()> {
    if draft {
        store.mark_ready(pr).await?;
    }
    store.comment(pr, &marked(Stage::Review, note)).await
}

/// Reads `item` again into `snapshot`, as a snapshot taken now holds it,
/// with pull requests merged since `merged_since`. Returns the items read:
/// an issue's pull requests are read with it, and so are the ended ones a
/// snapshot may hold only for the issues it holds.
async fn read_into(
    snapshot: &mut Snapshot,
    shell: &Shell,
    item: Item,
    merged_since: Timestamp,
) -> Result<Vec<Item>> {
    let store = &shell.store;
    let mut read = vec![item];
    match item {
        Item::Issue(number) => {
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
                    match store.machine_pull(pr).await? {
                        Some(pull) => snapshot.put_pull(pull),
                        None => snapshot.remove_pull(pr),
                    }
                }
            }
        }
        Item::Pull(number) => {
            match store.machine_pull(number).await? {
                Some(pull) => snapshot.put_pull(pull),
                None => snapshot.remove_pull(number),
            }
            match store.merged_pull(number, merged_since).await? {
                Some(merged) => snapshot.put_merged(merged),
                None => snapshot.remove_merged(number),
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
        | Action::HandBack { issue, .. }
        | Action::CloseTracking { issue } => vec![Item::Issue(*issue)],
        Action::DiscardTickets { tracking, tickets } => tickets
            .iter()
            .chain([tracking])
            .map(|issue| Item::Issue(*issue))
            .collect(),
        // A merge closes the issues the pull request closes.
        Action::Merge { pr, .. } => snapshot
            .pull(*pr)
            .into_iter()
            .flat_map(|p| p.closes.iter().map(|issue| Item::Issue(*issue)))
            .chain([Item::Pull(*pr)])
            .collect(),
        Action::MarkReady { pr }
        | Action::MarkReadyForMergeState { pr }
        | Action::HoldForReview { pr, .. } => vec![Item::Pull(*pr)],
        Action::Start(_)
        | Action::Cancel { .. }
        | Action::RememberRebase(_)
        | Action::ForgetFailedMerges { .. }
        | Action::RecordLastRuns(_)
        | Action::RunWorkflow(_) => vec![],
    }
}
