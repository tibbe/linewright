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

use crate::api::{Publish, Served};
use crate::automation::{self, Automation, AutomationFiles, AutomationName, LastScheduledRuns};
use crate::briefs::Briefs;
use crate::clock::Clock;
use crate::credentials::Credentials;
use crate::domain::{
    AutoMerge, IssueCloseReason, Item, Number, Snapshot, Timestamp, UpdateMethod, UpdateRule, label,
};
use crate::ending::Ends;
use crate::harness::{Harness, UsageLimit};
use crate::holds::Holds;
use crate::reconcile::{
    Action, Automations, CutItem, Discard, DoneRun, Drain, FailedMerge, LiveRun, Memory, Policy,
    Subject, Work, WorkKind, held_back_by_drain, hold_note, reconcile,
};
use crate::run::{self, AutomationEnd, Shell};
use crate::runs::{self, Begin, Recorded, RunOf, Runs, StartedBy};
use crate::spec_glob::SpecGlob;
use crate::status::{FailedPolls, Poll, Polling, Polls, View};
use crate::store::github::SESSION_TOKEN_VARIABLE;
use crate::store::{IssueRead, Store};
use crate::webhook::{self, Pending};

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
    /// `None` leaves green pull requests for review.
    pub auto_merge: Option<AutoMerge>,
    pub require_label: Option<String>,
    pub briefs: Briefs,
    pub work_dir: PathBuf,
    pub schedule_automations: bool,
    pub waits_out_usage_limit: bool,
    pub session_token: SessionToken,
    /// Linewright's credentials, refused in the commits a session's result
    /// would push, and redacted from what a session's work prints that
    /// reaches Linewright other than through the harness, such as git's
    /// output from hooks the session left in its clone.
    pub credentials: Credentials,
    /// What drains and stops the factory.
    pub ends: Ends,
}

/// The GitHub token agent sessions are given.
#[derive(Clone)]
pub enum SessionToken {
    /// Linewright's own, which can write to GitHub.
    Linewright(String),
    /// The operator's token for sessions, from [`SESSION_TOKEN_VARIABLE`].
    Separate(String),
    /// None, as in a replay, whose sessions do not read GitHub.
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
    cancel: CancellationToken,
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
    schedule_automations: bool,
    last_scheduled_runs: LastScheduledRuns,
    automation_files: AutomationFiles,
    memory: Memory,
    holds_record: HoldsRecord,
    kept: Option<Kept>,
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
        let mut settings = store
            .settings()
            .await
            .context("reading the repository's settings")?;
        if let Some(method) = config.update_method {
            settings.update_rule = UpdateRule::Always(method);
        }
        if let Some(auto_merge) = config.auto_merge {
            if let Some(method) = auto_merge.merge_method {
                settings.merge_method = method;
            }
            settings.check_auto_merge(config.update_method)?;
        }
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
            update_rule = ?settings.update_rule,
            auto_merge = config.auto_merge.is_some(),
            merge_method = %settings.merge_method,
            "started"
        );
        let last_scheduled_runs = if config.schedule_automations {
            LastScheduledRuns::load(&config.work_dir)
        } else {
            LastScheduledRuns::default()
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
                auto_merge: config.auto_merge.map(|_| settings.merge_method),
                update_rule: settings.update_rule,
                allowed_merge_methods: settings.allowed_merge_methods.clone(),
                require_label: config.require_label,
            },
            store,
            harness,
            clock,
            briefs: config.briefs,
            identity,
            settings,
            result_retries: config.result_retries,
            session_timeout: config.session_timeout,
            runs: runs.clone(),
            work_dir: config.work_dir,
            waits_out_usage_limit: config.waits_out_usage_limit,
            credentials: config.credentials,
            report_refusal,
        };
        Ok(Factory {
            shell: Arc::new(shell),
            runs: JoinSet::new(),
            refusals,
            live: HashMap::new(),
            ends: config.ends,
            schedule_automations: config.schedule_automations,
            last_scheduled_runs,
            automation_files: AutomationFiles::default(),
            memory: Memory {
                done_runs: done_runs(&runs),
                held: held.unwrap_or_default(),
                ..Memory::default()
            },
            holds_record,
            kept: None,
            written: BTreeSet::new(),
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

    /// Cancelling it begins a drain: no item is claimed and no run starts
    /// from then on, and [`Factory::serve`] returns once no run is in flight.
    pub fn draining(&self) -> CancellationToken {
        self.ends.draining()
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
        self.memory.ended_cuts.clear();
        self.written.clear();
        let kept = Kept {
            snapshot: Arc::new(snapshot),
            taken_at: now,
            read_at: HashMap::new(),
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
        let Some(mut kept) = self.kept.clone() else {
            return self.cycle().await;
        };
        // Reap first, as a poll does: a run that ends during the reads stays
        // live for this refresh.
        self.reap();
        let shell = &self.shell;
        let now = shell.clock.now();
        let window_opened = now - shell.policy.spec_lookback;
        let snapshot = Arc::make_mut(&mut kept.snapshot);
        let mut read = read_all(snapshot, shell, self.written.clone(), window_opened).await?;
        let named: BTreeSet<Item> = subjects
            .into_iter()
            .flat_map(|subject| subject.into().items(snapshot))
            .filter(|item| !read.contains(item))
            .collect();
        if read.is_empty() && named.is_empty() {
            return Ok(vec![]);
        }
        read.extend(read_all(snapshot, shell, named, window_opened).await?);
        let others = closed_by_pulls_read(snapshot, &read);
        read.extend(read_all(snapshot, shell, others, window_opened).await?);
        kept.read_at
            .extend(read.into_iter().map(|item| (item, now)));
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
        while !stopping.is_cancelled() {
            // The poll reads what deliveries named before it as well.
            pending.take();
            let poll = match self.cycle().await {
                Ok(_) => Poll::Succeeded,
                Err(e) => {
                    // Redacted as recorded, as the same text is logged.
                    let error = self.shell.credentials.redaction().apply(&format!("{e:#}"));
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
            loop {
                if self.drained() {
                    info!("drained: no run is in flight");
                    return ServeEnd::Drained;
                }
                tokio::select! {
                    () = tokio::time::sleep_until(next_poll) => break,
                    () = stopping.cancelled() => break,
                    () = draining.cancelled(), if self.memory.drain.is_none() => {
                        self.note_drain();
                    }
                    // Turns the drain into a stop, which the loop then takes.
                    () = ends.deadline_passed(clock.as_ref()) => {}
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
                publish.served.send_replace(self.served(next_poll_at));
            }
        }
        ServeEnd::Stop
    }

    fn drained(&self) -> bool {
        self.memory.drain.is_some() && self.runs.is_empty()
    }

    /// Takes a drain asked for into memory, once.
    fn note_drain(&mut self) {
        if !self.ends.draining().is_cancelled() || self.memory.drain.is_some() {
            return;
        }
        self.memory.drain = Some(Drain {
            since: self.shell.clock.now(),
        });
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
        Some(Served::new(self.view(next_poll)?, self.shell.store.clone()))
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
        let live = self.live_runs();
        let automations = Automations {
            defined,
            last_scheduled_runs: self.last_scheduled_runs.clone(),
            in_flight: self.automations_in_flight(),
        };
        self.take_shown_holds(snapshot);
        self.memory.retain_open(snapshot);
        self.record_holds();
        let mut actions = reconcile(
            snapshot,
            &live,
            &self.memory,
            &automations,
            now,
            &self.shell.policy,
        );
        let mut planned_draining = self.memory.drain.is_some();
        let mut next = 0;
        while let Some(action) = actions.get(next).cloned() {
            if self.ends.stopping().is_cancelled() {
                break;
            }
            self.note_drain();
            if !planned_draining && self.memory.drain.is_some() {
                // Begun since the plan was made: the rest of the plan is
                // taken as the drain has it.
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

    pub async fn run_automation(&self, name: &AutomationName) -> Result<()> {
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
        run_and_log(
            shell,
            &automation,
            begin_automation(shell, name, StartedBy::Hand)?,
            self.ends.stopping(),
        )
        .await
        .with_context(|| format!("the run of the automation `{name}` failed"))
    }

    async fn automations(&mut self) -> Vec<Automation> {
        if !self.schedule_automations {
            return vec![];
        }
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
        let done = self.shell.runs.read_run(&in_flight.id);
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
            Subject::SpecChange(pr) => self.memory.ended_cuts.push(pr),
        }
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
                        in_flight,
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
            Action::Claim { item } => {
                info!(%item, "claiming");
                store.assign(item.number()).await?;
            }
            Action::LetGo { issue, comment } => {
                info!(
                    issue = issue.0,
                    "letting go: its label does not ask for Linewright"
                );
                // Commented first, so that a stop between the two leaves
                // the issue assigned, to be let go of at the next poll.
                if let Some(comment) = comment {
                    store.comment(*issue, comment).await?;
                }
                store.unassign(*issue).await?;
            }
            Action::Relabel { issue, remove, add } => {
                info!(issue = issue.0, remove, add, "relabeling");
                store.remove_label(*issue, remove).await?;
                store.add_label(*issue, add).await?;
            }
            Action::UnassignAsked { issue, assignees } => {
                info!(issue = issue.0, ?assignees, "unassigning the people asked");
                store.unassign_people(*issue, assignees).await?;
            }
            Action::RemoveStatusLabel { issue, label } => {
                info!(issue = issue.0, %label, "removing a status label");
                store.remove_label(*issue, label.label()).await?;
            }
            Action::HandBack { item, kind, reason } => {
                info!(%item, reason, "handing back");
                let hand_back = run::HandBack {
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
                store
                    .close_issue(*issue, IssueCloseReason::Completed)
                    .await?;
            }
            Action::DiscardCut(discarding) => {
                let Discard {
                    item,
                    peoples_pulls,
                    tickets,
                    pulls,
                } = discarding;
                let designed = item.designed();
                warn!(item = designed.0, "discarding a cut to design it again");
                let on_pull =
                    discard_note(designed, "pull request for a ticket of its earlier cut is");
                // A pull request of a person is seen only through its open
                // ticket, so it closes first; the discard leaves the ticket
                // alone until it is closed, so nothing hands it back.
                for pr in peoples_pulls {
                    store.comment(*pr, &on_pull).await?;
                    store.close_pull(*pr).await?;
                }
                // Each ticket closes before Linewright's pull request, so
                // that Linewright never sees its pull request closed unmerged
                // while the ticket is open, which would hand the ticket back.
                let on_ticket = discard_note(designed, "ticket from its earlier cut is");
                for ticket in tickets {
                    discard(store.as_ref(), *ticket, &on_ticket).await?;
                }
                for pr in pulls {
                    store.comment(*pr, &on_pull).await?;
                    store.close_pull(*pr).await?;
                }
                // The parent last, so that a discard that stops part way is
                // found again on the next poll.
                match item {
                    CutItem::SpecChange { tracking, .. } => {
                        let on_tracking =
                            discard_note(designed, "tracking issue and its tickets are");
                        discard(store.as_ref(), *tracking, &on_tracking).await?;
                    }
                    CutItem::Issue(issue) => {
                        let mut closed: Vec<Number> = discarding.all_pulls().collect();
                        closed.sort_unstable();
                        store
                            .comment(*issue, &discarded_note(tickets, &closed))
                            .await?;
                    }
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
            Action::RecordLastScheduledRuns(last_scheduled_runs) => {
                self.last_scheduled_runs = last_scheduled_runs.clone();
                last_scheduled_runs.save(&self.shell.work_dir)?;
            }
            Action::RunAutomation(automation) => {
                let running = begin_automation(&self.shell, &automation.name, StartedBy::Schedule)?;
                let in_flight = running.in_flight();
                let cancel = CancellationToken::new();
                let shell = self.shell.clone();
                let handle = self.runs.spawn({
                    let (automation, cancel) = (automation.clone(), cancel.clone());
                    // A run that fails is logged; there is nothing to hand back.
                    async move {
                        let _ = run_and_log(&shell, &automation, running, cancel).await;
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

fn done_runs(runs: &Runs) -> Vec<DoneRun> {
    runs.read_or_none()
        .iter()
        .filter_map(Recorded::done_on_pull)
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
    automation: &Automation,
    running: runs::Running,
    cancel: CancellationToken,
) -> Result<()> {
    let name = &automation.name;
    info!(automation = %name, run = %running.id(), "automation run started");
    match run::automation(shell, automation, running, cancel).await {
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

/// Closes `issue`, of a cut discarded, as not planned, saying why in `note`.
async fn discard(store: &dyn Store, issue: Number, note: &str) -> Result<()> {
    store.comment(issue, note).await?;
    store.close_issue(issue, IssueCloseReason::NotPlanned).await
}

/// What an item of a cut discarded to design `designed` again says, the
/// item being `what` and its verb.
fn discard_note(designed: Number, what: &str) -> String {
    format!("{designed} is being designed again, so this {what} discarded.")
}

/// What an issue says of the `tickets` and `pulls` of its earlier cut that
/// were discarded, at least one of them.
fn discarded_note(tickets: &[Number], pulls: &[Number]) -> String {
    let listed = |one: &str, many: &str, numbers: &[Number]| {
        let numbers: Vec<String> = numbers.iter().map(Number::to_string).collect();
        match numbers.as_slice() {
            [] => None,
            [only] => Some(format!("{one} {only}")),
            [rest @ .., last] => Some(format!("{many} {} and {last}", rest.join(", "))),
        }
    };
    let lists: Vec<String> = [
        listed("ticket", "tickets", tickets),
        listed("pull request", "pull requests", pulls),
    ]
    .into_iter()
    .flatten()
    .collect();
    let verb = if tickets.len() + pulls.len() == 1 {
        "is"
    } else {
        "are"
    };
    format!(
        "This issue is being designed again, so the {} of its earlier cut {verb} discarded.",
        lists.join(" and ")
    )
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
    window_opened: Timestamp,
) -> Result<BTreeSet<Item>> {
    let mut read = BTreeSet::new();
    for item in items {
        let read_with = read_into(snapshot, shell, item, window_opened).await;
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
    window_opened: Timestamp,
) -> Result<Vec<Item>> {
    let store = &shell.store;
    match item {
        Item::Issue(number) => read_issue_into(snapshot, shell, number, window_opened).await,
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
            match store.merged_pull(number, window_opened).await? {
                Some(merged) => snapshot.put_merged(merged),
                None => snapshot.remove_merged(number),
            }
            let mut read = vec![item];
            for issue in closing {
                read.extend(read_issue_into(snapshot, shell, issue, window_opened).await?);
            }
            Ok(read)
        }
    }
}

async fn read_issue_into(
    snapshot: &mut Snapshot,
    shell: &Shell,
    number: Number,
    window_opened: Timestamp,
) -> Result<Vec<Item>> {
    let store = &shell.store;
    let mut read = vec![Item::Issue(number)];
    snapshot.remove_closed(number);
    let left = match store.issue(number, window_opened).await? {
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
        IssueRead::Closed(closed) => {
            snapshot.relink(number, false);
            snapshot.put_closed(*closed);
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
        Action::Claim { item } => vec![*item],
        Action::LetGo { issue, .. }
        | Action::Relabel { issue, .. }
        | Action::UnassignAsked { issue, .. }
        | Action::RemoveStatusLabel { issue, .. }
        | Action::CloseTracking { issue } => vec![Item::Issue(*issue)],
        Action::DiscardCut(discard) => discard
            .tickets
            .iter()
            .copied()
            .chain([discard.item.parent()])
            .map(Item::Issue)
            .chain(discard.all_pulls().map(Item::Pull))
            .collect(),
        Action::Merge { pr, .. } => snapshot
            .pull(*pr)
            .into_iter()
            .flat_map(|p| p.closes.iter().map(|issue| Item::Issue(*issue)))
            .chain([Item::Pull(*pr)])
            .collect(),
        Action::MarkReady { pr }
        | Action::MarkReadyForMergeState { pr }
        | Action::HoldForReview { pr, .. } => vec![Item::Pull(*pr)],
        Action::HandBack { item, .. } | Action::ClearStall { item } => vec![*item],
        Action::Start(_)
        | Action::Cancel { .. }
        | Action::RememberRebase(_)
        | Action::DeleteBranch { .. }
        | Action::RecordLastScheduledRuns(_)
        | Action::RunAutomation(_) => vec![],
    }
}
