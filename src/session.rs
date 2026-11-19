use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::apply::hand_back::{self, FollowUps, HandBack, LogTail, SendBack, Stopped};
use crate::attachments::Dir;
use crate::bash_command;
use crate::briefs::Briefs;
use crate::clock::Clock;
use crate::clones::{CloneDir, Clones};
use crate::domain::Timestamp;
use crate::environment;
use crate::git;
use crate::harness::{
    Deadline, GhReach, Harness, NextTurn, Purpose, Reporter, SessionError, SessionSpec, Turn,
    UsageLimit,
};
use crate::prompts::{self, SentBack};
use crate::reconcile::Subject;
use crate::redaction::Redaction;
use crate::results::{self, ParseError};
use crate::run::RunWith;
use crate::secrets::Secrets;
use crate::store::{Identity, Store};
use crate::transcript::{self, MaintenanceStatus};
use crate::work_settings::{Maintenance, MaintenanceCommand, SessionSettings};

/// The parts every session shares, whatever it is for.
pub(crate) struct Sessions {
    /// Fetched from and cloned.
    pub store: Arc<dyn Store>,
    pub harness: Arc<dyn Harness>,
    /// The `bash` that runs the maintenance command.
    pub bash: PathBuf,
    pub clock: Arc<dyn Clock>,
    pub identity: Identity,
    pub default_branch: String,
    pub secrets: Secrets,
    pub clones: Clones,
    /// What each session's clone starts from.
    pub environment: environment::Source,
    /// How the `gh` of each session and maintenance command reaches GitHub.
    pub gh: GhReach,
    pub result_retries: u32,
    pub waits_out_usage_limit: bool,
    pub report_refusal: tokio::sync::mpsc::UnboundedSender<UsageLimit>,
}

/// Where the records of sessions go, and what keeps what each ran.
#[async_trait::async_trait]
pub(crate) trait RecordsSessions: Sync {
    /// A new directory, made now, for the record of a session of `purpose`
    /// that starts `now`.
    fn record_dir(&self, purpose: &Purpose, now: Timestamp) -> Result<PathBuf>;

    /// Records the session whose record is in `dir`, run with `settings`
    /// and `briefs`, and gives what records what the harness reports of it.
    async fn session(&self, dir: &Path, settings: &SessionSettings, briefs: &Briefs) -> Reporter;
}

/// What a session works with that is not shared by every session.
#[derive(Clone, Copy)]
pub(crate) struct SessionCtx<'a> {
    pub with: &'a RunWith,
    pub cancel: &'a Cancellation,
    pub records: &'a dyn RecordsSessions,
}

pub(crate) enum Failure {
    Infrastructure {
        error: anyhow::Error,
        /// What the hand-back adds after the reason.
        note: String,
    },
    HandBack(HandBack),
    Apply {
        error: anyhow::Error,
        /// What the hand-back adds after the reason.
        note: String,
    },
    Cancelled,
    UsageLimit(UsageLimit),
}

/// A run's cancellation, which only a run in session takes: a cancelled run
/// posts nothing more (docs/specs/runs.md, "Cancelling"), and one that has
/// begun applying its result applies all of it, unless none of it was applied
/// and it goes back to its session.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<Cancel>);

#[derive(Default)]
struct Cancel {
    /// Fired once the phase is cancelled.
    token: CancellationToken,
    phase: Mutex<Phase>,
}

#[derive(Clone, Copy, Default)]
enum Phase {
    #[default]
    InSession,
    /// Applying its result; `stop` holds a cancel that came meanwhile.
    Applying {
        stop: bool,
    },
    Cancelled,
}

impl Cancellation {
    /// Cancels the run unless it is applying its result, and returns whether
    /// the run is cancelled. A cancel while applying is kept for
    /// [`Self::back_to_session`].
    pub(crate) fn cancel(&self) -> bool {
        let mut phase = self.phase();
        match *phase {
            Phase::InSession => {
                *phase = Phase::Cancelled;
                self.0.token.cancel();
                true
            }
            Phase::Applying { .. } => {
                *phase = Phase::Applying { stop: true };
                false
            }
            Phase::Cancelled => true,
        }
    }

    /// Begins applying the result, unless the run was cancelled first.
    pub(crate) fn begin_applying(&self) -> Result<(), Failure> {
        let mut phase = self.phase();
        match *phase {
            Phase::InSession => {
                *phase = Phase::Applying { stop: false };
                Ok(())
            }
            Phase::Applying { .. } => Ok(()),
            Phase::Cancelled => Err(Failure::Cancelled),
        }
    }

    /// Returns to the session, nothing of the result applied: the run is
    /// cancelled then if a cancel came while it was applying.
    pub(crate) fn back_to_session(&self) -> Result<(), Failure> {
        let mut phase = self.phase();
        match *phase {
            Phase::InSession | Phase::Applying { stop: false } => {
                *phase = Phase::InSession;
                Ok(())
            }
            Phase::Applying { stop: true } | Phase::Cancelled => {
                *phase = Phase::Cancelled;
                self.0.token.cancel();
                Err(Failure::Cancelled)
            }
        }
    }

    async fn cancelled(&self) {
        self.0.token.cancelled().await;
    }

    fn phase(&self) -> std::sync::MutexGuard<'_, Phase> {
        lock_set_whole(&self.0.phase)
    }
}

impl Failure {
    /// Adds `note` to what the failure's hand-back says. A cancelled run and
    /// one that ends for the usage limit post nothing, so they take none.
    pub(crate) fn noted(self, note: &str) -> Failure {
        match self {
            Failure::Infrastructure {
                error,
                note: before,
            } => Failure::Infrastructure {
                error,
                note: before + note,
            },
            Failure::Apply {
                error,
                note: before,
            } => Failure::Apply {
                error,
                note: before + note,
            },
            Failure::HandBack(hand_back) => Failure::HandBack(HandBack {
                reason: hand_back.reason + note,
                ..hand_back
            }),
            failure @ (Failure::Cancelled | Failure::UsageLimit(_)) => failure,
        }
    }
}

pub(crate) fn infra(error: anyhow::Error) -> Failure {
    Failure::Infrastructure {
        error,
        note: String::new(),
    }
}

/// A clone, removed when dropped.
pub(crate) struct Workspace {
    /// What the maintenance command left running, ended before the clone is
    /// removed.
    leftovers: Option<bash_command::Leftovers>,
    clone_dir: CloneDir,
    pub(crate) dir: PathBuf,
    scratch: PathBuf,
    plugin: PathBuf,
}

impl Workspace {
    /// A session's clone in `clone_dir`: a copy of the environment with
    /// `branch` checked out in it, or a plain clone of `branch` without an
    /// environment (docs/specs/sessions.md, Environment, "A session's clone").
    async fn make(
        sessions: &Sessions,
        briefs: &Briefs,
        clone_dir: CloneDir,
        branch: &str,
    ) -> Result<Workspace> {
        let Some(lease) = sessions.environment.lease() else {
            return Workspace::clone_in(sessions, briefs, clone_dir, branch).await;
        };
        Workspace::create(sessions, briefs, clone_dir, async |dir| {
            lease.copy_to(dir).await?;
            drop(lease);
            git::configure(dir, &[KEEPS_REFLOGS]).await?;
            sessions
                .store
                .fetch(dir)
                .await
                .context("fetching into the copy of the environment")?;
            git::check_out_fresh(dir, branch).await
        })
        .await
    }

    /// A clone of `branch` of its own, for work that needs no session.
    pub(crate) async fn clone(
        sessions: &Sessions,
        briefs: &Briefs,
        purpose: &Purpose,
        branch: &str,
    ) -> Result<Workspace> {
        Workspace::clone_in(
            sessions,
            briefs,
            Workspace::clone_dir(sessions, purpose).await?,
            branch,
        )
        .await
    }

    /// A plain clone of `branch` in `clone_dir`.
    async fn clone_in(
        sessions: &Sessions,
        briefs: &Briefs,
        clone_dir: CloneDir,
        branch: &str,
    ) -> Result<Workspace> {
        Workspace::create(sessions, briefs, clone_dir, async |dir| {
            sessions
                .store
                .clone_repo(dir, branch, &[KEEPS_REFLOGS])
                .await
        })
        .await
    }

    /// The directory a clone for `purpose` is made in, its repository in
    /// [`Workspace::repo_in`].
    async fn clone_dir(sessions: &Sessions, purpose: &Purpose) -> Result<CloneDir> {
        sessions.clones.make(&purpose_name(purpose)).await
    }

    /// A new, empty directory beside the clone's repository, removed with
    /// the clone.
    ///
    /// # Errors
    ///
    /// When the directory cannot be made.
    pub(crate) fn make_beside(&self, beside: Beside) -> Result<PathBuf> {
        let dir = self.clone_dir.path().join(beside.name());
        std::fs::create_dir(&dir)
            .with_context(|| format!("making {} beside the clone", beside.name()))?;
        Ok(dir)
    }

    /// Where the repository of a clone made in `clone_dir` is.
    fn repo_in(clone_dir: &Path) -> PathBuf {
        clone_dir.join("repo")
    }

    /// A new workspace in `clone_dir`, whose repository `fill` makes at the
    /// path it is given, which does not exist yet. The repository is then
    /// given [`clone_config`], and the workspace a scratch directory and
    /// the plugin of `briefs`.
    async fn create(
        sessions: &Sessions,
        briefs: &Briefs,
        clone_dir: CloneDir,
        fill: impl AsyncFnOnce(&Path) -> Result<()>,
    ) -> Result<Workspace> {
        let dir = Workspace::repo_in(clone_dir.path());
        fill(&dir).await?;
        git::configure(&dir, &clone_config(sessions)).await?;
        let scratch = clone_dir.path().join("tmp");
        tokio::fs::create_dir(&scratch)
            .await
            .context("creating a scratch directory")?;
        let plugin = clone_dir.path().join("plugin");
        briefs
            .write_plugin(&plugin)
            .context("writing the brief's plugin")?;
        Ok(Workspace {
            leftovers: None,
            clone_dir,
            dir,
            scratch,
            plugin,
        })
    }
}

/// A directory that a clone's directory holds beside its repository, its
/// scratch directory and its brief's plugin.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Beside {
    /// A scorer's copy of the trial it scores.
    Trial,
}

impl Beside {
    fn name(self) -> &'static str {
        match self {
            Beside::Trial => "replay",
        }
    }
}

/// The git setting that has a clone log its branch's updates from
/// its checkout on, whatever the environment or the host's git config say.
const KEEPS_REFLOGS: (&str, &str) = ("core.logAllRefUpdates", "true");

/// The git settings of every clone Linewright makes: Linewright's identity, and
/// no maintenance git starts by itself.
fn clone_config(sessions: &Sessions) -> [(&str, &str); 3] {
    [
        ("user.name", sessions.identity.name.as_str()),
        ("user.email", sessions.identity.email.as_str()),
        ("maintenance.auto", "false"),
    ]
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if let Some(leftovers) = self.leftovers.take() {
            self.clone_dir.then_remove(move || leftovers.end());
        }
    }
}

pub fn subject_name(subject: Subject) -> String {
    match subject {
        Subject::Issue(n) => format!("issue-{}", n.0),
        Subject::Pull(n) => format!("pull-{}", n.0),
    }
}

pub fn purpose_name(purpose: &Purpose) -> String {
    match purpose {
        Purpose::Work { subject, .. } => subject_name(*subject),
        Purpose::Automation(name) => format!("automation-{name}"),
        Purpose::Scorer => "judge".into(),
    }
}

/// A new directory, made now, in `sessions_dir` for the record of a session
/// of `purpose` that starts `now`.
pub(crate) fn new_record_dir(
    sessions_dir: &Path,
    purpose: &Purpose,
    now: Timestamp,
) -> Result<PathBuf> {
    let parent = sessions_dir.join(purpose_name(purpose));
    std::fs::create_dir_all(&parent).context("creating the sessions directory")?;
    let kind = match purpose {
        Purpose::Work { kind, .. } => kind.name(),
        Purpose::Automation(_) => "automation",
        Purpose::Scorer => "judge",
    };
    let base = format!("{}-{kind}", now.format("%Y%m%dT%H%M%SZ"));
    for n in 1.. {
        let dir = if n == 1 {
            parent.join(&base)
        } else {
            parent.join(format!("{base}-{n}"))
        };
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!()
}

/// Why a result goes back to its session.
pub(crate) enum Rejection {
    /// Sent back with `followup`, counted against `result-retries`; `note`
    /// is what the hand-back adds once they run out.
    Retry {
        followup: String,
        note: Option<String>,
    },
    /// Sent back with `followup` as a new question, the count reset: the
    /// answer to new feedback is a new result.
    Resume(String),
    Fail(Failure),
}

impl Rejection {
    /// Sent back with `followup`, the hand-back adding nothing.
    pub(crate) fn retry(followup: String) -> Rejection {
        Rejection::Retry {
            followup,
            note: None,
        }
    }
}

/// Why a result's first writes refused it.
pub(crate) enum Refused {
    /// Before they wrote to the session's work, which is as it left it.
    Untouched(Rejection),
    /// As they wrote the work, whose failure says what became of it.
    Writing(Rejection),
}

impl From<anyhow::Error> for Refused {
    fn from(error: anyhow::Error) -> Refused {
        Refused::Untouched(error.into())
    }
}

/// How a conversation ended before its result's first writes were done.
pub(crate) enum Unbegun {
    /// In session, or sent back to it: the stage's work is as the session
    /// left it.
    InSession(Failure),
    /// In the result's first writes, whose failure says what became of the
    /// work.
    Begun(Failure),
}

impl Unbegun {
    /// The failure, noted with what `keep` keeps of the work the session
    /// left.
    pub(crate) async fn keeping<'a>(self, keep: impl AsyncFnOnce() -> Stopped<'a>) -> Failure {
        match self {
            Unbegun::InSession(failure) => failure.noted(&keep().await.note()),
            Unbegun::Begun(failure) => failure,
        }
    }
}

impl From<Unbegun> for Failure {
    fn from(unbegun: Unbegun) -> Failure {
        match unbegun {
            Unbegun::InSession(failure) | Unbegun::Begun(failure) => failure,
        }
    }
}

impl From<anyhow::Error> for Rejection {
    fn from(error: anyhow::Error) -> Rejection {
        Rejection::Fail(infra(error))
    }
}

/// Sends a result that does not parse back to the session with its `error`.
pub(crate) fn parse_retry(briefs: &Briefs) -> impl Fn(ParseError) -> Rejection + '_ {
    |error| {
        Rejection::retry(prompts::result_error(
            &briefs.templates,
            &SentBack::Result(&error),
        ))
    }
}

/// Runs a session until `accept` takes its result, in session, and `begin`
/// has made the result's first writes, once applying it has begun. A result
/// either rejects goes back to the session. The session's end is in its
/// transcript once it returns, however it ends.
pub(crate) async fn converse<T, U, F, Fut, B, BFut>(
    sessions: &Sessions,
    ctx: SessionCtx<'_>,
    spec: &mut SessionSpec,
    first: NextTurn,
    accept: F,
    begin: B,
) -> Result<(U, Turn), Unbegun>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T, Rejection>>,
    B: Fn(T) -> BFut,
    BFut: Future<Output = Result<U, Refused>>,
{
    let conversed = conversation(sessions, ctx, spec, first, accept, begin).await;
    end(sessions, &spec.record, &spec.report, spec.deadline);
    conversed
}

/// Ends the session whose record is `record` now: writes its end there, or
/// else logs that it is not written, as the session has ended either way,
/// and reports to `report` the time its `deadline` counted.
fn end(sessions: &Sessions, record: &Path, report: &Reporter, deadline: Deadline) {
    if let Err(unrecorded) = append_event(record, &transcript::session_ended(sessions.clock.now()))
    {
        warn!(
            error = format!("{unrecorded:#}"),
            "the session's end is not recorded"
        );
    }
    report.ended(deadline.counted(tokio::time::Instant::now()));
}

/// [`converse`], up to the session's end.
async fn conversation<T, U, F, Fut, B, BFut>(
    sessions: &Sessions,
    ctx: SessionCtx<'_>,
    spec: &mut SessionSpec,
    first: NextTurn,
    accept: F,
    begin: B,
) -> Result<(U, Turn), Unbegun>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T, Rejection>>,
    B: Fn(T) -> BFut,
    BFut: Future<Output = Result<U, Refused>>,
{
    let mut follow_ups = FollowUps::default();
    let in_session = Unbegun::InSession;
    let mut turn = take_turn(sessions, ctx, spec, first)
        .await
        .map_err(in_session)?;
    loop {
        let refused = match accept(turn.message.clone()).await {
            Ok(result) => {
                let json = results::json_value(&turn.message)
                    .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
                    .unwrap_or_else(|| turn.message.clone());
                record(spec, "result.json", &json);
                ctx.cancel.begin_applying().map_err(in_session)?;
                match begin(result).await {
                    Ok(written) => return Ok((written, turn)),
                    Err(Refused::Writing(Rejection::Fail(failure))) => {
                        return Err(Unbegun::Begun(failure));
                    }
                    Err(refused) => {
                        ctx.cancel.back_to_session().map_err(in_session)?;
                        refused
                    }
                }
            }
            Err(rejection) => Refused::Untouched(rejection),
        };
        let (rejection, unbegun): (_, fn(Failure) -> Unbegun) = match refused {
            Refused::Untouched(rejection) => (rejection, Unbegun::InSession),
            Refused::Writing(rejection) => (rejection, Unbegun::Begun),
        };
        let step = match rejection {
            Rejection::Fail(failure) => return Err(unbegun(failure)),
            Rejection::Retry { followup, note } => {
                let session_log = scrub(&turn.message, &spec.dir);
                match follow_ups.send_back(sessions.result_retries, &followup, &session_log) {
                    SendBack::Retry => NextTurn::Resume {
                        session: turn.session.clone(),
                        prompt: followup,
                    },
                    SendBack::HandBack(hand_back) => {
                        let failure = Failure::HandBack(hand_back);
                        return Err(unbegun(failure.noted(note.as_deref().unwrap_or_default())));
                    }
                }
            }
            Rejection::Resume(followup) => {
                follow_ups.resumed();
                NextTurn::Resume {
                    session: turn.session.clone(),
                    prompt: followup,
                }
            }
        };
        turn = take_turn(sessions, ctx, spec, step)
            .await
            .map_err(in_session)?;
    }
}

fn record(spec: &SessionSpec, name: &str, text: &str) {
    let _ = std::fs::write(spec.record.join(name), text);
}

async fn take_turn(
    sessions: &Sessions,
    ctx: SessionCtx<'_>,
    spec: &mut SessionSpec,
    mut next: NextTurn,
) -> Result<Turn, Failure> {
    let subject = purpose_name(&spec.purpose);
    loop {
        let turn = match &next {
            NextTurn::Start => {
                record(spec, transcript::BRIEF, &spec.prompt);
                cancellable(ctx, sessions.harness.start(spec)).await?
            }
            NextTurn::Resume { session, prompt } => {
                record(spec, &followup_name(&spec.record), prompt);
                cancellable(ctx, sessions.harness.resume(spec, session, prompt)).await?
            }
        };
        let (limit, session) = match turn {
            Ok(turn) => return Ok(turn),
            Err(SessionError::UsageLimit { limit, session }) => (limit, session),
            Err(SessionError::Infrastructure(reason)) => {
                return Err(infra(anyhow::anyhow!(reason)));
            }
            Err(SessionError::TimedOut { log_tail }) => {
                return Err(Failure::HandBack(hand_back::timed_out(
                    &ctx.with.settings.timeout_setting,
                    LogTail::session(&scrub(&log_tail, &spec.dir)),
                )));
            }
            Err(SessionError::Failed { reason, log_tail }) => {
                return Err(Failure::HandBack(hand_back::session_failed(
                    &reason,
                    &scrub(&log_tail, &spec.dir),
                )));
            }
        };
        let _ = sessions.report_refusal.send(limit);
        if !sessions.waits_out_usage_limit {
            return Err(Failure::UsageLimit(limit));
        }
        let resets_at = limit.resets_at;
        let refused_at = tokio::time::Instant::now();
        let now = sessions.clock.now();
        let until = limit.wait_until(now);
        warn!(subject, %resets_at, "session refused for the usage limit");
        info!(subject, %resets_at, %until, "waiting out the usage limit");
        append_event(&spec.record, &transcript::usage_limit_waiting(now, until))
            .context("writing the wait for the usage limit")
            .map_err(infra)?;
        let waited = cancellable(ctx, sessions.clock.sleep_until(until)).await;
        spec.deadline =
            UsageLimit::deadline_after_wait(spec.deadline, refused_at, tokio::time::Instant::now());
        waited?;
        append_event(
            &spec.record,
            &transcript::usage_limit_waited(sessions.clock.now()),
        )
        .context("writing the end of the wait for the usage limit")
        .map_err(infra)?;
        info!(subject, %resets_at, "usage limit reset; resuming the session");
        next = NextTurn::Resume {
            session,
            prompt: prompts::usage_limit_reset(&ctx.with.briefs.templates),
        };
    }
}

/// The output of `turn`, unless `ctx` is cancelled first.
async fn cancellable<T>(ctx: SessionCtx<'_>, turn: impl Future<Output = T>) -> Result<T, Failure> {
    tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => Err(Failure::Cancelled),
        output = turn => Ok(output),
    }
}

fn followup_name(record: &Path) -> String {
    let n = (1..=usize::MAX)
        .find(|n| !record.join(transcript::followup_file(*n)).exists())
        .expect("a session has fewer follow-ups than numbers");
    transcript::followup_file(n)
}

/// Why a session's clone was not made.
enum Unmade {
    Cancelled,
    /// The session's deadline came first.
    TimedOut,
    Failed(anyhow::Error),
}

impl Unmade {
    /// The error the clone's entry shows: a failure's chain, redacted and
    /// with the clone's repository `repo`, once known, shown as `.`.
    fn shown(&self, redaction: &Redaction, repo: Option<&Path>) -> String {
        match self {
            Unmade::Cancelled => "cancelled".into(),
            Unmade::TimedOut => "timed out".into(),
            Unmade::Failed(error) => {
                let shown = redaction.apply(&format!("{error:#}"));
                match repo {
                    Some(repo) => scrub(&shown, repo),
                    None => shown,
                }
            }
        }
    }

    /// How the session ends (docs/specs/runs.md, "Stages", 1a).
    fn failure(self, settings: &SessionSettings) -> Failure {
        match self {
            Unmade::Cancelled => Failure::Cancelled,
            Unmade::TimedOut => {
                Failure::HandBack(hand_back::timed_out(&settings.timeout_setting, None))
            }
            Unmade::Failed(error) => infra(error.context("making the session's clone")),
        }
    }
}

/// When something that started `at` and took `duration` ended.
fn ended_after(at: Timestamp, duration: std::time::Duration) -> Timestamp {
    at + chrono::TimeDelta::from_std(duration).unwrap_or(chrono::TimeDelta::MAX)
}

/// Appends the `line` Linewright writes to the transcript of the session
/// whose record is `record`.
fn append_event(record: &Path, line: &str) -> Result<()> {
    let mut transcript = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(record.join(transcript::OUTPUT))
        .context("opening the session's transcript")?;
    writeln!(transcript, "{line}").context("writing to the session's transcript")
}

fn scrub(text: &str, dir: &Path) -> String {
    text.replace(&dir.to_string_lossy().into_owned(), ".")
}

/// A session opened in its clone, whose prompt is still to be filled.
pub(crate) struct Opened<'a> {
    settings: &'a SessionSettings,
    purpose: Purpose,
    pub(crate) record: PathBuf,
    attachments: Dir,
    report: Reporter,
    dir: PathBuf,
    scratch: PathBuf,
    plugin: PathBuf,
    gh: GhReach,
    /// When the session times out, the making of its clone and its
    /// maintenance command included.
    deadline: Deadline,
}

/// Opens the session of `purpose`, with its record and its attachments
/// directory, recorded in `ctx`'s records, then makes its clone of `branch`
/// and runs `check_out` on it, then runs its maintenance command
/// (docs/specs/runs.md, "Stages"). Making the clone, `check_out` included,
/// and the maintenance command count against the session's timeout. Each is
/// an entry in the session's transcript, its start and end written there
/// whether or not the agent starts, and the session's end too when it fails
/// once its record is made.
pub(crate) async fn open_session<'a, T>(
    sessions: &Sessions,
    ctx: SessionCtx<'a>,
    purpose: Purpose,
    branch: &str,
    check_out: impl AsyncFnOnce(&Path) -> Result<T>,
) -> Result<(Opened<'a>, Workspace, T), Failure> {
    let record = ctx
        .records
        .record_dir(&purpose, sessions.clock.now())
        .map_err(infra)?;
    let settings = &ctx.with.settings;
    let report = ctx
        .records
        .session(&record, settings, &ctx.with.briefs)
        .await;
    let deadline = Deadline::after(tokio::time::Instant::now(), settings.session_timeout.value);
    let opened = open_in(
        sessions,
        ctx,
        purpose,
        branch,
        check_out,
        Begun {
            record: record.clone(),
            report: report.clone(),
            deadline,
        },
    )
    .await;
    if opened.is_err() {
        end(sessions, &record, &report, deadline);
    }
    opened
}

/// A session recorded, with its deadline set, whose clone is still to be
/// made.
struct Begun {
    record: PathBuf,
    report: Reporter,
    deadline: Deadline,
}

/// [`open_session`], once it is [`Begun`].
async fn open_in<'a, T>(
    sessions: &Sessions,
    ctx: SessionCtx<'a>,
    purpose: Purpose,
    branch: &str,
    check_out: impl AsyncFnOnce(&Path) -> Result<T>,
    begun: Begun,
) -> Result<(Opened<'a>, Workspace, T), Failure> {
    let Begun {
        record,
        report,
        deadline,
    } = begun;
    let settings = &ctx.with.settings;
    let attachments = Dir::create(&record)
        .context("creating the attachments directory")
        .map_err(infra)?;
    let started_at = sessions.clock.now();
    let started = tokio::time::Instant::now();
    append_event(&record, &transcript::clone_started(started_at))
        .context("writing the clone's start")
        .map_err(infra)?;
    let mut repo = None;
    let making = async {
        let clone_dir = Workspace::clone_dir(sessions, &purpose).await?;
        repo = Some(Workspace::repo_in(clone_dir.path()));
        let ws = Workspace::make(sessions, &ctx.with.briefs, clone_dir, branch).await?;
        let checked_out = check_out(&ws.dir).await?;
        Ok::<_, anyhow::Error>((ws, checked_out))
    };
    let made = tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => Err(Unmade::Cancelled),
        () = tokio::time::sleep_until(deadline.at()) => Err(Unmade::TimedOut),
        made = making => made.map_err(Unmade::Failed),
    };
    let duration = started.elapsed();
    let error = made
        .as_ref()
        .err()
        .map(|unmade| unmade.shown(sessions.secrets.redaction(), repo.as_deref()));
    let ended = append_event(
        &record,
        &transcript::clone_ended(
            ended_after(started_at, duration),
            duration.as_secs_f64(),
            error.as_deref(),
        ),
    )
    .context("writing the clone's end");
    let (mut ws, checked_out) = match made {
        Ok(made) => {
            ended.map_err(infra)?;
            made
        }
        Err(unmade) => {
            if let Err(unrecorded) = ended {
                warn!(
                    error = format!("{unrecorded:#}"),
                    "the clone's end is not recorded"
                );
            }
            return Err(unmade.failure(settings));
        }
    };
    if let Maintenance::Command(command) = &settings.maintenance.value {
        run_maintenance(sessions, ctx, &purpose, command, &record, deadline, &mut ws).await?;
    }
    let opened = Opened {
        settings,
        purpose,
        record,
        attachments,
        report,
        dir: ws.dir.clone(),
        scratch: ws.scratch.clone(),
        plugin: ws.plugin.clone(),
        gh: sessions.gh.clone(),
        deadline,
    };
    Ok((opened, ws, checked_out))
}

/// Opens the session of `purpose` on the default branch, as the clone
/// checked it out.
pub(crate) async fn open_on_default_branch<'a>(
    sessions: &Sessions,
    ctx: SessionCtx<'a>,
    purpose: Purpose,
) -> Result<(Opened<'a>, Workspace), Failure> {
    let branch = &sessions.default_branch;
    let (opened, ws, ()) = open_session(sessions, ctx, purpose, branch, async |_| Ok(())).await?;
    Ok((opened, ws))
}

/// Runs the maintenance `command` of the session of `purpose` in `ws`, which
/// keeps what it leaves running, until the session's `deadline`, with its
/// start and end in the session's transcript in `record` and its output beside
/// it. It fails as the session would end.
async fn run_maintenance(
    sessions: &Sessions,
    ctx: SessionCtx<'_>,
    purpose: &Purpose,
    command: &MaintenanceCommand,
    record: &Path,
    deadline: Deadline,
    ws: &mut Workspace,
) -> Result<(), Failure> {
    let settings = &ctx.with.settings;
    let redaction = sessions.secrets.redaction();
    let started_at = sessions.clock.now();
    append_event(
        record,
        &transcript::maintenance_started(started_at, &redaction.apply(command.as_str())),
    )
    .context("writing the maintenance command's start")
    .map_err(infra)?;
    let request = bash_command::Request {
        command: command.as_str(),
        dir: &ws.dir,
        environment: sessions.harness.environment(
            &settings.environment,
            &ws.scratch,
            settings.effort_set(),
            &sessions.gh,
        ),
        deadline: deadline.at(),
        log: &record.join(transcript::MAINTENANCE_OUTPUT),
    };
    let (ran, leftovers) =
        bash_command::run(&sessions.bash, request, redaction, ctx.cancel.cancelled())
            .await
            .context("running the maintenance command")
            .map_err(infra)?;
    ws.leftovers = Some(leftovers);
    append_event(
        record,
        &transcript::maintenance_ended(
            ended_after(started_at, ran.duration),
            ran.status,
            ran.duration.as_secs_f64(),
        ),
    )
    .context("writing the maintenance command's end")
    .map_err(infra)?;
    match after_maintenance(ran.status) {
        AfterMaintenance::GoesOn => Ok(()),
        AfterMaintenance::GoesOnPastAFailure => {
            warn!(
                subject = purpose_name(purpose),
                status = ran.status.account(),
                "the maintenance command failed; going on to the agent"
            );
            Ok(())
        }
        AfterMaintenance::TimesOut => Err(Failure::HandBack(hand_back::timed_out(
            &settings.timeout_setting,
            LogTail::maintenance(&scrub(&ran.output, &ws.dir)),
        ))),
        AfterMaintenance::Cancelled => Err(Failure::Cancelled),
    }
}

/// How a session goes on after its maintenance command.
#[derive(Debug, PartialEq, Eq)]
pub enum AfterMaintenance {
    /// To the agent's first prompt.
    GoesOn,
    /// To the agent's first prompt all the same (docs/specs/runs.md, Stages,
    /// 2a).
    GoesOnPastAFailure,
    /// It times out: the command was still running at its deadline.
    TimesOut,
    Cancelled,
}

/// How a session goes on after its maintenance command ended as `status`.
pub fn after_maintenance(status: MaintenanceStatus) -> AfterMaintenance {
    match status {
        MaintenanceStatus::Exited(0) => AfterMaintenance::GoesOn,
        MaintenanceStatus::Exited(_) | MaintenanceStatus::Signalled(_) => {
            AfterMaintenance::GoesOnPastAFailure
        }
        MaintenanceStatus::TimedOut => AfterMaintenance::TimesOut,
        MaintenanceStatus::Cancelled => AfterMaintenance::Cancelled,
    }
}

impl Opened<'_> {
    /// `failure`, with the session's end written to its transcript: the
    /// session ends before its agent starts.
    pub(crate) fn ended(&self, sessions: &Sessions, failure: Failure) -> Failure {
        self.end(sessions);
        failure
    }

    /// Ends the session before its agent starts.
    pub(crate) fn end(&self, sessions: &Sessions) {
        end(sessions, &self.record, &self.report, self.deadline);
    }

    /// The session, sent the `prompt` made for its attachments directory,
    /// and that directory.
    pub(crate) fn spec(self, prompt: impl FnOnce(&Path) -> String) -> (SessionSpec, Dir) {
        let Opened {
            settings,
            purpose,
            record,
            attachments,
            report,
            dir,
            scratch,
            plugin,
            gh,
            deadline,
        } = self;
        let spec = SessionSpec {
            record,
            attachments: attachments.path().to_path_buf(),
            prompt: prompt(attachments.path()),
            purpose,
            dir,
            scratch,
            plugin,
            model: settings.model_set(),
            effort: settings.effort_set(),
            environment: settings.environment.clone(),
            gh,
            deadline,
            report,
        };
        (spec, attachments)
    }
}

/// Locks `mutex`, even if a panic poisoned it: what it holds is set whole,
/// so what a panic left is still sound.
pub(crate) fn lock_set_whole<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_log::test]
    fn a_cancel_before_applying_cancels_and_applying_then_fails() {
        let cancellation = Cancellation::default();

        assert!(cancellation.cancel());

        assert!(matches!(
            cancellation.begin_applying(),
            Err(Failure::Cancelled)
        ));
        assert!(cancellation.0.token.is_cancelled());
    }

    #[test_log::test]
    fn a_cancel_while_applying_does_not_stop_the_run() {
        let cancellation = Cancellation::default();

        assert!(cancellation.begin_applying().is_ok());

        assert!(!cancellation.cancel());
        assert!(!cancellation.0.token.is_cancelled());
        assert!(cancellation.begin_applying().is_ok());
    }

    #[test_log::test]
    fn back_in_session_with_no_cancel_meanwhile_the_run_can_be_cancelled_again() {
        let cancellation = Cancellation::default();
        cancellation.begin_applying().ok();

        assert!(cancellation.back_to_session().is_ok());

        assert!(cancellation.cancel());
        assert!(cancellation.0.token.is_cancelled());
    }

    #[test_log::test]
    fn back_in_session_after_a_cancel_while_applying_the_run_is_cancelled() {
        let cancellation = Cancellation::default();
        cancellation.begin_applying().ok();
        cancellation.cancel();

        assert!(matches!(
            cancellation.back_to_session(),
            Err(Failure::Cancelled)
        ));

        assert!(cancellation.0.token.is_cancelled());
        assert!(cancellation.cancel());
    }

    #[test_log::test]
    fn applying_again_after_being_cancelled_fails() {
        let cancellation = Cancellation::default();
        cancellation.begin_applying().ok();
        cancellation.cancel();
        cancellation.back_to_session().ok();

        assert!(matches!(
            cancellation.begin_applying(),
            Err(Failure::Cancelled)
        ));
    }
}
