//! The action that shows a trial's transcripts (`docs/specs/benchmarks.md`,
//! Benchmark run, A trial's transcripts), and the lookup of a benchmark
//! run's trial.

use std::num::NonZeroU32;

use std::path::PathBuf;

use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use super::Missing;
use super::benchmarks::{benchmark_run_record, trial_output_dir};
use super::run::{EntryScreen, RunHeading, RunScreen, SessionScreen};
use crate::access::{Permit, Read};
use crate::api::Sources;
use crate::benchmarks::{
    BenchmarkRunId, BenchmarkRunRecord, ConfigurationId, TaskId, TrialName, keyed_trials,
};
use crate::domain::Number;
use crate::logs::{self, KeptRuns, Which};
use crate::runs::files::RunFiles;
use crate::runs::{Recorded, RunId, RunOf};
use crate::transcript::lines::{Stamped, Verbosity};
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

/// Which of a trial's transcripts, in the order of its screen; it
/// serializes as its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
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

impl TrialAt {
    /// The trial of benchmark run `benchmark_run` that `configuration`,
    /// `task` and `repetition` name, as a call gives them; `None` where
    /// one can name no trial, such as repetition 0.
    pub fn of_parts(
        benchmark_run: BenchmarkRunId,
        configuration: i64,
        task: i64,
        repetition: i64,
    ) -> Option<TrialAt> {
        Some(TrialAt {
            benchmark_run,
            configuration: configuration.to_string().parse().ok()?,
            task: task.to_string().parse().ok()?,
            repetition: u32::try_from(repetition).ok()?.try_into().ok()?,
        })
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

impl NoTrial {
    /// What a call for a trial of benchmark run `benchmark_run` gets where
    /// there is none (`docs/specs/access.md`, The MCP server, A trial that
    /// does not exist).
    pub fn message(self, benchmark_run: BenchmarkRunId) -> String {
        match self {
            NoTrial::BenchmarkRun => Missing::BenchmarkRun(benchmark_run.to_string()).to_string(),
            NoTrial::Trial => "No such trial.".to_string(),
        }
    }
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

/// The transcripts of the trial of benchmark run `benchmark_run` that
/// `configuration`, `task` and `repetition` name, as one list, in the view
/// `verbosity` says; or else why there is no such trial, which parts that
/// can name none make a trial the benchmark run lacks.
///
/// # Errors
///
/// As [`get_trial`], and when the benchmark run cannot be read.
pub async fn get_trial_transcripts(
    permit: &Permit<Read>,
    sources: &Sources,
    benchmark_run: BenchmarkRunId,
    (configuration, task, repetition): (i64, i64, i64),
    verbosity: Verbosity,
) -> Result<Result<TrialTranscripts, NoTrial>> {
    let Some(at) = TrialAt::of_parts(benchmark_run, configuration, task, repetition) else {
        let record = benchmark_run_record(permit, sources, benchmark_run).await?;
        return Ok(Err(match record {
            Some(_) => NoTrial::Trial,
            None => NoTrial::BenchmarkRun,
        }));
    };
    Ok(get_trial(permit, sources, &at, verbosity)
        .await?
        .map(TrialTranscripts::from))
}

/// A trial's transcripts as one list (`docs/specs/access.md`, The MCP
/// server, Transcripts): each that has started, with its sessions, and
/// their entries in the order of its screen, the run's then the scorer's.
#[derive(Clone, Debug, PartialEq)]
pub struct TrialTranscripts {
    pub transcripts: Vec<TranscriptScreen>,
    pub entries: Vec<TrialEntry>,
}

/// One of a trial's transcripts, with the heading and the sessions its
/// screen shows.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct TranscriptScreen {
    pub transcript: TrialTranscript,
    /// The heading of its run.
    #[serde(flatten)]
    pub heading: RunHeading,
    /// The sessions it shows, in order.
    pub sessions: Vec<SessionScreen>,
}

/// An entry of a trial's transcripts: which transcript it is of, and its
/// ID there, which names it among all the trial's entries.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct TrialEntry {
    transcript: TrialTranscript,
    /// Its own among the trial's entries, as its transcripts grow: what
    /// `before` names. Its transcript, then its ID there, such as
    /// `scorer/1/2`.
    id: String,
    #[serde(flatten)]
    line: Stamped,
}

impl TrialEntry {
    /// `entry`, of the transcript `transcript`.
    pub fn new(transcript: TrialTranscript, entry: EntryScreen) -> TrialEntry {
        TrialEntry {
            transcript,
            id: format!("{}/{}", transcript.name(), entry.id),
            line: entry.line,
        }
    }

    /// Its ID among the trial's entries.
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl From<TrialTranscriptsScreen> for TrialTranscripts {
    fn from(screen: TrialTranscriptsScreen) -> TrialTranscripts {
        let started = match screen {
            TrialTranscriptsScreen::NotBegun => vec![],
            TrialTranscriptsScreen::Begun { run, scorer } => {
                let mut started = vec![(TrialTranscript::Run, run)];
                started.extend(scorer.map(|scorer| (TrialTranscript::Scorer, scorer)));
                started
            }
        };
        let mut shown = TrialTranscripts {
            transcripts: vec![],
            entries: vec![],
        };
        for (transcript, run) in started {
            let RunScreen {
                heading,
                sessions,
                entries,
            } = *run;
            shown.transcripts.push(TranscriptScreen {
                transcript,
                heading,
                sessions,
            });
            shown.entries.extend(
                entries
                    .into_iter()
                    .map(|entry| TrialEntry::new(transcript, entry)),
            );
        }
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_log::test]
    fn a_trial_transcript_serializes_as_its_name() {
        for transcript in TrialTranscript::ALL {
            assert_eq!(serde_json::to_value(transcript).unwrap(), transcript.name());
        }
    }

    #[test_log::test]
    fn parts_that_can_name_no_trial_give_none() {
        let id: BenchmarkRunId = "1".parse().unwrap();
        assert!(TrialAt::of_parts(id, 1, 1, 1).is_some());
        for (configuration, task, repetition) in [
            (0, 1, 1),
            (-1, 1, 1),
            (1, 0, 1),
            (1, 1, 0),
            (1, 1, -1),
            (1, 1, 1 << 40),
        ] {
            assert_eq!(
                TrialAt::of_parts(id, configuration, task, repetition),
                None,
                "{configuration} {task} {repetition}"
            );
        }
    }
}
