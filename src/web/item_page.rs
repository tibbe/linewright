//! The item page (`docs/specs/design_system.md`, The item page): #N's
//! heading, its facts in the board's terms and its runs.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use tokio_util::sync::CancellationToken;

use super::cards::{card_line, lane_head, tone};
use super::facts::{Fact, Piece};
use super::html::{self, Html};
use super::names::Names;
use super::runs_table::RunsShown;
use super::{ErrorPage, Frame, Page, events, frame_regions, render};
use crate::api::{self, Sources, Unexplained};
use crate::board;
use crate::domain::{Number, Snapshot};
use crate::explain::{self, LeftAloneIssue, LeftAloneItem, LeftAlonePull, PullExclusion};
use crate::item_facts::{self, ItemFacts, Related, StateFact};
use crate::phrase::{Phrase, Tone};
use crate::reconcile::NotSpecChange;
use crate::runs;
use crate::status;

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
    fn of(facts: &ItemFacts, names: &Names) -> ItemShown {
        let repo = names.repo();
        ItemShown {
            heading: html::titled(facts.number, &facts.title, repo),
            pull_request: facts.pull_request,
            as_of: html::time(facts.last_read),
            facts: shown_facts(facts, names),
        }
    }
}

/// `facts` in the order the item page's table gives them, each while it is
/// shown (`docs/specs/design_system.md`, The item page), with the items its
/// phrases name as `names` names them.
fn shown_facts(facts: &ItemFacts, names: &Names) -> Vec<Fact> {
    let repo = names.repo();
    let text = |phrase: &Phrase| Piece::Text(html::phrase(phrase, names));
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
        Some(Fact {
            label: "State",
            lines: vec![state(&facts.state, names)],
        }),
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
/// status; for a tracking issue, "Tracking", then when its lane finished, if
/// it did; or why Linewright leaves the item.
fn state(state: &StateFact, names: &Names) -> Vec<Piece> {
    let then = |first: &str, line: Option<(Html, Option<Tone>)>| {
        let mut pieces = vec![Piece::Text(html::escape(first))];
        if let Some((line, tone)) = line {
            pieces.push(Piece::Text(html::escape(" · ")));
            pieces.push(match tone {
                Some(tone) => Piece::Status { tone, words: line },
                None => Piece::Text(line),
            });
        }
        pieces
    };
    match state {
        StateFact::Card { column, card } => then(
            column.name(),
            card_line(card).map(|line| (line.html(names), card.status().map(tone))),
        ),
        StateFact::Tracking { finished } => then(
            "Tracking",
            finished.map(|at| {
                (
                    html::phrase(&Phrase::from("finished ").time(at), names),
                    None,
                )
            }),
        ),
        StateFact::LeftAlone(item) => then(
            "Left alone",
            Some((html::phrase(&left_alone_worded(item), names), None)),
        ),
    }
}

/// An item a fact lists, as its card is: its heading as a card's is, or its
/// number alone where its title is not known; or a tracking issue as its
/// lane's head is, with no card status.
fn related(item: &Related, repo: &str) -> Piece {
    match item {
        Related::Card {
            number,
            title,
            pull_request,
            status,
        } => Piece::Item {
            tone: status.map(tone),
            pull_request: *pull_request,
            heading: match title {
                Some(title) => html::titled(*number, title, repo),
                None => html::item(*number, repo),
            },
        },
        Related::LaneHead(lane) => Piece::Item {
            tone: None,
            pull_request: false,
            heading: lane_head(lane, repo),
        },
    }
}

/// What keeps `item` out of Linewright's work, in the UI's own words.
fn left_alone_worded(item: &LeftAloneItem) -> Phrase {
    match item {
        LeftAloneItem::Issue(LeftAloneIssue::Closed)
        | LeftAloneItem::PullRequest(LeftAlonePull::Closed) => "closed".into(),
        LeftAloneItem::Issue(LeftAloneIssue::Open { left_alone }) => left_alone.worded(),
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

/// How many of an item's runs its page lists, the latest
/// (`docs/specs/design_system.md`, Lists of runs).
const RUNS_SHOWN: usize = 10;

/// #`number`'s latest runs, from their records, whether or not Linewright
/// manages it.
fn item_runs(sources: &Sources, number: Number, names: &Names) -> RunsShown {
    let query = runs::Query {
        item: Some(number),
        limit: Some(RUNS_SHOWN),
        ..runs::Query::default()
    };
    let listed = runs::list(api::recorded_or_none(sources), &query, sources.clock.now());
    RunsShown::of_item(listed, number, names)
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
        Ok((facts, snapshot)) => {
            let names = names(&sources, Some(&snapshot));
            ItemPage {
                item: ItemShown::of(&facts, &names),
                runs: item_runs(&sources, number, &names),
                page,
                frame,
            }
            .into_response()
        }
        Err(e) => error_page(page, frame, &e),
    }
}

/// How the item page names items: with the titles of the snapshot its facts
/// were answered from, or by number alone without one.
fn names<'a>(sources: &'a Sources, snapshot: Option<&'a Snapshot>) -> Names<'a> {
    Names::of(&sources.work_dir.repo, snapshot)
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
        let in_view = api::explaining(&sources).ok();
        let names = names(
            &sources,
            in_view.as_ref().map(|in_view| &*in_view.view.snapshot),
        );
        if let Some(in_view) = &in_view
            && let Some(answer) =
                explain::explain_in_view(&in_view.view, &in_view.recorded, number, in_view.now)
        {
            let board = board::board(&in_view.view, &in_view.recorded, in_view.now);
            let facts = item_facts::item_facts(&answer, &in_view.view, &board);
            let item = ItemShown::of(&facts, &names);
            regions.push(("item", render(&ItemRegion { item: &item })));
        }
        let runs = item_runs(&sources, number, &names);
        regions.push(("runs", render(&RunsRegion { runs: &runs })));
        regions
    };
    events(regions, stopping)
}
