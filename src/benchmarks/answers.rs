//! Benchmark runs as the commands answer them (`docs/specs/benchmarks.md`,
//! Benchmarks and benchmark runs on the command line).

use std::fmt::Write as _;
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::cost::Usd;
use crate::domain::Timestamp;
use crate::holds::{BenchmarkRunHold, Hold};
use crate::phrase::Phrase;
use crate::reconcile::Wait;
use crate::results::Verdict;
use crate::runs::OutcomeName;
use crate::status::describe_wait;
use crate::task::{TaskName, TaskStage};

use super::results::{self, Figures, Mean, Results};
use super::runs::outcome_name;
use super::{
    BenchmarkName, BenchmarkRunId, BenchmarkRunRecord, BenchmarkRunState, ConfigurationName,
    NoScore, Repetitions, TrialEnd, TrialFigures, TrialState,
};

/// A benchmark run's state as it is shown: an open benchmark run runs or waits,
/// as `serve` alone knows, and waits for `serve` where it does not answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "state", content = "waiting_for")]
pub enum Shown {
    Running,
    Waiting(WaitingFor),
    Ended,
    Cancelled,
}

/// What a waiting benchmark run waits for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// What `benchmark-run show ID` answers.
#[derive(Serialize)]
pub struct BenchmarkRunAnswer {
    pub id: BenchmarkRunId,
    pub benchmark: BenchmarkName,
    pub stage: TaskStage,
    pub launched_at: Timestamp,
    /// When it ended or was cancelled, if it has and that was kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    #[serde(flatten)]
    pub shown: Shown,
    pub repetitions: Repetitions,
    pub tasks: Vec<TaskName>,
    pub configurations: Vec<ConfigurationName>,
    /// Every trial, in the order they start.
    pub trials: Vec<KeyedTrialAnswer>,
    pub results: Results,
}

/// A trial of a benchmark run, with its score.
#[derive(Serialize)]
pub struct KeyedTrialAnswer {
    pub repetition: NonZeroU32,
    pub task: TaskName,
    pub configuration: ConfigurationName,
    #[serde(flatten)]
    pub state: TrialAnswer,
}

/// Where a trial and its scorer stand.
#[derive(Serialize)]
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
        /// `None` when it was cancelled before its run ended.
        #[serde(flatten)]
        run: Option<TrialFiguresAnswer>,
        #[serde(flatten)]
        why: WhyUnscored,
    },
}

/// Why a trial has no score.
#[derive(Serialize)]
#[serde(tag = "unscored", rename_all = "lowercase")]
pub enum WhyUnscored {
    Outcome,
    Scorer {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        scorer_cost: Option<Usd>,
    },
    Cancelled,
}

#[derive(Serialize)]
pub struct TrialFiguresAnswer {
    pub outcome: OutcomeName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<Usd>,
    pub seconds: f64,
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
                run: Some(no_score.run().into()),
                why: match no_score {
                    NoScore::Outcome(_) => WhyUnscored::Outcome,
                    NoScore::Scorer {
                        why, scorer_money, ..
                    } => WhyUnscored::Scorer {
                        reason: why.clone(),
                        scorer_cost: *scorer_money,
                    },
                },
            },
            TrialState::Ended(TrialEnd::Cancelled { run }) => TrialAnswer::Unscored {
                run: run.as_ref().map(Into::into),
                why: WhyUnscored::Cancelled,
            },
        }
    }
}

impl BenchmarkRunAnswer {
    pub fn new(record: &BenchmarkRunRecord, shown: Shown) -> BenchmarkRunAnswer {
        let trials = record
            .trials
            .iter()
            .filter_map(|(key, state)| {
                Some(KeyedTrialAnswer {
                    repetition: key.repetition,
                    task: record.tasks.get(key.task)?.clone(),
                    configuration: record.configurations.get(key.configuration)?.clone(),
                    state: state.into(),
                })
            })
            .collect();
        BenchmarkRunAnswer {
            id: record.id,
            benchmark: record.benchmark.name.clone(),
            stage: record.benchmark.stage,
            launched_at: record.launched_at,
            ended_at: record.state.ended_at(),
            shown,
            repetitions: record.repetitions,
            tasks: record.tasks.clone(),
            configurations: record.configurations.clone(),
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
        "benchmark run: {}\nbenchmark: {}\nstage: {}\nlaunched: {}\n{}state: {}\nrepetitions: {}\n\
         trials ended: {} of {}\ncost: {cost}\nscorers' cost: {scorers}\n",
        answer.id,
        answer.benchmark,
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
            "configuration {}:\n  pass rate: {}\n{}",
            configuration.name,
            percent(configuration.pass_rate),
            describe_figures(&configuration.figures, "  "),
        );
        for task in &configuration.tasks {
            let _ = write!(
                text,
                "  task {}:\n    passes: {}\n{}",
                task.task,
                task.passes
                    .map_or_else(|| "none".into(), |passes| passes.to_string()),
                describe_figures(&task.figures, "    "),
            );
        }
    }
    for trial in &answer.trials {
        let _ = writeln!(
            text,
            "trial {}/{}/{}: {}",
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

/// A benchmark run as `benchmark show NAME` lists it.
#[derive(Serialize)]
pub struct BenchmarkRunSummary {
    pub id: BenchmarkRunId,
    pub launched_at: Timestamp,
    #[serde(flatten)]
    pub shown: Shown,
    pub pass_rates: Vec<PassRate>,
}

#[derive(Serialize)]
pub struct PassRate {
    pub configuration: ConfigurationName,
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
                    configuration: configuration.name,
                    pass_rate: configuration.pass_rate,
                })
                .collect(),
        }
    }
}

/// A benchmark run as a line of `benchmark show NAME`.
pub(super) fn render_summary(summary: &BenchmarkRunSummary) -> String {
    let rates: Vec<String> = summary
        .pass_rates
        .iter()
        .map(|rate| format!("{} {}", rate.configuration, percent(rate.pass_rate)))
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
    use std::time::Duration;

    use super::*;
    use crate::benchmarks::{Benchmark, ScoreFigures, TrialKey};
    use crate::runs::Ending;

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
                TrialState::Ended(TrialEnd::Cancelled { run: None }),
            ),
            (key(2, 1), TrialState::Running),
        ]);
        let record = BenchmarkRunRecord {
            id: "7".parse().unwrap(),
            benchmark: Benchmark {
                name: "board".parse().unwrap(),
                stage: TaskStage::Design,
            },
            launched_at: "2026-11-17T12:00:00Z".parse().unwrap(),
            repetitions: NonZeroU32::MIN,
            state: BenchmarkRunState::Cancelled {
                at: Some("2026-11-17T12:30:00Z".parse().unwrap()),
            },
            tasks: ["columns", "lanes", "rows"]
                .iter()
                .map(|task| task.parse().unwrap())
                .collect(),
            configurations: ["as-is", "short"]
                .iter()
                .map(|name| name.parse().unwrap())
                .collect(),
            trials,
        };

        let text = render_benchmark_run(&BenchmarkRunAnswer::new(
            &record,
            Shown::without_serve(record.state),
        ));

        assert_eq!(
            text,
            "benchmark run: 7\n\
             benchmark: board\n\
             stage: design\n\
             launched: 2026-11-17 12:00:00 UTC\n\
             ended: 2026-11-17 12:30:00 UTC\n\
             state: cancelled\n\
             repetitions: 1\n\
             trials ended: 4 of 6\n\
             cost: $0.66, 1 without a cost\n\
             scorers' cost: $0.06, 1 without a cost\n\
             configuration as-is:\n  \
               pass rate: 100%\n  \
               cost: mean $0.20, total $0.40\n  \
               time: mean 50s, total 1m 40s\n  \
               leaks: 1\n  \
               unscored: 0\n  \
               task columns:\n    \
                 passes: 1 of 1\n    \
                 cost: mean $0.30, total $0.30\n    \
                 time: mean 1m 30s, total 1m 30s\n    \
                 leaks: 0\n    \
                 unscored: 0\n  \
               task lanes:\n    \
                 passes: none\n    \
                 cost: mean $0.10, total $0.10\n    \
                 time: mean 10s, total 10s\n    \
                 leaks: 1\n    \
                 unscored: 0\n  \
               task rows:\n    \
                 passes: none\n    \
                 cost: none\n    \
                 time: none\n    \
                 leaks: 0\n    \
                 unscored: 0\n\
             configuration short:\n  \
               pass rate: none\n  \
               cost: mean $0.20, total $0.20, 1 without a cost\n  \
               time: mean 25s, total 50s\n  \
               leaks: 0\n  \
               unscored: 2\n  \
               task columns:\n    \
                 passes: none\n    \
                 cost: none, 1 without a cost\n    \
                 time: mean 30s, total 30s\n    \
                 leaks: 0\n    \
                 unscored: 1\n  \
               task lanes:\n    \
                 passes: none\n    \
                 cost: mean $0.20, total $0.20\n    \
                 time: mean 20s, total 20s\n    \
                 leaks: 0\n    \
                 unscored: 1\n  \
               task rows:\n    \
                 passes: none\n    \
                 cost: none\n    \
                 time: none\n    \
                 leaks: 0\n    \
                 unscored: 0\n\
             trial as-is/columns/1: scored pass, run done, cost $0.30, took 1m 30s, scorer's cost $0.05\n\
             trial short/columns/1: no score: its run is not done or handed back, run failed, cost none, took 30s\n\
             trial as-is/lanes/1: scored fail, with a leak, run handed-back, cost $0.10, took 10s\n\
             trial short/lanes/1: no score: the scorer made none: it gave up, run done, cost $0.20, took 20s, scorer's cost $0.01\n\
             trial as-is/rows/1: no score: cancelled\n\
             trial short/rows/1: running\n"
        );
    }

    #[test_log::test]
    fn a_benchmark_run_says_what_it_waits_for() {
        let record = BenchmarkRunRecord {
            id: "3".parse().unwrap(),
            benchmark: Benchmark {
                name: "board".parse().unwrap(),
                stage: TaskStage::Design,
            },
            launched_at: "2026-11-17T12:00:00Z".parse().unwrap(),
            repetitions: NonZeroU32::MIN,
            state: BenchmarkRunState::Open,
            tasks: vec!["columns".parse().unwrap()],
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
