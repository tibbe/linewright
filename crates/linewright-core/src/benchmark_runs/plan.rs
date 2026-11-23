//! What starts next of the open benchmark runs (`docs/specs/benchmarks.md`,
//! Benchmark runs: Order, Trials at once, and the state table).

use std::collections::BTreeMap;

use crate::benchmarks::{
    BenchmarkRunId, BenchmarkRunRecord, Shown, TrialKey, TrialRef, TrialState, WaitingFor,
};
use crate::holds::{BenchmarkRunHold, InForce};

/// Whether the steps of benchmark runs may start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Open,
    /// The first in rank of the holds over them.
    Held(BenchmarkRunHold),
    /// A run of the pipeline waits for a place under `max-runs`.
    PipelineWaitsForRoom,
}

impl Gate {
    /// The gate of the steps under the holds `in_force`, while a run of the
    /// pipeline waits for room or not: held by the first in rank of the
    /// holds over a benchmark run.
    pub fn at(in_force: &InForce, pipeline_waits_for_room: bool) -> Gate {
        match in_force.holding_benchmark_runs() {
            Some(hold) => Gate::Held(hold),
            None if pipeline_waits_for_room => Gate::PipelineWaitsForRoom,
            None => Gate::Open,
        }
    }
}

/// A trial or a scorer of a trial, to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    Trial(TrialRef),
    Scorer(TrialRef),
}

impl Step {
    pub fn trial(self) -> TrialRef {
        match self {
            Step::Trial(trial) | Step::Scorer(trial) => trial,
        }
    }
}

/// What starts, and how each open benchmark run shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// In the order they start.
    pub starts: Vec<Step>,
    /// Each open benchmark run: running, waiting, or ended once none of its
    /// trials is left to end.
    pub shown: BTreeMap<BenchmarkRunId, Shown>,
}

/// What starts of `open`, the open benchmark runs, with `in_flight` trials and
/// scorers of benchmark runs in flight of `max_trials` at most, and only once
/// `gate` is open. Benchmark runs go in the order they were
/// launched. In each, a scorer waiting for its trial starts before the
/// next trial, and trials start by repetition, then task, then
/// configuration.
pub fn plan(open: &[BenchmarkRunRecord], in_flight: usize, max_trials: usize, gate: Gate) -> Plan {
    let mut benchmark_runs: Vec<&BenchmarkRunRecord> = open.iter().collect();
    benchmark_runs.sort_by_key(|record| record.id);
    let mut taken = in_flight;
    let mut plan = Plan::default();
    for record in benchmark_runs {
        let trial = |key: TrialKey| TrialRef {
            benchmark_run: record.id,
            key,
        };
        let scorers = record
            .trials
            .iter()
            .filter(|(_, state)| matches!(state, TrialState::Ran { .. }))
            .map(|(key, _)| Step::Scorer(trial(*key)));
        let trials = record
            .trials
            .iter()
            .filter(|(_, state)| matches!(state, TrialState::Pending))
            .map(|(key, _)| Step::Trial(trial(*key)));
        let mut started = record
            .trials
            .values()
            .any(|state| matches!(state, TrialState::Running | TrialState::Scoring { .. }));
        let mut waiting = None;
        for step in scorers.chain(trials) {
            let waits_for = match gate {
                Gate::Open if taken < max_trials => {
                    plan.starts.push(step);
                    taken += 1;
                    started = true;
                    continue;
                }
                Gate::Open => WaitingFor::FreePlace {
                    in_flight: taken,
                    max_trials,
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
