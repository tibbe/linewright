//! Capturing a run as a task (`docs/specs/benchmarks.md`, Capturing), and
//! what a screen reads to offer it.

use tracing::warn;

use super::benchmarks::{benchmark_taken, benchmarks, task_taken};
use super::{Action, Ended, logged};
use crate::access::{Act, Holder, Permit, Read};
use crate::api::Sources;
use crate::benchmarks::KeptTask;
use crate::name::Name;
use crate::runs::RunId;
use crate::task::{
    self, BenchmarkChoice, CapturableRun, CaptureError, Criteria, Refusal, TaskStage,
};

/// Run `run`, if it can be captured, or why not.
pub async fn capturable_run(
    _: &Permit<Read>,
    sources: &Sources,
    run: &RunId,
) -> Result<CapturableRun, CaptureError> {
    task::capturable_run(&sources.runs, run).await
}

/// The runs of `stage` that can be captured, latest first. Runs that do not
/// read are logged and left out.
pub async fn capturable_runs(_: &Permit<Read>, sources: &Sources, stage: TaskStage) -> Vec<RunId> {
    let recorded = match sources.runs.read().await {
        Ok(recorded) => recorded,
        Err(e) => {
            warn!(error = format!("{e:#}"), "reading the runs failed");
            return vec![];
        }
    };
    let mut of_stage: Vec<_> = recorded
        .into_iter()
        .filter(|run| task::capturable_stage(&run.id.of).is_some_and(|(of, _)| of == stage))
        .collect();
    of_stage.sort_by_key(|run| std::cmp::Reverse(run.started_at));
    let mut runs = vec![];
    for id in of_stage.into_iter().map(|run| run.id) {
        match task::capturable_run(&sources.runs, &id).await {
            Ok(_) => runs.push(id),
            Err(e) if e.refusal() == Refusal::Failed => {
                warn!(run = %id, error = e.why(), "reading whether a run can be captured failed");
            }
            Err(_) => {}
        }
    }
    runs
}

/// Why a run was not captured.
#[derive(Debug, thiserror::Error)]
pub enum AddTaskError {
    /// `serve` has not polled yet (`docs/specs/benchmarks.md`, Capturing,
    /// Before the first poll).
    #[error("Linewright can't capture a run before its first poll.")]
    NotPolled,
    /// The capture was refused or failed for `error`, as `why` says of it,
    /// redacted.
    #[error("{why}")]
    Capture { error: CaptureError, why: String },
}

impl AddTaskError {
    /// Its message (`docs/specs/benchmarks.md`, Capturing, Refusals on a
    /// screen). A refusal the spec gives no message keeps the capture's own
    /// wording.
    pub fn said(&self) -> String {
        let AddTaskError::Capture { error, why } = self else {
            return self.to_string();
        };
        match error {
            CaptureError::Taken { task, benchmark } => task_taken(task, benchmark),
            CaptureError::OtherStage {
                run,
                stage,
                benchmark,
            } => format!(
                "Run {run} is of {stage}, and {} holds tasks of {}.",
                benchmark.name, benchmark.stage
            ),
            CaptureError::NoInput(run) => format!("Run {run} recorded no input."),
            CaptureError::Lacks { run, part } => {
                format!("The record of run {run} lacks {part}.")
            }
            CaptureError::NoSuchRun(run) => no_such_run(run),
            CaptureError::BenchmarkTaken(name) => benchmark_taken(name),
            CaptureError::NotCapturable(_)
            | CaptureError::NoSuchBenchmark(_)
            | CaptureError::Failed(_) => why.clone(),
        }
    }
}

/// The refusal of a capture of run `run`, which does not exist
/// (`docs/specs/benchmarks.md`, Capturing, Refusals on a screen).
pub fn no_such_run(run: &impl std::fmt::Display) -> String {
    format!("No such run {run}.")
}

/// Captures run `run` as the task `name`, with `criteria` as what a good
/// result is, into the benchmark `choice`, with the store `serve` holds
/// (`docs/specs/benchmarks.md`, Capturing).
///
/// # Errors
///
/// Before `serve`'s first poll, and as [`task::capturable`] and
/// [`task::capture`].
pub async fn add_task(
    permit: &Permit<Act, impl Holder>,
    sources: &Sources,
    run: &RunId,
    choice: &BenchmarkChoice,
    name: &Name,
    criteria: &Criteria,
) -> Result<KeptTask, AddTaskError> {
    let changed = format!("run {run}, as the task {name}");
    let Some(served) = sources.served.borrow().clone() else {
        let refused = AddTaskError::NotPolled;
        logged(
            permit.holder(),
            Action::AddTask,
            &changed,
            Ended::Refused(&refused),
        );
        return Err(refused);
    };
    let benchmarks = benchmarks(sources);
    let captured = match task::capturable(&sources.runs, &benchmarks, run, name, choice).await {
        Ok(capturable) => task::capture(served.store(), capturable, criteria, &benchmarks).await,
        Err(e) => Err(e),
    };
    match captured {
        Ok(kept) => {
            let kept_as = format!(
                "run {run}, as the task {} named {name} in benchmark {}",
                kept.id, kept.benchmark.id
            );
            logged(permit.holder(), Action::AddTask, &kept_as, Ended::Done);
            Ok(kept)
        }
        Err(error) => {
            let why = served.redaction().apply(&error.why());
            let how = match error.refusal() {
                Refusal::NoSuch | Refusal::Refused => Ended::Refused(&why),
                Refusal::Failed => Ended::Failed(&why),
            };
            logged(permit.holder(), Action::AddTask, &changed, how);
            Err(AddTaskError::Capture { error, why })
        }
    }
}
