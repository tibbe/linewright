//! The item page (`docs/specs/board.md`, Item): #N's
//! heading, its facts in the board's terms and its runs.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::Path;
use axum::response::{IntoResponse, Response};
use tokio_util::sync::CancellationToken;

use super::caller::Reader;
use super::cards::{self, card_line, lane_head};
use super::facts::{Fact, Piece};
use super::html::{self, Html};
use super::icons::ItemKind;
use super::names::Names;
use super::runs_table::{RunsRegion, RunsShown};
use super::shell::{self, Area, Frame, Heading, Missing, Navigation, Place, Thing};
use super::{ErrorPage, Page, events, nothing_there, render};
use crate::access::{Permit, Read};
use crate::actions::{self, ItemInView};
use crate::api::{Sources, Unexplained};
use crate::domain::{Number, Snapshot};
use crate::item_facts::{ItemFacts, Related, StateFact};
use crate::lifecycle;
use crate::managed::{LeftAloneIssue, LeftAloneItem, LeftAlonePull, PullExclusion};
use crate::phrase::{Phrase, Status};
use crate::reconcile::NotSpecChange;
use crate::status;

/// #N's facts, ready for the page.
struct ItemShown {
    facts: Vec<Fact>,
}

impl ItemShown {
    fn of(facts: &ItemFacts, names: &Names) -> ItemShown {
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
fn heading(facts: &ItemFacts, repo: &str) -> Heading {
    Heading {
        title: format!("{} {}", facts.number, facts.title),
        name: html::rendered(&ItemName {
            kind: (!matches!(facts.state, StateFact::Tracking { .. }))
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

/// The State fact's line: the item's state in the States words
/// (`docs/specs/board.md`, States), then the card's line with its
/// card status; for a tracking issue, its state, then when its lane
/// finished, if it did; for an item Linewright leaves, its state, where it
/// has one, then "Left alone" and why.
fn state(state: &StateFact, names: &Names) -> Vec<Piece> {
    let then = |first: Option<lifecycle::State>, lines: Vec<(Html, Option<Status>)>| {
        let first = first.map(|item_state| (html::escape(item_state.shown()), None));
        let mut pieces = vec![];
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
    };
    match state {
        StateFact::Card { state, card, .. } => then(
            Some(*state),
            card_line(card)
                .map(|line| (line.html(names), card.status().map(cards::status)))
                .into_iter()
                .collect(),
        ),
        StateFact::Tracking { state, finished } => then(
            *state,
            finished
                .map(|at| {
                    (
                        html::phrase(&Phrase::from("finished ").time(at), names),
                        None,
                    )
                })
                .into_iter()
                .collect(),
        ),
        StateFact::LeftAlone { state, why } => then(
            *state,
            vec![
                (html::escape("Left alone"), None),
                (html::phrase(&left_alone_worded(why), names), None),
            ],
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
            status: status.map(cards::status),
            kind: ItemKind::of(*pull_request),
            heading: match title {
                Some(title) => html::titled(*number, title, repo),
                None => html::item(*number, repo),
            },
        },
        Related::LaneHead(lane) => Piece::LaneHead(lane_head(lane, repo)),
    }
}

/// What keeps `item` out of Linewright's work, in the UI's own words.
fn left_alone_worded(item: &LeftAloneItem) -> Phrase {
    match item {
        LeftAloneItem::Issue(LeftAloneIssue::Closed)
        | LeftAloneItem::PullRequest(LeftAlonePull::Closed) => "closed".into(),
        LeftAloneItem::Issue(LeftAloneIssue::Open { left_alone }) => left_alone.worded(),
        LeftAloneItem::PullRequest(LeftAlonePull::Open { left_alone }) => match left_alone {
            PullExclusion::NotWorked { author } => {
                format!("opened by {author}, and Linewright has not worked it").into()
            }
        },
        LeftAloneItem::PullRequest(LeftAlonePull::Merged { left_alone }) => match left_alone {
            NotSpecChange::Linewrights => "opened by Linewright".into(),
            NotSpecChange::OtherBranch { base } => Phrase::from("merged into ")
                .code(base.clone())
                .text(", not the default branch"),
            NotSpecChange::NoSpecFile { spec_glob } => Phrase::from("touches no file matching ")
                .code(spec_glob.clone())
                .text(", the spec-glob"),
            NotSpecChange::TakenUp { issue } => {
                Phrase::from("a spec change taken up in ").item(*issue)
            }
            NotSpecChange::Designed { label } => Phrase::from("a spec change labeled ")
                .code(label.name())
                .text(", taken up already"),
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
        LeftAloneItem::PullRequest(LeftAlonePull::TakingUp) => {
            "a spec change Linewright takes up at its next poll".into()
        }
    }
}

/// How many of an item's runs its page lists, the latest
/// (`docs/specs/board.md`, Item, Runs).
const RUNS_SHOWN: usize = 10;

/// #`number`'s latest runs, from their records, whether or not Linewright
/// manages it.
async fn item_runs(
    permit: &Permit<Read>,
    sources: &Sources,
    number: Number,
    names: &Names<'_>,
) -> RunsShown {
    let listed = actions::item_runs(permit, sources, number, RUNS_SHOWN).await;
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
        Ok((facts, snapshot)) => {
            let names = names(&sources, Some(&snapshot));
            ItemPage {
                page: page(
                    number,
                    Thing::Found(heading(&facts, names.repo())),
                    navigation,
                ),
                item: ItemShown::of(&facts, &names),
                runs: item_runs(&permit, &sources, number, &names).await,
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
    Extension(stopping): Extension<CancellationToken>,
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
            let runs = item_runs(&permit, &sources, number, &names).await;
            regions.push(("runs", render(&RunsRegion { runs: &runs })));
            regions
        }
    };
    events(regions, stopping)
}
