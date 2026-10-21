use std::cmp::Ordering;
use std::fmt::{self, Write as _};
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::automation::AutomationName;
use crate::domain::{Number, Timestamp};
use crate::phrase::{Phrase, Tone};
use crate::reconcile::{DoneRun, OLD_DESIGN, WorkKind};
use crate::records;
use crate::status::Paint;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RunOf {
    Work {
        item: Number,
        stage: WorkKind,
    },
    Automation {
        // The name in records written before the rename, as with
        // `OLD_AUTOMATIONS`.
        #[serde(alias = "workflow")]
        automation: AutomationName,
    },
}

impl RunOf {
    pub fn item(&self) -> Option<Number> {
        match self {
            RunOf::Work { item, .. } => Some(*item),
            RunOf::Automation { .. } => None,
        }
    }
}

/// How an automation's run started: on its schedule or by hand. Not part of
/// which run it is, so kept off [`RunOf`] and [`RunId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartedBy {
    Schedule,
    Hand,
}

/// What a run about to begin is of, and for an automation's run how it
/// started, which an item's run has no say in.
#[derive(Clone, Debug)]
pub(crate) enum Begin {
    Work {
        item: Number,
        stage: WorkKind,
    },
    Automation {
        automation: AutomationName,
        started_by: StartedBy,
    },
}

impl Begin {
    fn into_parts(self) -> (RunOf, Option<StartedBy>) {
        match self {
            Begin::Work { item, stage } => (RunOf::Work { item, stage }, None),
            Begin::Automation {
                automation,
                started_by,
            } => (RunOf::Automation { automation }, Some(started_by)),
        }
    }
}

/// A run's ID with, of an automation's run, how it started; an item's run
/// says nothing of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "StartedFields", try_from = "StartedFields")]
pub struct Started {
    id: RunId,
    by: Option<StartedBy>,
}

impl Started {
    /// An automation's run that says nothing of how it started, as those from
    /// before runs did, reads as scheduled: runs by hand from then cannot be
    /// told apart.
    pub fn new(id: RunId, by: Option<StartedBy>) -> Result<Started, String> {
        let by = match (&id.of, by) {
            (RunOf::Work { .. }, None) => None,
            (RunOf::Work { .. }, Some(_)) => {
                return Err(format!(
                    "the run {id} is an item's, which does not say how it started"
                ));
            }
            (RunOf::Automation { .. }, by) => Some(by.unwrap_or(StartedBy::Schedule)),
        };
        Ok(Started { id, by })
    }

    pub fn id(&self) -> &RunId {
        &self.id
    }

    /// `Some` for an automation's run, `None` for an item's.
    pub fn by(&self) -> Option<StartedBy> {
        self.by
    }

    pub fn by_hand(&self) -> bool {
        self.by == Some(StartedBy::Hand)
    }
}

#[derive(Serialize, Deserialize)]
struct StartedFields {
    run: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_by: Option<StartedBy>,
}

impl From<Started> for StartedFields {
    fn from(started: Started) -> StartedFields {
        StartedFields {
            run: started.id,
            started_by: started.by,
        }
    }
}

impl TryFrom<StartedFields> for Started {
    type Error = String;

    fn try_from(fields: StartedFields) -> Result<Started, String> {
        Started::new(fields.run, fields.started_by)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunId {
    pub of: RunOf,
    pub n: u32,
}

impl std::str::FromStr for RunId {
    type Err = String;

    fn from_str(id: &str) -> Result<RunId, String> {
        let not_one = || format!("{id:?} is not a run ID");
        let (head, tail) = id.split_once('/').ok_or_else(not_one)?;
        let (of, n) = match tail.rsplit_once('-') {
            Some((stage, n)) => {
                let item = head.parse().map(Number).map_err(|_| not_one())?;
                let stage = stage.parse()?;
                (RunOf::Work { item, stage }, n)
            }
            None => {
                let automation = head.parse()?;
                (RunOf::Automation { automation }, tail)
            }
        };
        let n = n.parse().map_err(|_| not_one())?;
        Ok(RunId { of, n })
    }
}

impl Serialize for RunId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RunId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<RunId, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.of {
            RunOf::Work { item, stage } => write!(f, "{}/{}-{}", item.0, stage.name(), self.n),
            RunOf::Automation { automation } => write!(f, "{automation}/{}", self.n),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ending {
    Done,
    HandedBack,
    Cancelled,
    UsageLimit,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    #[serde(flatten)]
    of: RunOf,
    n: u32,
    /// Of an automation's run; absent from those recorded before it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_by: Option<StartedBy>,
    started_at: Timestamp,
    end: Option<End>,
    /// Relative to the work directory.
    sessions: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    answered_until: Option<Timestamp>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct End {
    at: Timestamp,
    ending: Ending,
}

#[derive(Clone, Debug)]
pub struct Runs {
    work_dir: PathBuf,
}

impl Runs {
    pub fn new(work_dir: &Path) -> Runs {
        Runs {
            work_dir: work_dir.to_path_buf(),
        }
    }

    fn dir(&self) -> PathBuf {
        self.work_dir.join("runs")
    }

    fn dir_of(&self, of: &RunOf) -> PathBuf {
        match of {
            RunOf::Work { item, .. } => self.dir().join("items").join(item.0.to_string()),
            RunOf::Automation { automation } => {
                self.dir().join(AUTOMATIONS).join(automation.to_string())
            }
        }
    }

    /// Where an earlier version kept the runs of `of`, if anywhere else.
    fn old_place_of(&self, of: &RunOf) -> Option<OldPlace> {
        match of {
            RunOf::Work {
                stage: WorkKind::Design,
                ..
            } => Some(OldPlace {
                dir: self.dir_of(of),
                prefix: format!("{OLD_DESIGN}-"),
            }),
            RunOf::Work { .. } => None,
            RunOf::Automation { automation } => Some(OldPlace {
                dir: self
                    .dir()
                    .join(OLD_AUTOMATIONS)
                    .join(automation.to_string()),
                prefix: file_prefix(of),
            }),
        }
    }

    pub(crate) fn begin(&self, begin: Begin, now: Timestamp) -> Result<Running> {
        let (of, started_by) = begin.into_parts();
        let dir = self.dir_of(&of);
        std::fs::create_dir_all(&dir).context("creating the runs' directory")?;
        let prefix = file_prefix(&of);
        let old = self.old_place_of(&of);
        let mut n = numbers(&dir, &prefix)
            .chain(old.iter().flat_map(|old| numbers(&old.dir, &old.prefix)))
            .max()
            .unwrap_or(0)
            + 1;
        // Creating the lock reserves the number, against another process
        // numbering a run at the same time.
        let (lock, lock_path) = loop {
            let path = dir.join(format!("{prefix}{n}.lock"));
            match File::options().write(true).create_new(true).open(&path) {
                Ok(file) => break (file, path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => {
                    return Err(e).with_context(|| format!("creating {}", path.display()));
                }
            }
        };
        lock.try_lock()
            .with_context(|| format!("locking {}", lock_path.display()))?;
        let running = Running {
            id: RunId { of: of.clone(), n },
            path: lock_path.with_extension("json"),
            lock_path,
            work_dir: self.work_dir.clone(),
            record: Mutex::new(Record {
                of,
                n,
                started_by,
                started_at: now,
                end: None,
                sessions: vec![],
                answered_until: None,
            }),
            activity: Activity::default(),
            _lock: lock,
        };
        running.save()?;
        Ok(running)
    }

    fn path_of(&self, id: &RunId) -> PathBuf {
        let path = self
            .dir_of(&id.of)
            .join(format!("{}{}.json", file_prefix(&id.of), id.n));
        match self.old_place_of(&id.of) {
            Some(old) if !path.exists() && old.path(id.n).exists() => old.path(id.n),
            _ => path,
        }
    }

    /// A record that does not read is logged and taken as missing.
    pub fn read_run(&self, id: &RunId) -> Option<Recorded> {
        self.load(&self.path_of(id))
    }

    /// `None` if the run has no record; a record that does not read fails.
    pub fn try_read_run(&self, id: &RunId) -> Result<Option<Recorded>> {
        let path = self.path_of(id);
        let record = records::read::<Record>(&path)?;
        record
            .map(|record| Recorded::of(record, &path, &self.work_dir))
            .transpose()
            .map_err(anyhow::Error::msg)
    }

    /// A record that does not read is logged and taken as missing.
    fn load(&self, path: &Path) -> Option<Recorded> {
        let record = records::load::<Record>(path, "a run's record")?;
        Recorded::of(record, path, &self.work_dir)
            .inspect_err(|e| warn!(path = %path.display(), error = e, "ignoring a run's record"))
            .ok()
    }

    /// A record that does not read is logged and skipped.
    /// As [`Runs::read`], with a failure to read logged and taken as no
    /// runs, for an answer that does without them.
    pub fn read_or_none(&self) -> Vec<Recorded> {
        self.read().unwrap_or_else(|e| {
            warn!(error = format!("{e:#}"), "reading the runs' records failed");
            vec![]
        })
    }

    pub fn read(&self) -> Result<Vec<Recorded>> {
        let mut recorded = vec![];
        for kind in ["items", AUTOMATIONS, OLD_AUTOMATIONS] {
            for dir in entries(&self.dir().join(kind))? {
                for path in entries(&dir)? {
                    if path.extension().is_some_and(|e| e == "json")
                        && let Some(record) = self.load(&path)
                    {
                        recorded.push(record);
                    }
                }
            }
        }
        Ok(recorded)
    }
}

const AUTOMATIONS: &str = "automations";

/// Where versions before automations were renamed from workflows kept
/// automations' runs: read beside [`AUTOMATIONS`], never written to.
const OLD_AUTOMATIONS: &str = "workflows";

/// Where an earlier version kept a run's records: their directory and the
/// prefix of their files.
struct OldPlace {
    dir: PathBuf,
    prefix: String,
}

impl OldPlace {
    fn path(&self, n: u32) -> PathBuf {
        self.dir.join(format!("{}{n}.json", self.prefix))
    }
}

fn file_prefix(of: &RunOf) -> String {
    match of {
        RunOf::Work { stage, .. } => format!("{}-", stage.name()),
        RunOf::Automation { .. } => String::new(),
    }
}

fn numbers(dir: &Path, prefix: &str) -> impl Iterator<Item = u32> {
    entries(dir)
        .unwrap_or_default()
        .into_iter()
        .filter_map(move |path| {
            let stem = path.file_stem()?.to_str()?;
            stem.strip_prefix(prefix)?.parse().ok()
        })
}

fn entries(dir: &Path) -> Result<Vec<PathBuf>> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|entry| Ok(entry?.path()))
            .collect::<std::io::Result<_>>()
            .with_context(|| format!("reading {}", dir.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e).with_context(|| format!("reading {}", dir.display())),
    }
}

/// When a run's agent last produced output: `None` until the run starts a
/// session, whose start counts as its first.
#[derive(Clone, Debug, Default)]
pub struct Activity(Arc<Mutex<Option<Timestamp>>>);

impl Activity {
    pub fn note(&self, at: Timestamp) {
        let mut last = self.0.lock().unwrap();
        *last = Some(last.map_or(at, |last| last.max(at)));
    }

    pub fn last(&self) -> Option<Timestamp> {
        *self.0.lock().unwrap()
    }
}

#[derive(Clone, Debug)]
pub struct Watched {
    pub id: RunId,
    pub started_at: Timestamp,
    pub activity: Activity,
}

impl Watched {
    pub fn read(&self) -> InFlight {
        InFlight {
            id: self.id.clone(),
            started_at: self.started_at,
            last_output: self.activity.last(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InFlight {
    pub id: RunId,
    pub started_at: Timestamp,
    pub last_output: Option<Timestamp>,
}

impl InFlight {
    pub fn at(&self, now: Timestamp) -> RunInFlight {
        RunInFlight {
            id: self.id.clone(),
            started_at: self.started_at,
            duration_seconds: (now - self.started_at).num_seconds(),
            // Both counted in whole seconds from the start, so that when the
            // quiet time began, derived from them, holds still as time passes.
            quiet_seconds: self.last_output.map(|last| {
                let ran = (now - self.started_at).num_seconds();
                (ran - (last - self.started_at).num_seconds()).max(0)
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "RunInFlightAnswer", try_from = "RunInFlightAnswer")]
pub struct RunInFlight {
    pub id: RunId,
    pub started_at: Timestamp,
    pub duration_seconds: i64,
    /// `None` until it starts a session.
    pub quiet_seconds: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct RunInFlightAnswer {
    id: String,
    #[serde(flatten)]
    of: RunOf,
    started_at: Timestamp,
    duration_seconds: i64,
    quiet_seconds: Option<i64>,
}

impl From<RunInFlight> for RunInFlightAnswer {
    fn from(run: RunInFlight) -> RunInFlightAnswer {
        RunInFlightAnswer {
            id: run.id.to_string(),
            of: run.id.of,
            started_at: run.started_at,
            duration_seconds: run.duration_seconds,
            quiet_seconds: run.quiet_seconds,
        }
    }
}

impl TryFrom<RunInFlightAnswer> for RunInFlight {
    type Error = String;

    fn try_from(answer: RunInFlightAnswer) -> Result<RunInFlight, String> {
        let id: RunId = answer.id.parse()?;
        if id.of != answer.of {
            return Err(format!("the run {id} is not of what the answer says"));
        }
        Ok(RunInFlight {
            id,
            started_at: answer.started_at,
            duration_seconds: answer.duration_seconds,
            quiet_seconds: answer.quiet_seconds,
        })
    }
}

impl RunInFlight {
    pub(crate) fn render(&self, paint: Paint) -> String {
        format!("{}: {}", paint.bold(&self.id), self.describe())
    }

    /// What the run is of, how long it has run and its quiet time.
    pub(crate) fn describe(&self) -> Phrase {
        let now = self.started_at + chrono::TimeDelta::seconds(self.duration_seconds);
        let phrase = describe_of(&self.id.of)
            .text(", running for ")
            .duration(non_negative(self.duration_seconds), Some(self.started_at))
            .text(", ");
        match self.quiet_seconds {
            None => phrase.text("no session yet"),
            Some(seconds) => phrase.text("quiet for ").duration(
                non_negative(seconds),
                Some(now - chrono::TimeDelta::seconds(seconds)),
            ),
        }
    }
}

/// A duration measured on clocks that may disagree, as 0 when they make it
/// negative.
fn non_negative(seconds: i64) -> u64 {
    u64::try_from(seconds).unwrap_or(0)
}

fn describe_of(of: &RunOf) -> Phrase {
    match of {
        RunOf::Work { item, stage } => Phrase::from(*item).text(format!(" {}", stage.name())),
        RunOf::Automation { automation } => format!("automation {automation}").into(),
    }
}

/// A run dropped without ending, as when its process dies, reads as
/// interrupted.
pub(crate) struct Running {
    id: RunId,
    path: PathBuf,
    lock_path: PathBuf,
    work_dir: PathBuf,
    record: Mutex<Record>,
    activity: Activity,
    /// Locked while the run is in flight. The kernel releases the lock when
    /// the process ends however it ends.
    _lock: File,
}

impl Running {
    pub fn id(&self) -> &RunId {
        &self.id
    }

    pub fn watched(&self) -> Watched {
        Watched {
            id: self.id.clone(),
            started_at: self.record.lock().unwrap().started_at,
            activity: self.activity.clone(),
        }
    }

    pub fn activity(&self) -> &Activity {
        &self.activity
    }

    pub fn session(&self, dir: &Path, now: Timestamp) {
        self.activity.note(now);
        let dir = dir.strip_prefix(&self.work_dir).unwrap_or(dir);
        self.record.lock().unwrap().sessions.push(dir.to_path_buf());
        self.save_or_log();
    }

    pub fn answered(&self, at: Timestamp) {
        self.record.lock().unwrap().answered_until = Some(at);
        self.save_or_log();
    }

    pub fn end(self, now: Timestamp, ending: Ending) {
        self.record.lock().unwrap().end = Some(End { at: now, ending });
        self.save_or_log();
        if let Err(e) = std::fs::remove_file(&self.lock_path) {
            warn!(path = %self.lock_path.display(), error = %e, "removing a run's lock failed");
        }
    }

    fn save(&self) -> Result<()> {
        let record = self.record.lock().unwrap().clone();
        records::save(&self.path, &record, "a run's record")
    }

    fn save_or_log(&self) {
        if let Err(e) = self.save() {
            warn!(run = %self.id, error = format!("{e:#}"), "recording the run failed");
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub id: RunId,
    /// Set with `id`, from a [`Started`], so that the two agree.
    started_by: Option<StartedBy>,
    pub started_at: Timestamp,
    pub state: State,
    /// In the order it started them.
    pub sessions: Vec<PathBuf>,
    /// For a rework, when it last read the feedback it answered.
    pub answered_until: Option<Timestamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ended { at: Timestamp, ending: Ending },
    InFlight,
    Interrupted,
}

impl Recorded {
    pub fn new(
        started: Started,
        started_at: Timestamp,
        state: State,
        sessions: Vec<PathBuf>,
        answered_until: Option<Timestamp>,
    ) -> Recorded {
        Recorded {
            id: started.id,
            started_by: started.by,
            started_at,
            state,
            sessions,
            answered_until,
        }
    }

    pub fn started(&self) -> Started {
        Started {
            id: self.id.clone(),
            by: self.started_by,
        }
    }

    fn of(record: Record, path: &Path, work_dir: &Path) -> Result<Recorded, String> {
        let state = match record.end {
            Some(End { at, ending }) => State::Ended { at, ending },
            None if is_locked(&path.with_extension("lock")) => State::InFlight,
            None => State::Interrupted,
        };
        let id = RunId {
            of: record.of,
            n: record.n,
        };
        Ok(Recorded::new(
            Started::new(id, record.started_by)?,
            record.started_at,
            state,
            record
                .sessions
                .iter()
                .map(|dir| work_dir.join(dir))
                .collect(),
            record.answered_until,
        ))
    }

    pub fn done_on_pull(&self) -> Option<DoneRun> {
        match (&self.id.of, self.state) {
            (
                RunOf::Work { item, stage },
                State::Ended {
                    at,
                    ending: Ending::Done,
                },
            ) if stage.is_on_pull() => Some(DoneRun {
                pr: *item,
                kind: *stage,
                done_at: self.answered_until.unwrap_or(at),
            }),
            _ => None,
        }
    }
}

fn is_locked(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    matches!(file.try_lock(), Err(TryLockError::WouldBlock))
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    pub item: Option<Number>,
    pub stage: Option<WorkKind>,
    pub since: Option<Timestamp>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// Newest first.
    pub runs: Vec<Run>,
    pub left_out: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "RunAnswer", try_from = "RunAnswer")]
pub struct Run {
    pub id: RunId,
    /// Set with `id`, from a [`Started`], so that the two agree.
    started_by: Option<StartedBy>,
    pub started_at: Timestamp,
    pub outcome: Outcome,
}

impl Run {
    pub fn started(&self) -> Started {
        Started {
            id: self.id.clone(),
            by: self.started_by,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ended { ending: Ending, seconds: i64 },
    InFlight { seconds: i64 },
    Interrupted,
}

#[derive(Serialize, Deserialize)]
struct RunAnswer {
    id: String,
    #[serde(flatten)]
    of: RunOf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_by: Option<StartedBy>,
    started_at: Timestamp,
    duration_seconds: Option<i64>,
    outcome: OutcomeName,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(untagged)]
enum OutcomeName {
    Ended(Ending),
    Unended(Unended),
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Unended {
    InFlight,
    Interrupted,
}

impl From<Run> for RunAnswer {
    fn from(run: Run) -> RunAnswer {
        let (outcome, duration_seconds) = match run.outcome {
            Outcome::Ended { ending, seconds } => (OutcomeName::Ended(ending), Some(seconds)),
            Outcome::InFlight { seconds } => {
                (OutcomeName::Unended(Unended::InFlight), Some(seconds))
            }
            Outcome::Interrupted => (OutcomeName::Unended(Unended::Interrupted), None),
        };
        RunAnswer {
            id: run.id.to_string(),
            of: run.id.of,
            started_by: run.started_by,
            started_at: run.started_at,
            duration_seconds,
            outcome,
        }
    }
}

impl TryFrom<RunAnswer> for Run {
    type Error = String;

    fn try_from(answer: RunAnswer) -> Result<Run, String> {
        let id: RunId = answer.id.parse()?;
        if id.of != answer.of {
            return Err(format!("the run {id} is not of what the answer says"));
        }
        let Started { id, by: started_by } = Started::new(id, answer.started_by)?;
        let outcome = match (answer.outcome, answer.duration_seconds) {
            (OutcomeName::Ended(ending), Some(seconds)) => Outcome::Ended { ending, seconds },
            (OutcomeName::Unended(Unended::InFlight), Some(seconds)) => {
                Outcome::InFlight { seconds }
            }
            (OutcomeName::Unended(Unended::Interrupted), None) => Outcome::Interrupted,
            _ => {
                return Err(format!(
                    "the run {id} has a duration only if it is not interrupted"
                ));
            }
        };
        Ok(Run {
            id,
            started_by,
            started_at: answer.started_at,
            outcome,
        })
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0} has no runs")]
pub struct NoRuns(pub Number);

pub fn of_item(recorded: &[Recorded], number: Number) -> Vec<Recorded> {
    recorded
        .iter()
        .filter(|run| run.id.of.item() == Some(number))
        .cloned()
        .collect()
}

/// An automation's last run: its newest, by its schedule or by hand, and how
/// it started. The one place that says which run that is.
pub fn last_of_automation<'a>(
    recorded: &'a [Recorded],
    name: &AutomationName,
) -> Option<(&'a Recorded, StartedBy)> {
    recorded
        .iter()
        .filter_map(|run| match (&run.id.of, run.started_by) {
            (RunOf::Automation { automation }, Some(by)) if automation == name => Some((run, by)),
            _ => None,
        })
        .min_by(|(a, _), (b, _)| newest_first(a, b))
}

/// Asking for an item's runs when it has none is an error, however the rest
/// of the query narrows them.
pub fn list(recorded: Vec<Recorded>, query: &Query, now: Timestamp) -> Result<Listing, NoRuns> {
    let mut runs: Vec<Recorded> = match query.item {
        Some(number) => {
            let of_item = of_item(&recorded, number);
            if of_item.is_empty() {
                return Err(NoRuns(number));
            }
            of_item
        }
        None => recorded,
    };
    runs.retain(|run| {
        query
            .stage
            .is_none_or(|wanted| matches!(run.id.of, RunOf::Work { stage, .. } if stage == wanted))
            && query.since.is_none_or(|since| run.started_at >= since)
    });
    sort_newest_first(&mut runs);
    let left_out = query
        .limit
        .map_or(0, |limit| runs.len().saturating_sub(limit));
    runs.truncate(runs.len() - left_out);
    Ok(Listing {
        runs: runs.into_iter().map(|run| run.at(now)).collect(),
        left_out,
    })
}

pub(crate) fn sort_newest_first(runs: &mut [Recorded]) {
    runs.sort_by(newest_first);
}

fn newest_first(a: &Recorded, b: &Recorded) -> Ordering {
    (b.started_at, b.id.n)
        .cmp(&(a.started_at, a.id.n))
        .then_with(|| b.id.to_string().cmp(&a.id.to_string()))
}

impl Recorded {
    pub(crate) fn at(self, now: Timestamp) -> Run {
        let seconds = |until: Timestamp| (until - self.started_at).num_seconds();
        let outcome = match self.state {
            State::Ended { at, ending } => Outcome::Ended {
                ending,
                seconds: seconds(at),
            },
            State::InFlight => Outcome::InFlight {
                seconds: seconds(now),
            },
            State::Interrupted => Outcome::Interrupted,
        };
        Run {
            id: self.id,
            started_by: self.started_by,
            started_at: self.started_at,
            outcome,
        }
    }
}

pub fn render(listing: &Listing, colour: bool) -> String {
    let paint = Paint(colour);
    let mut text = String::new();
    for run in &listing.runs {
        let _ = writeln!(text, "{}", render_run(run, paint));
    }
    if listing.left_out > 0 {
        let shown = listing.runs.len() + listing.left_out;
        let _ = writeln!(
            text,
            "{}",
            render_left_out(listing.left_out, &format!("--limit {shown}"))
        );
    }
    text
}

pub(crate) fn render_left_out(left_out: usize, how: &str) -> String {
    format!("left out: {}", left_out_phrase(left_out, how))
}

/// How many earlier runs a list left out, and the command that shows them.
pub(crate) fn left_out_phrase(left_out: usize, how: &str) -> Phrase {
    format!("{}; show them with {how}", earlier_runs(left_out)).into()
}

/// How many earlier runs a list left out.
pub(crate) fn earlier_runs(left_out: usize) -> String {
    let runs = if left_out == 1 { "run" } else { "runs" };
    format!("{left_out} earlier {runs}")
}

impl Outcome {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Outcome::Ended { ending, .. } => match ending {
                Ending::Done => "done",
                Ending::HandedBack => "handed back",
                Ending::Cancelled => "cancelled",
                Ending::UsageLimit => "ended for the usage limit",
                Ending::Failed => "failed",
            },
            Outcome::InFlight { .. } => "in flight",
            Outcome::Interrupted => "interrupted",
        }
    }

    /// Whether the run failed at its work, as opposed to being stopped.
    pub(crate) fn failed(self) -> bool {
        matches!(
            self,
            Outcome::Ended {
                ending: Ending::Failed | Ending::HandedBack,
                ..
            }
        )
    }

    pub(crate) fn tone(self) -> Tone {
        match self {
            Outcome::Ended { ending, .. } => match ending {
                Ending::Done => Tone::Success,
                Ending::HandedBack | Ending::Failed => Tone::Failure,
                Ending::Cancelled | Ending::UsageLimit => Tone::Cancelled,
            },
            Outcome::InFlight { .. } => Tone::InProgress,
            Outcome::Interrupted => Tone::Cancelled,
        }
    }
}

pub(crate) fn render_run(run: &Run, paint: Paint) -> String {
    let word = run.outcome.word();
    let outcome = match run.outcome {
        Outcome::Ended { ending, .. } => match ending {
            Ending::Done | Ending::Cancelled => word.into(),
            Ending::HandedBack | Ending::Failed => paint.red(word),
            Ending::UsageLimit => paint.yellow(word),
        },
        Outcome::InFlight { .. } => paint.bold(word),
        Outcome::Interrupted => paint.yellow(word),
    };
    format!("{}: {outcome} — {}", paint.bold(&run.id), describe_run(run))
}

/// What a run is of, whether by hand, when it started and how long it ran,
/// after its outcome.
pub(crate) fn describe_run(run: &Run) -> Phrase {
    let phrase =
        describe_of(&run.id.of).then(describe_start(run.started().by_hand(), run.started_at));
    match run.outcome {
        Outcome::Ended { seconds, .. } => {
            phrase.text(", ran ").duration(non_negative(seconds), None)
        }
        Outcome::InFlight { seconds } => phrase
            .text(", ran ")
            .duration(non_negative(seconds), Some(run.started_at)),
        Outcome::Interrupted => phrase,
    }
}

/// Whether a run was by hand and when it started, after what it is of or its
/// ID.
pub(crate) fn describe_start(by_hand: bool, started_at: Timestamp) -> Phrase {
    let by_hand = if by_hand { " by hand" } else { "" };
    Phrase::from(by_hand).text(", started ").time(started_at)
}

#[derive(Debug, thiserror::Error)]
pub enum ListError {
    #[error(transparent)]
    NoRuns(#[from] NoRuns),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub fn answer(work_dir: &RepoWorkDir, query: &Query, now: Timestamp) -> Result<Listing, ListError> {
    let recorded = Runs::new(&work_dir.resolve()).read()?;
    Ok(list(recorded, query, now)?)
}

#[derive(Clone, Debug)]
pub struct RepoWorkDir {
    pub base: PathBuf,
    pub repo: String,
}

impl RepoWorkDir {
    /// Falls back to a directory spelled otherwise, as GitHub names
    /// repositories case-insensitively. Found afresh each time, as `serve`
    /// may create it after it starts.
    pub fn resolve(&self) -> PathBuf {
        work_dir(&self.base, &self.repo)
    }
}

fn work_dir(base: &Path, repo: &str) -> PathBuf {
    let exact = base.join(repo);
    if exact.exists() {
        return exact;
    }
    let spelled = |dir: &Path, name: &str| {
        entries(dir).unwrap_or_default().into_iter().find(|path| {
            path.file_name()
                .and_then(|found| found.to_str())
                .is_some_and(|found| found.eq_ignore_ascii_case(name))
        })
    };
    repo.split_once('/')
        .and_then(|(owner, name)| spelled(&spelled(base, owner)?, name))
        .unwrap_or(exact)
}
