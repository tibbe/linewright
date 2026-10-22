//! The runs page (`docs/specs/design_system.md`, The web UI): what `runs`
//! lists, with its narrowing as the fields of a form.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::runs_table::RunsShown;
use super::{Frame, Page, events, frame_regions, render, unanswered, with_query};
use crate::api::{self, Sources};
use crate::config::{self, Since};
use crate::domain::{Number, Timestamp};
use crate::phrase::Tone;
use crate::reconcile::WorkKind;
use crate::runs;

/// The form's fields as the reader filled them in, an empty one leaving the
/// list unnarrowed by it.
#[derive(Deserialize)]
pub(super) struct Fields {
    #[serde(default)]
    item: String,
    #[serde(default)]
    stage: String,
    #[serde(default)]
    since: String,
    #[serde(default)]
    limit: String,
}

/// The narrowing the form asks for, read as `runs` reads its own.
struct Narrowing {
    /// Trimmed, to fill the form in again.
    fields: Fields,
    narrowed: Result<Narrowed, String>,
}

struct Narrowed {
    item: Option<Number>,
    stage: Option<WorkKind>,
    /// Read as `--since` reads it, so that a duration stays one ago.
    since: Option<Since>,
    limit: Option<usize>,
}

impl Narrowing {
    fn of(fields: &Fields) -> Narrowing {
        let fields = Fields {
            item: fields.item.trim().to_string(),
            stage: fields.stage.trim().to_string(),
            since: fields.since.trim().to_string(),
            limit: fields.limit.trim().to_string(),
        };
        Narrowing {
            narrowed: Narrowed::of(&fields),
            fields,
        }
    }

    /// The fields given, with or without the limit, as the query of `path`.
    fn path(&self, path: &str, limited: bool) -> String {
        let mut query = form_urlencoded::Serializer::new(String::new());
        let Fields {
            item,
            stage,
            since,
            limit,
        } = &self.fields;
        for (name, value) in [("item", item), ("stage", stage), ("since", since)]
            .into_iter()
            .chain(limited.then_some(("limit", limit)))
        {
            if !value.is_empty() {
                query.append_pair(name, value);
            }
        }
        with_query(path, &query.finish())
    }
}

fn given(field: &str) -> Option<&str> {
    Some(field).filter(|field| !field.is_empty())
}

impl Narrowed {
    fn of(fields: &Fields) -> Result<Narrowed, String> {
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
        let limit = given(&fields.limit)
            .map(|limit| {
                limit
                    .parse()
                    .map_err(|_| format!("{limit:?} is not a number of runs"))
            })
            .transpose()?;
        Ok(Narrowed {
            item,
            stage,
            since,
            limit,
        })
    }

    fn query(&self, now: Timestamp) -> runs::Query {
        runs::Query {
            item: self.item,
            stage: self.stage,
            since: self.since.map(|since| since.at(now)),
            limit: self.limit,
        }
    }
}

/// The form, filled in as the reader left it.
struct Form {
    item: String,
    stages: Vec<(&'static str, bool)>,
    since: String,
    limit: String,
}

impl Form {
    fn of(fields: &Fields) -> Form {
        Form {
            item: fields.item.clone(),
            stages: WorkKind::ALL
                .into_iter()
                .map(|stage| (stage.name(), fields.stage == stage.name()))
                .collect(),
            since: fields.since.clone(),
            limit: fields.limit.clone(),
        }
    }
}

/// The runs as the narrowing lists them, or why they are not listed, and
/// the status the page answers with.
fn listed(sources: &Sources, narrowing: &Narrowing) -> (StatusCode, RunsShown) {
    let now = sources.clock.now();
    let query = match &narrowing.narrowed {
        Ok(narrowed) => narrowed.query(now),
        Err(e) => return (StatusCode::BAD_REQUEST, RunsShown::Unlisted(e.clone())),
    };
    match runs::answer(&sources.work_dir, &query, now) {
        Ok(listing) => {
            let view = api::view(sources);
            let runs = RunsShown::all(
                &listing,
                view.as_ref().map(|view| &*view.snapshot),
                &sources.work_dir.repo,
                narrowing.path("/runs", false),
            );
            (StatusCode::OK, runs)
        }
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
    form: Form,
    runs: RunsShown,
}

#[derive(Template)]
#[template(path = "runs.html", block = "runs")]
struct RunsRegion<'a> {
    runs: &'a RunsShown,
}

pub(super) async fn runs_page(
    State(sources): State<Sources>,
    Query(fields): Query<Fields>,
) -> Response {
    let narrowing = Narrowing::of(&fields);
    let (status, runs) = listed(&sources, &narrowing);
    let page = RunsPage {
        page: Page::new(
            format!("Runs · {}", sources.work_dir.repo),
            narrowing.path("/events/runs", true),
        )
        .filling_the_viewport(),
        frame: Frame::of(&sources),
        form: Form::of(&narrowing.fields),
        runs,
    };
    (status, page).into_response()
}

/// The runs page's regions: the frame's and the list, listed again as runs
/// start and end.
pub(super) async fn runs_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Query(fields): Query<Fields>,
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
