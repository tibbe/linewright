//! What sessions and runs cost: their time, their tokens for each model and
//! the harness's estimate of what those cost (`docs/specs/runs.md`, Cost).

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A count of each of the four kinds of tokens Linewright counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

impl Tokens {
    /// `None` past what a `u64` counts.
    fn checked_add(self, other: Tokens) -> Option<Tokens> {
        Some(Tokens {
            input: self.input.checked_add(other.input)?,
            output: self.output.checked_add(other.output)?,
            cache_write: self.cache_write.checked_add(other.cache_write)?,
            cache_read: self.cache_read.checked_add(other.cache_read)?,
        })
    }
}

/// A finite, non-negative amount in US dollars, as the harness estimates it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(into = "f64", try_from = "f64")]
pub struct Usd(f64);

impl Usd {
    pub const ZERO: Usd = Usd(0.0);

    /// `None` unless `dollars` is finite and not negative.
    pub fn new(dollars: f64) -> Option<Usd> {
        (dollars.is_finite() && dollars >= 0.0).then_some(Usd(dollars))
    }

    pub fn dollars(self) -> f64 {
        self.0
    }

    fn checked_add(self, other: Usd) -> Option<Usd> {
        Usd::new(self.0 + other.0)
    }
}

impl From<Usd> for f64 {
    fn from(usd: Usd) -> f64 {
        usd.0
    }
}

impl TryFrom<f64> for Usd {
    type Error = String;

    fn try_from(dollars: f64) -> Result<Usd, String> {
        Usd::new(dollars).ok_or_else(|| format!("{dollars} is not an amount of dollars"))
    }
}

/// What one model used, or a sum of it. `None` where a figure is absent:
/// the harness left out a kind of tokens or could not price the model, or
/// a part of the sum was absent.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spend {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Tokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd: Option<Usd>,
}

impl Spend {
    /// No tokens, which cost nothing.
    pub const ZERO: Spend = Spend {
        tokens: Some(Tokens {
            input: 0,
            output: 0,
            cache_write: 0,
            cache_read: 0,
        }),
        usd: Some(Usd::ZERO),
    };

    /// The sum of the two, each figure absent where either's is.
    #[must_use]
    pub fn plus(self, other: Spend) -> Spend {
        Spend {
            tokens: self
                .tokens
                .zip(other.tokens)
                .and_then(|(a, b)| a.checked_add(b)),
            usd: self.usd.zip(other.usd).and_then(|(a, b)| a.checked_add(b)),
        }
    }
}

/// What a session used, by the model's name as the harness gives it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Figures(pub BTreeMap<String, Spend>);

impl Figures {
    /// The sum of the two, model by model.
    #[must_use]
    fn plus(mut self, other: &Figures) -> Figures {
        for (model, &spend) in &other.0 {
            let summed = match self.0.get(model) {
                Some(&had) => had.plus(spend),
                None => spend,
            };
            self.0.insert(model.clone(), summed);
        }
        self
    }
}

/// What the harness says of a turn's spend once the turn ends.
#[derive(Clone, Debug, PartialEq)]
pub enum TurnSpend {
    /// That turn's process alone.
    OfTurn(Figures),
    /// The session's so far, the turns before it included.
    OfSessionSoFar(Figures),
    /// The harness gave no figures.
    None,
}

/// A session's spend as its turns start and end.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionSpend {
    /// `None` once a turn gave no figures.
    so_far: Option<Figures>,
    in_flight: bool,
}

impl Default for SessionSpend {
    fn default() -> SessionSpend {
        SessionSpend::new()
    }
}

impl SessionSpend {
    /// No turn yet: no tokens.
    pub fn new() -> SessionSpend {
        SessionSpend {
            so_far: Some(Figures::default()),
            in_flight: false,
        }
    }

    pub fn turn_started(&mut self) {
        self.in_flight = true;
    }

    pub fn turn_ended(&mut self, spend: TurnSpend) {
        self.in_flight = false;
        self.so_far = match spend {
            TurnSpend::OfTurn(turn) => self.so_far.take().map(|so_far| so_far.plus(&turn)),
            TurnSpend::OfSessionSoFar(so_far) => self.so_far.take().map(|_| so_far),
            TurnSpend::None => None,
        };
    }

    /// `None` once a turn gave no figures, or while one is in flight.
    pub fn figures(&self) -> Option<&Figures> {
        if self.in_flight {
            None
        } else {
            self.so_far.as_ref()
        }
    }
}

/// A session's cost as its run's record keeps it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionCost {
    /// What the session timeout counted, once the session ended.
    #[serde(
        rename = "seconds",
        default,
        skip_serializing_if = "Option::is_none",
        with = "seconds"
    )]
    pub time: Option<Duration>,
    /// `None` when the harness gave no figures for it.
    #[serde(rename = "models", default, skip_serializing_if = "Option::is_none")]
    pub spend: Option<Figures>,
}

/// A run's sums of tokens and money.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunSpend {
    pub models: Figures,
    /// All the models'.
    pub all: Spend,
}

/// The cost of a run: the sum of its sessions'.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunCost {
    /// `None` when a session's is.
    #[serde(
        rename = "seconds",
        default,
        skip_serializing_if = "Option::is_none",
        with = "seconds"
    )]
    pub time: Option<Duration>,
    /// `None` when a session gave no figures.
    #[serde(flatten)]
    pub spend: Option<RunSpend>,
}

impl RunCost {
    /// The sum of `sessions`. No sessions: no time and no tokens.
    pub fn of<'a>(sessions: impl IntoIterator<Item = &'a SessionCost>) -> RunCost {
        let mut cost = RunCost {
            time: Some(Duration::ZERO),
            spend: Some(RunSpend {
                models: Figures::default(),
                all: Spend::ZERO,
            }),
        };
        for session in sessions {
            cost.time = cost
                .time
                .zip(session.time)
                .and_then(|(a, b)| a.checked_add(b));
            cost.spend = cost
                .spend
                .zip(session.spend.as_ref())
                .map(|(run, figures)| RunSpend {
                    models: run.models.plus(figures),
                    all: figures
                        .0
                        .values()
                        .fold(run.all, |all, &spend| all.plus(spend)),
                });
        }
        cost
    }
}

/// A [`Duration`] as a number of seconds.
mod seconds {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    #[expect(
        clippy::ref_option,
        reason = "serde's `with` passes the field by reference"
    )]
    pub(super) fn serialize<S: Serializer>(
        time: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match time {
            Some(time) => serializer.serialize_f64(time.as_secs_f64()),
            None => serializer.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        Option::<f64>::deserialize(deserializer)?
            .map(|seconds| Duration::try_from_secs_f64(seconds).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tokens(input: u64, output: u64, cache_write: u64, cache_read: u64) -> Tokens {
        Tokens {
            input,
            output,
            cache_write,
            cache_read,
        }
    }

    fn spent(tokens: Tokens, dollars: f64) -> Spend {
        Spend {
            tokens: Some(tokens),
            usd: Usd::new(dollars),
        }
    }

    fn figures(models: &[(&str, Spend)]) -> Figures {
        Figures(
            models
                .iter()
                .map(|(model, spend)| ((*model).to_string(), *spend))
                .collect(),
        )
    }

    fn session(seconds: Option<u64>, spend: Option<Figures>) -> SessionCost {
        SessionCost {
            time: seconds.map(Duration::from_secs),
            spend,
        }
    }

    #[test_log::test]
    fn a_session_with_no_turn_used_no_tokens() {
        assert_eq!(SessionSpend::new().figures(), Some(&Figures::default()));
    }

    #[test_log::test]
    fn a_follow_up_that_gives_its_own_figures_adds_them_to_the_sessions() {
        let mut spend = SessionSpend::new();
        spend.turn_started();
        spend.turn_ended(TurnSpend::OfTurn(figures(&[(
            "opus",
            spent(tokens(10, 100, 1000, 5000), 1.5),
        )])));
        spend.turn_started();
        spend.turn_ended(TurnSpend::OfTurn(figures(&[
            ("opus", spent(tokens(1, 20, 0, 6000), 0.5)),
            ("haiku", spent(tokens(300, 30, 0, 0), 0.25)),
        ])));

        assert_eq!(
            spend.figures(),
            Some(&figures(&[
                ("haiku", spent(tokens(300, 30, 0, 0), 0.25)),
                ("opus", spent(tokens(11, 120, 1000, 11000), 2.0)),
            ]))
        );
    }

    #[test_log::test]
    fn a_follow_up_that_gives_the_sessions_figures_so_far_replaces_the_earlier_ones() {
        let mut spend = SessionSpend::new();
        spend.turn_started();
        spend.turn_ended(TurnSpend::OfSessionSoFar(figures(&[(
            "opus",
            spent(tokens(10, 100, 1000, 5000), 1.5),
        )])));
        spend.turn_started();
        spend.turn_ended(TurnSpend::OfSessionSoFar(figures(&[(
            "opus",
            spent(tokens(11, 120, 1000, 11000), 2.0),
        )])));

        assert_eq!(
            spend.figures(),
            Some(&figures(&[(
                "opus",
                spent(tokens(11, 120, 1000, 11000), 2.0)
            )]))
        );
    }

    #[test_log::test]
    fn a_session_has_no_figures_while_a_turn_is_in_flight() {
        let mut spend = SessionSpend::new();
        spend.turn_started();
        spend.turn_ended(TurnSpend::OfTurn(figures(&[(
            "opus",
            spent(tokens(1, 1, 1, 1), 1.0),
        )])));
        spend.turn_started();

        assert_eq!(spend.figures(), None);
    }

    #[test_log::test]
    fn a_session_with_a_turn_that_gave_no_figures_has_none_from_then_on() {
        let mut spend = SessionSpend::new();
        spend.turn_started();
        spend.turn_ended(TurnSpend::None);
        spend.turn_started();
        spend.turn_ended(TurnSpend::OfSessionSoFar(figures(&[(
            "opus",
            spent(tokens(1, 1, 1, 1), 1.0),
        )])));

        assert_eq!(spend.figures(), None);
    }

    #[test_log::test]
    fn a_model_without_a_cost_keeps_its_tokens_across_turns() {
        let mut spend = SessionSpend::new();
        for _ in 0..2 {
            spend.turn_started();
            spend.turn_ended(TurnSpend::OfTurn(figures(&[(
                "local",
                Spend {
                    tokens: Some(tokens(5, 6, 7, 8)),
                    usd: None,
                },
            )])));
        }

        assert_eq!(
            spend.figures(),
            Some(&figures(&[(
                "local",
                Spend {
                    tokens: Some(tokens(10, 12, 14, 16)),
                    usd: None,
                }
            )]))
        );
    }

    #[test_log::test]
    fn a_runs_cost_adds_up_its_sessions_for_each_model_and_for_all() {
        let first = session(
            Some(60),
            Some(figures(&[
                ("opus", spent(tokens(10, 100, 1000, 5000), 1.5)),
                ("haiku", spent(tokens(300, 30, 0, 0), 0.25)),
            ])),
        );
        let second = session(
            Some(30),
            Some(figures(&[("opus", spent(tokens(1, 20, 0, 6000), 0.5))])),
        );

        let cost = RunCost::of([&first, &second]);

        assert_eq!(
            cost,
            RunCost {
                time: Some(Duration::from_secs(90)),
                spend: Some(RunSpend {
                    models: Figures(BTreeMap::from([
                        ("haiku".into(), spent(tokens(300, 30, 0, 0), 0.25)),
                        ("opus".into(), spent(tokens(11, 120, 1000, 11000), 2.0)),
                    ])),
                    all: spent(tokens(311, 150, 1000, 11000), 2.25),
                }),
            }
        );
    }

    #[test_log::test]
    fn a_run_without_a_session_took_no_time_and_used_no_tokens() {
        assert_eq!(
            RunCost::of([]),
            RunCost {
                time: Some(Duration::ZERO),
                spend: Some(RunSpend {
                    models: Figures::default(),
                    all: spent(tokens(0, 0, 0, 0), 0.0),
                }),
            }
        );
    }

    #[test_log::test]
    fn a_crashed_session_leaves_absent_every_sum_of_tokens_and_money_but_not_the_time() {
        let crashed = session(Some(10), None);
        let other = session(
            Some(20),
            Some(figures(&[("haiku", spent(tokens(5, 6, 7, 8), 0.5))])),
        );

        let cost = RunCost::of([&crashed, &other]);

        assert_eq!(
            cost,
            RunCost {
                time: Some(Duration::from_secs(30)),
                spend: None,
            }
        );
    }

    #[test_log::test]
    fn a_run_with_a_crashed_session_is_kept_with_its_time_alone() {
        let cost = RunCost::of([&session(Some(10), None)]);

        assert_eq!(
            serde_json::to_value(cost).unwrap(),
            json!({"seconds": 10.0})
        );
    }

    #[test_log::test]
    fn a_run_whose_sessions_gave_figures_is_kept_with_its_sums() {
        let cost = RunCost::of([&session(
            Some(10),
            Some(figures(&[(
                "local",
                Spend {
                    tokens: Some(tokens(1, 2, 3, 4)),
                    usd: None,
                },
            )])),
        )]);

        let local = json!({"tokens": {"input": 1, "output": 2, "cache_write": 3, "cache_read": 4}});
        assert_eq!(
            serde_json::to_value(cost).unwrap(),
            json!({"seconds": 10.0, "models": {"local": local}, "all": local})
        );
    }

    #[test_log::test]
    fn a_run_without_a_session_is_kept_with_no_models_and_nothing_spent() {
        assert_eq!(
            serde_json::to_value(RunCost::of([])).unwrap(),
            json!({
                "seconds": 0.0,
                "models": {},
                "all": {
                    "tokens": {"input": 0, "output": 0, "cache_write": 0, "cache_read": 0},
                    "usd": 0.0,
                },
            })
        );
    }

    #[test_log::test]
    fn a_model_the_harness_cannot_price_leaves_its_cost_and_all_the_models_absent() {
        let one = session(
            Some(10),
            Some(figures(&[
                (
                    "local",
                    Spend {
                        tokens: Some(tokens(1, 2, 3, 4)),
                        usd: None,
                    },
                ),
                ("opus", spent(tokens(10, 20, 30, 40), 1.0)),
            ])),
        );

        let cost = RunCost::of([&one]);

        assert_eq!(
            cost,
            RunCost {
                time: Some(Duration::from_secs(10)),
                spend: Some(RunSpend {
                    models: Figures(BTreeMap::from([
                        (
                            "local".into(),
                            Spend {
                                tokens: Some(tokens(1, 2, 3, 4)),
                                usd: None,
                            }
                        ),
                        ("opus".into(), spent(tokens(10, 20, 30, 40), 1.0)),
                    ])),
                    all: Spend {
                        tokens: Some(tokens(11, 22, 33, 44)),
                        usd: None,
                    },
                }),
            }
        );
    }

    #[test_log::test]
    fn a_session_without_a_time_leaves_the_runs_time_absent() {
        let ended = session(Some(10), Some(Figures::default()));
        let unended = session(None, Some(Figures::default()));

        assert_eq!(RunCost::of([&ended, &unended]).time, None);
    }

    #[test_log::test]
    fn a_cost_is_kept_with_its_absent_figures_left_out() {
        let kept = session(
            Some(90),
            Some(figures(&[
                ("opus", spent(tokens(1, 2, 3, 4), 1.5)),
                (
                    "local",
                    Spend {
                        tokens: Some(tokens(5, 6, 7, 8)),
                        usd: None,
                    },
                ),
            ])),
        );
        let crashed = session(None, None);

        let json = serde_json::to_value([&kept, &crashed]).unwrap();

        assert_eq!(
            json,
            json!([
                {
                    "seconds": 90.0,
                    "models": {
                        "local": {
                            "tokens": {"input": 5, "output": 6, "cache_write": 7, "cache_read": 8},
                        },
                        "opus": {
                            "tokens": {"input": 1, "output": 2, "cache_write": 3, "cache_read": 4},
                            "usd": 1.5,
                        },
                    },
                },
                {},
            ])
        );
        assert_eq!(
            serde_json::from_value::<Vec<SessionCost>>(json).unwrap(),
            [kept, crashed]
        );
    }

    #[test_log::test]
    fn an_amount_of_dollars_is_finite_and_not_negative() {
        assert_eq!(
            [-0.5, f64::NAN, f64::INFINITY, 0.0, 2.5].map(|d| Usd::new(d).map(Usd::dollars)),
            [None, None, None, Some(0.0), Some(2.5)]
        );
    }
}
