//! The state a trial shows (`docs/specs/benchmarks.md`, Benchmark run, A
//! trial's status), decided from what is kept of it and of its benchmark run.

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
    Leak,
    Scoring,
    Running,
}

impl TrialShown<'_> {
    /// The state `trial` shows, in a benchmark run in the state
    /// `benchmark_run`: the first row of the spec's table that fits it.
    pub fn of(benchmark_run: BenchmarkRunState, trial: &TrialState) -> TrialShown<'_> {
        let cancelled = matches!(benchmark_run, BenchmarkRunState::Cancelled { .. });
        match trial {
            TrialState::Ended(TrialEnd::Cancelled(_)) => TrialShown::Cancelled,
            TrialState::Pending
            | TrialState::Running
            | TrialState::Ran { .. }
            | TrialState::Scoring { .. }
                if cancelled =>
            {
                TrialShown::Cancelled
            }
            TrialState::Ended(TrialEnd::Unscored(no_score)) => TrialShown::NoScore(no_score),
            TrialState::Ended(TrialEnd::Scored { score, .. }) => {
                if score.leak {
                    TrialShown::Leak
                } else if score.correctness == Verdict::Pass {
                    TrialShown::Pass
                } else {
                    TrialShown::Fail
                }
            }
            TrialState::Scoring { .. } => TrialShown::Scoring,
            TrialState::Ran { .. } => TrialShown::Waiting,
            TrialState::Running => TrialShown::Running,
            TrialState::Pending => TrialShown::Waiting,
        }
    }

    /// The name of its state, which gives no reason or detail
    /// (`docs/specs/components.md`, Status badge).
    pub fn name(&self) -> &'static str {
        match self {
            TrialShown::Cancelled => "cancelled",
            TrialShown::Waiting => "waiting",
            TrialShown::NoScore(_) => "no score",
            TrialShown::Pass => "pass",
            TrialShown::Fail => "fail",
            TrialShown::Leak => "leak",
            TrialShown::Scoring => "scoring",
            TrialShown::Running => "running",
        }
    }

    /// Its reason for no score, if it has one (`docs/specs/benchmarks.md`,
    /// Benchmark run, A trial's reason).
    pub fn reason(&self) -> Option<String> {
        match self {
            TrialShown::NoScore(no_score) => Some(no_score.reason()),
            TrialShown::Cancelled
            | TrialShown::Waiting
            | TrialShown::Pass
            | TrialShown::Fail
            | TrialShown::Leak
            | TrialShown::Scoring
            | TrialShown::Running => None,
        }
    }

    /// The status it shows as: a run not done or handed back shows as its
    /// outcome does (`docs/specs/runs.md`, Ending), and the other causes of
    /// no score as failed.
    pub fn status(&self) -> Status {
        match self {
            TrialShown::Cancelled => Status::Cancelled,
            TrialShown::Waiting => Status::Paused,
            TrialShown::NoScore(NoScore::Outcome(run)) => run.outcome.status(),
            TrialShown::NoScore(NoScore::BuildFailed { .. } | NoScore::Scorer { .. })
            | TrialShown::Fail
            | TrialShown::Leak => Status::Failed,
            TrialShown::Pass => Status::Done,
            TrialShown::Scoring | TrialShown::Running => Status::InFlight,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::benchmarks::{Cancelled, ScoreFigures, TrialFigures};

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

    fn shown_in(benchmark_run: BenchmarkRunState, state: &TrialState) -> (&'static str, Status) {
        let shown = TrialShown::of(benchmark_run, state);
        (shown.name(), shown.status())
    }

    fn shown(state: &TrialState) -> (&'static str, Status) {
        shown_in(BenchmarkRunState::Open, state)
    }

    const CANCELLED: BenchmarkRunState = BenchmarkRunState::Cancelled { at: None };

    /// `docs/specs/benchmarks.md`, Benchmark run, A trial's status: each row
    /// of the table, in its order, with its name and status.
    #[test_log::test]
    fn each_row_of_the_spec_s_table_gives_its_name_and_status() {
        let rows = [
            (
                "it is cancelled",
                CANCELLED,
                TrialState::Ended(TrialEnd::Cancelled(Cancelled::AfterRun(ended("done")))),
            ),
            (
                "a restart interrupted its run or its scorer, which has not started again",
                BenchmarkRunState::Open,
                TrialState::Ran { run: ended("done") },
            ),
            (
                "its build failed",
                BenchmarkRunState::Open,
                unscored(NoScore::BuildFailed {
                    time: Duration::from_secs(5),
                }),
            ),
            (
                "its run ended, but not done or handed back",
                BenchmarkRunState::Open,
                unscored(NoScore::Outcome(ended("failed"))),
            ),
            (
                "its scorer made no score",
                BenchmarkRunState::Open,
                unscored(NoScore::Scorer {
                    run: ended("done"),
                    why: "The scorer gave up: the task has no criteria".into(),
                    scorer_money: None,
                }),
            ),
            (
                "its score passes, with no leak",
                BenchmarkRunState::Open,
                scored(Verdict::Pass, false),
            ),
            (
                "its score fails, with no leak",
                BenchmarkRunState::Open,
                scored(Verdict::Fail, false),
            ),
            (
                "its score has a leak",
                BenchmarkRunState::Open,
                scored(Verdict::Fail, true),
            ),
            (
                "its scorer is running",
                BenchmarkRunState::Open,
                TrialState::Scoring { run: ended("done") },
            ),
            (
                "its run ended done or handed back",
                BenchmarkRunState::Open,
                TrialState::Ran {
                    run: ended("handed-back"),
                },
            ),
            (
                "its build or its run is running",
                BenchmarkRunState::Open,
                TrialState::Running,
            ),
            (
                "it has not started",
                BenchmarkRunState::Open,
                TrialState::Pending,
            ),
        ];

        let shown: Vec<_> = rows
            .iter()
            .map(|(row, benchmark_run, state)| (*row, shown_in(*benchmark_run, state)))
            .collect();

        assert_eq!(
            shown,
            [
                ("it is cancelled", ("cancelled", Status::Cancelled)),
                (
                    "a restart interrupted its run or its scorer, which has not started again",
                    ("waiting", Status::Paused)
                ),
                ("its build failed", ("no score", Status::Failed)),
                (
                    "its run ended, but not done or handed back",
                    ("no score", Status::Failed)
                ),
                ("its scorer made no score", ("no score", Status::Failed)),
                ("its score passes, with no leak", ("pass", Status::Done)),
                ("its score fails, with no leak", ("fail", Status::Failed)),
                ("its score has a leak", ("leak", Status::Failed)),
                ("its scorer is running", ("scoring", Status::InFlight)),
                (
                    "its run ended done or handed back",
                    ("waiting", Status::Paused)
                ),
                (
                    "its build or its run is running",
                    ("running", Status::InFlight)
                ),
                ("it has not started", ("waiting", Status::Paused)),
            ]
        );
    }

    /// `docs/specs/benchmarks.md`, Benchmark run, A trial's status: a leak
    /// is the first row that fits a score that passes with one.
    #[test_log::test]
    fn a_scored_trial_with_a_leak_that_passes_shows_leak_failed() {
        assert_eq!(
            shown(&scored(Verdict::Pass, true)),
            ("leak", Status::Failed)
        );
    }

    /// `docs/specs/benchmarks.md`, Benchmark runs, Cancelled trials: at once,
    /// also while the cancel stops its run or its scorer; a trial with a
    /// score or a reason keeps them.
    #[test_log::test]
    fn every_trial_of_a_cancelled_benchmark_run_with_no_score_and_no_reason_shows_cancelled() {
        let states = [
            TrialState::Pending,
            TrialState::Running,
            TrialState::Ran { run: ended("done") },
            TrialState::Scoring { run: ended("done") },
            TrialState::Ended(TrialEnd::Cancelled(Cancelled::BeforeRun)),
            scored(Verdict::Pass, false),
            unscored(NoScore::Outcome(ended("failed"))),
        ];

        let shown = states.map(|state| shown_in(CANCELLED, &state));

        assert_eq!(
            shown,
            [
                ("cancelled", Status::Cancelled),
                ("cancelled", Status::Cancelled),
                ("cancelled", Status::Cancelled),
                ("cancelled", Status::Cancelled),
                ("cancelled", Status::Cancelled),
                ("pass", Status::Done),
                ("no score", Status::Failed),
            ]
        );
    }

    /// `docs/specs/benchmarks.md`, Benchmark run, A trial's status: as its
    /// run's outcome does (`docs/specs/runs.md`, Ending).
    #[test_log::test]
    fn a_trial_whose_run_is_not_done_or_handed_back_shows_no_score_as_its_runs_outcome() {
        let outcomes = ["failed", "cancelled", "usage-limit", "interrupted"];

        let shown = outcomes.map(|outcome| shown(&unscored(NoScore::Outcome(ended(outcome)))));

        assert_eq!(
            shown,
            [
                ("no score", Status::Failed),
                ("no score", Status::Cancelled),
                ("no score", Status::Cancelled),
                ("no score", Status::Cancelled),
            ]
        );
    }

    /// `docs/specs/benchmarks.md`, Benchmark run, A trial's reason.
    #[test_log::test]
    fn a_trial_without_a_score_gives_the_spec_s_reason() {
        let reasons = [
            NoScore::BuildFailed {
                time: Duration::from_secs(5),
            },
            NoScore::Outcome(ended("failed")),
            NoScore::Outcome(ended("usage-limit")),
            NoScore::Scorer {
                run: ended("done"),
                why: "The scorer gave up: the task has no criteria".into(),
                scorer_money: None,
            },
        ]
        .map(|no_score| no_score.reason());

        assert_eq!(
            reasons,
            [
                "No score. The build of its environment failed.",
                "No score. The run's outcome: failed",
                "No score. The run's outcome: cancelled",
                "No score. The scorer gave up: the task has no criteria",
            ]
        );
    }
}
