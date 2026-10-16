use std::fmt::{self, Write as _};
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::domain::{Number, Timestamp};
use crate::reconcile::{DoneRun, WorkKind};
use crate::records;
use crate::status::{Paint, utc};
use crate::workflow::WorkflowName;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RunOf {
    Work { item: Number, stage: WorkKind },
    Workflow { workflow: WorkflowName },
}

impl RunOf {
    pub fn item(&self) -> Option<Number> {
        match self {
            RunOf::Work { item, .. } => Some(*item),
            RunOf::Workflow { .. } => None,
        }
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
                let workflow = head.parse()?;
                (RunOf::Workflow { workflow }, tail)
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
            RunOf::Workflow { workflow } => write!(f, "{workflow}/{}", self.n),
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
            RunOf::Workflow { workflow } => self.dir().join("workflows").join(workflow.to_string()),
        }
    }

    pub(crate) fn begin(&self, of: RunOf, now: Timestamp) -> Result<Running> {
        let dir = self.dir_of(&of);
        std::fs::create_dir_all(&dir).context("creating the runs' directory")?;
        let prefix = file_prefix(&of);
        let mut n = numbers(&dir, &prefix).max().unwrap_or(0) + 1;
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

    /// A record that does not read is logged and taken as missing.
    pub fn read_run(&self, id: &RunId) -> Option<Recorded> {
        let path = self
            .dir_of(&id.of)
            .join(format!("{}{}.json", file_prefix(&id.of), id.n));
        let record = records::load::<Record>(&path, "a run's record")?;
        Some(Recorded::of(record, &path, &self.work_dir))
    }

    /// A record that does not read is logged and skipped.
    pub fn read(&self) -> Result<Vec<Recorded>> {
        let mut recorded = vec![];
        for kind in ["items", "workflows"] {
            for dir in entries(&self.dir().join(kind))? {
                for path in entries(&dir)? {
                    if path.extension().is_some_and(|e| e == "json")
                        && let Some(record) = records::load::<Record>(&path, "a run's record")
                    {
                        recorded.push(Recorded::of(record, &path, &self.work_dir));
                    }
                }
            }
        }
        Ok(recorded)
    }
}

fn file_prefix(of: &RunOf) -> String {
    match of {
        RunOf::Work { stage, .. } => format!("{}-", stage.name()),
        RunOf::Workflow { .. } => String::new(),
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
            quiet_seconds: self
                .last_output
                .map(|last| (now - last).num_seconds().max(0)),
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
        let quiet = self
            .quiet_seconds
            .map_or("no session yet".into(), |seconds| {
                format!("quiet for {}", duration(seconds))
            });
        format!(
            "{}: {}, running for {}, {quiet}",
            paint.bold(&self.id.to_string()),
            describe_of(&self.id.of),
            duration(self.duration_seconds)
        )
    }
}

fn describe_of(of: &RunOf) -> String {
    match of {
        RunOf::Work { item, stage } => format!("{item} {}", stage.name()),
        RunOf::Workflow { workflow } => format!("workflow {workflow}"),
    }
}

fn duration(seconds: i64) -> String {
    let duration = std::time::Duration::from_secs(seconds.max(0).unsigned_abs());
    humantime::format_duration(duration).to_string()
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
    fn of(record: Record, path: &Path, work_dir: &Path) -> Recorded {
        let state = match record.end {
            Some(End { at, ending }) => State::Ended { at, ending },
            None if is_locked(&path.with_extension("lock")) => State::InFlight,
            None => State::Interrupted,
        };
        Recorded {
            id: RunId {
                of: record.of,
                n: record.n,
            },
            started_at: record.started_at,
            state,
            sessions: record
                .sessions
                .iter()
                .map(|dir| work_dir.join(dir))
                .collect(),
            answered_until: record.answered_until,
        }
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
    pub started_at: Timestamp,
    pub outcome: Outcome,
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
    runs.sort_by(|a, b| {
        (b.started_at, b.id.n)
            .cmp(&(a.started_at, a.id.n))
            .then_with(|| b.id.to_string().cmp(&a.id.to_string()))
    });
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
    let runs = if left_out == 1 { "run" } else { "runs" };
    format!("left out: {left_out} earlier {runs}; show them with {how}")
}

pub(crate) fn render_run(run: &Run, paint: Paint) -> String {
    let (outcome, seconds) = match run.outcome {
        Outcome::Ended { ending, seconds } => {
            let ending = match ending {
                Ending::Done => "done".into(),
                Ending::HandedBack => paint.red("handed back"),
                Ending::Cancelled => "cancelled".into(),
                Ending::UsageLimit => paint.yellow("ended for the usage limit"),
                Ending::Failed => paint.red("failed"),
            };
            (ending, Some(seconds))
        }
        Outcome::InFlight { seconds } => (paint.bold("in flight"), Some(seconds)),
        Outcome::Interrupted => (paint.yellow("interrupted"), None),
    };
    let ran = seconds.map_or(String::new(), |seconds| {
        format!(", ran {}", duration(seconds))
    });
    format!(
        "{}: {outcome} — {}, started {}{ran}",
        paint.bold(&run.id.to_string()),
        describe_of(&run.id.of),
        utc(run.started_at)
    )
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
