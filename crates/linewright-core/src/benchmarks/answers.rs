//! Benchmark runs as the commands answer them (`docs/specs/benchmarks.md`,
//! Benchmarks and benchmark runs on the command line).

use std::fmt::Write as _;
use std::num::NonZeroU32;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::cost::Usd;
use crate::domain::Timestamp;
use crate::holds::{BenchmarkRunHold, Hold};
use crate::name::Name;
use crate::phrase::Phrase;
use crate::reconcile::Wait;
use crate::results::Verdict;
use crate::runs::OutcomeName;
use crate::status::describe_wait;
use crate::task::TaskStage;

use super::results::{self, Figures, Mean, Results};
use super::runs::outcome_name;
use super::{
    Benchmark, BenchmarkId, BenchmarkRunId, BenchmarkRunRecord, BenchmarkRunState, ConfigurationId,
    NoScore, Repetitions, RunTask, TaskId, TrialEnd, TrialFigures, TrialName, TrialState,
};

/// A benchmark run's state as it is shown: an open benchmark run runs or waits,
/// as `serve` alone knows, and waits for `serve` where it does not answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase", tag = "state", content = "waiting_for")]
pub enum Shown {
    Running,
    Waiting(WaitingFor),
    Ended,
    Cancelled,
}

/// What a waiting benchmark run waits for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum WaitingFor {
    /// No `serve` runs it.
    Serve,
    /// A run of the pipeline waits for a place under `max-runs`.
    Pipeline,
    /// Trials and scorers of benchmark runs take every place under
    /// `max-trials`.
    FreePlace {
        in_flight: usize,
        max_trials: usize,
    },
    Hold(BenchmarkRunHold),
}

impl WaitingFor {
    /// What a benchmark run waits for, in words, such as "the usage limit,
    /// until 14:30".
    pub fn phrase(&self) -> Phrase {
        match self {
            WaitingFor::Serve => "serve".into(),
            WaitingFor::Pipeline => "the pipeline: a run waits for a place under max-runs".into(),
            WaitingFor::FreePlace {
                in_flight,
                max_trials,
            } => format!("a place under max-trials: {in_flight} of {max_trials} in flight").into(),
            WaitingFor::Hold(hold) => describe_wait(&Wait::from(Hold::from(*hold))),
        }
    }
}

impl Shown {
    /// The state of `state` where `serve` does not say how an open
    /// benchmark run stands.
    pub fn without_serve(state: BenchmarkRunState) -> Shown {
        match state {
            BenchmarkRunState::Open => Shown::Waiting(WaitingFor::Serve),
            BenchmarkRunState::Ended { .. } => Shown::Ended,
            BenchmarkRunState::Cancelled { .. } => Shown::Cancelled,
        }
    }

    /// The state of the benchmark run kept as `state`, given how `serve` shows
    /// it, if it does: what is kept wins once the benchmark run is not open.
    pub fn of(state: BenchmarkRunState, served: Option<Shown>) -> Shown {
        match (state, served) {
            (BenchmarkRunState::Open, Some(shown)) => shown,
            (state, _) => Shown::without_serve(state),
        }
    }

    /// The state in words, with what a waiting benchmark run waits for.
    pub fn phrase(&self) -> Phrase {
        match self {
            Shown::Running => "running".into(),
            Shown::Waiting(waiting_for) => Phrase::from("waiting for ").then(waiting_for.phrase()),
            Shown::Ended => "ended".into(),
            Shown::Cancelled => "cancelled".into(),
        }
    }
}

/// A benchmark as a benchmark run's answer names it.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct BenchmarkOfRun {
    pub id: BenchmarkId,
    pub name: Name,
}

/// A configuration of a benchmark run, by its ID.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ConfigurationAnswer {
    pub id: ConfigurationId,
    pub name: Name,
}

/// What `benchmark-run show ID` answers.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct BenchmarkRunAnswer {
    pub id: BenchmarkRunId,
    pub benchmark: BenchmarkOfRun,
    pub stage: TaskStage,
    pub launched_at: Timestamp,
    /// When it ended or was cancelled, if it has and that was kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    #[serde(flatten)]
    pub shown: Shown,
    pub repetitions: Repetitions,
    pub tasks: Vec<RunTask>,
    pub configurations: Vec<ConfigurationAnswer>,
    /// Every trial, in the order they start.
    pub trials: Vec<KeyedTrialAnswer>,
    pub results: Results,
}

/// A trial of a benchmark run, by the IDs of its configuration and its task,
/// with its score.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct KeyedTrialAnswer {
    pub repetition: NonZeroU32,
    pub task: TaskId,
    pub configuration: ConfigurationId,
    #[serde(skip)]
    pub name: TrialName,
    #[serde(flatten)]
    pub state: TrialAnswer,
}

/// Where a trial and its scorer stand.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum TrialAnswer {
    Pending,
    Running,
    Ran {
        #[serde(flatten)]
        run: TrialFiguresAnswer,
    },
    Scoring {
        #[serde(flatten)]
        run: TrialFiguresAnswer,
    },
    Scored {
        #[serde(flatten)]
        run: TrialFiguresAnswer,
        correctness: Verdict,
        leak: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        scorer_cost: Option<Usd>,
    },
    Unscored {
        /// `None` when its build failed, or it was cancelled before its run
        /// ended.
        #[serde(flatten)]
        run: Option<TrialFiguresAnswer>,
        #[serde(flatten)]
        why: WhyUnscored,
    },
}

/// Why a trial has no score.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "unscored", rename_all = "lowercase")]
pub enum WhyUnscored {
    /// The build of its environment failed: it has no run, only its time.
    Build {
        seconds: f64,
    },
    Outcome,
    Scorer {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        scorer_cost: Option<Usd>,
    },
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct TrialFiguresAnswer {
    pub outcome: OutcomeName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<Usd>,
    pub seconds: f64,
}

impl TrialAnswer {
    /// The figures of its run, once its run has ended.
    pub fn run(&self) -> Option<&TrialFiguresAnswer> {
        match self {
            TrialAnswer::Pending | TrialAnswer::Running => None,
            TrialAnswer::Ran { run }
            | TrialAnswer::Scoring { run }
            | TrialAnswer::Scored { run, .. } => Some(run),
            TrialAnswer::Unscored { run, .. } => run.as_ref(),
        }
    }

    /// Its time in seconds, once it has one: its run's, or the time to a
    /// build that failed.
    pub fn seconds(&self) -> Option<f64> {
        match self {
            TrialAnswer::Unscored {
                why: WhyUnscored::Build { seconds },
                ..
            } => Some(*seconds),
            TrialAnswer::Pending
            | TrialAnswer::Running
            | TrialAnswer::Ran { .. }
            | TrialAnswer::Scoring { .. }
            | TrialAnswer::Scored { .. }
            | TrialAnswer::Unscored { .. } => self.run().map(|run| run.seconds),
        }
    }
}

impl From<&TrialFigures> for TrialFiguresAnswer {
    fn from(trial: &TrialFigures) -> TrialFiguresAnswer {
        TrialFiguresAnswer {
            outcome: trial.outcome,
            cost: trial.money,
            seconds: trial.time.as_secs_f64(),
        }
    }
}

impl From<&TrialState> for TrialAnswer {
    fn from(state: &TrialState) -> TrialAnswer {
        match state {
            TrialState::Pending => TrialAnswer::Pending,
            TrialState::Running => TrialAnswer::Running,
            TrialState::Ran { run } => TrialAnswer::Ran { run: run.into() },
            TrialState::Scoring { run } => TrialAnswer::Scoring { run: run.into() },
            TrialState::Ended(TrialEnd::Scored { run, score }) => TrialAnswer::Scored {
                run: run.into(),
                correctness: score.correctness,
                leak: score.leak,
                scorer_cost: score.scorer_money,
            },
            TrialState::Ended(TrialEnd::Unscored(no_score)) => TrialAnswer::Unscored {
                run: no_score.run().map(Into::into),
                why: match no_score {
                    NoScore::BuildFailed { time } => WhyUnscored::Build {
                        seconds: time.as_secs_f64(),
                    },
                    NoScore::Outcome(_) => WhyUnscored::Outcome,
                    NoScore::Scorer {
                        why, scorer_money, ..
                    } => WhyUnscored::Scorer {
                        reason: why.clone(),
                        scorer_cost: *scorer_money,
                    },
                },
            },
            TrialState::Ended(TrialEnd::Cancelled(cancelled)) => TrialAnswer::Unscored {
                run: cancelled.run().map(Into::into),
                why: WhyUnscored::Cancelled,
            },
        }
    }
}

impl From<&Benchmark> for BenchmarkOfRun {
    fn from(benchmark: &Benchmark) -> BenchmarkOfRun {
        BenchmarkOfRun {
            id: benchmark.id,
            name: benchmark.name.clone(),
        }
    }
}

/// Each trial of `record` whose task and configuration it holds, in the
/// order they start: its state as kept, and as `benchmark-run show` answers
/// it.
pub fn keyed_trials(record: &BenchmarkRunRecord) -> Vec<(&TrialState, KeyedTrialAnswer)> {
    record
        .trials
        .iter()
        .filter_map(|(key, state)| {
            let of = record.trial_of(key)?;
            Some((
                state,
                KeyedTrialAnswer {
                    repetition: key.repetition,
                    task: of.task.id,
                    configuration: of.configuration,
                    name: TrialName::new(key.repetition, &of.task.name, of.configuration_name),
                    state: state.into(),
                },
            ))
        })
        .collect()
}

impl BenchmarkRunAnswer {
    pub fn new(record: &BenchmarkRunRecord, shown: Shown) -> BenchmarkRunAnswer {
        let trials = keyed_trials(record)
            .into_iter()
            .map(|(_, trial)| trial)
            .collect();
        BenchmarkRunAnswer {
            id: record.id,
            benchmark: (&record.benchmark).into(),
            stage: record.benchmark.stage,
            launched_at: record.launched_at,
            ended_at: record.state.ended_at(),
            shown,
            repetitions: record.repetitions,
            tasks: record.tasks.clone(),
            configurations: record
                .configurations
                .iter()
                .enumerate()
                .map(|(place, name)| ConfigurationAnswer {
                    id: ConfigurationId::of_place(place),
                    name: name.clone(),
                })
                .collect(),
            trials,
            results: results::results(record),
        }
    }
}

/// A benchmark run as `benchmark-run show ID` prints it: its facts, each
/// configuration's figures, then each trial.
pub fn render_benchmark_run(answer: &BenchmarkRunAnswer) -> String {
    let results = &answer.results;
    let mut cost = dollars(results.cost.total);
    without(&mut cost, results.cost.none, "a cost");
    let mut scorers = dollars(results.scorers.total);
    without(&mut scorers, results.scorers.none, "a cost");
    let mut text = format!(
        "benchmark run: {}\nbenchmark: {} ({})\nstage: {}\nlaunched: {}\n{}state: {}\n\
         repetitions: {}\ntrials ended: {} of {}\ncost: {cost}\nscorers' cost: {scorers}\n",
        answer.id,
        answer.benchmark.id,
        answer.benchmark.name,
        answer.stage,
        Phrase::default().time(answer.launched_at),
        answer.ended_at.map_or_else(String::new, |at| format!(
            "ended: {}\n",
            Phrase::default().time(at)
        )),
        answer.shown.phrase(),
        answer.repetitions,
        results.trials_ended,
        results.all,
    );
    for configuration in &results.configurations {
        let _ = write!(
            text,
            "configuration {} ({}):\n  pass rate: {}\n{}",
            configuration.id,
            configuration.name,
            percent(configuration.pass_rate),
            describe_figures(&configuration.figures, "  "),
        );
        for task in &configuration.tasks {
            let _ = write!(
                text,
                "  task {} ({}):\n    passes: {}\n{}",
                task.id,
                task.name,
                task.passes
                    .map_or_else(|| "none".into(), |passes| passes.to_string()),
                describe_figures(&task.figures, "    "),
            );
        }
    }
    for trial in &answer.trials {
        let _ = writeln!(
            text,
            "{} ({}/{}/{}): {}",
            trial.name,
            trial.configuration,
            trial.task,
            trial.repetition,
            describe_trial(&trial.state)
        );
    }
    text
}

/// The lines of `figures`, each indented by `indent`.
fn describe_figures(figures: &Figures, indent: &str) -> String {
    format!(
        "{indent}cost: {}\n{indent}time: {}\n{indent}leaks: {}\n{indent}unscored: {}\n",
        mean(&figures.cost, dollars, "a cost"),
        mean(&figures.time, duration, "a time"),
        figures.leaks,
        figures.unscored,
    )
}

/// Where a run stands, then its score and its figures, if it has
/// them.
fn describe_trial(state: &TrialAnswer) -> String {
    let (mut words, run, scorer_cost) = match state {
        TrialAnswer::Pending => ("pending".to_string(), None, None),
        TrialAnswer::Running => ("running".into(), None, None),
        TrialAnswer::Ran { run } => ("ran".into(), Some(run), None),
        TrialAnswer::Scoring { run } => ("scoring".into(), Some(run), None),
        TrialAnswer::Scored {
            run,
            correctness,
            leak,
            scorer_cost,
        } => {
            let leak = if *leak { ", with a leak" } else { "" };
            (
                format!("scored {}{leak}", correctness.name()),
                Some(run),
                *scorer_cost,
            )
        }
        TrialAnswer::Unscored { run, why } => {
            let (why, scorer_cost) = match why {
                WhyUnscored::Build { seconds } => (
                    format!(
                        "the build of its environment failed, took {}",
                        duration(*seconds)
                    ),
                    None,
                ),
                WhyUnscored::Outcome => ("its run is not done or handed back".into(), None),
                WhyUnscored::Scorer {
                    reason,
                    scorer_cost,
                } => (format!("the scorer made none: {reason}"), *scorer_cost),
                WhyUnscored::Cancelled => ("cancelled".into(), None),
            };
            (format!("no score: {why}"), run.as_ref(), scorer_cost)
        }
    };
    if let Some(run) = run {
        let _ = write!(
            words,
            ", run {}, cost {}, took {}",
            outcome_name(run.outcome),
            run.cost
                .map_or_else(|| "none".into(), |cost| dollars(cost.into())),
            duration(run.seconds)
        );
    }
    if let Some(scorer_cost) = scorer_cost {
        let _ = write!(words, ", scorer's cost {}", dollars(scorer_cost.into()));
    }
    words
}

/// `figure`'s mean and total in the units `unit` gives, and how many
/// trials have no `what`.
pub fn mean(figure: &Mean, unit: fn(f64) -> String, what: &str) -> String {
    let mut text = match figure.mean {
        Some(each) => format!("mean {}, total {}", unit(each), unit(figure.total)),
        None => "none".into(),
    };
    without(&mut text, figure.none, what);
    text
}

/// Adds to `text` how many have no `what`, if any.
pub fn without(text: &mut String, none: usize, what: &str) {
    if none > 0 {
        let _ = write!(text, ", {none} without {what}");
    }
}

pub fn dollars(amount: f64) -> String {
    format!("${amount:.2}")
}

/// A share as a percentage, or `none` when there is nothing to count.
pub fn percent(share: Option<f64>) -> String {
    share.map_or_else(|| "none".into(), |share| format!("{:.0}%", share * 100.0))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a trial's time is a non-negative number of seconds far below u64::MAX"
)]
pub fn duration(seconds: f64) -> String {
    Phrase::default()
        .duration(seconds.round() as u64, None)
        .to_string()
}

/// A benchmark run as `benchmark show BENCHMARK` lists it.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct BenchmarkRunSummary {
    pub id: BenchmarkRunId,
    pub launched_at: Timestamp,
    #[serde(flatten)]
    pub shown: Shown,
    pub pass_rates: Vec<PassRate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct PassRate {
    pub configuration: ConfigurationAnswer,
    pub pass_rate: Option<f64>,
}

impl BenchmarkRunSummary {
    pub fn new(record: &BenchmarkRunRecord, shown: Shown) -> BenchmarkRunSummary {
        BenchmarkRunSummary {
            id: record.id,
            launched_at: record.launched_at,
            shown,
            pass_rates: results::results(record)
                .configurations
                .into_iter()
                .map(|configuration| PassRate {
                    configuration: ConfigurationAnswer {
                        id: configuration.id,
                        name: configuration.name,
                    },
                    pass_rate: configuration.pass_rate,
                })
                .collect(),
        }
    }
}

/// A benchmark run as a line of `benchmark show BENCHMARK`.
pub(super) fn render_summary(summary: &BenchmarkRunSummary) -> String {
    let rates: Vec<String> = summary
        .pass_rates
        .iter()
        .map(|rate| format!("{} {}", rate.configuration.name, percent(rate.pass_rate)))
        .collect();
    format!(
        "benchmark run {}: {}, launched {}, pass rates {}\n",
        summary.id,
        summary.shown.phrase(),
        Phrase::default().time(summary.launched_at),
        rates.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::num::NonZeroUsize;
    use std::time::Duration;

    use super::*;
    use crate::benchmarks::results::{ConfigurationResults, Passes, TaskResults, Total};
    use crate::benchmarks::{Benchmark, Cancelled, ScoreFigures, TrialKey};
    use crate::runs::Ending;

    /// The tasks `names`, their IDs from 1.
    #[track_caller]
    fn run_tasks(names: &[&str]) -> Vec<RunTask> {
        names
            .iter()
            .zip(1..)
            .map(|(name, id)| RunTask {
                id: id.to_string().parse().unwrap(),
                name: name.parse().unwrap(),
            })
            .collect()
    }

    #[track_caller]
    fn key(task: usize, configuration: usize) -> TrialKey {
        TrialKey {
            repetition: NonZeroU32::MIN,
            task,
            configuration,
        }
    }

    #[track_caller]
    fn trial(ending: Ending, money: Option<f64>, seconds: u64) -> TrialFigures {
        TrialFigures {
            outcome: OutcomeName::Ended(ending),
            money: money.map(|money| Usd::new(money).unwrap()),
            time: Duration::from_secs(seconds),
        }
    }

    /// A figure's mean, total and how many trials have none.
    fn mean_of(mean: Option<f64>, total: f64, none: usize) -> Mean {
        Mean { mean, total, none }
    }

    #[track_caller]
    fn task_results(
        id: &str,
        name: &str,
        passes: Option<(usize, usize)>,
        figures: Figures,
    ) -> TaskResults {
        TaskResults {
            id: id.parse().unwrap(),
            name: name.parse().unwrap(),
            passes: passes.map(|(pass, of)| Passes {
                pass,
                of: NonZeroUsize::new(of).unwrap(),
            }),
            figures,
        }
    }

    /// Figures with nothing to count.
    fn no_figures() -> Figures {
        Figures {
            cost: mean_of(None, 0.0, 0),
            time: mean_of(None, 0.0, 0),
            leaks: 0,
            unscored: 0,
        }
    }

    /// Results of two configurations, with a figure and with none, and with
    /// and without trials that lack it, set by hand and not worked out from
    /// any trials.
    #[track_caller]
    fn given_results() -> Results {
        Results {
            configurations: vec![
                ConfigurationResults {
                    id: ConfigurationId::of_place(0),
                    name: "as-is".parse().unwrap(),
                    pass_rate: Some(0.5),
                    figures: Figures {
                        cost: mean_of(Some(0.2), 0.4, 0),
                        time: mean_of(Some(50.0), 100.0, 0),
                        leaks: 1,
                        unscored: 0,
                    },
                    tasks: vec![
                        task_results(
                            "1",
                            "columns",
                            Some((1, 2)),
                            Figures {
                                cost: mean_of(Some(0.3), 0.6, 0),
                                time: mean_of(Some(90.0), 180.0, 0),
                                leaks: 1,
                                unscored: 0,
                            },
                        ),
                        task_results("3", "rows", None, no_figures()),
                    ],
                },
                ConfigurationResults {
                    id: ConfigurationId::of_place(1),
                    name: "short".parse().unwrap(),
                    pass_rate: None,
                    figures: Figures {
                        cost: mean_of(None, 0.0, 2),
                        time: mean_of(Some(25.0), 50.0, 0),
                        leaks: 0,
                        unscored: 2,
                    },
                    tasks: vec![task_results(
                        "2",
                        "lanes",
                        None,
                        Figures {
                            cost: mean_of(Some(0.2), 0.2, 1),
                            time: mean_of(Some(20.0), 20.0, 0),
                            leaks: 0,
                            unscored: 1,
                        },
                    )],
                },
            ],
            scorers: Total {
                total: 0.06,
                none: 1,
            },
            trials_ended: 5,
            all: 7,
            cost: Total {
                total: 0.7,
                none: 1,
            },
        }
    }

    #[test_log::test]
    fn a_benchmark_run_prints_its_facts_its_configurations_figures_and_each_trial() {
        let trials = BTreeMap::from([
            (
                key(0, 0),
                TrialState::Ended(TrialEnd::Scored {
                    run: trial(Ending::Done, Some(0.3), 90),
                    score: ScoreFigures {
                        correctness: Verdict::Pass,
                        leak: false,
                        scorer_money: Usd::new(0.05),
                    },
                }),
            ),
            (
                key(0, 1),
                TrialState::Ended(TrialEnd::Unscored(NoScore::Outcome(trial(
                    Ending::Failed,
                    None,
                    30,
                )))),
            ),
            (
                key(1, 0),
                TrialState::Ended(TrialEnd::Scored {
                    run: trial(Ending::HandedBack, Some(0.1), 10),
                    score: ScoreFigures {
                        correctness: Verdict::Fail,
                        leak: true,
                        scorer_money: None,
                    },
                }),
            ),
            (
                key(1, 1),
                TrialState::Ended(TrialEnd::Unscored(NoScore::Scorer {
                    run: trial(Ending::Done, Some(0.2), 20),
                    why: "it gave up".into(),
                    scorer_money: Usd::new(0.01),
                })),
            ),
            (
                key(2, 0),
                TrialState::Ended(TrialEnd::Cancelled(Cancelled::BeforeRun)),
            ),
            (
                key(2, 1),
                TrialState::Ended(TrialEnd::Cancelled(Cancelled::RunStopped(trial(
                    Ending::Cancelled,
                    Some(0.04),
                    15,
                )))),
            ),
            (
                TrialKey {
                    repetition: NonZeroU32::new(2).unwrap(),
                    ..key(2, 1)
                },
                TrialState::Running,
            ),
        ]);
        let record = BenchmarkRunRecord {
            id: "7".parse().unwrap(),
            benchmark: Benchmark {
                id: "1".parse().unwrap(),
                name: "board".parse().unwrap(),
                stage: TaskStage::Design,
            },
            launched_at: "2026-11-17T12:00:00Z".parse().unwrap(),
            repetitions: NonZeroU32::new(2).unwrap(),
            state: BenchmarkRunState::Cancelled {
                at: Some("2026-11-17T12:30:00Z".parse().unwrap()),
            },
            tasks: run_tasks(&["columns", "lanes", "rows"]),
            configurations: ["as-is", "short"]
                .iter()
                .map(|name| name.parse().unwrap())
                .collect(),
            trials,
        };

        let answer = BenchmarkRunAnswer {
            results: given_results(),
            ..BenchmarkRunAnswer::new(&record, Shown::without_serve(record.state))
        };

        let text = render_benchmark_run(&answer);

        assert_eq!(
            text,
            "benchmark run: 7\n\
             benchmark: 1 (board)\n\
             stage: design\n\
             launched: 2026-11-17 12:00:00 UTC\n\
             ended: 2026-11-17 12:30:00 UTC\n\
             state: cancelled\n\
             repetitions: 2\n\
             trials ended: 5 of 7\n\
             cost: $0.70, 1 without a cost\n\
             scorers' cost: $0.06, 1 without a cost\n\
             configuration 1 (as-is):\n  \
               pass rate: 50%\n  \
               cost: mean $0.20, total $0.40\n  \
               time: mean 50s, total 1m 40s\n  \
               leaks: 1\n  \
               unscored: 0\n  \
               task 1 (columns):\n    \
                 passes: 1 of 2\n    \
                 cost: mean $0.30, total $0.60\n    \
                 time: mean 1m 30s, total 3m\n    \
                 leaks: 1\n    \
                 unscored: 0\n  \
               task 3 (rows):\n    \
                 passes: none\n    \
                 cost: none\n    \
                 time: none\n    \
                 leaks: 0\n    \
                 unscored: 0\n\
             configuration 2 (short):\n  \
               pass rate: none\n  \
               cost: none, 2 without a cost\n  \
               time: mean 25s, total 50s\n  \
               leaks: 0\n  \
               unscored: 2\n  \
               task 2 (lanes):\n    \
                 passes: none\n    \
                 cost: mean $0.20, total $0.20, 1 without a cost\n    \
                 time: mean 20s, total 20s\n    \
                 leaks: 0\n    \
                 unscored: 1\n\
             trial 1 of columns under as-is (1/1/1): scored pass, run done, cost $0.30, took 1m 30s, scorer's cost $0.05\n\
             trial 1 of columns under short (2/1/1): no score: its run is not done or handed back, run failed, cost none, took 30s\n\
             trial 1 of lanes under as-is (1/2/1): scored fail, with a leak, run handed-back, cost $0.10, took 10s\n\
             trial 1 of lanes under short (2/2/1): no score: the scorer made none: it gave up, run done, cost $0.20, took 20s, scorer's cost $0.01\n\
             trial 1 of rows under as-is (1/3/1): no score: cancelled\n\
             trial 1 of rows under short (2/3/1): no score: cancelled, run cancelled, cost $0.04, took 15s\n\
             trial 2 of rows under short (2/3/2): running\n"
        );
    }

    #[test_log::test]
    fn a_trial_whose_build_failed_prints_why_and_its_time_with_no_run() {
        let state = TrialState::Ended(TrialEnd::Unscored(NoScore::BuildFailed {
            time: Duration::from_secs(9),
        }));

        let answer = TrialAnswer::from(&state);

        assert_eq!(
            describe_trial(&answer),
            "no score: the build of its environment failed, took 9s"
        );
        assert_eq!(
            serde_json::to_value(&answer).unwrap(),
            serde_json::json!({"state": "unscored", "unscored": "build", "seconds": 9.0})
        );
    }

    #[test_log::test]
    fn a_benchmark_run_says_what_it_waits_for() {
        let record = BenchmarkRunRecord {
            id: "3".parse().unwrap(),
            benchmark: Benchmark {
                id: "1".parse().unwrap(),
                name: "board".parse().unwrap(),
                stage: TaskStage::Design,
            },
            launched_at: "2026-11-17T12:00:00Z".parse().unwrap(),
            repetitions: NonZeroU32::MIN,
            state: BenchmarkRunState::Open,
            tasks: run_tasks(&["columns"]),
            configurations: vec!["as-is".parse().unwrap()],
            trials: BTreeMap::from([(key(0, 0), TrialState::Pending)]),
        };
        let line = |shown| render_summary(&BenchmarkRunSummary::new(&record, shown));

        assert_eq!(
            [
                Shown::Running,
                Shown::Waiting(WaitingFor::Serve),
                Shown::Waiting(WaitingFor::Pipeline),
                Shown::Waiting(WaitingFor::FreePlace {
                    in_flight: 1,
                    max_trials: 1
                }),
                Shown::Waiting(WaitingFor::Hold(BenchmarkRunHold::UsageLimit {
                    resets_at: "2026-11-17T14:30:00Z".parse().unwrap()
                })),
            ]
            .map(line),
            [
                "benchmark run 3: running, launched 2026-11-17 12:00:00 UTC, pass rates as-is none\n",
                "benchmark run 3: waiting for serve, launched 2026-11-17 12:00:00 UTC, pass rates as-is none\n",
                "benchmark run 3: waiting for the pipeline: a run waits for a place under max-runs, \
                 launched 2026-11-17 12:00:00 UTC, pass rates as-is none\n",
                "benchmark run 3: waiting for a place under max-trials: 1 of 1 in flight, \
                 launched 2026-11-17 12:00:00 UTC, pass rates as-is none\n",
                "benchmark run 3: waiting for the usage limit, until 2026-11-17 14:30:00 UTC, \
                 launched 2026-11-17 12:00:00 UTC, pass rates as-is none\n",
            ]
        );
    }

    #[test_log::test]
    fn a_benchmark_run_not_open_shows_as_it_is_kept_whatever_serve_last_showed() {
        assert_eq!(
            Shown::of(
                BenchmarkRunState::Cancelled { at: None },
                Some(Shown::Running)
            ),
            Shown::Cancelled
        );
        assert_eq!(
            Shown::of(BenchmarkRunState::Open, Some(Shown::Running)),
            Shown::Running
        );
        assert_eq!(
            Shown::of(BenchmarkRunState::Open, None),
            Shown::Waiting(WaitingFor::Serve)
        );
    }
}
