//! The item page (`docs/specs/design_system.md`, The item page): #N's
//! heading, its facts in the board's terms and its runs.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use tokio_util::sync::CancellationToken;

use super::cards::{card_line, lane_line, tone};
use super::facts::{Fact, Piece};
use super::html::{self, Html};
use super::runs_table::RunsShown;
use super::{ErrorPage, Frame, Page, events, frame_regions, render};
use crate::api::{self, Sources, Unexplained};
use crate::board;
use crate::domain::{Number, StatusLabel, Timestamp, label};
use crate::explain::{
    self, LeftAloneIssue, LeftAloneItem, LeftAlonePull, OpenIssueExclusion, PullExclusion,
};
use crate::item_facts::{self, ItemFacts, Related, StateFact};
use crate::phrase::{Phrase, Tone};
use crate::reconcile::NotSpecChange;
use crate::runs;
use crate::status::{self, IssueExclusion};

/// #N's heading and facts, ready for the page.
struct ItemShown {
    heading: Html,
    /// Whether it carries the pull request icon, as its card does.
    pull_request: bool,
    /// When `serve` last read it.
    as_of: Html,
    facts: Vec<Fact>,
}

impl ItemShown {
    /// Its runs are left to [`item_runs`], to show as a table.
    fn of(facts: &ItemFacts, repo: &str) -> ItemShown {
        ItemShown {
            heading: html::titled(facts.number, &facts.title, repo),
            pull_request: facts.pull_request,
            as_of: html::time(facts.last_read),
            facts: shown_facts(facts, repo),
        }
    }
}

/// `facts` in the order the item page's table gives them, each while it is
/// shown (`docs/specs/design_system.md`, The item page).
fn shown_facts(facts: &ItemFacts, repo: &str) -> Vec<Fact> {
    let text = |phrase: &Phrase| Piece::Text(html::phrase(phrase, repo));
    let fact = |label, lines: Vec<Vec<Piece>>| (!lines.is_empty()).then_some(Fact { label, lines });
    let items = |items: &[Related]| items.iter().map(|item| vec![related(item, repo)]).collect();
    let tickets = facts.tickets.as_ref().map(|tickets| {
        let closed = format!("{} of {} closed", tickets.closed, tickets.tickets.len());
        vec![
            vec![text(&closed.into())],
            tickets
                .tickets
                .iter()
                .map(|ticket| related(ticket, repo))
                .collect(),
        ]
    });
    [
        Some(
            Fact {
                label: "State",
                lines: vec![state(&facts.state)],
            }
            .map(|words| html::phrase(&words, repo)),
        ),
        fact(
            "Waiting for",
            facts
                .waiting_for
                .iter()
                .map(|wait| vec![text(&status::describe_wait(wait))])
                .collect(),
        ),
        fact("Tracking issue", items(facts.tracking_issue.as_slice())),
        fact("Tickets", tickets.unwrap_or_default()),
        fact("Spec change", items(facts.spec_change.as_slice())),
        fact("Pull requests", items(&facts.pull_requests)),
        fact("Closes", items(&facts.closes)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The State fact's line: the column and the card's line with its card
/// status, a tracking issue's lane head's line, or why Linewright leaves
/// the item.
fn state(state: &StateFact) -> Vec<Piece<Phrase>> {
    let then = |first: &str, line: Option<(Phrase, Option<Tone>)>| {
        let mut pieces = vec![Piece::Text(Phrase::from(first))];
        if let Some((line, tone)) = line {
            pieces.push(Piece::Text(" · ".into()));
            pieces.push(match tone {
                Some(tone) => Piece::Status { tone, words: line },
                None => Piece::Text(line),
            });
        }
        pieces
    };
    let tracking = |finished: Option<Timestamp>| {
        then(
            "Tracking",
            finished.map(|at| (Phrase::from("finished ").time(at), None)),
        )
    };
    match state {
        StateFact::Card { column, card } => then(
            column.name(),
            card_line(card).map(|line| (line, card.status().map(tone))),
        ),
        StateFact::Lane(lane) => match lane_line(lane) {
            Some((tone, words)) => vec![Piece::Status { tone, words }],
            None => tracking(lane.finished()),
        },
        StateFact::Tracking => tracking(None),
        StateFact::LeftAlone(item) => then("Left alone", Some((left_alone_worded(item), None))),
    }
}

/// An item a fact lists, as its card is: its heading as a card's is, or its
/// number alone where its title is not known.
fn related(item: &Related, repo: &str) -> Piece {
    let heading = match &item.title {
        Some(title) => html::titled(item.number, title, repo),
        None => html::item(item.number, repo),
    };
    Piece::Item {
        tone: item.status.map(tone),
        pull_request: item.pull_request,
        heading,
    }
}

/// What keeps `item` out of Linewright's work, in the UI's own words.
fn left_alone_worded(item: &LeftAloneItem) -> Phrase {
    match item {
        LeftAloneItem::Issue(LeftAloneIssue::Closed)
        | LeftAloneItem::PullRequest(LeftAlonePull::Closed) => "closed".into(),
        LeftAloneItem::Issue(LeftAloneIssue::Open { left_alone }) => match left_alone {
            OpenIssueExclusion::NotInView => Phrase::from("labeled none of ")
                .then(one_of(&label::IN_VIEW))
                .text(", and Linewright neither opened it nor is assigned to it"),
            OpenIssueExclusion::Held(IssueExclusion::NoPipelineLabel) => {
                Phrase::from("labeled none of ")
                    .then(one_of(&StatusLabel::ALL.map(StatusLabel::label)))
            }
            OpenIssueExclusion::Held(IssueExclusion::NoTickets) => {
                "a tracking issue with no tickets, and none are being cut".into()
            }
            OpenIssueExclusion::Held(IssueExclusion::NotMarked { label }) => {
                Phrase::from("not labeled ")
                    .code(label.clone())
                    .text(", the required label, and Linewright has not worked it")
            }
        },
        LeftAloneItem::PullRequest(LeftAlonePull::Open { left_alone }) => match left_alone {
            PullExclusion::NotLinewrights { author } => format!("opened by {author}").into(),
            PullExclusion::ClosesNoIssue => "closes no issue Linewright could work on".into(),
            PullExclusion::Superseded { issue, by } => Phrase::from("for ")
                .item(*issue)
                .text(", which Linewright works on through ")
                .item(*by),
        },
        LeftAloneItem::PullRequest(LeftAlonePull::Merged { left_alone }) => match left_alone {
            NotSpecChange::Linewrights => "opened by Linewright".into(),
            NotSpecChange::OtherBranch { base } => Phrase::from("merged into ")
                .code(base.clone())
                .text(", not the default branch"),
            NotSpecChange::NoSpecFile { spec_glob } => Phrase::from("touches no file matching ")
                .code(spec_glob.clone())
                .text(", the spec-glob"),
            NotSpecChange::Designed { .. } => "designed already".into(),
            NotSpecChange::BeforeLookback {
                merged_at,
                window_opened,
            } => Phrase::from("merged ")
                .time(*merged_at)
                .text(", before the spec-lookback window opened ")
                .time(*window_opened),
            NotSpecChange::RequiredLabel { label } => Phrase::from("not labeled ")
                .code(label.clone())
                .text(", the required label"),
        },
    }
}

/// `names` as code, listed with "or".
fn one_of(names: &[&str]) -> Phrase {
    Phrase::list(names.iter().map(|&name| Phrase::default().code(name)), "or")
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
    match api::item_facts(&sources, number).await {
        Ok(facts) => ItemPage {
            item: ItemShown::of(&facts, &sources.work_dir.repo),
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
/// explains it, without reading GitHub, when it holds the item, with the
/// view's board, and its runs from their records, so that they keep up
/// whether or not the view holds it.
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
            let board = board::board(&in_view.view, &in_view.recorded, in_view.now);
            let facts = item_facts::item_facts(&answer, &in_view.view, &board);
            let item = ItemShown::of(&facts, &sources.work_dir.repo);
            regions.push(("item", render(&ItemRegion { item: &item })));
        }
        let runs = item_runs(&sources, number);
        regions.push(("runs", render(&RunsRegion { runs: &runs })));
        regions
    };
    events(regions, stopping)
}
