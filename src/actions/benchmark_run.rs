//! The action that shows a benchmark run (`docs/specs/benchmarks.md`,
//! Benchmark run), and what it shows.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use tracing::warn;

use super::benchmarks::{benchmark_run_record, trial_output_dir};
use crate::access::{Permit, Read};
use crate::api::Sources;
use crate::benchmarks::results::Results;
use crate::benchmarks::{
    BenchmarkOfRun, BenchmarkRunAnswer, BenchmarkRunId, BenchmarkRunRecord, BenchmarkRunState,
    ConfigurationAnswer, ConfigurationId, KeyedTrialAnswer, RunTask, Shown, TaskId, TrialShown,
    TrialState, keyed_trials,
};
use crate::domain::Timestamp;
use crate::phrase::Status;
use crate::results::Leak;
use crate::task::TaskStage;
use crate::trial::{self, scorer};

/// A benchmark run as its screen shows it, with the names `benchmark-run
/// show --json` gives the same facts.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct BenchmarkRunScreen {
    pub id: BenchmarkRunId,
    pub benchmark: BenchmarkOfRun,
    pub stage: TaskStage,
    pub launched_at: Timestamp,
    /// When it ended or was cancelled, if it has and that was kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    /// Its state, with what it waits for while it waits.
    #[serde(flatten)]
    pub shown: Shown,
    /// Whether it is kept as open, neither ended nor cancelled, whatever
    /// `serve` shows of it.
    #[serde(skip)]
    pub open: bool,
    /// From its launch to its end, or up to now while it is open; `None`
    /// where its end was not kept.
    pub duration_seconds: Option<u64>,
    pub repetitions: NonZeroU32,
    /// In the order of the benchmark at its launch.
    pub tasks: Vec<RunTask>,
    /// In the order of their IDs, `as-is` first.
    pub configurations: Vec<ConfigurationAnswer>,
    /// Every trial, in the order they start.
    pub trials: Vec<TrialScreen>,
    pub results: Results,
}

/// A trial of a benchmark run (`docs/specs/benchmarks.md`, Benchmark run,
/// A trial's status, A trial's score and A trial's reason).
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct TrialScreen {
    #[serde(flatten)]
    pub trial: KeyedTrialAnswer,
    /// The name of its state on the screen, such as "pass" or "no score".
    pub status: &'static str,
    /// What the scorer said of its score, or its reason for none.
    #[serde(flatten)]
    pub said: Option<TrialSaid>,
    /// What its status shows as.
    #[serde(skip)]
    pub shows_as: Status,
    /// Whether its run's transcript has started.
    #[serde(skip)]
    pub run_began: bool,
    /// Whether its scorer's transcript has started.
    #[serde(skip)]
    pub scorer_began: bool,
}

/// What a trial's details say besides its score's verdicts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum TrialSaid {
    /// The scorer's reasoning on its score, as its output holds it.
    Reasoning {
        correctness_reasoning: String,
        leak_reasoning: String,
        /// The tool calls that show a leak the scorer found.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        leak_calls: Vec<String>,
    },
    /// Its reason for no score, such as "No score. The build of its
    /// environment failed."
    NoScore { no_score: String },
}

impl BenchmarkRunScreen {
    /// Its trials of task `task` under configuration `configuration`, in
    /// the order they start.
    pub fn trials_of(&self, task: TaskId, configuration: ConfigurationId) -> Vec<&TrialScreen> {
        self.trials
            .iter()
            .filter(|trial| trial.trial.task == task && trial.trial.configuration == configuration)
            .collect()
    }
}

/// How benchmark run `record` shows: an open one as `serve` last showed it.
pub fn shown_of(sources: &Sources, record: &BenchmarkRunRecord) -> Shown {
    let served = sources
        .benchmark_runs
        .shown()
        .benchmark_runs
        .get(&record.id)
        .cloned();
    Shown::of(record.state, served)
}

/// Benchmark run `id` as its screen shows it now, `None` where there is no
/// such benchmark run; what the trials' outputs hold read through
/// `outputs`.
pub async fn get_benchmark_run(
    permit: &Permit<Read>,
    sources: &Sources,
    id: BenchmarkRunId,
    outputs: &Outputs,
) -> Result<Option<BenchmarkRunScreen>> {
    let Some(record) = benchmark_run_record(permit, sources, id).await? else {
        return Ok(None);
    };
    let BenchmarkRunAnswer {
        id: _,
        benchmark,
        stage,
        launched_at,
        ended_at,
        shown,
        repetitions,
        tasks,
        configurations,
        trials: _,
        results,
    } = BenchmarkRunAnswer::new(&record, shown_of(sources, &record));
    let trials = keyed_trials(&record)
        .into_iter()
        .map(|(state, trial)| {
            let output = trial_output_dir(
                permit,
                sources,
                id,
                trial.configuration,
                trial.task,
                trial.repetition,
            );
            let reasoning = state.score().and_then(|_| outputs.reasoning(&output));
            trial_screen(
                record.state,
                state,
                trial,
                outputs.began(&output),
                reasoning.as_deref(),
            )
        })
        .collect();
    Ok(Some(BenchmarkRunScreen {
        id,
        benchmark,
        stage,
        launched_at,
        ended_at,
        shown,
        open: record.state == BenchmarkRunState::Open,
        duration_seconds: record
            .duration(sources.clock.now())
            .map(|duration| duration.as_secs()),
        repetitions,
        tasks,
        configurations,
        trials,
        results,
    }))
}

/// `trial`, kept as `state`, of a benchmark run kept as `benchmark_run`,
/// with whether its output `began` and what its scorer said of its score,
/// `reasoning`.
fn trial_screen(
    benchmark_run: BenchmarkRunState,
    state: &TrialState,
    trial: KeyedTrialAnswer,
    began: bool,
    reasoning: Option<&Reasoning>,
) -> TrialScreen {
    let shown = TrialShown::of(benchmark_run, state);
    let said = match (reasoning, shown.reason()) {
        (Some(reasoning), _) => Some(reasoning.said()),
        (None, reason) => reason.map(|no_score| TrialSaid::NoScore { no_score }),
    };
    TrialScreen {
        status: shown.name(),
        said,
        shows_as: shown.status(),
        run_began: began && !state.run_never_started(),
        scorer_began: began && state.scorer_began(),
        trial,
    }
}

/// What the trials' outputs hold that a benchmark run's screen shows, by
/// output, as read
/// once it holds it: that an output has begun, and the scorers' reasoning
/// of a scored trial, whose score is never replaced.
#[derive(Clone, Default)]
pub struct Outputs(Arc<Mutex<HashMap<PathBuf, OutputRead>>>);

#[derive(Clone, Default)]
struct OutputRead {
    began: bool,
    score: Score,
}

/// What an output's score gave the screen.
#[derive(Clone, Default)]
enum Score {
    #[default]
    Unread,
    /// The scorers' reasoning, or none where the output holds no score
    /// that reads.
    Read(Option<Arc<Reasoning>>),
}

/// What a scorer said of a trial: its reasoning on its correctness, and
/// on a leak, with the tool calls that show one it found.
struct Reasoning {
    correctness: String,
    leak: Leak,
}

impl Reasoning {
    fn said(&self) -> TrialSaid {
        let (leak_reasoning, leak_calls) = match &self.leak {
            Leak::None { reasoning } => (reasoning.clone(), vec![]),
            Leak::Found { reasoning, calls } => (
                reasoning.clone(),
                calls.iter().map(str::to_string).collect(),
            ),
        };
        TrialSaid::Reasoning {
            correctness_reasoning: self.correctness.clone(),
            leak_reasoning,
            leak_calls,
        }
    }
}

impl Outputs {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, OutputRead>> {
        self.0.lock().expect("the outputs' lock is not poisoned")
    }

    /// Whether the output in `dir` has begun.
    fn began(&self, dir: &Path) -> bool {
        let mut read = self.lock();
        let output = read.entry(dir.to_path_buf()).or_default();
        if !output.began {
            output.began = trial::began(dir);
        }
        output.began
    }

    /// The reasoning of the score the output in `dir` of a scored
    /// trial holds, if it holds one that reads; one that does not read is
    /// logged.
    fn reasoning(&self, dir: &Path) -> Option<Arc<Reasoning>> {
        let mut read = self.lock();
        let output = read.entry(dir.to_path_buf()).or_default();
        if let Score::Read(reasoning) = &output.score {
            return reasoning.clone();
        }
        let reasoning = match scorer::read(dir) {
            Ok(kept) => kept.map(|kept| {
                Arc::new(Reasoning {
                    correctness: kept.correctness.reasoning,
                    leak: kept.leak,
                })
            }),
            Err(e) => {
                warn!(output = %dir.display(), error = format!("{e:#}"), "reading a score failed");
                None
            }
        };
        output.score = Score::Read(reasoning.clone());
        reasoning
    }
}
