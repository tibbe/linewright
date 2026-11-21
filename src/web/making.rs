//! Making a thing in a dialog, such as one that "New" opens
//! (`docs/specs/design_system.md`, Dialogs and Making;
//! `docs/specs/web/design_system.md`, Making), drawn by the `new_dialog` and
//! `new_form` macros, its form answering into itself, and what was made said
//! on a screen that does not show it.

use std::collections::BTreeMap;

use askama::Template;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use super::html::Html;
use super::render;
use crate::benchmarks::BenchmarkName;

/// A dialog's fields as the reader left them, with the fields it draws.
pub(super) trait Form: Default {
    type Field: Field;
}

/// A field a dialog draws.
pub(super) trait Field: Ord + Clone {
    /// Whether Linewright cannot make the thing without it
    /// (`docs/specs/design_system.md`, Making, Required).
    #[expect(dead_code, reason = "every dialog refuses as a whole")]
    fn required(&self) -> bool;
}

/// Why Linewright refused a dialog's form: of its fields `F`, or of none.
pub(super) enum Refusal<F: Field> {
    /// Of one or more fields, each with its reason under it.
    #[expect(dead_code, reason = "every dialog refuses as a whole")]
    Fields(FieldRefusals<F>),
    /// Of no field, with its reason above the buttons.
    Whole(String),
}

/// One or more fields refused, at most one reason for each.
#[expect(dead_code, reason = "every dialog refuses as a whole")]
pub(super) struct FieldRefusals<F: Field>(BTreeMap<F, String>);

#[expect(dead_code, reason = "every dialog refuses as a whole")]
impl<F: Field> FieldRefusals<F> {
    /// `field` refused for `reason`.
    pub(super) fn of(field: F, reason: impl Into<String>) -> FieldRefusals<F> {
        FieldRefusals(BTreeMap::from([(field, reason.into())]))
    }

    /// These, and `field` refused for `reason` unless it already is.
    pub(super) fn and(mut self, field: F, reason: impl Into<String>) -> FieldRefusals<F> {
        self.0.entry(field).or_insert_with(|| reason.into());
        self
    }

    /// The fields `refused`, the first reason for each, or `None` for none.
    pub(super) fn collect(
        refused: impl IntoIterator<Item = (F, String)>,
    ) -> Option<FieldRefusals<F>> {
        let mut refusals = BTreeMap::new();
        for (field, reason) in refused {
            refusals.entry(field).or_insert(reason);
        }
        (!refusals.is_empty()).then_some(FieldRefusals(refusals))
    }
}

/// A dialog's form as drawn: the fields as the reader left them, and why
/// Linewright refused, if it did.
pub(super) struct Making<F: Form> {
    pub(super) fields: F,
    pub(super) refusal: Option<Refusal<F::Field>>,
}

impl<F: Form> Making<F> {
    /// The form as it starts.
    pub(super) fn blank() -> Making<F> {
        Making::of(F::default())
    }

    /// The form holding `fields`, unrefused.
    pub(super) fn of(fields: F) -> Making<F> {
        Making {
            fields,
            refusal: None,
        }
    }

    /// The form as the reader sent it, refused for `refusal`.
    pub(super) fn refused(fields: F, refusal: Refusal<F::Field>) -> Making<F> {
        Making {
            fields,
            refusal: Some(refusal),
        }
    }

    /// The reason of a refusal of no field, which the form shows above its
    /// buttons.
    pub(super) fn whole(&self) -> Option<&str> {
        match &self.refusal {
            Some(Refusal::Whole(reason)) => Some(reason),
            Some(Refusal::Fields(_)) | None => None,
        }
    }
}

/// What a making dialog's form's POST answers, which htmx swaps into the
/// form.
pub(super) enum Answer<Form: Template> {
    /// 200 with the form as it starts, and the `made` event, on which the
    /// dialog closes; with what was made, if `shown` says the screen does
    /// not show it.
    Made { blank: Form, shown: MadeShown },
    /// The form as sent, with its refusal.
    Refused { status: StatusCode, form: Form },
}

/// Whether the dialog's screen shows what it made
/// (`docs/specs/design_system.md`, Making, What was made).
pub(super) enum MadeShown {
    /// The screen shows it, so says nothing of it.
    OnScreen,
    /// The screen does not show it, so says this of it under its heading,
    /// until the reader leaves.
    Elsewhere(Html),
}

/// The event, sent in `HX-Trigger`, on which a making dialog closes.
const MADE: &str = "made";

impl<Form: Template> IntoResponse for Answer<Form> {
    fn into_response(self) -> Response {
        match self {
            Answer::Made { blank, shown } => {
                let mut answer = render(&blank);
                if let MadeShown::Elsewhere(said) = shown {
                    answer.push_str(&made_said(&said));
                }
                (
                    StatusCode::OK,
                    [("HX-Trigger", HeaderValue::from_static(MADE))],
                    axum::response::Html(answer),
                )
                    .into_response()
            }
            Answer::Refused { status, form } => {
                (status, axum::response::Html(render(&form))).into_response()
            }
        }
    }
}

/// `said` of what was made, swapped out of band into the shell's `#made`,
/// which no stream sends, as a daisyUI success alert
/// (`docs/specs/web/design_system.md`, Making).
fn made_said(said: &Html) -> String {
    format!(
        r#"<div id="made" hx-swap-oob="innerHTML"><div class="alert alert-success" role="status"><span>{said}</span></div></div>"#
    )
}

/// The refusal of a required field that is empty or holds only white space
/// (`docs/specs/design_system.md`, Making, Required).
#[expect(dead_code, reason = "every dialog refuses as a whole")]
pub(super) const REQUIRED: &str = "Required.";

/// The refusal of a name not in the form of a name
/// (`docs/specs/benchmarks.md`, New benchmark, Refusals).
pub(super) const NOT_A_NAME: &str =
    "A name holds only lowercase letters, digits and hyphens, and starts with a letter or a digit.";

/// The refusal of a benchmark named as one that exists
/// (`docs/specs/benchmarks.md`, New benchmark, Refusals).
pub(super) fn benchmark_taken(name: &BenchmarkName) -> String {
    format!("A benchmark named {name} already exists.")
}
