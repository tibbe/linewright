use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::domain::Number;
use crate::runs::files::RunFiles;
use crate::runs::{self, Recorded, RunId, Runs, Started, State};
use crate::transcript::{
    self, After, FollowedLine, Follower, InFlight, ReadTo, SessionRecord, Transcript, Verbosity,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "QueryString", into = "QueryString")]
pub enum Which {
    /// The item's run in flight, or else its latest run that started a
    /// session.
    Item(Number),
    /// The run `id`, which must be a run on `item` when one is given.
    Run { id: RunId, item: Option<Number> },
}

#[derive(Serialize, Deserialize)]
struct QueryString {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    item: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run: Option<RunId>,
}

impl From<Which> for QueryString {
    fn from(which: Which) -> QueryString {
        match which {
            Which::Item(item) => QueryString {
                item: Some(item),
                run: None,
            },
            Which::Run { id, item } => QueryString {
                item,
                run: Some(id),
            },
        }
    }
}

impl TryFrom<QueryString> for Which {
    type Error = &'static str;

    fn try_from(query: QueryString) -> Result<Which, &'static str> {
        Which::new(query.item, query.run)
    }
}

impl Which {
    pub fn new(item: Option<Number>, run: Option<RunId>) -> Result<Which, &'static str> {
        match (item, run) {
            (item, Some(id)) => Ok(Which::Run { id, item }),
            (Some(item), None) => Ok(Which::Item(item)),
            (None, None) => Err("an item or a run is needed"),
        }
    }

    pub fn item(&self) -> Option<Number> {
        match self {
            Which::Item(item) => Some(*item),
            Which::Run { item, .. } => *item,
        }
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum NoSuchRun {
    #[error("{0} has no runs")]
    NoRuns(Number),
    #[error("there is no run {0}")]
    NoRun(RunId),
    #[error("{run} is not a run on {item}")]
    NotOf { run: RunId, item: Number },
}

pub fn pick(recorded: &[Recorded], which: &Which) -> Result<Recorded, NoSuchRun> {
    match which {
        Which::Run { id, item } => {
            let run = recorded
                .iter()
                .find(|run| run.id == *id)
                .cloned()
                .ok_or_else(|| NoSuchRun::NoRun(id.clone()))?;
            match item {
                Some(item) if !runs::of_item(recorded, *item).contains(&run) => {
                    Err(NoSuchRun::NotOf {
                        run: run.id,
                        item: *item,
                    })
                }
                _ => Ok(run),
            }
        }
        Which::Item(number) => {
            let mut runs = runs::of_item(recorded, *number);
            runs.sort_by_key(|run| (run.started_at, run.id.n));
            let in_flight = runs.iter().rposition(|run| run.state == State::InFlight);
            let with_session = || runs.iter().rposition(|run| !run.sessions.is_empty());
            let latest = in_flight
                .or_else(with_session)
                .or(runs.len().checked_sub(1))
                .ok_or(NoSuchRun::NoRuns(*number))?;
            Ok(runs.swap_remove(latest))
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LogsError {
    #[error(transparent)]
    NoSuchRun(#[from] NoSuchRun),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Why `which` names no run of none.
fn no_run_among_none(which: &Which) -> NoSuchRun {
    match which {
        Which::Item(number) => NoSuchRun::NoRuns(*number),
        Which::Run { id, .. } => NoSuchRun::NoRun(id.clone()),
    }
}

/// Where runs are kept: in the database, or as files under a directory.
#[derive(Clone)]
pub enum KeptRuns {
    Database(Runs),
    Files(RunFiles),
}

impl KeptRuns {
    /// Run `id`, or `None` if it has no record; a record that does not read
    /// fails.
    async fn try_read_run(&self, id: &RunId) -> anyhow::Result<Option<Recorded>> {
        match self {
            KeptRuns::Database(runs) => runs.try_read_run(id).await,
            KeptRuns::Files(files) => files.read(id),
        }
    }

    /// As [`KeptRuns::try_read_run`], with a record that does not read
    /// logged and taken as none.
    async fn read_run(&self, id: &RunId) -> Option<Recorded> {
        match self {
            KeptRuns::Database(runs) => runs.read_run(id).await,
            KeptRuns::Files(files) => files
                .read(id)
                .inspect_err(
                    |e| warn!(run = %id, error = format!("{e:#}"), "ignoring a run's record"),
                )
                .ok()
                .flatten(),
        }
    }

    /// The runs among which `which` is picked.
    async fn read_for(&self, which: &Which) -> anyhow::Result<Vec<Recorded>> {
        match (self, which) {
            (KeptRuns::Database(runs), Which::Item(_)) => runs.read().await,
            (KeptRuns::Files(files), Which::Item(item)) => files.read_of_item(*item),
            (_, Which::Run { id, .. }) => Ok(self
                .try_read_run(id)
                .await
                .with_context(|| format!("reading the record of {id}"))?
                .into_iter()
                .collect()),
        }
    }
}

/// The run `which` names. A run named by its ID whose record does not read
/// is an error, not a run that is missing. Its record is read once, so a
/// record that stops reading in between is never taken as missing.
async fn pick_from(kept: &KeptRuns, which: &Which) -> Result<Recorded, LogsError> {
    let recorded = kept.read_for(which).await?;
    Ok(pick(&recorded, which)?)
}

/// The transcript of the run `which` names of `runs`, or of none where there
/// is no database.
pub async fn answer(runs: Option<&Runs>, which: &Which) -> Result<Transcript, LogsError> {
    let runs = runs.ok_or_else(|| no_run_among_none(which))?;
    let run = pick_from(&KeptRuns::Database(runs.clone()), which).await?;
    let mut records = run
        .sessions
        .iter()
        .map(|session| {
            let record = SessionRecord::read(&session.dir)
                .with_context(|| format!("reading the session in {}", session.dir.display()))?;
            Ok(SessionRecord {
                ran: session.ran().cloned(),
                ..record
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let in_flight = (run.state == State::InFlight).then(|| {
        if let Some(last) = records.last_mut() {
            last.output
                .truncate(transcript::whole_lines(&last.output).len());
        }
        let written = records.iter().rposition(|record| !record.output.is_empty());
        written.map_or(InFlight::Unread, |i| {
            InFlight::ReadTo(ReadTo {
                session: i + 1,
                line: records[i].output.lines().count(),
            })
        })
    });
    let last = records.len();
    let sessions = records
        .iter()
        .enumerate()
        .map(|(i, record)| {
            let written = transcript::Written::of(i + 1, last, in_flight.is_none());
            transcript::parse(record, written)
        })
        .collect();
    Ok(Transcript {
        run: run.started(),
        in_flight,
        sessions,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "FollowQueryString", into = "FollowQueryString")]
pub struct FollowQuery {
    pub which: Which,
    pub verbosity: Verbosity,
}

#[derive(Serialize, Deserialize)]
struct FollowQueryString {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    item: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run: Option<RunId>,
    #[serde(default)]
    verbose: bool,
}

impl From<FollowQuery> for FollowQueryString {
    fn from(query: FollowQuery) -> FollowQueryString {
        let QueryString { item, run } = query.which.into();
        FollowQueryString {
            item,
            run,
            verbose: query.verbosity == Verbosity::Full,
        }
    }
}

impl TryFrom<FollowQueryString> for FollowQuery {
    type Error = &'static str;

    fn try_from(query: FollowQueryString) -> Result<FollowQuery, &'static str> {
        Ok(FollowQuery {
            which: Which::new(query.item, query.run)?,
            verbosity: Verbosity::of(query.verbose),
        })
    }
}

/// How often a followed run's records are read again.
const FOLLOW_EVERY: Duration = Duration::from_millis(100);

/// A run's records, read as they grow.
pub struct RunRecords {
    kept: KeptRuns,
    run: Started,
    sessions: Vec<SessionRecord>,
    ended: bool,
}

impl RunRecords {
    /// The records of the run `which` names of the runs `kept`.
    pub async fn start(kept: KeptRuns, which: &Which) -> Result<RunRecords, LogsError> {
        let run = pick_from(&kept, which).await?.started();
        Ok(RunRecords {
            kept,
            run,
            sessions: vec![],
            ended: false,
        })
    }

    pub fn run(&self) -> &RunId {
        self.run.id()
    }

    pub fn started(&self) -> &Started {
        &self.run
    }

    /// Reads what the run's records hold now: whether it has ended, and
    /// each of its sessions' records, with what it ran with.
    pub async fn read(&mut self) -> anyhow::Result<()> {
        let recorded = self
            .kept
            .read_run(self.run.id())
            .await
            .with_context(|| format!("reading the record of {}", self.run.id()))?;
        self.ended = recorded.state != State::InFlight;
        for (i, session) in recorded.sessions.iter().enumerate() {
            if i == self.sessions.len() {
                self.sessions.push(SessionRecord {
                    ran: session.ran().cloned(),
                    ..SessionRecord::default()
                });
            }
            self.sessions[i]
                .read_more(&session.dir)
                .with_context(|| format!("reading the session in {}", session.dir.display()))?;
        }
        Ok(())
    }

    /// What `made` makes of the run's records, read again until it makes
    /// something or the run has ended; `None` once the run had ended when
    /// they were last read.
    pub async fn next<T>(
        &mut self,
        mut made: impl FnMut(&RunRecords) -> Vec<T>,
    ) -> anyhow::Result<Option<Vec<T>>> {
        if self.ended {
            return Ok(None);
        }
        loop {
            self.read().await?;
            let made = made(self);
            if !made.is_empty() || self.ended {
                return Ok(Some(made));
            }
            tokio::time::sleep(FOLLOW_EVERY).await;
        }
    }

    /// Its sessions' records, as last read, in order.
    pub fn sessions(&self) -> &[SessionRecord] {
        &self.sessions
    }

    /// Whether the run had ended when its records were last read.
    pub fn ended(&self) -> bool {
        self.ended
    }
}

/// A run's transcript followed as `logs` shows it.
pub struct Following {
    records: RunRecords,
    follower: Follower,
}

impl Following {
    pub async fn start(
        runs: Option<&Runs>,
        which: &Which,
        verbosity: Verbosity,
        after: Option<After>,
    ) -> Result<Following, LogsError> {
        let runs = runs.ok_or_else(|| no_run_among_none(which))?;
        let records = RunRecords::start(KeptRuns::Database(runs.clone()), which).await?;
        let follower = Follower::new(records.started().clone(), verbosity, after);
        Ok(Following { records, follower })
    }

    pub fn run(&self) -> &RunId {
        self.records.run()
    }

    /// The next lines to show, each with where it was made, once there are
    /// some or the run has ended; `None` once it has and every line has
    /// been given.
    pub async fn next(&mut self) -> anyhow::Result<Option<Vec<FollowedLine>>> {
        let Following { records, follower } = self;
        records.next(|records| lines(follower, records)).await
    }

    /// Whether the run had ended when its records were last read.
    pub fn ended(&self) -> bool {
        self.records.ended()
    }

    /// The next lines to show of what the run's records hold now, without
    /// waiting for more.
    pub async fn read(&mut self) -> anyhow::Result<Vec<FollowedLine>> {
        self.records.read().await?;
        Ok(lines(&mut self.follower, &self.records))
    }
}

/// The next lines `follower` makes of `records` as last read.
fn lines(follower: &mut Follower, records: &RunRecords) -> Vec<FollowedLine> {
    if records.ended() {
        follower.end(records.sessions())
    } else {
        follower.update(records.sessions())
    }
}
