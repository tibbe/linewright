//! Capturing a run as a task (`docs/specs/benchmarks.md`, Capturing), and
//! what a screen reads to offer it.

use tracing::warn;

use super::benchmarks::benchmarks;
use super::{Action, Ended, logged};
use crate::access::{Act, Permit, Read};
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

/// Captures run `run` as the task `name`, with `criteria` as what a good
/// result is, into the benchmark `choice`, with the store `serve` holds
/// (`docs/specs/benchmarks.md`, Capturing).
///
/// # Errors
///
/// Before `serve`'s first poll, and as [`task::capturable`] and
/// [`task::capture`].
pub async fn add_task(
    permit: &Permit<Act>,
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
            permit.caller(),
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
            logged(permit.caller(), Action::AddTask, &kept_as, Ended::Done);
            Ok(kept)
        }
        Err(error) => {
            let why = served.redaction().apply(&error.why());
            let how = match error.refusal() {
                Refusal::NoSuch | Refusal::Refused => Ended::Refused(&why),
                Refusal::Failed => Ended::Failed(&why),
            };
            logged(permit.caller(), Action::AddTask, &changed, how);
            Err(AddTaskError::Capture { error, why })
        }
    }
}
