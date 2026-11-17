//! Capturing a run as a case from its page (`docs/specs/design_system.md`,
//! The run page, Capturing): the page's form, and the `POST` on the run's
//! path that captures it.

use std::fmt::Write as _;
use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tracing::{info, warn};

use super::html::{self, Html};
use super::write::{Refused, Write, alert};
use super::{run_path, suites};
use crate::api::Sources;
use crate::benchmarks::{SuiteName, Suites};
use crate::case::{
    self, CRITERIA_REQUIRED, CaptureError, CaseName, Criteria, Refusal, ReplayStage, SuiteChoice,
};
use crate::runs::RunId;

/// The run page's offer to capture its run as a case.
pub(super) struct Offer {
    run: RunId,
    /// The suites of the run's stage, by name.
    suites: Vec<SuiteName>,
}

impl Offer {
    /// The offer for run `id`, if it can be captured. A run whose records
    /// do not read is logged and offers none.
    pub(super) async fn of(sources: &Sources, id: &RunId) -> Option<Offer> {
        let run = match case::capturable_run(&sources.runs, id).await {
            Ok(run) => run,
            Err(e) => {
                if e.refusal() == Refusal::Failed {
                    warn!(run = %id, error = e.why(), "reading whether a run can be captured failed");
                }
                return None;
            }
        };
        let suites = match suites_of(&suites(sources), run.stage()).await {
            Ok(suites) => suites,
            Err(e) => {
                warn!(run = %id, error = format!("{e:#}"), "reading the suites failed");
                return None;
            }
        };
        Some(Offer {
            run: run.id().clone(),
            suites,
        })
    }

    /// The form, its suite the first of the run's stage or else a new one,
    /// its name filled in with the suggested one, and where it says what
    /// came of a capture.
    pub(super) fn html(&self, sources: &Sources) -> Html {
        let path = html::escape(&run_path(&self.run));
        let name = html::escape(CaseName::suggested(&self.run).as_str());
        html::built(format!(
            r##"<details class="capture-offer mb-3"><summary class="btn btn-sm">Capture as a case</summary><form class="capture mt-2 flex max-w-xl flex-col gap-2" method="post" action="{path}" hx-post="{path}" hx-target="#capture-said" hx-swap="innerHTML"><label class="select select-sm w-full"><span class="label">suite</span>{select}</label><label class="new-suite-name input input-sm w-full"><span class="label">new suite</span><input name="new-suite" autocomplete="off" placeholder="its name"></label><label class="input input-sm w-full"><span class="label">name</span><input name="name" value="{name}" required pattern="[a-z0-9\-]+" autocomplete="off"></label><textarea class="textarea textarea-sm w-full" name="criteria" rows="4" required placeholder="criteria: what a good result is" aria-label="criteria"></textarea>{ready}</form><div id="capture-said" class="mt-2" role="status"></div></details>"##,
            select = suite_select(&self.suites, None, ""),
            ready = Offer::ready_html(sources),
        ))
    }

    /// The region of an offer's form that changes with whether a capture
    /// can run yet, by its id.
    pub(super) fn region(sources: &Sources) -> (&'static str, String) {
        (READY, Offer::ready_html(sources).to_string())
    }

    pub(super) fn ready_html(sources: &Sources) -> Html {
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

const NEW_SUITE_UNNAMED: &str = "the new suite needs a name";

const NOT_POLLED: &str = "the capture can't run before Linewright's first poll";

/// The names of the suites of `stage`.
async fn suites_of(suites: &Suites, stage: ReplayStage) -> anyhow::Result<Vec<SuiteName>> {
    Ok(suites
        .list()
        .await?
        .into_iter()
        .filter(|listed| listed.suite.stage == stage)
        .map(|listed| listed.suite.name)
        .collect())
}

/// The form's choice of a suite: each of `suites`, with `selected` chosen,
/// or else the first, then a new one; with `attributes` added.
fn suite_select(suites: &[SuiteName], selected: Option<&SuiteName>, attributes: &str) -> String {
    let mut options = String::new();
    for suite in suites {
        let chosen = if Some(suite) == selected {
            " selected"
        } else {
            ""
        };
        let suite = html::escape(suite.as_str());
        let _ = write!(
            options,
            r#"<option value="{suite}"{chosen}>{suite}</option>"#
        );
    }
    format!(
        r#"<select id="{SUITE_SELECT}" name="suite"{attributes}>{options}<option class="new-suite" value="">a new suite</option></select>"#
    )
}

const SUITE_SELECT: &str = "capture-suite";

/// What the form sends.
#[derive(Deserialize)]
struct CaptureForm {
    /// Empty for a new suite.
    #[serde(default)]
    suite: String,
    #[serde(default, rename = "new-suite")]
    new_suite: String,
    #[serde(flatten)]
    case: CaseFields,
}

/// What a capture's form sends of the case.
#[derive(Deserialize)]
pub(super) struct CaseFields {
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
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Refused> {
    Write::Capture.same_origin(&headers, &uri)?;
    let run: RunId = id
        .parse()
        .map_err(|error: String| Write::Capture.refused(StatusCode::NOT_FOUND, &error))?;
    let form: CaptureForm = Write::Capture.read_form(&body)?;
    let choice = match (form.suite.as_str(), form.new_suite.as_str()) {
        ("", "") => Err(NEW_SUITE_UNNAMED.to_string()),
        ("", new) => new.parse().map(SuiteChoice::New),
        (named, _) => named.parse().map(SuiteChoice::Named),
    }
    .map_err(|error| Write::Capture.refused(StatusCode::UNPROCESSABLE_ENTITY, &error))?;
    let captured = capture_run(&sources, &run, &choice, &form.case).await?;
    let mut answer = captured.said(&run, &choice).0;
    match suites_of(&suites(&sources), captured.stage).await {
        Ok(names) => {
            answer.push_str(&suite_select(
                &names,
                Some(choice.suite()),
                r#" hx-swap-oob="true""#,
            ));
        }
        Err(e) => warn!(error = format!("{e:#}"), "reading the suites failed"),
    }
    Ok((StatusCode::OK, axum::response::Html(answer)).into_response())
}

/// A run captured as a case.
pub(super) struct Captured {
    /// The case's directory.
    dir: PathBuf,
    /// The run's stage.
    stage: ReplayStage,
}

impl Captured {
    /// What the page says of `run` captured as it, into `choice`.
    pub(super) fn said(&self, run: &RunId, choice: &SuiteChoice) -> axum::response::Html<String> {
        let new = match choice {
            SuiteChoice::Named(_) => "",
            SuiteChoice::New(_) => "new ",
        };
        let said = format!(
            "Captured {} as the case <code>{}</code> in the {new}suite {}",
            html::escape(&run.to_string()),
            html::escape(&self.dir.display().to_string()),
            html::escape(choice.suite().as_str()),
        );
        alert("alert-success", &said)
    }
}

/// Captures `run` as the case `case` names into the suite `choice`, with
/// the store `serve` holds; or else says why it did not.
pub(super) async fn capture_run(
    sources: &Sources,
    run: &RunId,
    choice: &SuiteChoice,
    case: &CaseFields,
) -> Result<Captured, Refused> {
    let not_captured = |status, why: &str| Write::Capture.refused(status, why);
    let name: CaseName = case
        .name
        .parse()
        .map_err(|error: String| not_captured(StatusCode::UNPROCESSABLE_ENTITY, &error))?;
    let criteria = Criteria::parse(&case.criteria)
        .ok_or_else(|| not_captured(StatusCode::UNPROCESSABLE_ENTITY, CRITERIA_REQUIRED))?;
    let served = sources
        .served
        .borrow()
        .clone()
        .ok_or_else(|| not_captured(StatusCode::SERVICE_UNAVAILABLE, NOT_POLLED))?;
    let suites = suites(sources);
    let captured = match case::capturable(&sources.runs, &suites, run, &name, choice).await {
        Ok(capturable) => {
            let stage = capturable.stage();
            case::capture(served.store(), capturable, &criteria, &suites)
                .await
                .map(|dir| Captured { dir, stage })
        }
        Err(e) => Err(e),
    };
    match captured {
        Ok(captured) => {
            info!(run = %run, case = %captured.dir.display(), suite = %choice.suite(), "captured a run as a case");
            Ok(captured)
        }
        Err(e) => {
            let why = served.redaction().apply(&e.why());
            warn!(run = %run, case = %name, error = why, "a capture failed");
            Err(not_captured(status_of(&e), &why))
        }
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
