pub mod files;

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::TimeDelta;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::automation::AutomationName;
use crate::briefs::Briefs;
use crate::domain::{Number, Repo, Timestamp};
use crate::harness::{Purpose, Reported, Reporter};
use crate::input::Input;
use crate::layer::{Layer, Source};
use crate::phrase::{Phrase, Tone};
use crate::reconcile::{EndedRun, WorkKind};
use crate::records;
use crate::session::{self, RecordsSessions};
use crate::status::Paint;
use crate::work_settings::{Effort, SessionSettings, Setting};
use files::{End, Record, RecordedSession, RunFiles};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// A session of a run: where its records are, and what it ran with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionEntry {
    pub dir: PathBuf,
    /// `None` for a session recorded before Linewright kept it.
    pub ran: Option<Ran>,
}

impl From<PathBuf> for SessionEntry {
    fn from(dir: PathBuf) -> SessionEntry {
        SessionEntry { dir, ran: None }
    }
}

/// What a session ran with: its model and effort as Linewright set them,
/// the model as the harness reported it ran, and where each of its settings
/// and prompt files came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "KeptRan")]
pub struct Ran {
    /// `None` is the harness's choice.
    pub model: Option<String>,
    /// `None` until the harness reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_model: Option<String>,
    /// `None` is the harness's choice.
    pub effort: Option<Effort>,
    pub layers: Layers,
}

/// A [`Ran`] as a record holds it.
#[derive(Deserialize)]
struct KeptRan {
    model: Option<String>,
    #[serde(default)]
    reported_model: Option<String>,
    effort: Option<Effort>,
    layers: Layers,
}

/// A record kept before the harness was a source gave a model or effort
/// none set as Linewright's.
impl From<KeptRan> for Ran {
    fn from(kept: KeptRan) -> Ran {
        let mut layers = kept.layers;
        for (setting, set) in [
            (Setting::MODEL, kept.model.is_some()),
            (Setting::EFFORT, kept.effort.is_some()),
        ] {
            if let Some(source) = layers.settings.get_mut(&setting)
                && !set
                && *source == Source::Layer(Layer::Linewright)
            {
                *source = Source::Harness;
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layers {
    pub settings: BTreeMap<Setting, Source>,
    /// By the prompt's path under the prompts' root. A record kept before
    /// prompts were the only layered files has its paths under the briefs'
    /// root, such as `prompts/triage.md` and `plugin/skills/tdd/SKILL.md`.
    #[serde(alias = "briefs")]
    pub prompts: BTreeMap<String, Layer>,
}

impl Ran {
    pub fn new(settings: &SessionSettings, briefs: &Briefs) -> Ran {
        Ran {
            model: settings.model_set(),
            reported_model: None,
            effort: settings.effort_set(),
            layers: Layers {
                settings: settings.sources(),
                prompts: briefs.layers().clone(),
            },
        }
    }

    /// The model the harness reported the session ran, else the one set.
    pub fn model_ran(&self) -> Option<&str> {
        self.reported_model.as_deref().or(self.model.as_deref())
    }
}

/// The work directory's store of runs.
#[derive(Clone, Debug)]
pub struct Runs {
    files: RunFiles,
}

impl Runs {
    pub fn new(work_dir: &Path) -> Runs {
        Runs {
            files: RunFiles::new(work_dir),
        }
    }

    /// Begins a run of `begin`, numbered after every run of the same item
    /// and stage, or of the same automation, with no two runs given the
    /// same number, even by two processes at once.
    pub(crate) async fn begin(&self, begin: Begin, now: Timestamp) -> Result<Running> {
        self.files.begin_kept(begin, now, Keeping::Store).await
    }

    /// The input run `id` recorded at its start, `None` when it recorded
    /// none.
    pub async fn read_input(&self, id: &RunId) -> Result<Option<Input>> {
        let (files, id) = (self.files.clone(), id.clone());
        off_runtime(move || files.read_input(&id)).await
    }

    /// A record that does not read is logged and taken as missing.
    pub async fn read_run(&self, id: &RunId) -> Option<Recorded> {
        let (files, id) = (self.files.clone(), id.clone());
        off_runtime(move || files.read_run(&id)).await
    }

    /// `None` if the run has no record; a record that does not read fails.
    pub async fn try_read_run(&self, id: &RunId) -> Result<Option<Recorded>> {
        let (files, id) = (self.files.clone(), id.clone());
        off_runtime(move || files.try_read_run(&id)).await
    }

    /// As [`Runs::read`], with a record that does not read logged and
    /// skipped, and a failure to read logged and taken as no runs.
    pub async fn read_or_none(&self) -> Vec<Recorded> {
        self.read().await.unwrap_or_else(|e| {
            warn!(error = format!("{e:#}"), "reading the runs' records failed");
            vec![]
        })
    }

    pub async fn read(&self) -> Result<Vec<Recorded>> {
        let files = self.files.clone();
        off_runtime(move || files.read_legacy()).await
    }
}

/// What `work` gives, done on a thread where blocking is allowed: once
/// begun, it runs to its end even if the caller is dropped. A panic in it
/// goes on in the caller. It never gives anything when the runtime shuts
/// down before `work` begins.
async fn off_runtime<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(work).await {
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
    keeping: Keeping,
    lock_path: PathBuf,
    /// The directory its record and its sessions' are under.
    dir: PathBuf,
    /// Locked while the run is in flight. The kernel releases the lock when
    /// the process ends however it ends.
    _lock: File,
}

/// Where a run is kept, which decides whether it records its input.
enum Keeping {
    /// In the work directory's store of runs, with its input.
    Store,
    /// As the file form of one run alone, with no input.
    Files,
}

impl Running {
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
    pub fn session(
        &self,
        dir: &Path,
        settings: &SessionSettings,
        briefs: &Briefs,
    ) -> SessionRecorder {
        let dir = dir.strip_prefix(&self.dir).unwrap_or(dir);
        let index = {
            let mut record = self.kept.record.lock().unwrap();
            record.sessions.push(RecordedSession::Ran {
                dir: dir.to_path_buf(),
                ran: Ran::new(settings, briefs),
            });
            record.sessions.len() - 1
        };
        self.kept.save_or_log();
        SessionRecorder {
            kept: self.kept.clone(),
            index,
        }
    }

    /// What keeps the input the run was given, `None` for a run that
    /// records none.
    pub fn input_recorder(&self) -> Option<InputRecorder> {
        match self.keeping {
            Keeping::Store => Some(InputRecorder {
                path: files::input_path(&self.kept.path),
            }),
            Keeping::Files => None,
        }
    }

    pub async fn answered(&self, at: Timestamp) {
        self.kept.record.lock().unwrap().answered_until = Some(at);
        let kept = self.kept.clone();
        off_runtime(move || kept.save_or_log()).await;
    }

    /// Once begun, the run ends even if the caller is dropped.
    pub async fn end(self, now: Timestamp, ending: Ending) {
        off_runtime(move || {
            self.kept.record.lock().unwrap().end = Some(End { at: now, ending });
            self.kept.save_or_log();
            if let Err(e) = std::fs::remove_file(&self.lock_path) {
                warn!(path = %self.lock_path.display(), error = %e, "removing a run's lock failed");
            }
        })
        .await;
    }
}

/// Its sessions' records are kept in the sessions directory beside its
/// record, and each is a session of its record.
impl RecordsSessions for Running {
    fn record_dir(&self, purpose: &Purpose, now: Timestamp) -> Result<PathBuf> {
        session::new_record_dir(&sessions_dir(&self.dir), purpose, now)
    }

    fn session(&self, dir: &Path, settings: &SessionSettings, briefs: &Briefs) -> Reporter {
        let recorder = Running::session(self, dir, settings, briefs);
        Reporter::new(move |reported| recorder.reported(reported))
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
pub(crate) struct InputRecorder {
    path: PathBuf,
}

impl InputRecorder {
    /// Keeps `input` as what the run was given, as it is: unredacted.
    pub async fn record(&self, input: Input) -> Result<()> {
        let path = self.path.clone();
        off_runtime(move || records::save(&path, &input, "a run's input")).await
    }
}

/// A run's record, as it is while the run is in flight, and where it is
/// saved.
#[derive(Clone)]
struct RunRecord {
    id: RunId,
    path: PathBuf,
    record: Arc<Mutex<Record>>,
}

impl RunRecord {
    /// Holds the record while it writes it, so that what one write saves
    /// is never overwritten by an older copy.
    fn save(&self) -> Result<()> {
        let record = self.record.lock().unwrap();
        records::save(&self.path, &*record, "a run's record")
    }

    fn save_or_log(&self) {
        if let Err(e) = self.save() {
            warn!(run = %self.id, error = format!("{e:#}"), "recording the run failed");
        }
    }
}

/// Records what the harness reports of one of a run's sessions.
pub(crate) struct SessionRecorder {
    kept: RunRecord,
    /// The session's among the record's.
    index: usize,
}

impl SessionRecorder {
    /// The last report wins. The record is written in a task of its own,
    /// as it is when that task writes it.
    pub fn reported(&self, reported: Reported) {
        if let RecordedSession::Ran { ran, .. } =
            &mut self.kept.record.lock().unwrap().sessions[self.index]
        {
            ran.reported_model = Some(reported.model);
        }
        let kept = self.kept.clone();
        tokio::spawn(off_runtime(move || kept.save_or_log()));
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sessions: Option<Vec<Option<Ran>>>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum OutcomeName {
    Ended(Ending),
    Unended(Unended),
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Unended {
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

/// An automation's last run: its newest, by its schedule or by hand, and how
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
            .map(|session| session.ran.clone())
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

/// A session's model, as the harness reported it ran or else as set, and
/// its effort as set.
pub(crate) fn describe_ran(ran: Option<&Ran>) -> String {
    let Some(ran) = ran else {
        return "model and effort not recorded".into();
    };
    format!(
        "model {}, effort {}",
        ran.model_ran().unwrap_or(HARNESS_CHOICE),
        ran.effort.map_or(HARNESS_CHOICE, Effort::name)
    )
}

/// A model or effort Linewright left unset.
pub(crate) const HARNESS_CHOICE: &str = "the harness's choice";

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
                Ending::Failed => Tone::Failure,
                Ending::HandedBack => Tone::Pending,
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

pub async fn answer(
    work_dir: &RepoWorkDir,
    query: &Query,
    now: Timestamp,
) -> Result<Listing, ListError> {
    let recorded = Runs::new(&work_dir.resolve()).read().await?;
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
    use crate::work_settings::{Environments, Partial, RepoSettings, RunKind, WorkSettings};

    fn at(time: &str) -> Timestamp {
        time.parse().unwrap()
    }

    /// Writes `record` as the file `path` under `work_dir`, as a run left it.
    fn left(work_dir: &Path, path: &str, record: &serde_json::Value) {
        let path = work_dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, record.to_string()).unwrap();
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

    fn work(item: u64, stage: WorkKind) -> Begin {
        Begin::Work {
            item: Number(item),
            stage,
        }
    }

    #[test_log::test(tokio::test)]
    async fn a_run_is_numbered_on_from_the_recorded_runs_of_its_stage_on_its_item() {
        let work_dir = tempfile::tempdir().unwrap();
        for (path, item, stage, n) in [
            ("runs/items/1/triage-1.json", 1, "triage", 1),
            ("runs/items/1/triage-2.json", 1, "triage", 2),
            ("runs/items/1/implement-1.json", 1, "implement", 1),
            ("runs/items/3/rework-1.json", 3, "rework", 1),
        ] {
            left(work_dir.path(), path, &ended(item, stage, n));
        }
        let runs = Runs::new(work_dir.path());

        let ids = begun(
            &runs,
            vec![
                work(1, WorkKind::Triage),
                work(1, WorkKind::Triage),
                work(1, WorkKind::Implement),
                work(3, WorkKind::Rework),
                work(3, WorkKind::FixCi),
                work(2, WorkKind::Triage),
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
            ]
        );
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

    /// The record of an automation's run as the version before automations
    /// were renamed from workflows kept it.
    fn workflow(name: &str, n: u32) -> serde_json::Value {
        serde_json::json!({
            "workflow": name,
            "n": n,
            "started_at": "2026-09-28T06:00:00Z",
            "end": {"at": "2026-09-28T06:05:00Z", "ending": "done"},
            "sessions": [],
        })
    }

    #[test_log::test(tokio::test)]
    async fn a_run_is_numbered_on_from_its_runs_where_this_version_keeps_them_and_where_the_previous_did()
     {
        let work_dir = tempfile::tempdir().unwrap();
        for (path, record) in [
            ("runs/workflows/weekly/1.json", workflow("weekly", 1)),
            ("runs/automations/weekly/2.json", automation("weekly", 2)),
            ("runs/workflows/daily/4.json", workflow("daily", 4)),
            ("runs/items/4/tickets-2.json", ended(4, "tickets", 2)),
            ("runs/items/4/design-1.json", ended(4, "design", 1)),
        ] {
            left(work_dir.path(), path, &record);
        }
        let runs = Runs::new(work_dir.path());
        let by_hand = |name: &str| Begin::Automation {
            automation: name.parse().unwrap(),
            started_by: StartedBy::Hand,
        };

        let ids = begun(
            &runs,
            vec![
                by_hand("weekly"),
                by_hand("daily"),
                work(4, WorkKind::Design),
            ],
        )
        .await;

        assert_eq!(ids, ["weekly/3", "daily/5", "4/design-3"]);
    }

    #[test_log::test(tokio::test)]
    async fn what_a_runs_sessions_ran_with_reads_back_from_its_record() {
        let work_dir = tempfile::tempdir().unwrap();
        let settings = opus_for_triage();
        let session = work_dir.path().join("sessions/1/triage-1");
        let running = Runs::new(work_dir.path())
            .begin(work(1, WorkKind::Triage), at("2026-10-15T09:00:00Z"))
            .await
            .unwrap();

        running
            .session(&session, &settings, &Briefs::shipped())
            .reported(Reported {
                model: "claude-opus-5-5".into(),
            });
        running.end(at("2026-10-15T09:01:00Z"), Ending::Done).await;
        let recorded = Runs::new(work_dir.path()).read().await.unwrap();

        let [run] = &recorded[..] else {
            panic!("one run: {recorded:#?}");
        };
        let [
            SessionEntry {
                dir,
                ran: Some(ran),
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
            (Some("opus"), Some("claude-opus-5-5"), None)
        );
        assert_eq!(
            serde_json::to_value(&ran.layers.settings).unwrap(),
            serde_json::json!({
                "model": "operator",
                "effort": "harness",
                "session-timeout": "linewright",
                "maintenance": "linewright",
                "session-env": "linewright",
                "session-pass-env": "linewright",
            })
        );
        assert_eq!(
            ran.layers.prompts.get("triage.md"),
            Some(&Layer::Linewright)
        );
    }

    /// A triage run's settings, with the operator's model `opus`.
    fn opus_for_triage() -> SessionSettings {
        WorkSettings {
            flags_and_env: Partial {
                model: Some("opus".into()),
                ..Partial::default()
            },
            ..WorkSettings::default()
        }
        .for_run(
            RunKind::Stage(WorkKind::Triage),
            &RepoSettings::default(),
            &Environments::default(),
        )
    }

    #[test_log::test(tokio::test)]
    async fn a_report_written_after_its_run_ended_keeps_the_end() {
        let work_dir = tempfile::tempdir().unwrap();
        let runs = Runs::new(work_dir.path());
        let running = runs
            .begin(work(1, WorkKind::Triage), at("2026-10-15T09:00:00Z"))
            .await
            .unwrap();
        let recorder = running.session(
            &work_dir.path().join("sessions/1/triage-1"),
            &opus_for_triage(),
            &Briefs::shipped(),
        );

        recorder.reported(Reported {
            model: "claude-opus-5-5".into(),
        });
        running.end(at("2026-10-15T09:01:00Z"), Ending::Done).await;
        while tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks()
            > 0
        {
            tokio::task::yield_now().await;
        }
        let recorded = runs.read().await.unwrap();

        let [run] = &recorded[..] else {
            panic!("one run: {recorded:#?}");
        };
        assert_eq!(
            (
                run.state,
                run.sessions
                    .iter()
                    .map(|session| session.ran.as_ref().and_then(Ran::model_ran))
                    .collect::<Vec<_>>()
            ),
            (
                State::Ended {
                    at: at("2026-10-15T09:01:00Z"),
                    ending: Ending::Done
                },
                vec![Some("claude-opus-5-5")]
            )
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_run_of_the_store_records_its_input_and_one_kept_as_files_cannot() {
        let work_dir = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let begin = || work(1, WorkKind::Triage);
        let started = at("2026-10-15T09:00:00Z");

        let stored = Runs::new(work_dir.path())
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

    #[test_log::test]
    fn runs_written_as_files_read_back_from_where_they_are_written() {
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
                Started::new("weekly/3".parse().unwrap(), Some(StartedBy::Hand)).unwrap(),
                at("2026-10-15T10:00:00Z"),
                state,
                vec![],
                None,
            )
        };
        let files = RunFiles::new(out.path());

        files.write(&handed_back).unwrap();
        files.write(&in_flight(State::InFlight)).unwrap();
        let mut read = files.read_legacy().unwrap();

        sort_newest_first(&mut read);
        assert_eq!(read, [in_flight(State::Interrupted), handed_back]);
        let record: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(out.path().join("runs/items/7/implement-2.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            record["sessions"],
            serde_json::json!(["sessions/7/implement-2"])
        );
        assert!(out.path().join("runs/automations/weekly/3.json").exists());
    }
}
