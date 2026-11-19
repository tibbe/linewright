//! Making a thing in a "New" dialog (`docs/specs/design_system.md`, Dialogs
//! and Making; `docs/specs/web/design_system.md`, Making), drawn by the
//! `new_button` and `new_dialog` macros, its form answering into itself.

use askama::Template;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use super::render;
use crate::benchmarks::{MakeError, SuiteName};

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
/// (`docs/specs/benchmarks.md`, New suite, Refusals).
pub(super) const NOT_A_NAME: &str =
    "A name holds only lowercase letters, digits and hyphens, and starts with a letter or a digit.";

/// The refusal of a suite that was not made, or why making it failed.
pub(super) fn suite_not_made(error: &MakeError) -> String {
    match error {
        MakeError::Taken(name) => suite_taken(name),
        MakeError::Failed(e) => format!("{e:#}"),
    }
}

/// The refusal of a suite named as one that exists
/// (`docs/specs/benchmarks.md`, New suite, Refusals).
fn suite_taken(name: &SuiteName) -> String {
    format!("A suite named {name} already exists.")
}
