//! A card's line as a page shows it (`docs/specs/board.md`, Cards), and a
//! lane's head (Lanes).

use linewright_core::actions::LaneHeadScreen;
use linewright_core::board::CardStatus;
use linewright_core::domain::ItemKind;
use linewright_core::phrase::{Phrase, Status};

use super::components::{Fit, ItemName};
use super::html::{self, Html};
use super::icons;
use super::names::Naming;

/// A card's line as HTML, naming the items of the repo `repo` by their number
/// alone (`docs/specs/board.md`, Cards, Its line).
pub(super) fn line_html(line: &Phrase, repo: &str) -> Html {
    html::phrase(line, &Naming::CardLine { repo })
}

/// A lane's head (`docs/specs/board.md`, Lanes, A lane's head, A lane and
/// Finished lanes), one `inline-flex` line: the item's icon, then the
/// tracking issue's or the item's item name with only its title cut to fit,
/// then the tracking issue's tickets closed of total, and when it finished,
/// if it did, in one `span` of class `tail`.
pub(super) fn lane_head(head: &LaneHeadScreen, repo: &str) -> Html {
    let name = html::rendered(&ItemName {
        number: head.number,
        title: &head.title,
        repo,
        fit: Fit::OneLine,
    });
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
    let tail = tail.map_or_else(Html::default, |tail| {
        html::span("tail shrink-0 whitespace-nowrap", &tail)
    });
    html::span(
        "inline-flex max-w-full min-w-0 items-center gap-[0.25em]",
        &icons::item_kind_svg(&ItemKind::of(head.pull_request))
            .then(&name)
            .then(&tail),
    )
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
