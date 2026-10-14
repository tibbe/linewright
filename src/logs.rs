//! `lw logs`: which run's transcript to show, and the transcript as
//! the sessions' records keep it (`docs/specs/monitoring.md`, "logs").

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::domain::Number;
use crate::runs::{Recorded, RepoWorkDir, RunId, RunOf, Runs, State};
use crate::transcript::{self, SessionRecord, Transcript};

/// Which run's transcript to show.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "QueryString", into = "QueryString")]
pub enum Which {
    /// The item's run in flight, or else its latest run that started a
    /// session.
    Item(Number),
    /// The run `id`, which must be a run on `item` when one is given.
    Run { id: RunId, item: Option<Number> },
}

/// [`Which`] as the query string of a request for it.
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
    /// The run `run` names, which must be on `item` when both are given, or
    /// else `item`'s.
    pub fn new(item: Option<Number>, run: Option<RunId>) -> Result<Which, &'static str> {
        match (item, run) {
            (item, Some(id)) => Ok(Which::Run { id, item }),
            (Some(item), None) => Ok(Which::Item(item)),
            (None, None) => Err("an item or a run is needed"),
        }
    }

    /// `self` as the query string of a request for it.
    pub fn query_string(&self) -> String {
        serde_urlencoded::to_string(self).expect("a query serializes")
    }
}

/// Why `logs` has no run to show.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum NoSuchRun {
    #[error("{0} has no runs")]
    NoRuns(Number),
    #[error("there is no run {0}")]
    NoRun(RunId),
    #[error("{run} is not a run on {item}")]
    NotOf { run: RunId, item: Number },
}

/// The run of `recorded` that `which` names.
pub fn pick(recorded: Vec<Recorded>, which: &Which) -> Result<Recorded, NoSuchRun> {
    match which {
        Which::Run { id, item } => {
            let run = recorded
                .into_iter()
                .find(|run| run.id == *id)
                .ok_or_else(|| NoSuchRun::NoRun(id.clone()))?;
            match (item, &run.id.of) {
                (Some(item), RunOf::Work { item: of, .. }) if of == item => Ok(run),
                (Some(item), _) => Err(NoSuchRun::NotOf {
                    run: run.id,
                    item: *item,
                }),
                (None, _) => Ok(run),
            }
        }
        Which::Item(number) => {
            let mut runs: Vec<Recorded> = recorded
                .into_iter()
                .filter(|run| run.id.of.item() == Some(*number))
                .collect();
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

/// Why `logs` has no transcript to show.
#[derive(Debug, thiserror::Error)]
pub enum LogsError {
    #[error(transparent)]
    NoSuchRun(#[from] NoSuchRun),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// The transcript of the run `which` names, from the records in
/// `work_dir`, as `serve` gives it and as `logs` gives it with no `serve`
/// running.
pub fn answer(work_dir: &RepoWorkDir, which: &Which) -> Result<Transcript, LogsError> {
    let recorded = Runs::new(&work_dir.resolve()).read()?;
    let run = pick(recorded, which)?;
    let sessions = run
        .sessions
        .iter()
        .map(|dir| {
            SessionRecord::read(dir)
                .map(|record| transcript::parse(&record))
                .with_context(|| format!("reading the session in {}", dir.display()))
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(Transcript {
        run: run.id,
        sessions,
    })
}
