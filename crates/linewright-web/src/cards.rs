//! A card's line as a page shows it (`docs/specs/board.md`, Cards), and a
//! lane's head (Lanes).

use askama::Template;

use linewright_core::actions::LaneHeadScreen;
use linewright_core::board::CardStatus;
use linewright_core::domain::ItemKind;
use linewright_core::phrase::{Phrase, Status};

use super::html::{self, Html};
use super::icons;
use super::item_parts::{Cut, ItemNumberLink, ItemTitle};
use super::names::Naming;

/// A card's line as HTML, naming the items of the repo `repo` by their number
/// alone (`docs/specs/board.md`, Cards, Its line).
pub(super) fn line_html(line: &Phrase, repo: &str) -> Html {
    html::phrase(line, &Naming::CardLine { repo })
}

/// A lane's head (`docs/specs/board.md`, Lanes, A lane's head, A lane and
/// Finished lanes), one `inline-flex` line: the item's icon, then the
/// tracking issue's or the item's title, cut to fit, and its number, then
/// the rest of the head.
pub(super) fn lane_head(head: &LaneHeadScreen, repo: &str) -> Html {
    let name = {
        let title = ItemTitle {
            number: head.number,
            title: &head.title,
            cut: Cut::ToFit,
        };
        let number = ItemNumberLink::after(&title, repo);
        html::rendered(&OneLineName { title, number })
    };
    html::span(
        "inline-flex max-w-full min-w-0 items-center gap-[0.25em]",
        &icons::item_kind_svg(&ItemKind::of(head.pull_request))
            .then(&name)
            .then(&lane_rest(head).unwrap_or_default()),
    )
}

/// A lane head's title and number, on one line.
#[derive(Template)]
#[template(
    source = r#"<span class="inline-flex max-w-full min-w-0 items-baseline gap-[0.25em]">{{ title }} {{ number }}</span>"#,
    ext = "html"
)]
struct OneLineName<'a> {
    title: ItemTitle<'a>,
    number: ItemNumberLink<'a>,
}

/// The rest of a lane's head, if it has any: the tracking issue's tickets
/// closed of total, and when it finished, if it did, in one `span` of
/// class `tail`.
pub(super) fn lane_rest(head: &LaneHeadScreen) -> Option<Html> {
    let closed = head.tickets.map(|tickets| {
        html::escape(&format!(
            " — {}/{} tickets closed",
            tickets.tickets_closed, tickets.tickets
        ))
    });
    let finished = head
        .finished
        .map(|at| html::escape(" — finished ").then(&html::time(at)));
    let rest = match (closed, finished) {
        (Some(closed), Some(finished)) => Some(closed.then(&finished)),
        (closed, finished) => closed.or(finished),
    };
    rest.map(|rest| html::span("tail shrink-0 whitespace-nowrap", &rest))
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
