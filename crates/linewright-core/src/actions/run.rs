//! The action that shows a run (`docs/specs/runs.md`, Run), and what it
//! shows: the model that the page renders its heading from and the tool
//! returns.

use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::access::{Permit, Read};
use crate::api::Sources;
use crate::domain::Timestamp;
use crate::logs;
use crate::runs::{Outcome, OutcomeName, Ran, Recorded, RunId, RunOf, StartedBy};
use crate::transcript::lines::{self, Stamped, Verbosity};
use crate::transcript::{End, Session};

/// A run as its screen shows it, with the names `runs` and `logs` give the
/// same facts: its heading, its sessions with their settings, and its
/// transcripts' entries.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct RunScreen {
    #[serde(flatten)]
    pub heading: RunHeading,
    /// The sessions it shows, in order.
    pub sessions: Vec<SessionScreen>,
    /// The entries of its transcripts, in order.
    pub entries: Vec<EntryScreen>,
}

/// An entry of a run's transcripts, as Run shows it, by the names `logs
/// --json` gives its facts.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct EntryScreen {
    /// Its own, as the transcript grows: what `before` names.
    pub id: String,
    #[serde(flatten)]
    pub line: Stamped,
}

/// A run's heading (`docs/specs/runs.md`, Run, Heading), as `runs` lists
/// the run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct RunHeading {
    pub id: RunId,
    /// Its item and stage, or its automation.
    #[serde(flatten)]
    pub of: RunOf,
    /// How an automation's run started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_by: Option<StartedBy>,
    pub started_at: Timestamp,
    /// How long it ran, or has run so far; `None` for an interrupted run.
    pub duration_seconds: Option<i64>,
    pub outcome: OutcomeName,
    /// Whether it ended without starting a session, which its sessions
    /// say.
    #[serde(skip)]
    pub started_no_session: bool,
}

/// A session of a run (`docs/specs/runs.md`, Run, A session's heading and
/// Working): its number, when it started and how long it has run, what it
/// ran with as `runs N` gives it, and while it goes on with no entry
/// running, what it does.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct SessionScreen {
    /// Its number in the run, from 1.
    pub session: usize,
    pub started_at: Option<Timestamp>,
    /// How long it has run, leaving out its waits for the usage limit to
    /// reset; `None` where when it started is not known.
    pub duration_seconds: Option<f64>,
    /// `None` for a session recorded before Linewright kept it.
    #[serde(flatten)]
    pub ran: Option<Ran>,
    /// While it waits for the usage limit to reset, until when.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_limit_resets_at: Option<Timestamp>,
    /// While a turn goes on, how long it has gone on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_seconds: Option<f64>,
}

impl SessionScreen {
    /// The `n`th session, from 1, as `session` holds its transcript, which
    /// ran with `ran`, as of `now`.
    fn of(n: usize, session: &Session, ran: Option<Ran>, now: Timestamp) -> SessionScreen {
        let idle = session.end == End::Going && !session.runs_an_entry();
        let waiting = session.waiting().filter(|_| idle);
        SessionScreen {
            session: n,
            started_at: session.started_at,
            duration_seconds: session.lasted_so_far().map(|lasted| lasted.seconds(now)),
            ran,
            usage_limit_resets_at: waiting.map(|wait| wait.until),
            working_seconds: (idle && waiting.is_none())
                .then(|| session.turn_so_far())
                .flatten()
                .map(|lasted| lasted.seconds(now)),
        }
    }
}

impl RunHeading {
    /// Whether it is an automation's run by hand.
    pub fn by_hand(&self) -> bool {
        self.started_by == Some(StartedBy::Hand)
    }

    /// `recorded`'s heading as of `now`.
    pub fn of(recorded: &Recorded, now: Timestamp) -> RunHeading {
        let run = recorded.clone().at(now);
        RunHeading {
            of: run.id.of.clone(),
            started_by: run.started().by(),
            started_at: run.started_at,
            duration_seconds: run.outcome.duration_seconds(),
            outcome: run.outcome.name(),
            started_no_session: !matches!(run.outcome, Outcome::InFlight { .. })
                && recorded.sessions.is_empty(),
            id: run.id,
        }
    }
}

/// Run `id`'s heading now, `None` where there is no such run.
pub async fn run_heading(
    _: &Permit<Read>,
    sources: &Sources,
    id: &RunId,
) -> Result<Option<RunHeading>> {
    let recorded = sources.runs.try_read_run(id).await?;
    Ok(recorded.map(|recorded| RunHeading::of(&recorded, sources.clock.now())))
}

/// Run `id` as its screen shows it now, its transcripts in the view
/// `verbosity` says; `None` where there is no such run.
pub async fn get_run(
    _: &Permit<Read>,
    sources: &Sources,
    id: &RunId,
    verbosity: Verbosity,
) -> Result<Option<RunScreen>> {
    let Some(recorded) = sources.runs.try_read_run(id).await? else {
        return Ok(None);
    };
    let now = sources.clock.now();
    let transcript = logs::transcript_of(&recorded)?;
    let sessions = transcript
        .sessions
        .iter()
        .zip(&recorded.sessions)
        .enumerate()
        .filter(|(i, _)| lines::shows_session(&transcript, i + 1))
        .map(|(i, (session, entry))| SessionScreen::of(i + 1, session, entry.ran().cloned(), now))
        .collect();
    let entries = lines::entries(&transcript, verbosity)
        .into_iter()
        .map(|(entry, line)| EntryScreen { id: entry, line })
        .collect();
    Ok(Some(RunScreen {
        heading: RunHeading::of(&recorded, now),
        sessions,
        entries,
    }))
}
