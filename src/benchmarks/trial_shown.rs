//! The state a trial shows (`docs/specs/benchmarks.md`, Benchmark run, A
//! trial's status), decided from what is kept of it.

use crate::phrase::Status;
use crate::results::Verdict;

use super::{BenchmarkRunState, NoScore, TrialEnd, TrialState};

/// The state a trial shows: its Pass rate's name and status.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrialShown<'a> {
    Cancelled,
    Waiting,
    NoScore(&'a NoScore),
    Pass,
    Fail,
    Leak { correctness: Verdict },
    Scoring,
    Running,
}

impl TrialShown<'_> {
    /// The state `trial` shows, in a benchmark run in the state
    /// `benchmark_run`.
    pub fn of(_benchmark_run: BenchmarkRunState, trial: &TrialState) -> TrialShown<'_> {
        match trial {
            TrialState::Ended(TrialEnd::Cancelled { .. }) => TrialShown::Cancelled,
            TrialState::Pending | TrialState::Ran { .. } => TrialShown::Waiting,
            TrialState::Ended(TrialEnd::Unscored(no_score)) => TrialShown::NoScore(no_score),
            TrialState::Ended(TrialEnd::Scored { score, .. }) => {
                if score.leak {
                    TrialShown::Leak {
                        correctness: score.correctness,
                    }
                } else if score.correctness == Verdict::Pass {
                    TrialShown::Pass
                } else {
                    TrialShown::Fail
                }
            }
            TrialState::Scoring { .. } => TrialShown::Scoring,
            TrialState::Running => TrialShown::Running,
        }
    }

    /// The name of its state.
    pub fn name(&self) -> String {
        match self {
            TrialShown::Cancelled => "no score: cancelled".into(),
            TrialShown::Waiting => "waiting".into(),
            TrialShown::NoScore(NoScore::Outcome(_)) => {
                "no score: its run is not done or handed back".into()
            }
            TrialShown::NoScore(NoScore::Scorer { why, .. }) => {
                format!("no score: the scorer made none: {why}")
            }
            TrialShown::Pass => "pass".into(),
            TrialShown::Fail => "fail".into(),
            TrialShown::Leak { correctness } => format!("{}, with a leak", correctness.name()),
            TrialShown::Scoring => "scoring".into(),
            TrialShown::Running => "running".into(),
        }
    }

    /// The status it shows as: a run not done or handed back shows as its
    /// outcome does (`docs/specs/runs.md`, Ending).
    pub fn status(&self) -> Status {
        match self {
            TrialShown::Cancelled => Status::Cancelled,
            TrialShown::Waiting => Status::Paused,
            TrialShown::NoScore(NoScore::Outcome(run)) => run.outcome.status(),
            TrialShown::NoScore(NoScore::Scorer { .. })
            | TrialShown::Fail
            | TrialShown::Leak { .. } => Status::Failed,
            TrialShown::Pass => Status::Done,
            TrialShown::Scoring | TrialShown::Running => Status::InFlight,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::benchmarks::{ScoreFigures, TrialFigures};

    #[track_caller]
    fn ended(outcome: &str) -> TrialFigures {
        TrialFigures {
            outcome: serde_json::from_value(outcome.into()).unwrap(),
            money: None,
            time: Duration::from_secs(60),
        }
    }

    fn scored(correctness: Verdict, leak: bool) -> TrialState {
        TrialState::Ended(TrialEnd::Scored {
            run: ended("done"),
            score: ScoreFigures {
                correctness,
                leak,
                scorer_money: None,
            },
        })
    }

    fn unscored(no_score: NoScore) -> TrialState {
        TrialState::Ended(TrialEnd::Unscored(no_score))
    }

    fn shown(state: &TrialState) -> (Status, String) {
        let shown = TrialShown::of(BenchmarkRunState::Open, state);
        (shown.status(), shown.name())
    }

    /// `docs/specs/benchmarks.md`, Benchmark run, A trial's status, and the
    /// Pass rate column's word.
    #[test_log::test]
    fn each_trial_shows_as_the_status_its_state_gives() {
        let states = [
            TrialState::Pending,
            TrialState::Ran { run: ended("done") },
            TrialState::Running,
            TrialState::Scoring { run: ended("done") },
            scored(Verdict::Pass, false),
            scored(Verdict::Pass, true),
            scored(Verdict::Fail, false),
            unscored(NoScore::Scorer {
                run: ended("done"),
                why: "it gave up".into(),
                scorer_money: None,
            }),
        ];

        let shown: Vec<(Status, String)> = states.iter().map(shown).collect();

        assert_eq!(
            shown,
            [
                (Status::Paused, "waiting".into()),
                (Status::Paused, "waiting".into()),
                (Status::InFlight, "running".into()),
                (Status::InFlight, "scoring".into()),
                (Status::Done, "pass".into()),
                (Status::Failed, "pass, with a leak".into()),
                (Status::Failed, "fail".into()),
                (
                    Status::Failed,
                    "no score: the scorer made none: it gave up".into()
                ),
            ]
        );
    }

    /// `docs/specs/benchmarks.md`, Benchmark run, A trial's status: as its
    /// run's outcome does (`docs/specs/runs.md`, Ending).
    #[test_log::test]
    fn a_trial_whose_run_is_not_done_or_handed_back_shows_as_its_runs_outcome() {
        let outcomes = [
            "failed",
            "cancelled",
            "usage-limit",
            "interrupted",
            "handed-back",
        ];

        let shown: Vec<Status> = outcomes
            .map(|outcome| unscored(NoScore::Outcome(ended(outcome))))
            .iter()
            .map(|state| shown(state).0)
            .collect();

        assert_eq!(
            shown,
            [
                Status::Failed,
                Status::Cancelled,
                Status::Cancelled,
                Status::Cancelled,
                Status::Waiting,
            ]
        );
    }

    /// A trial of a cancelled benchmark run shows as a cancelled run does
    /// (`docs/specs/runs.md`, Ending).
    #[test_log::test]
    fn a_trial_its_benchmark_runs_cancel_left_without_a_score_shows_cancelled() {
        let states = [
            TrialState::Ended(TrialEnd::Cancelled {
                run: Some(ended("done")),
            }),
            TrialState::Ended(TrialEnd::Cancelled { run: None }),
        ];

        let shown = states.map(|state| shown(&state).0);

        assert_eq!(shown, [Status::Cancelled; 2]);
    }
}
