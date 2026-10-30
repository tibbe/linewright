//! The board (`docs/specs/design_system.md`, The board, Cards): every managed
//! item as a card in a column, in a lane per piece of work, from `serve`'s
//! view and the time alone.

use std::cmp::Reverse;

use crate::domain::{
    CheckState, ClosedItem, ClosedKind, Issue, IssueCloseReason, Login, Merge, Mergeability,
    Number, Pull, PullState, ReviewDecision, StatusLabel, SubIssue, Timestamp, Tracks,
    last_labeling, last_unassigned,
};
use crate::reconcile::{NotSpecChange, Wait, WorkKind};
use crate::runs::{Ending, Recorded, RunOf, State};
use crate::status::{self, ItemState, Managed, View};

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
    /// The cards Linewright's pull requests would have on their own, where
    /// each shows on the card of an issue it closes instead
    /// (`docs/specs/design_system.md`, The item page, Its card), which no
    /// lane or count holds.
    own_cards: Vec<(Column, Card)>,
}

impl Board {
    /// The card #`number` has on the board, or for a pull request shown on
    /// the card of an issue it closes, the card it would have on its own.
    pub fn card(&self, number: Number) -> Option<(Column, &Card)> {
        let on_lanes = self.lanes.iter().flat_map(|lane| {
            Column::ALL
                .into_iter()
                .flat_map(move |column| lane.cards(column).iter().map(move |card| (column, card)))
        });
        let own = self.own_cards.iter().map(|(column, card)| (*column, card));
        on_lanes.chain(own).find(|(_, card)| card.number == number)
    }

    /// The lane the tracking issue #`number` heads.
    pub fn lane_headed_by(&self, number: Number) -> Option<&Lane> {
        self.lanes.iter().find(
            |lane| matches!(lane.head, LaneHead::Tracking { number: head, .. } if head == number),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lane {
    pub head: LaneHead,
    /// By column, each ascending by number.
    columns: [Vec<Card>; Column::ALL.len()],
}

impl Lane {
    pub fn cards(&self, column: Column) -> &[Card] {
        &self.columns[column as usize]
    }

    /// Whether it has no cards.
    fn is_empty(&self) -> bool {
        self.columns.iter().all(Vec::is_empty)
    }

    /// When its last card entered Done, if every card is in Done
    /// (`docs/specs/design_system.md`, The board, Finished lanes).
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
                Some(CardLine::Ended { at, .. }) => Some(at),
                _ => None,
            })
            .max()
    }

    /// What its head shows.
    pub fn heading(&self) -> LaneHeading {
        LaneHeading {
            head: self.head.clone(),
            finished: self.finished(),
        }
    }

    fn of(head: LaneHead) -> Lane {
        Lane {
            head,
            columns: Default::default(),
        }
    }

    /// When any of its cards last changed (`docs/specs/design_system.md`,
    /// The board, A lane's last change).
    fn last_change(&self) -> Timestamp {
        self.columns
            .iter()
            .flatten()
            .map(|card| card.changed)
            .max()
            .unwrap_or_default()
    }

    /// Its place on the board (`docs/specs/design_system.md`, The board, Order
    /// of lanes): the unfinished first, latest changed first, then the
    /// finished, latest finished first, then by number.
    fn order(&self) -> (bool, Reverse<Timestamp>, Number) {
        let finished = self.finished();
        let latest = finished.unwrap_or_else(|| self.last_change());
        (finished.is_some(), Reverse(latest), self.head.number())
    }
}

/// What a lane is the work of (`GLOSSARY.md`, **Lane**).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaneHead {
    /// Work cut into tickets: the tracking issue, with its tickets as the
    /// lane's cards.
    Tracking {
        number: Number,
        title: String,
        tickets_closed: usize,
        tickets: usize,
    },
    /// An item worked as one, the lane's one card: an issue, a spec change's
    /// pull request, or a pull request of Linewright's that closes no issue
    /// on the board.
    Item { number: Number, title: String },
}

/// What a lane's head shows (`docs/specs/design_system.md`, The board,
/// Lanes and Finished lanes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneHeading {
    pub head: LaneHead,
    /// When it finished, if it did (`docs/specs/design_system.md`, The
    /// board, Finished lanes).
    pub finished: Option<Timestamp>,
}

impl LaneHead {
    pub fn number(&self) -> Number {
        match self {
            LaneHead::Tracking { number, .. } | LaneHead::Item { number, .. } => *number,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub number: Number,
    pub title: String,
    /// Whether it is a pull request, which carries a pull request icon.
    pub pull_request: bool,
    /// What its column does not say; `None` shows the title alone, or
    /// nothing but when it entered its column where it says so.
    pub line: Option<CardLine>,
    /// When it last entered its column (`docs/specs/design_system.md`,
    /// Cards, When), which its line ends with: set in Triage, Needs a person
    /// and Ready alone.
    pub entered: Option<Timestamp>,
    /// When it last changed, for its lane's order.
    changed: Timestamp,
}

impl Card {
    fn new(
        number: Number,
        title: &str,
        pull_request: bool,
        line: Option<CardLine>,
        changed: Timestamp,
    ) -> Card {
        Card {
            number,
            title: title.to_string(),
            pull_request,
            line,
            entered: None,
            changed,
        }
    }

    /// Its card status, that of its line.
    pub fn status(&self) -> Option<CardStatus> {
        self.line.as_ref().and_then(CardLine::status)
    }

    /// The card in Done of an item that ended as `ended`, entering Done at
    /// `at`.
    fn ended(
        number: Number,
        title: &str,
        ended: Ended,
        at: Timestamp,
        changed: Timestamp,
    ) -> (Column, Card) {
        let line = CardLine::Ended { ended, at };
        let card = Card::new(number, title, ended.of_pull_request(), Some(line), changed);
        (Column::Done, card)
    }
}

/// The line under a card's title (`docs/specs/design_system.md`, Cards):
/// what sets the item apart from the rest of its column, or what a person
/// acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardLine {
    // Needs a person.
    /// Handed back in a stall (`GLOSSARY.md`, **Stall**).
    Stalled,
    /// The people asked (`docs/specs/design_system.md`, Cards).
    Asked(Vec<Login>),
    HandedBack,
    DecisionNeeded,
    // Ready, and Working with no run in flight.
    BlockedBy(Number),
    AssignedTo(Login),
    /// Linewright was unassigned from it.
    Unassigned,
    Paused(Hold),
    Queued,
    // Working.
    Running(Running),
    // In review.
    /// Of an item other than its pull request.
    Review {
        pull_request: Number,
        /// Who opened it, when not Linewright.
        author: Option<Login>,
        /// What stops it merging, the first that holds.
        stops: Option<Stop>,
    },
    /// Of the pull request itself, what stops it merging.
    MergeStop(Stop),
    // Done.
    Ended {
        ended: Ended,
        /// When it entered Done.
        at: Timestamp,
    },
}

impl CardLine {
    /// Its card status (`docs/specs/design_system.md`, Cards, Card status),
    /// none where the table gives none.
    pub fn status(&self) -> Option<CardStatus> {
        match self {
            CardLine::Stalled => Some(CardStatus::Failed),
            CardLine::Asked(_) | CardLine::HandedBack | CardLine::DecisionNeeded => {
                Some(CardStatus::WaitingForAPerson)
            }
            CardLine::Paused(_) | CardLine::Queued => Some(CardStatus::PausedOrQueued),
            CardLine::Running(_) => Some(CardStatus::InFlight),
            CardLine::Review { stops, .. } => stops.as_ref().and_then(Stop::status),
            CardLine::MergeStop(stop) => stop.status(),
            CardLine::BlockedBy(_)
            | CardLine::AssignedTo(_)
            | CardLine::Unassigned
            | CardLine::Ended { .. } => None,
        }
    }
}

/// How pressing what a card's line reports is (`GLOSSARY.md`, **Card
/// status**; `docs/specs/design_system.md`, Cards), most pressing first, so
/// that the least is the most pressing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CardStatus {
    Failed,
    WaitingForAPerson,
    InFlight,
    PausedOrQueued,
}

/// What holds back a run that would start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    UsageLimit,
    Drain,
}

/// A run in flight: its stage and how long it has run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Running {
    pub stage: WorkKind,
    pub elapsed: Elapsed,
}

/// How long a run has gone on, kept up as it goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Elapsed {
    pub since: Timestamp,
    pub seconds: u64,
}

impl Elapsed {
    /// From `since` to `now`, none if `since` is later.
    fn of(since: Timestamp, now: Timestamp) -> Elapsed {
        Elapsed {
            since,
            seconds: u64::try_from((now - since).num_seconds()).unwrap_or(0),
        }
    }
}

/// What stops a pull request merging.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Handed back in a stall (`GLOSSARY.md`, **Stall**).
    Stalled,
    Running(Running),
    ChecksFailed,
    ChecksRunning,
    Conflicts,
    /// Behind the default branch, `branch`.
    Behind {
        branch: String,
    },
    Draft,
    /// GitHub's review decision is that changes are requested.
    ChangesRequested,
    /// GitHub requires a review it has not had ([`Pull::review_required`]).
    ReviewRequired,
    /// GitHub's review decision is approved.
    Approved,
}

impl Stop {
    /// Its card status, as the table of what stops a merge gives it
    /// (`docs/specs/design_system.md`, Cards).
    pub fn status(&self) -> Option<CardStatus> {
        match self {
            Stop::Stalled | Stop::ChecksFailed | Stop::Conflicts => Some(CardStatus::Failed),
            Stop::Running(_) | Stop::ChecksRunning => Some(CardStatus::InFlight),
            Stop::ChangesRequested | Stop::ReviewRequired => Some(CardStatus::WaitingForAPerson),
            Stop::Behind { .. } | Stop::Draft | Stop::Approved => None,
        }
    }
}

/// How an item in Done ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// An issue the pull request closes, the first to merge since it was
    /// last opened.
    ClosedIn(Number),
    /// An issue closed otherwise, with GitHub's reason where it is known.
    Closed(Option<IssueCloseReason>),
    /// A pull request of Linewright's, merged.
    Merged,
    /// A pull request of Linewright's, closed without merging.
    ClosedUnmerged,
    /// A spec change designed with nothing to build.
    NothingToBuild,
}

impl Ended {
    /// Whether it is how a pull request ended, rather than an issue.
    fn of_pull_request(self) -> bool {
        match self {
            Ended::ClosedIn(_) | Ended::Closed(_) => false,
            Ended::Merged | Ended::ClosedUnmerged | Ended::NothingToBuild => true,
        }
    }
}

/// `recorded` is every run's record, which tells whether a run handed an
/// issue back.
pub fn board(view: &View, recorded: &[Recorded], now: Timestamp) -> Board {
    let snapshot = &view.snapshot;
    let linewright = &view.policy.linewright;
    let closed: Vec<&ClosedItem> = snapshot.closed.iter().collect();
    // Linewright's worked pull requests are managed items of their own; the
    // rest are issues and spec changes.
    let (pulls, items): (Vec<Managed>, Vec<Managed>) = status::managed(view, now)
        .into_iter()
        .partition(|managed| snapshot.pull(managed.number).is_some());
    let changed = |on: &[Number]| last_change(view, recorded, on, now);
    let pulls_closing = |item: Number| -> Vec<&Managed> {
        pulls
            .iter()
            .filter(|pull| {
                snapshot
                    .pull(pull.number)
                    .is_some_and(|p| p.closes.contains(&item))
            })
            .collect()
    };

    let default_branch = &snapshot.default_branch;
    // An open issue with a pull request that closes it merged is in Done, the
    // rightmost column, whatever else holds of it, so `column` never sees it.
    let merged: Vec<(&Issue, Ended, Timestamp)> = snapshot
        .issues
        .iter()
        .filter(|issue| issue.open && issue.tracks(linewright).is_none())
        .filter_map(|issue| {
            let closed_by = snapshot.pulls_closing(issue);
            let merge = issue.merged_since_opened(&closed_by, default_branch);
            let (ended, at) = issue_ended(merge, None)?;
            issue
                .worked_by(linewright, &closed_by)
                .then_some((issue, ended, at))
        })
        .collect();

    let mut cards: Vec<(Column, Card)> = vec![];
    let mut own_cards: Vec<(Column, Card)> = vec![];
    for &(issue, ended, at) in &merged {
        cards.push(Card::ended(
            issue.number,
            &issue.title,
            ended,
            at,
            changed(&[issue.number]),
        ));
    }
    let in_done = |number: Number| merged.iter().any(|(issue, ..)| issue.number == number);
    for managed in items.iter().filter(|managed| !in_done(managed.number)) {
        let folded = pulls_closing(managed.number);
        let mut on: Vec<Number> = vec![managed.number];
        on.extend(folded.iter().map(|pull| pull.number));
        let run = run_on(view, &on, now);
        let Some(column) = column(&managed.state, run.is_some()) else {
            continue;
        };
        let waits: Vec<Wait> = managed
            .waiting_for
            .iter()
            .chain(folded.iter().flat_map(|pull| &pull.waiting_for))
            .cloned()
            .collect();
        let issue = snapshot.issue(managed.number);
        let line = match column {
            Column::Triage | Column::Done => None,
            Column::NeedsAPerson => needs_a_person_line(
                &managed.state,
                snapshot.is_stalled(managed.number),
                issue,
                recorded,
                linewright,
            ),
            Column::Ready => ready_line(issue, &waits),
            Column::Working => working_line(run, &waits),
            Column::InReview => review_line(&managed.state, issue, run, view),
        };
        // A spec change not yet designed is its merged pull request.
        let pull_request = snapshot.merged_pull(managed.number).is_some();
        let card = Card {
            entered: entered(column, managed.number, view),
            ..Card::new(
                managed.number,
                &managed.title,
                pull_request,
                line,
                changed(&on),
            )
        };
        cards.push((column, card));
    }
    for item in &closed {
        let ClosedKind::Issue {
            facts,
            merged_by,
            reason,
            ..
        } = &item.kind
        else {
            continue;
        };
        if item.tracks(linewright).is_some() || !item.worked_by(linewright) {
            continue;
        }
        let closing = Closing {
            at: item.closed_at,
            merged_by: *merged_by,
            reason: *reason,
        };
        let merge = facts.work.merged_since_opened(default_branch);
        let Some((ended, at)) = issue_ended(merge, Some(closing)) else {
            continue;
        };
        cards.push(Card::ended(
            item.number,
            &item.title,
            ended,
            at,
            changed(&[item.number]),
        ));
    }
    let on_the_board: Vec<Number> = cards.iter().map(|(_, card)| card.number).collect();
    let closes_none = |closes: &[Number]| !closes.iter().any(|n| on_the_board.contains(n));
    // A pull request of Linewright's closing an issue on the board shows on
    // that issue's card, and keeps the card it would have on its own apart.
    let mut place = |closes: &[Number], card: (Column, Card)| {
        if closes_none(closes) {
            cards.push(card);
        } else {
            own_cards.push(card);
        }
    };
    for managed in &pulls {
        let Some(pull) = snapshot.pull(managed.number) else {
            continue;
        };
        let run = run_on(view, &[managed.number], now);
        let line = merge_stop(managed.number, None, run, view).map(CardLine::MergeStop);
        let pull_request = true;
        let changed = changed(&[managed.number]);
        let card = Card::new(managed.number, &managed.title, pull_request, line, changed);
        place(&pull.closes, (Column::InReview, card));
    }
    for item in &closed {
        let ClosedKind::Pull { closes } = &item.kind else {
            continue;
        };
        let card = Card::ended(
            item.number,
            &item.title,
            Ended::ClosedUnmerged,
            item.closed_at,
            changed(&[item.number]),
        );
        place(closes, card);
    }
    for pr in &snapshot.merged {
        if &pr.author == linewright {
            let card = Card::ended(
                pr.number,
                &pr.title,
                Ended::Merged,
                pr.merged_at,
                changed(&[pr.number]),
            );
            place(&pr.closes, card);
        }
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
            let ClosedKind::Issue { facts, .. } = &item.kind else {
                return None;
            };
            Some(Tracker::new(
                item.number,
                &item.title,
                tracks,
                &facts.sub_issues,
                linewright,
            ))
        }))
        .collect();
    trackers.sort_by_key(|tracker| tracker.number);

    for pr in &snapshot.merged {
        let Err(NotSpecChange::Designed { label }) =
            status::spec_change_standing(pr, snapshot, now, &view.policy)
        else {
            continue;
        };
        let cut = trackers
            .iter()
            .any(|tracker| tracker.tracks == Tracks::SpecChange(pr.number));
        if !cut {
            let designed = last_labeling(&pr.events, label.name())
                .map_or(pr.merged_at, |labeling| labeling.at);
            cards.push(Card::ended(
                pr.number,
                &pr.title,
                Ended::NothingToBuild,
                designed,
                changed(&[pr.number]),
            ));
        }
    }

    cards.sort_by_key(|(_, card)| card.number);
    let mut lanes: Vec<Lane> = trackers
        .iter()
        .map(|tracker| Lane::of(tracker.head()))
        .collect();
    for (column, card) in cards {
        let tracker = trackers
            .iter()
            .position(|tracker| tracker.tickets.contains(&card.number));
        match tracker {
            Some(i) => lanes[i].columns[column as usize].push(card),
            None => {
                let mut lane = Lane::of(LaneHead::Item {
                    number: card.number,
                    title: card.title.clone(),
                });
                lane.columns[column as usize].push(card);
                lanes.push(lane);
            }
        }
    }
    // A tracking issue with none of its tickets on the board has no lane,
    // and a finished lane leaves the board whole once it finished before the
    // Done window opened.
    let windows = view.policy.windows(now);
    lanes.retain(|lane| {
        !lane.is_empty() && lane.finished().is_none_or(|at| windows.in_done_window(at))
    });
    lanes.sort_by_key(Lane::order);
    let needing_a_person = lanes
        .iter()
        .map(|lane| lane.cards(Column::NeedsAPerson).len())
        .sum();
    Board {
        lanes,
        needing_a_person,
        own_cards,
    }
}

/// An issue's closing: when, the pull request whose merge closed it where it
/// is known, and GitHub's reason.
struct Closing {
    at: Timestamp,
    merged_by: Option<Number>,
    reason: Option<IssueCloseReason>,
}

/// Whether an issue Linewright worked is in Done (`docs/specs/design_system.md`,
/// The board, Cards) and if so how it ended and when it entered Done, from
/// `merge`, the first pull request that closes it to merge into the default
/// branch since it was last opened, and `closing`, if it is closed: either
/// puts it in Done, and "closed in" names that pull request before the
/// closer.
fn issue_ended(merge: Option<Merge>, closing: Option<Closing>) -> Option<(Ended, Timestamp)> {
    match (merge, closing) {
        (Some(merge), closing) => Some((
            Ended::ClosedIn(merge.pull_request),
            closing.map_or(merge.at, |closing| merge.at.min(closing.at)),
        )),
        (None, Some(closing)) => Some((
            closing
                .merged_by
                .map_or(Ended::Closed(closing.reason), Ended::ClosedIn),
            closing.at,
        )),
        (None, None) => None,
    }
}

/// The first of `rules` that holds.
fn first<T>(rules: impl IntoIterator<Item = Option<T>>) -> Option<T> {
    rules.into_iter().flatten().next()
}

/// The line in Needs a person, of an item `stalled` or not.
fn needs_a_person_line(
    state: &ItemState,
    stalled: bool,
    issue: Option<&Issue>,
    recorded: &[Recorded],
    linewright: &Login,
) -> Option<CardLine> {
    let let_go = issue.is_some_and(|issue| let_go_of(issue, recorded, linewright));
    let asked = || CardLine::Asked(issue.map_or(vec![], |issue| issue.assignees_but(linewright)));
    first([
        stalled.then_some(CardLine::Stalled),
        matches!(state, ItemState::WaitingOnThePeopleAsked).then(asked),
        (matches!(state, ItemState::WithTheMaintainer) && let_go).then_some(CardLine::HandedBack),
        matches!(state, ItemState::WithTheMaintainer).then_some(CardLine::DecisionNeeded),
    ])
}

/// Whether Linewright's last act on `issue` was letting go of it: of its
/// runs on the issue, by when each ended, and its pull requests closing the
/// issue that were closed unmerged, the latest is a run that handed it back
/// or such a closing.
fn let_go_of(issue: &Issue, recorded: &[Recorded], linewright: &Login) -> bool {
    let runs = recorded
        .iter()
        .filter(|run| run.id.of.item() == Some(issue.number))
        .filter_map(|run| match run.state {
            State::Ended { at, ending } => Some((at, ending == Ending::HandedBack)),
            State::InFlight | State::Interrupted => None,
        });
    let closings = issue
        .closed_by
        .iter()
        .filter(|pull| &pull.author == linewright)
        .filter_map(|pull| match pull.state {
            PullState::Closed { at } => Some((at, true)),
            PullState::Open { .. } | PullState::Merged { .. } => None,
        });
    runs.chain(closings)
        .max_by_key(|(at, _)| *at)
        .is_some_and(|(_, let_go)| let_go)
}

/// The line in Ready, from its issue's open blockers, if it is an issue,
/// and what it waits for. An issue waiting for someone else or for its
/// label again waits for nothing past that, so its blockers come from the
/// issue itself.
fn ready_line(issue: Option<&Issue>, waits: &[Wait]) -> Option<CardLine> {
    let blocker = issue.and_then(|issue| issue.open_blockers().into_iter().min());
    let assignee = waits.iter().find_map(|wait| {
        let Wait::Assignee { assignee } = wait else {
            return None;
        };
        Some(assignee.clone())
    });
    let unassigned = waits.iter().any(Wait::is_unassigned);
    first([
        blocker.map(CardLine::BlockedBy),
        assignee.map(CardLine::AssignedTo),
        unassigned.then_some(CardLine::Unassigned),
        held(waits),
    ])
}

/// The line in Working: its run in flight, or else as in Ready, what holds
/// back the run that would start.
fn working_line(run: Option<Running>, waits: &[Wait]) -> Option<CardLine> {
    first([run.map(CardLine::Running), held(waits)])
}

/// A hold, or the runs in flight at `max-runs`, holding back the run that
/// would start.
fn held(waits: &[Wait]) -> Option<CardLine> {
    let waits_for = |holds: fn(&Wait) -> bool| waits.iter().any(holds);
    first([
        waits_for(|wait| matches!(wait, Wait::UsageLimit { .. }))
            .then_some(CardLine::Paused(Hold::UsageLimit)),
        waits_for(|wait| matches!(wait, Wait::Drain)).then_some(CardLine::Paused(Hold::Drain)),
        waits_for(|wait| matches!(wait, Wait::FreeRun { .. })).then_some(CardLine::Queued),
    ])
}

/// The line in In review of an item other than its pull request: its pull
/// request and what stops it merging.
fn review_line(
    state: &ItemState,
    issue: Option<&Issue>,
    run: Option<Running>,
    view: &View,
) -> Option<CardLine> {
    let (pull_request, author) = if let ItemState::InReview(review) = state {
        (review.pull_request, None)
    } else if let ItemState::InReviewElsewhere {
        pull_request,
        author,
    } = state
    {
        (
            *pull_request,
            Some(author.clone()).filter(|author| author != &view.policy.linewright),
        )
    } else {
        return None;
    };
    Some(CardLine::Review {
        pull_request,
        author,
        stops: merge_stop(pull_request, issue, run, view),
    })
}

/// What stops `pull_request` merging, the first that holds. Someone else's
/// pull request the snapshot does not hold is known only by `issue`'s
/// reference to it.
fn merge_stop(
    pull_request: Number,
    issue: Option<&Issue>,
    run: Option<Running>,
    view: &View,
) -> Option<Stop> {
    let snapshot = &view.snapshot;
    let pull = snapshot.pull(pull_request);
    let pull_state = pull.map(|pull| pull.state).or_else(|| {
        issue?
            .closed_by
            .iter()
            .find(|closing| closing.number == pull_request)
            .map(|closing| closing.state)
    });
    let checks = |held: CheckState| {
        pull.is_some_and(|pull| pull.checks.iter().any(|check| check.state == held))
    };
    let mergeability = pull.map(|pull| &pull.mergeability);
    let stops = [
        pull.is_some_and(Pull::is_stalled).then_some(Stop::Stalled),
        run.map(Stop::Running),
        checks(CheckState::Failed).then_some(Stop::ChecksFailed),
        checks(CheckState::Pending).then_some(Stop::ChecksRunning),
        matches!(mergeability, Some(Mergeability::Conflicting)).then_some(Stop::Conflicts),
        matches!(mergeability, Some(Mergeability::Behind)).then(|| Stop::Behind {
            branch: snapshot.default_branch.clone(),
        }),
        matches!(pull_state, Some(PullState::Open { draft: true })).then_some(Stop::Draft),
        pull.is_some_and(|pull| pull.review_decision == Some(ReviewDecision::ChangesRequested))
            .then_some(Stop::ChangesRequested),
        pull.is_some_and(Pull::review_required)
            .then_some(Stop::ReviewRequired),
        pull.is_some_and(|pull| pull.review_decision == Some(ReviewDecision::Approved))
            .then_some(Stop::Approved),
    ];
    first(stops)
}

/// A tracking issue the snapshot holds, open or closed, heading its lane.
struct Tracker {
    number: Number,
    title: String,
    tracks: Tracks,
    /// Its tickets, the cards of its lane.
    tickets: Vec<Number>,
    tickets_closed: usize,
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
            number,
            title: title.to_string(),
            tracks,
            tickets: tickets.iter().map(|s| s.number).collect(),
            tickets_closed: tickets.iter().filter(|s| !s.is_open()).count(),
        }
    }

    fn head(&self) -> LaneHead {
        LaneHead::Tracking {
            number: self.number,
            title: self.title.clone(),
            tickets_closed: self.tickets_closed,
            tickets: self.tickets.len(),
        }
    }
}

/// When anything last happened to `items`, a card's item and its pull
/// requests (`docs/specs/design_system.md`, The board, A lane's last
/// change): on GitHub, as it last updated them and every pull request
/// closing them, and as those merged or closed, and in Linewright, as its
/// runs on them started and ended. A run in flight is happening `now`.
fn last_change(view: &View, recorded: &[Recorded], items: &[Number], now: Timestamp) -> Timestamp {
    let snapshot = &view.snapshot;
    let on_github = items.iter().filter_map(|&item| snapshot.updated_at(item));
    let closings = items
        .iter()
        .filter_map(|&item| snapshot.issue(item))
        .flat_map(|issue| &issue.closed_by)
        .flat_map(|pull| match pull.state {
            PullState::Merged { at } | PullState::Closed { at } => {
                [Some(at), Some(pull.updated_at)]
            }
            PullState::Open { .. } => [None, Some(pull.updated_at)],
        })
        .flatten();
    let runs = recorded
        .iter()
        .filter(|run| run.id.of.item().is_some_and(|item| items.contains(&item)))
        .flat_map(|run| {
            let ended = match run.state {
                State::Ended { at, .. } => Some(at),
                State::InFlight | State::Interrupted => None,
            };
            [Some(run.started_at), ended]
        })
        .flatten();
    let in_flight = run_on(view, items, now).map(|_| now);
    on_github
        .chain(closings)
        .chain(runs)
        .chain(in_flight)
        .max()
        .unwrap_or_default()
}

/// When the item #`number` entered `column`, where its card says so: when
/// it last entered one of the column's states, by the latest of the events
/// read that move it there, or else when it was created, or for a spec
/// change when it merged.
fn entered(column: Column, number: Number, view: &View) -> Option<Timestamp> {
    let snapshot = &view.snapshot;
    let linewright = &view.policy.linewright;
    let entered = if let Some(issue) = snapshot.issue(number) {
        let moves = match column {
            Column::Triage => vec![
                issue.last_labeled(StatusLabel::NeedsTriage),
                issue.last_reopened(),
            ],
            Column::NeedsAPerson => vec![
                issue
                    .status_label()
                    .and_then(|held| issue.last_labeled(held)),
            ],
            Column::Ready => vec![
                issue.last_labeled(StatusLabel::ReadyForAgent),
                last_unassigned(&issue.events, linewright),
            ],
            Column::Working | Column::InReview | Column::Done => return None,
        };
        moves
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(issue.created_at)
    } else {
        let pr = snapshot
            .merged_pull(number)
            .filter(|_| column == Column::Ready)?;
        let required = view
            .policy
            .require_label
            .as_deref()
            .and_then(|required| last_labeling(&pr.events, required))
            .map(|order| order.at);
        [last_unassigned(&pr.events, linewright), required]
            .into_iter()
            .flatten()
            .fold(pr.merged_at, Timestamp::max)
    };
    Some(entered)
}

/// The run in flight on any of `items`.
fn run_on(view: &View, items: &[Number], now: Timestamp) -> Option<Running> {
    view.in_flight.iter().find_map(|run| {
        let RunOf::Work { item, stage } = run.id.of else {
            return None;
        };
        items.contains(&item).then(|| Running {
            stage,
            elapsed: Elapsed::of(run.started_at, now),
        })
    })
}

/// The rightmost column whose facts hold of an open item: its state and
/// whether a run is in flight on it. A tracking issue fits none. A closed
/// item is in Done, the rightmost of all.
fn column(state: &ItemState, run_in_flight: bool) -> Option<Column> {
    use ItemState as S;
    if matches!(state, S::Tracking { .. }) {
        return None;
    }
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
            matches!(state, S::WaitingOnThePeopleAsked | S::WithTheMaintainer),
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
