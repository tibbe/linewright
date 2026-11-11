//! The runs page (`docs/specs/design_system.md`, Lists of runs): every run,
//! narrowed by its filters (`docs/specs/design_system.md`, Filters): the
//! item, the stage and the time since which runs started.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::names::Names;
use super::runs_table::RunsShown;
use super::{Frame, Page, events, frame_regions, given, render, unanswered, with_filters};
use crate::api::{self, Sources};
use crate::config::{self, Since};
use crate::domain::{Number, Timestamp};
use crate::phrase::Tone;
use crate::reconcile::WorkKind;
use crate::runs;

/// The filters as the page's address carries them, an empty one leaving the
/// list unnarrowed by it.
#[derive(Deserialize)]
pub(super) struct FilterQuery {
    #[serde(default)]
    item: String,
    #[serde(default)]
    stage: String,
    #[serde(default)]
    since: String,
}

/// The narrowing the filters ask for, read as `runs` reads its own.
struct Narrowing {
    /// Trimmed, to fill the filters in again.
    fields: FilterQuery,
    narrowed: Result<Narrowed, String>,
}

struct Narrowed {
    item: Option<Number>,
    stage: Option<WorkKind>,
    /// Read as `--since` reads it, so that a duration stays one ago.
    since: Option<Since>,
}

impl Narrowing {
    fn of(fields: &FilterQuery) -> Narrowing {
        let fields = FilterQuery {
            item: fields.item.trim().to_string(),
            stage: fields.stage.trim().to_string(),
            since: fields.since.trim().to_string(),
        };
        Narrowing {
            narrowed: Narrowed::of(&fields),
            fields,
        }
    }

    /// The fields given, as the query of `path`.
    fn path(&self, path: &str) -> String {
        let FilterQuery { item, stage, since } = &self.fields;
        with_filters(path, &[("item", item), ("stage", stage), ("since", since)])
    }
}

impl Narrowed {
    fn of(fields: &FilterQuery) -> Result<Narrowed, String> {
        let item = given(&fields.item)
            .map(|item| {
                item.trim_start_matches('#')
                    .parse()
                    .map(Number)
                    .map_err(|_| format!("{item:?} is not an issue or pull request number"))
            })
            .transpose()?;
        let stage = given(&fields.stage)
            .map(str::parse::<WorkKind>)
            .transpose()?;
        let since = given(&fields.since).map(config::parse_since).transpose()?;
        Ok(Narrowed { item, stage, since })
    }

    fn query(&self, now: Timestamp) -> runs::Query {
        runs::Query {
            item: self.item,
            stage: self.stage,
            since: self.since.map(|since| since.at(now)),
            limit: None,
        }
    }
}

/// The filters, filled in as the reader left them.
struct Filters {
    item: String,
    stages: Vec<(&'static str, bool)>,
    since: String,
}

impl Filters {
    fn of(fields: &FilterQuery) -> Filters {
        Filters {
            item: fields.item.clone(),
            stages: WorkKind::ALL
                .into_iter()
                .map(|stage| (stage.name(), fields.stage == stage.name()))
                .collect(),
            since: fields.since.clone(),
        }
    }
}

/// The runs as the narrowing lists them, all of them, or why they are not
/// listed, and the status the page answers with. An item without runs is
/// "No runs", not a page that is not there.
fn listed(sources: &Sources, narrowing: &Narrowing) -> (StatusCode, RunsShown) {
    let now = sources.clock.now();
    let query = match &narrowing.narrowed {
        Ok(narrowed) => narrowed.query(now),
        Err(e) => return (StatusCode::BAD_REQUEST, RunsShown::Unlisted(e.clone())),
    };
    match runs::answer(&sources.work_dir, &query, now) {
        Ok(listing) => {
            let view = api::view(sources);
            let names = Names::of(
                &sources.work_dir.repo,
                view.as_ref().map(|view| &*view.snapshot),
            );
            let runs = RunsShown::all(&listing, &names);
            (StatusCode::OK, runs)
        }
        Err(runs::ListError::NoRuns(_)) => (StatusCode::OK, RunsShown::NoRuns),
        Err(e) => {
            let (status, error) = unanswered(e);
            (status, RunsShown::Unlisted(error))
        }
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "runs.html")]
struct RunsPage {
    page: Page,
    frame: Frame,
    filters: Filters,
    runs: RunsShown,
}

#[derive(Template)]
#[template(path = "runs.html", block = "runs")]
struct RunsRegion<'a> {
    runs: &'a RunsShown,
}

pub(super) async fn runs_page(
    State(sources): State<Sources>,
    Query(fields): Query<FilterQuery>,
) -> Response {
    let narrowing = Narrowing::of(&fields);
    let (status, runs) = listed(&sources, &narrowing);
    let page = RunsPage {
        page: Page::new(
            format!("Runs · {}", sources.work_dir.repo),
            narrowing.path("/events/runs"),
        )
        .filling_the_viewport(),
        frame: Frame::of(&sources),
        filters: Filters::of(&narrowing.fields),
        runs,
    };
    (status, page).into_response()
}

/// The runs page's regions: the frame's and the list, listed again as runs
/// start and end.
pub(super) async fn runs_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Query(fields): Query<FilterQuery>,
) -> Response {
    let narrowing = Narrowing::of(&fields);
    let regions = move || {
        let mut regions = frame_regions(&Frame::of(&sources));
        let (_, runs) = listed(&sources, &narrowing);
        regions.push(("runs", render(&RunsRegion { runs: &runs })));
        regions
    };
    events(regions, stopping)
}
