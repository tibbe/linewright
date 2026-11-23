//! The actions on benchmarks, their tasks and their benchmark runs
//! (`docs/specs/benchmarks.md`).

use std::num::NonZeroU32;
use std::path::PathBuf;

use anyhow::Result;

use super::{Action, Ended, Ending, logged};
use crate::access::{Act, Permit, Read};
use crate::api::Sources;
use crate::benchmarks::{
    Benchmark, BenchmarkId, BenchmarkRunId, BenchmarkRunRecord, BenchmarkRuns, BenchmarkShown,
    Benchmarks, CancelRefusal, ConfigurationId, EditTaskError, LaunchRefusal, Launched, MakeError,
    RemoveError, RemovedTask, RenameConfigurationError, RenameError, Repetitions, TaskChange,
    TaskId, TaskShown,
};
use crate::name::Name;
use crate::task::TaskStage;
use crate::trial::Configuration;

/// The refusal of a benchmark named as one that exists
/// (`docs/specs/benchmarks.md`, New benchmark, Refusals).
pub fn benchmark_taken(name: &Name) -> String {
    format!("A benchmark named {name} already exists.")
}

/// The refusal of a task named as another task of its benchmark
/// (`docs/specs/benchmarks.md`, Capturing, Refusals on a screen).
pub fn task_taken(task: &Name, benchmark: &Name) -> String {
    format!("A task named {task} already exists in {benchmark}.")
}

/// The refusal of the name `name` that another configuration has
/// (`docs/specs/benchmarks.md`, New benchmark run, Refusals).
pub fn another_named(name: &Name) -> String {
    format!("Another configuration is named {name}.")
}

/// The refusal of a benchmark run on the benchmark `name`, which has no
/// tasks (`docs/specs/benchmarks.md`, New benchmark run, Refusals).
pub fn no_tasks(name: &Name) -> String {
    format!("The benchmark {name} has no tasks.")
}

/// The refusal of a change to the name of `as-is`, which keeps it.
pub const AS_IS_KEEPS_ITS_NAME: &str = "The configuration as-is keeps its name.";

/// The benchmarks of `serve`'s work directory.
pub(super) fn benchmarks(sources: &Sources) -> Benchmarks {
    Benchmarks::new(sources.runs.database().clone())
}

/// The benchmark runs of `serve`'s work directory.
pub(super) fn benchmark_runs(sources: &Sources) -> BenchmarkRuns {
    BenchmarkRuns::new(sources.runs.database().clone())
}

/// The benchmarks of `stage`, by name.
pub async fn benchmarks_of_stage(
    _: &Permit<Read>,
    sources: &Sources,
    stage: TaskStage,
) -> Result<Vec<Benchmark>> {
    Ok(benchmarks(sources)
        .list()
        .await?
        .into_iter()
        .filter(|listed| listed.benchmark.stage == stage)
        .map(|listed| listed.benchmark)
        .collect())
}

/// Benchmark `id`, with its tasks and its benchmark runs, `None` where
/// there is no such benchmark.
pub async fn benchmark_shown(
    _: &Permit<Read>,
    sources: &Sources,
    id: BenchmarkId,
) -> Result<Option<BenchmarkShown>> {
    benchmarks(sources).show(id).await
}

/// Task `id` as it was when it was removed, `None` where no such task was
/// removed: what Benchmark says of it once (`docs/specs/benchmarks.md`,
/// Task, After "Remove").
pub async fn removed_task(
    _: &Permit<Read>,
    sources: &Sources,
    id: TaskId,
) -> Result<Option<RemovedTask>> {
    benchmarks(sources).removed_task(id).await
}

/// Task `id`, with its benchmark and what its directory gives of it,
/// `None` where there is no such task.
pub async fn task_shown(
    _: &Permit<Read>,
    sources: &Sources,
    id: TaskId,
) -> Result<Option<TaskShown>> {
    benchmarks(sources).task(id).await
}

/// Benchmark run `id` as it is kept, `None` where there is no such
/// benchmark run.
pub async fn benchmark_run_record(
    _: &Permit<Read>,
    sources: &Sources,
    id: BenchmarkRunId,
) -> Result<Option<BenchmarkRunRecord>> {
    benchmark_runs(sources).show(id).await
}

/// The output of the trial of benchmark run `id` that is `repetition` of
/// task `task` under `configuration`.
pub fn trial_output_dir(
    _: &Permit<Read>,
    sources: &Sources,
    id: BenchmarkRunId,
    configuration: ConfigurationId,
    task: TaskId,
    repetition: NonZeroU32,
) -> PathBuf {
    benchmark_runs(sources).output_dir(id, configuration, task, repetition)
}

/// Makes the benchmark `name` of `stage` (`docs/specs/benchmarks.md`, New
/// benchmark).
///
/// # Errors
///
/// When the name is taken, or the benchmark cannot be written.
pub async fn create_benchmark(
    permit: &Permit<Act>,
    sources: &Sources,
    name: &Name,
    stage: TaskStage,
) -> Result<Benchmark, MakeError> {
    let made = benchmarks(sources).make(name, stage).await;
    let how = Ended::of(&made);
    let changed = match &made {
        Ok(benchmark) => format!("benchmark {} named {name} of {stage}", benchmark.id),
        Err(_) => format!("a benchmark named {name} of {stage}"),
    };
    logged(permit.caller(), Action::CreateBenchmark, &changed, how);
    made
}

/// Renames benchmark `id` to `name` (`docs/specs/benchmarks.md`, Edit
/// benchmark).
///
/// # Errors
///
/// When there is no such benchmark, the name is taken, or the change cannot
/// be written.
pub async fn edit_benchmark(
    permit: &Permit<Act>,
    sources: &Sources,
    id: BenchmarkId,
    name: &Name,
) -> Result<(), RenameError> {
    let renamed = benchmarks(sources).rename(id, name).await;
    let how = Ended::of(&renamed);
    logged(
        permit.caller(),
        Action::EditBenchmark,
        &format!("benchmark {id}, renamed to {name}"),
        how,
    );
    renamed
}

/// Changes task `task` of benchmark `benchmark` as `change` says
/// (`docs/specs/benchmarks.md`, Edit task).
///
/// # Errors
///
/// When there is no such benchmark or task, the name is taken in the
/// benchmark, or the change cannot be written.
pub async fn edit_task(
    permit: &Permit<Act>,
    sources: &Sources,
    benchmark: BenchmarkId,
    task: TaskId,
    change: &TaskChange,
) -> Result<(), EditTaskError> {
    let changed = benchmarks(sources).edit_task(benchmark, task, change).await;
    let how = Ended::of(&changed);
    logged(
        permit.caller(),
        Action::EditTask,
        &format!("task {task} of benchmark {benchmark}"),
        how,
    );
    changed
}

/// Removes task `task` from benchmark `benchmark`, and gives its name
/// (`docs/specs/benchmarks.md`, Remove task).
///
/// # Errors
///
/// When there is no such benchmark or task, or the removal cannot be
/// written.
pub async fn remove_task(
    permit: &Permit<Act>,
    sources: &Sources,
    benchmark: BenchmarkId,
    task: TaskId,
) -> Result<Name, RemoveError> {
    let removed = benchmarks(sources).remove_task(benchmark, task).await;
    let how = Ended::of(&removed);
    let changed = match &removed {
        Ok(name) => format!("task {task} named {name} of benchmark {benchmark}"),
        Err(_) => format!("task {task} of benchmark {benchmark}"),
    };
    logged(permit.caller(), Action::RemoveTask, &changed, how);
    removed
}

/// Launches a benchmark run on benchmark `benchmark` with `configurations`
/// and `repetitions`, and tells `serve` so (`docs/specs/benchmarks.md`, New
/// benchmark run).
///
/// # Errors
///
/// As [`BenchmarkRuns::launch`].
pub async fn run_benchmark(
    permit: &Permit<Act>,
    sources: &Sources,
    benchmark: BenchmarkId,
    configurations: Vec<(Name, Configuration)>,
    repetitions: Repetitions,
) -> Result<Launched, LaunchRefusal> {
    let launched = benchmark_runs(sources)
        .launch(benchmark, configurations, repetitions, sources.clock.now())
        .await;
    if launched.is_ok() {
        sources.benchmark_runs.changed();
    }
    let how = Ended::of(&launched);
    let changed = match &launched {
        Ok(launched) => format!(
            "benchmark run {} on benchmark {benchmark}, of {} trials",
            launched.id, launched.trials
        ),
        Err(_) => format!("a benchmark run on benchmark {benchmark}"),
    };
    logged(permit.caller(), Action::RunBenchmark, &changed, how);
    launched
}

/// Renames configuration `configuration` of benchmark run `id` to `name`
/// (`docs/specs/benchmarks.md`, Edit configuration).
///
/// # Errors
///
/// As [`BenchmarkRuns::rename_configuration`].
pub async fn edit_configuration(
    permit: &Permit<Act>,
    sources: &Sources,
    id: BenchmarkRunId,
    configuration: ConfigurationId,
    name: &Name,
) -> Result<(), RenameConfigurationError> {
    let renamed = benchmark_runs(sources)
        .rename_configuration(id, configuration, name)
        .await;
    let how = Ended::of(&renamed);
    logged(
        permit.caller(),
        Action::EditConfiguration,
        &format!("configuration {configuration} of benchmark run {id}, renamed to {name}"),
        how,
    );
    renamed
}

/// Cancels benchmark run `id`, and tells `serve` so
/// (`docs/specs/benchmarks.md`, Cancel benchmark run).
///
/// # Errors
///
/// When there is no such benchmark run, it has ended, or the change cannot
/// be written.
pub async fn cancel_benchmark_run(
    permit: &Permit<Act>,
    sources: &Sources,
    id: BenchmarkRunId,
) -> Result<(), CancelRefusal> {
    let cancelled = benchmark_runs(sources)
        .cancel(id, sources.clock.now())
        .await;
    if cancelled.is_ok() {
        sources.benchmark_runs.changed();
    }
    let how = Ended::of(&cancelled);
    logged(
        permit.caller(),
        Action::CancelBenchmarkRun,
        &format!("benchmark run {id}"),
        how,
    );
    cancelled
}

impl Ending for MakeError {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            MakeError::Taken(_) => None,
            MakeError::Failed(e) => Some(e),
        }
    }
}

impl Ending for RenameError {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            RenameError::NoSuchBenchmark(_) | RenameError::Taken(_) => None,
            RenameError::Failed(e) => Some(e),
        }
    }
}

impl Ending for EditTaskError {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            EditTaskError::NoSuchBenchmark(_)
            | EditTaskError::NoSuchTask { .. }
            | EditTaskError::Taken { .. } => None,
            EditTaskError::Failed(e) => Some(e),
        }
    }
}

impl Ending for RemoveError {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            RemoveError::NoSuchBenchmark(_) | RemoveError::NoSuchTask { .. } => None,
            RemoveError::Failed(e) => Some(e),
        }
    }
}

impl Ending for LaunchRefusal {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            LaunchRefusal::NoSuchBenchmark(_)
            | LaunchRefusal::NoTasks(_)
            | LaunchRefusal::DuplicateConfiguration(_) => None,
            LaunchRefusal::Failed(e) => Some(e),
        }
    }
}

impl Ending for RenameConfigurationError {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            RenameConfigurationError::NoSuchBenchmarkRun(_)
            | RenameConfigurationError::NoSuchConfiguration { .. }
            | RenameConfigurationError::AsIs
            | RenameConfigurationError::Taken(_) => None,
            RenameConfigurationError::Failed(e) => Some(e),
        }
    }
}

impl Ending for CancelRefusal {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            CancelRefusal::NoSuchBenchmarkRun(_) | CancelRefusal::NotOpen { .. } => None,
            CancelRefusal::Failed(e) => Some(e),
        }
    }
}
