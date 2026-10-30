//! The item page's facts about #N (`docs/specs/design_system.md`, The item
//! page): where it stands in the board's terms, what Linewright waits for
//! and the items it relates to, from what `explain` answers of it, the view
//! it answered from and that view's board.

use crate::board::{self, Board, Card, CardStatus, Column, Lane, LaneHeading};
use crate::domain::{ClosedKind, Number, PullState, SubIssue, Timestamp, Tracks, tickets_finished};
use crate::explain::{Answer, ChildRole, ParentRole};
use crate::managed::LeftAloneItem;
use crate::reconcile::Wait;
use crate::runs::Recorded;
use crate::status::View;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemFacts {
    pub number: Number,
    pub title: String,
    /// Whether it is a pull request, which carries a pull request icon.
    pub pull_request: bool,
    /// When `serve` last read it.
    pub last_read: Timestamp,
    pub state: StateFact,
    pub waiting_for: Vec<Wait>,
    pub tracking_issue: Option<Related>,
    pub tickets: Option<Tickets>,
    pub spec_change: Option<Related>,
    /// Those open or merged.
    pub pull_requests: Vec<Related>,
    pub closes: Vec<Related>,
}

/// The State fact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateFact {
    /// Its card and its column, on the board or off it.
    Card { column: Column, card: Card },
    /// A tracking issue, and when its lane finished, if it finished, on the
    /// board or off it.
    Tracking { finished: Option<Timestamp> },
    /// Linewright leaves it, and why.
    LeftAlone(LeftAloneItem),
}

/// An item a fact lists (`docs/specs/design_system.md`, The item page,
/// Related items).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Related {
    /// An item shown as its card is, with its card status; or a tracking
    /// issue heading no lane, by its number and title alone.
    Card {
        number: Number,
        /// `None` where the snapshot does not hold it.
        title: Option<String>,
        pull_request: bool,
        status: Option<CardStatus>,
    },
    /// A tracking issue heading a lane, shown as that lane's head.
    LaneHead(LaneHeading),
}

/// A tracking issue's tickets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tickets {
    pub closed: usize,
    /// Each ticket, in its tracking issue's order.
    pub tickets: Vec<Related>,
}

/// `answer`'s item's facts. `view` is the view `explain` answered from,
/// `board` that view's board, `recorded` every run's record and `now` when
/// `explain` answered.
pub fn item_facts(
    answer: &Answer,
    view: &View,
    board: &Board,
    recorded: &[Recorded],
    now: Timestamp,
) -> ItemFacts {
    let number = answer.number();
    // A managed item's card, on the board or off it.
    let card_of = |item: Number| -> Option<(Column, Card)> {
        if let Some((column, card)) = board.card(item) {
            return Some((column, card.clone()));
        }
        let managed = board
            .managed()
            .iter()
            .find(|managed| managed.number == item)?;
        board::card(managed, view, recorded, now).ok()
    };
    let related = |item: Number, pull_request: bool| {
        if let Some(lane) = board.lane_headed_by(item) {
            return Related::LaneHead(lane.heading());
        }
        let card = card_of(item).map(|(_, card)| card);
        Related::Card {
            number: item,
            title: card
                .as_ref()
                .map(|card| card.title.clone())
                .or_else(|| view.snapshot.title(item).map(str::to_string)),
            pull_request,
            status: card.as_ref().and_then(Card::status),
        }
    };
    let family = answer.family();
    let tracking_issue = family
        .parents
        .iter()
        .find(|parent| parent.role == ParentRole::TrackingIssue)
        .map(|parent| parent.number)
        .or_else(|| {
            family
                .children
                .iter()
                .find(|child| child.role == ChildRole::TrackingIssue)
                .map(|child| child.number)
        });
    let spec_change = family
        .parents
        .iter()
        .find(|parent| parent.role == ParentRole::SpecChange)
        .map(|parent| related(parent.number, true));
    let closes = family
        .parents
        .iter()
        .filter(|parent| parent.role == ParentRole::IssueItCloses)
        .map(|parent| related(parent.number, false))
        .collect();
    let all_tickets = tickets_of(number, view);
    let tickets = all_tickets.as_ref().filter(|tickets| !tickets.is_empty());
    let tickets = tickets.map(|tickets| Tickets {
        closed: tickets.iter().filter(|ticket| !ticket.is_open()).count(),
        tickets: tickets
            .iter()
            .map(|ticket| related(ticket.number, false))
            .collect(),
    });
    let pull_requests = view
        .snapshot
        .issue(number)
        .map(|issue| view.snapshot.pulls_closing(issue))
        .unwrap_or_default()
        .into_iter()
        .filter(|pull| !matches!(pull.state, PullState::Closed { .. }))
        .map(|pull| related(pull.number, true))
        .collect();
    let card = card_of(number);
    let lane = board.lane_headed_by(number);
    let state = if let Some((column, card)) = &card {
        StateFact::Card {
            column: *column,
            card: card.clone(),
        }
    } else if lane.is_some() || all_tickets.is_some() {
        // Its lane's finish time while it is on the board; off it, its
        // tickets'.
        StateFact::Tracking {
            finished: lane
                .and_then(Lane::finished)
                .or_else(|| all_tickets.as_deref().and_then(tickets_finished)),
        }
    } else {
        match answer {
            Answer::LeftAlone(left_alone) => StateFact::LeftAlone(left_alone.item.clone()),
            // Of the items Linewright manages, only a tracking issue has no
            // card.
            Answer::Managed(_) => StateFact::Tracking { finished: None },
        }
    };
    ItemFacts {
        number,
        title: answer.title().to_string(),
        pull_request: card.map_or_else(|| answer.is_pull_request(), |(_, card)| card.pull_request),
        last_read: answer.last_read(),
        state,
        waiting_for: answer.waiting_for().to_vec(),
        tracking_issue: tracking_issue.map(|issue| related(issue, false)),
        tickets,
        spec_change,
        pull_requests,
        closes,
    }
}

/// #`number`'s tickets, the same the lane it heads counts, if it is a
/// tracking issue.
fn tickets_of(number: Number, view: &View) -> Option<Vec<&SubIssue>> {
    let linewright = &view.policy.linewright;
    let (tracks, sub_issues): (Option<Tracks>, &[SubIssue]) =
        if let Some(issue) = view.snapshot.issue(number) {
            (issue.tracks(linewright), &issue.sub_issues)
        } else {
            let closed = view.snapshot.closed.iter().find(|i| i.number == number)?;
            let ClosedKind::Issue { facts, .. } = &closed.kind else {
                return None;
            };
            (closed.tracks(linewright), &facts.sub_issues)
        };
    Some(tracks?.tickets(sub_issues, linewright).collect())
}
