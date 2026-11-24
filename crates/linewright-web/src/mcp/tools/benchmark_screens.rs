//! The tools that show benchmarks (`docs/specs/access.md`, The MCP server,
//! What a tool shows): each returns what its screen shows.

use std::num::{NonZeroU32, NonZeroUsize};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use linewright_core::access::{Permit, Read};
use linewright_core::actions::{
    self, Action, BenchmarkRow, BenchmarkRunScreen, BenchmarkScreen, BenchmarksFilters, Missing,
    NoTrial, Outputs, RunHeading, RunScreen, SessionScreen, TaskScreen, TrialAt, TrialTranscript,
    TrialTranscriptsScreen,
};
use linewright_core::benchmarks::BenchmarkRunId;
use linewright_core::task::TaskStage;
use linewright_core::transcript::lines::Verbosity;

use super::benchmarks::{benchmark_id, benchmark_run_id, task_id};
use super::{Context, Tool};
use crate::mcp::page::{Earlier, Paging};
use trial_entry::TrialEntry;

/// See the benchmarks, in the order of their names, narrowed by the
/// filters of Benchmarks (`docs/specs/benchmarks.md`, Benchmarks).
// It holds the fields of `Paging` rather than flattening it, since serde
// reads no unknown field of a struct that flattens another as refused
// (<https://serde.rs/container-attrs.html#deny_unknown_fields>).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct ListBenchmarks {
    /// Keeps the benchmarks whose name holds this text, whatever its letter
    /// case.
    name: Option<String>,
    /// Keeps the benchmarks of this stage.
    stage: Option<TaskStage>,
    /// How many to give at most: 100 without it.
    limit: Option<NonZeroUsize>,
    /// The `after` of the list that gave the ones before: gives those after
    /// it.
    after: Option<String>,
}

/// The benchmarks, as [`ListBenchmarks`] gives them: a page of them, by
/// the names `benchmarks --json` gives them.
#[derive(Serialize, JsonSchema)]
pub(super) struct BenchmarksPage {
    benchmarks: Vec<BenchmarkRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    left_out: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<String>,
}

impl Tool for ListBenchmarks {
    const ACTION: Action = Action::ListBenchmarks;
    type Level = Read;
    type Output = BenchmarksPage;

    async fn run(
        self,
        permit: &Permit<Read>,
        context: &Context<'_>,
    ) -> Result<BenchmarksPage, String> {
        let filters = BenchmarksFilters::new(self.name.as_deref(), self.stage);
        let shown = actions::list_benchmarks(permit, context.sources, &filters)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let paging = Paging {
            limit: self.limit,
            after: self.after,
        };
        let page = paging.page(shown.benchmarks, |row| row.listed.benchmark.id.to_string())?;
        Ok(BenchmarksPage {
            benchmarks: page.items,
            left_out: page.left_out,
            after: page.after,
        })
    }
}

/// See a benchmark: its benchmark runs, latest first, and its tasks
/// (`docs/specs/benchmarks.md`, Benchmark).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct GetBenchmark {
    benchmark_id: i64,
}

impl Tool for GetBenchmark {
    const ACTION: Action = Action::GetBenchmark;
    type Level = Read;
    type Output = BenchmarkScreen;

    async fn run(
        self,
        permit: &Permit<Read>,
        context: &Context<'_>,
    ) -> Result<BenchmarkScreen, String> {
        let id = benchmark_id(self.benchmark_id)?;
        actions::get_benchmark(permit, context.sources, id)
            .await
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| Missing::Benchmark(id.to_string()).to_string())
    }
}

/// See a task: its facts and its criteria (`docs/specs/benchmarks.md`,
/// Task).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct GetTask {
    task_id: i64,
}

impl Tool for GetTask {
    const ACTION: Action = Action::GetTask;
    type Level = Read;
    type Output = TaskScreen;

    async fn run(self, permit: &Permit<Read>, context: &Context<'_>) -> Result<TaskScreen, String> {
        let id = task_id(self.task_id)?;
        actions::get_task(permit, context.sources, id)
            .await
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| Missing::Task(id.to_string()).to_string())
    }
}

/// See a benchmark run: its facts, its results and each of its trials
/// (`docs/specs/benchmarks.md`, Benchmark run).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct GetBenchmarkRun {
    benchmark_run_id: i64,
}

impl Tool for GetBenchmarkRun {
    const ACTION: Action = Action::GetBenchmarkRun;
    type Level = Read;
    type Output = BenchmarkRunScreen;

    async fn run(
        self,
        permit: &Permit<Read>,
        context: &Context<'_>,
    ) -> Result<BenchmarkRunScreen, String> {
        let id = benchmark_run_id(self.benchmark_run_id)?;
        actions::get_benchmark_run(permit, context.sources, id, &Outputs::default())
            .await
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| Missing::BenchmarkRun(id.to_string()).to_string())
    }
}

/// See a trial's transcripts: its run's, then its scorer's, each once it
/// has started, and the latest of their entries as one list
/// (`docs/specs/benchmarks.md`, Benchmark run, A trial's transcripts).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct GetTrial {
    benchmark_run_id: i64,
    configuration_id: i64,
    task_id: i64,
    /// Which repetition of the task under the configuration, from 1.
    repetition: NonZeroU32,
    /// Whether to give the transcripts in full, as the verbose switch does.
    #[serde(default)]
    verbose: bool,
    /// How many of the latest entries to give at most: 100 without it.
    limit: Option<NonZeroUsize>,
    /// The `before` of the call that gave the entries after: gives those
    /// before them.
    before: Option<String>,
}

/// A transcript of a trial, as [`GetTrial`] gives it: the heading and
/// the sessions of its run.
#[derive(Serialize, JsonSchema)]
pub(super) struct TrialTranscriptScreen {
    #[serde(flatten)]
    heading: RunHeading,
    sessions: Vec<SessionScreen>,
}

/// A trial's transcripts, as [`GetTrial`] gives them: its run and its
/// scorer, each once it has started, and the latest of their entries, the
/// run's then the scorer's, with how many earlier ones it left out and
/// the `before` that gives those.
#[derive(Serialize, JsonSchema)]
pub(super) struct TrialPage {
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<TrialTranscriptScreen>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scorer: Option<TrialTranscriptScreen>,
    /// In order.
    entries: Vec<TrialEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    left_out: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<String>,
}

impl TrialTranscriptScreen {
    /// `screen`, the screen of the trial's transcript `transcript`, as
    /// [`GetTrial`] gives it, and its entries.
    fn of(
        transcript: TrialTranscript,
        screen: RunScreen,
    ) -> (TrialTranscriptScreen, Vec<TrialEntry>) {
        let entries = screen
            .entries
            .into_iter()
            .map(|entry| TrialEntry::of(transcript, entry))
            .collect();
        let shown = TrialTranscriptScreen {
            heading: screen.heading,
            sessions: screen.sessions,
        };
        (shown, entries)
    }
}

mod trial_entry {
    use schemars::JsonSchema;
    use serde::Serialize;

    use linewright_core::actions::{EntryScreen, TrialTranscript};
    use linewright_core::transcript::lines::Stamped;

    /// An entry of a trial's transcripts, as [`super::GetTrial`] gives it:
    /// its ID within the trial, `run/ID` or `scorer/ID`, which is what
    /// `before` names, and the transcript it is of.
    #[derive(Serialize, JsonSchema)]
    pub(in crate::mcp) struct TrialEntry {
        id: String,
        transcript: TrialTranscript,
        #[serde(flatten)]
        line: Stamped,
    }

    impl TrialEntry {
        /// `entry` of the trial's transcript `transcript`.
        pub(in crate::mcp) fn of(transcript: TrialTranscript, entry: EntryScreen) -> TrialEntry {
            TrialEntry {
                id: format!("{}/{}", transcript.name(), entry.id),
                transcript,
                line: entry.line,
            }
        }

        /// Its ID within the trial.
        pub(in crate::mcp) fn id(&self) -> &str {
            &self.id
        }
    }
}

impl Tool for GetTrial {
    const ACTION: Action = Action::GetTrial;
    type Level = Read;
    type Output = TrialPage;

    async fn run(self, permit: &Permit<Read>, context: &Context<'_>) -> Result<TrialPage, String> {
        let benchmark_run = benchmark_run_id(self.benchmark_run_id)?;
        let (Ok(configuration), Ok(task)) = (
            self.configuration_id.to_string().parse(),
            self.task_id.to_string().parse(),
        ) else {
            let no_trial =
                match actions::benchmark_run_record(permit, context.sources, benchmark_run)
                    .await
                    .map_err(|e| format!("{e:#}"))?
                {
                    None => NoTrial::BenchmarkRun,
                    Some(_) => NoTrial::Trial,
                };
            return Err(refusal(no_trial, benchmark_run));
        };
        let at = TrialAt {
            benchmark_run,
            configuration,
            task,
            repetition: self.repetition,
        };
        let screen = actions::get_trial(permit, context.sources, &at, Verbosity::of(self.verbose))
            .await
            .map_err(|e| format!("{e:#}"))?
            .map_err(|no_trial| refusal(no_trial, benchmark_run))?;
        let (run, scorer) = match screen {
            TrialTranscriptsScreen::NotBegun => (None, None),
            TrialTranscriptsScreen::Begun { run, scorer } => (
                Some(TrialTranscriptScreen::of(TrialTranscript::Run, *run)),
                scorer.map(|scorer| TrialTranscriptScreen::of(TrialTranscript::Scorer, *scorer)),
            ),
        };
        let (run, run_entries) = run.unzip();
        let (scorer, scorer_entries) = scorer.unzip();
        let entries: Vec<TrialEntry> = run_entries
            .into_iter()
            .chain(scorer_entries)
            .flatten()
            .collect();
        let earlier = Earlier {
            limit: self.limit,
            before: self.before,
        };
        let latest = earlier.latest(entries, |entry| entry.id().to_string())?;
        Ok(TrialPage {
            run,
            scorer,
            entries: latest.items,
            left_out: latest.left_out,
            before: latest.before,
        })
    }
}

/// The refusal of a trial of benchmark run `benchmark_run` for
/// `no_trial`: as `get_benchmark_run` refuses a benchmark run that does not
/// exist, and else "No such trial." (`docs/specs/access.md`, The MCP
/// server, A trial that does not exist).
fn refusal(no_trial: NoTrial, benchmark_run: BenchmarkRunId) -> String {
    match no_trial {
        NoTrial::BenchmarkRun => Missing::BenchmarkRun(benchmark_run.to_string()).to_string(),
        NoTrial::Trial => "No such trial.".to_string(),
    }
}
