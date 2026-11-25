//! The board (`docs/specs/board.md`, Board): the managed items
//! as cards, in a lane per piece of work and a column per stage.

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::Query;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use linewright_core::actions::{self, BoardScreen, CardScreen, LaneScreen};
use linewright_core::board::Column;
use linewright_core::domain::{ItemKind, Login, Number};
use linewright_core::phrase::Status;

use super::caller::Reader;
use super::cards::{self, lane_head, line_html};
use super::html::Html;
use super::item_parts::{OneLineItem, Weight};
use super::shell::{self, Area, Frame, Navigation, Place};
use super::{Page, events, render, with_filters};

/// The Person filter as the page's address carries it, an empty or blank one
/// leaving the board unnarrowed (`docs/specs/board.md`, Filtering by person).
#[derive(Deserialize)]
pub(super) struct FilterQuery {
    #[serde(default)]
    person: String,
}

impl FilterQuery {
    /// The person the board is narrowed to, if any.
    fn person(&self) -> Option<Login> {
        actions::chosen_person(&self.person)
    }
}

/// The board (`docs/specs/board.md`, Board), ready for the page.
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

/// A card (`docs/specs/board.md`, Cards).
struct CardShown {
    number: Number,
    title: String,
    /// The repo its item is in.
    repo: String,
    kind: ItemKind,
    /// The status its card status shows as, if it has one.
    status: Option<Status>,
    line: Option<Html>,
    /// Whether it is in Done, which is quieter than the rest.
    quiet: bool,
}

impl BoardShown {
    fn of(board: &BoardScreen, repo: &str) -> BoardShown {
        let (finished, unfinished): (Vec<&LaneScreen>, Vec<&LaneScreen>) = board
            .lanes
            .iter()
            .partition(|lane| lane.head.finished.is_some());
        let shown = |lanes: Vec<&LaneScreen>| {
            lanes
                .into_iter()
                .map(|lane| LaneShown::of(lane, repo))
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
    fn of(lane: &LaneScreen, repo: &str) -> LaneShown {
        LaneShown {
            id: format!("lane-{}", lane.head.number.0),
            head: lane_head(&lane.head, repo),
            cells: Column::ALL
                .into_iter()
                .map(|column| CellShown {
                    column: column.name(),
                    cards: lane
                        .cards
                        .iter()
                        .filter(|card| card.column == column)
                        .map(|card| CardShown::of(card, repo))
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
    fn of(card: &CardScreen, repo: &str) -> CardShown {
        CardShown {
            number: card.number,
            title: card.title.clone(),
            repo: repo.to_owned(),
            kind: ItemKind::of(card.pull_request),
            status: card.status.map(cards::status),
            line: card.line.as_ref().map(|line| line_html(line, repo)),
            quiet: card.column == Column::Done,
        }
    }
}

impl CardShown {
    /// Its title, cut to fit its `card-title`, then its number in
    /// `font-normal`.
    fn name(&self) -> OneLineItem<'_> {
        OneLineItem::of(self.number, &self.title, &self.repo, Weight::Normal)
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "board.html")]
struct BoardPage {
    page: Page,
    frame: Frame,
    board: BoardShown,
    people: Vec<Login>,
    /// The Person filter's options: each person's login and whether they
    /// are chosen.
    person_options: Vec<(String, bool)>,
}

#[derive(Template)]
#[template(path = "board.html", block = "board")]
struct BoardRegion<'a> {
    board: &'a BoardShown,
}

/// The people Person is chosen from: the stream's `people` region.
#[derive(Template)]
#[template(path = "board.html", block = "people")]
struct PeopleRegion<'a> {
    people: &'a [Login],
}

/// The board's page, its stream's narrowed to `person`, if set, and its
/// title counting `needing_a_person`.
fn page(person: Option<&Login>, needing_a_person: usize, navigation: Navigation) -> Page {
    Page::new(
        Place::Navigation(Area::Board),
        events_path(person),
        navigation,
    )
    .needing_a_person(needing_a_person)
    .filling_the_viewport()
}

/// The Person filter's options: each of `people`'s logins and whether it is
/// `person`.
fn person_options(people: &[Login], person: Option<&Login>) -> Vec<(String, bool)> {
    people
        .iter()
        .map(|login| (login.0.clone(), person == Some(login)))
        .collect()
}

/// The board's stream, narrowed to `person`, if set.
fn events_path(person: Option<&Login>) -> String {
    let person = person.map_or("", |person| &person.0);
    with_filters("/events/", &[("person", person)])
}

pub(super) async fn board_page(
    Reader { permit, sources }: Reader,
    Query(filters): Query<FilterQuery>,
    navigation: Navigation,
) -> Response {
    let person = filters.person();
    let board = actions::get_board(&permit, &sources, person.as_ref()).await;
    let person_options = person_options(&board.people, person.as_ref());
    BoardPage {
        page: page(person.as_ref(), board.needing_a_person, navigation),
        frame: Frame::of(&permit, &sources).await,
        board: BoardShown::of(&board, &sources.work_dir.repo),
        people: board.people,
        person_options,
    }
    .into_response()
}

/// The board's regions: the shell's, the board narrowed
/// to its person, if set, and the people Person is chosen from.
pub(super) async fn board_events(
    Reader { permit, sources }: Reader,
    watch: super::caller::Watch,
    Query(filters): Query<FilterQuery>,
    navigation: Navigation,
) -> Response {
    let person = filters.person();
    let regions = move || {
        let (permit, sources) = (permit.clone(), sources.clone());
        let person = person.clone();
        async move {
            let frame = Frame::of(&permit, &sources).await;
            let board = actions::get_board(&permit, &sources, person.as_ref()).await;
            let mut regions = shell::regions(
                &frame,
                &page(person.as_ref(), board.needing_a_person, navigation),
            );
            regions.push((
                "board",
                render(&BoardRegion {
                    board: &BoardShown::of(&board, &sources.work_dir.repo),
                }),
            ));
            regions.push((
                "people",
                render(&PeopleRegion {
                    people: &board.people,
                }),
            ));
            regions
        }
    };
    events(regions, watch)
}
