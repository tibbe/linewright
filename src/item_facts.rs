//! The item page's facts about #N (`docs/specs/design_system.md`, The item
//! page): where it stands in the board's terms, what Linewright waits for
//! and the items it relates to, from what `explain` answers of it, the view
//! it answered from and that view's board.

use crate::board::{Board, Card, CardStatus, Column, Lane};
use crate::domain::{ClosedKind, Number, PullState, SubIssue, Timestamp, Tracks};
use crate::explain::{Answer, ChildRole, LeftAloneItem, ParentRole};
use crate::reconcile::Wait;
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
    /// Its card, or the card it would have on its own, and its column.
    Card { column: Column, card: Card },
    /// A tracking issue's lane, whose head's line and card status it takes.
    Lane(Lane),
    /// A tracking issue with no lane, none of its tickets being on the
    /// board.
    Tracking,
    /// Linewright leaves it, and why.
    LeftAlone(LeftAloneItem),
}

/// An item a fact lists, shown as its card is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Related {
    pub number: Number,
    /// `None` where the snapshot does not hold it.
    pub title: Option<String>,
    pub pull_request: bool,
    /// Its card's status, or for a tracking issue its lane's.
    pub status: Option<CardStatus>,
}

/// A tracking issue's tickets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tickets {
    pub closed: usize,
    /// Each ticket, in its tracking issue's order.
    pub tickets: Vec<Related>,
}

/// `answer`'s item's facts. `view` is the view `explain` answered from, and
/// `board` that view's board.
pub fn item_facts(answer: &Answer, view: &View, board: &Board) -> ItemFacts {
    let number = answer.number();
    let related = |item: Number, pull_request: bool| {
        let card = board.card(item).map(|(_, card)| card);
        let lane = board
            .lane_headed_by(item)
            .and_then(|lane| lane.most_pressing())
            .map(|(_, status)| status);
        Related {
            number: item,
            title: card
                .map(|card| card.title.clone())
                .or_else(|| view.snapshot.title(item).map(str::to_string)),
            pull_request,
            status: card.and_then(Card::status).or(lane),
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
    let tickets = tickets_of(number, view).filter(|tickets| !tickets.is_empty());
    let tickets = tickets.map(|tickets| Tickets {
        closed: tickets.iter().filter(|ticket| !ticket.linked.open).count(),
        tickets: tickets
            .iter()
            .map(|ticket| related(ticket.linked.number, false))
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
    let card = board.card(number);
    let lane = board.lane_headed_by(number);
    let state = if let Some((column, card)) = card {
        StateFact::Card {
            column,
            card: card.clone(),
        }
    } else if let Some(lane) = lane {
        StateFact::Lane(lane.clone())
    } else {
        match answer {
            Answer::LeftAlone(left_alone) => StateFact::LeftAlone(left_alone.item.clone()),
            // Of the items Linewright manages, only a tracking issue has no
            // card.
            Answer::Managed(_) => StateFact::Tracking,
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
            let ClosedKind::Issue { sub_issues, .. } = &closed.kind else {
                return None;
            };
            (closed.tracks(linewright), sub_issues)
        };
    Some(tracks?.tickets(sub_issues, linewright).collect())
}
