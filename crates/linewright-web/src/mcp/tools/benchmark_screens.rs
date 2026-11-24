//! The tools that show benchmarks (`docs/specs/access.md`, The MCP server,
//! What a tool shows): each returns what its screen shows.

use std::num::NonZeroUsize;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use linewright_core::access::{Permit, Read};
use linewright_core::actions::{
    self, Action, BenchmarkRow, BenchmarkRunScreen, BenchmarkScreen, BenchmarksFilters, Missing,
    Outputs, TaskScreen, TranscriptScreen, TrialEntry,
};
use linewright_core::task::TaskStage;
use linewright_core::transcript::lines::Verbosity;

use super::benchmarks::{benchmark_id, benchmark_run_id, task_id};
use super::{Context, Tool};
use crate::mcp::page::{Earlier, Latest, Paging};

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

/// See a trial's transcripts: the run's, then the scorer's, each once it
/// has started, with the latest of their entries as one list
/// (`docs/specs/benchmarks.md`, Benchmark run, A trial's transcripts).
// It holds the fields of `Earlier` rather than flattening it, since serde
// reads no unknown field of a struct that flattens another as refused
// (<https://serde.rs/container-attrs.html#deny_unknown_fields>).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct GetTrial {
    benchmark_run_id: i64,
    /// The ID of the configuration in its benchmark run, as
    /// `get_benchmark_run` gives it.
    configuration_id: i64,
    /// The ID of the task, as `get_benchmark_run` gives it.
    task_id: i64,
    /// Which repetition of the task under the configuration, from 1.
    repetition: i64,
    /// Whether to give the transcripts in full, as the verbose switch does.
    #[serde(default)]
    verbose: bool,
    /// How many of the latest entries to give at most: 100 without it.
    limit: Option<NonZeroUsize>,
    /// The `before` of the call that gave the entries after: gives those
    /// before them.
    before: Option<String>,
}

/// A trial's transcripts, as [`GetTrial`] gives them: each that has
/// started, with its sessions, and the latest of their entries, with how
/// many earlier ones it left out and the `before` that gives those.
#[derive(Serialize, JsonSchema)]
pub(super) struct TrialPage {
    transcripts: Vec<TranscriptScreen>,
    #[serde(flatten)]
    latest: Latest<TrialEntry>,
}

impl Tool for GetTrial {
    const ACTION: Action = Action::GetTrial;
    type Level = Read;
    type Output = TrialPage;

    async fn run(self, permit: &Permit<Read>, context: &Context<'_>) -> Result<TrialPage, String> {
        let id = benchmark_run_id(self.benchmark_run_id)?;
        let parts = (self.configuration_id, self.task_id, self.repetition);
        let verbosity = Verbosity::of(self.verbose);
        let shown = actions::get_trial_transcripts(permit, context.sources, id, parts, verbosity)
            .await
            .map_err(|e| format!("{e:#}"))?
            .map_err(|no| no.message(id))?;
        let earlier = Earlier {
            limit: self.limit,
            before: self.before,
        };
        Ok(TrialPage {
            transcripts: shown.transcripts,
            latest: earlier.latest(shown.entries, |entry| entry.id().to_string())?,
        })
    }
}
