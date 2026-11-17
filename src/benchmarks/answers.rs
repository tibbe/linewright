//! Experiments as the commands answer them (`docs/specs/benchmarks.md`,
//! Suites and experiments on the command line).

use std::fmt::Write as _;
use std::num::NonZeroU32;

use serde::Serialize;

use crate::case::{CaseName, ReplayStage};
use crate::cost::Usd;
use crate::domain::Timestamp;
use crate::phrase::Phrase;
use crate::results::Verdict;
use crate::runs::OutcomeName;

use super::experiments::outcome_name;
use super::results::{self, Mean, Results};
use super::{
    ConfigurationName, ExperimentId, ExperimentRecord, ExperimentState, NoJudgement, Repetitions,
    ReplayFigures, SuiteName, TrialEnd, TrialState,
};

/// An experiment's state as it is shown: an open experiment runs or waits,
/// as `serve` alone knows, and waits for `serve` where it does not answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase", tag = "state", content = "waiting_for")]
pub enum Shown {
    Waiting(WaitingFor),
    Ended,
    Cancelled,
}

/// What a waiting experiment waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WaitingFor {
    Serve,
}

impl Shown {
    /// The state of `state` where `serve` does not say how an open
    /// experiment stands.
    pub fn without_serve(state: ExperimentState) -> Shown {
        match state {
            ExperimentState::Open => Shown::Waiting(WaitingFor::Serve),
            ExperimentState::Ended => Shown::Ended,
            ExperimentState::Cancelled => Shown::Cancelled,
        }
    }

    fn words(self) -> &'static str {
        match self {
            Shown::Waiting(WaitingFor::Serve) => "waiting for serve",
            Shown::Ended => "ended",
            Shown::Cancelled => "cancelled",
        }
    }
}

/// What `experiment ID` answers.
#[derive(Serialize)]
pub struct ExperimentAnswer {
    pub id: ExperimentId,
    pub suite: SuiteName,
    pub stage: ReplayStage,
    pub launched_at: Timestamp,
    #[serde(flatten)]
    pub shown: Shown,
    pub repetitions: Repetitions,
    pub cases: Vec<CaseName>,
    pub configurations: Vec<ConfigurationName>,
    /// Every replay, in the order they start.
    pub replays: Vec<ReplayAnswer>,
    pub results: Results,
}

/// A replay of an experiment, with its judgement.
#[derive(Serialize)]
pub struct ReplayAnswer {
    pub repetition: NonZeroU32,
    pub case: CaseName,
    pub configuration: ConfigurationName,
    #[serde(flatten)]
    pub trial: TrialAnswer,
}

/// Where a replay and its judge stand.
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum TrialAnswer {
    Pending,
    Replaying,
    Replayed {
        #[serde(flatten)]
        replay: ReplayFiguresAnswer,
    },
    Judging {
        #[serde(flatten)]
        replay: ReplayFiguresAnswer,
    },
    Judged {
        #[serde(flatten)]
        replay: ReplayFiguresAnswer,
        correctness: Verdict,
        leak: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        judge_cost: Option<Usd>,
    },
    Unjudged {
        /// `None` when it was cancelled before its replay ended.
        #[serde(flatten)]
        replay: Option<ReplayFiguresAnswer>,
        #[serde(flatten)]
        why: WhyUnjudged,
    },
}

/// Why a replay has no judgement.
#[derive(Serialize)]
#[serde(tag = "unjudged", rename_all = "lowercase")]
pub enum WhyUnjudged {
    Outcome,
    Judge {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        judge_cost: Option<Usd>,
    },
    Cancelled,
}

#[derive(Serialize)]
pub struct ReplayFiguresAnswer {
    pub outcome: OutcomeName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<Usd>,
    pub seconds: f64,
}

impl From<&ReplayFigures> for ReplayFiguresAnswer {
    fn from(replay: &ReplayFigures) -> ReplayFiguresAnswer {
        ReplayFiguresAnswer {
            outcome: replay.outcome,
            cost: replay.money,
            seconds: replay.time.as_secs_f64(),
        }
    }
}

impl From<&TrialState> for TrialAnswer {
    fn from(state: &TrialState) -> TrialAnswer {
        match state {
            TrialState::Pending => TrialAnswer::Pending,
            TrialState::Replaying => TrialAnswer::Replaying,
            TrialState::Replayed { replay } => TrialAnswer::Replayed {
                replay: replay.into(),
            },
            TrialState::Judging { replay } => TrialAnswer::Judging {
                replay: replay.into(),
            },
            TrialState::Ended(TrialEnd::Judged { replay, judgement }) => TrialAnswer::Judged {
                replay: replay.into(),
                correctness: judgement.correctness,
                leak: judgement.leak,
                judge_cost: judgement.judge_money,
            },
            TrialState::Ended(TrialEnd::Unjudged { replay, why }) => TrialAnswer::Unjudged {
                replay: Some(replay.into()),
                why: match why {
                    NoJudgement::Outcome => WhyUnjudged::Outcome,
                    NoJudgement::Judge {
                        reason,
                        judge_money,
                    } => WhyUnjudged::Judge {
                        reason: reason.clone(),
                        judge_cost: *judge_money,
                    },
                    NoJudgement::Cancelled => WhyUnjudged::Cancelled,
                },
            },
            TrialState::Ended(TrialEnd::Cancelled) => TrialAnswer::Unjudged {
                replay: None,
                why: WhyUnjudged::Cancelled,
            },
        }
    }
}

impl ExperimentAnswer {
    pub fn new(record: &ExperimentRecord, shown: Shown) -> ExperimentAnswer {
        let replays = record
            .trials
            .iter()
            .filter_map(|(key, state)| {
                Some(ReplayAnswer {
                    repetition: key.repetition,
                    case: record.cases.get(key.case)?.clone(),
                    configuration: record.configurations.get(key.configuration)?.clone(),
                    trial: state.into(),
                })
            })
            .collect();
        ExperimentAnswer {
            id: record.id,
            suite: record.suite.name.clone(),
            stage: record.suite.stage,
            launched_at: record.launched_at,
            shown,
            repetitions: record.repetitions,
            cases: record.cases.clone(),
            configurations: record.configurations.clone(),
            replays,
            results: results::results(record),
        }
    }
}

/// An experiment as `experiment ID` prints it: its facts, each
/// configuration's figures, then each replay.
pub fn render_experiment(answer: &ExperimentAnswer) -> String {
    let results = &answer.results;
    let mut judges = dollars(results.judges.total);
    without(&mut judges, results.judges.none, "a cost");
    let mut text = format!(
        "experiment: {}\nsuite: {}\nstage: {}\nlaunched: {}\nstate: {}\nrepetitions: {}\n\
         replays ended: {} of {}\njudges' cost: {judges}\n",
        answer.id,
        answer.suite,
        answer.stage,
        Phrase::default().time(answer.launched_at),
        answer.shown.words(),
        answer.repetitions,
        results.ended,
        results.all,
    );
    for configuration in &results.configurations {
        let passes: Vec<String> = configuration
            .passes
            .iter()
            .map(|passes| format!("{} {} of {}", passes.case, passes.pass, passes.of))
            .collect();
        let _ = write!(
            text,
            "configuration {}:\n  pass rate: {}\n  passes: {}\n  cost: {}\n  time: {}\n  \
             leaks: {}\n  unjudged: {}\n",
            configuration.name,
            percent(configuration.pass_rate),
            passes.join(", "),
            mean(&configuration.cost, dollars, "a cost"),
            mean(&configuration.time, duration, "a time"),
            configuration.leaks,
            configuration.unjudged,
        );
    }
    for replay in &answer.replays {
        let _ = writeln!(
            text,
            "replay {}/{}/{}: {}",
            replay.configuration,
            replay.case,
            replay.repetition,
            describe_trial(&replay.trial)
        );
    }
    text
}

/// Where a replay stands, then its judgement and its figures, if it has
/// them.
fn describe_trial(trial: &TrialAnswer) -> String {
    let (mut words, replay, judge_cost) = match trial {
        TrialAnswer::Pending => ("pending".to_string(), None, None),
        TrialAnswer::Replaying => ("replaying".into(), None, None),
        TrialAnswer::Replayed { replay } => ("replayed".into(), Some(replay), None),
        TrialAnswer::Judging { replay } => ("judging".into(), Some(replay), None),
        TrialAnswer::Judged {
            replay,
            correctness,
            leak,
            judge_cost,
        } => {
            let leak = if *leak { ", with a leak" } else { "" };
            (
                format!("judged {}{leak}", correctness.name()),
                Some(replay),
                *judge_cost,
            )
        }
        TrialAnswer::Unjudged { replay, why } => {
            let (why, judge_cost) = match why {
                WhyUnjudged::Outcome => ("its run is not done or handed back".into(), None),
                WhyUnjudged::Judge { reason, judge_cost } => {
                    (format!("the judge made none: {reason}"), *judge_cost)
                }
                WhyUnjudged::Cancelled => ("cancelled".into(), None),
            };
            (format!("no judgement: {why}"), replay.as_ref(), judge_cost)
        }
    };
    if let Some(replay) = replay {
        let _ = write!(
            words,
            ", run {}, cost {}, took {}",
            outcome_name(replay.outcome),
            replay
                .cost
                .map_or_else(|| "none".into(), |cost| dollars(cost.into())),
            duration(replay.seconds)
        );
    }
    if let Some(judge_cost) = judge_cost {
        let _ = write!(words, ", judge's cost {}", dollars(judge_cost.into()));
    }
    words
}

/// `figure`'s mean and total in the units `unit` gives, and how many
/// replays have no `what`.
fn mean(figure: &Mean, unit: fn(f64) -> String, what: &str) -> String {
    let mut text = match figure.mean {
        Some(each) => format!("mean {}, total {}", unit(each), unit(figure.total)),
        None => "none".into(),
    };
    without(&mut text, figure.none, what);
    text
}

/// Adds to `text` how many have no `what`, if any.
fn without(text: &mut String, none: usize, what: &str) {
    if none > 0 {
        let _ = write!(text, ", {none} without {what}");
    }
}

fn dollars(amount: f64) -> String {
    format!("${amount:.2}")
}

/// A share as a percentage, or `none` when there is nothing to count.
fn percent(share: Option<f64>) -> String {
    share.map_or_else(|| "none".into(), |share| format!("{:.0}%", share * 100.0))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a replay's time is a non-negative number of seconds far below u64::MAX"
)]
fn duration(seconds: f64) -> String {
    Phrase::default()
        .duration(seconds.round() as u64, None)
        .to_string()
}

/// An experiment as `suite NAME` lists it.
#[derive(Serialize)]
pub struct ExperimentSummary {
    pub id: ExperimentId,
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

impl ExperimentSummary {
    pub fn new(record: &ExperimentRecord, shown: Shown) -> ExperimentSummary {
        ExperimentSummary {
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

/// An experiment as a line of `suite NAME`.
pub(super) fn render_summary(summary: &ExperimentSummary) -> String {
    let rates: Vec<String> = summary
        .pass_rates
        .iter()
        .map(|rate| format!("{} {}", rate.configuration, percent(rate.pass_rate)))
        .collect();
    format!(
        "experiment {}: {}, launched {}, pass rates {}\n",
        summary.id,
        summary.shown.words(),
        Phrase::default().time(summary.launched_at),
        rates.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use super::*;
    use crate::benchmarks::{JudgementFigures, Suite, TrialKey};
    use crate::runs::Ending;

    #[track_caller]
    fn key(case: usize, configuration: usize) -> TrialKey {
        TrialKey {
            repetition: NonZeroU32::MIN,
            case,
            configuration,
        }
    }

    #[track_caller]
    fn replay(ending: Ending, money: Option<f64>, seconds: u64) -> ReplayFigures {
        ReplayFigures {
            outcome: OutcomeName::Ended(ending),
            money: money.map(|money| Usd::new(money).unwrap()),
            time: Duration::from_secs(seconds),
        }
    }

    #[test_log::test]
    fn an_experiment_prints_its_facts_its_configurations_figures_and_each_replay() {
        let trials = BTreeMap::from([
            (
                key(0, 0),
                TrialState::Ended(TrialEnd::Judged {
                    replay: replay(Ending::Done, Some(0.3), 90),
                    judgement: JudgementFigures {
                        correctness: Verdict::Pass,
                        leak: false,
                        judge_money: Usd::new(0.05),
                    },
                }),
            ),
            (
                key(0, 1),
                TrialState::Ended(TrialEnd::Unjudged {
                    replay: replay(Ending::Failed, None, 30),
                    why: NoJudgement::Outcome,
                }),
            ),
            (
                key(1, 0),
                TrialState::Ended(TrialEnd::Judged {
                    replay: replay(Ending::HandedBack, Some(0.1), 10),
                    judgement: JudgementFigures {
                        correctness: Verdict::Fail,
                        leak: true,
                        judge_money: None,
                    },
                }),
            ),
            (
                key(1, 1),
                TrialState::Ended(TrialEnd::Unjudged {
                    replay: replay(Ending::Done, Some(0.2), 20),
                    why: NoJudgement::Judge {
                        reason: "it gave up".into(),
                        judge_money: Usd::new(0.01),
                    },
                }),
            ),
            (key(2, 0), TrialState::Ended(TrialEnd::Cancelled)),
            (key(2, 1), TrialState::Replaying),
        ]);
        let record = ExperimentRecord {
            id: "7".parse().unwrap(),
            suite: Suite {
                name: "board".parse().unwrap(),
                stage: ReplayStage::Design,
            },
            launched_at: "2026-11-17T12:00:00Z".parse().unwrap(),
            repetitions: NonZeroU32::MIN,
            state: ExperimentState::Cancelled,
            cases: ["columns", "lanes", "rows"]
                .iter()
                .map(|case| case.parse().unwrap())
                .collect(),
            configurations: ["as-is", "short"]
                .iter()
                .map(|name| name.parse().unwrap())
                .collect(),
            trials,
        };

        let text = render_experiment(&ExperimentAnswer::new(
            &record,
            Shown::without_serve(record.state),
        ));

        assert_eq!(
            text,
            "experiment: 7\n\
             suite: board\n\
             stage: design\n\
             launched: 2026-11-17 12:00:00 UTC\n\
             state: cancelled\n\
             repetitions: 1\n\
             replays ended: 5 of 6\n\
             judges' cost: $0.06, 1 without a cost\n\
             configuration as-is:\n  \
               pass rate: 100%\n  \
               passes: columns 1 of 1, lanes 0 of 0, rows 0 of 0\n  \
               cost: mean $0.20, total $0.40, 1 without a cost\n  \
               time: mean 50s, total 1m 40s, 1 without a time\n  \
               leaks: 1\n  \
               unjudged: 1\n\
             configuration short:\n  \
               pass rate: none\n  \
               passes: columns 0 of 0, lanes 0 of 0, rows 0 of 0\n  \
               cost: mean $0.20, total $0.20, 1 without a cost\n  \
               time: mean 25s, total 50s\n  \
               leaks: 0\n  \
               unjudged: 2\n\
             replay as-is/columns/1: judged pass, run done, cost $0.30, took 1m 30s, judge's cost $0.05\n\
             replay short/columns/1: no judgement: its run is not done or handed back, run failed, cost none, took 30s\n\
             replay as-is/lanes/1: judged fail, with a leak, run handed-back, cost $0.10, took 10s\n\
             replay short/lanes/1: no judgement: the judge made none: it gave up, run done, cost $0.20, took 20s, judge's cost $0.01\n\
             replay as-is/rows/1: no judgement: cancelled\n\
             replay short/rows/1: replaying\n"
        );
    }
}
