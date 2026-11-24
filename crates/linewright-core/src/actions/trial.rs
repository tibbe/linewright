//! The action that shows a trial's transcripts (`docs/specs/benchmarks.md`,
//! Benchmark run, A trial's transcripts), and the lookup of a benchmark
//! run's trial.

use std::num::NonZeroU32;

use std::path::PathBuf;

use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use super::benchmarks::{benchmark_run_record, trial_output_dir};
use super::run::RunScreen;
use crate::access::{Permit, Read};
use crate::api::Sources;
use crate::benchmarks::{
    BenchmarkRunId, BenchmarkRunRecord, ConfigurationId, TaskId, TrialName, keyed_trials,
};
use crate::domain::Number;
use crate::logs::{self, KeptRuns, Which};
use crate::runs::files::RunFiles;
use crate::runs::{Recorded, RunId, RunOf};
use crate::transcript::lines::Verbosity;
use crate::trial;

/// A trial, as its benchmark run, configuration, task and repetition name
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrialAt {
    pub benchmark_run: BenchmarkRunId,
    pub configuration: ConfigurationId,
    pub task: TaskId,
    pub repetition: NonZeroU32,
}

/// Which of a trial's transcripts, in the order of its screen. It
/// serializes as its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrialTranscript {
    Run,
    Scorer,
}

impl TrialTranscript {
    pub const ALL: [TrialTranscript; 2] = [TrialTranscript::Run, TrialTranscript::Scorer];

    /// Its name: "run" or "scorer".
    pub fn name(self) -> &'static str {
        match self {
            TrialTranscript::Run => "run",
            TrialTranscript::Scorer => "scorer",
        }
    }
}

/// Why there is no trial to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoTrial {
    /// There is no such benchmark run.
    BenchmarkRun,
    /// The benchmark run lacks the configuration, the task or the
    /// repetition.
    Trial,
}

/// Where a trial's transcript is read from: the runs it is kept among, and
/// which of them it is.
#[derive(Clone)]
pub struct KeptTranscript {
    pub kept: KeptRuns,
    pub which: Which,
}

/// A trial found in its benchmark run, with where its transcripts are kept
/// once it has begun.
#[derive(Clone)]
pub struct FoundTrial {
    pub name: TrialName,
    pub benchmark_run: BenchmarkRunRecord,
    begun: Option<Begun>,
}

/// A trial whose output has begun: its output, and the task's run it names
/// on item `item`.
#[derive(Clone)]
struct Begun {
    output: PathBuf,
    task_run: RunId,
    item: Number,
}

impl FoundTrial {
    /// Where its transcript `transcript` is kept, which holds no run until
    /// that transcript has started; `None` before the trial has begun.
    pub fn transcript(&self, transcript: TrialTranscript) -> Option<KeptTranscript> {
        let begun = self.begun.as_ref()?;
        Some(match transcript {
            TrialTranscript::Run => KeptTranscript {
                kept: KeptRuns::Files(RunFiles::new(&begun.output)),
                which: Which::Item(begun.item),
            },
            TrialTranscript::Scorer => KeptTranscript {
                kept: KeptRuns::Scorer(begun.output.clone()),
                which: Which::Run {
                    id: begun.task_run.clone(),
                    item: None,
                },
            },
        })
    }
}

/// The trial at `at`, or why there is none. An output that names a run
/// that runs no task is no trial.
///
/// # Errors
///
/// When the benchmark run or the trial's output cannot be read.
pub async fn find_trial(
    permit: &Permit<Read>,
    sources: &Sources,
    at: &TrialAt,
) -> Result<Result<FoundTrial, NoTrial>> {
    let Some(record) = benchmark_run_record(permit, sources, at.benchmark_run).await? else {
        return Ok(Err(NoTrial::BenchmarkRun));
    };
    let Some(name) = keyed_trials(&record)
        .into_iter()
        .map(|(_, trial)| trial)
        .find(|trial| {
            trial.configuration == at.configuration
                && trial.task == at.task
                && trial.repetition == at.repetition
        })
        .map(|trial| trial.name)
    else {
        return Ok(Err(NoTrial::Trial));
    };
    let output = trial_output_dir(
        permit,
        sources,
        at.benchmark_run,
        at.configuration,
        at.task,
        at.repetition,
    );
    let begun = match trial::task_run(&output)? {
        None => None,
        Some(task_run) => {
            let RunOf::Work { item, .. } = task_run.of else {
                return Ok(Err(NoTrial::Trial));
            };
            Some(Begun {
                output,
                task_run,
                item,
            })
        }
    };
    Ok(Ok(FoundTrial {
        name,
        benchmark_run: record,
        begun,
    }))
}

/// A trial's transcripts as its screen shows them: none before its run has
/// started, then its run's, and its scorer's once that has started too.
#[derive(Clone, Debug, PartialEq)]
pub enum TrialTranscriptsScreen {
    NotBegun,
    Begun {
        run: Box<RunScreen>,
        scorer: Option<Box<RunScreen>>,
    },
}

/// The transcripts of the trial at `at` as its screen shows them now, in
/// the view `verbosity` says, or why there is no such trial.
///
/// # Errors
///
/// As [`find_trial`], and when a transcript's records cannot be read.
pub async fn get_trial(
    permit: &Permit<Read>,
    sources: &Sources,
    at: &TrialAt,
    verbosity: Verbosity,
) -> Result<Result<TrialTranscriptsScreen, NoTrial>> {
    let found = match find_trial(permit, sources, at).await? {
        Ok(found) => found,
        Err(no_trial) => return Ok(Err(no_trial)),
    };
    let read = async |transcript| match found.transcript(transcript) {
        Some(kept) => logs::picked(&kept.kept, &kept.which).await,
        None => Ok(None),
    };
    let run = read(TrialTranscript::Run).await?;
    let scorer = read(TrialTranscript::Scorer).await?;
    let now = sources.clock.now();
    let Some(run) = run else {
        return Ok(Ok(TrialTranscriptsScreen::NotBegun));
    };
    let screen = |recorded: &Recorded| RunScreen::of(recorded, verbosity, now).map(Box::new);
    Ok(Ok(TrialTranscriptsScreen::Begun {
        run: screen(&run)?,
        scorer: scorer.as_ref().map(screen).transpose()?,
    }))
}
