//! The local API's routes that change something, each doing its action as
//! the operator (`docs/specs/operation.md`, Commands, `serve` does the
//! work), and what they take and answer.

use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path as Resource, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{patch, post};
use serde::{Deserialize, Serialize};

use super::{ErrorAnswer, Missing, Sources, error_response, json_response};
use crate::access::Permit;
use crate::actions::{self, AddTaskError, LaunchOfDirsRefusal};
use crate::benchmarks::{
    Benchmark, BenchmarkId, BenchmarkRunId, CancelRefusal, ConfigurationId, EditTaskError,
    LaunchRefusal, MakeError, RemoveError, RenameConfigurationError, RenameError, Repetitions,
    TaskChange, TaskId,
};
use crate::name::Name;
use crate::runs::RunId;
use crate::task::{BenchmarkChoice, CaptureError, Criteria, Refusal, TaskStage};

/// The routes, under `/api/`.
pub(super) fn routes() -> Router<Sources> {
    Router::new()
        .route("/tasks", post(capture))
        .route("/benchmarks", post(make_benchmark))
        .route("/benchmarks/{id}", patch(rename_benchmark))
        .route(
            "/benchmarks/{id}/tasks/{task}",
            patch(edit_task).delete(remove_task),
        )
        .route("/benchmark-runs", post(launch_benchmark_run))
        .route(
            "/benchmark-runs/{id}/configurations/{configuration}",
            patch(rename_configuration),
        )
        .route("/benchmark-runs/{id}/cancel", post(cancel_benchmark_run))
}

/// Why a route did not act.
enum Unacted {
    /// The request does not read, saying why.
    Unread(String),
    /// What it was asked to change does not exist.
    NoSuch(Missing, String),
    /// Linewright refuses the change, saying why.
    Refused(String),
    Failed(String),
}

/// The answer of a route once `acting` ends, which it does whether or not
/// the request's client hangs up first: refused with 409, and 404 for what
/// does not exist.
async fn answer<T: Serialize + Send + 'static>(
    acting: impl Future<Output = Result<T, Unacted>> + Send + 'static,
) -> Response {
    let acted = match tokio::spawn(acting).await {
        Ok(acted) => acted,
        Err(e) => Err(Unacted::Failed(format!("acting ended: {e}"))),
    };
    match acted {
        Ok(answer) => json_response(StatusCode::OK, &answer),
        Err(Unacted::Unread(why)) => error_response(StatusCode::BAD_REQUEST, &why),
        Err(Unacted::NoSuch(missing, error)) => json_response(
            StatusCode::NOT_FOUND,
            &ErrorAnswer {
                error,
                missing: Some(missing),
            },
        ),
        Err(Unacted::Refused(why)) => error_response(StatusCode::CONFLICT, &why),
        Err(Unacted::Failed(why)) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &why),
    }
}

/// The request's `body` as `T`.
fn asked<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, Unacted> {
    serde_json::from_slice(body)
        .map_err(|e| Unacted::Unread(format!("the request's body does not read: {e}")))
}

/// The ID `id` names, or that `missing` does not exist, as `no_such` says.
fn parsed_id<T: std::str::FromStr>(
    id: &str,
    missing: Missing,
    no_such: String,
) -> Result<T, Unacted> {
    id.parse().map_err(|_| Unacted::NoSuch(missing, no_such))
}

fn failed(e: &anyhow::Error) -> Unacted {
    Unacted::Failed(format!("{e:#}"))
}

/// That there is no benchmark `id`.
pub fn no_such_benchmark(id: &impl std::fmt::Display) -> String {
    format!("there is no benchmark {id}")
}

/// That there is no benchmark run `id`.
pub fn no_such_benchmark_run(id: &impl std::fmt::Display) -> String {
    format!("there is no benchmark run {id}")
}

/// What `POST /api/tasks` takes: capture run `run` as the task `name` of
/// benchmark `benchmark`, with `criteria`.
#[derive(Serialize, Deserialize)]
pub struct CaptureAsked {
    pub run: RunId,
    pub benchmark: BenchmarkId,
    pub name: Name,
    pub criteria: Criteria,
}

/// A run captured as a task.
#[derive(Serialize, Deserialize)]
pub struct Captured {
    pub run: RunId,
    pub task: TaskId,
    pub name: Name,
    pub benchmark: Benchmark,
    pub dir: PathBuf,
}

async fn capture(State(sources): State<Sources>, body: Bytes) -> Response {
    answer(async move { captured(&sources, &body).await }).await
}

async fn captured(sources: &Sources, body: &Bytes) -> Result<Captured, Unacted> {
    let asked: CaptureAsked = asked(body)?;
    let choice = BenchmarkChoice::Existing(asked.benchmark);
    let kept = actions::add_task(
        &Permit::operator(),
        sources,
        &asked.run,
        &choice,
        &asked.name,
        &asked.criteria,
    )
    .await
    .map_err(|e| match e {
        AddTaskError::NotPolled => Unacted::Refused(e.to_string()),
        AddTaskError::Capture {
            error: CaptureError::NoSuchBenchmark(_),
            why,
        } => Unacted::NoSuch(Missing::Benchmark, why),
        AddTaskError::Capture {
            error: CaptureError::NoSuchRun(_),
            why,
        } => Unacted::NoSuch(Missing::Run, why),
        AddTaskError::Capture { error, why } => match error.refusal() {
            Refusal::Failed => Unacted::Failed(why),
            Refusal::NoSuch | Refusal::Refused => Unacted::Refused(why),
        },
    })?;
    Ok(Captured {
        run: asked.run,
        task: kept.id,
        name: asked.name,
        benchmark: kept.benchmark,
        dir: kept.dir,
    })
}

/// What `POST /api/benchmarks` takes: make the benchmark `name` of `stage`.
#[derive(Serialize, Deserialize)]
pub struct BenchmarkAsked {
    pub name: Name,
    pub stage: TaskStage,
}

async fn make_benchmark(State(sources): State<Sources>, body: Bytes) -> Response {
    answer(async move { made_benchmark(&sources, &body).await }).await
}

async fn made_benchmark(sources: &Sources, body: &Bytes) -> Result<Benchmark, Unacted> {
    let asked: BenchmarkAsked = asked(body)?;
    actions::create_benchmark(&Permit::operator(), sources, &asked.name, asked.stage)
        .await
        .map_err(|e| match e {
            MakeError::Taken(_) => Unacted::Refused(e.to_string()),
            MakeError::Failed(e) => failed(&e),
        })
}

/// What `PATCH /api/benchmarks/{id}` and
/// `PATCH /api/benchmark-runs/{id}/configurations/{configuration}` take:
/// the new name.
#[derive(Serialize, Deserialize)]
pub struct RenameAsked {
    pub name: Name,
}

/// A benchmark renamed.
#[derive(Serialize, Deserialize)]
pub struct Renamed {
    pub id: BenchmarkId,
    pub name: Name,
}

async fn rename_benchmark(
    State(sources): State<Sources>,
    Resource(id): Resource<String>,
    body: Bytes,
) -> Response {
    answer(async move { renamed_benchmark(&sources, &id, &body).await }).await
}

async fn renamed_benchmark(
    sources: &Sources,
    id_given: &str,
    body: &Bytes,
) -> Result<Renamed, Unacted> {
    let no_such = no_such_benchmark(&id_given);
    let id: BenchmarkId = parsed_id(id_given, Missing::Benchmark, no_such)?;
    let asked: RenameAsked = asked(body)?;
    actions::edit_benchmark(&Permit::operator(), sources, id, &asked.name)
        .await
        .map_err(|e| match e {
            RenameError::NoSuchBenchmark(_) => Unacted::NoSuch(Missing::Benchmark, e.to_string()),
            RenameError::Taken(_) => Unacted::Refused(e.to_string()),
            RenameError::Failed(e) => failed(&e),
        })?;
    Ok(Renamed {
        id,
        name: asked.name,
    })
}

/// What `PATCH /api/benchmarks/{id}/tasks/{task}` takes: a new name, new
/// criteria, or both.
#[derive(Serialize, Deserialize)]
pub struct TaskChangeAsked {
    pub name: Option<Name>,
    pub criteria: Option<Criteria>,
}

/// A task changed.
#[derive(Serialize, Deserialize)]
pub struct Edited {
    pub benchmark: BenchmarkId,
    pub task: TaskId,
}

/// The benchmark `benchmark` and its task `task`, or that either does not
/// exist.
fn benchmark_and_task(benchmark: &str, task: &str) -> Result<(BenchmarkId, TaskId), Unacted> {
    let benchmark_id = parsed_id(benchmark, Missing::Benchmark, no_such_benchmark(&benchmark))?;
    let no_such = format!("the benchmark {benchmark} has no task {task}");
    Ok((benchmark_id, parsed_id(task, Missing::Task, no_such)?))
}

async fn edit_task(
    State(sources): State<Sources>,
    Resource((benchmark, task)): Resource<(String, String)>,
    body: Bytes,
) -> Response {
    answer(async move { edited_task(&sources, &benchmark, &task, &body).await }).await
}

async fn edited_task(
    sources: &Sources,
    benchmark: &str,
    task: &str,
    body: &Bytes,
) -> Result<Edited, Unacted> {
    let (benchmark, task) = benchmark_and_task(benchmark, task)?;
    let asked: TaskChangeAsked = asked(body)?;
    let change = TaskChange::of(asked.name, asked.criteria).ok_or_else(|| {
        Unacted::Unread("a change of a task needs a name, criteria or both".to_string())
    })?;
    actions::edit_task(&Permit::operator(), sources, benchmark, task, &change)
        .await
        .map_err(|e| match e {
            EditTaskError::NoSuchBenchmark(_) => Unacted::NoSuch(Missing::Benchmark, e.to_string()),
            EditTaskError::NoSuchTask { .. } => Unacted::NoSuch(Missing::Task, e.to_string()),
            EditTaskError::Taken { .. } => Unacted::Refused(e.to_string()),
            EditTaskError::Failed(e) => failed(&e),
        })?;
    Ok(Edited { benchmark, task })
}

/// A task removed.
#[derive(Serialize, Deserialize)]
pub struct Removed {
    pub benchmark: BenchmarkId,
    pub task: TaskId,
    pub name: Name,
}

async fn remove_task(
    State(sources): State<Sources>,
    Resource((benchmark, task)): Resource<(String, String)>,
) -> Response {
    answer(async move { removed_task(&sources, &benchmark, &task).await }).await
}

async fn removed_task(sources: &Sources, benchmark: &str, task: &str) -> Result<Removed, Unacted> {
    let (benchmark, task) = benchmark_and_task(benchmark, task)?;
    let name = actions::remove_task(&Permit::operator(), sources, benchmark, task)
        .await
        .map_err(|e| match e {
            RemoveError::NoSuchBenchmark(_) => Unacted::NoSuch(Missing::Benchmark, e.to_string()),
            RemoveError::NoSuchTask { .. } => Unacted::NoSuch(Missing::Task, e.to_string()),
            RemoveError::Failed(e) => failed(&e),
        })?;
    Ok(Removed {
        benchmark,
        task,
        name,
    })
}

/// An absolute path, which names the same file whatever the directory of
/// whoever reads it.
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct AbsolutePath(PathBuf);

impl AbsolutePath {
    /// `path`, made absolute against the current directory.
    ///
    /// # Errors
    ///
    /// When `path` is relative and the current directory does not read.
    pub fn of(path: &Path) -> std::io::Result<AbsolutePath> {
        std::path::absolute(path).map(AbsolutePath)
    }
}

/// A path as it serializes, refusing one that is not absolute.
impl<'de> Deserialize<'de> for AbsolutePath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<AbsolutePath, D::Error> {
        let path = PathBuf::deserialize(deserializer)?;
        if !path.is_absolute() {
            let why = format!("{} is not an absolute path", path.display());
            return Err(serde::de::Error::custom(why));
        }
        Ok(AbsolutePath(path))
    }
}

/// What `POST /api/benchmark-runs` takes: launch a benchmark run on
/// benchmark `benchmark` with `as-is` and `configurations`, each repeated
/// `repetitions` times.
#[derive(Serialize, Deserialize)]
pub struct LaunchAsked {
    pub benchmark: BenchmarkId,
    pub configurations: Vec<ConfigurationAsked>,
    pub repetitions: Repetitions,
}

/// A configuration named `name`, in the directory `dir` on `serve`'s host.
#[derive(Serialize, Deserialize)]
pub struct ConfigurationAsked {
    pub name: Name,
    pub dir: AbsolutePath,
}

/// A benchmark run launched.
#[derive(Serialize, Deserialize)]
pub struct LaunchedAnswer {
    pub id: BenchmarkRunId,
    pub benchmark: BenchmarkId,
    pub benchmark_name: Name,
    pub trials: usize,
}

async fn launch_benchmark_run(State(sources): State<Sources>, body: Bytes) -> Response {
    answer(async move { launched_benchmark_run(&sources, &body).await }).await
}

async fn launched_benchmark_run(
    sources: &Sources,
    body: &Bytes,
) -> Result<LaunchedAnswer, Unacted> {
    let asked: LaunchAsked = asked(body)?;
    let dirs = asked
        .configurations
        .into_iter()
        .map(|ConfigurationAsked { name, dir }| (name, dir.0))
        .collect();
    let (benchmark, launched) = actions::run_benchmark_of_dirs(
        &Permit::operator(),
        sources,
        asked.benchmark,
        dirs,
        asked.repetitions,
    )
    .await
    .map_err(|e| match e {
        LaunchOfDirsRefusal::Launch(LaunchRefusal::NoSuchBenchmark(_)) => {
            Unacted::NoSuch(Missing::Benchmark, e.to_string())
        }
        LaunchOfDirsRefusal::Launch(LaunchRefusal::Failed(e)) => failed(&e),
        LaunchOfDirsRefusal::Configuration(_)
        | LaunchOfDirsRefusal::Launch(
            LaunchRefusal::NoTasks(_) | LaunchRefusal::DuplicateConfiguration(_),
        ) => Unacted::Refused(e.to_string()),
    })?;
    Ok(LaunchedAnswer {
        id: launched.id,
        benchmark: benchmark.id,
        benchmark_name: benchmark.name,
        trials: launched.trials,
    })
}

/// A configuration renamed.
#[derive(Serialize, Deserialize)]
pub struct RenamedConfiguration {
    pub id: BenchmarkRunId,
    pub configuration: ConfigurationId,
    pub name: Name,
}

async fn rename_configuration(
    State(sources): State<Sources>,
    Resource((id, configuration)): Resource<(String, String)>,
    body: Bytes,
) -> Response {
    answer(async move { renamed_configuration(&sources, &id, &configuration, &body).await }).await
}

async fn renamed_configuration(
    sources: &Sources,
    id_given: &str,
    configuration_given: &str,
    body: &Bytes,
) -> Result<RenamedConfiguration, Unacted> {
    let no_such_run = no_such_benchmark_run(&id_given);
    let id: BenchmarkRunId = parsed_id(id_given, Missing::BenchmarkRun, no_such_run)?;
    let no_such_configuration =
        format!("benchmark run {id} has no configuration {configuration_given}");
    let configuration: ConfigurationId = parsed_id(
        configuration_given,
        Missing::Configuration,
        no_such_configuration,
    )?;
    let asked: RenameAsked = asked(body)?;
    actions::edit_configuration(&Permit::operator(), sources, id, configuration, &asked.name)
        .await
        .map_err(|e| match e {
            RenameConfigurationError::NoSuchBenchmarkRun(_) => {
                Unacted::NoSuch(Missing::BenchmarkRun, e.to_string())
            }
            RenameConfigurationError::NoSuchConfiguration { .. } => {
                Unacted::NoSuch(Missing::Configuration, e.to_string())
            }
            RenameConfigurationError::AsIs | RenameConfigurationError::Taken(_) => {
                Unacted::Refused(e.to_string())
            }
            RenameConfigurationError::Failed(e) => failed(&e),
        })?;
    Ok(RenamedConfiguration {
        id,
        configuration,
        name: asked.name,
    })
}

/// A benchmark run cancelled.
#[derive(Serialize, Deserialize)]
pub struct CancelledAnswer {
    pub id: BenchmarkRunId,
}

async fn cancel_benchmark_run(
    State(sources): State<Sources>,
    Resource(id): Resource<String>,
) -> Response {
    answer(async move { cancelled_benchmark_run(&sources, &id).await }).await
}

async fn cancelled_benchmark_run(
    sources: &Sources,
    id_given: &str,
) -> Result<CancelledAnswer, Unacted> {
    let no_such = no_such_benchmark_run(&id_given);
    let id: BenchmarkRunId = parsed_id(id_given, Missing::BenchmarkRun, no_such)?;
    actions::cancel_benchmark_run(&Permit::operator(), sources, id)
        .await
        .map_err(|e| match e {
            CancelRefusal::NoSuchBenchmarkRun(_) => {
                Unacted::NoSuch(Missing::BenchmarkRun, e.to_string())
            }
            CancelRefusal::NotOpen { .. } => Unacted::Refused(e.to_string()),
            CancelRefusal::Failed(e) => failed(&e),
        })?;
    Ok(CancelledAnswer { id })
}
