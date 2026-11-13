//! Capturing a run as a case from its page (`docs/specs/design_system.md`,
//! The run page, Capturing): the page's form, and the `POST` on the run's
//! path that captures it.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tracing::{info, warn};

use super::html::{self, Html};
use super::run_path;
use crate::api::Sources;
use crate::case::{self, CaptureError, CaseName, Refusal};
use crate::runs::{RunId, Runs};

/// The run page's offer to capture its run as a case.
pub(super) struct Offer {
    run: RunId,
}

impl Offer {
    /// The offer for run `id`, if it can be captured. A run whose records
    /// do not read is logged and offers none.
    pub(super) fn of(sources: &Sources, id: &RunId) -> Option<Offer> {
        let runs = Runs::new(&sources.work_dir.resolve());
        match case::capturable_run(&runs, id) {
            Ok(run) => Some(Offer {
                run: run.id().clone(),
            }),
            Err(e) => {
                if e.refusal() == Refusal::Failed {
                    warn!(run = %id, error = e.why(), "reading whether a run can be captured failed");
                }
                None
            }
        }
    }

    /// The form, its name filled in with the suggested one, and where it
    /// says what came of a capture.
    pub(super) fn html(&self, sources: &Sources) -> Html {
        let path = html::escape(&run_path(&self.run));
        let name = html::escape(CaseName::suggested(&self.run).as_str());
        html::built(format!(
            r##"<details class="capture-offer mb-3"><summary class="btn btn-sm">Capture as a case</summary><form class="capture mt-2 flex max-w-xl flex-col gap-2" method="post" action="{path}" hx-post="{path}" hx-target="#capture-said" hx-swap="innerHTML"><label class="input input-sm w-full"><span class="label">name</span><input name="name" value="{name}" required pattern="[a-z0-9\-]+" autocomplete="off"></label><textarea class="textarea textarea-sm w-full" name="criteria" rows="4" placeholder="criteria: what a good result is" aria-label="criteria"></textarea>{ready}</form><div id="capture-said" class="mt-2" role="status"></div></details>"##,
            ready = Offer::ready_html(sources),
        ))
    }

    /// The region of an offer's form that changes with whether a capture
    /// can run yet, by its id.
    pub(super) fn region(sources: &Sources) -> (&'static str, String) {
        (READY, Offer::ready_html(sources).to_string())
    }

    fn ready_html(sources: &Sources) -> Html {
        let polled = sources.served.borrow().is_some();
        let (disabled, note) = if polled {
            ("", String::new())
        } else {
            (
                " disabled",
                format!(r#"<span class="text-muted text-sm">{NOT_POLLED}</span>"#),
            )
        };
        html::built(format!(
            r#"<div id="{READY}" class="flex items-center gap-2"><button class="btn btn-sm btn-primary" type="submit"{disabled}>Capture</button>{note}</div>"#
        ))
    }
}

const READY: &str = "capture-ready";

const NOT_POLLED: &str = "the capture can't run before Linewright's first poll";

/// What the form sends.
#[derive(Deserialize)]
struct CaptureForm {
    name: String,
    #[serde(default)]
    criteria: String,
}

/// Captures run `id` as the case the form in `body` names, with the store
/// `serve` holds, and answers with what the page says of it: where the case
/// was written or why it was not. Refused, writing nothing, unless the
/// browser shows the request came from the web UI's own pages.
pub(super) async fn capture(
    State(sources): State<Sources>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(reason) = not_same_origin(&headers) {
        warn!(run = id, reason, "refused a capture");
        return (
            StatusCode::FORBIDDEN,
            "a capture is taken only from the web UI's own pages",
        )
            .into_response();
    }
    let run: RunId = match id.parse() {
        Ok(run) => run,
        Err(error) => return (StatusCode::NOT_FOUND, error).into_response(),
    };
    let form: CaptureForm = match serde_urlencoded::from_bytes(&body) {
        Ok(form) => form,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("the form does not read: {e}"),
            )
                .into_response();
        }
    };
    let name: CaseName = match form.name.parse() {
        Ok(name) => name,
        Err(error) => return said_not(StatusCode::UNPROCESSABLE_ENTITY, &error),
    };
    let criteria = criteria_given(&form.criteria);
    let Some(served) = sources.served.borrow().clone() else {
        return said_not(StatusCode::SERVICE_UNAVAILABLE, NOT_POLLED);
    };
    let work_dir = sources.work_dir.resolve();
    let captured = match case::capturable(
        &Runs::new(&work_dir),
        &work_dir.join(case::CASES),
        &run,
        &name,
    ) {
        Ok(capturable) => case::capture(served.store(), capturable, criteria).await,
        Err(e) => Err(e),
    };
    match captured {
        Ok(dir) => {
            info!(run = %run, case = %dir.display(), "captured a run as a case");
            let said = format!(
                "Captured {} as the case <code>{}</code>",
                html::escape(&run.to_string()),
                html::escape(&dir.display().to_string()),
            );
            (StatusCode::OK, alert("alert-success", &said)).into_response()
        }
        Err(e) => {
            let why = served.redaction().apply(&e.why());
            warn!(run = %run, case = %name, error = why, "a capture failed");
            said_not(status_of(&e), &why)
        }
    }
}

/// The criteria the form's `criteria` field gives: none when it is blank.
fn criteria_given(field: &str) -> Option<&str> {
    Some(field).filter(|field| !field.trim().is_empty())
}

/// The status a capture that did not happen for `e` is answered with.
fn status_of(e: &CaptureError) -> StatusCode {
    match e.refusal() {
        Refusal::NoSuchRun => StatusCode::NOT_FOUND,
        Refusal::Refused => StatusCode::UNPROCESSABLE_ENTITY,
        Refusal::Failed => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// What the page says of a capture that did not happen, for `why`.
fn said_not(status: StatusCode, why: &str) -> Response {
    let said = format!("Not captured: {}", html::escape(why));
    (status, alert("alert-error", &said)).into_response()
}

/// `said`, in an alert of `kind`.
fn alert(kind: &str, said: &str) -> axum::response::Html<String> {
    axum::response::Html(format!(r#"<p class="alert alert-soft {kind}">{said}</p>"#))
}

/// Why `headers` do not show a request came from a page of the same origin,
/// if they don't: the browser's `Sec-Fetch-Site`, or where it lacks one, an
/// `Origin` whose host and port are the `Host`'s.
/// <https://www.w3.org/TR/fetch-metadata/#sec-fetch-site-header>,
/// <https://www.rfc-editor.org/rfc/rfc6454#section-7>
fn not_same_origin(headers: &HeaderMap) -> Option<&'static str> {
    if let Some(site) = headers.get(SEC_FETCH_SITE) {
        return (site != "same-origin").then_some("its Sec-Fetch-Site is not same-origin");
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Some("it has neither Sec-Fetch-Site nor Origin");
    };
    let host = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok());
    let authority = origin
        .to_str()
        .ok()
        .and_then(|origin| origin.split_once("://"))
        .filter(|(scheme, _)| matches!(*scheme, "http" | "https"))
        .map(|(_, authority)| authority);
    match (authority, host) {
        (Some(authority), Some(host)) if authority.eq_ignore_ascii_case(host) => None,
        _ => Some("its Origin is not its Host"),
    }
}

const SEC_FETCH_SITE: &str = "sec-fetch-site";

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    header::HeaderName::from_static(name),
                    header::HeaderValue::from_static(value),
                )
            })
            .collect()
    }

    #[test_log::test]
    fn a_request_is_same_origin_by_its_sec_fetch_site_or_its_origin_and_host() {
        let same = [
            vec![
                ("sec-fetch-site", "same-origin"),
                ("host", "localhost:8080"),
            ],
            vec![
                ("sec-fetch-site", "same-origin"),
                ("origin", "https://lw.example.com"),
                ("host", "127.0.0.1:8080"),
            ],
            vec![
                ("origin", "http://localhost:8080"),
                ("host", "localhost:8080"),
            ],
            vec![
                ("origin", "https://LW.example.com"),
                ("host", "lw.example.com"),
            ],
        ];
        for pairs in same {
            assert_eq!(not_same_origin(&headers(&pairs)), None, "{pairs:?}");
        }
        let not = [
            vec![("sec-fetch-site", "cross-site"), ("host", "localhost:8080")],
            vec![("sec-fetch-site", "same-site"), ("host", "localhost:8080")],
            vec![("sec-fetch-site", "none"), ("host", "localhost:8080")],
            vec![
                ("sec-fetch-site", "cross-site"),
                ("origin", "http://localhost:8080"),
                ("host", "localhost:8080"),
            ],
            vec![
                ("origin", "http://evil.example"),
                ("host", "localhost:8080"),
            ],
            vec![
                ("origin", "http://localhost:9090"),
                ("host", "localhost:8080"),
            ],
            vec![("origin", "http://localhost"), ("host", "localhost:8080")],
            vec![("origin", "null"), ("host", "localhost:8080")],
            vec![("origin", "http://localhost:8080")],
            vec![("host", "localhost:8080")],
        ];
        for pairs in not {
            assert!(not_same_origin(&headers(&pairs)).is_some(), "{pairs:?}");
        }
    }
}
