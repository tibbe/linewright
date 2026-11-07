use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::domain::Number;
use crate::runs::{self, Recorded, RepoWorkDir, RunId, Runs, SessionEntry, Started, State};
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

/// The run `which` names. A run named by its ID whose record does not read
/// is an error, not a run that is missing.
fn pick_from(runs: &Runs, which: &Which) -> Result<Recorded, LogsError> {
    if let Which::Run { id, .. } = which {
        runs.try_read_run(id)
            .with_context(|| format!("reading the record of {id}"))?;
    }
    Ok(pick(&runs.read()?, which)?)
}

pub fn answer(work_dir: &RepoWorkDir, which: &Which) -> Result<Transcript, LogsError> {
    let run = pick_from(&Runs::new(&work_dir.resolve()), which)?;
    let mut records = run
        .sessions
        .iter()
        .map(|SessionEntry { dir, ran }| {
            let record = SessionRecord::read(dir)
                .with_context(|| format!("reading the session in {}", dir.display()))?;
            Ok(SessionRecord {
                ran: ran.clone(),
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
    Ok(Transcript {
        run: run.started(),
        in_flight,
        sessions: records.iter().map(transcript::parse).collect(),
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
    runs: Runs,
    run: Started,
    sessions: Vec<SessionRecord>,
    ended: bool,
}

impl RunRecords {
    pub fn start(work_dir: &RepoWorkDir, which: &Which) -> Result<RunRecords, LogsError> {
        let runs = Runs::new(&work_dir.resolve());
        let run = pick_from(&runs, which)?.started();
        Ok(RunRecords {
            runs,
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
    pub fn read(&mut self) -> anyhow::Result<()> {
        let recorded = self
            .runs
            .read_run(self.run.id())
            .with_context(|| format!("reading the record of {}", self.run.id()))?;
        self.ended = recorded.state != State::InFlight;
        for (i, SessionEntry { dir, ran }) in recorded.sessions.iter().enumerate() {
            if i == self.sessions.len() {
                self.sessions.push(SessionRecord {
                    ran: ran.clone(),
                    ..SessionRecord::default()
                });
            }
            self.sessions[i]
                .read_more(dir)
                .with_context(|| format!("reading the session in {}", dir.display()))?;
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
            self.read()?;
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
    pub fn start(
        work_dir: &RepoWorkDir,
        which: &Which,
        verbosity: Verbosity,
        after: Option<After>,
    ) -> Result<Following, LogsError> {
        let records = RunRecords::start(work_dir, which)?;
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
    pub fn read(&mut self) -> anyhow::Result<Vec<FollowedLine>> {
        self.records.read()?;
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
