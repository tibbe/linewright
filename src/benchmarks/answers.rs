//! Experiments as the commands answer them (`docs/specs/benchmarks.md`,
//! Suites and experiments on the command line).

use std::fmt::Write as _;
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::case::{CaseName, ReplayStage};
use crate::cost::Usd;
use crate::domain::Timestamp;
use crate::holds::{ExperimentHold, Hold};
use crate::phrase::Phrase;
use crate::reconcile::Wait;
use crate::results::Verdict;
use crate::runs::OutcomeName;
use crate::status::describe_wait;

use super::experiments::outcome_name;
use super::results::{self, Figures, Mean, Results};
use super::{
    ConfigurationName, ExperimentId, ExperimentRecord, ExperimentState, NoJudgement, Repetitions,
    ReplayFigures, SuiteName, TrialEnd, TrialState,
};

/// An experiment's state as it is shown: an open experiment runs or waits,
/// as `serve` alone knows, and waits for `serve` where it does not answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "state", content = "waiting_for")]
pub enum Shown {
    Running,
    Waiting(WaitingFor),
    Ended,
    Cancelled,
}

/// What a waiting experiment waits for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaitingFor {
    /// No `serve` runs it.
    Serve,
    /// A run of the pipeline waits for a place under `max-runs`.
    Pipeline,
    /// Replays and judges of experiments take every place under
    /// `max-replays`.
    FreeReplay {
        in_flight: usize,
        max_replays: usize,
    },
    Hold(ExperimentHold),
}

impl WaitingFor {
    /// What an experiment waits for, in words, such as "the usage limit,
    /// until 14:30".
    pub fn phrase(&self) -> Phrase {
        match self {
            WaitingFor::Serve => "serve".into(),
            WaitingFor::Pipeline => "the pipeline: a run waits for a place under max-runs".into(),
            WaitingFor::FreeReplay {
                in_flight,
                max_replays,
            } => format!("a free replay: {in_flight} of {max_replays} in flight").into(),
            WaitingFor::Hold(hold) => describe_wait(&Wait::from(Hold::from(*hold))),
        }
    }
}

impl Shown {
    /// The state of `state` where `serve` does not say how an open
    /// experiment stands.
    pub fn without_serve(state: ExperimentState) -> Shown {
        match state {
            ExperimentState::Open => Shown::Waiting(WaitingFor::Serve),
            ExperimentState::Ended { .. } => Shown::Ended,
            ExperimentState::Cancelled { .. } => Shown::Cancelled,
        }
    }

    /// The state of the experiment kept as `state`, given how `serve` shows
    /// it, if it does: what is kept wins once the experiment is not open.
    pub fn of(state: ExperimentState, served: Option<Shown>) -> Shown {
        match (state, served) {
            (ExperimentState::Open, Some(shown)) => shown,
            (state, _) => Shown::without_serve(state),
        }
    }

    /// The state in words, with what a waiting experiment waits for.
    pub fn phrase(&self) -> Phrase {
        match self {
            Shown::Running => "running".into(),
            Shown::Waiting(waiting_for) => Phrase::from("waiting for ").then(waiting_for.phrase()),
            Shown::Ended => "ended".into(),
            Shown::Cancelled => "cancelled".into(),
        }
    }
}

/// What `experiment show ID` answers.
#[derive(Serialize)]
pub struct ExperimentAnswer {
    pub id: ExperimentId,
    pub suite: SuiteName,
    pub stage: ReplayStage,
    pub launched_at: Timestamp,
    /// When it ended or was cancelled, if it has and that was kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
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
            ended_at: record.state.ended_at(),
            shown,
            repetitions: record.repetitions,
            cases: record.cases.clone(),
            configurations: record.configurations.clone(),
            replays,
            results: results::results(record),
        }
    }
}

/// An experiment as `experiment show ID` prints it: its facts, each
/// configuration's figures, then each replay.
pub fn render_experiment(answer: &ExperimentAnswer) -> String {
    let results = &answer.results;
    let mut cost = dollars(results.cost.total);
    without(&mut cost, results.cost.none, "a cost");
    let mut judges = dollars(results.judges.total);
    without(&mut judges, results.judges.none, "a cost");
    let mut text = format!(
        "experiment: {}\nsuite: {}\nstage: {}\nlaunched: {}\n{}state: {}\nrepetitions: {}\n\
         replays ended: {} of {}\ncost: {cost}\njudges' cost: {judges}\n",
        answer.id,
        answer.suite,
        answer.stage,
        Phrase::default().time(answer.launched_at),
        answer.ended_at.map_or_else(String::new, |at| format!(
            "ended: {}\n",
            Phrase::default().time(at)
        )),
        answer.shown.phrase(),
        answer.repetitions,
        results.replays_ended,
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
        for case in &configuration.cases {
            let _ = write!(
                text,
                "  case {}:\n    passes: {}\n{}",
                case.case,
                case.passes
                    .map_or_else(|| "none".into(), |passes| passes.to_string()),
                describe_figures(&case.figures, "    "),
            );
        }
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

/// The lines of `figures`, each indented by `indent`.
fn describe_figures(figures: &Figures, indent: &str) -> String {
    format!(
        "{indent}cost: {}\n{indent}time: {}\n{indent}leaks: {}\n{indent}unjudged: {}\n",
        mean(&figures.cost, dollars, "a cost"),
        mean(&figures.time, duration, "a time"),
        figures.leaks,
        figures.unjudged,
    )
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
    reason = "a replay's time is a non-negative number of seconds far below u64::MAX"
)]
pub fn duration(seconds: f64) -> String {
    Phrase::default()
        .duration(seconds.round() as u64, None)
        .to_string()
}

/// An experiment as `suite show NAME` lists it.
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

/// An experiment as a line of `suite show NAME`.
pub(super) fn render_summary(summary: &ExperimentSummary) -> String {
    let rates: Vec<String> = summary
        .pass_rates
        .iter()
        .map(|rate| format!("{} {}", rate.configuration, percent(rate.pass_rate)))
        .collect();
    format!(
        "experiment {}: {}, launched {}, pass rates {}\n",
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
            state: ExperimentState::Cancelled {
                at: Some("2026-11-17T12:30:00Z".parse().unwrap()),
            },
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
             ended: 2026-11-17 12:30:00 UTC\n\
             state: cancelled\n\
             repetitions: 1\n\
             replays ended: 4 of 6\n\
             cost: $0.66, 1 without a cost\n\
             judges' cost: $0.06, 1 without a cost\n\
             configuration as-is:\n  \
               pass rate: 100%\n  \
               cost: mean $0.20, total $0.40\n  \
               time: mean 50s, total 1m 40s\n  \
               leaks: 1\n  \
               unjudged: 0\n  \
               case columns:\n    \
                 passes: 1 of 1\n    \
                 cost: mean $0.30, total $0.30\n    \
                 time: mean 1m 30s, total 1m 30s\n    \
                 leaks: 0\n    \
                 unjudged: 0\n  \
               case lanes:\n    \
                 passes: none\n    \
                 cost: mean $0.10, total $0.10\n    \
                 time: mean 10s, total 10s\n    \
                 leaks: 1\n    \
                 unjudged: 0\n  \
               case rows:\n    \
                 passes: none\n    \
                 cost: none\n    \
                 time: none\n    \
                 leaks: 0\n    \
                 unjudged: 0\n\
             configuration short:\n  \
               pass rate: none\n  \
               cost: mean $0.20, total $0.20, 1 without a cost\n  \
               time: mean 25s, total 50s\n  \
               leaks: 0\n  \
               unjudged: 2\n  \
               case columns:\n    \
                 passes: none\n    \
                 cost: none, 1 without a cost\n    \
                 time: mean 30s, total 30s\n    \
                 leaks: 0\n    \
                 unjudged: 1\n  \
               case lanes:\n    \
                 passes: none\n    \
                 cost: mean $0.20, total $0.20\n    \
                 time: mean 20s, total 20s\n    \
                 leaks: 0\n    \
                 unjudged: 1\n  \
               case rows:\n    \
                 passes: none\n    \
                 cost: none\n    \
                 time: none\n    \
                 leaks: 0\n    \
                 unjudged: 0\n\
             replay as-is/columns/1: judged pass, run done, cost $0.30, took 1m 30s, judge's cost $0.05\n\
             replay short/columns/1: no judgement: its run is not done or handed back, run failed, cost none, took 30s\n\
             replay as-is/lanes/1: judged fail, with a leak, run handed-back, cost $0.10, took 10s\n\
             replay short/lanes/1: no judgement: the judge made none: it gave up, run done, cost $0.20, took 20s, judge's cost $0.01\n\
             replay as-is/rows/1: no judgement: cancelled\n\
             replay short/rows/1: replaying\n"
        );
    }

    #[test_log::test]
    fn an_experiment_says_what_it_waits_for() {
        let record = ExperimentRecord {
            id: "3".parse().unwrap(),
            suite: Suite {
                name: "board".parse().unwrap(),
                stage: ReplayStage::Design,
            },
            launched_at: "2026-11-17T12:00:00Z".parse().unwrap(),
            repetitions: NonZeroU32::MIN,
            state: ExperimentState::Open,
            cases: vec!["columns".parse().unwrap()],
            configurations: vec!["as-is".parse().unwrap()],
            trials: BTreeMap::from([(key(0, 0), TrialState::Pending)]),
        };
        let line = |shown| render_summary(&ExperimentSummary::new(&record, shown));

        assert_eq!(
            [
                Shown::Running,
                Shown::Waiting(WaitingFor::Serve),
                Shown::Waiting(WaitingFor::Pipeline),
                Shown::Waiting(WaitingFor::FreeReplay {
                    in_flight: 1,
                    max_replays: 1
                }),
                Shown::Waiting(WaitingFor::Hold(ExperimentHold::UsageLimit {
                    resets_at: "2026-11-17T14:30:00Z".parse().unwrap()
                })),
            ]
            .map(line),
            [
                "experiment 3: running, launched 2026-11-17 12:00:00 UTC, pass rates as-is none\n",
                "experiment 3: waiting for serve, launched 2026-11-17 12:00:00 UTC, pass rates as-is none\n",
                "experiment 3: waiting for the pipeline: a run waits for a place under max-runs, \
                 launched 2026-11-17 12:00:00 UTC, pass rates as-is none\n",
                "experiment 3: waiting for a free replay: 1 of 1 in flight, \
                 launched 2026-11-17 12:00:00 UTC, pass rates as-is none\n",
                "experiment 3: waiting for the usage limit, until 2026-11-17 14:30:00 UTC, \
                 launched 2026-11-17 12:00:00 UTC, pass rates as-is none\n",
            ]
        );
    }

    #[test_log::test]
    fn an_experiment_not_open_shows_as_it_is_kept_whatever_serve_last_showed() {
        assert_eq!(
            Shown::of(
                ExperimentState::Cancelled { at: None },
                Some(Shown::Running)
            ),
            Shown::Cancelled
        );
        assert_eq!(
            Shown::of(ExperimentState::Open, Some(Shown::Running)),
            Shown::Running
        );
        assert_eq!(
            Shown::of(ExperimentState::Open, None),
            Shown::Waiting(WaitingFor::Serve)
        );
    }
}
