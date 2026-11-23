//! Capturing a run as a task from its page (`docs/specs/runs.md`, Run,
//! Capturing): the making dialog that "Add task" opens, and the `POST` on the
//! run's path that captures it.

use askama::Template;
use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{StatusCode, Uri};
use serde::Deserialize;
use tracing::warn;

use super::benchmark_page::BenchmarkTab;
use super::caller::{Asking, Reader};
use super::html::{self, Html};
use super::making::{
    self, Answer, Field, FieldRefusals, Form, MadeShown, Making, REQUIRED, benchmark_taken, both,
    task_taken,
};
use super::write::Write;
use super::{benchmark_tab_path, run_path};
use crate::access::{Act, Permit, Read};
use crate::actions::{self, Action, AddTaskError};
use crate::api::Sources;
use crate::benchmarks::Benchmark;
use crate::name::Name;
use crate::runs::RunId;
use crate::task::{BenchmarkChoice, CaptureError, Criteria, Refusal, capturable_stage};

/// The id of the capture's dialog, which "Add task" opens.
pub(super) const DIALOG: &str = "capture";

/// The run page's offer to capture its run as a task.
pub(super) struct Offer {
    run: RunId,
    /// The benchmarks of the run's stage, by name.
    benchmarks: Vec<Benchmark>,
}

impl Offer {
    /// The offer for run `id`, if it can be captured. A run whose records
    /// do not read is logged and offers none.
    pub(super) async fn of(permit: &Permit<Read>, sources: &Sources, id: &RunId) -> Option<Offer> {
        let run = match actions::capturable_run(permit, sources, id).await {
            Ok(run) => run,
            Err(e) => {
                if e.refusal() == Refusal::Failed {
                    warn!(run = %id, error = e.why(), "reading whether a run can be captured failed");
                }
                return None;
            }
        };
        let benchmarks = match actions::benchmarks_of_stage(permit, sources, run.stage()).await {
            Ok(benchmarks) => benchmarks,
            Err(e) => {
                warn!(run = %id, error = format!("{e:#}"), "reading the benchmarks failed");
                return None;
            }
        };
        Some(Offer {
            run: run.id().clone(),
            benchmarks,
        })
    }

    /// The capture's dialog, [`DIALOG`], with its form as it starts. The page
    /// draws it outside every region a stream sends.
    pub(super) fn html(self) -> Html {
        html::rendered(&CaptureDialog {
            form: CaptureForm::blank(&self.run, self.benchmarks),
        })
    }
}

/// The benchmarks of `run`'s stage, by name, none for a run of no such stage
/// or when they do not read, which is logged.
async fn benchmarks_of_run(
    permit: &Permit<Read>,
    sources: &Sources,
    run: &RunId,
) -> Vec<Benchmark> {
    let Some((stage, _)) = capturable_stage(&run.of) else {
        return vec![];
    };
    actions::benchmarks_of_stage(permit, sources, stage)
        .await
        .unwrap_or_else(|e| {
            warn!(run = %run, error = format!("{e:#}"), "reading the benchmarks failed");
            vec![]
        })
}

/// The capture's dialog (`docs/specs/runs.md`, Run, Capturing), holding
/// its form.
#[derive(Template)]
#[template(path = "capture_dialog.html")]
struct CaptureDialog {
    form: CaptureForm,
}

impl CaptureDialog {
    fn form(&self) -> Html {
        html::rendered(&self.form)
    }
}

/// The capture's fields, as the form sends them.
#[derive(Default, Deserialize)]
pub(super) struct CaptureFields {
    /// The ID of the benchmark chosen, empty for a new one.
    #[serde(default)]
    benchmark: String,
    /// The new benchmark's name.
    #[serde(default, rename = "new-benchmark")]
    new_benchmark: String,
    #[serde(flatten)]
    task: TaskFields,
}

impl Form for CaptureFields {
    type Field = CaptureField;
}

impl CaptureFields {
    /// The benchmark chosen and the task, or else the refusal of each field
    /// that does not hold one (`docs/specs/runs.md`, Run, Capturing;
    /// `docs/specs/design_system.md`, Making, Required).
    fn checked(&self) -> Result<(BenchmarkChoice, Task), FieldRefusals<CaptureField>> {
        let choice = if self.benchmark.is_empty() {
            making::parse_name(&self.new_benchmark)
                .map(BenchmarkChoice::New)
                .map_err(|why| FieldRefusals::of(CaptureField::NewBenchmark, why))
        } else {
            self.benchmark
                .parse()
                .map(BenchmarkChoice::Existing)
                .map_err(|why: String| FieldRefusals::of(CaptureField::Benchmark, why))
        };
        both(
            choice,
            self.task.checked().map_err(FieldRefusals::into_fields),
        )
    }
}

/// A field of the capture's dialog.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum CaptureField {
    /// The benchmark chosen.
    Benchmark,
    /// The new benchmark's name.
    NewBenchmark,
    Task(TaskField),
}

impl Field for CaptureField {
    fn required(&self) -> bool {
        match self {
            CaptureField::Benchmark | CaptureField::NewBenchmark => true,
            CaptureField::Task(field) => field.required(),
        }
    }
}

impl From<TaskField> for CaptureField {
    fn from(field: TaskField) -> CaptureField {
        CaptureField::Task(field)
    }
}

impl CapturingField for CaptureField {
    fn naming(choice: &BenchmarkChoice) -> Option<CaptureField> {
        Some(match choice {
            BenchmarkChoice::Existing(_) => CaptureField::Benchmark,
            BenchmarkChoice::New(_) => CaptureField::NewBenchmark,
        })
    }
}

/// What the capture's dialog's form holds: its fields, each under its label,
/// then the reason of a refusal of no field, if there is one, then "Cancel"
/// and "Add task".
#[derive(Template)]
#[template(path = "capture_form.html")]
pub(super) struct CaptureForm {
    /// Where it posts: the run's path.
    path: String,
    making: Making<CaptureFields>,
    /// The benchmarks it offers, by name, then a new one.
    benchmarks: Vec<Benchmark>,
}

impl CaptureForm {
    /// The form of the capture of `run` as it starts, offering `benchmarks`:
    /// the first of them chosen, or else a new one, and the name empty.
    fn blank(run: &RunId, benchmarks: Vec<Benchmark>) -> CaptureForm {
        CaptureForm {
            path: run_path(run),
            making: Making::of(CaptureFields {
                benchmark: benchmarks
                    .first()
                    .map(|benchmark| benchmark.id.to_string())
                    .unwrap_or_default(),
                new_benchmark: String::new(),
                task: TaskFields {
                    name: String::new(),
                    criteria: String::new(),
                },
            }),
            benchmarks,
        }
    }

    /// It refused `sent` for `refusal`, answered with `status`.
    fn refused_as_sent(
        mut self,
        sent: CaptureFields,
        (status, refusal): (StatusCode, making::Refusal<CaptureField>),
    ) -> Answer<CaptureForm> {
        self.making = Making::refused(sent, refusal);
        Answer::Refused { status, form: self }
    }

    /// It refused `sent` for `why`, of no field, answered with `status`.
    fn refused_whole(
        self,
        sent: CaptureFields,
        (status, why): (StatusCode, String),
    ) -> Answer<CaptureForm> {
        self.refused_as_sent(sent, (status, making::Refusal::Whole(why)))
    }

    /// Whether "New benchmark" is chosen: as sent, or as the one choice
    /// offered. Another chosen, "Benchmark name" is hidden, and disabled so
    /// that the browser neither checks nor sends it. A benchmark sent that
    /// is no longer offered leaves the first offered chosen.
    fn new_benchmark_chosen(&self) -> bool {
        self.making.fields.benchmark.is_empty() || self.benchmarks.is_empty()
    }

    /// Whether `benchmark` is the one chosen.
    fn chosen(&self, benchmark: &Benchmark) -> bool {
        benchmark.id.to_string() == self.making.fields.benchmark
    }
}

/// What a capture's form, or Edit task's, sends of the task.
#[derive(Clone, Default, Deserialize)]
pub(super) struct TaskFields {
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) criteria: String,
}

impl TaskFields {
    /// The task they name, its criteria with each line break as typed, which
    /// a browser sends as CR LF
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#the-textarea-element>),
    /// or else the refusal of each field that does not hold its part
    /// (`docs/specs/benchmarks.md`, Capturing, Refusals on a screen;
    /// `docs/specs/design_system.md`, Making, Required).
    pub(super) fn checked(&self) -> Result<Task, FieldRefusals<TaskField>> {
        let name =
            making::parse_name(&self.name).map_err(|why| FieldRefusals::of(TaskField::Name, why));
        let criteria = Criteria::parse(&self.criteria.replace("\r\n", "\n"))
            .ok_or_else(|| FieldRefusals::of(TaskField::Criteria, REQUIRED));
        both(name, criteria).map(|(name, criteria)| Task { name, criteria })
    }
}

impl Form for TaskFields {
    type Field = TaskField;
}

/// A task a capture makes, or the change of a task that Edit task saves.
pub(super) struct Task {
    pub(super) name: Name,
    pub(super) criteria: Criteria,
}

/// A field of a capture's task, which New task and Add task both draw.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum TaskField {
    Name,
    Criteria,
}

impl Field for TaskField {
    fn required(&self) -> bool {
        match self {
            TaskField::Name | TaskField::Criteria => true,
        }
    }
}

/// A field of a dialog that captures a run, which draws the fields of its
/// task.
pub(super) trait CapturingField: Field + From<TaskField> {
    /// The field that chose the benchmark `choice`, if the dialog has one.
    fn naming(choice: &BenchmarkChoice) -> Option<Self>;
}

/// Captures run `id` as the task the form in `body` names, with the store
/// `serve` holds, and answers with the capture's form: as it starts once
/// captured, with what it made said under the screen's heading, or as sent
/// with why it was refused. Refused, writing nothing, unless the browser
/// shows the request came from the web UI's own pages.
pub(super) async fn capture(
    Reader {
        permit: reading,
        sources,
    }: Reader,
    asking: Asking,
    Path(id): Path<String>,
    uri: Uri,
    body: Bytes,
) -> Answer<CaptureForm> {
    const WRITE: Write = Write::Capture;
    let permitted = WRITE.permit(Action::AddTask, &asking, &uri);
    let (sent, permitted) = match WRITE.read_form::<CaptureFields>(&body) {
        Ok(sent) => (sent, permitted),
        Err(unread) => (CaptureFields::default(), permitted.and(Err(unread))),
    };
    let Ok(run) = id.parse::<RunId>() else {
        let form = CaptureForm {
            path: uri.path().to_string(),
            making: Making::blank(),
            benchmarks: vec![],
        };
        return form.refused_whole(sent, (StatusCode::NOT_FOUND, no_such_run(&id)));
    };
    let form = CaptureForm::blank(&run, benchmarks_of_run(&reading, &sources, &run).await);
    let permit = match permitted {
        Ok(permit) => permit,
        Err(refused) => return form.refused_whole(sent, refused.into_parts()),
    };
    match capture_sent(&permit, &sources, &run, &sent).await {
        Ok(captured) => Answer::Made {
            shown: MadeShown::Elsewhere(captured.said(&run)),
            blank: CaptureForm::blank(&run, with_made(form.benchmarks, &captured)),
        },
        Err(why) => form.refused_as_sent(sent, why),
    }
}

/// `benchmarks`, by name, with the one `captured` made among them.
fn with_made(mut benchmarks: Vec<Benchmark>, captured: &Captured) -> Vec<Benchmark> {
    if captured.made
        && let Err(at) =
            benchmarks.binary_search_by(|benchmark| benchmark.name.cmp(&captured.benchmark.name))
    {
        benchmarks.insert(at, captured.benchmark.clone());
    }
    benchmarks
}

/// Captures `run` into the benchmark `sent` chooses, as the task it names;
/// or else the status and the refusal.
async fn capture_sent(
    permit: &Permit<Act>,
    sources: &Sources,
    run: &RunId,
    sent: &CaptureFields,
) -> Result<Captured, (StatusCode, making::Refusal<CaptureField>)> {
    let (choice, task) = sent.checked().map_err(|refusals| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            making::Refusal::Fields(refusals),
        )
    })?;
    capture_run(permit, sources, run, &choice, task)
        .await
        .map_err(|not| not.refusal(&choice))
}

/// A run captured as a task.
pub(super) struct Captured {
    /// The task's name.
    name: Name,
    /// The benchmark it was captured into.
    benchmark: Benchmark,
    /// Whether the capture made the benchmark.
    made: bool,
}

impl Captured {
    /// What the screen says of `run` captured as it (`docs/specs/runs.md`,
    /// Run, Capturing, What was made), linking to the benchmark's Tasks.
    fn said(&self, run: &RunId) -> Html {
        let new = if self.made { "new " } else { "" };
        html::built(format!(
            r#"Captured run {} as the task {} in the {new}benchmark <a class="link" href="{}">{}</a>."#,
            html::escape(&run.to_string()),
            html::escape(self.name.as_str()),
            html::escape(&benchmark_tab_path(self.benchmark.id, BenchmarkTab::Tasks)),
            html::escape(self.benchmark.name.as_str()),
        ))
    }
}

/// Why a capture did not happen.
pub(super) struct NotCaptured(AddTaskError);

impl NotCaptured {
    /// The status the capture into `choice` that did not happen is answered
    /// with, and its refusal in a dialog of the fields `F`
    /// (`docs/specs/benchmarks.md`, Capturing, Refusals on a screen;
    /// `docs/specs/design_system.md`, Making, A refusal): a taken name under
    /// the task's name, a benchmark taken or gone under the field that chose
    /// it, where there is one, and the rest above the buttons.
    pub(super) fn refusal<F: CapturingField>(
        &self,
        choice: &BenchmarkChoice,
    ) -> (StatusCode, making::Refusal<F>) {
        let AddTaskError::Capture { error, why } = &self.0 else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                making::Refusal::Whole(self.0.to_string()),
            );
        };
        let reason = reason(error, why);
        let field = match error {
            CaptureError::Taken { .. } => Some(F::from(TaskField::Name)),
            CaptureError::BenchmarkTaken(_) | CaptureError::NoSuchBenchmark(_) => F::naming(choice),
            CaptureError::NoSuchRun(_)
            | CaptureError::NotCapturable(_)
            | CaptureError::NoInput(_)
            | CaptureError::Lacks { .. }
            | CaptureError::OtherStage { .. }
            | CaptureError::Failed(_) => None,
        };
        match field {
            Some(field) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                making::Refusal::Fields(FieldRefusals::of(field, reason)),
            ),
            None => (status_of(error), making::Refusal::Whole(reason)),
        }
    }
}

/// What a screen says of a capture that did not happen for `error`, as `why`
/// says of it, redacted. A refusal the spec gives no message keeps the
/// capture's own wording.
fn reason(error: &CaptureError, why: &str) -> String {
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
        CaptureError::Lacks { run, part } => format!("The record of run {run} lacks {part}."),
        CaptureError::NoSuchRun(run) => no_such_run(run),
        CaptureError::BenchmarkTaken(name) => benchmark_taken(name),
        CaptureError::NotCapturable(_)
        | CaptureError::NoSuchBenchmark(_)
        | CaptureError::Failed(_) => why.to_string(),
    }
}

/// The refusal of a capture of run `run`, which does not exist
/// (`docs/specs/benchmarks.md`, Capturing, Refusals on a screen).
fn no_such_run(run: &impl std::fmt::Display) -> String {
    format!("No such run {run}.")
}

/// Captures `run` as `task` into the benchmark `choice`; or else says why it
/// did not (`docs/specs/benchmarks.md`, Capturing, Before the first poll).
pub(super) async fn capture_run(
    permit: &Permit<Act>,
    sources: &Sources,
    run: &RunId,
    choice: &BenchmarkChoice,
    Task { name, criteria }: Task,
) -> Result<Captured, NotCaptured> {
    match actions::add_task(permit, sources, run, choice, &name, &criteria).await {
        Ok(kept) => Ok(Captured {
            name,
            benchmark: kept.benchmark,
            made: matches!(choice, BenchmarkChoice::New(_)),
        }),
        Err(not) => Err(NotCaptured(not)),
    }
}

/// The status a capture that did not happen for `e` is answered with.
fn status_of(e: &CaptureError) -> StatusCode {
    match e.refusal() {
        Refusal::NoSuch => StatusCode::NOT_FOUND,
        Refusal::Refused => StatusCode::UNPROCESSABLE_ENTITY,
        Refusal::Failed => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
