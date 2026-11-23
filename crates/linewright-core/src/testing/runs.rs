use std::collections::BTreeMap;

use chrono::TimeDelta;

use crate::domain::Number;
use crate::reconcile::WorkKind;
use crate::runs::{
    Ending, Layers, Ran, Recorded, RunId, RunOf, SessionEntry, SessionKept, Started, StartedBy,
    State,
};
use crate::testing::at;
use crate::work_settings::Effort;

/// A run that started as `started` says at `start` and ended done a minute
/// later, having started `sessions`.
pub fn ended_done(started: Started, start: &str, sessions: Vec<SessionEntry>) -> Recorded {
    Recorded::new(
        started,
        at(start),
        State::Ended {
            at: at(start) + TimeDelta::minutes(1),
            ending: Ending::Done,
        },
        sessions,
        None,
    )
}

pub fn ended(of: RunOf, n: u32, by: Option<StartedBy>, start: &str) -> Recorded {
    ended_done(Started::new(RunId { of, n }, by).unwrap(), start, vec![])
}

pub fn work(item: u64, stage: WorkKind, n: u32, start: &str) -> Recorded {
    let of = RunOf::Work {
        item: Number(item),
        stage,
    };
    ended(of, n, None, start)
}

pub fn ran(model: Option<&str>, reported_model: Option<&str>, effort: Option<Effort>) -> Ran {
    Ran {
        model: model.map(str::to_string),
        reported_model: reported_model.map(str::to_string),
        effort,
        layers: Layers {
            settings: BTreeMap::new(),
            briefs: BTreeMap::new(),
        },
    }
}

pub fn session(n: usize, ran: Option<Ran>) -> SessionEntry {
    SessionEntry {
        dir: format!("sessions/1/triage-1/{n}").into(),
        kept: ran.map(|ran| SessionKept { ran, cost: None }),
    }
}
