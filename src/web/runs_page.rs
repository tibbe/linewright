//! The runs page (`docs/specs/runs.md`, Runs): every run,
//! narrowed by its filters (`docs/specs/design_system.md`, Filters): the
//! item, the stage and the time since which runs started.

use std::sync::Arc;

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::caller::Reader;
use super::names::Names;
use super::pages::{PageHref, Requested};
use super::runs_table::{RunsRegion, RunsShown};
use super::shell::{self, Area, Frame, Navigation, Place};
use super::{Page, events, given, render, unanswered, with_filters};
use crate::access::{Permit, Read};
use crate::actions;
use crate::api::Sources;
use crate::config::{self, Since};
use crate::domain::{Number, Timestamp};
use crate::reconcile::WorkKind;
use crate::runs;

/// What the page's address carries: the filters and the page of the table.
#[derive(Deserialize)]
pub(super) struct RunsQuery {
    #[serde(flatten)]
    filters: FilterQuery,
    #[serde(default)]
    page: Requested,
}

/// The filters as the page's address carries them, an empty one leaving the
/// list unnarrowed by it.
#[derive(Deserialize)]
struct FilterQuery {
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
    /// The page of the table the address asks for.
    page: Requested,
}

struct Narrowed {
    item: Option<Number>,
    stage: Option<WorkKind>,
    /// Read as `--since` reads it, so that a duration stays one ago.
    since: Option<Since>,
}

impl Narrowing {
    fn of(query: &RunsQuery) -> Narrowing {
        let fields = &query.filters;
        let fields = FilterQuery {
            item: fields.item.trim().to_string(),
            stage: fields.stage.trim().to_string(),
            since: fields.since.trim().to_string(),
        };
        Narrowing {
            narrowed: Narrowed::of(&fields),
            fields,
            page: query.page,
        }
    }

    /// The fields given and the page `page`, as the query of `path`.
    fn path(&self, path: &str, page: Requested) -> String {
        let FilterQuery { item, stage, since } = &self.fields;
        with_filters(
            path,
            &[
                ("item", item),
                ("stage", stage),
                ("since", since),
                ("page", &page.field()),
            ],
        )
    }

    /// Where the link to page `page` of the table leads.
    fn href(&self, page: Requested) -> PageHref {
        PageHref::at(self.path("/runs", page))
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
async fn listed(
    permit: &Permit<Read>,
    sources: &Sources,
    narrowing: &Narrowing,
) -> (StatusCode, RunsShown) {
    let now = sources.clock.now();
    let query = match &narrowing.narrowed {
        Ok(narrowed) => narrowed.query(now),
        Err(e) => return (StatusCode::BAD_REQUEST, RunsShown::Unlisted(e.clone())),
    };
    match actions::list_runs(permit, sources, &query, now).await {
        Ok(listing) => {
            let snapshot = actions::view_snapshot(permit, sources);
            let names = Names::of(&sources.work_dir.repo, snapshot.as_deref());
            let runs = RunsShown::all(&listing, &names, narrowing.page, |page| {
                narrowing.href(page)
            });
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

/// The runs page, its stream's narrowed as `narrowing` says.
fn page(narrowing: &Narrowing, navigation: Navigation) -> Page {
    Page::new(
        Place::Navigation(Area::Runs),
        narrowing.path("/events/runs", narrowing.page),
        navigation,
    )
    .filling_the_viewport()
}

pub(super) async fn runs_page(
    Reader { permit, sources }: Reader,
    Query(query): Query<RunsQuery>,
    navigation: Navigation,
) -> Response {
    let narrowing = Narrowing::of(&query);
    let (status, runs) = listed(&permit, &sources, &narrowing).await;
    let page = RunsPage {
        page: page(&narrowing, navigation),
        frame: Frame::of(&permit, &sources).await,
        filters: Filters::of(&narrowing.fields),
        runs,
    };
    (status, page).into_response()
}

/// The runs page's regions: the shell's and the list, listed again as runs
/// start and end.
pub(super) async fn runs_events(
    Reader { permit, sources }: Reader,
    watch: super::caller::Watch,
    Query(query): Query<RunsQuery>,
    navigation: Navigation,
) -> Response {
    let narrowing = Arc::new(Narrowing::of(&query));
    let regions = move || {
        let (permit, sources) = (permit.clone(), sources.clone());
        let narrowing = narrowing.clone();
        async move {
            let frame = Frame::of(&permit, &sources).await;
            let mut regions = shell::regions(&frame, &page(&narrowing, navigation));
            let (_, runs) = listed(&permit, &sources, &narrowing).await;
            regions.push(("runs", render(&RunsRegion { runs: &runs })));
            regions
        }
    };
    events(regions, watch)
}
