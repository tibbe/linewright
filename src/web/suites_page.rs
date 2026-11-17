//! The suites page (`docs/specs/design_system.md`, The suites page): a row
//! per suite, and the form that makes one.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::html;
use super::write::{Refused, Write, alert};
use super::{ErrorPage, Frame, Page, events, frame_regions, render, suite_path, suites};
use crate::api::Sources;
use crate::benchmarks::{MakeError, SuiteListed, SuiteName};
use crate::case::ReplayStage;

/// A suite's row.
struct Row {
    name: SuiteName,
    stage: ReplayStage,
    cases: usize,
    href: String,
}

impl From<SuiteListed> for Row {
    fn from(listed: SuiteListed) -> Row {
        Row {
            href: suite_path(&listed.suite.name),
            name: listed.suite.name,
            stage: listed.suite.stage,
            cases: listed.cases,
        }
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "suites.html")]
struct SuitesPage {
    page: Page,
    frame: Frame,
    suites: Vec<Row>,
}

#[derive(Template)]
#[template(path = "suites.html", block = "suites")]
struct SuitesRegion<'a> {
    suites: &'a [Row],
}

/// The suites' rows, by name.
async fn listed(sources: &Sources) -> anyhow::Result<Vec<Row>> {
    let listed = suites(sources).list().await?;
    Ok(listed.into_iter().map(Row::from).collect())
}

pub(super) async fn suites_page(State(sources): State<Sources>) -> Response {
    let page = Page::new(
        format!("Suites · {}", sources.work_dir.repo),
        "/events/suites".into(),
    );
    let frame = Frame::of(&sources).await;
    match listed(&sources).await {
        Ok(suites) => SuitesPage {
            page,
            frame,
            suites,
        }
        .into_response(),
        Err(e) => {
            let error = format!("{e:#}");
            warn!(error, "reading the suites failed");
            let page = ErrorPage {
                page,
                frame,
                region: "suites",
                error,
            };
            (StatusCode::INTERNAL_SERVER_ERROR, page).into_response()
        }
    }
}

/// The suites page's regions: the frame's, and the suites once they read.
pub(super) async fn suites_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
) -> Response {
    let regions = move || {
        let sources = sources.clone();
        async move {
            let mut regions = frame_regions(&Frame::of(&sources).await);
            if let Ok(suites) = listed(&sources).await {
                regions.push(("suites", render(&SuitesRegion { suites: &suites })));
            }
            regions
        }
    };
    events(regions, stopping)
}

/// What the form that makes a suite sends.
#[derive(Deserialize)]
struct MakeForm {
    name: String,
    stage: String,
}

/// Makes the suite the form in `body` names, and answers with what the page
/// says of it. Refused, writing nothing, unless the browser shows the
/// request came from the web UI's own pages.
pub(super) async fn make_suite(
    State(sources): State<Sources>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Refused> {
    const WRITE: Write = Write::MakeSuite;
    WRITE.same_origin(&headers, &uri)?;
    let form: MakeForm = WRITE.read_form(&body)?;
    let unprocessable = |error: String| WRITE.refused(StatusCode::UNPROCESSABLE_ENTITY, &error);
    let name: SuiteName = form.name.parse().map_err(unprocessable)?;
    let stage: ReplayStage = form.stage.parse().map_err(unprocessable)?;
    match suites(&sources).make(&name, stage).await {
        Ok(suite) => {
            info!(suite = %suite.name, stage = %suite.stage, "made a suite");
            let said = format!(
                r#"Made the {} suite <a class="link" href="{}">{}</a>"#,
                html::escape(suite.stage.name()),
                html::escape(&suite_path(&suite.name)),
                html::escape(suite.name.as_str()),
            );
            Ok((StatusCode::OK, alert("alert-success", &said)).into_response())
        }
        Err(e @ MakeError::Taken(_)) => Err(unprocessable(e.to_string())),
        Err(MakeError::Failed(e)) => {
            let why = format!("{e:#}");
            warn!(suite = %name, error = why, "making a suite failed");
            Err(WRITE.refused(StatusCode::INTERNAL_SERVER_ERROR, &why))
        }
    }
}
