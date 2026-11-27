//! Scoring a trial's output against the criteria of its task, in a
//! session of the scorer's own (`docs/specs/benchmarks.md`, Scoring).

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use tracing::{error, warn};

use super::{
    COMMITS, OUTCOME, OwnDir, Setting, TASK, Under, Unrun, WRITES, in_own_dir, write_build,
    write_whole_new,
};
use crate::apply::hand_back::{self, LogTail};
use crate::benchmarks::ScoreFigures;
use crate::briefs::Briefs;
use crate::clock::{Clock, SystemClock};
use crate::cost::{RunCost, SessionCost, Usd};
use crate::domain::Timestamp;
use crate::factory::FactoryConfig;
use crate::git;
use crate::harness::{Harness, NextTurn, Purpose, Reporter};
use crate::prompts::{self, Repo};
use crate::results::{self, Correctness, Leak, ScorerResult};
use crate::run::RunWith;
use crate::runs::{
    Ending, OutcomeName, Ran, Recorded, RunId, SessionEntry, SessionKept, SessionReports, Started,
    State, UnexpectedEnd,
};
use crate::session::{
    self, Beside, Cancellation, Ended, Failure, RecordsSessions, SessionCtx, Sessions, converse,
    infra, lock_set_whole, open_session, parse_retry,
};
use crate::task::Task;
use crate::work_settings::{RunKind, SessionSettings};
use crate::{COMMAND, version_and_commit};

mod copy;

pub use copy::TrialCopy;

/// The part of an output that holds its score.
pub const SCORE: &str = "score.json";

/// How a trial's run ended, of the endings a scorer scores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scoreable {
    Done,
    HandedBack,
}

/// The output of a trial that a scorer can score: its run ended done or
/// handed back, and it was run from the task it is scored against.
#[derive(Clone, Debug)]
pub struct ScoreableTrial {
    /// Absolute.
    dir: PathBuf,
    outcome: Scoreable,
    /// The branch of the trial's last push that its commits hold, if any.
    pushed: Option<String>,
}

/// Why an output cannot be scored.
#[derive(Debug, thiserror::Error)]
pub enum ScoreRefusal {
    #[error("the output's directory cannot be used: {0}")]
    Unusable(std::io::Error),
    #[error("the output has no `{OUTCOME}` part")]
    NoOutcome,
    #[error(
        "the trial's outcome is `{0}`: only a trial whose run ended done or handed back is scored"
    )]
    Outcome(String),
    #[error("the output has no `{TASK}` part, which names the run its task was captured from")]
    NoTask,
    #[error("the output's `{TASK}` part names the run {named}, not {task}, the run of the task")]
    OtherRun { named: String, task: RunId },
    #[error("the output's `{part}` part cannot be read: {error}")]
    Unreadable { part: &'static str, error: String },
    #[error("the output already holds a score, `{SCORE}`")]
    Scored,
}

impl ScoreableTrial {
    /// The output in `dir`, run from `task`.
    ///
    /// # Errors
    ///
    /// When the output's outcome is missing or other than done or handed
    /// back, its `task` part is missing or names a run other than `task`'s,
    /// it holds a score, or a part cannot be read.
    pub async fn open(dir: &Path, task: &Task) -> Result<ScoreableTrial, ScoreRefusal> {
        let dir = std::path::absolute(dir).map_err(ScoreRefusal::Unusable)?;
        let ended = read_part(&dir, OUTCOME)?.ok_or(ScoreRefusal::NoOutcome)?;
        let outcome = match serde_json::from_value(serde_json::Value::String(ended.clone())) {
            Ok(OutcomeName::Ended(Ending::Done)) => Scoreable::Done,
            Ok(OutcomeName::Ended(Ending::HandedBack)) => Scoreable::HandedBack,
            Ok(_) | Err(_) => return Err(ScoreRefusal::Outcome(ended)),
        };
        let named = read_part(&dir, TASK)?.ok_or(ScoreRefusal::NoTask)?;
        if named.parse::<RunId>().as_ref() != Ok(task.run()) {
            return Err(ScoreRefusal::OtherRun {
                named,
                task: task.run().clone(),
            });
        }
        match std::fs::symlink_metadata(dir.join(SCORE)) {
            Ok(_) => return Err(ScoreRefusal::Scored),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(ScoreRefusal::Unreadable {
                    part: SCORE,
                    error: e.to_string(),
                });
            }
        }
        let unreadable = |error: anyhow::Error| ScoreRefusal::Unreadable {
            part: WRITES,
            error: format!("{error:#}"),
        };
        let mut pushed = None;
        for branch in pushes(&dir.join(WRITES))
            .map_err(unreadable)?
            .into_iter()
            .rev()
        {
            if holds(&dir.join(COMMITS), &branch).await {
                pushed = Some(branch);
                break;
            }
        }
        Ok(ScoreableTrial {
            dir,
            outcome,
            pushed,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn outcome(&self) -> Scoreable {
        self.outcome
    }
}

/// The text of the output's part `name`, its trailing white space trimmed,
/// or `None` when there is none.
fn read_part(dir: &Path, name: &'static str) -> Result<Option<String>, ScoreRefusal> {
    match std::fs::read_to_string(dir.join(name)) {
        Ok(text) => Ok(Some(text.trim_end().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ScoreRefusal::Unreadable {
            part: name,
            error: e.to_string(),
        }),
    }
}

/// A push, as the writes keep it.
#[derive(Deserialize)]
struct Push {
    branch: String,
}

/// The branch of each push kept in the writes in `writes`, in the order
/// they were made.
fn pushes(writes: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(writes) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut pushes = vec![];
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().into_owned();
        let number = name
            .strip_suffix("-push.json")
            .and_then(|number| number.parse::<u32>().ok());
        if let Some(number) = number {
            pushes.push((number, name));
        }
    }
    pushes.sort();
    pushes
        .into_iter()
        .map(|(_, name)| {
            let text = std::fs::read_to_string(writes.join(&name))?;
            let push: Push = serde_json::from_str(&text).with_context(|| name.clone())?;
            Ok(push.branch)
        })
        .collect()
}

/// Whether the bare repository `repository` holds `branch`: a push the
/// writes keep is kept before it is made, and it may then have failed.
async fn holds(repository: &Path, branch: &str) -> bool {
    git::git(
        repository,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .await
    .is_ok()
}

/// The record of a scorer's session: its directory under the output's
/// `scorer/`, and what the session ran with and what it cost once it is
/// opened. It is removed when dropped, unless the score that names it
/// is kept.
#[derive(Debug)]
pub struct ScorerRecord {
    /// Absolute.
    dir: PathBuf,
    /// Relative to the output.
    relative: PathBuf,
    session: Arc<Mutex<Option<SessionReports>>>,
    kept: bool,
}

impl ScorerRecord {
    /// A new, empty directory for the record of a scorer of `scoreable`.
    ///
    /// # Errors
    ///
    /// When the directory cannot be made.
    pub fn make(scoreable: &ScoreableTrial) -> Result<ScorerRecord> {
        let dir = session::new_record_dir(&scoreable.dir, &Purpose::Scorer, SystemClock.now())
            .context("making the record of the scorer's session")?;
        let relative = dir
            .strip_prefix(&scoreable.dir)
            .context("the scorer's record is in the output")?
            .to_path_buf();
        Ok(ScorerRecord {
            dir,
            relative,
            session: Arc::default(),
            kept: false,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What the scorer's session spends, readable once the record is gone.
    pub fn spent(&self) -> ScorerSpent {
        ScorerSpent(self.session.clone())
    }

    /// Empties the record for the session's retry.
    fn restart(&self) -> Result<()> {
        std::fs::remove_dir_all(&self.dir)
            .and_then(|()| std::fs::create_dir(&self.dir))
            .context("emptying the record of the scorer's session")
    }
}

impl Drop for ScorerRecord {
    fn drop(&mut self) {
        if !self.kept {
            discard(&self.dir);
        }
    }
}

/// What a scorer's session spent, as its record reports it.
#[derive(Clone, Debug)]
pub struct ScorerSpent(Arc<Mutex<Option<SessionReports>>>);

impl ScorerSpent {
    /// What the session cost so far: `None` before it opened, when it gave
    /// no figures, or once its score took them.
    pub fn money(&self) -> Option<Usd> {
        lock_set_whole(&self.0)
            .as_ref()
            .and_then(|session| money(&session.cost()))
    }
}

/// What a session that cost `cost` cost in money, `None` when it gave no
/// figures.
fn money(cost: &SessionCost) -> Option<Usd> {
    RunCost::of([cost]).spend.and_then(|spend| spend.all.usd)
}

/// What a benchmark run's results need of the score the output in `dir`
/// holds, if it holds one.
///
/// # Errors
///
/// When it cannot be read, or does not read as a score.
pub fn kept(dir: &Path) -> Result<Option<ScoreFigures>> {
    Ok(read(dir)?.map(|kept| ScoreFigures {
        correctness: kept.correctness.verdict,
        leak: kept.leak.found(),
        scorer_money: money(&kept.session.cost),
    }))
}

/// A score the output in a directory holds, as it reads.
#[derive(Debug, Deserialize)]
pub struct KeptScore {
    pub correctness: Correctness,
    pub leak: Leak,
    session: KeptScorerSession,
}

#[derive(Debug, Deserialize)]
struct KeptScorerSession {
    record: PathBuf,
    #[serde(flatten)]
    ran: Ran,
    cost: SessionCost,
}

/// The score the output in `dir` holds, if it holds one.
///
/// # Errors
///
/// When it cannot be read, or does not read as a score.
pub fn read(dir: &Path) -> Result<Option<KeptScore>> {
    let path = dir.join(SCORE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    serde_json::from_str(&text)
        .map(Some)
        .with_context(|| format!("reading {}", path.display()))
}

/// The scorer of the output in `dir` as a run `id` of one session: ended
/// once the output holds its score, and in flight while it holds none
/// and the scorer's record is there. `None` when there is neither.
///
/// # Errors
///
/// When the score or the scorers' directory cannot be read.
pub fn recorded(dir: &Path, id: &RunId) -> Result<Option<Recorded>> {
    let started = Started::new(id.clone(), None).map_err(anyhow::Error::msg)?;
    let modified = |path: &Path| -> Result<Timestamp> {
        let at = std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .with_context(|| format!("reading {}", path.display()))?;
        Ok(at.into())
    };
    if let Some(kept) = read(dir)? {
        let record = dir.join(&kept.session.record);
        let session = SessionEntry {
            kept: Some(SessionKept {
                ran: kept.session.ran,
                cost: Some(kept.session.cost),
            }),
            dir: record.clone(),
        };
        let state = State::Ended {
            at: modified(&dir.join(SCORE))?,
            ending: Ending::Done,
        };
        return Ok(Some(Recorded::new(
            started,
            started_at(&record).map_or_else(|| modified(&record), Ok)?,
            state,
            vec![session],
            None,
        )));
    }
    let scorers = dir.join(session::purpose_name(&Purpose::Scorer));
    let entries = match std::fs::read_dir(&scorers) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", scorers.display())),
    };
    let mut records = Vec::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("reading {}", scorers.display()))?
            .path();
        if path.is_dir() {
            records.push(path);
        }
    }
    records.sort();
    let Some(record) = records.pop() else {
        return Ok(None);
    };
    Ok(Some(Recorded::new(
        started,
        started_at(&record).map_or_else(|| modified(&record), Ok)?,
        State::InFlight,
        vec![record.into()],
        None,
    )))
}

/// When the scorer whose record is in `dir` started, as the record's name
/// gives it ([`session::new_record_dir`]), if it does.
fn started_at(dir: &Path) -> Option<Timestamp> {
    let name = dir.file_name()?.to_str()?;
    let stamp = name.get(..16)?;
    chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%SZ")
        .ok()
        .map(|at| at.and_utc())
}

/// Removes the records of scorers of `scoreable` that kept no score, such
/// as one a stop left behind.
pub fn discard_unkept(scoreable: &ScoreableTrial) {
    let scorers = scoreable.dir.join(session::purpose_name(&Purpose::Scorer));
    match std::fs::remove_dir_all(&scorers) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            warn!(dir = %scorers.display(), error = %e, "removing the scorers' records failed");
        }
        _ => {}
    }
}

/// Removes the scorer's record in `dir`, and the output's `scorer/` with it
/// when that holds no other, logging when it cannot.
fn discard(dir: &Path) {
    let mut removed = std::fs::remove_dir_all(dir);
    for _ in 0..2 {
        match &removed {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                removed = std::fs::remove_dir_all(dir);
            }
            _ => break,
        }
    }
    match removed {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            warn!(dir = %dir.display(), error = %e, "removing the scorer's record failed");
        }
        _ => {
            if let Some(scorers) = dir.parent() {
                let _ = std::fs::remove_dir(scorers);
            }
        }
    }
}

/// The scorer's one session is recorded in the record's directory, and what
/// it ran with and what it cost are kept for its score, as the run's
/// record keeps a session's.
#[async_trait::async_trait]
impl RecordsSessions for ScorerRecord {
    fn record_dir(&self, _purpose: &Purpose, _now: Timestamp) -> Result<PathBuf> {
        Ok(self.dir.clone())
    }

    async fn session(&self, _dir: &Path, settings: &SessionSettings, briefs: &Briefs) -> Reporter {
        *lock_set_whole(&self.session) = Some(SessionReports::new(Ran::new(settings, briefs)));
        let (reported, ended) = (self.session.clone(), self.session.clone());
        Reporter::new(move |report| {
            if let Some(session) = lock_set_whole(&reported).as_mut() {
                session.reported(report);
            }
        })
        .ending(move |time| {
            if let Some(session) = lock_set_whole(&ended).as_mut() {
                session.ended(time);
            }
        })
    }
}

/// What a scorer decided about a trial, made only from its accepted
/// result. Kept as `score.json` in the output, its record with it.
#[derive(Debug, Serialize)]
pub struct Score {
    correctness: Correctness,
    leak: Leak,
    session: ScorerSession,
    /// The version of Linewright that ran the scorer.
    linewright: String,
    #[serde(skip)]
    record: ScorerRecord,
}

/// The scorer's session: where its record is, relative to the output, what
/// it ran with and what it cost.
#[derive(Debug, Serialize)]
struct ScorerSession {
    record: PathBuf,
    #[serde(flatten)]
    ran: Ran,
    cost: SessionCost,
}

impl Score {
    /// What a benchmark run's results need of it.
    pub fn figures(&self) -> ScoreFigures {
        ScoreFigures {
            correctness: self.correctness.verdict,
            leak: self.leak.found(),
            scorer_money: money(&self.session.cost),
        }
    }
}

/// Why a scorer made no score: one variant for each row of
/// `docs/specs/benchmarks.md`, Scoring, "A scorer that makes no score", but
/// the signal, which ends the command before it has one. `Display` gives the
/// row's words.
#[derive(Debug)]
pub enum Unscored {
    GaveUp(String),
    /// Its session ended unexpectedly.
    Ended(Ended),
    /// The build of its environment failed: the tail of the build's log, if
    /// its setup command ran.
    BuildFailed {
        log_tail: Option<LogTail>,
    },
    /// Its score could not be written: what the file system gave.
    NotKept(std::io::Error),
    /// Any other cause, but a cancel or a restart. Shown without its cause,
    /// which [`Unscored::told`] logs.
    Unexpected(anyhow::Error),
    /// Its session was cancelled, by a cancel or a restart, whose trial
    /// never shows why its scorer made no score. Worded as `Unexpected`,
    /// and not logged.
    Cancelled,
}

impl Unscored {
    /// The words to report, the cause of an [`Unscored::Unexpected`]
    /// logged first.
    pub fn told(self) -> String {
        if let Unscored::Unexpected(cause) = &self {
            error!(
                error = format!("{cause:#}"),
                "the scorer stopped unexpectedly"
            );
        }
        self.to_string()
    }
}

impl fmt::Display for Unscored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unscored::GaveUp(reason) => write!(f, "The scorer gave up: {reason}"),
            Unscored::Ended(Ended { end, log_tail }) => {
                write!(f, "The scorer's session ended without a score: {end}")?;
                match log_tail {
                    Some(tail) => write!(f, "\n\n{}", tail.text()),
                    None => Ok(()),
                }
            }
            Unscored::BuildFailed { log_tail } => {
                write!(f, "The scorer's environment could not be built.")?;
                match log_tail {
                    Some(tail) => write!(f, "\n\n{}", tail.text()),
                    None => Ok(()),
                }
            }
            Unscored::NotKept(error) => write!(f, "The score could not be kept: {error}"),
            Unscored::Unexpected(_) | Unscored::Cancelled => {
                write!(f, "The scorer stopped unexpectedly.")
            }
        }
    }
}

impl std::error::Error for Unscored {}

impl From<Failure> for Unscored {
    fn from(failure: Failure) -> Unscored {
        match failure {
            Failure::HandBack(hand_back) => Unscored::Unexpected(anyhow!(hand_back.reason)),
            Failure::Ended { ended, .. } => Unscored::Ended(ended),
            Failure::Infrastructure { error, .. } | Failure::Apply { error, .. } => {
                Unscored::Unexpected(error)
            }
            Failure::Cancelled => Unscored::Cancelled,
        }
    }
}

/// Runs a scorer of `scoreable`, the output of a trial of `task`, with
/// `config` and `harness`, in the setting a trial of `task` has, its
/// session recorded in `record`. Its clone has the branch the trial
/// pushed last checked out, from the output's commits, or the task's
/// default branch when it pushed none. A session that fails on its
/// infrastructure is retried once, as a stage's is. `under` cancels the
/// scorer's session, and says what it does at the usage limit.
///
/// # Errors
///
/// When the scorer gives up, its session fails, times out or ends with no
/// result accepted, the usage limit is reached, or the setting cannot be
/// prepared. The record is removed then, and a failure's error is redacted.
pub async fn run(
    task: &Task,
    scoreable: &ScoreableTrial,
    record: ScorerRecord,
    harness: Arc<dyn Harness>,
    config: FactoryConfig,
    under: &Under,
) -> Result<Score, Unscored> {
    let redaction = config.secrets.redaction().clone();
    let version = version_and_commit(config.commit);
    let (work_dir, winding_up) = (config.work_dir.clone(), config.winding_up.clone());
    let scored = in_own_dir(&work_dir, winding_up, "scorer", async |own| {
        scorer_in(task, scoreable, &record, harness, &config, under, own).await
    })
    .await
    .map_err(Unscored::Unexpected)
    .flatten()
    .map_err(|unscored| match unscored {
        Unscored::Unexpected(error) => {
            Unscored::Unexpected(anyhow!(redaction.apply(&format!("{error:#}"))))
        }
        unscored @ (Unscored::GaveUp(_)
        | Unscored::Ended(_)
        | Unscored::BuildFailed { .. }
        | Unscored::NotKept(_)
        | Unscored::Cancelled) => unscored,
    })?;
    let (correctness, leak) = scored;
    let session = lock_set_whole(&record.session)
        .take()
        .ok_or_else(|| Unscored::Unexpected(anyhow!("the scorer's session is not recorded")))?;
    Ok(Score {
        correctness,
        leak,
        session: ScorerSession {
            record: record.relative.clone(),
            ran: session.ran().clone(),
            cost: session.cost(),
        },
        linewright: format!("{COMMAND} {version}"),
        record,
    })
}

/// [`run`], in the scorer's own directory `own`.
async fn scorer_in(
    task: &Task,
    scoreable: &ScoreableTrial,
    record: &ScorerRecord,
    harness: Arc<dyn Harness>,
    config: &FactoryConfig,
    under: &Under,
    own: OwnDir<'_>,
) -> Result<(Correctness, Leak), Unscored> {
    let repository = own.path.join(COMMITS);
    let built = Mutex::new(None);
    let prepared = Setting::prepare(
        task,
        &RunKind::Scorer,
        config,
        harness.clone(),
        &own.path.join("store"),
        &repository,
        |build| {
            *lock_set_whole(&built) = Some(build.clone());
            Ok(())
        },
    )
    .await;
    let built = lock_set_whole(&built).take();
    let setting = match (prepared, built) {
        (Ok(setting), built) => {
            if let Some(build) = built {
                write_build(record.dir(), &build)
                    .context("writing the build's output")
                    .map_err(Unscored::Unexpected)?;
            }
            setting
        }
        (Err(Unrun::BuildFailed(failed)), _) => {
            warn!(
                why = failed.why,
                "the build of the scorer's environment failed"
            );
            return Err(Unscored::BuildFailed {
                log_tail: failed
                    .output
                    .and_then(|build| LogTail::maintenance(&build.output)),
            });
        }
        (Err(Unrun::Failed(e)), built) => {
            if let Some(build) = built {
                error!(
                    output = build.output,
                    "the build of the scorer's environment"
                );
            }
            return Err(Unscored::Unexpected(e));
        }
    };
    if let Some(branch) = &scoreable.pushed {
        fetch_pushed(&repository, &scoreable.dir.join(COMMITS), branch)
            .await
            .map_err(Unscored::Unexpected)?;
    }
    let sessions = setting.sessions(
        task,
        setting.store.clone(),
        harness,
        config,
        own.clones(),
        &under.usage_limit,
    );
    let with = RunWith {
        briefs: setting.repo.briefs.clone(),
        settings: config.work.for_run(
            RunKind::Scorer,
            &setting.repo.settings,
            &config.environments,
        ),
    };
    let mut retries = 0;
    let result = loop {
        match session(
            task,
            scoreable,
            record,
            config,
            &sessions,
            &with,
            &under.cancel,
        )
        .await
        {
            Err(Failure::Infrastructure { error, .. })
                if hand_back::retries_infrastructure_failure(retries) =>
            {
                retries += 1;
                let error = config.secrets.redaction().apply(&format!("{error:#}"));
                warn!(
                    error,
                    "the scorer's session failed on its infrastructure; retrying once"
                );
                record.restart().map_err(Unscored::Unexpected)?;
            }
            Err(Failure::Infrastructure { error, .. }) => {
                let error = config.secrets.redaction().apply(&format!("{error:#}"));
                let end = UnexpectedEnd::Infrastructure { error };
                return Err(Unscored::Ended(Ended::without_log(end)));
            }
            ended => break ended?,
        }
    };
    drop(setting);
    match result {
        ScorerResult::Scored { correctness, leak } => Ok((correctness, leak)),
        ScorerResult::GaveUp { reason } => Err(Unscored::GaveUp(reason)),
    }
}

/// The scorer's session in `sessions`, recorded in `record`, on what the
/// trial left, cancelled by `cancel`: sent the filled
/// `briefs/prompts/scorer.md`, until its result is accepted.
async fn session(
    task: &Task,
    scoreable: &ScoreableTrial,
    record: &ScorerRecord,
    config: &FactoryConfig,
    sessions: &Sessions,
    with: &RunWith,
    cancel: &Cancellation,
) -> Result<ScorerResult, Failure> {
    let ctx = SessionCtx {
        with,
        cancel,
        records: record,
    };
    let branch = scoreable
        .pushed
        .as_deref()
        .unwrap_or(&sessions.default_branch);
    let (opened, ws, ()) =
        open_session(sessions, ctx, Purpose::Scorer, branch, async |_| Ok(())).await?;
    let copying = async { TrialCopy::make(scoreable, &ws.make_beside(Beside::Trial)?).await };
    let copy = copying
        .await
        .context("copying the trial for the scorer")
        .map_err(|error| opened.ended(sessions, infra(error)))?;
    let settings = &task.properties().settings;
    let repo = Repo {
        name: &settings.repo,
        default_branch: &sessions.default_branch,
        spec_glob: config.spec_glob.pattern(),
    };
    let briefs = &with.briefs;
    let (mut spec, _attachments) = opened.spec(|_| {
        prompts::scorer(
            &briefs.templates,
            &repo,
            task.stage().kind(),
            task.criteria(),
            &copy,
        )
    });
    let (result, _) =
        converse(
            sessions,
            ctx,
            &mut spec,
            NextTurn::Start,
            |message| async move {
                results::parse::<ScorerResult>(&message).map_err(parse_retry(briefs))
            },
            |result| async move { Ok(result) },
        )
        .await?;
    Ok(result)
}

/// Fetches `branch` from the output's commits `commits` into the bare
/// repository `repository`, in place of the branch it holds, if any.
async fn fetch_pushed(repository: &Path, commits: &Path, branch: &str) -> Result<()> {
    let commits = commits
        .to_str()
        .context("the output's commits are at a UTF-8 path")?;
    git::git(
        repository,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            commits,
            &format!("+refs/heads/{branch}:refs/heads/{branch}"),
        ],
    )
    .await
    .with_context(|| format!("fetching {branch}, the branch the trial pushed, from its commits"))?;
    Ok(())
}

/// Keeps `score` in `scoreable`'s output, so that a reader sees it
/// whole or not at all. It never replaces a score the output holds.
///
/// # Errors
///
/// As [`Unscored::NotKept`] when the score cannot be written, or the
/// output came to hold one since it was opened: nothing of `score` is
/// kept then.
pub fn keep(scoreable: &ScoreableTrial, mut score: Score) -> Result<(), Unscored> {
    let mut text = serde_json::to_string_pretty(&score)
        .context("serializing the score")
        .map_err(Unscored::Unexpected)?;
    text.push('\n');
    let own = score
        .session
        .record
        .file_name()
        .context("the scorer's record has a name")
        .map_err(Unscored::Unexpected)?
        .to_string_lossy()
        .into_owned();
    let path = scoreable.dir.join(SCORE);
    match write_whole_new(&path, &scoreable.dir.join(format!(".{SCORE}-{own}")), text) {
        Ok(()) => {
            score.record.kept = true;
            Ok(())
        }
        Err(e) => Err(Unscored::NotKept(e)),
    }
}
