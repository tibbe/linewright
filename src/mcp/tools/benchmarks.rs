//! The tools that change benchmarks, their tasks and their benchmark runs
//! (`docs/specs/benchmarks.md`, Screens), each returning what it made,
//! changed or removed, with the names that `--json` gives its facts.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Context, Tool, id, name, refused};
use crate::access::{Act, Permit, Read};
use crate::actions::{
    self, AS_IS_KEEPS_ITS_NAME, Action, Missing, REQUIRED, another_named, benchmark_taken,
    no_such_run, no_tasks, task_taken,
};
use crate::benchmarks::{
    Benchmark, BenchmarkId, BenchmarkRunId, BenchmarkRunRecord, CancelRefusal, ConfigurationId,
    EditTaskError, LaunchRefusal, MakeError, RemoveError, RenameConfigurationError, RenameError,
    TaskChange, TaskId, TaskShown,
};
use crate::domain::Timestamp;
use crate::name::Name;
use crate::runs::RunId;
use crate::task::{BenchmarkChoice, Criteria, TaskStage};
use crate::web::launch::{GivenConfiguration, given_launch};
use crate::work_settings::Effort;

/// A benchmark, as a tool returns it.
#[derive(Serialize, JsonSchema)]
pub(super) struct BenchmarkOut {
    #[schemars(with = "i64")]
    id: BenchmarkId,
    #[schemars(with = "String")]
    name: Name,
    stage: TaskStage,
}

impl From<Benchmark> for BenchmarkOut {
    fn from(benchmark: Benchmark) -> BenchmarkOut {
        BenchmarkOut {
            id: benchmark.id,
            name: benchmark.name,
            stage: benchmark.stage,
        }
    }
}

/// A task, as a tool returns it.
#[derive(Serialize, JsonSchema)]
pub(super) struct TaskOut {
    #[schemars(with = "i64")]
    id: TaskId,
    #[schemars(with = "String")]
    name: Name,
    /// The run it was captured from.
    #[schemars(with = "String")]
    run: RunId,
    criteria: String,
    benchmark: BenchmarkOut,
}

/// The ID of a benchmark a tool is given.
fn benchmark_id(given: i64) -> Result<BenchmarkId, String> {
    id(given, str::parse, Missing::Benchmark)
}

/// The ID of a task a tool is given.
fn task_id(given: i64) -> Result<TaskId, String> {
    id(given, str::parse, Missing::Task)
}

/// The ID of a benchmark run a tool is given.
fn benchmark_run_id(given: i64) -> Result<BenchmarkRunId, String> {
    id(given, str::parse, Missing::BenchmarkRun)
}

/// Criteria a tool is given, or else the refusal of `criteria` that holds
/// none.
fn criteria(given: &str) -> Result<Criteria, String> {
    Criteria::parse(given).ok_or_else(|| refused("criteria", REQUIRED))
}

/// Task `task` as `permit` reads it, or else that it does not exist, or
/// why it did not read.
async fn shown_task(
    permit: &Permit<Act>,
    context: &Context<'_>,
    task: TaskId,
) -> Result<TaskShown, String> {
    let reading: Permit<Read> = permit.clone().into();
    match actions::get_task(&reading, context.sources, task).await {
        Ok(Some(shown)) => Ok(shown),
        Ok(None) => Err(Missing::Task(task.to_string()).to_string()),
        Err(e) => Err(format!("{e:#}")),
    }
}

/// Capture a run as a task, into a benchmark of its stage or a new one
/// (`docs/specs/benchmarks.md`, Capturing).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct AddTask {
    /// The run to capture.
    run_id: String,
    /// The benchmark to capture it into, without `benchmark_name`.
    benchmark_id: Option<i64>,
    /// The name of a new benchmark to capture it into, without
    /// `benchmark_id`.
    benchmark_name: Option<String>,
    /// The task's name.
    name: String,
    /// What a good result is.
    criteria: String,
}

impl Tool for AddTask {
    const ACTION: Action = Action::AddTask;
    type Level = Act;
    type Output = TaskOut;

    fn changing(&self) -> String {
        format!("run {}, as the task {}", self.run_id, self.name)
    }

    async fn run(self, permit: &Permit<Act>, context: &Context<'_>) -> Result<TaskOut, String> {
        let run: RunId = self.run_id.parse().map_err(|_| no_such_run(&self.run_id))?;
        let choice = match (self.benchmark_id, &self.benchmark_name) {
            (Some(id), None) => BenchmarkChoice::Existing(benchmark_id(id)?),
            (None, Some(new)) => BenchmarkChoice::New(name("benchmark_name", new)?),
            (Some(_), Some(_)) | (None, None) => {
                return Err("Give one of benchmark_id and benchmark_name.".into());
            }
        };
        let name = name("name", &self.name)?;
        let criteria = criteria(&self.criteria)?;
        let kept = actions::add_task(permit, context.sources, &run, &choice, &name, &criteria)
            .await
            .map_err(|e| e.said())?;
        Ok(TaskOut {
            id: kept.id,
            name,
            run,
            criteria: criteria.as_str().to_string(),
            benchmark: kept.benchmark.into(),
        })
    }
}

/// Make a benchmark (`docs/specs/benchmarks.md`, New benchmark).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct CreateBenchmark {
    name: String,
    /// The stage of its tasks: triage without it.
    stage: Option<TaskStage>,
}

impl Tool for CreateBenchmark {
    const ACTION: Action = Action::CreateBenchmark;
    type Level = Act;
    type Output = BenchmarkOut;

    fn changing(&self) -> String {
        format!("a benchmark named {}", self.name)
    }

    async fn run(
        self,
        permit: &Permit<Act>,
        context: &Context<'_>,
    ) -> Result<BenchmarkOut, String> {
        let name = name("name", &self.name)?;
        let stage = self.stage.unwrap_or(TaskStage::Triage);
        match actions::create_benchmark(permit, context.sources, &name, stage).await {
            Ok(benchmark) => Ok(benchmark.into()),
            Err(MakeError::Taken(taken)) => Err(benchmark_taken(&taken)),
            Err(MakeError::Failed(e)) => Err(format!("{e:#}")),
        }
    }
}

/// Change a benchmark's name (`docs/specs/benchmarks.md`, Edit benchmark).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct EditBenchmark {
    benchmark_id: i64,
    name: String,
}

impl Tool for EditBenchmark {
    const ACTION: Action = Action::EditBenchmark;
    type Level = Act;
    type Output = BenchmarkOut;

    fn changing(&self) -> String {
        format!("benchmark {}, renamed to {}", self.benchmark_id, self.name)
    }

    async fn run(
        self,
        permit: &Permit<Act>,
        context: &Context<'_>,
    ) -> Result<BenchmarkOut, String> {
        let id = benchmark_id(self.benchmark_id)?;
        let no_such = || Missing::Benchmark(id.to_string()).to_string();
        let name = name("name", &self.name)?;
        match actions::edit_benchmark(permit, context.sources, id, &name).await {
            Ok(()) => {}
            Err(RenameError::NoSuchBenchmark(_)) => return Err(no_such()),
            Err(RenameError::Taken(taken)) => {
                return Err(benchmark_taken(&taken));
            }
            Err(RenameError::Failed(e)) => return Err(format!("{e:#}")),
        }
        let reading: Permit<Read> = permit.clone().into();
        match actions::get_benchmark(&reading, context.sources, id).await {
            Ok(Some(shown)) => Ok(shown.benchmark.into()),
            Ok(None) => Err(no_such()),
            Err(e) => Err(format!("{e:#}")),
        }
    }
}

/// Change a task's name and criteria (`docs/specs/benchmarks.md`, Edit
/// task).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct EditTask {
    task_id: i64,
    name: String,
    /// What a good result is.
    criteria: String,
}

impl Tool for EditTask {
    const ACTION: Action = Action::EditTask;
    type Level = Act;
    type Output = TaskOut;

    fn changing(&self) -> String {
        format!("task {}", self.task_id)
    }

    async fn run(self, permit: &Permit<Act>, context: &Context<'_>) -> Result<TaskOut, String> {
        let task = task_id(self.task_id)?;
        let shown = shown_task(permit, context, task).await?;
        let name = name("name", &self.name)?;
        let criteria = criteria(&self.criteria)?;
        let change = TaskChange::Both(name.clone(), criteria.clone());
        let benchmark = shown.benchmark;
        match actions::edit_task(permit, context.sources, benchmark.id, task, &change).await {
            Ok(()) => Ok(TaskOut {
                id: task,
                name,
                run: shown.run,
                criteria: criteria.as_str().to_string(),
                benchmark: benchmark.into(),
            }),
            Err(EditTaskError::NoSuchBenchmark(_) | EditTaskError::NoSuchTask { .. }) => {
                Err(Missing::Task(task.to_string()).to_string())
            }
            Err(EditTaskError::Taken { task, benchmark }) => Err(task_taken(&task, &benchmark)),
            Err(EditTaskError::Failed(e)) => Err(format!("{e:#}")),
        }
    }
}

/// A task removed from its benchmark, as a tool returns it.
#[derive(Serialize, JsonSchema)]
pub(super) struct RemovedOut {
    /// The benchmark it was removed from.
    #[schemars(with = "i64")]
    benchmark: BenchmarkId,
    #[schemars(with = "i64")]
    task: TaskId,
    #[schemars(with = "String")]
    name: Name,
}

/// Remove a task and its directory (`docs/specs/benchmarks.md`, Remove
/// task).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct RemoveTask {
    task_id: i64,
}

impl Tool for RemoveTask {
    const ACTION: Action = Action::RemoveTask;
    type Level = Act;
    type Output = RemovedOut;

    fn changing(&self) -> String {
        format!("task {}", self.task_id)
    }

    async fn run(self, permit: &Permit<Act>, context: &Context<'_>) -> Result<RemovedOut, String> {
        let task = task_id(self.task_id)?;
        let benchmark = shown_task(permit, context, task).await?.benchmark.id;
        match actions::remove_task(permit, context.sources, benchmark, task).await {
            Ok(name) => Ok(RemovedOut {
                benchmark,
                task,
                name,
            }),
            Err(e @ (RemoveError::NoSuchBenchmark(_) | RemoveError::NoSuchTask { .. })) => {
                Err(e.to_string())
            }
            Err(RemoveError::Failed(e)) => Err(format!("{e:#}")),
        }
    }
}

/// A configuration but `as-is` of a benchmark run to launch
/// (`docs/specs/benchmarks.md`, New benchmark run, Configurations).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ConfigurationIn {
    name: String,
    /// The model its runs use, where it is not the stage's.
    model: Option<String>,
    /// The effort its runs use, where it is not the stage's.
    effort: Option<Effort>,
    /// How long its sessions may take, such as 6h, where it is not the
    /// stage's.
    session_timeout: Option<String>,
    /// The text of each file of the brief it changes, by its path under
    /// `briefs/`.
    #[serde(default)]
    files_of_the_brief: BTreeMap<String, String>,
}

/// A benchmark run launched, as a tool returns it.
#[derive(Serialize, JsonSchema)]
pub(super) struct LaunchedOut {
    #[schemars(with = "i64")]
    id: BenchmarkRunId,
    #[schemars(with = "i64")]
    benchmark: BenchmarkId,
    /// How many trials it makes.
    trials: usize,
}

/// Launch a benchmark run on a benchmark, with `as-is` and the
/// configurations given (`docs/specs/benchmarks.md`, New benchmark run).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct RunBenchmark {
    benchmark_id: i64,
    /// The configurations besides `as-is`, which is always first.
    #[serde(default)]
    configurations: Vec<ConfigurationIn>,
    /// How many trials of each task each configuration makes: 1 without
    /// it.
    repetitions: Option<NonZeroU32>,
}

impl Tool for RunBenchmark {
    const ACTION: Action = Action::RunBenchmark;
    type Level = Act;
    type Output = LaunchedOut;

    fn changing(&self) -> String {
        format!("a benchmark run on benchmark {}", self.benchmark_id)
    }

    async fn run(self, permit: &Permit<Act>, context: &Context<'_>) -> Result<LaunchedOut, String> {
        let benchmark = benchmark_id(self.benchmark_id)?;
        let reading: Permit<Read> = permit.clone().into();
        let stage = match actions::get_benchmark(&reading, context.sources, benchmark).await {
            Ok(Some(shown)) => shown.benchmark.stage,
            Ok(None) => return Err(Missing::Benchmark(benchmark.to_string()).to_string()),
            Err(e) => return Err(format!("{e:#}")),
        };
        let given = self
            .configurations
            .into_iter()
            .map(|given| GivenConfiguration {
                name: given.name,
                model: given.model.unwrap_or_default(),
                effort: given.effort.map(Effort::name).unwrap_or_default().into(),
                session_timeout: given.session_timeout.unwrap_or_default(),
                files: given.files_of_the_brief.into_iter().collect(),
            })
            .collect();
        let repetitions = self.repetitions.map_or(1, NonZeroU32::get).to_string();
        let (repetitions, configurations) =
            given_launch(context.sources, stage, &repetitions, given).map_err(|refusals| {
                refusals
                    .iter()
                    .map(|(argument, reason)| refused(argument, reason))
                    .collect::<Vec<_>>()
                    .join("\n")
            })?;
        match actions::run_benchmark(
            permit,
            context.sources,
            benchmark,
            configurations,
            repetitions,
        )
        .await
        {
            Ok(launched) => Ok(LaunchedOut {
                id: launched.id,
                benchmark,
                trials: launched.trials,
            }),
            Err(e @ LaunchRefusal::NoSuchBenchmark(_)) => Err(e.to_string()),
            Err(LaunchRefusal::NoTasks(name)) => Err(no_tasks(&name)),
            Err(LaunchRefusal::DuplicateConfiguration(name)) => Err(another_named(&name)),
            Err(LaunchRefusal::Failed(e)) => Err(format!("{e:#}")),
        }
    }
}

/// A configuration of a benchmark run, as a tool returns it.
#[derive(Serialize, JsonSchema)]
pub(super) struct ConfigurationOut {
    #[schemars(with = "i64")]
    benchmark_run: BenchmarkRunId,
    #[schemars(with = "usize")]
    id: ConfigurationId,
    #[schemars(with = "String")]
    name: Name,
}

/// Change the name of a configuration of a benchmark run
/// (`docs/specs/benchmarks.md`, Edit configuration).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct EditConfiguration {
    benchmark_run_id: i64,
    /// The configuration's ID in its benchmark run, 1 for `as-is`.
    configuration_id: i64,
    name: String,
}

impl Tool for EditConfiguration {
    const ACTION: Action = Action::EditConfiguration;
    type Level = Act;
    type Output = ConfigurationOut;

    fn changing(&self) -> String {
        format!(
            "configuration {} of benchmark run {}, renamed to {}",
            self.configuration_id, self.benchmark_run_id, self.name
        )
    }

    async fn run(
        self,
        permit: &Permit<Act>,
        context: &Context<'_>,
    ) -> Result<ConfigurationOut, String> {
        let benchmark_run = benchmark_run_id(self.benchmark_run_id)?;
        let configuration: ConfigurationId =
            id(self.configuration_id, str::parse, Missing::Configuration)?;
        let name = name("name", &self.name)?;
        let renamed = actions::edit_configuration(
            permit,
            context.sources,
            benchmark_run,
            configuration,
            &name,
        )
        .await;
        match renamed {
            Ok(()) => Ok(ConfigurationOut {
                benchmark_run,
                id: configuration,
                name,
            }),
            Err(RenameConfigurationError::NoSuchBenchmarkRun(_)) => {
                Err(Missing::BenchmarkRun(benchmark_run.to_string()).to_string())
            }
            Err(RenameConfigurationError::NoSuchConfiguration { .. }) => {
                Err(Missing::Configuration(configuration.to_string()).to_string())
            }
            Err(RenameConfigurationError::AsIs) => Err(AS_IS_KEEPS_ITS_NAME.into()),
            Err(RenameConfigurationError::Taken(taken)) => Err(another_named(&taken)),
            Err(RenameConfigurationError::Failed(e)) => Err(format!("{e:#}")),
        }
    }
}

/// A benchmark as a benchmark run names it.
#[derive(Serialize, JsonSchema)]
pub(super) struct BenchmarkOfRun {
    #[schemars(with = "i64")]
    id: BenchmarkId,
    #[schemars(with = "String")]
    name: Name,
}

/// A benchmark run as a tool returns it once cancelled, with the names
/// `benchmark-run show --json` gives.
#[derive(Serialize, JsonSchema)]
pub(super) struct CancelledOut {
    #[schemars(with = "i64")]
    id: BenchmarkRunId,
    benchmark: BenchmarkOfRun,
    stage: TaskStage,
    launched_at: Timestamp,
    /// When it was cancelled, if that was kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    ended_at: Option<Timestamp>,
    state: Cancelled,
}

/// The state of a cancelled benchmark run.
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(inline)]
pub(super) enum Cancelled {
    Cancelled,
}

impl CancelledOut {
    /// `record`, cancelled.
    fn of(record: BenchmarkRunRecord) -> CancelledOut {
        CancelledOut {
            id: record.id,
            benchmark: BenchmarkOfRun {
                id: record.benchmark.id,
                name: record.benchmark.name,
            },
            stage: record.benchmark.stage,
            launched_at: record.launched_at,
            ended_at: record.state.ended_at(),
            state: Cancelled::Cancelled,
        }
    }
}

/// Cancel a benchmark run, stopping its running trials and scorers
/// (`docs/specs/benchmarks.md`, Cancel benchmark run).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct CancelBenchmarkRun {
    benchmark_run_id: i64,
}

impl Tool for CancelBenchmarkRun {
    const ACTION: Action = Action::CancelBenchmarkRun;
    type Level = Act;
    type Output = CancelledOut;

    fn changing(&self) -> String {
        format!("benchmark run {}", self.benchmark_run_id)
    }

    async fn run(
        self,
        permit: &Permit<Act>,
        context: &Context<'_>,
    ) -> Result<CancelledOut, String> {
        let id = benchmark_run_id(self.benchmark_run_id)?;
        let no_such = || Missing::BenchmarkRun(id.to_string()).to_string();
        match actions::cancel_benchmark_run(permit, context.sources, id).await {
            Ok(()) => {}
            Err(e @ (CancelRefusal::NoSuchBenchmarkRun(_) | CancelRefusal::NotOpen { .. })) => {
                return Err(e.to_string());
            }
            Err(CancelRefusal::Failed(e)) => return Err(format!("{e:#}")),
        }
        let reading: Permit<Read> = permit.clone().into();
        match actions::get_benchmark_run(&reading, context.sources, id).await {
            Ok(Some(record)) => Ok(CancelledOut::of(record)),
            Ok(None) => Err(no_such()),
            Err(e) => Err(format!("{e:#}")),
        }
    }
}
