//! A card's line as a page shows it (`docs/specs/board.md`, Cards), and a
//! lane's head (Lanes).

use askama::Template;

use linewright_core::actions::LaneHeadScreen;
use linewright_core::board::CardStatus;
use linewright_core::domain::ItemKind;
use linewright_core::phrase::{Phrase, Status};

use super::html::{self, Html};
use super::icons;
use super::item_parts::{Cut, ItemNumberLink, ItemTitle, Weight};
use super::names::Naming;

/// A card's line as HTML, naming the items of the repo `repo` by their number
/// alone (`docs/specs/board.md`, Cards, Its line).
pub(super) fn line_html(line: &Phrase, repo: &str) -> Html {
    html::phrase(line, &Naming::CardLine { repo })
}

/// A lane's head (`docs/specs/board.md`, Lanes, A lane's head, A lane and
/// Finished lanes), one `inline-flex` line: the item's icon, then the
/// tracking issue's or the item's title, cut to fit, its number in
/// `font-normal`, then the rest of the head, `gap-1` apart.
pub(super) fn lane_head(head: &LaneHeadScreen, repo: &str) -> Html {
    let title = ItemTitle {
        number: head.number,
        title: &head.title,
        cut: Cut::ToFit,
    };
    html::rendered(&LaneHead {
        icon: icons::item_kind_svg(&ItemKind::of(head.pull_request)),
        number: ItemNumberLink::after(&title, repo, Weight::Normal),
        title,
        rest: lane_rest(head),
    })
}

/// A lane's head, on one line.
#[derive(Template)]
#[template(
    source = r#"<span class="inline-flex max-w-full min-w-0 items-center gap-1">{{ icon|safe }}{{ title }} {{ number }}{% if let Some(rest) = rest %} {{ rest|safe }}{% endif %}</span>"#,
    ext = "html"
)]
struct LaneHead<'a> {
    icon: Html,
    title: ItemTitle<'a>,
    number: ItemNumberLink<'a>,
    rest: Option<Html>,
}

/// The rest of a lane's head, if it has any: the tracking issue's tickets
/// closed of total, and when it finished, if it did, a dash before each,
/// in one `span` of class `tail` that starts with no space.
pub(super) fn lane_rest(head: &LaneHeadScreen) -> Option<Html> {
    let closed = head.tickets.map(|tickets| {
        html::escape(&format!(
            "— {}/{} tickets closed",
            tickets.tickets_closed, tickets.tickets
        ))
    });
    let finished = head
        .finished
        .map(|at| html::escape("— finished ").then(&html::time(at)));
    let rest = match (closed, finished) {
        (Some(closed), Some(finished)) => Some(closed.then(&html::escape(" ")).then(&finished)),
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
