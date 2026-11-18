//! What starts next of the open experiments (`docs/specs/benchmarks.md`,
//! Experiments: Order, Replays at once, and the state table).

use std::collections::BTreeMap;

use crate::benchmarks::{
    ExperimentId, ExperimentRecord, Shown, TrialKey, TrialRef, TrialState, WaitingFor,
};
use crate::holds::{ExperimentHold, InForce};

/// Whether the steps of experiments may start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Open,
    /// The first in rank of the holds over them.
    Held(ExperimentHold),
    /// A run of the pipeline waits for a place under `max-runs`.
    PipelineWaitsForRoom,
}

impl Gate {
    /// The gate of the steps under the holds `in_force`, while a run of the
    /// pipeline waits for room or not: held by the first in rank of the
    /// holds over an experiment.
    pub fn at(in_force: &InForce, pipeline_waits_for_room: bool) -> Gate {
        match in_force.holding_experiments() {
            Some(hold) => Gate::Held(hold),
            None if pipeline_waits_for_room => Gate::PipelineWaitsForRoom,
            None => Gate::Open,
        }
    }
}

/// A replay or a judge of a trial, to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    Replay(TrialRef),
    Judge(TrialRef),
}

impl Step {
    pub fn trial(self) -> TrialRef {
        match self {
            Step::Replay(trial) | Step::Judge(trial) => trial,
        }
    }
}

/// What starts, and how each open experiment shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// In the order they start.
    pub starts: Vec<Step>,
    /// Each open experiment: running, waiting, or ended once none of its
    /// trials is left to end.
    pub shown: BTreeMap<ExperimentId, Shown>,
}

/// What starts of `open`, the open experiments, with `in_flight` replays and
/// judges of experiments in flight of `max_replays` at most, and only once
/// `gate` is open. Experiments go in the order they were
/// launched. In each, a judge waiting for its replay's trial starts before
/// the next replay, and replays start by repetition, then case, then
/// configuration.
pub fn plan(open: &[ExperimentRecord], in_flight: usize, max_replays: usize, gate: Gate) -> Plan {
    let mut experiments: Vec<&ExperimentRecord> = open.iter().collect();
    experiments.sort_by_key(|record| record.id);
    let mut taken = in_flight;
    let mut plan = Plan::default();
    for record in experiments {
        let trial = |key: TrialKey| TrialRef {
            experiment: record.id,
            key,
        };
        let judges = record
            .trials
            .iter()
            .filter(|(_, state)| matches!(state, TrialState::Replayed { .. }))
            .map(|(key, _)| Step::Judge(trial(*key)));
        let replays = record
            .trials
            .iter()
            .filter(|(_, state)| matches!(state, TrialState::Pending))
            .map(|(key, _)| Step::Replay(trial(*key)));
        let mut started = record
            .trials
            .values()
            .any(|state| matches!(state, TrialState::Replaying | TrialState::Judging { .. }));
        let mut waiting = None;
        for step in judges.chain(replays) {
            let waits_for = match gate {
                Gate::Open if taken < max_replays => {
                    plan.starts.push(step);
                    taken += 1;
                    started = true;
                    continue;
                }
                Gate::Open => WaitingFor::FreeReplay {
                    in_flight: taken,
                    max_replays,
                },
                Gate::Held(hold) => WaitingFor::Hold(hold),
                Gate::PipelineWaitsForRoom => WaitingFor::Pipeline,
            };
            waiting.get_or_insert(waits_for);
        }
        let shown = match waiting {
            _ if started => Shown::Running,
            Some(waiting) => Shown::Waiting(waiting),
            None => Shown::Ended,
        };
        plan.shown.insert(record.id, shown);
    }
    plan
}
