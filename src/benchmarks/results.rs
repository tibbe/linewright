//! An experiment's results (`docs/specs/benchmarks.md`, Results): figures of
//! the replays that have ended, worked out on each read.

use std::num::NonZeroUsize;

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
    /// How many of its replays have ended: those whose run has, whether or
    /// not their judge has.
    pub replays_ended: usize,
    /// How many replays it makes.
    pub all: usize,
    /// The cost of its replays and its judges, in US dollars, and how many of
    /// its replays have none.
    pub cost: Total,
}

/// A configuration's figures.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConfigurationResults {
    pub name: ConfigurationName,
    /// Of its replays with a judgement and no leak, the share that pass;
    /// `None` with no such replay.
    pub pass_rate: Option<f64>,
    #[serde(flatten)]
    pub figures: Figures,
    /// For each case, in the experiment's order.
    pub cases: Vec<CaseResults>,
}

/// The figures of a configuration's replays, or of its replays of a case.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Figures {
    /// In US dollars, of the replays that have a cost.
    pub cost: Mean,
    /// In seconds.
    pub time: Mean,
    /// How many of the replays have a judgement with a leak.
    pub leaks: usize,
    /// How many of the replays have no judgement for a reason the spec
    /// gives: their run's outcome, their judge, or a cancel after their run.
    pub unjudged: usize,
}

/// A configuration's figures of its replays of a case.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CaseResults {
    pub case: CaseName,
    /// `None` with no replay with a judgement and no leak.
    pub passes: Option<Passes>,
    #[serde(flatten)]
    pub figures: Figures,
}

/// How many replays pass, of those with a judgement and no leak.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Passes {
    pub pass: usize,
    pub of: NonZeroUsize,
}

impl std::fmt::Display for Passes {
    /// As "2 of 3" (`docs/specs/benchmarks.md`, Experiment, The table).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} of {}", self.pass, self.of)
    }
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

/// The results of `record`, of the replays that have ended.
pub fn results(record: &ExperimentRecord) -> Results {
    let mut judges = Sum::default();
    let mut tallies: Vec<Vec<Tally>> = record
        .configurations
        .iter()
        .map(|_| record.cases.iter().map(|_| Tally::default()).collect())
        .collect();
    for (key, state) in &record.trials {
        let Some(tally) = tallies
            .get_mut(key.configuration)
            .and_then(|cases| cases.get_mut(key.case))
        else {
            continue;
        };
        match state {
            TrialState::Pending
            | TrialState::Replaying
            | TrialState::Ended(TrialEnd::Cancelled) => {}
            TrialState::Replayed { replay } | TrialState::Judging { replay } => {
                tally.replay(replay);
            }
            TrialState::Ended(TrialEnd::Judged { replay, judgement }) => {
                tally.replay(replay);
                judges.add(judgement.judge_money.map(f64::from));
                if judgement.leak {
                    tally.leaks += 1;
                } else {
                    tally.of += 1;
                    if judgement.correctness == Verdict::Pass {
                        tally.pass += 1;
                    }
                }
            }
            TrialState::Ended(TrialEnd::Unjudged { replay, why }) => {
                tally.replay(replay);
                tally.unjudged += 1;
                if let NoJudgement::Judge { judge_money, .. } = why {
                    judges.add(judge_money.map(f64::from));
                }
            }
        }
    }
    let replays_ended = tallies.iter().flatten().map(|tally| tally.ended).sum();
    let configurations: Vec<ConfigurationResults> = record
        .configurations
        .iter()
        .zip(tallies)
        .map(|(name, cases)| {
            let all = cases.iter().fold(Tally::default(), Tally::and);
            ConfigurationResults {
                name: name.clone(),
                pass_rate: share(all.pass, all.of),
                figures: all.figures(),
                cases: record
                    .cases
                    .iter()
                    .zip(cases)
                    .map(|(case, tally)| CaseResults {
                        case: case.clone(),
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
    let (replays_cost, without_cost) =
        configurations
            .iter()
            .fold((0.0, 0), |(total, none), configuration| {
                let cost = &configuration.figures.cost;
                (total + cost.total, none + cost.none)
            });
    Results {
        configurations,
        cost: Total {
            total: replays_cost + judges.total,
            none: without_cost,
        },
        judges: Total {
            total: judges.total,
            none: judges.none,
        },
        replays_ended,
        all: record.trials.len(),
    }
}

/// The figures of a configuration's replays of a case as they are counted.
#[derive(Default)]
struct Tally {
    /// How many of the replays have ended.
    ended: usize,
    pass: usize,
    of: usize,
    cost: Sum,
    time: Sum,
    leaks: usize,
    unjudged: usize,
}

impl Tally {
    /// Counts a replay that ended, with its cost and its time.
    fn replay(&mut self, replay: &ReplayFigures) {
        self.ended += 1;
        self.cost.add(replay.money.map(f64::from));
        self.time.add(Some(replay.time.as_secs_f64()));
    }

    /// The tally of the replays of both.
    fn and(self, other: &Tally) -> Tally {
        Tally {
            ended: self.ended + other.ended,
            pass: self.pass + other.pass,
            of: self.of + other.of,
            cost: self.cost.and(&other.cost),
            time: self.time.and(&other.time),
            leaks: self.leaks + other.leaks,
            unjudged: self.unjudged + other.unjudged,
        }
    }

    fn figures(&self) -> Figures {
        Figures {
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
    fn passes(pass: usize, of: usize) -> Passes {
        Passes {
            pass,
            of: NonZeroUsize::new(of).unwrap(),
        }
    }

    fn unjudged(replay: ReplayFigures, why: NoJudgement) -> TrialState {
        TrialState::Ended(TrialEnd::Unjudged { replay, why })
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
        assert_eq!(as_is.cases[0].passes, Some(passes(1, 2)));
        assert_eq!(as_is.cases[0].figures.leaks, 1);
        assert_eq!(as_is.figures.leaks, 1);
        assert_eq!(as_is.figures.unjudged, 0);
        assert_eq!((results.replays_ended, results.all), (3, 3));
    }

    #[test_log::test]
    fn each_case_has_the_figures_of_a_configuration_of_its_own_replays() {
        let record = experiment(
            &["columns", "lanes"],
            &[],
            4,
            vec![
                (
                    key(1, 0, 0),
                    judged(replay(Some(0.5), 60), Verdict::Pass, false, None),
                ),
                (
                    key(2, 0, 0),
                    judged(replay(None, 30), Verdict::Pass, false, None),
                ),
                (
                    key(3, 0, 0),
                    judged(replay(Some(0.25), 90), Verdict::Fail, false, None),
                ),
                (
                    key(4, 0, 0),
                    judged(replay(Some(0.75), 60), Verdict::Pass, true, None),
                ),
                (
                    key(1, 1, 0),
                    judged(replay(Some(1.0), 100), Verdict::Pass, false, None),
                ),
                (
                    key(2, 1, 0),
                    unjudged(replay(Some(2.0), 200), NoJudgement::Outcome),
                ),
            ],
        );

        let as_is = &results(&record).configurations[0];

        let columns = &as_is.cases[0];
        assert_eq!(columns.case.as_str(), "columns");
        assert_eq!(columns.passes, Some(passes(2, 3)));
        approx(columns.figures.cost.mean, 0.5);
        approx(Some(columns.figures.cost.total), 1.5);
        assert_eq!(columns.figures.cost.none, 1);
        approx(columns.figures.time.mean, 60.0);
        approx(Some(columns.figures.time.total), 240.0);
        assert_eq!((columns.figures.leaks, columns.figures.unjudged), (1, 0));
        let lanes = &as_is.cases[1];
        assert_eq!(lanes.passes, Some(passes(1, 1)));
        approx(lanes.figures.cost.mean, 1.5);
        approx(Some(lanes.figures.cost.total), 3.0);
        assert_eq!(lanes.figures.cost.none, 0);
        approx(Some(lanes.figures.time.total), 300.0);
        assert_eq!((lanes.figures.leaks, lanes.figures.unjudged), (0, 1));
        approx(as_is.pass_rate, 0.75);
        approx(as_is.figures.cost.mean, 0.9);
        approx(Some(as_is.figures.cost.total), 4.5);
        assert_eq!(as_is.figures.cost.none, 1);
        approx(Some(as_is.figures.time.total), 540.0);
        assert_eq!((as_is.figures.leaks, as_is.figures.unjudged), (1, 1));
    }

    #[test_log::test]
    fn with_nothing_to_count_the_passes_of_a_case_a_pass_rate_and_a_mean_are_absent() {
        let record = experiment(
            &["lanes"],
            &["short"],
            1,
            vec![
                (key(1, 0, 0), TrialState::Replaying),
                (
                    key(1, 0, 1),
                    unjudged(replay(None, 10), NoJudgement::Outcome),
                ),
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
        assert_eq!(as_is.cases[0].passes, None);
        assert_eq!(as_is.figures.cost, empty);
        assert_eq!(as_is.figures.time, empty);
        assert_eq!(as_is.cases[0].figures.cost, empty);
        let short = &results.configurations[1];
        assert_eq!(short.name.as_str(), "short");
        assert_eq!(short.pass_rate, None);
        assert_eq!(short.cases[0].passes, None);
        assert_eq!(short.figures.cost.mean, None);
        assert_eq!(short.figures.cost.none, 1);
        assert_eq!(short.figures.unjudged, 1);
    }

    #[test_log::test]
    fn a_replay_has_ended_once_its_run_has_whether_or_not_its_judge_has_and_not_when_cancelled_before()
     {
        let record = experiment(
            &["columns", "lanes", "rows"],
            &["short"],
            1,
            vec![
                (key(1, 0, 0), TrialState::Replaying),
                (
                    key(1, 0, 1),
                    TrialState::Replayed {
                        replay: replay(Some(1.0), 10),
                    },
                ),
                (
                    key(1, 1, 0),
                    TrialState::Judging {
                        replay: replay(Some(2.0), 20),
                    },
                ),
                (
                    key(1, 1, 1),
                    judged(replay(Some(4.0), 40), Verdict::Pass, false, None),
                ),
                (key(1, 2, 0), TrialState::Ended(TrialEnd::Cancelled)),
            ],
        );

        let results = results(&record);

        assert_eq!((results.replays_ended, results.all), (3, 6));
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
        assert_eq!(as_is.figures.unjudged, 0);
    }

    #[test_log::test]
    fn unjudged_counts_the_replays_without_a_judgement_for_a_reason_the_spec_gives() {
        let record = experiment(
            &["columns"],
            &[],
            5,
            vec![
                (
                    key(1, 0, 0),
                    unjudged(replay(Some(0.1), 10), NoJudgement::Outcome),
                ),
                (
                    key(2, 0, 0),
                    unjudged(
                        replay(Some(0.1), 10),
                        NoJudgement::Judge {
                            reason: "it gave up".into(),
                            judge_money: None,
                        },
                    ),
                ),
                (
                    key(3, 0, 0),
                    unjudged(replay(Some(0.1), 10), NoJudgement::Cancelled),
                ),
                (key(4, 0, 0), TrialState::Ended(TrialEnd::Cancelled)),
            ],
        );

        let as_is = &results(&record).configurations[0];

        assert_eq!(as_is.figures.unjudged, 3);
        assert_eq!(as_is.cases[0].figures.unjudged, 3);
    }

    #[test_log::test]
    fn the_experiments_cost_sums_its_replays_and_its_judges_and_counts_the_replays_with_none() {
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
                    judged(replay(None, 10), Verdict::Fail, false, Some(0.07)),
                ),
                (
                    key(2, 0, 0),
                    unjudged(
                        replay(Some(1.0), 10),
                        NoJudgement::Judge {
                            reason: "it gave up".into(),
                            judge_money: Usd::new(0.02),
                        },
                    ),
                ),
                (
                    key(2, 0, 1),
                    judged(replay(Some(2.0), 10), Verdict::Pass, false, None),
                ),
            ],
        );

        let results = results(&record);

        approx(Some(results.configurations[0].figures.cost.total), 2.0);
        approx(Some(results.configurations[1].figures.cost.total), 2.0);
        approx(Some(results.judges.total), 0.14);
        assert_eq!(results.judges.none, 1);
        approx(Some(results.cost.total), 4.14);
        assert_eq!(results.cost.none, 1);
    }
}
