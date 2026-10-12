//! The manager: each poll takes a snapshot of the store, asks the reconcile
//! step what to do, and does it, starting runs as tasks and holding only the
//! runs in flight between polls (`docs/specs/architecture.md`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::TimeDelta;
use tokio::task::{Id, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::briefs::Briefs;
use crate::clock::Clock;
use crate::domain::{MergeMethod, Number, Snapshot, Timestamp, UpdateMethod, UpdateRule, label};
use crate::marker::{Stage, marked};
use crate::reconcile::{
    Action, FailedMerge, LiveRun, Memory, Policy, Workflows, hold_note, reconcile,
};
use crate::run::{self, Shell};
use crate::runtime::Runtime;
use crate::spec_glob::SpecGlob;
use crate::store::{CloseReason, Store};
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
        })
    }

    /// One poll: reconcile a fresh snapshot with the runs in flight and act
    /// on the decisions. Returns the decisions.
    /// Cancelled to stop: a poll then takes no more actions, though the one
    /// in progress finishes, so no action is left half done.
    pub fn stopping(&self) -> CancellationToken {
        self.stopping.clone()
    }

    pub async fn cycle(&mut self) -> Result<Vec<Action>> {
        // Reap first: a run that ends during the snapshot stays live for this
        // poll, so the snapshot cannot miss what it wrote.
        self.reap();
        let shell = &self.shell;
        let now = shell.clock.now();
        let snapshot = Arc::new(
            shell
                .store
                .snapshot(now - shell.policy.spec_lookback)
                .await
                .context("taking a snapshot")?,
        );
        let mut live: Vec<LiveRun> = vec![];
        let mut in_flight: Vec<WorkflowName> = vec![];
        for entry in self.live.values() {
            match &entry.running {
                Running::Work(run) => live.push(*run),
                Running::Workflow(name) => in_flight.push(name.clone()),
            }
        }
        let workflows = Workflows {
            defined: self.workflows().await,
            last_runs: self.last_runs.clone(),
            in_flight,
        };
        self.memory.retain_open(&snapshot);
        let actions = reconcile(
            &snapshot,
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
            if let Err(e) = self.execute(action, &snapshot, now).await {
                error!(action = ?action, error = format!("{e:#}"), "action failed");
            }
        }
        Ok(actions)
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
        self.live.remove(&id);
    }

    async fn execute(
        &mut self,
        action: &Action,
        snapshot: &Arc<Snapshot>,
        as_of: Timestamp,
    ) -> Result<()> {
        let store = &self.shell.store;
        match action {
            Action::Start(work) => {
                let cancel = CancellationToken::new();
                let handle = self.runs.spawn(run::run(
                    self.shell.clone(),
                    work.clone(),
                    snapshot.clone(),
                    as_of,
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
                info!(subject = ?subject, reason, "cancelling run");
                for entry in self.live.values() {
                    if matches!(&entry.running, Running::Work(run) if run.subject == *subject) {
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
