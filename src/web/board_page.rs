//! The board (`docs/specs/monitoring.md`, The board): the managed items
//! as cards, in a lane per piece of work and a column per stage.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use tokio_util::sync::CancellationToken;

use super::html::{self, Html};
use super::{Frame, Page, events, frame_regions, render};
use crate::api::{self, Sources};
use crate::board::{Board, Card, CardLine, Column, Ended, Lane, LaneHead};
use crate::explain;
use crate::phrase::{Phrase, Tone};
use crate::status;

/// The board (`docs/specs/monitoring.md`, The board), ready for the page.
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

/// A card (`docs/specs/monitoring.md`, Cards).
struct CardShown {
    heading: Html,
    standing: Html,
    /// Its run in flight, each wait, or how it ended.
    line: Vec<StatusShown>,
    pull: Option<PullShown>,
}

/// A status's word and what follows it.
struct StatusShown {
    tone: Tone,
    word: &'static str,
    rest: Html,
}

struct PullShown {
    item: Html,
    tone: Option<Tone>,
    ci: &'static str,
}

impl BoardShown {
    fn of(board: &Board, repo: &str) -> BoardShown {
        let (finished, unfinished): (Vec<&Lane>, Vec<&Lane>) = board
            .lanes
            .iter()
            .partition(|lane| lane.finished().is_some());
        let shown = |lanes: Vec<&Lane>| {
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
    fn of(lane: &Lane, repo: &str) -> LaneShown {
        let head = match &lane.head {
            LaneHead::Tracking {
                number,
                title,
                tickets_closed,
                tickets,
            } => html::titled(*number, title, repo).then(&html::escape(&format!(
                " — {tickets_closed}/{tickets} tickets closed"
            ))),
            LaneHead::Item { number, title } => html::titled(*number, title, repo),
        };
        let head = match lane.finished() {
            Some(at) => head
                .then(&html::escape(" — finished "))
                .then(&html::time(at)),
            None => head,
        };
        LaneShown {
            id: format!("lane-{}", lane.head.number().0),
            head,
            cells: Column::ALL
                .into_iter()
                .map(|column| CellShown {
                    column: column.name(),
                    cards: lane
                        .cards(column)
                        .iter()
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
    fn of(card: &Card, repo: &str) -> CardShown {
        let html = |phrase: &Phrase| html::phrase(phrase, repo);
        let after = |phrase: &Phrase| html::escape(": ").then(&html(phrase));
        let line = match &card.line {
            CardLine::Run(run) => vec![StatusShown {
                tone: Tone::InProgress,
                word: "run in flight",
                rest: after(&run.describe()),
            }],
            CardLine::Waits(waits) => {
                let mut waits: Vec<Phrase> = waits.iter().map(status::describe_wait).collect();
                if waits.is_empty() {
                    waits.push(explain::NOTHING_WAITED_FOR.into());
                }
                waits
                    .iter()
                    .map(|wait| StatusShown {
                        tone: Tone::Pending,
                        word: "waiting for",
                        rest: after(wait),
                    })
                    .collect()
            }
            CardLine::Ended { ended, .. } => vec![ended_shown(*ended, repo)],
        };
        CardShown {
            heading: html::titled(card.number, &card.title, repo),
            standing: html(&card.standing),
            line,
            pull: card.pull.map(|pull| PullShown {
                item: html::item(pull.pull_request, repo),
                tone: pull.ci.tone(),
                ci: pull.ci.describe(),
            }),
        }
    }
}

fn ended_shown(ended: Ended, repo: &str) -> StatusShown {
    let (tone, word, rest) = match ended {
        Ended::MergedBy(pull) => (
            Tone::Success,
            "merged",
            html::escape(" by ").then(&html::item(pull, repo)),
        ),
        Ended::ClosedUnmerged => (Tone::Cancelled, "closed without merging", Html::default()),
        Ended::Closed => (Tone::Cancelled, "closed", Html::default()),
        Ended::NothingToBuild => (
            Tone::Success,
            "designed",
            html::escape(": nothing to build"),
        ),
    };
    StatusShown { tone, word, rest }
}

#[derive(Template, WebTemplate)]
#[template(path = "board.html")]
struct BoardPage {
    page: Page,
    frame: Frame,
    board: BoardShown,
}

#[derive(Template)]
#[template(path = "board.html", block = "main")]
struct BoardRegion<'a> {
    board: &'a BoardShown,
}

/// The tab's title, with the count of cards in Needs a person.
fn board_title(board: &Board, repo: &str) -> String {
    format!("({}) Board · {repo}", board.needing_a_person)
}

pub(super) async fn board_page(State(sources): State<Sources>) -> Response {
    let repo = &sources.work_dir.repo;
    let board = api::board_answer(&sources);
    BoardPage {
        page: Page::new(board_title(&board, repo), "/events/".into()),
        frame: Frame::of(&sources),
        board: BoardShown::of(&board, repo),
    }
    .into_response()
}

/// The board's regions: the frame's, the tab's title and the board.
pub(super) async fn board_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
) -> Response {
    let regions = move || {
        let repo = &sources.work_dir.repo;
        let mut regions = frame_regions(&Frame::of(&sources));
        let board = api::board_answer(&sources);
        let title = format!(
            r#"<title id="title">{}</title>"#,
            html::escape(&board_title(&board, repo))
        );
        let shown = BoardShown::of(&board, repo);
        regions.push(("title", title));
        regions.push(("board", render(&BoardRegion { board: &shown })));
        regions
    };
    events(regions, stopping)
}
