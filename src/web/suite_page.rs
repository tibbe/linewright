//! The suite page (`docs/specs/design_system.md`, The suite page): suite
//! NAME's cases, each offering to remove it, and the form that captures a
//! run of the suite's stage into it.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::capture::{CaseFields, Offer, capture_run};
use super::html::{self, Html};
use super::write::{Refused, Write, alert};
use super::{ErrorPage, Frame, Page, events, frame_regions, render, run_path, suite_path, suites};
use crate::api::Sources;
use crate::benchmarks::{CaseEntry, RemoveError, Suite, SuiteName, SuiteShown};
use crate::case::{self, CaseName, Refusal, ReplayStage, SuiteChoice};
use crate::runs::{Recorded, RunId};

/// A case's row.
struct CaseRow {
    name: CaseName,
    run: RunId,
    run_href: String,
    criteria: CriteriaShown,
}

/// A case's criteria, as its row shows them.
enum CriteriaShown {
    Read {
        /// Their first line that holds more than white space.
        first_line: String,
        text: String,
    },
    /// Why they do not read.
    Unreadable(String),
}

impl From<CaseEntry> for CaseRow {
    fn from(case: CaseEntry) -> CaseRow {
        CaseRow {
            run_href: run_path(&case.run),
            name: case.name,
            run: case.run,
            criteria: match case.criteria {
                Ok(criteria) => CriteriaShown::Read {
                    first_line: criteria
                        .as_str()
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or_default()
                        .to_string(),
                    text: criteria.as_str().to_string(),
                },
                Err(refused) => CriteriaShown::Unreadable(refused.why),
            },
        }
    }
}

/// A run the form offers to capture.
struct RunChoice {
    id: RunId,
    /// The name the form suggests for its case.
    name: CaseName,
}

#[derive(Template, WebTemplate)]
#[template(path = "suite.html")]
struct SuitePage {
    page: Page,
    frame: Frame,
    suite: Suite,
    path: String,
    cases: Vec<CaseRow>,
    /// The runs of the suite's stage that can be captured, latest first.
    runs: Vec<RunChoice>,
    ready: Html,
}

#[derive(Template)]
#[template(path = "suite.html", block = "cases")]
struct CasesRegion<'a> {
    path: &'a str,
    cases: &'a [CaseRow],
}

#[derive(Template)]
#[template(path = "suite.html", block = "runs")]
struct RunsRegion<'a> {
    suite: &'a Suite,
    runs: &'a [RunChoice],
}

/// Why there is no suite page: no such suite, or one that does not read.
enum Unshown {
    NoSuch(String),
    Failed(anyhow::Error),
}

/// What is said of a suite named `name` that there is none of.
fn no_such(name: &str) -> String {
    format!("there is no suite {name}")
}

/// Suite `name` with its cases, by name.
async fn shown(sources: &Sources, name: &str) -> Result<(Suite, Vec<CaseRow>), Unshown> {
    let parsed: SuiteName = name.parse().map_err(|_| Unshown::NoSuch(no_such(name)))?;
    match suites(sources).show(&parsed).await {
        Ok(Some(SuiteShown { suite, cases, .. })) => {
            Ok((suite, cases.into_iter().map(CaseRow::from).collect()))
        }
        Ok(None) => Err(Unshown::NoSuch(no_such(name))),
        Err(e) => Err(Unshown::Failed(e)),
    }
}

/// Of `recorded`, the runs of `stage`, latest first.
fn of_stage(recorded: Vec<Recorded>, stage: ReplayStage) -> Vec<RunId> {
    let mut runs: Vec<Recorded> = recorded
        .into_iter()
        .filter(|run| case::capturable_stage(&run.id.of).is_some_and(|(of, _)| of == stage))
        .collect();
    runs.sort_by_key(|run| std::cmp::Reverse(run.started_at));
    runs.into_iter().map(|run| run.id).collect()
}

/// The runs of `stage` that can be captured, latest first. Runs that do not
/// read are logged and left out.
async fn runs_of(sources: &Sources, stage: ReplayStage) -> Vec<RunChoice> {
    let recorded = match sources.runs.read().await {
        Ok(recorded) => recorded,
        Err(e) => {
            warn!(error = format!("{e:#}"), "reading the runs failed");
            return vec![];
        }
    };
    let mut runs = vec![];
    for id in of_stage(recorded, stage) {
        match case::capturable_run(&sources.runs, &id).await {
            Ok(_) => runs.push(RunChoice {
                name: CaseName::suggested(&id),
                id,
            }),
            Err(e) if e.refusal() == Refusal::Failed => {
                warn!(run = %id, error = e.why(), "reading whether a run can be captured failed");
            }
            Err(_) => {}
        }
    }
    runs
}

pub(super) async fn suite_page(
    State(sources): State<Sources>,
    Path(name): Path<String>,
) -> Response {
    let title = format!("{name} · {}", sources.work_dir.repo);
    let frame = Frame::of(&sources).await;
    match shown(&sources, &name).await {
        Ok((suite, cases)) => {
            let path = suite_path(&suite.name);
            SuitePage {
                page: Page::new(title, format!("/events{path}")),
                frame,
                runs: runs_of(&sources, suite.stage).await,
                path,
                suite,
                cases,
                ready: Offer::ready_html(&sources),
            }
            .into_response()
        }
        Err(unshown) => {
            let (status, error) = match unshown {
                Unshown::NoSuch(error) => (StatusCode::NOT_FOUND, error),
                Unshown::Failed(e) => {
                    let error = format!("{e:#}");
                    warn!(suite = name, error, "reading a suite failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, error)
                }
            };
            let page = ErrorPage {
                page: Page::new(title, "/events/suites".into()),
                frame,
                region: "cases",
                error,
            };
            (status, page).into_response()
        }
    }
}

/// The suite page's regions: the frame's, and once the suite reads, its
/// cases, the runs the form offers and whether a capture can run yet.
pub(super) async fn suite_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Path(name): Path<String>,
) -> Response {
    let regions = move || {
        let sources = sources.clone();
        let name = name.clone();
        async move {
            let mut regions = frame_regions(&Frame::of(&sources).await);
            if let Ok((suite, cases)) = shown(&sources, &name).await {
                regions.push((
                    "cases",
                    render(&CasesRegion {
                        path: &suite_path(&suite.name),
                        cases: &cases,
                    }),
                ));
                let runs = runs_of(&sources, suite.stage).await;
                regions.push((
                    "capture-runs",
                    render(&RunsRegion {
                        suite: &suite,
                        runs: &runs,
                    }),
                ));
                regions.push(Offer::region(&sources));
            }
            regions
        }
    };
    events(regions, stopping)
}

/// Suite `name` of a write's path, or else what the page says of there
/// being none.
fn suite_of_path(write: Write, name: &str) -> Result<SuiteName, Refused> {
    name.parse()
        .map_err(|_| write.refused(StatusCode::NOT_FOUND, &no_such(name)))
}

/// What the form that adds a case sends.
#[derive(Deserialize)]
struct AddForm {
    run: String,
    #[serde(flatten)]
    case: CaseFields,
}

/// Captures the run the form in `body` names into suite `name`, and answers
/// with what the page says of it. Refused, writing nothing, unless the
/// browser shows the request came from the web UI's own pages.
pub(super) async fn add_case(
    State(sources): State<Sources>,
    Path(name): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Refused> {
    const WRITE: Write = Write::Capture;
    WRITE.same_origin(&headers, &uri)?;
    let suite = suite_of_path(WRITE, &name)?;
    let form: AddForm = WRITE.read_form(&body)?;
    let run: RunId = form
        .run
        .parse()
        .map_err(|error: String| WRITE.refused(StatusCode::UNPROCESSABLE_ENTITY, &error))?;
    let choice = SuiteChoice::Named(suite);
    let captured = capture_run(&sources, &run, &choice, &form.case).await?;
    Ok((StatusCode::OK, captured.said(&run, &choice)).into_response())
}

/// What the form that removes a case sends.
#[derive(Deserialize)]
struct RemoveForm {
    case: String,
}

/// Removes the case the form in `body` names from suite `name`, and answers
/// with what the page says of it. Refused, removing nothing, unless the
/// browser shows the request came from the web UI's own pages.
pub(super) async fn remove_case(
    State(sources): State<Sources>,
    Path(name): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Refused> {
    const WRITE: Write = Write::RemoveCase;
    WRITE.same_origin(&headers, &uri)?;
    let suite = suite_of_path(WRITE, &name)?;
    let form: RemoveForm = WRITE.read_form(&body)?;
    let case: CaseName = form
        .case
        .parse()
        .map_err(|error: String| WRITE.refused(StatusCode::UNPROCESSABLE_ENTITY, &error))?;
    match suites(&sources).remove_case(&suite, &case).await {
        Ok(()) => {
            info!(suite = %suite, case = %case, "removed a case");
            let said = format!("Removed the case {}", html::escape(case.as_str()));
            Ok((StatusCode::OK, alert("alert-success", &said)).into_response())
        }
        Err(e @ (RemoveError::NoSuchSuite(_) | RemoveError::NoSuchCase { .. })) => {
            Err(WRITE.refused(StatusCode::NOT_FOUND, &e.to_string()))
        }
        Err(RemoveError::Failed(e)) => {
            let why = format!("{e:#}");
            warn!(suite = %suite, case = %case, error = why, "removing a case failed");
            Err(WRITE.refused(StatusCode::INTERNAL_SERVER_ERROR, &why))
        }
    }
}
