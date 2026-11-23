//! The web UI's writes: capturing a run, making a benchmark, changing a
//! benchmark, a task or a configuration, removing a task from its benchmark,
//! and launching and cancelling a benchmark run. Each is refused
//! without a permit of its action, such as for a request that did not come
//! from the web UI's own pages, and answered with what the page says of it.

use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use tracing::warn;

use super::caller::{Asking, NotPermitted};
use super::html;
use crate::access::{Act, Permit};
use crate::actions::Action;

/// A write the web UI takes.
#[derive(Clone, Copy, Debug)]
pub(super) enum Write {
    Capture,
    MakeBenchmark,
    /// Changing a thing in the dialog that "Edit" opens.
    Edit,
    RemoveTask,
    Launch,
    Cancel,
}

impl Write {
    /// How the page says it did not happen, as in "Not captured: why".
    fn not_done(self) -> &'static str {
        match self {
            Write::Capture => "Not captured",
            Write::MakeBenchmark => "Not made",
            Write::Edit => "Not saved",
            Write::RemoveTask => "Not removed",
            Write::Launch => "Not launched",
            Write::Cancel => "Not cancelled",
        }
    }

    /// It, as a noun, as in "a capture is taken only from …".
    fn noun(self) -> &'static str {
        match self {
            Write::Capture => "a capture",
            Write::MakeBenchmark => "a new benchmark",
            Write::Edit => "an edit",
            Write::RemoveTask => "a task's removal",
            Write::Launch => "a launch",
            Write::Cancel => "a cancel",
        }
    }

    /// It not happening, for `why`.
    pub(super) fn refused(self, status: StatusCode, why: &str) -> Refused {
        Refused {
            write: self,
            status,
            why: why.to_string(),
        }
    }

    /// The permit `asking` has to take it as `action` on the path `uri`, or
    /// else it refused: for a request that did not come from the web UI's
    /// own pages, logged with the path, and for a caller whose role does not
    /// reach the action's level.
    pub(super) fn permit(
        self,
        action: Action,
        asking: &Asking,
        uri: &Uri,
    ) -> Result<Permit<Act>, Refused> {
        asking.permit(action, uri.path()).map_err(|not| match not {
            NotPermitted::CrossSite(reason) => {
                warn!(path = %uri.path(), reason, "refused {}", self.noun());
                self.refused(
                    StatusCode::FORBIDDEN,
                    &format!("{} is taken only from the web UI's own pages", self.noun()),
                )
            }
            NotPermitted::NeedsLevel(needs) => {
                self.refused(StatusCode::FORBIDDEN, &needs.to_string())
            }
        })
    }

    /// As [`Write::permit`], with what `sent` holds, or else its refusal when
    /// the permit is given.
    pub(super) fn permit_with<T>(
        self,
        action: Action,
        asking: &Asking,
        uri: &Uri,
        sent: Result<T, Refused>,
    ) -> Result<(Permit<Act>, T), Refused> {
        let permit = self.permit(action, asking, uri)?;
        Ok((permit, sent?))
    }

    /// The form `body` holds, or else what the page says of it not reading.
    pub(super) fn read_form<T: serde::de::DeserializeOwned>(
        self,
        body: &[u8],
    ) -> Result<T, Refused> {
        serde_urlencoded::from_bytes(body).map_err(|e| {
            self.refused(
                StatusCode::BAD_REQUEST,
                &format!("the form does not read: {e}"),
            )
        })
    }
}

/// A write that did not happen: the status it is answered with, and why.
pub(super) struct Refused {
    write: Write,
    status: StatusCode,
    why: String,
}

impl Refused {
    /// The status it is answered with, and why.
    pub(super) fn into_parts(self) -> (StatusCode, String) {
        (self.status, self.why)
    }
}

/// What the page says of it, as in "Not captured: why", in an alert.
impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        let said = format!("{}: {}", self.write.not_done(), html::escape(&self.why));
        (self.status, alert("alert-error", &said)).into_response()
    }
}

/// `said`, in an alert of `kind`.
pub(super) fn alert(kind: &str, said: &str) -> axum::response::Html<String> {
    axum::response::Html(format!(
        r#"<p class="alert {kind}"><span>{said}</span></p>"#
    ))
}
