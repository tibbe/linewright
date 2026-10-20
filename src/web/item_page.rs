//! The item page (`docs/specs/monitoring.md`, The web UI): what `explain`
//! shows of an item.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use tokio_util::sync::CancellationToken;

use super::html::{self, Html};
use super::{ErrorPage, Frame, Page, events, frame_regions, render};
use crate::api::{self, Sources, Unexplained};
use crate::domain::Number;
use crate::explain::{self, Line};
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
    /// A wait or a run in flight, whose label is its status.
    Status {
        label: &'static str,
        tone: Tone,
        phrase: Html,
    },
    Past {
        label: &'static str,
        run: Html,
        outcome: Tone,
        word: &'static str,
        about: Html,
    },
}

impl ItemShown {
    fn of(answer: &explain::Answer, repo: &str) -> ItemShown {
        let html = |phrase: &Phrase| html::phrase(phrase, repo);
        let lines = answer
            .lines()
            .iter()
            .map(|line| {
                let label = line.label();
                match line {
                    Line::Fact { phrase, tone, .. } => LineShown::Fact {
                        label,
                        tone: *tone,
                        phrase: html(phrase),
                    },
                    Line::Wait(wait) => LineShown::Status {
                        label,
                        tone: Tone::Pending,
                        phrase: html(&status::describe_wait(wait)),
                    },
                    Line::InFlight(run) => LineShown::Status {
                        label,
                        tone: Tone::InProgress,
                        phrase: html::escape(&format!("{}: ", run.id)).then(&html(&run.describe())),
                    },
                    Line::Past(run) => LineShown::Past {
                        label,
                        run: html::escape(&run.id.to_string()),
                        outcome: run.outcome.tone(),
                        word: run.outcome.word(),
                        about: html(&runs::describe_run(run)),
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

#[derive(Template, WebTemplate)]
#[template(path = "item.html")]
struct ItemPage {
    page: Page,
    frame: Frame,
    item: ItemShown,
}

#[derive(Template)]
#[template(path = "item.html", block = "main")]
struct ItemRegion<'a> {
    item: &'a ItemShown,
}

pub(super) async fn item_page(
    State(sources): State<Sources>,
    Path(number): Path<Number>,
) -> Response {
    let page = Page::new(
        format!("{number} · {}", sources.work_dir.repo),
        format!("/events/items/{}", number.0),
    );
    let frame = Frame::of(&sources);
    match api::item_answer(&sources, number).await {
        Ok(answer) => ItemPage {
            item: ItemShown::of(&answer, &sources.work_dir.repo),
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

/// The item page's regions: the frame's, and the item as `serve`'s view
/// explains it, without reading GitHub, when it holds the item.
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
        regions
    };
    events(regions, stopping)
}
