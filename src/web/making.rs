//! Making a thing in a "New" dialog (`docs/specs/design_system.md`, Dialogs
//! and Making; `docs/specs/web/design_system.md`, Making), drawn by the
//! `new_button` and `new_dialog` macros, its form answering into itself.

use askama::Template;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use super::capture::NotCaptured;
use super::render;
use crate::benchmarks::{BenchmarkName, ConfigurationName, LaunchRefusal, MakeError};
use crate::repo_files::FileError;
use crate::task::{CaptureError, TaskName};

/// A dialog's form as drawn: the fields as the reader left them, and why
/// Linewright refused, if it did.
pub(super) struct Making<Fields> {
    pub(super) fields: Fields,
    pub(super) refusal: Option<String>,
}

impl<Fields: Default> Making<Fields> {
    /// The form as it starts.
    pub(super) fn blank() -> Making<Fields> {
        Making {
            fields: Fields::default(),
            refusal: None,
        }
    }
}

impl<Fields> Making<Fields> {
    /// The form as the reader sent it, refused for `refusal`.
    pub(super) fn refused(fields: Fields, refusal: String) -> Making<Fields> {
        Making {
            fields,
            refusal: Some(refusal),
        }
    }
}

/// What a "New" form's POST answers, which htmx swaps into the form.
pub(super) enum Answer<Form: Template> {
    /// 200 with the form as it starts, and the `made` event, on which the
    /// dialog closes.
    Made { blank: Form },
    /// The form as sent, with its refusal.
    Refused { status: StatusCode, form: Form },
}

/// The event, sent in `HX-Trigger`, on which a "New" dialog closes.
const MADE: &str = "made";

impl<Form: Template> IntoResponse for Answer<Form> {
    fn into_response(self) -> Response {
        match self {
            Answer::Made { blank } => (
                StatusCode::OK,
                [("HX-Trigger", HeaderValue::from_static(MADE))],
                axum::response::Html(render(&blank)),
            )
                .into_response(),
            Answer::Refused { status, form } => {
                (status, axum::response::Html(render(&form))).into_response()
            }
        }
    }
}

/// The refusal of a name not in the form of a name
/// (`docs/specs/benchmarks.md`, New benchmark, Refusals).
pub(super) const NOT_A_NAME: &str =
    "A name holds only lowercase letters, digits and hyphens, and starts with a letter or a digit.";

/// The refusal of a benchmark that was not made, or why making it failed.
pub(super) fn benchmark_not_made(error: &MakeError) -> String {
    match error {
        MakeError::Taken(name) => benchmark_taken(name),
        MakeError::Failed(e) => format!("{e:#}"),
    }
}

/// The refusal of a benchmark named as one that exists
/// (`docs/specs/benchmarks.md`, New benchmark, Refusals).
fn benchmark_taken(name: &BenchmarkName) -> String {
    format!("A benchmark named {name} already exists.")
}

/// The refusal of a task that was not made (`docs/specs/benchmarks.md`, New
/// task, Refusals), or why making it was refused otherwise or failed.
pub(super) fn task_not_made(not: &NotCaptured) -> String {
    match not {
        NotCaptured::Name(_) => NOT_A_NAME.into(),
        NotCaptured::NoCriteria => CRITERIA_NEEDED.into(),
        NotCaptured::Capture {
            error: CaptureError::Taken(name),
            ..
        } => task_taken(name),
        NotCaptured::NotPolled | NotCaptured::Capture { .. } => not.why().into(),
    }
}

/// The refusal of a task without criteria.
const CRITERIA_NEEDED: &str = "A task needs criteria.";

/// The refusal of a task named as one that exists.
fn task_taken(name: &TaskName) -> String {
    format!("A task named {name} already exists.")
}

/// The refusal of repetitions that are not a whole number, 1 or more.
pub(super) const REPETITIONS: &str = "Repetitions are a whole number, 1 or more.";

/// The refusal of a configuration with no name, the configuration `added`
/// after `as-is`, counting from 0. It is named by its place in the New
/// benchmark run dialog, where `as-is` is 1st.
pub(super) fn configuration_unnamed(added: usize) -> String {
    const AFTER_AS_IS: usize = 2;
    format!(
        "The {} configuration needs a name.",
        ordinal(added + AFTER_AS_IS)
    )
}

/// `n` as an ordinal, such as "2nd" or "11th".
fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// The refusal of configuration `name`, whose files have `errors`, for the
/// first of them.
pub(super) fn configuration_unusable(name: &ConfigurationName, errors: &[FileError]) -> String {
    match errors.first() {
        Some(FileError { path, error }) => {
            format!("The configuration {name} has an error in {path}: {error}")
        }
        None => format!("The configuration {name} has an error."),
    }
}

/// The status and the refusal of a benchmark run that was not launched, or
/// why launching it failed.
pub(super) fn launch_refused(refusal: &LaunchRefusal) -> (StatusCode, String) {
    match refusal {
        LaunchRefusal::NoSuchBenchmark(_) => (StatusCode::NOT_FOUND, refusal.to_string()),
        LaunchRefusal::NoTasks(name) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("The benchmark {name} has no tasks."),
        ),
        LaunchRefusal::DuplicateConfiguration(name) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("Two configurations are named {name}."),
        ),
        LaunchRefusal::Failed(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}
