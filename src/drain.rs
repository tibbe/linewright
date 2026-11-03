//! What `lw drain` says as it waits for `serve` to drain
//! (`docs/specs/operation.md`, "Stopping").

use chrono::TimeDelta;
use serde::{Deserialize, Serialize};

use crate::domain::Timestamp;
use crate::ending::{Ending, StopBy};
use crate::runs::RunId;
use crate::status::Paint;
use crate::winding_up::Remaining;

/// A drain request to the local API.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asked {
    /// With none, the drain waits however long.
    pub timeout: Option<Timeout>,
}

/// How long until a drain becomes a stop, in milliseconds on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "i64", from = "i64")]
pub struct Timeout(TimeDelta);

impl Timeout {
    /// As `--timeout` has it, such as 2h.
    pub fn parse(value: &str) -> Result<Timeout, String> {
        humantime::parse_duration(value)
            .ok()
            .and_then(|timeout| TimeDelta::from_std(timeout).ok())
            .map(Timeout)
            .ok_or_else(|| format!("{value:?} is not a duration (2h)"))
    }

    /// `None` for a deadline beyond any time there is.
    pub fn deadline(self, from: Timestamp) -> Option<Timestamp> {
        from.checked_add_signed(self.0)
    }
}

impl From<Timeout> for i64 {
    fn from(timeout: Timeout) -> i64 {
        timeout.0.num_milliseconds()
    }
}

impl From<i64> for Timeout {
    fn from(ms: i64) -> Timeout {
        Timeout(TimeDelta::milliseconds(ms))
    }
}

/// The local API's answer, once a stop begins or `serve` stops answering,
/// to what began the stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stop {
    /// `None` if `serve` stops answering without a stop, as after a drain.
    pub by: Option<StopBy>,
}

/// The local API's answer to a drain request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Taken {
    pub was: Was,
    /// Tells this `serve` from one started after it exits.
    pub serving_since: Timestamp,
    /// The runs in flight then.
    pub runs: Vec<RunId>,
}

/// The local API's answer to what `serve` waits for before it exits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exiting {
    /// Tells this `serve` from one started after it exits.
    pub serving_since: Timestamp,
    /// Empty until the wait begins.
    pub waits: Vec<Remaining>,
}

/// Where `serve` was in ending when it took a drain request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Was {
    Serving,
    Draining,
    Stopping,
}

impl Was {
    /// `None` for a `serve` exiting at once, which takes no request.
    pub fn of(ending: Ending) -> Option<Was> {
        match ending {
            Ending::Serving => Some(Was::Serving),
            Ending::Draining { .. } => Some(Was::Draining),
            Ending::Stopping { .. } => Some(Was::Stopping),
            Ending::Exited => None,
        }
    }
}

/// One thing `drain` says, on a line of its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Said {
    Taken {
        was: Was,
    },
    WaitingFor {
        run: RunId,
    },
    Ended {
        run: RunId,
    },
    /// The drain's deadline passed with runs in flight.
    BecameAStop,
    /// `serve` waits for `what` before it exits.
    WaitingForExit {
        what: Remaining,
    },
    EndedForExit {
        what: Remaining,
    },
    /// `serve` has exited: it no longer answers and has released its claim
    /// on the repository, or another has started since.
    Exited,
}

/// The runs a drain waits for, then what `serve` waits for before it exits.
#[derive(Debug, Default)]
pub struct Waiting {
    runs: Vec<RunId>,
    exiting: Vec<Remaining>,
}

impl Waiting {
    /// What changed since the runs in flight were last seen: the runs in
    /// `in_flight` it did not wait for yet, and the runs it waited for that
    /// are no longer in flight, which have ended.
    pub fn seen(&mut self, in_flight: &[RunId]) -> Vec<Said> {
        let (ended, new) = changed(&mut self.runs, in_flight);
        let ended = ended.into_iter().map(|run| Said::Ended { run });
        let new = new.into_iter().map(|run| Said::WaitingFor { run });
        ended.chain(new).collect()
    }

    /// What changed since what `serve` waits for before it exits was last
    /// seen, as [`Waiting::seen`] has it for runs.
    pub fn seen_exiting(&mut self, waits: &[Remaining]) -> Vec<Said> {
        let (ended, new) = changed(&mut self.exiting, waits);
        let ended = ended.into_iter().map(|what| Said::EndedForExit { what });
        let new = new.into_iter().map(|what| Said::WaitingForExit { what });
        ended.chain(new).collect()
    }

    /// Once `serve` has exited, nothing it waited for is left.
    pub fn exited(self) -> Vec<Said> {
        let runs = self.runs.into_iter().map(|run| Said::Ended { run });
        let exiting = self
            .exiting
            .into_iter()
            .map(|what| Said::EndedForExit { what });
        runs.chain(exiting).chain([Said::Exited]).collect()
    }
}

/// Updates `listed` to `now`, returning what left it and what joined it.
fn changed<T: Clone + PartialEq>(listed: &mut Vec<T>, now: &[T]) -> (Vec<T>, Vec<T>) {
    let (still, gone): (Vec<T>, Vec<T>) = std::mem::take(listed)
        .into_iter()
        .partition(|seen| now.contains(seen));
    let new: Vec<T> = now
        .iter()
        .filter(|seen| !still.contains(seen))
        .cloned()
        .collect();
    *listed = still;
    listed.extend(new.iter().cloned());
    (gone, new)
}

/// What `drain` says once `serve` begins to stop, by what if anything.
pub fn stop_begun(by: Option<StopBy>) -> Vec<Said> {
    match by {
        Some(StopBy::Deadline) => vec![Said::BecameAStop],
        Some(StopBy::Signal) | None => vec![],
    }
}

/// As `label: value`, ending in a newline.
pub fn render(said: &Said, colour: bool) -> String {
    let paint = Paint(colour);
    let line = match said {
        Said::Taken { was } => match was {
            Was::Serving => "drain: begun".into(),
            Was::Draining => paint.yellow("drain: already under way"),
            Was::Stopping => paint.yellow("stop: already under way"),
        },
        Said::WaitingFor { run } => format!("waiting for: {run}"),
        Said::Ended { run } => format!("ended: {run}"),
        Said::BecameAStop => paint.yellow("drain: became a stop at its deadline"),
        Said::WaitingForExit { what } => format!("waiting for: {what}"),
        Said::EndedForExit { what } => format!("ended: {what}"),
        Said::Exited => "serve: exited".into(),
    };
    format!("{line}\n")
}

/// As one JSON object, ending in a newline.
pub fn json(said: &Said) -> String {
    format!(
        "{}\n",
        serde_json::to_string(said).expect("the answers serialize")
    )
}
