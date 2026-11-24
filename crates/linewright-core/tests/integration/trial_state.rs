//! The state a trial moves to on a change (`docs/specs/benchmarks.md`,
//! Trials, "A failed build"; Scoring, "Which trials"; Benchmark runs, "A
//! trial without a score" and "Cancelled trials"), decided by
//! `TrialState::changed`.

use std::time::Duration;

use linewright_core::benchmarks::{
    Cancelled, NoScore, ScoreFigures, TrialChange, TrialEnd, TrialFigures, TrialState,
};
use linewright_core::cost::Usd;
use linewright_core::results::Verdict;
use linewright_core::runs::{Ending, OutcomeName, Unended};
use rstest::rstest;

/// The figures of a run that ended `ending`.
fn run(ending: Ending) -> TrialFigures {
    TrialFigures {
        outcome: OutcomeName::Ended(ending),
        money: Usd::new(0.25),
        time: Duration::from_millis(12_500),
    }
}

fn score() -> ScoreFigures {
    ScoreFigures {
        correctness: Verdict::Pass,
        leak: true,
        scorer_money: Usd::new(0.05),
    }
}

fn ran() -> TrialState {
    TrialState::Ran {
        run: run(Ending::Done),
    }
}

fn scoring() -> TrialState {
    TrialState::Scoring {
        run: run(Ending::Done),
    }
}

fn unscored(no_score: NoScore) -> TrialState {
    TrialState::Ended(TrialEnd::Unscored(no_score))
}

fn cancelled(cancelled: Cancelled) -> TrialState {
    TrialState::Ended(TrialEnd::Cancelled(cancelled))
}

fn build_fails() -> TrialChange {
    TrialChange::BuildFails {
        time: Duration::from_secs(5),
    }
}

fn scorer_makes_none() -> TrialChange {
    TrialChange::ScorerMakesNone {
        why: "The scorer gave up: The output is empty.".into(),
        scorer_money: Usd::new(0.5),
    }
}

#[rstest]
#[case::a_pending_trial_whose_run_starts_is_running(
    TrialState::Pending,
    TrialChange::RunStarts,
    TrialState::Running
)]
#[case::a_run_that_ends_done_awaits_its_scorer(
    TrialState::Running,
    TrialChange::RunEnds(run(Ending::Done)),
    TrialState::Ran { run: run(Ending::Done) }
)]
#[case::a_run_that_ends_handed_back_awaits_its_scorer(
    TrialState::Running,
    TrialChange::RunEnds(run(Ending::HandedBack)),
    TrialState::Ran { run: run(Ending::HandedBack) }
)]
#[case::a_run_that_fails_ends_its_trial_without_a_score(
    TrialState::Running,
    TrialChange::RunEnds(run(Ending::Failed)),
    unscored(NoScore::Outcome(run(Ending::Failed)))
)]
#[case::a_run_that_ends_for_the_usage_limit_ends_its_trial_without_a_score(
    TrialState::Running,
    TrialChange::RunEnds(run(Ending::UsageLimit)),
    unscored(NoScore::Outcome(run(Ending::UsageLimit)))
)]
#[case::a_failed_build_ends_its_trial_without_a_score_with_its_time(
    TrialState::Running,
    build_fails(),
    unscored(NoScore::BuildFailed { time: Duration::from_secs(5) })
)]
#[case::a_trial_whose_scorer_starts_is_scoring(ran(), TrialChange::ScorerStarts, scoring())]
#[case::a_score_ends_its_trial_with_its_run_and_its_score(
    scoring(),
    TrialChange::Scored(score()),
    TrialState::Ended(TrialEnd::Scored { run: run(Ending::Done), score: score() })
)]
#[case::a_scorer_that_makes_none_ends_its_trial_without_a_score_and_says_why(
    scoring(),
    scorer_makes_none(),
    unscored(NoScore::Scorer {
        run: run(Ending::Done),
        why: "The scorer gave up: The output is empty.".into(),
        scorer_money: Usd::new(0.5),
    })
)]
#[case::a_pending_trial_cancelled_is_cancelled_before_its_run(
    TrialState::Pending,
    TrialChange::Cancelled { stopped: None },
    cancelled(Cancelled::BeforeRun)
)]
#[case::a_running_trial_whose_build_failed_after_the_cancel_is_cancelled_before_its_run(
    TrialState::Running,
    TrialChange::CancelledBeforeRun,
    cancelled(Cancelled::BeforeRun)
)]
#[case::a_running_trial_cancelled_keeps_the_run_the_cancel_stopped(
    TrialState::Running,
    TrialChange::Cancelled { stopped: Some(run(Ending::Cancelled)) },
    cancelled(Cancelled::RunStopped(run(Ending::Cancelled)))
)]
#[case::a_running_trial_cancelled_as_its_run_ended_done_keeps_that_run(
    TrialState::Running,
    TrialChange::Cancelled { stopped: Some(run(Ending::Done)) },
    cancelled(Cancelled::AfterRun(run(Ending::Done)))
)]
#[case::a_running_trial_cancelled_with_no_figures_has_a_stopped_run_with_no_money_and_no_time(
    TrialState::Running,
    TrialChange::Cancelled { stopped: None },
    cancelled(Cancelled::RunStopped(TrialFigures {
        outcome: OutcomeName::Unended(Unended::Interrupted),
        money: None,
        time: Duration::ZERO,
    }))
)]
#[case::a_trial_awaiting_its_scorer_cancelled_keeps_its_run(
    ran(),
    TrialChange::Cancelled { stopped: None },
    cancelled(Cancelled::AfterRun(run(Ending::Done)))
)]
#[case::a_trial_scoring_cancelled_keeps_its_run(
    scoring(),
    TrialChange::Cancelled { stopped: None },
    cancelled(Cancelled::AfterRun(run(Ending::Done)))
)]
#[case::a_build_failing_once_cancelled_ends_its_trial_cancelled_before_its_run(
    TrialState::Running,
    TrialChange::on_cancel(Some(build_fails())),
    cancelled(Cancelled::BeforeRun)
)]
#[case::a_run_ending_once_cancelled_ends_its_trial_cancelled_with_that_run(
    TrialState::Running,
    TrialChange::on_cancel(Some(TrialChange::RunEnds(run(Ending::Cancelled)))),
    cancelled(Cancelled::RunStopped(run(Ending::Cancelled)))
)]
#[case::a_scorer_ending_once_cancelled_ends_its_trial_cancelled_after_its_run(
    scoring(),
    TrialChange::on_cancel(Some(scorer_makes_none())),
    cancelled(Cancelled::AfterRun(run(Ending::Done)))
)]
#[test_log::test]
fn a_trials_state_after_a_change(
    #[case] state: TrialState,
    #[case] change: TrialChange,
    #[case] expected: TrialState,
) {
    let changed = state.changed(change);

    assert_eq!(changed.map_err(|e| format!("{e:#}")), Ok(expected));
}

#[rstest]
#[case::a_scorer_starting_before_its_run(TrialState::Pending, TrialChange::ScorerStarts)]
#[case::a_run_ending_before_it_starts(TrialState::Pending, TrialChange::RunEnds(run(Ending::Done)))]
#[case::a_run_starting_twice(TrialState::Running, TrialChange::RunStarts)]
#[case::a_score_before_its_scorer_starts(ran(), TrialChange::Scored(score()))]
#[case::a_build_failing_after_its_run(scoring(), build_fails())]
#[case::a_cancel_stopping_a_run_that_had_ended(
    ran(),
    TrialChange::Cancelled { stopped: Some(run(Ending::Cancelled)) }
)]
#[case::a_cancel_with_a_stopped_run_before_the_run_starts(
    TrialState::Pending,
    TrialChange::Cancelled { stopped: Some(run(Ending::Cancelled)) }
)]
#[case::a_cancel_before_the_run_once_it_has_ended(ran(), TrialChange::CancelledBeforeRun)]
#[case::a_cancel_of_a_trial_with_a_score(
    TrialState::Ended(TrialEnd::Scored { run: run(Ending::Done), score: score() }),
    TrialChange::Cancelled { stopped: None }
)]
#[case::a_cancel_of_a_trial_without_a_score(
    unscored(NoScore::Outcome(run(Ending::Failed))),
    TrialChange::Cancelled { stopped: None }
)]
#[case::a_cancel_of_a_cancelled_trial(
    cancelled(Cancelled::BeforeRun),
    TrialChange::Cancelled { stopped: None }
)]
#[test_log::test]
fn a_change_that_is_not_of_the_trials_state_is_refused(
    #[case] state: TrialState,
    #[case] change: TrialChange,
) {
    let changed = state.changed(change);

    assert!(changed.is_err(), "{changed:?}");
}
