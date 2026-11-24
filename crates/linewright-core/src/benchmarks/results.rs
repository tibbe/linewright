//! A benchmark run's results (`docs/specs/benchmarks.md`, Results): figures of
//! the trials that have ended, worked out on each read.

use std::num::NonZeroUsize;
use std::time::Duration;

use schemars::JsonSchema;
use serde::Serialize;

use crate::cost::Usd;
use crate::name::Name;
use crate::results::Verdict;

use super::{
    BenchmarkRunRecord, Cancelled, ConfigurationId, NoScore, TaskId, TrialEnd, TrialFigures,
    TrialState,
};

/// A benchmark run's results.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Results {
    /// Each configuration's figures, in the benchmark run's order.
    pub configurations: Vec<ConfigurationResults>,
    /// The cost of the scorers, in US dollars, apart from the
    /// configurations'.
    pub scorers: Total,
    /// How many of its trials have ended: those whose run has, whether or
    /// not their scorer has, a run the cancel stopped among them.
    pub trials_ended: usize,
    /// How many trials it makes.
    pub all: usize,
    /// The cost of all its trials and its scorers, in US dollars, and how
    /// many of its trials that have ended have none.
    pub cost: Total,
}

/// A configuration's figures.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ConfigurationResults {
    pub id: ConfigurationId,
    pub name: Name,
    /// Of its trials with a score and no leak, the share that pass;
    /// `None` with no such trial.
    pub pass_rate: Option<f64>,
    #[serde(flatten)]
    pub figures: Figures,
    /// For each task, in the benchmark run's order.
    pub tasks: Vec<TaskResults>,
}

/// The figures of a configuration's trials, or of its trials of a task.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Figures {
    /// In US dollars, of the trials that have a cost.
    pub cost: Mean,
    /// In seconds.
    pub time: Mean,
    /// How many of the trials have a score with a leak.
    pub leaks: usize,
    /// How many of the trials have no score for a reason the spec
    /// gives: their run's outcome or their scorer.
    pub unscored: usize,
}

/// A configuration's figures of its trials of a task.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct TaskResults {
    pub id: TaskId,
    pub name: Name,
    /// `None` with no trial with a score and no leak.
    pub passes: Option<Passes>,
    #[serde(flatten)]
    pub figures: Figures,
}

/// How many trials pass, of those with a score and no leak.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Passes {
    pub pass: usize,
    pub of: NonZeroUsize,
}

impl std::fmt::Display for Passes {
    /// As "2 of 3" (`docs/specs/benchmarks.md`, Benchmark run, The table).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} of {}", self.pass, self.of)
    }
}

/// A figure's mean and total over the trials that have it, and how many
/// have none.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Mean {
    /// `None` when no trial has the figure.
    pub mean: Option<f64>,
    pub total: f64,
    pub none: usize,
}

/// A figure's total over what has it, and how many have none.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Total {
    pub total: f64,
    pub none: usize,
}

/// The results of `record`, of the trials that have ended.
pub fn results(record: &BenchmarkRunRecord) -> Results {
    let mut scorers = Sum::default();
    let mut stopped = Sum::default();
    let mut tallies: Vec<Vec<Tally>> = record
        .configurations
        .iter()
        .map(|_| record.tasks.iter().map(|_| Tally::default()).collect())
        .collect();
    for (key, state) in &record.trials {
        let Some(tally) = tallies
            .get_mut(key.configuration)
            .and_then(|tasks| tasks.get_mut(key.task))
        else {
            continue;
        };
        match state {
            TrialState::Pending
            | TrialState::Running
            | TrialState::Ended(TrialEnd::Cancelled(Cancelled::BeforeRun)) => {}
            TrialState::Ran { run }
            | TrialState::Scoring { run }
            | TrialState::Ended(TrialEnd::Cancelled(Cancelled::AfterRun(run))) => {
                tally.trial(run);
            }
            TrialState::Ended(TrialEnd::Cancelled(Cancelled::RunStopped(run))) => {
                stopped.add(run.money.map(f64::from));
            }
            TrialState::Ended(TrialEnd::Scored { run, score }) => {
                tally.trial(run);
                scorers.add(score.scorer_money.map(f64::from));
                if score.leak {
                    tally.leaks += 1;
                } else {
                    tally.of += 1;
                    if score.correctness == Verdict::Pass {
                        tally.pass += 1;
                    }
                }
            }
            TrialState::Ended(TrialEnd::Unscored(NoScore::BuildFailed { time })) => {
                tally.ended(None, *time);
                tally.unscored += 1;
            }
            TrialState::Ended(TrialEnd::Unscored(NoScore::Outcome(run))) => {
                tally.trial(run);
                tally.unscored += 1;
            }
            TrialState::Ended(TrialEnd::Unscored(NoScore::Scorer {
                run, scorer_money, ..
            })) => {
                tally.trial(run);
                tally.unscored += 1;
                scorers.add(scorer_money.map(f64::from));
            }
        }
    }
    let trials_ended = tallies
        .iter()
        .flatten()
        .map(|tally| tally.ended)
        .sum::<usize>()
        + stopped.count();
    let configurations: Vec<ConfigurationResults> = record
        .configurations
        .iter()
        .zip(tallies)
        .enumerate()
        .map(|(place, (name, tasks))| {
            let all = tasks.iter().fold(Tally::default(), Tally::and);
            ConfigurationResults {
                id: ConfigurationId::of_place(place),
                name: name.clone(),
                pass_rate: share(all.pass, all.of),
                figures: all.figures(),
                tasks: record
                    .tasks
                    .iter()
                    .zip(tasks)
                    .map(|(task, tally)| TaskResults {
                        id: task.id,
                        name: task.name.clone(),
                        passes: NonZeroUsize::new(tally.of).map(|of| Passes {
                            pass: tally.pass,
                            of,
                        }),
                        figures: tally.figures(),
                    })
                    .collect(),
            }
        })
        .collect();
    let (trials_cost, without_cost) =
        configurations
            .iter()
            .fold((0.0, 0), |(total, none), configuration| {
                let cost = &configuration.figures.cost;
                (total + cost.total, none + cost.none)
            });
    Results {
        configurations,
        cost: Total {
            total: trials_cost + stopped.total + scorers.total,
            none: without_cost + stopped.none,
        },
        scorers: Total {
            total: scorers.total,
            none: scorers.none,
        },
        trials_ended,
        all: record.trials.len(),
    }
}

/// The figures of a configuration's trials of a task as they are counted.
#[derive(Default)]
struct Tally {
    /// How many of the trials have ended.
    ended: usize,
    pass: usize,
    of: usize,
    cost: Sum,
    time: Sum,
    leaks: usize,
    unscored: usize,
}

impl Tally {
    /// Counts a trial that ended with its run's figures.
    fn trial(&mut self, trial: &TrialFigures) {
        self.ended(trial.money, trial.time);
    }

    /// Counts a trial that ended, with its cost, if it has one, and its time.
    fn ended(&mut self, money: Option<Usd>, time: Duration) {
        self.ended += 1;
        self.cost.add(money.map(f64::from));
        self.time.add(Some(time.as_secs_f64()));
    }

    /// The tally of the trials of both.
    fn and(self, other: &Tally) -> Tally {
        Tally {
            ended: self.ended + other.ended,
            pass: self.pass + other.pass,
            of: self.of + other.of,
            cost: self.cost.and(&other.cost),
            time: self.time.and(&other.time),
            leaks: self.leaks + other.leaks,
            unscored: self.unscored + other.unscored,
        }
    }

    fn figures(&self) -> Figures {
        Figures {
            cost: self.cost.mean(),
            time: self.time.mean(),
            leaks: self.leaks,
            unscored: self.unscored,
        }
    }
}

/// A figure's sum as the trials are counted.
#[derive(Default)]
struct Sum {
    total: f64,
    counted: usize,
    none: usize,
}

impl Sum {
    fn add(&mut self, figure: Option<f64>) {
        match figure {
            Some(figure) => {
                self.total += figure;
                self.counted += 1;
            }
            None => self.none += 1,
        }
    }

    /// How many figures it summed and how many were absent.
    fn count(&self) -> usize {
        self.counted + self.none
    }

    fn and(self, other: &Sum) -> Sum {
        Sum {
            total: self.total + other.total,
            counted: self.counted + other.counted,
            none: self.none + other.none,
        }
    }

    fn mean(&self) -> Mean {
        Mean {
            mean: share_of(self.total, self.counted),
            total: self.total,
            none: self.none,
        }
    }
}

/// `part` of `whole`; `None` of nothing.
fn share(part: usize, whole: usize) -> Option<f64> {
    share_of(count(part), whole)
}

fn share_of(part: f64, whole: usize) -> Option<f64> {
    (whole > 0).then(|| part / count(whole))
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a benchmark run counts far fewer trials than an f64 holds exactly"
)]
fn count(n: usize) -> f64 {
    n as f64
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use super::*;
    use crate::benchmarks::{
        Benchmark, BenchmarkRunId, BenchmarkRunState, RunTask, ScoreFigures, TrialFigures, TrialKey,
    };
    use crate::cost::Usd;
    use crate::runs::{Ending, OutcomeName};
    use crate::task::TaskStage;

    /// A benchmark run on the tasks `tasks` under `as-is` and `configurations`,
    /// with `trials` and every other trial pending.
    fn benchmark_run(
        tasks: &[&str],
        configurations: &[&str],
        repetitions: u32,
        trials: Vec<(TrialKey, TrialState)>,
    ) -> BenchmarkRunRecord {
        let mut all = BTreeMap::new();
        for repetition in 1..=repetitions {
            for task in 0..tasks.len() {
                for configuration in 0..=configurations.len() {
                    all.insert(key(repetition, task, configuration), TrialState::Pending);
                }
            }
        }
        all.extend(trials);
        BenchmarkRunRecord {
            id: "1".parse::<BenchmarkRunId>().unwrap(),
            benchmark: Benchmark {
                id: "1".parse().unwrap(),
                name: "board".parse().unwrap(),
                stage: TaskStage::Design,
            },
            launched_at: "2026-11-17T12:00:00Z".parse().unwrap(),
            repetitions: repetitions.try_into().unwrap(),
            state: BenchmarkRunState::Open,
            tasks: tasks
                .iter()
                .zip(1..)
                .map(|(task, id)| RunTask {
                    id: id.to_string().parse().unwrap(),
                    name: task.parse().unwrap(),
                })
                .collect(),
            configurations: std::iter::once("as-is")
                .chain(configurations.iter().copied())
                .map(|name| name.parse().unwrap())
                .collect(),
            trials: all,
        }
    }

    fn key(repetition: u32, task: usize, configuration: usize) -> TrialKey {
        TrialKey {
            repetition: repetition.try_into().unwrap(),
            task,
            configuration,
        }
    }

    fn trial(money: Option<f64>, seconds: u64) -> TrialFigures {
        TrialFigures {
            outcome: OutcomeName::Ended(Ending::Done),
            money: money.map(|money| Usd::new(money).unwrap()),
            time: Duration::from_secs(seconds),
        }
    }

    fn scored(
        run: TrialFigures,
        correctness: Verdict,
        leak: bool,
        scorer_money: Option<f64>,
    ) -> TrialState {
        TrialState::Ended(TrialEnd::Scored {
            run,
            score: ScoreFigures {
                correctness,
                leak,
                scorer_money: scorer_money.map(|money| Usd::new(money).unwrap()),
            },
        })
    }

    fn passes(pass: usize, of: usize) -> Passes {
        Passes {
            pass,
            of: NonZeroUsize::new(of).unwrap(),
        }
    }

    fn unscored(no_score: NoScore) -> TrialState {
        TrialState::Ended(TrialEnd::Unscored(no_score))
    }

    fn approx(figure: Option<f64>, expected: f64) {
        let figure = figure.expect("a figure");
        assert!(
            (figure - expected).abs() < 1e-9,
            "{figure} is not {expected}"
        );
    }

    #[test_log::test]
    fn a_trial_with_a_leak_counts_as_a_leak_and_not_in_the_pass_rate() {
        let record = benchmark_run(
            &["lanes"],
            &[],
            3,
            vec![
                (
                    key(1, 0, 0),
                    scored(trial(Some(0.1), 10), Verdict::Pass, false, None),
                ),
                (
                    key(2, 0, 0),
                    scored(trial(Some(0.1), 10), Verdict::Pass, true, None),
                ),
                (
                    key(3, 0, 0),
                    scored(trial(Some(0.1), 10), Verdict::Fail, false, None),
                ),
            ],
        );

        let results = results(&record);

        let as_is = &results.configurations[0];
        approx(as_is.pass_rate, 0.5);
        assert_eq!(as_is.tasks[0].passes, Some(passes(1, 2)));
        assert_eq!(as_is.tasks[0].figures.leaks, 1);
        assert_eq!(as_is.figures.leaks, 1);
        assert_eq!(as_is.figures.unscored, 0);
        assert_eq!((results.trials_ended, results.all), (3, 3));
    }

    #[test_log::test]
    fn each_task_has_the_figures_of_a_configuration_of_its_own_trials() {
        let record = benchmark_run(
            &["columns", "lanes"],
            &[],
            4,
            vec![
                (
                    key(1, 0, 0),
                    scored(trial(Some(0.5), 60), Verdict::Pass, false, None),
                ),
                (
                    key(2, 0, 0),
                    scored(trial(None, 30), Verdict::Pass, false, None),
                ),
                (
                    key(3, 0, 0),
                    scored(trial(Some(0.25), 90), Verdict::Fail, false, None),
                ),
                (
                    key(4, 0, 0),
                    scored(trial(Some(0.75), 60), Verdict::Pass, true, None),
                ),
                (
                    key(1, 1, 0),
                    scored(trial(Some(1.0), 100), Verdict::Pass, false, None),
                ),
                (
                    key(2, 1, 0),
                    unscored(NoScore::Outcome(trial(Some(2.0), 200))),
                ),
            ],
        );

        let as_is = &results(&record).configurations[0];

        let columns = &as_is.tasks[0];
        assert_eq!(columns.name.as_str(), "columns");
        assert_eq!(columns.passes, Some(passes(2, 3)));
        approx(columns.figures.cost.mean, 0.5);
        approx(Some(columns.figures.cost.total), 1.5);
        assert_eq!(columns.figures.cost.none, 1);
        approx(columns.figures.time.mean, 60.0);
        approx(Some(columns.figures.time.total), 240.0);
        assert_eq!((columns.figures.leaks, columns.figures.unscored), (1, 0));
        let lanes = &as_is.tasks[1];
        assert_eq!(lanes.passes, Some(passes(1, 1)));
        approx(lanes.figures.cost.mean, 1.5);
        approx(Some(lanes.figures.cost.total), 3.0);
        assert_eq!(lanes.figures.cost.none, 0);
        approx(Some(lanes.figures.time.total), 300.0);
        assert_eq!((lanes.figures.leaks, lanes.figures.unscored), (0, 1));
        approx(as_is.pass_rate, 0.75);
        approx(as_is.figures.cost.mean, 0.9);
        approx(Some(as_is.figures.cost.total), 4.5);
        assert_eq!(as_is.figures.cost.none, 1);
        approx(Some(as_is.figures.time.total), 540.0);
        assert_eq!((as_is.figures.leaks, as_is.figures.unscored), (1, 1));
    }

    #[test_log::test]
    fn with_nothing_to_count_the_passes_of_a_task_a_pass_rate_and_a_mean_are_absent() {
        let record = benchmark_run(
            &["lanes"],
            &["short"],
            1,
            vec![
                (key(1, 0, 0), TrialState::Running),
                (key(1, 0, 1), unscored(NoScore::Outcome(trial(None, 10)))),
            ],
        );

        let results = results(&record);

        let empty = Mean {
            mean: None,
            total: 0.0,
            none: 0,
        };
        let as_is = &results.configurations[0];
        assert_eq!(as_is.pass_rate, None);
        assert_eq!(as_is.tasks[0].passes, None);
        assert_eq!(as_is.figures.cost, empty);
        assert_eq!(as_is.figures.time, empty);
        assert_eq!(as_is.tasks[0].figures.cost, empty);
        let short = &results.configurations[1];
        assert_eq!(short.name.as_str(), "short");
        assert_eq!(short.pass_rate, None);
        assert_eq!(short.tasks[0].passes, None);
        assert_eq!(short.figures.cost.mean, None);
        assert_eq!(short.figures.cost.none, 1);
        assert_eq!(short.figures.unscored, 1);
    }

    #[test_log::test]
    fn a_trial_has_ended_once_its_run_has_whether_or_not_its_scorer_has_and_not_when_cancelled_before()
     {
        let record = benchmark_run(
            &["columns", "lanes", "rows"],
            &["short"],
            1,
            vec![
                (key(1, 0, 0), TrialState::Running),
                (
                    key(1, 0, 1),
                    TrialState::Ran {
                        run: trial(Some(1.0), 10),
                    },
                ),
                (
                    key(1, 1, 0),
                    TrialState::Scoring {
                        run: trial(Some(2.0), 20),
                    },
                ),
                (
                    key(1, 1, 1),
                    scored(trial(Some(4.0), 40), Verdict::Pass, false, None),
                ),
                (
                    key(1, 2, 0),
                    TrialState::Ended(TrialEnd::Cancelled(Cancelled::BeforeRun)),
                ),
            ],
        );

        let results = results(&record);

        assert_eq!((results.trials_ended, results.all), (3, 6));
        assert_eq!(
            results.cost,
            Total {
                total: 7.0,
                none: 0
            }
        );
        let as_is = &results.configurations[0];
        approx(Some(as_is.figures.cost.total), 2.0);
        assert_eq!(as_is.figures.cost.none, 0);
        approx(Some(as_is.figures.time.total), 20.0);
        assert_eq!(as_is.figures.time.none, 0);
        assert_eq!(as_is.pass_rate, None);
        assert_eq!(as_is.figures.unscored, 0);
    }

    #[test_log::test]
    fn unscored_counts_the_trials_without_a_score_for_a_reason_the_spec_gives() {
        let record = benchmark_run(
            &["columns"],
            &[],
            5,
            vec![
                (
                    key(1, 0, 0),
                    unscored(NoScore::Outcome(trial(Some(0.1), 10))),
                ),
                (
                    key(2, 0, 0),
                    unscored(NoScore::Scorer {
                        run: trial(Some(0.1), 10),
                        why: "it gave up".into(),
                        scorer_money: None,
                    }),
                ),
                (
                    key(3, 0, 0),
                    TrialState::Ended(TrialEnd::Cancelled(Cancelled::AfterRun(trial(
                        Some(0.1),
                        10,
                    )))),
                ),
                (
                    key(4, 0, 0),
                    TrialState::Ended(TrialEnd::Cancelled(Cancelled::BeforeRun)),
                ),
            ],
        );

        let as_is = &results(&record).configurations[0];

        assert_eq!(as_is.figures.unscored, 2);
        assert_eq!(as_is.tasks[0].figures.unscored, 2);
    }

    /// `docs/specs/benchmarks.md`, Benchmark runs, A trial without a score: a
    /// trial whose build failed has no run, so it has its time and no cost.
    #[test_log::test]
    fn a_trial_whose_build_failed_is_unscored_with_its_time_and_no_cost() {
        let record = benchmark_run(
            &["columns"],
            &[],
            2,
            vec![
                (
                    key(1, 0, 0),
                    unscored(NoScore::BuildFailed {
                        time: Duration::from_secs(5),
                    }),
                ),
                (
                    key(2, 0, 0),
                    scored(trial(Some(1.0), 15), Verdict::Pass, false, None),
                ),
            ],
        );

        let results = results(&record);

        assert_eq!((results.trials_ended, results.all), (2, 2));
        assert_eq!(results.cost.none, 1);
        let as_is = &results.configurations[0];
        assert_eq!(as_is.figures.unscored, 1);
        approx(Some(as_is.figures.cost.total), 1.0);
        assert_eq!(as_is.figures.cost.none, 1);
        approx(Some(as_is.figures.time.total), 20.0);
        assert_eq!(as_is.figures.time.none, 0);
        assert_eq!(
            as_is.pass_rate,
            Some(1.0),
            "a trial whose build failed is not among those passing or failing"
        );
    }

    /// `docs/specs/benchmarks.md`, Benchmark runs, Cancelled trials: a
    /// cancelled trial has no reason, but its run's cost and time count, and it
    /// has ended once its run has.
    #[test_log::test]
    fn a_cancelled_trial_is_not_unscored_and_the_run_it_had_counts() {
        let record = benchmark_run(
            &["columns"],
            &[],
            2,
            vec![
                (
                    key(1, 0, 0),
                    TrialState::Ended(TrialEnd::Cancelled(Cancelled::AfterRun(trial(
                        Some(0.5),
                        30,
                    )))),
                ),
                (
                    key(2, 0, 0),
                    TrialState::Ended(TrialEnd::Cancelled(Cancelled::BeforeRun)),
                ),
            ],
        );

        let results = results(&record);

        let as_is = &results.configurations[0];
        assert_eq!(as_is.figures.unscored, 0);
        approx(Some(as_is.figures.cost.total), 0.5);
        approx(Some(as_is.figures.time.total), 30.0);
        approx(Some(results.cost.total), 0.5);
        assert_eq!(results.trials_ended, 1);
    }

    /// `docs/specs/benchmarks.md`, Benchmark runs, Cancelled trials: the run
    /// of a trial in flight that the cancel stopped has ended and keeps its
    /// cost, which counts in the benchmark run's, but in no configuration's
    /// figures.
    #[test_log::test]
    fn a_run_the_cancel_stopped_has_ended_and_counts_in_the_benchmark_runs_cost_alone() {
        let stopped = |money: Option<f64>| {
            TrialState::Ended(TrialEnd::Cancelled(Cancelled::RunStopped(TrialFigures {
                outcome: OutcomeName::Ended(Ending::Cancelled),
                money: money.and_then(Usd::new),
                time: Duration::from_secs(40),
            })))
        };
        let record = benchmark_run(
            &["columns"],
            &[],
            3,
            vec![
                (
                    key(1, 0, 0),
                    scored(trial(Some(1.0), 10), Verdict::Pass, false, None),
                ),
                (key(2, 0, 0), stopped(Some(0.25))),
                (key(3, 0, 0), stopped(None)),
            ],
        );

        let results = results(&record);

        assert_eq!((results.trials_ended, results.all), (3, 3));
        approx(Some(results.cost.total), 1.25);
        assert_eq!(results.cost.none, 1);
        let as_is = &results.configurations[0];
        approx(Some(as_is.figures.cost.total), 1.0);
        assert_eq!(as_is.figures.cost.none, 0);
        approx(Some(as_is.figures.time.total), 10.0);
        assert_eq!(as_is.figures.time.none, 0);
        assert_eq!(as_is.figures.unscored, 0);
        approx(as_is.pass_rate, 1.0);
        approx(Some(as_is.tasks[0].figures.cost.total), 1.0);
        assert_eq!(as_is.tasks[0].figures.unscored, 0);
    }

    #[test_log::test]
    fn the_benchmark_runs_cost_sums_its_trials_and_its_scorers_and_counts_the_trials_with_none() {
        let record = benchmark_run(
            &["lanes"],
            &["short"],
            2,
            vec![
                (
                    key(1, 0, 0),
                    scored(trial(Some(1.0), 10), Verdict::Pass, false, Some(0.05)),
                ),
                (
                    key(1, 0, 1),
                    scored(trial(None, 10), Verdict::Fail, false, Some(0.07)),
                ),
                (
                    key(2, 0, 0),
                    unscored(NoScore::Scorer {
                        run: trial(Some(1.0), 10),
                        why: "it gave up".into(),
                        scorer_money: Usd::new(0.02),
                    }),
                ),
                (
                    key(2, 0, 1),
                    scored(trial(Some(2.0), 10), Verdict::Pass, false, None),
                ),
            ],
        );

        let results = results(&record);

        approx(Some(results.configurations[0].figures.cost.total), 2.0);
        approx(Some(results.configurations[1].figures.cost.total), 2.0);
        approx(Some(results.scorers.total), 0.14);
        assert_eq!(results.scorers.none, 1);
        approx(Some(results.cost.total), 4.14);
        assert_eq!(results.cost.none, 1);
    }
}
