//! The runs: each run `serve`, `once` and `workflow run` start is recorded
//! under its run ID in the work directory, from its start to its end and
//! with the sessions it started, and `lw runs` lists them
//! (`docs/specs/monitoring.md`, "runs").

use std::fmt::{self, Write as _};
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::domain::{Number, Timestamp};
use crate::reconcile::{OpenedFor, WorkKind};
use crate::records;
use crate::status::{Paint, utc};
use crate::workflow::WorkflowName;

/// What a run is of: a stage on an item, or a workflow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RunOf {
    Work { item: Number, stage: WorkKind },
    Workflow { workflow: WorkflowName },
}

impl RunOf {
    /// The item a stage's run is on; `None` for a workflow's.
    pub fn item(&self) -> Option<Number> {
        match self {
            RunOf::Work { item, .. } => Some(*item),
            RunOf::Workflow { .. } => None,
        }
    }
}

/// A run's ID: `<item>/<stage>-<n>` for the `n`th run of that stage on that
/// item, or `<workflow>/<n>` for a workflow's `n`th run.
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

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ending {
    /// It did its work, or found none left to do.
    Done,
    /// It handed the work back to the maintainer.
    HandedBack,
    /// Its subject stopped asking for it, or Linewright stopped.
    Cancelled,
    /// The runtime refused its session for the usage limit.
    UsageLimit,
    /// It failed and was not handed back: a workflow's run, which has
    /// nothing to hand back, or a run whose hand-back failed.
    Failed,
}

/// A run as its record keeps it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    #[serde(flatten)]
    of: RunOf,
    n: u32,
    started_at: Timestamp,
    /// `None` until the run ends.
    end: Option<End>,
    /// The records of the sessions it started, with their transcripts,
    /// relative to the work directory.
    sessions: Vec<PathBuf>,
    /// The pull request it works on or opened; `None` in records written
    /// before runs kept it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pr: Option<Number>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct End {
    at: Timestamp,
    ending: Ending,
}

/// The records of the runs on one repository, in its work directory.
#[derive(Clone, Debug)]
pub struct Runs {
    work_dir: PathBuf,
}

impl Runs {
    /// The records in the repository's work directory `work_dir`.
    pub fn new(work_dir: &Path) -> Runs {
        Runs {
            work_dir: work_dir.to_path_buf(),
        }
    }

    fn dir(&self) -> PathBuf {
        self.work_dir.join("runs")
    }

    /// The directory of the runs of what `of` names: `runs/items/<item>` or
    /// `runs/workflows/<workflow>`.
    fn dir_of(&self, of: &RunOf) -> PathBuf {
        match of {
            RunOf::Work { item, .. } => self.dir().join("items").join(item.0.to_string()),
            RunOf::Workflow { workflow } => self.dir().join("workflows").join(workflow.to_string()),
        }
    }

    /// Records the start, at `now`, of the next run of `of`, on the pull
    /// request `pr` if it works on one, numbered after every earlier one,
    /// and holds the run's lock until it ends.
    pub(crate) fn begin(&self, of: RunOf, pr: Option<Number>, now: Timestamp) -> Result<Running> {
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
                pr,
            }),
            activity: Activity::default(),
            _lock: lock,
        };
        running.save()?;
        Ok(running)
    }

    /// The run `id`, if it is recorded. A record that does not read is
    /// logged and taken as missing.
    pub fn read_run(&self, id: &RunId) -> Option<Recorded> {
        let path = self
            .dir_of(&id.of)
            .join(format!("{}{}.json", file_prefix(&id.of), id.n));
        let record = records::load::<Record>(&path, "a run's record")?;
        Some(Recorded::of(record, &path, &self.work_dir))
    }

    /// Every run recorded. A record that does not read is logged and
    /// skipped.
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

/// What starts the names of the files of `of`'s runs, before the number.
fn file_prefix(of: &RunOf) -> String {
    match of {
        RunOf::Work { stage, .. } => format!("{}-", stage.name()),
        RunOf::Workflow { .. } => String::new(),
    }
}

/// The numbers of the runs in `dir` whose files' names start with `prefix`.
fn numbers(dir: &Path, prefix: &str) -> impl Iterator<Item = u32> {
    entries(dir)
        .unwrap_or_default()
        .into_iter()
        .filter_map(move |path| {
            let stem = path.file_stem()?.to_str()?;
            stem.strip_prefix(prefix)?.parse().ok()
        })
}

/// The paths in `dir`, none when it does not exist.
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

/// When a run's agent last produced output, noted as `serve` hears it while
/// the run is in flight: `None` until the run starts a session, whose start
/// counts as its first.
#[derive(Clone, Debug, Default)]
pub struct Activity(Arc<Mutex<Option<Timestamp>>>);

impl Activity {
    /// Notes output at `at`, unless later output has been noted.
    pub fn note(&self, at: Timestamp) {
        let mut last = self.0.lock().unwrap();
        *last = Some(last.map_or(at, |last| last.max(at)));
    }

    pub fn last(&self) -> Option<Timestamp> {
        *self.0.lock().unwrap()
    }
}

/// A run in flight as `serve` holds it, its activity noted as it goes.
#[derive(Clone, Debug)]
pub struct Watched {
    pub id: RunId,
    pub started_at: Timestamp,
    pub activity: Activity,
}

impl Watched {
    /// The run as it stands now, its last output read afresh.
    pub fn read(&self) -> InFlight {
        InFlight {
            id: self.id.clone(),
            started_at: self.started_at,
            last_output: self.activity.last(),
        }
    }
}

/// A run in flight, as of a reading of its activity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InFlight {
    pub id: RunId,
    pub started_at: Timestamp,
    /// When its agent last produced output: `None` until it starts a
    /// session, whose start counts as its first.
    pub last_output: Option<Timestamp>,
}

impl InFlight {
    /// The run as `status` and `explain` answer it at `now`.
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

/// A run in flight as `status` and `explain` answer it: how long it has run
/// and its quiet time, how long since its agent last produced output
/// (`docs/specs/monitoring.md`, "status").
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
    /// The run as text: its run ID, what it is of, how long it has run and
    /// its quiet time, coloured with `paint`.
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

/// What a run is of, as text: `#42 rework` or `workflow weekly`.
fn describe_of(of: &RunOf) -> String {
    match of {
        RunOf::Work { item, stage } => format!("{item} {}", stage.name()),
        RunOf::Workflow { workflow } => format!("workflow {workflow}"),
    }
}

/// `seconds` as text, such as `1h 5m 3s`.
fn duration(seconds: i64) -> String {
    let duration = std::time::Duration::from_secs(seconds.max(0).unsigned_abs());
    humantime::format_duration(duration).to_string()
}

/// A run in flight, its record kept up to date until it ends. A run
/// dropped without ending, as when its process dies, reads as interrupted.
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

    /// The run as `serve` holds it while in flight.
    pub fn watched(&self) -> Watched {
        Watched {
            id: self.id.clone(),
            started_at: self.record.lock().unwrap().started_at,
            activity: self.activity.clone(),
        }
    }

    /// When the run's agent last produced output.
    pub fn activity(&self) -> &Activity {
        &self.activity
    }

    /// Links the session whose record is in `dir`, started at `now`, to the
    /// run. Its quiet time counts from the start.
    pub fn session(&self, dir: &Path, now: Timestamp) {
        self.activity.note(now);
        let dir = dir.strip_prefix(&self.work_dir).unwrap_or(dir);
        self.record.lock().unwrap().sessions.push(dir.to_path_buf());
        self.save_or_log();
    }

    /// Links the pull request `pr`, which the run opened, to the run.
    pub fn opened(&self, pr: Number) {
        self.record.lock().unwrap().pr = Some(pr);
        self.save_or_log();
    }

    /// Records the run's end, at `now`, as `ending`.
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

/// A run as its record and its lock show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub id: RunId,
    pub started_at: Timestamp,
    pub state: State,
    /// The records of the sessions it started, with their transcripts, in
    /// the order it started them.
    pub sessions: Vec<PathBuf>,
    /// The pull request it works on or opened, if it is known.
    pub pr: Option<Number>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ended {
        at: Timestamp,
        ending: Ending,
    },
    InFlight,
    /// It never ended: the process running it stopped first.
    Interrupted,
}

impl Recorded {
    /// The run `record`, at `path` in the work directory `work_dir`, in
    /// flight only while its lock is held.
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
            pr: record.pr,
        }
    }
}

/// Whether a process holds the lock at `path`.
fn is_locked(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    matches!(file.try_lock(), Err(TryLockError::WouldBlock))
}

/// Which runs `runs` lists.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    /// Only this item's.
    pub item: Option<Number>,
    /// Only the runs of this stage.
    pub stage: Option<WorkKind>,
    /// Only those started at or after.
    pub since: Option<Timestamp>,
    /// At most this many, the newest.
    pub limit: Option<usize>,
}

/// The answer to `runs`. Its fields keep their names and meanings
/// (`docs/specs/monitoring.md`, "Output").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// Newest first.
    pub runs: Vec<Run>,
    /// How many more runs the query's limit left out.
    pub left_out: usize,
}

/// A run as `runs` answers it: its run ID, with the item and stage or the
/// workflow it names beside it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "RunAnswer", try_from = "RunAnswer")]
pub struct Run {
    pub id: RunId,
    pub started_at: Timestamp,
    pub outcome: Outcome,
}

/// How a run ended and how long it ran, or how long it has run in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ended {
        ending: Ending,
        seconds: i64,
    },
    InFlight {
        seconds: i64,
    },
    /// The process running it stopped before it ended, at a time unknown.
    Interrupted,
}

#[derive(Serialize, Deserialize)]
struct RunAnswer {
    id: String,
    #[serde(flatten)]
    of: RunOf,
    started_at: Timestamp,
    /// `None` for a run interrupted.
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

/// Why `runs N` has nothing to list.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0} has no runs")]
pub struct NoRuns(pub Number);

/// The runs of `recorded` on #`number`, its own alone
/// (`docs/specs/monitoring.md`, "runs").
pub fn of_item(recorded: &[Recorded], number: Number) -> Vec<Recorded> {
    recorded
        .iter()
        .filter(|run| run.id.of.item() == Some(number))
        .cloned()
        .collect()
}

/// Which issue the runs `recorded` opened each pull request for: the item
/// of the implement run that records it, the only stage that opens one. A
/// pull request no run's record links to, as for records written before runs
/// kept their pull request, is for none.
pub fn opened_for_each(recorded: &[Recorded]) -> OpenedFor {
    OpenedFor::new(recorded.iter().filter_map(|run| match run.id.of {
        RunOf::Work {
            item,
            stage: WorkKind::Implement,
        } => run.pr.map(|pr| (pr, item)),
        _ => None,
    }))
}

/// The runs of `recorded` that `query` asks for at `now`, newest first.
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

/// Sorts `runs` newest first, as `runs` lists them.
pub(crate) fn sort_newest_first(runs: &mut [Recorded]) {
    runs.sort_by(|a, b| {
        (b.started_at, b.id.n)
            .cmp(&(a.started_at, a.id.n))
            .then_with(|| b.id.to_string().cmp(&a.id.to_string()))
    });
}

impl Recorded {
    /// The run as `runs` answers it at `now`.
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

/// The answer as text, one run per line, coloured if `colour`, ending with
/// how many runs the limit left out and how to show them
/// (`docs/specs/monitoring.md`, "Output").
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

/// The line ending a list of runs cut to a limit: how many earlier runs,
/// `left_out`, it left out, and `how` to show them
/// (`docs/specs/monitoring.md`, "Output").
pub(crate) fn render_left_out(left_out: usize, how: &str) -> String {
    let runs = if left_out == 1 { "run" } else { "runs" };
    format!("left out: {left_out} earlier {runs}; show them with {how}")
}

/// A run as `runs` lists it, coloured with `paint`.
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

/// Why `runs` has no listing to give.
#[derive(Debug, thiserror::Error)]
pub enum ListError {
    #[error(transparent)]
    NoRuns(#[from] NoRuns),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// The answer to `query` at `now` from the records in `work_dir`, as
/// `serve` gives it and as `runs` gives it with no `serve` running.
pub fn answer(work_dir: &RepoWorkDir, query: &Query, now: Timestamp) -> Result<Listing, ListError> {
    let recorded = Runs::new(&work_dir.resolve()).read()?;
    Ok(list(recorded, query, now)?)
}

/// A repository's work directory, as a monitoring command finds it from
/// its settings: `repo` under the `work-dir` setting `base`.
#[derive(Clone, Debug)]
pub struct RepoWorkDir {
    pub base: PathBuf,
    pub repo: String,
}

impl RepoWorkDir {
    /// The directory, which `serve` names as GitHub spells the repository:
    /// the one spelled as `repo` or, as GitHub names repositories
    /// case-insensitively, else one spelled otherwise. Found afresh each
    /// time, as `serve` may create it after it starts.
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
