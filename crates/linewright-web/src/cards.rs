//! A card's line as a page shows it (`docs/specs/board.md`, Cards), and a
//! lane's head (Lanes).

use linewright_core::actions::LaneHeadScreen;
use linewright_core::board::CardStatus;
use linewright_core::phrase::{Phrase, Status};

use super::html::{self, Html};
use super::names::Names;

/// A card's line as HTML, the one place a page names items by their number
/// alone (`docs/specs/board.md`, Naming items).
pub(super) fn line_html(line: &Phrase, names: &Names) -> Html {
    html::phrase(line, &names.numbers_only())
}

/// A lane's head (`docs/specs/board.md`, Lanes, A lane and
/// Finished lanes): the tracking issue's number and title and its tickets
/// closed of total, or the item's number and title; then when it finished,
/// if it did. What follows the name is one `span` of class `tail`.
pub(super) fn lane_head(head: &LaneHeadScreen, repo: &str) -> Html {
    let name = html::titled(head.number, &head.title, repo);
    let closed = head.tickets.map(|tickets| {
        html::escape(&format!(
            " — {}/{} tickets closed",
            tickets.tickets_closed, tickets.tickets
        ))
    });
    let finished = head
        .finished
        .map(|at| html::escape(" — finished ").then(&html::time(at)));
    let tail = match (closed, finished) {
        (Some(closed), Some(finished)) => Some(closed.then(&finished)),
        (closed, finished) => closed.or(finished),
    };
    match tail {
        Some(tail) => name.then(&html::span("tail", &tail)),
        None => name,
    }
}

/// The status a card status shows as (`docs/specs/board.md`, Cards, Card
/// status).
pub(super) fn status(card: CardStatus) -> Status {
    match card {
        CardStatus::Failed => Status::Failed,
        CardStatus::WaitingForAPerson => Status::Waiting,
        CardStatus::InFlight => Status::InFlight,
        CardStatus::PausedOrQueued => Status::Paused,
    }
}
