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
use crate::domain::{Snapshot, Timestamp, UpdateMethod, label};
use crate::marker::{Stage, marked};
use crate::reconcile::{Action, LiveRun, Policy, Subject, WorkKind, reconcile};
use crate::run::{self, Shell};
use crate::runtime::Runtime;
use crate::spec_glob::SpecGlob;
use crate::store::{CloseReason, Store};
use crate::workflow::{self, WorkflowName};

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
    pub briefs: Briefs,
    /// Where clones and session records go.
    pub work_dir: PathBuf,
}

struct LiveEntry {
    subject: Subject,
    kind: WorkKind,
    cancel: CancellationToken,
}

pub struct Factory {
    shell: Arc<Shell>,
    runs: JoinSet<()>,
    live: HashMap<Id, LiveEntry>,
    stopping: CancellationToken,
}

impl Factory {
    /// Checks the store can be worked (the token can push), creates missing
    /// labels and reads the repository's settings.
    pub async fn start(
        store: Arc<dyn Store>,
        runtime: Arc<dyn Runtime>,
        clock: Arc<dyn Clock>,
        config: FactoryConfig,
    ) -> Result<Factory> {
        let identity = store.identity().await.context("checking the token")?;
        store
            .ensure_labels(&label::ALL)
            .await
            .context("creating labels")?;
        let mut settings = store
            .settings()
            .await
            .context("reading the repository's settings")?;
        if let Some(method) = config.update_method {
            settings.update_method = method;
        }
        info!(
            machine_user = %identity.login,
            default_branch = %settings.default_branch,
            update_method = ?settings.update_method,
            "started"
        );
        let shell = Shell {
            policy: Policy {
                machine_user: identity.login.clone(),
                spec_glob: config.spec_glob,
                spec_lookback: config.spec_lookback,
                max_workers: config.max_workers,
                ci_grace: config.ci_grace,
                ci_timeout: config.ci_timeout,
                ci_attempts: config.ci_attempts,
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
        let live: Vec<LiveRun> = self
            .live
            .values()
            .map(|e| LiveRun {
                subject: e.subject,
                kind: e.kind,
            })
            .collect();
        let actions = reconcile(&snapshot, &live, now, &shell.policy);
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
        info!(workflow = %name, "workflow run started");
        let result = match run::workflow(shell, &workflow, self.stopping.clone()).await {
            Ok(result) => result,
            Err(e) => {
                error!(workflow = %name, error = format!("{e:#}"), "workflow run failed");
                return Err(e.context(format!("the run of the workflow `{name}` failed")));
            }
        };
        info!(workflow = %name, summary = result.summary, "workflow run finished");
        Ok(())
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
                        subject: work.subject(),
                        kind: work.kind(),
                        cancel,
                    },
                );
            }
            Action::Cancel { subject, reason } => {
                info!(subject = ?subject, reason, "cancelling run");
                for entry in self.live.values().filter(|e| e.subject == *subject) {
                    entry.cancel.cancel();
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
            Action::MarkReady { pr, reviewers } => {
                info!(pr = pr.0, "marking ready for review");
                let has_ci = snapshot.pull(*pr).is_some_and(|p| !p.checks.is_empty());
                store.mark_ready(*pr).await?;
                if !reviewers.is_empty() {
                    store.request_review(*pr, reviewers).await?;
                }
                let note = if has_ci {
                    "CI is green: ready for review."
                } else {
                    "No CI reported: ready for review."
                };
                store.comment(*pr, &marked(Stage::Review, note)).await?;
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
        }
        Ok(())
    }
}
