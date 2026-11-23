//! The item page (`docs/specs/board.md`, Item): #N's
//! heading, its facts in the board's terms and its runs.

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::Path;
use axum::response::{IntoResponse, Response};

use super::caller::Reader;
use super::cards::{self, lane_head, line_html};
use super::facts::{Fact, Piece};
use super::html::{self, Html};
use super::icons::ItemKind;
use super::names::Names;
use super::runs_table::{RunsRegion, RunsShown};
use super::shell::{self, Area, Frame, Heading, Missing, Navigation, Place, Thing};
use super::{ErrorPage, Page, events, nothing_there, render};
use crate::actions::{self, ItemFactsScreen, ItemInView, RelatedScreen, Standing};
use crate::api::{Sources, Unexplained};
use crate::domain::{Number, Snapshot};
use crate::phrase::{Phrase, Status};
use crate::status;

/// #N's facts, ready for the page.
struct ItemShown {
    facts: Vec<Fact>,
}

impl ItemShown {
    fn of(facts: &ItemFactsScreen, names: &Names) -> ItemShown {
        ItemShown {
            facts: shown_facts(facts, names),
        }
    }
}

/// #N's name in its heading: its number and title as on its card, with the
/// icon of an issue or a pull request beside it, but for a tracking issue,
/// whose card is its lane head (`docs/specs/board.md`, Item, Heading and Its
/// card; Cards, Issue or pull request).
#[derive(Template)]
#[template(
    source = r#"{% if let Some(kind) = kind %}{{ kind.svg() }}{% endif %}{{ titled|safe }}"#,
    ext = "html"
)]
struct ItemName {
    kind: Option<ItemKind>,
    titled: Html,
}

/// #N's heading: its name, and under it when `serve` last read it.
fn heading(facts: &ItemFactsScreen, repo: &str) -> Heading {
    Heading {
        title: format!("{} {}", facts.number, facts.title),
        name: html::rendered(&ItemName {
            kind: (!matches!(facts.standing, Standing::Tracking { .. }))
                .then(|| ItemKind::of(facts.pull_request)),
            titled: html::titled(facts.number, &facts.title, repo),
        }),
        state: None,
        line: Some(html::escape("as of ").then(&html::time(facts.last_read))),
    }
}

/// `facts` in the order the item page's table gives them, each while it is
/// shown (`docs/specs/board.md`, Item), with the items its
/// phrases name as `names` names them.
fn shown_facts(facts: &ItemFactsScreen, names: &Names) -> Vec<Fact> {
    let repo = names.repo();
    let text = |phrase: &Phrase| Piece::Text(html::phrase(phrase, names));
    let fact = |label, lines: Vec<Vec<Piece>>| (!lines.is_empty()).then_some(Fact { label, lines });
    let items =
        |items: &[RelatedScreen]| items.iter().map(|item| vec![related(item, repo)]).collect();
    let tickets = facts.tickets.map(|tickets| {
        let closed = format!("{} of {} closed", tickets.tickets_closed, tickets.tickets);
        vec![
            vec![text(&closed.into())],
            facts
                .ticket_items
                .iter()
                .map(|ticket| related(ticket, repo))
                .collect(),
        ]
    });
    [
        Some(Fact {
            label: "State",
            lines: vec![state(facts, names)],
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

/// The State fact's line: the item's state in the States words
/// (`docs/specs/board.md`, States), then the card's line with its
/// card status; for a tracking issue, its state, then when its lane
/// finished, if it did; for an item Linewright leaves, its state, where it
/// has one, then "Left alone" and why.
fn state(facts: &ItemFactsScreen, names: &Names) -> Vec<Piece> {
    let mut pieces = vec![];
    let first = facts
        .state
        .map(|item_state| (html::escape(item_state.shown()), None));
    let lines: Vec<(Html, Option<Status>)> = match &facts.standing {
        Standing::Card { line, status, .. } => line
            .as_ref()
            .map(|line| (line_html(line, names), status.map(cards::status)))
            .into_iter()
            .collect(),
        Standing::Tracking { finished } => finished
            .map(|at| {
                (
                    html::phrase(&Phrase::from("finished ").time(at), names),
                    None,
                )
            })
            .into_iter()
            .collect(),
        Standing::LeftAlone { kept_out_by } => vec![
            (html::escape("Left alone"), None),
            (html::phrase(kept_out_by, names), None),
        ],
    };
    for (line, status) in first.into_iter().chain(lines) {
        if !pieces.is_empty() {
            pieces.push(Piece::Text(html::escape(" · ")));
        }
        pieces.push(match status {
            Some(status) => Piece::Status {
                status,
                words: line,
            },
            None => Piece::Text(line),
        });
    }
    pieces
}

/// An item a fact lists, as its card is: its heading as a card's is, or its
/// number alone where its title is not known; or a tracking issue as its
/// lane's head is, with no card status.
fn related(item: &RelatedScreen, repo: &str) -> Piece {
    match item {
        RelatedScreen::Card {
            number,
            title,
            pull_request,
            status,
        } => Piece::Item {
            status: status.map(cards::status),
            kind: ItemKind::of(*pull_request),
            heading: match title {
                Some(title) => html::titled(*number, title, repo),
                None => html::item(*number, repo),
            },
        },
        RelatedScreen::LaneHead(lane) => Piece::LaneHead(lane_head(lane, repo)),
    }
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

/// Item `number`'s page, with `thing` its heading or that it is missing.
fn page(number: Number, thing: Thing, navigation: Navigation) -> Page {
    Page::new(
        Place::Alone(Area::Board, thing),
        format!("/events/items/{}", number.0),
        navigation,
    )
    .filling_the_viewport()
}

pub(super) async fn item_page(
    Reader { permit, sources }: Reader,
    Path(number): Path<Number>,
    navigation: Navigation,
) -> Response {
    let frame = Frame::of(&permit, &sources).await;
    match actions::get_item(&permit, &sources, number).await {
        Ok((item, snapshot)) => {
            let names = names(&sources, Some(&snapshot));
            ItemPage {
                page: page(
                    number,
                    Thing::Found(heading(&item.facts, names.repo())),
                    navigation,
                ),
                item: ItemShown::of(&item.facts, &names),
                runs: RunsShown::of_item(&item.runs, number, &names),
                frame,
            }
            .into_response()
        }
        Err(e) => error_page(number, frame, &e, navigation),
    }
}

/// How the item page names items: with the titles of the snapshot its facts
/// were answered from, or by number alone without one.
fn names<'a>(sources: &'a Sources, snapshot: Option<&'a Snapshot>) -> Names<'a> {
    Names::of(&sources.work_dir.repo, snapshot)
}

fn error_page(number: Number, frame: Frame, e: &Unexplained, navigation: Navigation) -> Response {
    if let Unexplained::NoSuchItem(missing) = e {
        return nothing_there(
            |thing| page(number, thing, navigation),
            Missing::Item(*missing),
            frame,
            "item",
        );
    }
    let page = ErrorPage {
        page: page(
            number,
            Thing::Found(Heading::named(number.to_string())),
            navigation,
        ),
        frame,
        region: "item",
        error: e.to_string(),
    };
    (e.status(), page).into_response()
}

/// The item page's regions: the notices; the rest of the shell's and the
/// item as `serve`'s view explains it, without reading GitHub, when it holds
/// the item, with the view's board; and its runs from their records whether or not the view
/// holds it.
pub(super) async fn item_events(
    Reader { permit, sources }: Reader,
    watch: super::caller::Watch,
    Path(number): Path<Number>,
    navigation: Navigation,
) -> Response {
    let regions = move || {
        let (permit, sources) = (permit.clone(), sources.clone());
        async move {
            let frame = Frame::of(&permit, &sources).await;
            let mut regions = vec![];
            let ItemInView { snapshot, facts } =
                actions::item_in_view(&permit, &sources, number).await;
            let names = names(&sources, snapshot.as_deref());
            if let Some(facts) = facts {
                let item = ItemShown::of(&facts, &names);
                let page = page(
                    number,
                    Thing::Found(heading(&facts, names.repo())),
                    navigation,
                );
                regions.extend(shell::regions(&frame, &page));
                regions.push(("item", render(&ItemRegion { item: &item })));
            } else {
                regions.push(shell::holds(&frame));
            }
            let runs = actions::item_runs(&permit, &sources, number).await;
            let runs = RunsShown::of_item(&runs, number, &names);
            regions.push(("runs", render(&RunsRegion { runs: &runs })));
            regions
        }
    };
    events(regions, watch)
}
