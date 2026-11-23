pub mod files;
mod rows;

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::TimeDelta;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::automation::AutomationName;
use crate::briefs::Briefs;
use crate::cost::{RunCost, SessionCost, SessionSpend};
use crate::database::Database;
use crate::domain::{Number, Repo, Timestamp};
use crate::harness::{Purpose, Reported, Reporter};
use crate::input::Input;
use crate::layer::Layer;
use crate::phrase::{Phrase, Status};
use crate::reconcile::{EndedRun, WorkKind};
use crate::records;
use crate::session::{self, RecordsSessions};
use crate::status::Paint;
use crate::work_settings::{Effort, SessionSettings, Setting, TimeoutSetting};
use crate::workflow_run::{AttemptId, TriggeredRun};
use files::{End, Record, RecordedSession, RunFiles};
use rows::{Change, RunRow};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum RunOf {
    Work {
        item: Number,
        stage: WorkKind,
    },
    Automation {
        /// Read also from `workflow`, its key in records written before
        /// automations were renamed.
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

/// How an automation's run started: on its schedule, by hand, or by the
/// attempt of a workflow run. Not part of which run it is, so kept off
/// [`RunOf`] and [`RunId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum StartedBy {
    Schedule,
    Hand,
    WorkflowRun(AttemptId),
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

#[derive(Clone, Debug, PartialEq, Eq, schemars::JsonSchema)]
#[schemars(with = "String")]
pub struct RunId {
    pub of: RunOf,
    pub n: u32,
}

impl std::str::FromStr for RunId {
    type Err = String;

    fn from_str(id: &str) -> Result<RunId, String> {
        let not_one = || format!("{id:?} is not a run ID");
        let (head, tail) = id.rsplit_once('/').ok_or_else(not_one)?;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Ending {
    Done,
    HandedBack,
    Cancelled,
    UsageLimit,
    Failed,
}

/// Why a session ended unexpectedly, other than its agent giving up: one
/// variant for each row of docs/specs/runs.md, Ending, "Unexpected ends", in
/// its order. `Display` gives the row's reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnexpectedEnd {
    /// It reached its timeout, `after`, which `setting` gives.
    TimedOut {
        after: std::time::Duration,
        setting: TimeoutSetting,
    },
    /// Its last result was still not accepted, `why` being what was last
    /// sent back to it.
    NotAccepted { why: String },
    /// It failed on its infrastructure, and again on the retry, with `error`.
    Infrastructure { error: String },
    /// The harness ended it with the error `kind`.
    HarnessError { kind: String },
    /// The harness stopped without a result.
    NoResult,
}

impl fmt::Display for UnexpectedEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnexpectedEnd::TimedOut { after, .. } => write!(
                f,
                "it timed out after {}",
                Phrase::default().duration(after.as_secs(), None)
            ),
            UnexpectedEnd::NotAccepted { why } => write!(f, "its result was not accepted: {why}"),
            UnexpectedEnd::Infrastructure { error } => {
                write!(f, "it failed on its infrastructure: {error}")
            }
            UnexpectedEnd::HarnessError { kind } => write!(f, "the harness ended it with `{kind}`"),
            UnexpectedEnd::NoResult => f.write_str("the harness stopped without a result"),
        }
    }
}

/// A session of a run: where its records are, and what Linewright kept of
/// it.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionEntry {
    pub dir: PathBuf,
    /// `None` for a session recorded before Linewright kept what it ran
    /// with.
    pub kept: Option<SessionKept>,
}

impl SessionEntry {
    /// What it ran with, `None` for a session recorded before Linewright
    /// kept it.
    pub fn ran(&self) -> Option<&Ran> {
        self.kept.as_ref().map(|kept| &kept.ran)
    }
}

impl From<PathBuf> for SessionEntry {
    fn from(dir: PathBuf) -> SessionEntry {
        SessionEntry { dir, kept: None }
    }
}

/// What Linewright kept of a session: what it ran with, and what it cost.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionKept {
    pub ran: Ran,
    /// `None` for a session recorded before Linewright kept it.
    pub cost: Option<SessionCost>,
}

/// What the reports of a session make of what it ran with and what it
/// cost: the last model reported wins, its spend is as its turns' reports
/// make it, and its time is the one its end reports.
#[derive(Debug)]
pub(crate) struct SessionReports {
    ran: Ran,
    spend: SessionSpend,
    time: Option<std::time::Duration>,
}

impl SessionReports {
    /// A session that runs with `ran`, before any report.
    pub(crate) fn new(ran: Ran) -> SessionReports {
        SessionReports {
            ran,
            spend: SessionSpend::new(),
            time: None,
        }
    }

    pub(crate) fn reported(&mut self, reported: Reported) {
        match reported {
            Reported::Model(model) => self.ran.reported_model = Some(model),
            Reported::TurnStarted => self.spend.turn_started(),
            Reported::TurnEnded(turn) => self.spend.turn_ended(turn),
        }
    }

    /// The session ended, its session timeout having counted `time`.
    pub(crate) fn ended(&mut self, time: std::time::Duration) {
        self.time = Some(time);
    }

    pub(crate) fn ran(&self) -> &Ran {
        &self.ran
    }

    pub(crate) fn cost(&self) -> SessionCost {
        SessionCost {
            time: self.time,
            spend: self.spend.figures().cloned(),
        }
    }
}

/// What a session ran with: its model and effort as Linewright set them,
/// the model as the harness reported it ran, and where each of its settings
/// and brief files came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(from = "KeptRan")]
pub struct Ran {
    /// `None` only in a record kept before every session had a model.
    pub model: Option<String>,
    /// `None` until the harness reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_model: Option<String>,
    /// `None` only in a record kept before every session had an effort.
    pub effort: Option<Effort>,
    pub layers: Layers,
}

/// A [`Ran`] as a record holds it.
#[derive(Deserialize, schemars::JsonSchema)]
struct KeptRan {
    model: Option<String>,
    #[serde(default)]
    reported_model: Option<String>,
    effort: Option<Effort>,
    layers: Layers,
}

/// A model or effort a record does not hold has no source. A record kept
/// before every session had both gives one it does not hold a source
/// anyway, Linewright's or the harness's, which no layer was.
impl From<KeptRan> for Ran {
    fn from(kept: KeptRan) -> Ran {
        let mut layers = kept.layers;
        for (setting, set) in [
            (Setting::MODEL, kept.model.is_some()),
            (Setting::EFFORT, kept.effort.is_some()),
        ] {
            if !set {
                layers.settings.remove(&setting);
            }
        }
        Ran {
            model: kept.model,
            reported_model: kept.reported_model,
            effort: kept.effort,
            layers,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(from = "KeptLayers")]
pub struct Layers {
    pub settings: BTreeMap<Setting, Layer>,
    /// By the file's path under `briefs/`, such as `prompts/triage.md` or
    /// `plugin/skills/tdd/SKILL.md`.
    pub briefs: BTreeMap<String, Layer>,
}

/// [`Layers`] as a record holds it: its brief files under `briefs`, or
/// under `prompts` by their paths under `briefs/prompts/`.
#[derive(Deserialize, schemars::JsonSchema)]
struct KeptLayers {
    settings: BTreeMap<Setting, KeptSource>,
    briefs: Option<BTreeMap<String, Layer>>,
    prompts: Option<BTreeMap<String, Layer>>,
}

impl From<KeptLayers> for Layers {
    fn from(kept: KeptLayers) -> Layers {
        let briefs = match (kept.briefs, kept.prompts) {
            (Some(briefs), _) => briefs,
            (None, prompts) => prompts
                .into_iter()
                .flatten()
                .map(|(path, layer)| (format!("prompts/{path}"), layer))
                .collect(),
        };
        Layers {
            settings: kept
                .settings
                .into_iter()
                .filter_map(|(setting, source)| match source {
                    KeptSource::Layer(layer) => Some((setting, layer)),
                    KeptSource::Harness => None,
                })
                .collect(),
            briefs,
        }
    }
}

/// Where a setting came from, as a record holds it: a layer, or, in a
/// record kept before every session had a model and effort, `harness`,
/// for a model or effort none set.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(try_from = "String")]
enum KeptSource {
    Layer(Layer),
    Harness,
}

/// The error is `Layer`'s, naming the source and the layers.
impl TryFrom<String> for KeptSource {
    type Error = serde::de::value::Error;

    fn try_from(name: String) -> Result<KeptSource, Self::Error> {
        if name == "harness" {
            return Ok(KeptSource::Harness);
        }
        Layer::deserialize(serde::de::value::StrDeserializer::new(&name)).map(KeptSource::Layer)
    }
}

impl Ran {
    pub fn new(settings: &SessionSettings, briefs: &Briefs) -> Ran {
        Ran {
            model: Some(settings.model.value.to_string()),
            reported_model: None,
            effort: Some(settings.effort.value),
            layers: Layers {
                settings: settings.sources(),
                briefs: briefs.layers().clone(),
            },
        }
    }

    /// The model the harness reported the session ran, else the one set.
    pub fn model_ran(&self) -> Option<&str> {
        self.reported_model.as_deref().or(self.model.as_deref())
    }
}

/// The work directory's store of runs, in its database.
#[derive(Clone)]
pub struct Runs {
    db: Database,
}

/// The directory of a work directory under which a run in flight holds its
/// lock, at the path of its record in the file form of runs.
const LOCKS: &str = "locks";

impl Runs {
    pub fn new(db: Database) -> Runs {
        Runs { db }
    }

    fn work_dir(&self) -> &Path {
        self.db.work_dir()
    }

    /// The database the runs are kept in.
    pub fn database(&self) -> &Database {
        &self.db
    }

    /// Begins a run of `begin`, numbered after every run of the same item
    /// and stage, or of the same automation, with no two runs given the
    /// same number, even by two processes at once. It holds its lock, so
    /// that it reads as in flight, before any other process can read it.
    pub(crate) async fn begin(&self, begin: Begin, now: Timestamp) -> Result<Running> {
        let (of, started_by) = begin.into_parts();
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .context("beginning a run")?;
        let n = rows::next_n(&mut tx, &of).await?;
        let record = Record::begun(RunId { of, n }, started_by, now);
        let row = rows::insert(&mut tx, &record).await?;
        let lock_path = lock_path(self.work_dir(), &record.id());
        let lock = hold(&lock_path)?;
        tx.commit().await.context("beginning a run")?;
        Ok(Running::new(
            record,
            Keeping::Database(RunRow::new(self.db.clone(), row)),
            lock_path,
            self.work_dir().to_path_buf(),
            lock,
        ))
    }

    /// The input run `id` recorded at its start, `None` when it recorded
    /// none.
    pub async fn read_input(&self, id: &RunId) -> Result<Option<Input>> {
        rows::read_input(self.db.pool(), id).await
    }

    /// A record that does not read is logged and taken as missing.
    pub async fn read_run(&self, id: &RunId) -> Option<Recorded> {
        self.try_read_run(id)
            .await
            .inspect_err(|e| warn!(run = %id, error = format!("{e:#}"), "ignoring a run's record"))
            .ok()
            .flatten()
    }

    /// `None` if the run has no record; a record that does not read fails.
    /// A run with no end reads as in flight if its lock was held as its
    /// record was read: a run records its end before it lets go of its lock.
    pub async fn try_read_run(&self, id: &RunId) -> Result<Option<Recorded>> {
        let held_before = self.is_held(id);
        rows::read_one(self.db.pool(), self.work_dir(), id, |id| {
            held_before || self.is_held(id)
        })
        .await
    }

    /// As [`Runs::read`], with a failure to read logged and taken as no
    /// runs.
    pub async fn read_or_none(&self) -> Vec<Recorded> {
        self.read().await.unwrap_or_else(|e| {
            warn!(error = format!("{e:#}"), "reading the runs' records failed");
            vec![]
        })
    }

    pub async fn read(&self) -> Result<Vec<Recorded>> {
        rows::read_all(self.db.pool(), self.work_dir(), |id| self.is_held(id)).await
    }

    /// Whether the lock of run `id` is held, by this process or another.
    fn is_held(&self, id: &RunId) -> bool {
        is_locked(&lock_path(self.work_dir(), id))
    }
}

/// Where the lock of run `id` is in `work_dir`.
fn lock_path(work_dir: &Path, id: &RunId) -> PathBuf {
    RunFiles::new(&work_dir.join(LOCKS))
        .record_path(id)
        .with_extension("lock")
}

/// The lock at `path`, made if need be, held.
fn hold(path: &Path) -> Result<File> {
    std::fs::create_dir_all(path.parent().expect("a lock is in a directory"))
        .context("creating the runs' locks' directory")?;
    let lock = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    lock.try_lock()
        .with_context(|| format!("locking {}", path.display()))?;
    Ok(lock)
}

/// Whether the lock at `path` is held, by this process or another.
fn is_locked(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    matches!(file.try_lock(), Err(TryLockError::WouldBlock))
}

/// What `work` gives, done on a thread where blocking is allowed: once
/// begun, it runs to its end even if the caller is dropped. A panic in it
/// goes on in the caller. It never gives anything when the runtime shuts
/// down before `work` begins.
async fn off_runtime<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    to_the_end(tokio::task::spawn_blocking(work)).await
}

/// What the task `task` gives. A panic in it goes on in the caller. It never
/// gives anything when the runtime shuts down before `task` ends.
async fn to_the_end<T>(task: tokio::task::JoinHandle<T>) -> T {
    match task.await {
        Ok(done) => done,
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        Err(_) => std::future::pending().await,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InFlight {
    pub id: RunId,
    pub started_at: Timestamp,
}

impl InFlight {
    pub fn at(&self, now: Timestamp) -> RunInFlight {
        RunInFlight {
            id: self.id.clone(),
            started_at: self.started_at,
            duration_seconds: (now - self.started_at).num_seconds(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "RunInFlightAnswer", try_from = "RunInFlightAnswer")]
pub struct RunInFlight {
    pub id: RunId,
    pub started_at: Timestamp,
    pub duration_seconds: i64,
}

#[derive(Serialize, Deserialize)]
struct RunInFlightAnswer {
    id: String,
    #[serde(flatten)]
    of: RunOf,
    started_at: Timestamp,
    duration_seconds: i64,
}

impl From<RunInFlight> for RunInFlightAnswer {
    fn from(run: RunInFlight) -> RunInFlightAnswer {
        RunInFlightAnswer {
            id: run.id.to_string(),
            of: run.id.of,
            started_at: run.started_at,
            duration_seconds: run.duration_seconds,
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
        })
    }
}

impl RunInFlight {
    pub(crate) fn render(&self, paint: Paint) -> String {
        format!("{}: {}", paint.bold(&self.id), self.describe())
    }

    /// What the run is of and how long it has run.
    pub(crate) fn describe(&self) -> Phrase {
        describe_of(&self.id.of).text(", running for ").span(
            TimeDelta::seconds(self.duration_seconds),
            Some(self.started_at),
        )
    }
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
    /// Shared with its sessions' [`SessionRecorder`]s.
    kept: RunRecord,
    lock_path: PathBuf,
    /// The directory its sessions' records are under: the work directory,
    /// or the directory of the file form that keeps it.
    dir: PathBuf,
    /// Locked while the run is in flight. The kernel releases the lock when
    /// the process ends however it ends.
    _lock: File,
}

/// Where a run is kept, which decides whether it records its input.
enum Keeping {
    /// In the work directory's database, with its input.
    Database(RunRow),
    /// As the file form of one run alone, at `path`, with no input.
    Files { path: PathBuf },
}

impl Running {
    fn new(
        record: Record,
        keeping: Keeping,
        lock_path: PathBuf,
        dir: PathBuf,
        lock: File,
    ) -> Running {
        Running {
            kept: RunRecord {
                id: record.id(),
                record: Arc::new(Mutex::new(record)),
                keeping: Arc::new(keeping),
                writing: Arc::default(),
            },
            lock_path,
            dir,
            _lock: lock,
        }
    }

    pub fn id(&self) -> &RunId {
        &self.kept.id
    }

    pub fn in_flight(&self) -> InFlight {
        InFlight {
            id: self.kept.id.clone(),
            started_at: self.kept.record.lock().unwrap().started_at,
        }
    }

    /// Records a session, and gives what records what the harness reports
    /// of it.
    pub async fn session(
        &self,
        dir: &Path,
        settings: &SessionSettings,
        briefs: &Briefs,
    ) -> SessionRecorder {
        let dir = dir.strip_prefix(&self.dir).unwrap_or(dir);
        let reports = SessionReports::new(Ran::new(settings, briefs));
        let index = {
            let mut record = self.kept.record.lock().unwrap();
            record.sessions.push(RecordedSession::Ran {
                dir: dir.to_path_buf(),
                ran: reports.ran().clone(),
                cost: Some(reports.cost()),
            });
            record.sessions.len() - 1
        };
        self.kept.write_or_log(Change::Session(index)).await;
        SessionRecorder {
            kept: self.kept.clone(),
            index,
            reports: Mutex::new(reports),
        }
    }

    /// What keeps the input the run was given, `None` for a run that
    /// records none.
    pub fn input_recorder(&self) -> Option<InputRecorder> {
        match &*self.kept.keeping {
            Keeping::Database(row) => Some(InputRecorder(row.clone())),
            Keeping::Files { .. } => None,
        }
    }

    pub async fn answered(&self, at: Timestamp) {
        self.kept.record.lock().unwrap().answered_until = Some(at);
        self.kept.write_or_log(Change::Answered).await;
    }

    /// Once begun, the run ends even if the caller is dropped.
    pub async fn end(self, now: Timestamp, ending: Ending) {
        to_the_end(tokio::spawn(async move {
            self.kept.record.lock().unwrap().end = Some(End { at: now, ending });
            self.kept.write_or_log(Change::EndAndSessions).await;
            if let Err(e) = std::fs::remove_file(&self.lock_path) {
                warn!(path = %self.lock_path.display(), error = %e, "removing a run's lock failed");
            }
        }))
        .await;
    }
}

/// Its sessions' records are kept in the sessions directory of its
/// directory, and each is a session of its record.
#[async_trait::async_trait]
impl RecordsSessions for Running {
    fn record_dir(&self, purpose: &Purpose, now: Timestamp) -> Result<PathBuf> {
        session::new_record_dir(&sessions_dir(&self.dir), purpose, now)
    }

    async fn session(&self, dir: &Path, settings: &SessionSettings, briefs: &Briefs) -> Reporter {
        let recorder = Arc::new(Running::session(self, dir, settings, briefs).await);
        let ending = recorder.clone();
        Reporter::new(move |reported| recorder.reported(reported))
            .ending(move |time| ending.ended(time))
    }
}

/// The directory of a work directory that keeps the records of the
/// sessions of its runs.
pub(crate) const SESSIONS: &str = "sessions";

/// Where the records of the sessions of the runs under `dir` are kept.
fn sessions_dir(dir: &Path) -> PathBuf {
    dir.join(SESSIONS)
}

/// Keeps the input a run was given.
pub(crate) struct InputRecorder(RunRow);

impl InputRecorder {
    /// Keeps `input` as what the run was given, as it is: unredacted.
    pub async fn record(&self, input: Input) -> Result<()> {
        self.0.write_input(&input).await
    }
}

/// A run's record, as it is while the run is in flight, and where it is
/// kept.
#[derive(Clone)]
struct RunRecord {
    id: RunId,
    record: Arc<Mutex<Record>>,
    keeping: Arc<Keeping>,
    /// Held by each write, which writes the record as it is once it holds
    /// it: a write that runs late never writes an older record over a newer
    /// one.
    writing: Arc<tokio::sync::Mutex<()>>,
}

impl RunRecord {
    async fn write(&self, change: Change) -> Result<()> {
        let _writing = self.writing.lock().await;
        let record = self.record.lock().unwrap().clone();
        match &*self.keeping {
            Keeping::Database(row) => row.write(&record, change).await,
            Keeping::Files { path } => {
                let path = path.clone();
                off_runtime(move || records::save(&path, &record, "a run's record")).await
            }
        }
    }

    async fn write_or_log(&self, change: Change) {
        if let Err(e) = self.write(change).await {
            warn!(run = %self.id, error = format!("{e:#}"), "recording the run failed");
        }
    }
}

/// Records what the harness reports of one of a run's sessions, and its
/// end, as [`SessionReports`] makes them. Each change is written in a task
/// of its own, as the record is when that task writes it.
pub(crate) struct SessionRecorder {
    kept: RunRecord,
    /// The session's among the record's.
    index: usize,
    reports: Mutex<SessionReports>,
}

impl SessionRecorder {
    pub fn reported(&self, reported: Reported) {
        self.change(|reports| reports.reported(reported));
    }

    /// The session ended, its session timeout having counted `time`.
    pub fn ended(&self, time: std::time::Duration) {
        self.change(|reports| reports.ended(time));
    }

    fn change(&self, change: impl FnOnce(&mut SessionReports)) {
        let mut reports = session::lock_set_whole(&self.reports);
        change(&mut reports);
        if let RecordedSession::Ran { ran, cost, .. } =
            &mut self.kept.record.lock().unwrap().sessions[self.index]
        {
            *ran = reports.ran().clone();
            *cost = Some(reports.cost());
        }
        let (kept, index) = (self.kept.clone(), self.index);
        tokio::spawn(async move { kept.write_or_log(Change::Session(index)).await });
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    pub id: RunId,
    /// Set with `id`, from a [`Started`], so that the two agree.
    started_by: Option<StartedBy>,
    pub started_at: Timestamp,
    pub state: State,
    /// In the order it started them.
    pub sessions: Vec<SessionEntry>,
    /// For a rework, when it last read the feedback it answered.
    pub answered_until: Option<Timestamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ended { at: Timestamp, ending: Ending },
    InFlight,
    Interrupted,
}

impl State {
    /// Whether the run ended with an outcome that does not show as
    /// cancelled (`docs/specs/runs.md`, Ending).
    pub fn ended_not_cancelled(self) -> bool {
        match self {
            State::Ended { ending, .. } => OutcomeName::Ended(ending).status() != Status::Cancelled,
            State::InFlight | State::Interrupted => false,
        }
    }
}

impl Recorded {
    pub fn new(
        started: Started,
        started_at: Timestamp,
        state: State,
        sessions: Vec<SessionEntry>,
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

    /// What the run cost: the sum of its sessions' costs. A session with no
    /// cost kept counts as one with no figures.
    pub fn cost(&self) -> RunCost {
        cost_of(&self.sessions)
    }

    /// This run, where it ran on a pull request and its record has an end.
    pub fn ended_on_pull(&self) -> Option<EndedRun> {
        match (&self.id.of, self.state) {
            (RunOf::Work { item, stage }, State::Ended { at, ending }) if stage.is_on_pull() => {
                Some(EndedRun {
                    pr: *item,
                    kind: *stage,
                    ending,
                    at,
                    answered_until: self.answered_until,
                })
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    pub item: Option<Number>,
    pub stage: Option<WorkKind>,
    pub since: Option<Timestamp>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Listing {
    /// Newest first.
    pub runs: Vec<Run>,
    pub left_out: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(into = "RunAnswer", try_from = "RunAnswer")]
pub struct Run {
    pub id: RunId,
    /// Set with `id`, from a [`Started`], so that the two agree.
    started_by: Option<StartedBy>,
    pub started_at: Timestamp,
    pub outcome: Outcome,
    /// What each of its sessions ran with, in a list of one item's runs:
    /// `None` for a session recorded before Linewright kept it.
    pub sessions: Option<Vec<Option<Ran>>>,
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

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
struct RunAnswer {
    id: String,
    #[serde(flatten)]
    of: RunOf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_by: Option<StartedBy>,
    started_at: Timestamp,
    duration_seconds: Option<i64>,
    outcome: OutcomeName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sessions: Option<Vec<Option<Ran>>>,
}

/// A run's outcome by its name in JSON (`docs/specs/runs.md`, Ending).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum OutcomeName {
    Ended(Ending),
    Unended(Unended),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Unended {
    InFlight,
    Interrupted,
}

impl OutcomeName {
    /// Whether the run failed at its work, as opposed to being stopped.
    pub(crate) fn failed(self) -> bool {
        matches!(
            self,
            OutcomeName::Ended(Ending::Failed | Ending::HandedBack)
        )
    }

    /// Its words (`docs/specs/runs.md`, Ending, the Outcome column).
    pub(crate) fn word(self) -> &'static str {
        match self {
            OutcomeName::Ended(ending) => match ending {
                Ending::Done => "done",
                Ending::HandedBack => "handed back",
                Ending::Cancelled => "cancelled",
                Ending::UsageLimit => "ended for the usage limit",
                Ending::Failed => "failed",
            },
            OutcomeName::Unended(Unended::InFlight) => "in flight",
            OutcomeName::Unended(Unended::Interrupted) => "interrupted",
        }
    }

    /// The status it shows as (`docs/specs/runs.md`, Ending).
    pub(crate) fn status(self) -> Status {
        match self {
            OutcomeName::Ended(ending) => match ending {
                Ending::Done => Status::Done,
                Ending::Failed => Status::Failed,
                Ending::HandedBack => Status::Waiting,
                Ending::Cancelled | Ending::UsageLimit => Status::Cancelled,
            },
            OutcomeName::Unended(Unended::InFlight) => Status::InFlight,
            OutcomeName::Unended(Unended::Interrupted) => Status::Cancelled,
        }
    }
}

impl From<Run> for RunAnswer {
    fn from(run: Run) -> RunAnswer {
        let duration_seconds = run.outcome.duration_seconds();
        RunAnswer {
            id: run.id.to_string(),
            of: run.id.of,
            started_by: run.started_by,
            started_at: run.started_at,
            duration_seconds,
            outcome: run.outcome.name(),
            sessions: run.sessions,
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
            sessions: answer.sessions,
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

/// An automation's last run: its newest, by its trigger or by hand, and how
/// it started.
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

/// The runs of automations that attempts of workflow runs started.
pub fn triggered_runs(recorded: &[Recorded]) -> Vec<TriggeredRun> {
    recorded
        .iter()
        .filter_map(|run| match (&run.id.of, run.started_by) {
            (RunOf::Automation { automation }, Some(StartedBy::WorkflowRun(attempt))) => {
                Some(TriggeredRun {
                    automation: automation.clone(),
                    attempt,
                    state: run.state,
                })
            }
            _ => None,
        })
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
    let with_sessions = query.item.is_some();
    Ok(Listing {
        runs: runs
            .into_iter()
            .map(|run| {
                let sessions = with_sessions.then(|| run.ran());
                run.listed(now, sessions)
            })
            .collect(),
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
    /// As a list of runs shows it, without its sessions.
    pub(crate) fn at(self, now: Timestamp) -> Run {
        self.listed(now, None)
    }

    /// What each of its sessions ran with, `None` for one recorded before
    /// Linewright kept it.
    pub fn ran(&self) -> Vec<Option<Ran>> {
        self.sessions
            .iter()
            .map(|session| session.ran().cloned())
            .collect()
    }

    fn listed(self, now: Timestamp, sessions: Option<Vec<Option<Ran>>>) -> Run {
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
            sessions,
        }
    }
}

pub fn render(listing: &Listing, colour: bool) -> String {
    let paint = Paint(colour);
    let mut text = String::new();
    for run in &listing.runs {
        let _ = writeln!(text, "{}", render_run(run, paint));
        for (i, ran) in run.sessions.iter().flatten().enumerate() {
            let _ = writeln!(text, "  session {}: {}", i + 1, describe_ran(ran.as_ref()));
        }
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

/// The sum of the costs of `sessions`, as [`Recorded::cost`] gives it.
fn cost_of(sessions: &[SessionEntry]) -> RunCost {
    let unkept = SessionCost::default();
    RunCost::of(sessions.iter().map(|session| {
        session
            .kept
            .as_ref()
            .and_then(|kept| kept.cost.as_ref())
            .unwrap_or(&unkept)
    }))
}

/// A session's model, as the harness reported it ran or else as set, and
/// its effort as set.
pub(crate) fn describe_ran(ran: Option<&Ran>) -> String {
    let Some(ran) = ran else {
        return "model and effort not recorded".into();
    };
    format!(
        "model {}, effort {}",
        ran.model_ran().unwrap_or(NOT_RECORDED),
        ran.effort.map_or(NOT_RECORDED, Effort::name)
    )
}

/// A model or effort a record kept before every session had one does not
/// hold.
const NOT_RECORDED: &str = "not recorded";

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
    /// How long the run ran, or has run so far; `None` for an interrupted
    /// run, which ran for no known time.
    pub(crate) fn duration_seconds(self) -> Option<i64> {
        match self {
            Outcome::Ended { seconds, .. } | Outcome::InFlight { seconds } => Some(seconds),
            Outcome::Interrupted => None,
        }
    }

    pub(crate) fn word(self) -> &'static str {
        self.name().word()
    }

    /// Its name in JSON.
    pub(crate) fn name(self) -> OutcomeName {
        match self {
            Outcome::Ended { ending, .. } => OutcomeName::Ended(ending),
            Outcome::InFlight { .. } => OutcomeName::Unended(Unended::InFlight),
            Outcome::Interrupted => OutcomeName::Unended(Unended::Interrupted),
        }
    }

    /// The status it shows as (`docs/specs/runs.md`, Ending).
    pub(crate) fn status(self) -> Status {
        self.name().status()
    }
}

pub(crate) fn render_run(run: &Run, paint: Paint) -> String {
    let word = run.outcome.word();
    let outcome = match run.outcome {
        Outcome::Ended { ending, .. } => match ending {
            Ending::Done | Ending::Cancelled => word.into(),
            Ending::Failed => paint.red(word),
            Ending::HandedBack | Ending::UsageLimit => paint.yellow(word),
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
    match duration(run) {
        Some(duration) => phrase.text(", ran ").then(duration),
        None => phrase,
    }
}

/// How long a run ran, running on while it is in flight; `None` for an
/// interrupted run, which ran for no known time.
pub(crate) fn duration(run: &Run) -> Option<Phrase> {
    match run.outcome {
        Outcome::Ended { seconds, .. } => {
            Some(Phrase::default().span(TimeDelta::seconds(seconds), None))
        }
        Outcome::InFlight { seconds } => {
            Some(Phrase::default().span(TimeDelta::seconds(seconds), Some(run.started_at)))
        }
        Outcome::Interrupted => None,
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

/// The runs `query` asks for, of `runs`, or of none where there is no
/// database.
pub async fn answer(
    runs: Option<&Runs>,
    query: &Query,
    now: Timestamp,
) -> Result<Listing, ListError> {
    let recorded = match runs {
        Some(runs) => runs.read().await?,
        None => vec![],
    };
    Ok(list(recorded, query, now)?)
}

#[derive(Clone, Debug)]
pub struct RepoWorkDir {
    pub base: PathBuf,
    pub repo: Repo,
}

impl RepoWorkDir {
    /// The work directory `base/owner/name` when it exists; else one whose
    /// owner and name match those ignoring ASCII case, as GitHub names
    /// repositories case-insensitively; else `base/owner/name`. It is looked
    /// up afresh on every call.
    pub fn resolve(&self) -> PathBuf {
        work_dir(&self.base, &self.repo)
    }
}

fn work_dir(base: &Path, repo: &Repo) -> PathBuf {
    let exact = base.join(&**repo);
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
    spelled(base, repo.owner())
        .and_then(|owner| spelled(&owner, repo.name()))
        .unwrap_or(exact)
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

#[cfg(test)]
mod tests {
    use std::panic::Location;

    use super::*;
    use crate::cost::{Figures, Spend, Tokens, TurnSpend, Usd};
    use crate::work_settings::{Environments, Model, RepoSettings, RunKind, SessionWork};

    fn at(time: &str) -> Timestamp {
        time.parse().unwrap()
    }

    /// Keeps `record` in the database of `runs`, as a run left it.
    #[track_caller]
    fn keep(runs: &Runs, record: serde_json::Value) -> impl Future<Output = ()> + '_ {
        let caller = Location::caller();
        async move {
            let record: Record = serde_json::from_value(record)
                .unwrap_or_else(|e| panic!("reading the record at {caller}: {e}"));
            let mut conn = runs.db.pool().acquire().await.unwrap();
            rows::insert(&mut conn, &record)
                .await
                .unwrap_or_else(|e| panic!("keeping the record at {caller}: {e:#}"));
        }
    }

    /// `docs/specs/runs.md`, Ending: each outcome, by its name in JSON, and
    /// the status it shows as.
    #[test_log::test]
    fn each_outcome_shows_as_the_status_the_ending_table_gives_it() {
        let shown: Vec<(&str, Status)> = [
            "done",
            "handed-back",
            "cancelled",
            "usage-limit",
            "failed",
            "in-flight",
            "interrupted",
        ]
        .into_iter()
        .map(|name| {
            let outcome: OutcomeName = serde_json::from_value(name.into()).unwrap();
            (name, outcome.status())
        })
        .collect();

        assert_eq!(
            shown,
            [
                ("done", Status::Done),
                ("handed-back", Status::Waiting),
                ("cancelled", Status::Cancelled),
                ("usage-limit", Status::Cancelled),
                ("failed", Status::Failed),
                ("in-flight", Status::InFlight),
                ("interrupted", Status::Cancelled),
            ]
        );
    }

    fn ended(item: u64, stage: &str, n: u32) -> serde_json::Value {
        serde_json::json!({
            "item": item,
            "stage": stage,
            "n": n,
            "started_at": "2026-10-14T12:00:00Z",
            "end": {"at": "2026-10-14T12:01:00Z", "ending": "done"},
            "sessions": [],
        })
    }

    /// The IDs of the runs of `begins`, begun one after another in `runs`
    /// and ended.
    #[track_caller]
    fn begun(runs: &Runs, begins: Vec<Begin>) -> impl Future<Output = Vec<String>> + '_ {
        let caller = Location::caller();
        async move {
            let mut ids = vec![];
            for begin in begins {
                let running = runs
                    .begin(begin, at("2026-10-15T09:00:00Z"))
                    .await
                    .unwrap_or_else(|e| panic!("beginning a run at {caller}: {e:#}"));
                ids.push(running.id().to_string());
                running.end(at("2026-10-15T09:01:00Z"), Ending::Done).await;
            }
            ids
        }
    }

    /// The runs of the database of `work_dir`.
    #[track_caller]
    fn runs_in(work_dir: &Path) -> impl Future<Output = Runs> + '_ {
        let caller = Location::caller();
        async move {
            let db = Database::open(work_dir)
                .await
                .unwrap_or_else(|e| panic!("opening the database at {caller}: {e:#}"));
            Runs::new(db)
        }
    }

    fn work(item: u64, stage: WorkKind) -> Begin {
        Begin::Work {
            item: Number(item),
            stage,
        }
    }

    fn automation(name: &str, n: u32) -> serde_json::Value {
        serde_json::json!({
            "automation": name,
            "n": n,
            "started_by": "schedule",
            "started_at": "2026-10-14T06:00:00Z",
            "end": {"at": "2026-10-14T06:05:00Z", "ending": "done"},
            "sessions": [],
        })
    }

    #[test_log::test(tokio::test)]
    async fn the_runs_attempts_started_read_back_with_their_attempts_and_others_are_not_triggered()
    {
        let work_dir = tempfile::tempdir().unwrap();
        let runs = runs_in(work_dir.path()).await;
        let attempt = crate::workflow_run::AttemptId {
            run: crate::workflow_run::WorkflowRunId(18_000_000_001),
            attempt: std::num::NonZeroU32::new(2).unwrap(),
        };
        let fix_ci = |started_by| Begin::Automation {
            automation: "fix-ci.md".parse().unwrap(),
            started_by,
        };

        begun(
            &runs,
            vec![
                fix_ci(StartedBy::Hand),
                fix_ci(StartedBy::WorkflowRun(attempt)),
                fix_ci(StartedBy::Schedule),
            ],
        )
        .await;

        let recorded = runs.read().await.unwrap();
        assert_eq!(
            triggered_runs(&recorded),
            [TriggeredRun {
                automation: "fix-ci.md".parse().unwrap(),
                attempt,
                state: State::Ended {
                    at: at("2026-10-15T09:01:00Z"),
                    ending: Ending::Done,
                },
            }]
        );
        let by: Vec<Option<StartedBy>> = recorded.iter().map(|run| run.started().by()).collect();
        assert_eq!(
            by,
            [
                Some(StartedBy::Hand),
                Some(StartedBy::WorkflowRun(attempt)),
                Some(StartedBy::Schedule)
            ]
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_run_is_numbered_on_from_the_recorded_runs_of_its_stage_on_its_item_or_of_its_automation()
     {
        let work_dir = tempfile::tempdir().unwrap();
        let runs = runs_in(work_dir.path()).await;
        for (item, stage, n) in [
            (1, "triage", 1),
            (1, "triage", 2),
            (1, "implement", 1),
            (3, "rework", 1),
        ] {
            keep(&runs, ended(item, stage, n)).await;
        }
        keep(&runs, automation("team/weekly.md", 2)).await;

        let ids = begun(
            &runs,
            vec![
                work(1, WorkKind::Triage),
                work(1, WorkKind::Triage),
                work(1, WorkKind::Implement),
                work(3, WorkKind::Rework),
                work(3, WorkKind::FixCi),
                work(2, WorkKind::Triage),
                Begin::Automation {
                    automation: "team/weekly.md".parse().unwrap(),
                    started_by: StartedBy::Hand,
                },
                Begin::Automation {
                    automation: "daily.md".parse().unwrap(),
                    started_by: StartedBy::Hand,
                },
            ],
        )
        .await;

        assert_eq!(
            ids,
            [
                "1/triage-3",
                "1/triage-4",
                "1/implement-2",
                "3/rework-2",
                "3/fix-ci-1",
                "2/triage-1",
                "team/weekly.md/3",
                "daily.md/1",
            ]
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_run_reads_as_in_flight_to_another_handle_and_as_interrupted_once_its_process_lets_go()
     {
        let work_dir = tempfile::tempdir().unwrap();
        let running = runs_in(work_dir.path())
            .await
            .begin(work(1, WorkKind::Triage), at("2026-10-15T09:00:00Z"))
            .await
            .unwrap();
        let other = runs_in(work_dir.path()).await;

        let in_flight = other.read().await.unwrap();
        drop(running);
        let let_go = other.read().await.unwrap();

        assert_eq!(
            [in_flight, let_go].map(|read| read.iter().map(|run| run.state).collect::<Vec<_>>()),
            [vec![State::InFlight], vec![State::Interrupted]]
        );
    }

    #[test_log::test(tokio::test)]
    async fn runs_begun_at_once_through_two_handles_take_numbers_of_their_own() {
        let work_dir = tempfile::tempdir().unwrap();
        let (one, two) = (
            runs_in(work_dir.path()).await,
            runs_in(work_dir.path()).await,
        );
        let begin = |runs: Runs| async move {
            let mut ids = vec![];
            for _ in 0..5 {
                let running = runs
                    .begin(work(1, WorkKind::Triage), at("2026-10-15T09:00:00Z"))
                    .await
                    .unwrap();
                ids.push(running.id().n);
            }
            ids
        };

        let (a, b) = tokio::join!(tokio::spawn(begin(one)), tokio::spawn(begin(two)));

        let mut numbers: Vec<u32> = a.unwrap().into_iter().chain(b.unwrap()).collect();
        numbers.sort_unstable();
        assert_eq!(numbers, (1..=10).collect::<Vec<_>>());
    }

    #[test_log::test(tokio::test)]
    async fn what_a_runs_sessions_ran_with_reads_back_from_its_record() {
        let work_dir = tempfile::tempdir().unwrap();
        let settings = opus_for_triage();
        let session = work_dir.path().join("sessions/1/triage-1");
        let running = runs_in(work_dir.path())
            .await
            .begin(work(1, WorkKind::Triage), at("2026-10-15T09:00:00Z"))
            .await
            .unwrap();

        running
            .session(&session, &settings, &Briefs::shipped())
            .await
            .reported(Reported::Model("claude-opus-5-5".into()));
        running.end(at("2026-10-15T09:01:00Z"), Ending::Done).await;
        let recorded = runs_in(work_dir.path()).await.read().await.unwrap();

        let [run] = &recorded[..] else {
            panic!("one run: {recorded:#?}");
        };
        let [
            SessionEntry {
                dir,
                kept: Some(SessionKept { ran, .. }),
            },
        ] = &run.sessions[..]
        else {
            panic!("one session with what it ran with: {run:#?}");
        };
        assert_eq!(dir, &session);
        assert_eq!(
            (
                ran.model.as_deref(),
                ran.reported_model.as_deref(),
                ran.effort
            ),
            (Some("opus"), Some("claude-opus-5-5"), Some(Effort::Medium))
        );
        assert_eq!(
            serde_json::to_value(&ran.layers.settings).unwrap(),
            serde_json::json!({
                "model": "operator",
                "effort": "linewright",
                "session-timeout": "linewright",
                "maintenance": "linewright",
                "session-env": "linewright",
                "session-pass-env": "linewright",
            })
        );
        assert_eq!(
            ran.layers.briefs.get("prompts/triage.md"),
            Some(&Layer::Linewright)
        );
    }

    /// A triage run's settings, with the operator's model `opus`.
    fn opus_for_triage() -> SessionSettings {
        SessionWork::with_model(Model::new("opus".into()).unwrap()).for_run(
            RunKind::Stage(WorkKind::Triage),
            &RepoSettings::default(),
            &Environments::default(),
        )
    }

    #[test_log::test(tokio::test)]
    async fn a_report_written_after_its_run_ended_keeps_the_end() {
        let work_dir = tempfile::tempdir().unwrap();
        let runs = runs_in(work_dir.path()).await;
        let running = runs
            .begin(work(1, WorkKind::Triage), at("2026-10-15T09:00:00Z"))
            .await
            .unwrap();
        let recorder = running
            .session(
                &work_dir.path().join("sessions/1/triage-1"),
                &opus_for_triage(),
                &Briefs::shipped(),
            )
            .await;

        recorder.reported(Reported::Model("claude-opus-5-5".into()));
        running.end(at("2026-10-15T09:01:00Z"), Ending::Done).await;
        recorder.reported(Reported::Model("claude-opus-5-6".into()));
        let models = |recorded: &[Recorded]| -> Vec<Option<String>> {
            recorded
                .iter()
                .flat_map(|run| &run.sessions)
                .map(|session| session.ran().and_then(Ran::model_ran).map(str::to_owned))
                .collect()
        };
        let recorded = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let recorded = runs.read().await.unwrap();
                if models(&recorded) == [Some("claude-opus-5-6".to_owned())] {
                    break recorded;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the late report is written");

        let [run] = &recorded[..] else {
            panic!("one run: {recorded:#?}");
        };
        assert_eq!(
            run.state,
            State::Ended {
                at: at("2026-10-15T09:01:00Z"),
                ending: Ending::Done
            }
        );
    }

    /// What `opus` used in a turn: `tokens` of each kind, for `dollars`.
    fn opus_used(tokens: u64, dollars: f64) -> Figures {
        Figures(BTreeMap::from([(
            "claude-opus-5-5".to_string(),
            Spend {
                tokens: Some(Tokens {
                    input: tokens,
                    output: tokens,
                    cache_write: tokens,
                    cache_read: tokens,
                }),
                usd: Usd::new(dollars),
            },
        )]))
    }

    /// The cost kept of each session of each run of `recorded`.
    fn costs(recorded: &[Recorded]) -> Vec<Option<SessionCost>> {
        recorded
            .iter()
            .flat_map(|run| &run.sessions)
            .map(|session| session.kept.as_ref().and_then(|kept| kept.cost.clone()))
            .collect()
    }

    /// The costs of the sessions of `runs`, once they read other than
    /// `before`.
    #[track_caller]
    fn costs_once_changed<'a>(
        runs: &'a Runs,
        before: &'a [Option<SessionCost>],
    ) -> impl Future<Output = Vec<Option<SessionCost>>> + 'a {
        let caller = Location::caller();
        async move {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let read = costs(&runs.read().await.unwrap());
                    if read != before {
                        break read;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("the costs at {caller} stayed {before:?}"))
        }
    }

    #[test_log::test(tokio::test)]
    async fn a_sessions_cost_is_kept_as_its_turns_and_its_end_are_reported() {
        let work_dir = tempfile::tempdir().unwrap();
        let runs = runs_in(work_dir.path()).await;
        let running = runs
            .begin(work(1, WorkKind::Triage), at("2026-10-15T09:00:00Z"))
            .await
            .unwrap();
        let recorder = running
            .session(
                &work_dir.path().join("sessions/1/triage-1"),
                &opus_for_triage(),
                &Briefs::shipped(),
            )
            .await;

        let before = costs(&runs.read().await.unwrap());
        recorder.reported(Reported::TurnStarted);
        let mid_turn = costs_once_changed(&runs, &before).await;
        recorder.reported(Reported::TurnEnded(TurnSpend::OfSessionSoFar(opus_used(
            10, 1.5,
        ))));
        recorder.ended(std::time::Duration::from_secs(90));
        running.end(at("2026-10-15T09:01:00Z"), Ending::Done).await;
        let recorded = runs_in(work_dir.path()).await.read().await.unwrap();

        assert_eq!(
            before,
            [Some(SessionCost {
                time: None,
                spend: Some(Figures::default()),
            })]
        );
        assert_eq!(mid_turn, [Some(SessionCost::default())]);
        assert_eq!(
            costs(&recorded),
            [Some(SessionCost {
                time: Some(std::time::Duration::from_secs(90)),
                spend: Some(opus_used(10, 1.5)),
            })]
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_runs_cost_counts_a_session_recorded_before_costs_were_kept_as_one_with_no_figures() {
        let work_dir = tempfile::tempdir().unwrap();
        let runs = runs_in(work_dir.path()).await;
        let ran = |cost: Option<serde_json::Value>| {
            let mut session = serde_json::json!({
                "dir": "sessions/1/triage-1",
                "model": "opus",
                "reported_model": "claude-opus-5-5",
                "effort": null,
                "layers": {"settings": {}, "briefs": {}},
            });
            if let Some(cost) = cost {
                session["cost"] = cost;
            }
            session
        };
        let mut record = ended(1, "triage", 1);
        record["sessions"] = serde_json::json!([
            ran(Some(serde_json::json!({
                "seconds": 30.0,
                "models": {"claude-haiku-5-5": {
                    "tokens": {"input": 1, "output": 2, "cache_write": 3, "cache_read": 4},
                    "usd": 0.5,
                }},
            }))),
            ran(None),
        ]);
        keep(&runs, record).await;

        let recorded = runs.read().await.unwrap();

        let [run] = &recorded[..] else {
            panic!("one run: {recorded:#?}");
        };
        assert_eq!(
            run.cost(),
            RunCost {
                time: None,
                spend: None,
            }
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_run_of_the_store_records_its_input_and_one_kept_as_files_cannot() {
        let work_dir = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let begin = || work(1, WorkKind::Triage);
        let started = at("2026-10-15T09:00:00Z");

        let stored = runs_in(work_dir.path())
            .await
            .begin(begin(), started)
            .await
            .unwrap();
        let as_files = RunFiles::new(out.path())
            .begin(begin(), started)
            .await
            .unwrap();

        assert_eq!(
            (
                stored.input_recorder().is_some(),
                as_files.input_recorder().is_some()
            ),
            (true, false)
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_run_kept_as_files_reads_in_flight_while_it_runs_ended_once_it_ends_and_interrupted_if_it_stopped_without_an_end()
     {
        let out = tempfile::tempdir().unwrap();
        let files = RunFiles::new(out.path());
        let ended: RunId = "1/triage-1".parse().unwrap();
        let stopped: RunId = "2/triage-1".parse().unwrap();
        let state = |id| files.read(id).unwrap().map(|run| run.state);
        assert_eq!(state(&ended), None);

        let running = files
            .begin(work(1, WorkKind::Triage), at("2026-10-15T09:00:00Z"))
            .await
            .unwrap();
        let in_flight = state(&ended);
        running.end(at("2026-10-15T09:05:00Z"), Ending::Done).await;
        drop(
            files
                .begin(work(2, WorkKind::Triage), at("2026-10-15T09:00:00Z"))
                .await
                .unwrap(),
        );

        assert_eq!(in_flight, Some(State::InFlight));
        assert_eq!(
            state(&ended),
            Some(State::Ended {
                at: at("2026-10-15T09:05:00Z"),
                ending: Ending::Done,
            })
        );
        assert_eq!(state(&stopped), Some(State::Interrupted));
    }

    #[test_log::test]
    fn a_run_written_as_files_is_its_record_at_its_place_and_one_with_no_end_is_written_with_none()
    {
        let out = tempfile::tempdir().unwrap();
        let session = out.path().join("sessions/7/implement-2");
        let handed_back = Recorded::new(
            Started::new("7/implement-2".parse().unwrap(), None).unwrap(),
            at("2026-10-15T09:00:00Z"),
            State::Ended {
                at: at("2026-10-15T09:05:00Z"),
                ending: Ending::HandedBack,
            },
            vec![session.into()],
            None,
        );
        let in_flight = |state| {
            Recorded::new(
                Started::new("team/weekly.md/3".parse().unwrap(), Some(StartedBy::Hand)).unwrap(),
                at("2026-10-15T10:00:00Z"),
                state,
                vec![],
                None,
            )
        };
        let files = RunFiles::new(out.path());

        files.write(&handed_back).unwrap();
        files.write(&in_flight(State::InFlight)).unwrap();

        let written = |path: &str| -> serde_json::Value {
            serde_json::from_str(&std::fs::read_to_string(out.path().join(path)).unwrap()).unwrap()
        };
        assert_eq!(
            [
                written("runs/items/7/implement-2.json"),
                written("runs/automations/team%2Fweekly.md/3.json"),
            ],
            [
                serde_json::json!({
                    "item": 7,
                    "stage": "implement",
                    "n": 2,
                    "started_at": "2026-10-15T09:00:00Z",
                    "end": {"at": "2026-10-15T09:05:00Z", "ending": "handed-back"},
                    "sessions": ["sessions/7/implement-2"],
                }),
                serde_json::json!({
                    "automation": "team/weekly.md",
                    "n": 3,
                    "started_by": "hand",
                    "started_at": "2026-10-15T10:00:00Z",
                    "end": null,
                    "sessions": [],
                }),
            ]
        );
    }
}
