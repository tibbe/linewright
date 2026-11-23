//! Making a thing in a dialog, such as one that "New" opens, or changing one
//! in a dialog that "Edit" opens (`docs/specs/design_system.md`, Dialogs,
//! Making and Editing; `docs/specs/web/design_system.md`, Making and
//! Editing), drawn by the `new_dialog` and `new_form` macros, its form
//! answering into itself, and what was made said on a screen that does not
//! show it.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use askama::Template;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use super::html::{self, Html};
use super::render;
use crate::name::Name;

/// A dialog's fields as the reader left them, with the fields it draws.
pub(super) trait Form: Default {
    type Field: Field;
}

/// A field a dialog draws.
pub(super) trait Field: Ord + Clone {
    /// Whether Linewright cannot make the thing without it
    /// (`docs/specs/design_system.md`, Making, Required).
    fn required(&self) -> bool;
}

/// Why Linewright refused a dialog's form: of its fields `F`, or of none.
pub(super) enum Refusal<F: Field> {
    /// Of one or more fields, each with its reason under it.
    Fields(FieldRefusals<F>),
    /// Of no field, with its reason above the buttons.
    Whole(String),
}

/// One or more fields refused, at most one reason for each.
#[derive(Debug, PartialEq)]
pub(super) struct FieldRefusals<F: Field>(BTreeMap<F, String>);

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

    /// These, each of `field` as a field of a dialog that draws it.
    pub(super) fn into_fields<G: Field + From<F>>(self) -> FieldRefusals<G> {
        FieldRefusals(
            self.0
                .into_iter()
                .map(|(field, reason)| (G::from(field), reason))
                .collect(),
        )
    }

    /// These, and each of `more` whose field is not refused yet.
    pub(super) fn with(mut self, more: FieldRefusals<F>) -> FieldRefusals<F> {
        for (field, reason) in more.0 {
            self.0.entry(field).or_insert(reason);
        }
        self
    }

    /// Why `field` is refused, if it is.
    fn reason(&self, field: &F) -> Option<&str> {
        self.0.get(field).map(String::as_str)
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

    /// How the form shows `field`: whether it is required, and why
    /// Linewright refused it, if it did.
    pub(super) fn field(&self, field: &F::Field) -> FieldShown {
        let reason = match &self.refusal {
            Some(Refusal::Fields(refusals)) => refusals.reason(field).map(str::to_string),
            Some(Refusal::Whole(_)) | None => None,
        };
        FieldShown {
            reason,
            ..FieldShown::unrefused(field)
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

/// A field as its dialog's form draws it with the `field` macro
/// (`docs/specs/design_system.md`, Making, Required and A refusal;
/// `docs/specs/web/design_system.md`, Making).
pub(super) struct FieldShown {
    required: bool,
    /// Why Linewright refused it, if it did.
    reason: Option<String>,
}

impl FieldShown {
    /// How a form shows `field` while Linewright has not refused it.
    pub(super) fn unrefused(field: &impl Field) -> FieldShown {
        FieldShown {
            required: field.required(),
            reason: None,
        }
    }

    pub(super) fn required(&self) -> bool {
        self.required
    }

    /// What its hint says while Linewright has not refused it: "Required."
    /// for a required field, and nothing for an optional one.
    pub(super) fn unrefused_hint(&self) -> &'static str {
        if self.required { REQUIRED } else { "" }
    }

    /// What its hint says: why Linewright refused it, or else
    /// [`FieldShown::unrefused_hint`].
    pub(super) fn hint(&self) -> &str {
        self.reason.as_deref().unwrap_or(self.unrefused_hint())
    }

    /// The attributes of its control `kind` whose id is `id`, but for its
    /// class `validator`: the browser's checks of a required field, which
    /// every value but one empty or of white space alone passes, and while
    /// Linewright refuses it, `aria-invalid`, described by its hint.
    pub(super) fn attributes(&self, id: &str, kind: Control) -> Html {
        let mut attributes = String::new();
        if self.required && kind != Control::SelectWithBlank {
            attributes.push_str(" required");
            if kind == Control::Input {
                attributes.push_str(r#" pattern="[\s\S]*\S[\s\S]*""#);
            }
        }
        if self.reason.is_some() {
            let _ = write!(
                attributes,
                r#" aria-invalid="true" aria-describedby="{}-hint""#,
                html::escape(id)
            );
        }
        html::built(attributes)
    }
}

/// The kind of a field's control.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Control {
    Input,
    Select,
    /// A select one of whose choices has an empty value. The browser's
    /// `required` takes that choice for no choice where it comes first, as
    /// the select's placeholder label option
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#placeholder-label-option>),
    /// so Linewright alone checks it.
    SelectWithBlank,
    Textarea,
}

/// What `a` and `b` checked, or else the refusals of both.
pub(super) fn both<A, B, F: Field>(
    a: Result<A, FieldRefusals<F>>,
    b: Result<B, FieldRefusals<F>>,
) -> Result<(A, B), FieldRefusals<F>> {
    match (a, b) {
        (Ok(a), Ok(b)) => Ok((a, b)),
        (Err(refused), Ok(_)) | (Ok(_), Err(refused)) => Err(refused),
        (Err(a), Err(b)) => Err(a.with(b)),
    }
}

/// The title of the making dialog `dialog`'s form, drawn by the `form_title`
/// macro, which a stream sends as a region of its page, so that it names a
/// thing as it is named now.
#[derive(Template)]
#[template(
    source = r#"{%- import "macros.html" as ui -%}{% call ui::form_title(dialog, title) %}{% endcall %}"#,
    ext = "html"
)]
pub(super) struct FormTitle<'a> {
    pub(super) dialog: &'a str,
    pub(super) title: String,
}

/// What a making dialog's form's POST answers, which htmx swaps into the
/// form.
pub(super) enum Answer<Form: Template> {
    /// 200 with the form as it starts, and the `made` event, on which the
    /// dialog closes; with what was made, if `shown` says the screen does
    /// not show it.
    Made { blank: Form, shown: MadeShown },
    /// 200 with the form holding what was saved, and the `made` event, on
    /// which the dialog closes (`docs/specs/design_system.md`, Editing,
    /// Saving).
    Saved { form: Form },
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

/// The event, sent in `HX-Trigger`, on which a making or editing dialog
/// closes.
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
            Answer::Saved { form } => (
                StatusCode::OK,
                [("HX-Trigger", HeaderValue::from_static(MADE))],
                axum::response::Html(render(&form)),
            )
                .into_response(),
            Answer::Refused { status, form } => {
                (status, axum::response::Html(render(&form))).into_response()
            }
        }
    }
}

/// `said` of what was made, swapped out of band into the shell's `#made`,
/// which no stream sends.
fn made_said(said: &Html) -> String {
    format!(
        r#"<div id="made" hx-swap-oob="innerHTML">{}</div>"#,
        success(said)
    )
}

/// `said` of what was made or removed, as a daisyUI success alert
/// (`docs/specs/web/design_system.md`, Making).
pub(super) fn success(said: &Html) -> Html {
    html::built(format!(
        r#"<div class="alert alert-success" role="status"><span>{said}</span></div>"#
    ))
}

pub(super) use crate::actions::REQUIRED;

/// The name `sent` (`docs/specs/benchmarks.md`, Names and IDs), clean, or
/// else [`REQUIRED`] while it is no name.
pub(super) fn parse_name(sent: &str) -> Result<Name, &'static str> {
    Name::parse(sent).map_err(|_| REQUIRED)
}
