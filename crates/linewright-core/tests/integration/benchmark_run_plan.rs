//! What `serve` starts next of the benchmark runs, through the functional core
//! (`docs/specs/benchmarks.md`, Benchmark runs: Order, Trials at once, and the
//! state table).

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::time::Duration;

use linewright_core::benchmark_runs::plan::{Gate, Step, plan};
use linewright_core::benchmarks::{
    Benchmark, BenchmarkRunId, BenchmarkRunRecord, BenchmarkRunState, RunTask, ScoreFigures, Shown,
    TrialEnd, TrialFigures, TrialKey, TrialRef, TrialState, WaitingFor,
};
use linewright_core::holds::{BenchmarkRunHold, InForce};
use linewright_core::prompts::{Readers, Reads};
use linewright_core::reconcile::Memory;
use linewright_core::repo_files::{FileError, Unusable};
use linewright_core::results::Verdict;
use linewright_core::runs::{Ending, OutcomeName};
use linewright_core::task::TaskStage;
use linewright_core::testing::{at, draining_since, limited_until};

fn id(n: u32) -> BenchmarkRunId {
    n.to_string().parse().unwrap()
}

fn key(repetition: u32, task: usize, configuration: usize) -> TrialKey {
    TrialKey {
        repetition: NonZeroU32::new(repetition).unwrap(),
        task,
        configuration,
    }
}

fn trial(benchmark_run: u32, (repetition, task, configuration): (u32, usize, usize)) -> TrialRef {
    TrialRef {
        benchmark_run: id(benchmark_run),
        key: key(repetition, task, configuration),
    }
}

fn trial_step(r: (u32, usize, usize)) -> Step {
    Step::Trial(trial(1, r))
}

fn done() -> TrialFigures {
    TrialFigures {
        outcome: OutcomeName::Ended(Ending::Done),
        money: None,
        time: Duration::from_secs(60),
    }
}

fn scored() -> TrialState {
    TrialState::Ended(TrialEnd::Scored {
        run: done(),
        score: ScoreFigures {
            correctness: Verdict::Pass,
            leak: false,
            scorer_money: None,
        },
    })
}

/// The open design benchmark run `n` of `tasks` tasks under `configurations`
/// configurations, `repetitions` times, each trial pending but those in
/// `states`.
fn benchmark_run(
    n: u32,
    (repetitions, tasks, configurations): (u32, usize, usize),
    states: &[((u32, usize, usize), TrialState)],
) -> BenchmarkRunRecord {
    let mut trials = BTreeMap::new();
    for repetition in 1..=repetitions {
        for task in 0..tasks {
            for configuration in 0..configurations {
                trials.insert(key(repetition, task, configuration), TrialState::Pending);
            }
        }
    }
    for ((repetition, task, configuration), state) in states {
        trials.insert(key(*repetition, *task, *configuration), state.clone());
    }
    BenchmarkRunRecord {
        id: id(n),
        benchmark: Benchmark {
            id: "1".parse().unwrap(),
            name: "board".parse().unwrap(),
            stage: TaskStage::Design,
        },
        launched_at: at("2026-11-17T12:00:00Z"),
        repetitions: NonZeroU32::new(repetitions).unwrap(),
        state: BenchmarkRunState::Open,
        tasks: (0..tasks)
            .map(|task| RunTask {
                id: (task + 1).to_string().parse().unwrap(),
                name: format!("case-{task}").parse().unwrap(),
            })
            .collect(),
        configurations: (0..configurations)
            .map(|configuration| format!("config-{configuration}").parse().unwrap())
            .collect(),
        trials,
    }
}

#[test_log::test]
fn trials_start_by_repetition_then_task_then_configuration() {
    let open_benchmark_runs = [benchmark_run(1, (2, 2, 2), &[])];

    let planned = plan(&open_benchmark_runs, 0, 8, Gate::Open);

    assert_eq!(
        planned.starts,
        [
            trial_step((1, 0, 0)),
            trial_step((1, 0, 1)),
            trial_step((1, 1, 0)),
            trial_step((1, 1, 1)),
            trial_step((2, 0, 0)),
            trial_step((2, 0, 1)),
            trial_step((2, 1, 0)),
            trial_step((2, 1, 1)),
        ]
    );
}

#[test_log::test]
fn a_benchmark_run_stopped_early_has_trials_of_the_same_tasks_under_every_configuration() {
    let ended = |r| (r, scored());
    let open_benchmark_runs = [benchmark_run(
        1,
        (2, 2, 3),
        &[
            ended((1, 0, 0)),
            ended((1, 0, 1)),
            ended((1, 0, 2)),
            ended((1, 1, 0)),
        ],
    )];

    let planned = plan(&open_benchmark_runs, 0, 2, Gate::Open);

    assert_eq!(
        planned.starts,
        [trial_step((1, 1, 1)), trial_step((1, 1, 2))]
    );
}

#[test_log::test]
fn a_pending_scorer_starts_before_the_next_trial() {
    let open_benchmark_runs = [benchmark_run(
        1,
        (1, 2, 1),
        &[((1, 0, 0), TrialState::Ran { run: done() })],
    )];

    let planned = plan(&open_benchmark_runs, 0, 1, Gate::Open);

    assert_eq!(planned.starts, [Step::Scorer(trial(1, (1, 0, 0)))]);
}

#[test_log::test]
fn no_step_starts_past_max_trials() {
    let open_benchmark_runs = [benchmark_run(
        1,
        (1, 3, 1),
        &[((1, 0, 0), TrialState::Running)],
    )];

    assert_eq!(
        plan(&open_benchmark_runs, 1, 2, Gate::Open).starts,
        [trial_step((1, 1, 0))]
    );
    assert_eq!(plan(&open_benchmark_runs, 2, 2, Gate::Open).starts, []);
}

#[test_log::test]
fn benchmark_runs_start_in_the_order_they_were_launched() {
    let open_benchmark_runs = [
        benchmark_run(1, (1, 1, 1), &[]),
        benchmark_run(2, (1, 1, 1), &[]),
    ];

    let planned = plan(&open_benchmark_runs, 0, 1, Gate::Open);

    assert_eq!(planned.starts, [trial_step((1, 0, 0))]);
    assert_eq!(
        planned.shown,
        BTreeMap::from([
            (id(1), Shown::Running),
            (
                id(2),
                Shown::Waiting(WaitingFor::FreePlace {
                    in_flight: 1,
                    max_trials: 1
                })
            ),
        ])
    );
}

#[test_log::test]
fn a_hold_holds_each_trial_and_the_benchmark_run_waits_for_it() {
    let usage_limit = BenchmarkRunHold::UsageLimit {
        resets_at: at("2026-11-17T14:30:00Z"),
    };
    let open_benchmark_runs = [benchmark_run(1, (1, 2, 1), &[])];

    let planned = plan(&open_benchmark_runs, 0, 4, Gate::Held(usage_limit));

    assert_eq!(planned.starts, []);
    assert_eq!(
        planned.shown,
        BTreeMap::from([(id(1), Shown::Waiting(WaitingFor::Hold(usage_limit)))])
    );
}

#[test_log::test]
fn nothing_starts_while_a_run_of_the_pipeline_waits_for_room() {
    let open_benchmark_runs = [benchmark_run(
        1,
        (1, 2, 1),
        &[((1, 0, 0), TrialState::Ran { run: done() })],
    )];

    let planned = plan(&open_benchmark_runs, 0, 4, Gate::PipelineWaitsForRoom);

    assert_eq!(planned.starts, []);
    assert_eq!(
        planned.shown,
        BTreeMap::from([(id(1), Shown::Waiting(WaitingFor::Pipeline))])
    );
}

#[test_log::test]
fn a_benchmark_run_with_a_step_in_flight_is_running_whatever_holds_its_next() {
    let open_benchmark_runs = [benchmark_run(
        1,
        (1, 2, 1),
        &[((1, 0, 0), TrialState::Scoring { run: done() })],
    )];

    let planned = plan(
        &open_benchmark_runs,
        1,
        4,
        Gate::Held(BenchmarkRunHold::Drain),
    );

    assert_eq!(planned.starts, []);
    assert_eq!(planned.shown, BTreeMap::from([(id(1), Shown::Running)]));
}

#[test_log::test]
fn an_open_benchmark_run_whose_trials_have_all_ended_is_ended() {
    let open_benchmark_runs = [
        benchmark_run(
            1,
            (1, 2, 1),
            &[
                ((1, 0, 0), scored()),
                (
                    (1, 1, 0),
                    TrialState::Ended(TrialEnd::Unscored(
                        linewright_core::benchmarks::NoScore::Outcome(TrialFigures {
                            outcome: OutcomeName::Ended(Ending::Failed),
                            ..done()
                        }),
                    )),
                ),
            ],
        ),
        benchmark_run(2, (1, 1, 1), &[]),
    ];

    let planned = plan(&open_benchmark_runs, 0, 1, Gate::Open);

    assert_eq!(planned.starts, [Step::Trial(trial(2, (1, 0, 0)))]);
    assert_eq!(
        planned.shown,
        BTreeMap::from([(id(1), Shown::Ended), (id(2), Shown::Running)])
    );
}

/// The files of the repository Linewright cannot use: `prompts/scorer.md`,
/// which the scorer alone reads, and `config.toml`, which every run reads.
fn unusable_files() -> Vec<Unusable> {
    vec![
        Unusable {
            file: FileError {
                path: ".linewright/briefs/prompts/scorer.md".into(),
                error: "it lacks the placeholders `{{criteria}}`".into(),
            },
            holds: Readers::Only(&[Reads::Scorer]),
        },
        Unusable {
            file: FileError {
                path: ".linewright/config.toml".into(),
                error: "invalid".into(),
            },
            holds: Readers::Every,
        },
    ]
}

#[test_log::test]
fn neither_the_environment_nor_a_file_holds_a_benchmark_run() {
    let memory = Memory {
        awaits_environment: true,
        ..Memory::default()
    };
    let unusable = unusable_files();
    let in_force = InForce::of(&memory, at("2026-11-17T12:00:00Z"), &unusable);

    assert_eq!(Gate::at(&in_force, false), Gate::Open);
}

#[test_log::test]
fn a_drain_holds_a_benchmark_run_before_the_usage_limit() {
    let memory = Memory {
        usage_limit: limited_until("2026-11-17T14:30:00Z").usage_limit,
        ..draining_since("2026-11-17T11:00:00Z")
    };
    let in_force = InForce::of(&memory, at("2026-11-17T12:00:00Z"), &[]);

    assert_eq!(
        Gate::at(&in_force, true),
        Gate::Held(BenchmarkRunHold::Drain)
    );
}

#[test_log::test]
fn a_hold_shows_before_the_pipelines_wait_for_room() {
    let memory = limited_until("2026-11-17T14:30:00Z");
    let in_force = InForce::of(&memory, at("2026-11-17T12:00:00Z"), &[]);

    assert_eq!(
        Gate::at(&in_force, true),
        Gate::Held(BenchmarkRunHold::UsageLimit {
            resets_at: at("2026-11-17T14:30:00Z")
        })
    );
}

#[test_log::test]
fn a_step_waits_while_a_run_of_the_pipeline_waits_for_room() {
    let in_force = InForce::of(&Memory::default(), at("2026-11-17T12:00:00Z"), &[]);

    assert_eq!(Gate::at(&in_force, true), Gate::PipelineWaitsForRoom);
    assert_eq!(Gate::at(&in_force, false), Gate::Open);
}
