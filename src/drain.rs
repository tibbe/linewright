//! What `lw drain` says as it waits for `serve` to drain
//! (`docs/specs/operation.md`, "Stopping").

use serde::{Deserialize, Serialize};

use crate::domain::Timestamp;
use crate::ending::Ending;
use crate::runs::RunId;
use crate::status::Paint;

/// The local API's answer to a drain request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Taken {
    pub was: Was,
    /// Tells this `serve` from one started after it exits.
    pub serving_since: Timestamp,
    /// The runs in flight then.
    pub runs: Vec<RunId>,
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
            Ending::Draining => Some(Was::Draining),
            Ending::Stopping => Some(Was::Stopping),
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
    /// `serve` has exited: its socket is gone.
    Exited,
}

/// The runs a drain waits for.
#[derive(Debug, Default)]
pub struct Waiting(Vec<RunId>);

impl Waiting {
    /// What changed since the runs in flight were last seen: the runs in
    /// `in_flight` it did not wait for yet, and the runs it waited for that
    /// are no longer in flight, which have ended.
    pub fn seen(&mut self, in_flight: &[RunId]) -> Vec<Said> {
        let (still, ended): (Vec<RunId>, Vec<RunId>) = std::mem::take(&mut self.0)
            .into_iter()
            .partition(|run| in_flight.contains(run));
        let new: Vec<RunId> = in_flight
            .iter()
            .filter(|run| !still.contains(run))
            .cloned()
            .collect();
        self.0 = still;
        self.0.extend(new.iter().cloned());
        let ended = ended.into_iter().map(|run| Said::Ended { run });
        let new = new.into_iter().map(|run| Said::WaitingFor { run });
        ended.chain(new).collect()
    }

    /// Once `serve` has exited, no run it waited for is in flight.
    pub fn exited(self) -> Vec<Said> {
        self.0
            .into_iter()
            .map(|run| Said::Ended { run })
            .chain([Said::Exited])
            .collect()
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
