//! An experiment's results (`docs/specs/benchmarks.md`, Results): figures of
//! the trials that have ended, worked out on each read.

use serde::Serialize;

use crate::case::CaseName;
use crate::results::Verdict;

use super::{
    ConfigurationName, ExperimentRecord, NoJudgement, ReplayFigures, TrialEnd, TrialState,
};

/// An experiment's results.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Results {
    /// Each configuration's figures, in the experiment's order.
    pub configurations: Vec<ConfigurationResults>,
    /// The cost of the judges, in US dollars, apart from the
    /// configurations'.
    pub judges: Total,
    /// How many of its replays have ended.
    pub ended: usize,
    /// How many replays it makes.
    pub all: usize,
}

/// A configuration's figures.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConfigurationResults {
    pub name: ConfigurationName,
    /// Of its replays with a judgement and no leak, the share that pass;
    /// `None` with no such replay.
    pub pass_rate: Option<f64>,
    /// For each case, in the experiment's order.
    pub passes: Vec<CasePasses>,
    /// In US dollars, of the replays that have a cost.
    pub cost: Mean,
    /// In seconds.
    pub time: Mean,
    /// How many of its replays have a judgement with a leak.
    pub leaks: usize,
    /// How many of its replays have no judgement.
    pub unjudged: usize,
}

/// How many of a configuration's replays of a case pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CasePasses {
    pub case: CaseName,
    pub pass: usize,
    /// Of those with a judgement and no leak.
    pub of: usize,
}

/// A figure's mean and total over the replays that have it, and how many
/// have none.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Mean {
    /// `None` when no replay has the figure.
    pub mean: Option<f64>,
    pub total: f64,
    pub none: usize,
}

/// A figure's total over what has it, and how many have none.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Total {
    pub total: f64,
    pub none: usize,
}

/// The results of `record`, of the trials that have ended.
pub fn results(record: &ExperimentRecord) -> Results {
    let mut judges = Sum::default();
    let mut configurations: Vec<Tally> = record
        .configurations
        .iter()
        .map(|_| Tally {
            passes: record
                .cases
                .iter()
                .map(|case| CasePasses {
                    case: case.clone(),
                    pass: 0,
                    of: 0,
                })
                .collect(),
            ..Tally::default()
        })
        .collect();
    let mut ended = 0;
    for (key, state) in &record.trials {
        let TrialState::Ended(end) = state else {
            continue;
        };
        ended += 1;
        let Some(tally) = configurations.get_mut(key.configuration) else {
            continue;
        };
        match end {
            TrialEnd::Judged { replay, judgement } => {
                tally.replay(Some(replay));
                judges.add(judgement.judge_money.map(f64::from));
                if judgement.leak {
                    tally.leaks += 1;
                } else if let Some(passes) = tally.passes.get_mut(key.case) {
                    passes.of += 1;
                    if judgement.correctness == Verdict::Pass {
                        passes.pass += 1;
                    }
                }
            }
            TrialEnd::Unjudged { replay, why } => {
                tally.replay(Some(replay));
                tally.unjudged += 1;
                if let NoJudgement::Judge { judge_money, .. } = why {
                    judges.add(judge_money.map(f64::from));
                }
            }
            TrialEnd::Cancelled => {
                tally.replay(None);
                tally.unjudged += 1;
            }
        }
    }
    Results {
        configurations: record
            .configurations
            .iter()
            .zip(configurations)
            .map(|(name, tally)| tally.results(name))
            .collect(),
        judges: Total {
            total: judges.total,
            none: judges.none,
        },
        ended,
        all: record.trials.len(),
    }
}

/// A configuration's figures as the trials are counted.
#[derive(Default)]
struct Tally {
    passes: Vec<CasePasses>,
    cost: Sum,
    time: Sum,
    leaks: usize,
    unjudged: usize,
}

impl Tally {
    /// Counts the cost and the time of a replay, which has neither without
    /// figures.
    fn replay(&mut self, replay: Option<&ReplayFigures>) {
        self.cost
            .add(replay.and_then(|replay| replay.money).map(f64::from));
        self.time
            .add(replay.map(|replay| replay.time.as_secs_f64()));
    }

    fn results(self, name: &ConfigurationName) -> ConfigurationResults {
        let pass = self.passes.iter().map(|passes| passes.pass).sum();
        let of = self.passes.iter().map(|passes| passes.of).sum();
        ConfigurationResults {
            name: name.clone(),
            pass_rate: share(pass, of),
            passes: self.passes,
            cost: self.cost.mean(),
            time: self.time.mean(),
            leaks: self.leaks,
            unjudged: self.unjudged,
        }
    }
}

/// A figure's sum as the replays are counted.
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
    reason = "an experiment counts far fewer replays than an f64 holds exactly"
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
        ExperimentId, ExperimentState, JudgementFigures, ReplayFigures, Suite, TrialKey,
    };
    use crate::case::ReplayStage;
    use crate::cost::Usd;
    use crate::runs::{Ending, OutcomeName};

    /// An experiment on the cases `cases` under `as-is` and `configurations`,
    /// with `trials` and every other trial pending.
    #[track_caller]
    fn experiment(
        cases: &[&str],
        configurations: &[&str],
        repetitions: u32,
        trials: Vec<(TrialKey, TrialState)>,
    ) -> ExperimentRecord {
        let mut all = BTreeMap::new();
        for repetition in 1..=repetitions {
            for case in 0..cases.len() {
                for configuration in 0..=configurations.len() {
                    all.insert(key(repetition, case, configuration), TrialState::Pending);
                }
            }
        }
        all.extend(trials);
        ExperimentRecord {
            id: "1".parse::<ExperimentId>().unwrap(),
            suite: Suite {
                name: "board".parse().unwrap(),
                stage: ReplayStage::Design,
            },
            launched_at: "2026-11-17T12:00:00Z".parse().unwrap(),
            repetitions: repetitions.try_into().unwrap(),
            state: ExperimentState::Open,
            cases: cases.iter().map(|case| case.parse().unwrap()).collect(),
            configurations: std::iter::once("as-is")
                .chain(configurations.iter().copied())
                .map(|name| name.parse().unwrap())
                .collect(),
            trials: all,
        }
    }

    #[track_caller]
    fn key(repetition: u32, case: usize, configuration: usize) -> TrialKey {
        TrialKey {
            repetition: repetition.try_into().unwrap(),
            case,
            configuration,
        }
    }

    #[track_caller]
    fn replay(money: Option<f64>, seconds: u64) -> ReplayFigures {
        ReplayFigures {
            outcome: OutcomeName::Ended(Ending::Done),
            money: money.map(|money| Usd::new(money).unwrap()),
            time: Duration::from_secs(seconds),
        }
    }

    #[track_caller]
    fn judged(
        replay: ReplayFigures,
        correctness: Verdict,
        leak: bool,
        judge_money: Option<f64>,
    ) -> TrialState {
        TrialState::Ended(TrialEnd::Judged {
            replay,
            judgement: JudgementFigures {
                correctness,
                leak,
                judge_money: judge_money.map(|money| Usd::new(money).unwrap()),
            },
        })
    }

    #[track_caller]
    fn passes(case: &str, pass: usize, of: usize) -> CasePasses {
        CasePasses {
            case: case.parse().unwrap(),
            pass,
            of,
        }
    }

    #[track_caller]
    fn approx(figure: Option<f64>, expected: f64) {
        let figure = figure.expect("a figure");
        assert!(
            (figure - expected).abs() < 1e-9,
            "{figure} is not {expected}"
        );
    }

    #[test_log::test]
    fn a_replay_with_a_leak_counts_as_a_leak_and_not_in_the_pass_rate() {
        let record = experiment(
            &["lanes"],
            &[],
            3,
            vec![
                (
                    key(1, 0, 0),
                    judged(replay(Some(0.1), 10), Verdict::Pass, false, None),
                ),
                (
                    key(2, 0, 0),
                    judged(replay(Some(0.1), 10), Verdict::Pass, true, None),
                ),
                (
                    key(3, 0, 0),
                    judged(replay(Some(0.1), 10), Verdict::Fail, false, None),
                ),
            ],
        );

        let results = results(&record);

        let as_is = &results.configurations[0];
        approx(as_is.pass_rate, 0.5);
        assert_eq!(as_is.passes, [passes("lanes", 1, 2)]);
        assert_eq!(as_is.leaks, 1);
        assert_eq!(as_is.unjudged, 0);
        assert_eq!((results.ended, results.all), (3, 3));
    }

    #[test_log::test]
    fn a_replay_with_no_cost_is_counted_apart_from_the_cost_and_still_in_the_time() {
        let record = experiment(
            &["columns", "lanes"],
            &[],
            1,
            vec![
                (
                    key(1, 0, 0),
                    judged(replay(Some(0.3), 10), Verdict::Pass, false, None),
                ),
                (
                    key(1, 1, 0),
                    TrialState::Ended(TrialEnd::Unjudged {
                        replay: ReplayFigures {
                            outcome: OutcomeName::Ended(Ending::Failed),
                            money: None,
                            time: Duration::from_secs(50),
                        },
                        why: NoJudgement::Outcome,
                    }),
                ),
            ],
        );

        let as_is = &results(&record).configurations[0];

        approx(as_is.cost.mean, 0.3);
        approx(Some(as_is.cost.total), 0.3);
        assert_eq!(as_is.cost.none, 1);
        approx(as_is.time.mean, 30.0);
        approx(Some(as_is.time.total), 60.0);
        assert_eq!(as_is.time.none, 0);
        approx(as_is.pass_rate, 1.0);
        assert_eq!(
            as_is.passes,
            [passes("columns", 1, 1), passes("lanes", 0, 0)]
        );
        assert_eq!(as_is.unjudged, 1);
    }

    #[test_log::test]
    fn with_nothing_to_count_a_pass_rate_and_a_mean_are_absent() {
        let record = experiment(
            &["lanes"],
            &["short"],
            1,
            vec![
                (key(1, 0, 0), TrialState::Replaying),
                (key(1, 0, 1), TrialState::Ended(TrialEnd::Cancelled)),
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
        assert_eq!(as_is.cost, empty);
        assert_eq!(as_is.time, empty);
        assert_eq!(as_is.unjudged, 0);
        let short = &results.configurations[1];
        assert_eq!(short.name.as_str(), "short");
        assert_eq!(short.pass_rate, None);
        assert_eq!(short.passes, [passes("lanes", 0, 0)]);
        assert_eq!(short.cost.mean, None);
        assert_eq!(short.cost.none, 1);
        assert_eq!(short.time.mean, None);
        assert_eq!(short.time.none, 1);
        assert_eq!(short.unjudged, 1);
        assert_eq!(
            results.judges,
            Total {
                total: 0.0,
                none: 0
            }
        );
        assert_eq!((results.ended, results.all), (1, 2));
    }

    #[test_log::test]
    fn the_judges_cost_is_apart_from_the_configurations_and_counts_a_judge_that_made_none() {
        let record = experiment(
            &["lanes"],
            &["short"],
            2,
            vec![
                (
                    key(1, 0, 0),
                    judged(replay(Some(1.0), 10), Verdict::Pass, false, Some(0.05)),
                ),
                (
                    key(1, 0, 1),
                    judged(replay(Some(2.0), 10), Verdict::Fail, false, Some(0.07)),
                ),
                (
                    key(2, 0, 0),
                    TrialState::Ended(TrialEnd::Unjudged {
                        replay: replay(Some(1.0), 10),
                        why: NoJudgement::Judge {
                            reason: "it gave up".into(),
                            judge_money: Usd::new(0.02),
                        },
                    }),
                ),
                (
                    key(2, 0, 1),
                    judged(replay(Some(2.0), 10), Verdict::Pass, false, None),
                ),
            ],
        );

        let results = results(&record);

        approx(Some(results.configurations[0].cost.total), 2.0);
        approx(Some(results.configurations[1].cost.total), 4.0);
        approx(Some(results.judges.total), 0.14);
        assert_eq!(results.judges.none, 1);
    }
}
