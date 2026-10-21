//! The server page (`docs/specs/monitoring.md`, The web UI): the facts
//! about Linewright itself, as `status` shows them.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use tokio_util::sync::CancellationToken;

use super::html::{self, Html};
use super::{Frame, Page, events, frame_regions, render};
use crate::api::{self, Sources};
use crate::status::{self, Fact};

#[derive(Template, WebTemplate)]
#[template(path = "server.html")]
struct ServerPage {
    page: Page,
    frame: Frame,
    facts: Vec<Html>,
}

#[derive(Template)]
#[template(path = "server.html", block = "main")]
struct ServerRegion<'a> {
    facts: &'a [Html],
}

/// The frame and the facts, from one answer of `status`.
fn shown(sources: &Sources) -> (Frame, Vec<Html>) {
    let linewright = api::status_answer(sources).linewright;
    let repo = &sources.work_dir.repo;
    let facts = status::linewright_facts(&linewright)
        .into_iter()
        .map(|fact| match fact {
            Fact::Plain(phrase) | Fact::Warning(phrase) | Fact::Problem(phrase) => {
                html::phrase(&phrase, repo)
            }
        })
        .collect();
    (Frame::of_status(sources, &linewright), facts)
}

pub(super) async fn server_page(State(sources): State<Sources>) -> Response {
    let (frame, facts) = shown(&sources);
    ServerPage {
        page: Page::new(
            format!("Server · {}", sources.work_dir.repo),
            "/events/server".into(),
        ),
        frame,
        facts,
    }
    .into_response()
}

/// The server page's regions: the frame's, and the facts.
pub(super) async fn server_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
) -> Response {
    let regions = move || {
        let (frame, facts) = shown(&sources);
        let mut regions = frame_regions(&frame);
        regions.push(("server", render(&ServerRegion { facts: &facts })));
        regions
    };
    events(regions, stopping)
}
