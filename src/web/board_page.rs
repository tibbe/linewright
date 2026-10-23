//! The board (`docs/specs/design_system.md`, The board): the managed items
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
use crate::board::{
    Board, Card, CardLine, Column, Elapsed, Ended, Hold, Lane, LaneHead, Running, Stop,
};
use crate::domain::{IssueCloseReason, ReviewDecision};
use crate::phrase::Phrase;
use crate::reconcile::WorkKind;

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
    line: Option<Html>,
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
        let line = card.line.as_ref().map(worded);
        let entered = card.entered.map(|at| Phrase::default().time(at));
        let line = match (line, entered) {
            (Some(line), Some(entered)) => Some(line.text(" · ").then(entered)),
            (line, entered) => line.or(entered),
        };
        CardShown {
            heading: html::titled(card.number, &card.title, repo),
            pull_request: card.pull_request,
            line: line.map(|line| html::phrase(&line, repo)),
        }
    }
}

/// `line` in the UI's own words.
fn worded(line: &CardLine) -> Phrase {
    match line {
        CardLine::Stalled => "stalled".into(),
        CardLine::ReporterAsked => "reporter asked".into(),
        CardLine::HandedBack => "handed back".into(),
        CardLine::DecisionNeeded => "decision needed".into(),
        CardLine::BlockedBy(issue) => Phrase::from("blocked by ").item(*issue),
        CardLine::AssignedTo(assignee) => format!("assigned to {assignee}").into(),
        CardLine::Unassigned => "unassigned".into(),
        CardLine::Paused(Hold::UsageLimit) => "paused for the usage limit".into(),
        CardLine::Paused(Hold::Drain) => "paused for the drain".into(),
        CardLine::Queued => "queued".into(),
        CardLine::Running(running) => running_worded(*running),
        CardLine::Review {
            pull_request,
            author,
            stops,
        } => {
            let phrase = Phrase::default().item(*pull_request);
            let phrase = match author {
                Some(author) => phrase.text(format!(" by {author}")),
                None => phrase,
            };
            match stops {
                Some(stop) => phrase.text(" · ").then(stop_worded(stop)),
                None => phrase,
            }
        }
        CardLine::MergeStop(stop) => stop_worded(stop),
        CardLine::Ended { ended, at } => ended_worded(*ended).text(" · ").time(*at),
    }
}

/// A run's stage, as a noun, and how long it has run, kept up as it runs.
fn running_worded(running: Running) -> Phrase {
    let stage = match running.stage {
        WorkKind::Triage => "triage",
        WorkKind::Design => "design",
        WorkKind::Implement => "implementation",
        WorkKind::Rework => "rework",
        WorkKind::FixCi => "CI fix",
        WorkKind::Update => "update",
    };
    Phrase::from(format!("{stage} · ")).then(ticking(running.elapsed))
}

/// How long `elapsed` has gone on, kept up as it goes on.
fn ticking(elapsed: Elapsed) -> Phrase {
    Phrase::default().duration(elapsed.seconds, Some(elapsed.since))
}

fn stop_worded(stop: &Stop) -> Phrase {
    match stop {
        Stop::Stalled => "stalled".into(),
        Stop::Running(running) => running_worded(*running),
        Stop::ChecksFailed => "checks failed".into(),
        Stop::ChecksRunning => "checks running".into(),
        Stop::Conflicts => "conflicts".into(),
        Stop::Behind { branch } => format!("behind {branch}").into(),
        Stop::Draft => "draft".into(),
        Stop::Review(ReviewDecision::ChangesRequested) => "changes requested".into(),
        Stop::Review(ReviewDecision::ReviewRequired) => "review required".into(),
        Stop::Review(ReviewDecision::Approved) => "approved".into(),
    }
}

fn ended_worded(ended: Ended) -> Phrase {
    match ended {
        Ended::ClosedIn(pull) => Phrase::from("closed in ").item(pull),
        Ended::Closed(None) => "closed".into(),
        Ended::Closed(Some(IssueCloseReason::Completed)) => "closed as completed".into(),
        Ended::Closed(Some(IssueCloseReason::NotPlanned)) => "closed as not planned".into(),
        Ended::Closed(Some(IssueCloseReason::Duplicate)) => "closed as a duplicate".into(),
        Ended::Merged => "merged".into(),
        Ended::ClosedUnmerged => "closed without merging".into(),
        Ended::NothingToBuild => "merged · nothing to build".into(),
    }
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
        page: Page::new(board_title(&board, repo), "/events/".into()).filling_the_viewport(),
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
