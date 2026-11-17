//! What `serve` starts next of the experiments, through the functional core
//! (`docs/specs/benchmarks.md`, Experiments: Order, Replays at once, and the
//! state table).

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::time::Duration;

use linewright::benchmarks::{
    ExperimentId, ExperimentRecord, ExperimentState, JudgementFigures, ReplayFigures, Shown, Suite,
    TrialEnd, TrialKey, TrialRef, TrialState, WaitingFor,
};
use linewright::case::ReplayStage;
use linewright::experiments::plan::{Gate, Step, StepKind, plan};
use linewright::holds::{Hold, InForce};
use linewright::prompts::{Readers, Reads};
use linewright::reconcile::Memory;
use linewright::repo_files::{FileError, Unusable};
use linewright::results::Verdict;
use linewright::runs::{Ending, OutcomeName};

use crate::common::{at, limited_until};

fn id(n: u32) -> ExperimentId {
    n.to_string().parse().unwrap()
}

fn key(repetition: u32, case: usize, configuration: usize) -> TrialKey {
    TrialKey {
        repetition: NonZeroU32::new(repetition).unwrap(),
        case,
        configuration,
    }
}

fn trial(experiment: u32, (repetition, case, configuration): (u32, usize, usize)) -> TrialRef {
    TrialRef {
        experiment: id(experiment),
        key: key(repetition, case, configuration),
    }
}

fn replay(r: (u32, usize, usize)) -> Step {
    Step::Replay(trial(1, r))
}

fn done() -> ReplayFigures {
    ReplayFigures {
        outcome: OutcomeName::Ended(Ending::Done),
        money: None,
        time: Duration::from_secs(60),
    }
}

fn judged() -> TrialState {
    TrialState::Ended(TrialEnd::Judged {
        replay: done(),
        judgement: JudgementFigures {
            correctness: Verdict::Pass,
            leak: false,
            judge_money: None,
        },
    })
}

/// The open design experiment `n` of `cases` cases under `configurations`
/// configurations, `repetitions` times, each trial pending but those in
/// `states`.
fn experiment(
    n: u32,
    (repetitions, cases, configurations): (u32, usize, usize),
    states: &[((u32, usize, usize), TrialState)],
) -> ExperimentRecord {
    let mut trials = BTreeMap::new();
    for repetition in 1..=repetitions {
        for case in 0..cases {
            for configuration in 0..configurations {
                trials.insert(key(repetition, case, configuration), TrialState::Pending);
            }
        }
    }
    for ((repetition, case, configuration), state) in states {
        trials.insert(key(*repetition, *case, *configuration), state.clone());
    }
    ExperimentRecord {
        id: id(n),
        suite: Suite {
            name: "board".parse().unwrap(),
            stage: ReplayStage::Design,
        },
        launched_at: at("2026-11-17T12:00:00Z"),
        repetitions: NonZeroU32::new(repetitions).unwrap(),
        state: ExperimentState::Open,
        cases: (0..cases)
            .map(|case| format!("case-{case}").parse().unwrap())
            .collect(),
        configurations: (0..configurations)
            .map(|configuration| format!("config-{configuration}").parse().unwrap())
            .collect(),
        trials,
    }
}

fn open(_: StepKind) -> Gate {
    Gate::Open
}

#[test_log::test]
fn replays_start_by_repetition_then_case_then_configuration() {
    let open_experiments = [experiment(1, (2, 2, 2), &[])];

    let planned = plan(&open_experiments, 0, 8, open);

    assert_eq!(
        planned.starts,
        [
            replay((1, 0, 0)),
            replay((1, 0, 1)),
            replay((1, 1, 0)),
            replay((1, 1, 1)),
            replay((2, 0, 0)),
            replay((2, 0, 1)),
            replay((2, 1, 0)),
            replay((2, 1, 1)),
        ]
    );
}

#[test_log::test]
fn an_experiment_stopped_early_has_replays_of_the_same_cases_under_every_configuration() {
    let ended = |r| (r, judged());
    let open_experiments = [experiment(
        1,
        (2, 2, 3),
        &[
            ended((1, 0, 0)),
            ended((1, 0, 1)),
            ended((1, 0, 2)),
            ended((1, 1, 0)),
        ],
    )];

    let planned = plan(&open_experiments, 0, 2, open);

    assert_eq!(planned.starts, [replay((1, 1, 1)), replay((1, 1, 2))]);
}

#[test_log::test]
fn a_pending_judge_starts_before_the_next_replay() {
    let open_experiments = [experiment(
        1,
        (1, 2, 1),
        &[((1, 0, 0), TrialState::Replayed { replay: done() })],
    )];

    let planned = plan(&open_experiments, 0, 1, open);

    assert_eq!(planned.starts, [Step::Judge(trial(1, (1, 0, 0)))]);
}

#[test_log::test]
fn no_step_starts_past_max_replays() {
    let open_experiments = [experiment(
        1,
        (1, 3, 1),
        &[((1, 0, 0), TrialState::Replaying)],
    )];

    assert_eq!(
        plan(&open_experiments, 1, 2, open).starts,
        [replay((1, 1, 0))]
    );
    assert_eq!(plan(&open_experiments, 2, 2, open).starts, []);
}

#[test_log::test]
fn experiments_start_in_the_order_they_were_launched() {
    let open_experiments = [experiment(1, (1, 1, 1), &[]), experiment(2, (1, 1, 1), &[])];

    let planned = plan(&open_experiments, 0, 1, open);

    assert_eq!(planned.starts, [replay((1, 0, 0))]);
    assert_eq!(
        planned.shown,
        BTreeMap::from([
            (id(1), Shown::Running),
            (
                id(2),
                Shown::Waiting(WaitingFor::FreeReplay {
                    in_flight: 1,
                    max_replays: 1
                })
            ),
        ])
    );
}

#[test_log::test]
fn a_hold_holds_each_replay_and_the_experiment_waits_for_it() {
    let usage_limit = Hold::UsageLimit {
        resets_at: at("2026-11-17T14:30:00Z"),
    };
    let open_experiments = [experiment(1, (1, 2, 1), &[])];

    let planned = plan(&open_experiments, 0, 4, |_| Gate::Held(usage_limit.clone()));

    assert_eq!(planned.starts, []);
    assert_eq!(
        planned.shown,
        BTreeMap::from([(id(1), Shown::Waiting(WaitingFor::Hold(usage_limit)))])
    );
}

#[test_log::test]
fn a_hold_over_the_judge_alone_holds_the_judges_and_not_the_replays() {
    let open_experiments = [experiment(
        1,
        (1, 2, 1),
        &[((1, 0, 0), TrialState::Replayed { replay: done() })],
    )];

    let planned = plan(&open_experiments, 0, 4, |kind| match kind {
        StepKind::Judge => Gate::Held(Hold::Drain),
        StepKind::Replay(_) => Gate::Open,
    });

    assert_eq!(planned.starts, [replay((1, 1, 0))]);
    assert_eq!(planned.shown, BTreeMap::from([(id(1), Shown::Running)]));
}

#[test_log::test]
fn the_gate_is_asked_of_the_experiments_stage() {
    let open_experiments = [experiment(1, (1, 1, 1), &[])];

    let planned = plan(&open_experiments, 0, 4, |kind| match kind {
        StepKind::Replay(ReplayStage::Design) => Gate::Held(Hold::Environment),
        StepKind::Replay(_) | StepKind::Judge => Gate::Open,
    });

    assert_eq!(planned.starts, []);
}

#[test_log::test]
fn nothing_starts_while_a_run_of_the_pipeline_waits_for_room() {
    let open_experiments = [experiment(
        1,
        (1, 2, 1),
        &[((1, 0, 0), TrialState::Replayed { replay: done() })],
    )];

    let planned = plan(&open_experiments, 0, 4, |_| Gate::PipelineWaitsForRoom);

    assert_eq!(planned.starts, []);
    assert_eq!(
        planned.shown,
        BTreeMap::from([(id(1), Shown::Waiting(WaitingFor::Pipeline))])
    );
}

#[test_log::test]
fn an_experiment_with_a_step_in_flight_is_running_whatever_holds_its_next() {
    let open_experiments = [experiment(
        1,
        (1, 2, 1),
        &[((1, 0, 0), TrialState::Judging { replay: done() })],
    )];

    let planned = plan(&open_experiments, 1, 4, |_| Gate::Held(Hold::Drain));

    assert_eq!(planned.starts, []);
    assert_eq!(planned.shown, BTreeMap::from([(id(1), Shown::Running)]));
}

#[test_log::test]
fn an_open_experiment_whose_trials_have_all_ended_is_ended() {
    let open_experiments = [
        experiment(
            1,
            (1, 2, 1),
            &[
                ((1, 0, 0), judged()),
                (
                    (1, 1, 0),
                    TrialState::Ended(TrialEnd::Unjudged {
                        replay: ReplayFigures {
                            outcome: OutcomeName::Ended(Ending::Failed),
                            ..done()
                        },
                        why: linewright::benchmarks::NoJudgement::Outcome,
                    }),
                ),
            ],
        ),
        experiment(2, (1, 1, 1), &[]),
    ];

    let planned = plan(&open_experiments, 0, 1, open);

    assert_eq!(planned.starts, [Step::Replay(trial(2, (1, 0, 0)))]);
    assert_eq!(
        planned.shown,
        BTreeMap::from([(id(1), Shown::Ended), (id(2), Shown::Running)])
    );
}

/// The file `prompts/judge.md` of the repository, which the judge alone
/// reads, unusable.
fn judge_prompt_unusable() -> Unusable {
    Unusable {
        file: FileError {
            path: ".linewright/briefs/prompts/judge.md".into(),
            error: "it lacks the placeholders `{{criteria}}`".into(),
        },
        holds: Readers::Only(&[Reads::Judge]),
    }
}

#[test_log::test]
fn a_step_is_held_as_a_run_of_its_kind_is() {
    let unusable = [judge_prompt_unusable()];
    let in_force = InForce::of(&Memory::default(), at("2026-11-17T12:00:00Z"), &unusable);

    assert_eq!(
        Gate::at(&in_force, false, StepKind::Judge),
        Gate::Held(Hold::File(judge_prompt_unusable().file))
    );
    assert_eq!(
        Gate::at(&in_force, false, StepKind::Replay(ReplayStage::Design)),
        Gate::Open
    );
}

#[test_log::test]
fn a_hold_shows_before_the_pipelines_wait_for_room() {
    let memory = limited_until("2026-11-17T14:30:00Z");
    let in_force = InForce::of(&memory, at("2026-11-17T12:00:00Z"), &[]);

    assert_eq!(
        Gate::at(&in_force, true, StepKind::Replay(ReplayStage::Triage)),
        Gate::Held(Hold::UsageLimit {
            resets_at: at("2026-11-17T14:30:00Z")
        })
    );
}

#[test_log::test]
fn a_step_waits_while_a_run_of_the_pipeline_waits_for_room() {
    let in_force = InForce::of(&Memory::default(), at("2026-11-17T12:00:00Z"), &[]);

    assert_eq!(
        Gate::at(&in_force, true, StepKind::Judge),
        Gate::PipelineWaitsForRoom
    );
    assert_eq!(Gate::at(&in_force, false, StepKind::Judge), Gate::Open);
}
