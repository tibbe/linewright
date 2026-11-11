//! The board (`docs/specs/design_system.md`, The board): the managed items
//! as cards, in a lane per piece of work and a column per stage.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::cards::{card_line, lane_head, tone};
use super::html::{self, Html};
use super::names::Names;
use super::{Frame, Page, events, frame_regions, given, render, with_filters};
use crate::api::{self, Sources};
use crate::board::{Board, Card, Column, Lane};
use crate::domain::Login;
use crate::phrase::Tone;

/// The Person filter as the page's address carries it, an empty or blank
/// one leaving the board unnarrowed (`docs/specs/design_system.md`,
/// Filtering by person).
#[derive(Deserialize)]
pub(super) struct FilterQuery {
    #[serde(default)]
    person: String,
}

impl FilterQuery {
    /// The person the board is narrowed to, if any.
    fn person(&self) -> Option<Login> {
        given(self.person.trim()).map(Login::new)
    }
}

/// The board as the page shows it: the whole board's tab title and people,
/// and its lanes narrowed to `person`, if set.
struct BoardPageShown {
    title: String,
    board: BoardShown,
    people: Vec<Login>,
}

impl BoardPageShown {
    fn of(board: &Board, person: Option<&Login>, repo: &str) -> BoardPageShown {
        let narrowed = person.map(|person| board.narrowed(person));
        BoardPageShown {
            title: board_title(board, repo),
            board: BoardShown::of(narrowed.as_ref().unwrap_or(board), repo),
            people: board.people(),
        }
    }
}

/// The board (`docs/specs/design_system.md`, The board), ready for the page.
struct BoardShown {
    columns: [&'static str; Column::ALL.len()],
    /// The lanes not finished, in the board's order.
    unfinished: Vec<LaneShown>,
    /// The finished lanes, latest finished first, each folded.
    finished: Vec<LaneShown>,
}

struct LaneShown {
    /// The id of its fold, stable through updates.
    id: String,
    /// The tracking issue and its tickets closed of total, or the item; and
    /// when it finished, if it did.
    head: Html,
    cells: Vec<CellShown>,
}

struct CellShown {
    column: &'static str,
    cards: Vec<CardShown>,
}

/// A card (`docs/specs/design_system.md`, Cards).
struct CardShown {
    heading: Html,
    /// Whether it carries the pull request icon.
    pull_request: bool,
    /// The tone of its card status, if it has one.
    tone: Option<Tone>,
    line: Option<Html>,
    /// Whether it is in Done, which is quieter than the rest.
    quiet: bool,
}

impl BoardShown {
    fn of(board: &Board, repo: &str) -> BoardShown {
        let names = Names::of(repo, None);
        let (finished, unfinished): (Vec<&Lane>, Vec<&Lane>) = board
            .lanes
            .iter()
            .partition(|lane| lane.finished().is_some());
        let shown = |lanes: Vec<&Lane>| {
            lanes
                .into_iter()
                .map(|lane| LaneShown::of(lane, &names))
                .collect()
        };
        BoardShown {
            columns: Column::ALL.map(Column::name),
            unfinished: shown(unfinished),
            finished: shown(finished),
        }
    }

    /// The finished fold's summary: how many finished lanes there are.
    fn finished_count(&self) -> String {
        counted(self.finished.len(), "finished lane")
    }
}

impl LaneShown {
    fn of(lane: &Lane, names: &Names) -> LaneShown {
        LaneShown {
            id: format!("lane-{}", lane.head.number().0),
            head: lane_head(&lane.heading(), names.repo()),
            cells: Column::ALL
                .into_iter()
                .map(|column| CellShown {
                    column: column.name(),
                    cards: lane
                        .cards(column)
                        .iter()
                        .map(|card| CardShown::of(card, column, names))
                        .collect(),
                })
                .collect(),
        }
    }
}

/// `n` and `thing`, plural but for one.
fn counted(n: usize, thing: &str) -> String {
    match n {
        1 => format!("1 {thing}"),
        _ => format!("{n} {thing}s"),
    }
}

impl CardShown {
    fn of(card: &Card, column: Column, names: &Names) -> CardShown {
        CardShown {
            heading: html::titled(card.number, &card.title, names.repo()),
            pull_request: card.pull_request,
            tone: card.status().map(tone),
            line: card_line(card).map(|line| line.html(names)),
            quiet: column == Column::Done,
        }
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "board.html")]
struct BoardPage {
    page: Page,
    frame: Frame,
    /// The person the board is narrowed to, empty while unset.
    person: String,
    board: BoardShown,
    people: Vec<Login>,
}

#[derive(Template)]
#[template(path = "board.html", block = "board")]
struct BoardRegion<'a> {
    board: &'a BoardShown,
}

#[derive(Template)]
#[template(path = "board.html", block = "people")]
struct PeopleRegion<'a> {
    people: &'a [Login],
}

/// The tab's title, with the count of cards in Needs a person on the whole
/// board.
fn board_title(board: &Board, repo: &str) -> String {
    format!("({}) Board · {repo}", board.needing_a_person)
}

/// The board's stream, narrowed to `person`, if set.
fn events_path(person: Option<&Login>) -> String {
    let person = person.map_or("", |person| &person.0);
    with_filters("/events/", &[("person", person)])
}

pub(super) async fn board_page(
    State(sources): State<Sources>,
    Query(filters): Query<FilterQuery>,
) -> Response {
    let repo = &sources.work_dir.repo;
    let person = filters.person();
    let shown = BoardPageShown::of(&api::board_answer(&sources), person.as_ref(), repo);
    BoardPage {
        page: Page::new(shown.title, events_path(person.as_ref())).filling_the_viewport(),
        frame: Frame::of(&sources),
        person: person.map_or_else(String::new, |person| person.0),
        board: shown.board,
        people: shown.people,
    }
    .into_response()
}

/// The board's regions: the frame's, the tab's title, the board narrowed
/// to its person, if set, and the people to suggest.
pub(super) async fn board_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Query(filters): Query<FilterQuery>,
) -> Response {
    let person = filters.person();
    let regions = move || {
        let repo = &sources.work_dir.repo;
        let mut regions = frame_regions(&Frame::of(&sources));
        let shown = BoardPageShown::of(&api::board_answer(&sources), person.as_ref(), repo);
        let title = format!(
            r#"<title id="title">{}</title>"#,
            html::escape(&shown.title)
        );
        regions.push(("title", title));
        regions.push((
            "board",
            render(&BoardRegion {
                board: &shown.board,
            }),
        ));
        regions.push((
            "people",
            render(&PeopleRegion {
                people: &shown.people,
            }),
        ));
        regions
    };
    events(regions, stopping)
}
