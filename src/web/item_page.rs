//! The item page (`docs/specs/design_system.md`, The web UI): what `explain`
//! shows of an item.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use tokio_util::sync::CancellationToken;

use super::html::{self, Html};
use super::runs_table::RunsShown;
use super::{ErrorPage, Frame, Page, events, frame_regions, render};
use crate::api::{self, Sources, Unexplained};
use crate::domain::Number;
use crate::explain::{self, ItemLine};
use crate::phrase::{Phrase, Tone};
use crate::runs;
use crate::status;

/// What `explain` shows of an item, ready for the page.
struct ItemShown {
    heading: Html,
    title: String,
    lines: Vec<LineShown>,
}

/// A line as `explain` prints it, with the tone of what it says.
enum LineShown {
    /// With the tone of the state it names, if any, by its words.
    Fact {
        label: &'static str,
        tone: Option<Tone>,
        phrase: Html,
    },
    /// A wait, whose label is its status.
    Status {
        label: &'static str,
        tone: Tone,
        phrase: Html,
    },
}

impl ItemShown {
    /// Its runs are left to [`item_runs`], to show as a table.
    fn of(answer: &explain::Answer, repo: &str) -> ItemShown {
        let html = |phrase: &Phrase| html::phrase(phrase, repo);
        let lines = answer
            .lines_without_runs()
            .iter()
            .map(|line| {
                let label = line.label();
                match line {
                    ItemLine::Fact { phrase, tone, .. } => LineShown::Fact {
                        label,
                        tone: *tone,
                        phrase: html(phrase),
                    },
                    ItemLine::Wait(wait) => LineShown::Status {
                        label,
                        tone: Tone::Pending,
                        phrase: html(&status::describe_wait(wait)),
                    },
                }
            })
            .collect();
        ItemShown {
            heading: html::item(answer.number(), repo),
            title: answer.title().to_string(),
            lines,
        }
    }
}

/// How many of an item's runs its page lists, the latest
/// (`docs/specs/design_system.md`, Lists of runs).
const RUNS_SHOWN: usize = 10;

/// #`number`'s latest runs, from their records, whether or not Linewright
/// manages it.
fn item_runs(sources: &Sources, number: Number) -> RunsShown {
    let query = runs::Query {
        item: Some(number),
        limit: Some(RUNS_SHOWN),
        ..runs::Query::default()
    };
    let listed = runs::list(api::recorded_or_none(sources), &query, sources.clock.now());
    RunsShown::of_item(listed, number, &sources.work_dir.repo)
}

#[derive(Template, WebTemplate)]
#[template(path = "item.html")]
struct ItemPage {
    page: Page,
    frame: Frame,
    item: ItemShown,
    runs: RunsShown,
}

#[derive(Template)]
#[template(path = "item.html", block = "item")]
struct ItemRegion<'a> {
    item: &'a ItemShown,
}

#[derive(Template)]
#[template(path = "item.html", block = "runs")]
struct RunsRegion<'a> {
    runs: &'a RunsShown,
}

pub(super) async fn item_page(
    State(sources): State<Sources>,
    Path(number): Path<Number>,
) -> Response {
    let page = Page::new(
        format!("{number} · {}", sources.work_dir.repo),
        format!("/events/items/{}", number.0),
    )
    .filling_the_viewport();
    let frame = Frame::of(&sources);
    match api::item_answer(&sources, number).await {
        Ok(answer) => ItemPage {
            item: ItemShown::of(&answer, &sources.work_dir.repo),
            runs: item_runs(&sources, number),
            page,
            frame,
        }
        .into_response(),
        Err(e) => error_page(page, frame, &e),
    }
}

fn error_page(page: Page, frame: Frame, e: &Unexplained) -> Response {
    let page = ErrorPage {
        page,
        frame,
        region: "item",
        error: e.to_string(),
    };
    (e.status(), page).into_response()
}

/// The item page's regions: the frame's, the item as `serve`'s view
/// explains it, without reading GitHub, when it holds the item, and its runs
/// from their records, so that they keep up whether or not the view holds
/// it.
pub(super) async fn item_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Path(number): Path<Number>,
) -> Response {
    let regions = move || {
        let mut regions = frame_regions(&Frame::of(&sources));
        if let Ok(in_view) = api::explaining(&sources)
            && let Some(answer) =
                explain::explain_in_view(&in_view.view, &in_view.recorded, number, in_view.now)
        {
            let item = ItemShown::of(&answer, &sources.work_dir.repo);
            regions.push(("item", render(&ItemRegion { item: &item })));
        }
        let runs = item_runs(&sources, number);
        regions.push(("runs", render(&RunsRegion { runs: &runs })));
        regions
    };
    events(regions, stopping)
}
