//! The board (`docs/specs/monitoring.md`, The board, Cards): every managed
//! item as a card in a column, in a lane per tracking issue, from `serve`'s
//! view and the time alone.

use std::cmp::Reverse;

use crate::domain::{ClosedItem, ClosedKind, Login, Number, SubIssue, Timestamp, Tracks};
use crate::phrase::Phrase;
use crate::reconcile::{NotSpecChange, Wait, not_a_spec_change};
use crate::runs::RunInFlight;
use crate::status::{self, InReview, ItemState, Managed, View};

/// In the board's order, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Column {
    Triage,
    NeedsAPerson,
    Ready,
    Working,
    InReview,
    Done,
}

impl Column {
    pub const ALL: [Column; 6] = [
        Column::Triage,
        Column::NeedsAPerson,
        Column::Ready,
        Column::Working,
        Column::InReview,
        Column::Done,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Column::Triage => "Triage",
            Column::NeedsAPerson => "Needs a person",
            Column::Ready => "Ready",
            Column::Working => "Working",
            Column::InReview => "In review",
            Column::Done => "Done",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Board {
    pub lanes: Vec<Lane>,
    /// The count of cards in Needs a person.
    pub needing_a_person: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lane {
    /// `None` for the lane of the items with no tracking issue.
    pub head: Option<LaneHead>,
    /// By column, each ascending by number.
    columns: [Vec<Card>; Column::ALL.len()],
}

impl Lane {
    pub fn cards(&self, column: Column) -> &[Card] {
        &self.columns[column as usize]
    }

    /// The count of its cards.
    pub fn count(&self) -> usize {
        self.columns.iter().map(Vec::len).sum()
    }

    /// When its last card closed, if every card is in Done.
    pub fn finished(&self) -> Option<Timestamp> {
        let open = Column::ALL
            .into_iter()
            .filter(|&column| column != Column::Done)
            .any(|column| !self.cards(column).is_empty());
        if open {
            return None;
        }
        self.cards(Column::Done)
            .iter()
            .filter_map(|card| match card.line {
                CardLine::Ended { at, .. } => Some(at),
                CardLine::Run(_) | CardLine::Waits(_) => None,
            })
            .max()
    }

    fn of(head: Option<LaneHead>) -> Lane {
        Lane {
            head,
            columns: Default::default(),
        }
    }

    /// Its place on the board (`docs/specs/monitoring.md`, The board, Order
    /// of lanes): by its group, the finished latest first, then by number,
    /// with no tracking issue last.
    fn order(&self) -> (Group, Reverse<Option<Timestamp>>, bool, Option<Number>) {
        let finished = self.finished();
        let group = if finished.is_some() {
            Group::Finished
        } else if self.cards(Column::NeedsAPerson).is_empty() {
            Group::Unfinished
        } else {
            Group::NeedingAPerson
        };
        let number = self.head.as_ref().map(|head| head.number);
        (group, Reverse(finished), number.is_none(), number)
    }
}

/// The groups of lanes, in the board's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    /// With a card in Needs a person.
    NeedingAPerson,
    Unfinished,
    Finished,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneHead {
    pub number: Number,
    pub title: String,
    pub tickets_closed: usize,
    pub tickets: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub number: Number,
    pub title: String,
    /// Where it stands, as `explain` words it.
    pub standing: Phrase,
    pub line: CardLine,
    /// Linewright's pull request that closes it.
    pub pull: Option<InReview>,
}

impl Card {
    /// The card of an open managed item, with its run in flight, if any, or
    /// else its waits.
    fn of(
        managed: &Managed,
        run: Option<RunInFlight>,
        waits: Vec<Wait>,
        pull: Option<InReview>,
    ) -> Card {
        Card {
            number: managed.number,
            title: managed.title.clone(),
            standing: status::describe(&managed.state),
            line: run.map_or(CardLine::Waits(waits), CardLine::Run),
            pull,
        }
    }

    /// The card in Done of an item that ended as `ended` at `closed_at`,
    /// standing as `explain` words a closed or merged item.
    fn ended(
        number: Number,
        title: &str,
        standing: &str,
        ended: Ended,
        closed_at: Timestamp,
    ) -> (Column, Card) {
        let card = Card {
            number,
            title: title.to_string(),
            standing: standing.into(),
            line: CardLine::Ended {
                ended,
                at: closed_at,
            },
            pull: None,
        };
        (Column::Done, card)
    }
}

/// What the pipeline is doing or waiting for on a card's item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardLine {
    Run(RunInFlight),
    /// Empty when Linewright waits for nothing: it acts at its next poll.
    Waits(Vec<Wait>),
    /// In Done: how it ended, and when it closed.
    Ended {
        ended: Ended,
        at: Timestamp,
    },
}

/// How an item in Done ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    MergedBy(Number),
    ClosedUnmerged,
    Closed,
    /// A spec change designed with nothing to build.
    NothingToBuild,
}

pub fn board(view: &View, now: Timestamp) -> Board {
    let snapshot = &view.snapshot;
    let linewright = &view.policy.linewright;
    let window_opened = now - view.policy.spec_lookback;
    let closed: Vec<&ClosedItem> = snapshot
        .closed
        .iter()
        .filter(|item| item.closed_since(window_opened))
        .collect();
    // Linewright's worked pull requests are managed items of their own; the
    // rest are issues and spec changes.
    let (pulls, items): (Vec<Managed>, Vec<Managed>) = status::managed(view, now)
        .into_iter()
        .partition(|managed| snapshot.pull(managed.number).is_some());
    let closing = |item: Number| -> Vec<&Managed> {
        pulls
            .iter()
            .filter(|pull| {
                snapshot
                    .pull(pull.number)
                    .is_some_and(|p| p.closes.contains(&item))
            })
            .collect()
    };

    let mut cards: Vec<(Column, Card)> = vec![];
    for managed in &items {
        let folded = closing(managed.number);
        let mut on: Vec<Number> = vec![managed.number];
        on.extend(folded.iter().map(|pull| pull.number));
        let run = run_on(view, &on, now);
        let Some(column) = column(&managed.state, run.is_some()) else {
            continue;
        };
        let waits = managed
            .waiting_for
            .iter()
            .chain(folded.iter().flat_map(|pull| &pull.waiting_for))
            .cloned()
            .collect();
        let pull = if let ItemState::InReview(review) = &managed.state {
            Some(*review)
        } else if let ItemState::InReviewElsewhere { pull_request, .. } = &managed.state {
            snapshot.pull(*pull_request).map(InReview::of)
        } else {
            None
        };
        cards.push((column, Card::of(managed, run, waits, pull)));
    }
    for item in &closed {
        let ClosedKind::Issue { merged_by, .. } = item.kind else {
            continue;
        };
        if item.tracks(linewright).is_some() {
            continue;
        }
        let ended = merged_by.map_or(Ended::Closed, Ended::MergedBy);
        cards.push(Card::ended(
            item.number,
            &item.title,
            "closed issue",
            ended,
            item.closed_at,
        ));
    }
    let on_the_board: Vec<Number> = cards.iter().map(|(_, card)| card.number).collect();
    let closes_none = |closes: &[Number]| !closes.iter().any(|n| on_the_board.contains(n));
    for managed in &pulls {
        let Some(pull) = snapshot.pull(managed.number) else {
            continue;
        };
        if !closes_none(&pull.closes) {
            continue;
        }
        let run = run_on(view, &[managed.number], now);
        let card = Card::of(managed, run, managed.waiting_for.clone(), None);
        cards.push((Column::InReview, card));
    }
    for item in &closed {
        let ClosedKind::Pull { closes } = &item.kind else {
            continue;
        };
        if !closes_none(closes) {
            continue;
        }
        cards.push(Card::ended(
            item.number,
            &item.title,
            "closed pull request",
            Ended::ClosedUnmerged,
            item.closed_at,
        ));
    }

    let mut trackers: Vec<Tracker> = snapshot
        .issues
        .iter()
        .filter(|issue| issue.open)
        .filter_map(|issue| {
            let tracks = issue.tracks(linewright)?;
            Some(Tracker::new(
                issue.number,
                &issue.title,
                tracks,
                &issue.sub_issues,
                linewright,
            ))
        })
        .chain(closed.iter().filter_map(|item| {
            let tracks = item.tracks(linewright)?;
            let ClosedKind::Issue { sub_issues, .. } = &item.kind else {
                return None;
            };
            Some(Tracker::new(
                item.number,
                &item.title,
                tracks,
                sub_issues,
                linewright,
            ))
        }))
        .collect();
    trackers.sort_by_key(|tracker| tracker.head.number);

    for pr in &snapshot.merged {
        let designed = matches!(
            not_a_spec_change(pr, &snapshot.default_branch, now, &view.policy),
            Some(NotSpecChange::Designed { .. })
        );
        let cut = trackers
            .iter()
            .any(|tracker| tracker.tracks == Tracks::SpecChange(pr.number));
        if designed && pr.merged_at >= window_opened && !cut {
            cards.push(Card::ended(
                pr.number,
                &pr.title,
                "merged pull request",
                Ended::NothingToBuild,
                pr.merged_at,
            ));
        }
    }

    cards.sort_by_key(|(_, card)| card.number);
    let mut lanes: Vec<Lane> = trackers
        .iter()
        .map(|tracker| Lane::of(Some(tracker.head.clone())))
        .collect();
    let mut rest = Lane::of(None);
    for (column, card) in cards {
        let lane = trackers
            .iter()
            .position(|tracker| tracker.tickets.contains(&card.number))
            .map_or(&mut rest, |i| &mut lanes[i]);
        lane.columns[column as usize].push(card);
    }
    lanes.push(rest);
    lanes.retain(|lane| lane.count() > 0);
    lanes.sort_by_key(Lane::order);
    let needing_a_person = lanes
        .iter()
        .map(|lane| lane.cards(Column::NeedsAPerson).len())
        .sum();
    Board {
        lanes,
        needing_a_person,
    }
}

/// A tracking issue, open or closed in the window, heading its lane.
struct Tracker {
    head: LaneHead,
    tracks: Tracks,
    /// Its tickets, the cards of its lane.
    tickets: Vec<Number>,
}

impl Tracker {
    fn new(
        number: Number,
        title: &str,
        tracks: Tracks,
        sub_issues: &[SubIssue],
        linewright: &Login,
    ) -> Tracker {
        let tickets: Vec<&SubIssue> = tracks.tickets(sub_issues, linewright).collect();
        Tracker {
            head: LaneHead {
                number,
                title: title.to_string(),
                tickets_closed: tickets.iter().filter(|s| !s.linked.open).count(),
                tickets: tickets.len(),
            },
            tracks,
            tickets: tickets.iter().map(|s| s.linked.number).collect(),
        }
    }
}

/// The run in flight on any of `items`.
fn run_on(view: &View, items: &[Number], now: Timestamp) -> Option<RunInFlight> {
    view.in_flight
        .iter()
        .find(|run| run.id.of.item().is_some_and(|item| items.contains(&item)))
        .map(|run| run.at(now))
}

/// The rightmost column whose facts hold of an open item: its state and
/// whether a run is in flight on it. A tracking issue fits none. A closed
/// item is in Done, the rightmost of all.
fn column(state: &ItemState, run_in_flight: bool) -> Option<Column> {
    use ItemState as S;
    let fits = [
        (
            matches!(state, S::InReview(_) | S::InReviewElsewhere { .. }),
            Column::InReview,
        ),
        (
            matches!(state, S::Claimed | S::BeingDesigned) || run_in_flight,
            Column::Working,
        ),
        (
            matches!(
                state,
                S::ReadyToClaim | S::ReadyToDesign | S::WaitingForDesign
            ),
            Column::Ready,
        ),
        (
            matches!(state, S::WaitingOnTheReporter | S::WithTheMaintainer),
            Column::NeedsAPerson,
        ),
        (
            matches!(state, S::WaitingForTriage | S::BeingTriaged),
            Column::Triage,
        ),
    ];
    fits.into_iter()
        .find(|(fits, _)| *fits)
        .map(|(_, column)| column)
}
