use std::collections::{BTreeSet, HashMap};

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::TimeDelta;
use tokio::task::{Id, JoinSet};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::api::{Publish, Served};
use crate::apply;
use crate::automation::{self, Automation, AutomationFiles, AutomationName, LastScheduledRuns};
use crate::briefs::OperatorPrompts;
use crate::clock::Clock;
use crate::clones::Clones;
use crate::domain::{
    IssueCloseReason, Item, MergeMethod, Number, PullHeldRead, PullRead, Snapshot, SpecChangeIssue,
    Timestamp, Windows, label,
};
use crate::ending::Ends;
use crate::environment::{self, BuildEnded, Environment, SetupRead};
use crate::harness::{Harness, UsageLimit};
use crate::pull_records::PullRecords;
use crate::reconcile::{
    Action, Automations, Discard, Drain, EndedRun, LiveRun, Memory, ParentNote, Policy, Subject,
    Work, held_back_by_drain, reconcile, requirements_note,
};
use crate::repo_files::{self, RepoLayer, Unusable, holding};
use crate::run::{self, AutomationEnd, Cancellation, Shell};
use crate::runs::{self, Begin, Recorded, RunOf, Runs, StartedBy};
use crate::secrets::Secrets;
use crate::spec_glob::SpecGlob;
use crate::status::{FailedPolls, Poll, Polling, Polls, View};
use crate::store::Store;
use crate::store::github::SESSION_TOKEN_VARIABLE;
use crate::webhook::{self, Pending};
use crate::winding_up::WindingUp;
use crate::work_settings::{Environments, RunKind, WorkSettings};

#[derive(Clone, Debug)]
pub struct FactoryConfig {
    pub spec_glob: SpecGlob,
    pub spec_lookback: TimeDelta,
    pub done_window: TimeDelta,
    pub max_runs: usize,
    pub ci_grace: TimeDelta,
    pub ci_timeout: TimeDelta,
    pub ci_attempts: u32,
    pub result_retries: u32,
    /// The `bash` that runs the maintenance command.
    pub bash: PathBuf,
    /// The operator's settings for sessions.
    pub work: WorkSettings,
    /// What sessions are given of Linewright's environment.
    pub environments: Environments,
    /// `None` follows the repository's settings.
    pub merge_method: Option<MergeMethod>,
    /// Merge green pull requests instead of leaving them for review.
    pub auto_merge: bool,
    pub require_label: Option<String>,
    /// The operator's layer of the prompts.
    pub prompts: OperatorPrompts,
    pub work_dir: PathBuf,
    pub waits_out_usage_limit: bool,
    pub session_token: SessionToken,
    /// Linewright's secrets, refused in the commits a session's result
    /// would push, and redacted from what a session's work prints that
    /// reaches Linewright other than through the harness, such as git's
    /// output from hooks the session left in its clone.
    pub secrets: Secrets,
    /// What drains and stops the factory.
    pub ends: Ends,
    /// What Linewright waits for before it exits, such as the removal of
    /// the runs' clones.
    pub winding_up: WindingUp,
}

/// The GitHub token agent sessions are given.
#[derive(Clone)]
pub enum SessionToken {
    /// Linewright's own, which can write to GitHub.
    Linewright(String),
    /// The operator's token for sessions, from [`SESSION_TOKEN_VARIABLE`].
    Separate(String),
    /// None, as in `serve-fake`, whose sessions do not read GitHub.
    Withheld,
}

impl SessionToken {
    /// `separate` is [`SESSION_TOKEN_VARIABLE`]'s value; empty, it is unset.
    pub fn new(linewright: String, separate: Option<String>) -> SessionToken {
        match separate {
            Some(token) if !token.is_empty() => SessionToken::Separate(token),
            _ => SessionToken::Linewright(linewright),
        }
    }

    pub fn given(&self) -> Option<&str> {
        match self {
            SessionToken::Linewright(token) | SessionToken::Separate(token) => Some(token),
            SessionToken::Withheld => None,
        }
    }
}

/// Without the token, so that no log shows it.
impl std::fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SessionToken::Linewright(_) => "Linewright",
            SessionToken::Separate(_) => "Separate",
            SessionToken::Withheld => "Withheld",
        })
    }
}

enum Running {
    Work(LiveRun),
    Automation,
}

struct LiveEntry {
    running: Running,
    in_flight: runs::InFlight,
    cancel: Cancellation,
}

impl LiveEntry {
    /// Cancels the run, unless it is applying its result.
    fn cancel(&self) {
        if !self.cancel.cancel() {
            info!(
                run = %self.in_flight.id,
                "the run is applying its result; left to finish"
            );
        }
    }
}

/// Why [`Factory::serve`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServeEnd {
    /// A drain began and no run is in flight any more.
    Drained,
    /// The runs in flight, if any, are left for [`Factory::stop`] to cancel.
    Stop,
}

pub struct Factory {
    shell: Arc<Shell>,
    runs: JoinSet<()>,
    refusals: tokio::sync::mpsc::UnboundedReceiver<UsageLimit>,
    live: HashMap<Id, LiveEntry>,
    ends: Ends,
    winding_up: WindingUp,
    last_scheduled_runs: LastScheduledRuns,
    automation_files: AutomationFiles,
    memory: Memory,
    pull_records_file: PullRecordsFile,
    environment: Environment,
    kept: Option<Kept>,
    /// The items Linewright's actions and runs changed since the kept
    /// snapshot last read them.
    written: BTreeSet<Item>,
    /// Whether the first poll since startup has labeled the old tracking
    /// issues ([`Snapshot::unlabeled_tracking_issues`]).
    labeled_old_tracking_issues: bool,
}

enum PullRecordsFile {
    /// There is no record yet: the next poll takes the comments naming what
    /// GitHub requires from what its snapshot shows, so that a Linewright
    /// that kept none does not post them again.
    Missing,
    /// `saved` is `None` until first written, and a save that failed leaves
    /// it behind the records remembered. The pull requests in `settling` were
    /// ready when the record began, before GitHub had worked out their merge
    /// state: they are taken from the snapshot too, once it has.
    Kept {
        saved: Option<PullRecords>,
        settling: BTreeSet<Number>,
    },
}

#[derive(Clone)]
struct Kept {
    snapshot: Arc<Snapshot>,
    taken_at: Timestamp,
    read_at: HashMap<Item, Timestamp>,
    /// Linewright's files in the repository, as the last reconcile read
    /// them.
    repo: Arc<RepoLayer>,
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
        harness: Arc<dyn Harness>,
        clock: Arc<dyn Clock>,
        config: FactoryConfig,
    ) -> Result<Factory> {
        let identity = store.identity().await.context("checking the token")?;
        match &config.session_token {
            SessionToken::Separate(token) => store
                .check_reads(token)
                .await
                .with_context(|| format!("checking {SESSION_TOKEN_VARIABLE}"))?,
            SessionToken::Linewright(_) => warn!(
                "sessions can write to GitHub: they are given Linewright's token; set \
                 {SESSION_TOKEN_VARIABLE} to a token that only reads to keep them from it"
            ),
            SessionToken::Withheld => {}
        }
        let settings = store
            .settings()
            .await
            .context("reading the repository's settings")?
            .configured(config.merge_method)?;
        let labels = store.labels().await.context("listing labels")?;
        for change in label::label_changes(&labels, &label::ALL) {
            match change {
                label::LabelChange::Create(label) => store
                    .create_label(&label)
                    .await
                    .with_context(|| format!("creating label {}", label.name))?,
                label::LabelChange::Describe { name, description } => store
                    .describe_label(&name, description)
                    .await
                    .with_context(|| format!("describing label {name}"))?,
            }
        }
        info!(
            account = %identity.login,
            default_branch = %settings.default_branch,
            auto_merge = config.auto_merge,
            merge_method = %settings.merge_method,
            "started"
        );
        let last_scheduled_runs = LastScheduledRuns::load(&config.work_dir);
        let (report_refusal, refusals) = tokio::sync::mpsc::unbounded_channel();
        let runs = Runs::new(&config.work_dir);
        let pulls = PullRecords::load(&config.work_dir);
        let pull_records_file = match &pulls {
            Some(saved) => PullRecordsFile::Kept {
                saved: Some(saved.clone()),
                settling: BTreeSet::new(),
            },
            None => PullRecordsFile::Missing,
        };
        let environment = Environment::open(
            environment::dir(&config.work_dir),
            environment::Builder {
                store: store.clone(),
                harness: harness.clone(),
                bash: config.bash.clone(),
                clock: clock.clone(),
                redaction: config.secrets.redaction().clone(),
                given: config.environments.pipeline.clone(),
                default_branch: settings.default_branch.clone(),
                winding_up: config.winding_up.clone(),
            },
        )
        .await?;
        let clones = Clones::new(&config.work_dir, config.winding_up.clone());
        clones.sweep();
        let shell = Shell {
            policy: Policy {
                linewright: identity.login.clone(),
                spec_glob: config.spec_glob,
                spec_lookback: config.spec_lookback,
                done_window: config.done_window,
                max_runs: config.max_runs,
                ci_grace: config.ci_grace,
                ci_timeout: config.ci_timeout,
                ci_attempts: config.ci_attempts,
                merge_method: settings.merge_method,
                auto_merge: config.auto_merge,
                require_label: config.require_label,
            },
            store,
            harness,
            bash: config.bash,
            clock,
            work: config.work,
            environments: config.environments,
            operator_prompts: config.prompts,
            identity,
            settings,
            result_retries: config.result_retries,
            runs: runs.clone(),
            work_dir: config.work_dir,
            waits_out_usage_limit: config.waits_out_usage_limit,
            secrets: config.secrets,
            report_refusal,
            environment: environment.source(),
            clones,
        };
        Ok(Factory {
            shell: Arc::new(shell),
            runs: JoinSet::new(),
            refusals,
            live: HashMap::new(),
            ends: config.ends,
            winding_up: config.winding_up,
            last_scheduled_runs,
            automation_files: AutomationFiles::default(),
            memory: Memory {
                ended_runs: ended_runs(&runs),
                pulls: pulls.unwrap_or_default(),
                ..Memory::default()
            },
            pull_records_file,
            environment,
            kept: None,
            written: BTreeSet::new(),
            labeled_old_tracking_issues: false,
        })
    }

    /// Cancelling it lets the action in progress finish, so no action is left
    /// half done.
    pub fn stopping(&self) -> CancellationToken {
        self.ends.stopping()
    }

    pub fn ends(&self) -> Ends {
        self.ends.clone()
    }

    /// What the factory's work leaves to wait for before Linewright exits.
    pub fn winding_up(&self) -> WindingUp {
        self.winding_up.clone()
    }

    /// Cancelling it begins a drain: no item is claimed and no run starts
    /// from then on, and [`Factory::serve`] returns once no run is in flight.
    pub fn draining(&self) -> CancellationToken {
        self.ends.draining()
    }

    pub async fn cycle(&mut self) -> Result<Vec<Action>> {
        self.reap_before_reading();
        let shell = &self.shell;
        let now = shell.clock.now();
        let windows = shell.policy.windows(now);
        let mut snapshot = shell
            .store
            .snapshot(windows)
            .await
            .context("taking a snapshot")?;
        if !self.labeled_old_tracking_issues {
            label_old_tracking_issues(&mut snapshot, shell, windows).await?;
            self.labeled_old_tracking_issues = true;
        }
        let repo = self.read_repo_files().await?;
        self.poll_environment(&repo);
        self.written.clear();
        let kept = Kept {
            snapshot: Arc::new(snapshot),
            taken_at: now,
            read_at: HashMap::new(),
            repo,
        };
        let defined = self.automations().await;
        Ok(self.reconcile(kept, defined, now).await)
    }

    /// The items Linewright changed are read before `subjects` are resolved:
    /// what a commit or a push names is found in the kept snapshot only once
    /// it holds them, such as a pull request a run opened or pushed to.
    pub async fn refresh(
        &mut self,
        subjects: impl IntoIterator<Item: Into<webhook::Subject>>,
    ) -> Result<Vec<Action>> {
        self.read_and_reconcile(subjects, false).await
    }

    /// [`Factory::refresh`]'s, but reconciling `always`, even when it reads
    /// nothing: once a build of the environment ends, the runs it held
    /// start.
    async fn read_and_reconcile(
        &mut self,
        subjects: impl IntoIterator<Item: Into<webhook::Subject>>,
        always: bool,
    ) -> Result<Vec<Action>> {
        let Some(mut kept) = self.kept.clone() else {
            return self.cycle().await;
        };
        self.reap_before_reading();
        let shell = &self.shell;
        let now = shell.clock.now();
        let windows = shell.policy.windows(now);
        let snapshot = Arc::make_mut(&mut kept.snapshot);
        let mut read = read_all(snapshot, shell, self.written.clone(), windows).await?;
        let named: BTreeSet<Item> = subjects
            .into_iter()
            .flat_map(|subject| subject.into().items(snapshot))
            .filter(|item| !read.contains(item))
            .collect();
        if read.is_empty() && named.is_empty() && !always {
            return Ok(vec![]);
        }
        read.extend(read_all(snapshot, shell, named, windows).await?);
        let others = snapshot.closed_by_pulls_read(&read);
        read.extend(read_all(snapshot, shell, others, windows).await?);
        let linewright = &shell.policy.linewright;
        let tickets = snapshot.unheld_closed_tickets(linewright);
        read.extend(
            read_all(
                snapshot,
                shell,
                tickets.into_iter().map(Item::Issue),
                windows,
            )
            .await?,
        );
        snapshot.drop_unkept_closed(windows, linewright);
        kept.read_at
            .extend(read.into_iter().map(|item| (item, now)));
        kept.repo = self.read_repo_files().await?;
        self.poll_environment(&kept.repo);
        self.written.clear();
        Ok(self.reconcile(kept, vec![], now).await)
    }

    /// A refresh never races a poll: one still running when the poll falls
    /// due finishes first. Only polls count among the polls failed: a
    /// refresh that fails leaves the next poll to catch up.
    pub async fn serve(
        &mut self,
        poll_interval: Duration,
        pending: &Pending,
        publish: &Publish,
    ) -> ServeEnd {
        let ends = self.ends.clone();
        let clock = self.shell.clock.clone();
        let stopping = ends.stopping();
        let draining = ends.draining();
        let mut failed = None;
        let mut asked = publish.environment.asked.lock().await;
        while !stopping.is_cancelled() {
            pending.take();
            let poll = match self.cycle().await {
                Ok(_) => Poll::Succeeded,
                Err(e) => {
                    let error = self.shell.secrets.redaction().apply(&format!("{e:#}"));
                    error!(error, "poll failed");
                    Poll::Failed {
                        at: self.shell.clock.now(),
                        error,
                    }
                }
            };
            failed = FailedPolls::after(failed, poll);
            let next_poll = Instant::now() + poll_interval;
            let next_poll_at = self.shell.clock.now() + poll_interval;
            publish.polling.send_replace(Some(Polling {
                next: next_poll_at,
                failed: failed.clone(),
            }));
            publish.served.send_replace(self.served(next_poll_at));
            publish.environment.tally(self.environment.tally());
            loop {
                if self.memory.drained(self.runs.len()) {
                    info!("drained: no run is in flight");
                    self.environment.exit().await;
                    return ServeEnd::Drained;
                }
                tokio::select! {
                    () = tokio::time::sleep_until(next_poll) => break,
                    () = stopping.cancelled() => break,
                    () = draining.cancelled(), if self.memory.drain.is_none() => {
                        self.note_drain();
                    }
                    () = ends.deadline_passed(clock.as_ref()) => {}
                    subjects = pending.next() => {
                        if let Err(e) = self.refresh(subjects).await {
                            error!(
                                error = format!("{e:#}"),
                                "acting on deliveries failed; the next poll catches up"
                            );
                        }
                    }
                    Some(result) = self.runs.join_next_with_id(), if !self.runs.is_empty() => {
                        self.finished(result);
                    }
                    Some(limit) = self.refusals.recv() => self.refused(limit),
                    Some(reply) = asked.recv() => {
                        let _ = reply.send(self.environment.rebuild());
                    }
                    end = self.environment.next_build_end(), if self.environment.is_building() => {
                        self.environment.ended(end);
                        let subjects: [webhook::Subject; 0] = [];
                        if let Err(e) = self.read_and_reconcile(subjects, true).await {
                            error!(
                                error = format!("{e:#}"),
                                "acting on the environment's build failed; the next poll catches up"
                            );
                        }
                    }
                }
                publish.served.send_replace(self.served(next_poll_at));
                publish.environment.tally(self.environment.tally());
            }
        }
        self.environment.exit().await;
        ServeEnd::Stop
    }

    /// Takes a drain asked for into memory, once.
    fn note_drain(&mut self) {
        if !self.ends.draining().is_cancelled() || self.memory.drain.is_some() {
            return;
        }
        self.memory.drain = Some(Drain {
            since: self.shell.clock.now(),
        });
        self.environment.drain();
        let runs: Vec<String> = self
            .live
            .values()
            .map(|entry| entry.in_flight.id.to_string())
            .collect();
        info!(
            runs = runs.join(" "),
            "drain begins: no item is claimed and no run starts; waiting for the runs in flight"
        );
    }

    fn served(&self, next_poll: Timestamp) -> Option<Served> {
        Some(Served::new(
            self.view(next_poll)?,
            self.shell.store.clone(),
            self.shell.secrets.redaction().clone(),
        ))
    }

    fn view(&self, next_poll: Timestamp) -> Option<View> {
        let kept = self.kept.as_ref()?;
        let live = self.live_runs();
        let in_flight = self
            .live
            .values()
            .map(|entry| entry.in_flight.clone())
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
            automations: self.automation_files.clone(),
            last_scheduled_runs: self.last_scheduled_runs.clone(),
            unusable: kept.repo.unusable.clone(),
        })
    }

    fn live_runs(&self) -> Vec<LiveRun> {
        self.live
            .values()
            .filter_map(|entry| match entry.running {
                Running::Work(run) => Some(run),
                Running::Automation => None,
            })
            .collect()
    }

    fn automations_in_flight(&self) -> Vec<AutomationName> {
        self.live
            .values()
            .filter_map(|entry| match &entry.in_flight.id.of {
                RunOf::Automation { automation } => Some(automation.clone()),
                RunOf::Work { .. } => None,
            })
            .collect()
    }

    async fn reconcile(
        &mut self,
        kept: Kept,
        defined: Vec<Automation>,
        now: Timestamp,
    ) -> Vec<Action> {
        let kept = self.kept.insert(kept).clone();
        let snapshot = &kept.snapshot;
        if let Some(resets_at) = self.memory.lift_usage_limit(now) {
            info!(%resets_at, "usage limit reset; sessions start again");
        }
        self.note_drain();
        self.memory.awaits_environment = !self.environment.ready();
        let live = self.live_runs();
        let automations = Automations {
            defined,
            last_scheduled_runs: self.last_scheduled_runs.clone(),
            in_flight: self.automations_in_flight(),
        };
        self.take_shown_comments(snapshot, now);
        self.memory.retain_open(snapshot);
        self.record_pulls();
        let mut actions = reconcile(
            snapshot,
            &live,
            &self.memory,
            &automations,
            now,
            &self.shell.policy,
            &kept.repo.unusable,
        );
        let mut planned_draining = self.memory.drain.is_some();
        let mut next = 0;
        while let Some(action) = actions.get(next).cloned() {
            if self.ends.stopping().is_cancelled() {
                break;
            }
            self.note_drain();
            let drain_begun_since_the_plan = !planned_draining && self.memory.drain.is_some();
            if drain_begun_since_the_plan {
                let rest = actions.split_off(next);
                actions.extend(held_back_by_drain(rest, &self.last_scheduled_runs));
                planned_draining = true;
                continue;
            }
            self.written.extend(written(&action, snapshot));
            if let Err(e) = self.execute(&action, &kept).await {
                error!(action = ?action, error = format!("{e:#}"), "action failed");
            }
            next += 1;
        }
        actions
    }

    /// The `setup` table as `repo` gives it.
    fn setup_read(&self, repo: &RepoLayer) -> SetupRead {
        let unusable = repo
            .unusable
            .iter()
            .any(|unusable| unusable.file.path == repo_files::CONFIG);
        if unusable {
            SetupRead::Unusable
        } else {
            SetupRead::Given(self.shell.work.setup(&repo.settings))
        }
    }

    /// Gives the environment the `setup` table `repo` holds.
    fn poll_environment(&mut self, repo: &RepoLayer) {
        let read = self.setup_read(repo);
        self.environment.poll(read, self.shell.clock.now());
    }

    /// Makes the environment ready for a run without `serve`
    /// (`docs/specs/sessions.md`, Building, Without `serve`): it is used when
    /// no trigger applies to it, and otherwise built first. `Err` names the
    /// build that failed when no environment can be had; the runs that
    /// would start a session are then held.
    pub async fn prepare_environment(&mut self) -> Result<()> {
        let repo = self.read_repo_files().await?;
        let read = self.setup_read(&repo);
        self.environment.prepare(read, self.shell.clock.now()).await
    }

    /// Builds the environment now and waits for the build, as
    /// `lw environment rebuild` does without `serve`: `None` without a setup
    /// command.
    pub async fn rebuild_environment(&mut self) -> Result<Option<BuildEnded>> {
        let repo = self.read_repo_files().await?;
        let read = self.setup_read(&repo);
        self.environment.rebuild_now(read).await
    }

    /// Linewright's files on the default branch as they are now, each that
    /// cannot be used logged.
    async fn read_repo_files(&self) -> Result<Arc<RepoLayer>> {
        let shell = &self.shell;
        let repo = RepoLayer::read(
            shell.store.as_ref(),
            &shell.settings.default_branch,
            &shell.operator_prompts,
        )
        .await
        .context("reading Linewright's files in the repository")?;
        for Unusable { file, .. } in &repo.unusable {
            warn!(
                path = file.path,
                error = file.error,
                "a file of the repository's cannot be used: the runs it holds wait"
            );
        }
        Ok(Arc::new(repo))
    }

    /// Runs the automation `name` once, now, from the environment, which is
    /// prepared first.
    pub async fn run_automation(&mut self, name: &AutomationName) -> Result<()> {
        self.prepare_environment()
            .await
            .with_context(|| format!("the automation `{name}` cannot run"))?;
        let shell = &self.shell;
        let branch = &shell.settings.default_branch;
        let path = name.path();
        let text = shell
            .store
            .read_file(branch, &path)
            .await
            .with_context(|| format!("reading {path}"))?
            .with_context(|| {
                format!("there is no automation `{name}`: `{branch}` has no {path}")
            })?;
        let automation =
            automation::parse(name, &text).with_context(|| format!("{path} does not parse"))?;
        let repo = self.read_repo_files().await?;
        if let Some(file) = holding(&repo.unusable, &RunKind::Automation(&automation)).first() {
            bail!(
                "the automation `{name}` cannot run until the default branch fixes {}, which \
                 Linewright cannot use: {}",
                file.path,
                file.error
            );
        }
        let cancel = Cancellation::default();
        let run = run_and_log(
            shell,
            &repo,
            &automation,
            begin_automation(shell, name, StartedBy::Hand)?,
            cancel.clone(),
        );
        let stopping = self.ends.stopping();
        let stop = async {
            stopping.cancelled().await;
            cancel.cancel();
            std::future::pending().await
        };
        tokio::select! {
            ended = run => ended,
            never = stop => never,
        }
        .with_context(|| format!("the run of the automation `{name}` failed"))
    }

    async fn automations(&mut self) -> Vec<Automation> {
        let branch = &self.shell.settings.default_branch;
        self.automation_files = match automation::read(self.shell.store.as_ref(), branch).await {
            Ok(files) => files,
            Err(e) => {
                error!(error = format!("{e:#}"), "listing the automations failed");
                AutomationFiles::default()
            }
        };
        for file in &self.automation_files.unparsed {
            warn!(
                path = file.path,
                error = file.reason,
                "skipping an automation file"
            );
        }
        self.automation_files.parsed.clone()
    }

    pub fn is_idle(&self) -> bool {
        self.runs.is_empty()
    }

    pub async fn next_finished(&mut self) {
        if let Some(result) = self.runs.join_next_with_id().await {
            self.finished(result);
        }
    }

    /// Waits until no run is in flight and no build is in progress.
    pub async fn settle(&mut self) {
        loop {
            if !self.is_idle() {
                self.next_finished().await;
            } else if self.environment.is_building() {
                self.environment.settle().await;
            } else {
                return;
            }
        }
    }

    pub async fn stop(mut self) {
        self.environment.exit().await;
        for entry in self.live.values() {
            entry.cancel();
        }
        self.settle().await;
    }

    /// Takes in the runs that have ended and the usage limits reported, so
    /// that a run that ends during the read that follows stays live for it.
    fn reap_before_reading(&mut self) {
        while let Some(result) = self.runs.try_join_next_with_id() {
            self.finished(result);
        }
        while let Ok(limit) = self.refusals.try_recv() {
            self.refused(limit);
        }
    }

    fn take_shown_comments(&mut self, snapshot: &Snapshot, now: Timestamp) {
        let linewright = &self.shell.policy.linewright;
        let pulls = &mut self.memory.pulls;
        match &mut self.pull_records_file {
            PullRecordsFile::Missing => {
                self.pull_records_file = PullRecordsFile::Kept {
                    saved: None,
                    settling: pulls.take_shown(snapshot, linewright, now, |_| true),
                };
            }
            PullRecordsFile::Kept { settling, .. } if !settling.is_empty() => {
                *settling =
                    pulls.take_shown(snapshot, linewright, now, |pr| settling.contains(&pr));
            }
            PullRecordsFile::Kept { .. } => {}
        }
    }

    fn record_pulls(&mut self) {
        let PullRecordsFile::Kept { saved, .. } = &mut self.pull_records_file else {
            return;
        };
        if saved.as_ref() == Some(&self.memory.pulls) {
            return;
        }
        match self.memory.pulls.save(&self.shell.work_dir) {
            Ok(()) => *saved = Some(self.memory.pulls.clone()),
            Err(e) => warn!(
                error = format!("{e:#}"),
                "recording Linewright's pull requests failed"
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
            running, in_flight, ..
        }) = self.live.remove(&id)
        else {
            return;
        };
        if self.memory.drain.is_some() {
            info!(
                run = %in_flight.id,
                still_in_flight = self.live.len(),
                "a run the drain waits for ended"
            );
        }
        let Running::Work(run) = running else {
            return;
        };
        let ended = self.shell.runs.read_run(&in_flight.id);
        self.memory
            .ended_runs
            .extend(ended.as_ref().and_then(Recorded::ended_on_pull));
        match run.subject {
            Subject::Issue(issue) => {
                self.written.insert(Item::Issue(issue));
            }
            Subject::Pull(pr) => {
                self.written.insert(Item::Pull(pr));
            }
        }
    }

    /// Opens the issue that takes up the spec change `pr`, then, if
    /// `let_go`, unassigns `pr`: stopped between the two, it leaves `pr`
    /// assigned, to be let go of at the next poll. The kept snapshot holds
    /// the issue from then on.
    async fn take_up_spec_change(
        &mut self,
        snapshot: &Snapshot,
        pr: Number,
        let_go: bool,
    ) -> Result<()> {
        let store = self.shell.store.clone();
        let spec_change = snapshot
            .merged_pull(pr)
            .with_context(|| format!("the snapshot holds no merged pull request {pr}"))?;
        let required = self.shell.creation_label().await?;
        let new = apply::filing::spec_change_issue(spec_change, required.as_deref());
        let issue = store.create_issue(new).await?;
        info!(
            pr = pr.0,
            issue = issue.0,
            "took up a spec change: opened its issue"
        );
        change_kept_snapshot(&mut self.kept, |latest| {
            latest.spec_change_issues.push(SpecChangeIssue {
                number: issue,
                spec_change: pr,
            });
        });
        self.written.insert(Item::Issue(issue));
        if let_go {
            store.unassign(pr).await?;
        }
        Ok(())
    }

    async fn execute(&mut self, action: &Action, kept: &Kept) -> Result<()> {
        let store = &self.shell.store;
        let snapshot = &kept.snapshot;
        match action {
            Action::Start(work) => {
                let shell = &self.shell;
                let running = shell.runs.begin(
                    Begin::Work {
                        item: work.item(),
                        stage: work.kind(),
                    },
                    shell.clock.now(),
                )?;
                let id = running.id().to_string();
                let in_flight = running.in_flight();
                let cancel = Cancellation::default();
                let reworked = match work.subject() {
                    Subject::Pull(pr) => self.memory.rework_answered_until(pr),
                    Subject::Issue(_) => None,
                };
                let ctx = run::Ctx {
                    with: shell.run_with(&kept.repo, RunKind::Stage(work.kind())),
                    as_of: kept.as_of(work),
                    reworked,
                    cancel: cancel.clone(),
                    run: running,
                };
                let handle =
                    self.runs
                        .spawn(run::run(shell.clone(), work.clone(), snapshot.clone(), ctx));
                info!(run = id, subject = ?work.subject(), kind = ?work.kind(), "run started");
                self.live.insert(
                    handle.id(),
                    LiveEntry {
                        running: Running::Work(LiveRun {
                            subject: work.subject(),
                            kind: work.kind(),
                        }),
                        in_flight,
                        cancel,
                    },
                );
            }
            Action::Cancel { subject, reason } => {
                for entry in self.live.values() {
                    if matches!(&entry.running, Running::Work(run) if run.subject == *subject) {
                        info!(subject = ?subject, reason, "cancelling run");
                        entry.cancel();
                    }
                }
            }
            Action::Claim { item } => {
                info!(%item, "claiming");
                store.assign(item.number()).await?;
            }
            Action::LetGo { issue, comment } => {
                info!(
                    issue = issue.0,
                    "letting go: its label does not ask for Linewright"
                );
                comment_and_let_go(store.as_ref(), *issue, comment.as_deref()).await?;
            }
            Action::Relabel { issue, to } => {
                info!(issue = issue.0, %to, "relabeling");
                let asks = apply::Asks {
                    status: Some(apply::StatusTo::Set(*to)),
                    ..apply::Asks::default()
                };
                run::ask(&**store, *issue, &asks).await?;
            }
            Action::UnassignAsked { issue, assignees } => {
                info!(issue = issue.0, ?assignees, "unassigning the people asked");
                store.unassign_people(*issue, assignees).await?;
            }
            Action::UnassignFromPull { pr, comment } => {
                info!(pr = pr.0, "letting go of a pull request it did not open");
                comment_and_let_go(store.as_ref(), *pr, Some(comment)).await?;
            }
            Action::OpenSpecChangeIssue { pr, let_go } => {
                self.take_up_spec_change(snapshot, *pr, *let_go).await?;
            }
            Action::RemoveStatusLabel { issue, label } => {
                info!(issue = issue.0, %label, "removing a status label");
                store.remove_label(*issue, label.label()).await?;
            }
            Action::HandBack { item, kind, reason } => {
                info!(%item, reason, "handing back");
                let hand_back = apply::hand_back::HandBack {
                    kind: *kind,
                    reason: reason.clone(),
                    log_tail: None,
                };
                run::hand_back(&self.shell, snapshot, *item, &hand_back).await?;
            }
            Action::ClearStall { item } => {
                info!(%item, "clearing the stall");
                store.remove_label(item.number(), label::STALLED).await?;
            }
            Action::MarkReady { pr } => {
                info!(pr = pr.0, "marking ready for review");
                let has_ci = snapshot.pull(*pr).is_some_and(|p| !p.checks.is_empty());
                let note = if has_ci {
                    "CI is green: ready for review."
                } else {
                    "No CI reported: ready for review."
                };
                store.mark_ready(*pr).await?;
                store.comment(*pr, note).await?;
            }
            Action::Merge { pr, head, method } => {
                info!(pr = pr.0, method = ?method, "merging");
                if let Err(e) = store.merge(*pr, head, *method).await {
                    let reason = format!("{e:#}");
                    warn!(pr = pr.0, reason, "merge failed");
                    let at = self.shell.clock.now();
                    self.memory.pulls.merge_failed(*pr, reason, at);
                    self.record_pulls();
                }
            }
            Action::CommentRequirements { pr, requires } => {
                info!(
                    pr = pr.0,
                    ?requires,
                    "commenting on what GitHub requires to merge"
                );
                store.comment(*pr, &requirements_note(requires)).await?;
                let at = self.shell.clock.now();
                self.memory.pulls.commented(*pr, at);
                self.record_pulls();
            }
            Action::CloseTracking { issue } => {
                info!(issue = issue.0, "closing tracking issue");
                store.comment(*issue, "All tickets are closed.").await?;
                store
                    .close_issue(*issue, IssueCloseReason::Completed)
                    .await?;
            }
            Action::DiscardCut(discarding) => {
                warn!(
                    issue = discarding.issue.0,
                    "discarding a cut to design it again"
                );
                discard_cut(store.as_ref(), discarding).await?;
            }
            Action::DeleteBranch { branch } => {
                info!(
                    branch = branch.as_str(),
                    "deleting the branch of a closed ticket"
                );
                store.delete_branch(branch.as_str()).await?;
                change_kept_snapshot(&mut self.kept, |latest| {
                    latest.ticket_branches.retain(|b| b.name != *branch);
                });
            }
            Action::RecordLastScheduledRuns(last_scheduled_runs) => {
                self.last_scheduled_runs = last_scheduled_runs.clone();
                last_scheduled_runs.save(&self.shell.work_dir)?;
            }
            Action::RunAutomation(automation) => {
                let running = begin_automation(&self.shell, &automation.name, StartedBy::Schedule)?;
                let in_flight = running.in_flight();
                let cancel = Cancellation::default();
                let shell = self.shell.clone();
                let repo = kept.repo.clone();
                let handle = self.runs.spawn({
                    let (automation, cancel) = (automation.clone(), cancel.clone());
                    async move {
                        let _ = run_and_log(&shell, &repo, &automation, running, cancel).await;
                    }
                });
                self.live.insert(
                    handle.id(),
                    LiveEntry {
                        running: Running::Automation,
                        in_flight,
                        cancel,
                    },
                );
            }
        }
        Ok(())
    }
}

fn ended_runs(runs: &Runs) -> Vec<EndedRun> {
    runs.read_or_none()
        .iter()
        .filter_map(Recorded::ended_on_pull)
        .collect()
}

fn begin_automation(
    shell: &Shell,
    name: &AutomationName,
    started_by: StartedBy,
) -> Result<runs::Running> {
    shell
        .runs
        .begin(
            Begin::Automation {
                automation: name.clone(),
                started_by,
            },
            shell.clock.now(),
        )
        .with_context(|| format!("recording the run of the automation `{name}`"))
}

async fn run_and_log(
    shell: &Shell,
    repo: &RepoLayer,
    automation: &Automation,
    running: runs::Running,
    cancel: Cancellation,
) -> Result<()> {
    let name = &automation.name;
    info!(automation = %name, run = %running.id(), "automation run started");
    let ctx = run::Ctx {
        with: shell.run_with(repo, RunKind::Automation(automation)),
        as_of: shell.clock.now(),
        reworked: None,
        cancel,
        run: running,
    };
    match run::automation(shell, automation, ctx).await {
        Ok(AutomationEnd::Finished(result)) => {
            info!(automation = %name, summary = result.summary, "automation run finished");
            Ok(())
        }
        Ok(AutomationEnd::UsageLimit(limit)) => {
            warn!(
                automation = %name,
                resets_at = %limit.resets_at,
                "automation run ended for the usage limit"
            );
            Ok(())
        }
        Err(e) => {
            error!(automation = %name, error = format!("{e:#}"), "automation run failed");
            Err(e)
        }
    }
}

/// Makes `change` to the snapshot in `kept`, if any.
fn change_kept_snapshot(kept: &mut Option<Kept>, change: impl FnOnce(&mut Snapshot)) {
    if let Some(kept) = kept {
        change(Arc::make_mut(&mut kept.snapshot));
    }
}

/// Comments `comment`, if any, on `number`, then unassigns Linewright from
/// it: stopped between the two, it leaves `number` assigned, to be let go of
/// at the next poll.
async fn comment_and_let_go(
    store: &dyn Store,
    number: Number,
    comment: Option<&str>,
) -> Result<()> {
    if let Some(comment) = comment {
        store.comment(number, comment).await?;
    }
    store.unassign(number).await
}

/// Discards a cut: closes its pull requests, then its open tickets, then
/// removes its tickets from the sub-issues. Stopped part way, it hands no
/// ticket back, and the next poll finds the discard again.
async fn discard_cut(store: &dyn Store, discarding: &Discard) -> Result<()> {
    let Discard {
        issue,
        pulls,
        tickets,
    } = discarding;
    let notes = discarding.notes();
    for pr in pulls {
        store.comment(*pr, &notes.on_pull).await?;
        store.close_pull(*pr).await?;
    }
    for ticket in discarding.open_tickets() {
        discard(store, ticket, &notes.on_ticket).await?;
    }
    if let Some(ParentNote { issue: on, note }) = &notes.on_issue {
        store.comment(*on, note).await?;
    }
    for ticket in tickets {
        store.remove_sub_issue(*issue, ticket.number).await?;
    }
    Ok(())
}

/// Closes `issue`, of a cut discarded, as not planned, saying why in `note`.
async fn discard(store: &dyn Store, issue: Number, note: &str) -> Result<()> {
    store.comment(issue, note).await?;
    store.close_issue(issue, IssueCloseReason::NotPlanned).await
}

/// Labels `linewright:designed` the open spec changes' tracking issues
/// Linewright filed before a tracking issue carried it, which it would
/// otherwise take for cuts to discard, and reads them into `snapshot` again.
/// One already labeled is left as it is.
async fn label_old_tracking_issues(
    snapshot: &mut Snapshot,
    shell: &Shell,
    windows: Windows,
) -> Result<()> {
    let old = snapshot.unlabeled_tracking_issues(&shell.policy.linewright);
    for &issue in &old {
        shell
            .store
            .add_label(issue, label::DESIGNED)
            .await
            .with_context(|| format!("labeling the tracking issue {issue} {}", label::DESIGNED))?;
        info!(%issue, "labeled an old tracking issue {}", label::DESIGNED);
    }
    read_all(snapshot, shell, old.into_iter().map(Item::Issue), windows).await?;
    Ok(())
}

async fn read_all(
    snapshot: &mut Snapshot,
    shell: &Shell,
    items: impl IntoIterator<Item = Item>,
    windows: Windows,
) -> Result<BTreeSet<Item>> {
    let mut read = BTreeSet::new();
    for item in items {
        let read_with = read_into(snapshot, shell, item, windows).await;
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
    windows: Windows,
) -> Result<Vec<Item>> {
    let store = &shell.store;
    match item {
        Item::Issue(number) => read_issue_into(snapshot, shell, number, windows).await,
        Item::Pull(number) => {
            let held = match store.linewright_pull(number).await? {
                Some(pull) => PullHeldRead::Linewrights(pull),
                None => PullHeldRead::Peoples(store.peoples_pull(number).await?),
            };
            let merged = store.merged_pull(number, windows).await?;
            let closing = snapshot.took_pull(number, PullRead { held, merged });
            let mut read = vec![item];
            for issue in closing {
                read.extend(read_issue_into(snapshot, shell, issue, windows).await?);
            }
            Ok(read)
        }
    }
}

async fn read_issue_into(
    snapshot: &mut Snapshot,
    shell: &Shell,
    number: Number,
    windows: Windows,
) -> Result<Vec<Item>> {
    let store = &shell.store;
    let issue = store.issue(number).await?;
    let taken = snapshot.took_issue(number, issue, windows, &shell.policy.linewright);
    let mut read = vec![Item::Issue(number)];
    read.extend(taken.pulls_read.into_iter().map(Item::Pull));
    for pr in taken.pulls_to_read {
        read.push(Item::Pull(pr));
        snapshot.took_ended_pull(pr, store.linewright_pull(pr).await?);
    }
    Ok(read)
}

/// The items `action` changes in the store, beyond what the runs it starts
/// write: deleting a ticket's branch leaves the ticket as it was.
fn written(action: &Action, snapshot: &Snapshot) -> Vec<Item> {
    if matches!(action, Action::Start(_) | Action::DeleteBranch { .. }) {
        vec![]
    } else {
        action.items(snapshot)
    }
}
