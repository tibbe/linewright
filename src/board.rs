//! The board (`docs/specs/design_system.md`, The board, Cards): every managed
//! item as a card in a column, in a lane per piece of work, from `serve`'s
//! view and the time alone.

use std::cmp::Reverse;

use crate::domain::{
    CheckState, ClosedItem, ClosedKind, Issue, IssueCloseReason, Login, Mergeability, Number, Pull,
    PullState, ReviewDecision, StatusLabel, SubIssue, Timestamp, Tracks, last_labeling,
    last_unassigned,
};
use crate::reconcile::{NotSpecChange, Wait, WorkKind, not_a_spec_change};
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
                Some(CardLine::Ended { at, .. }) => Some(at),
                _ => None,
            })
            .max()
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
    /// nothing but the time where it has sat.
    pub line: Option<CardLine>,
    /// How long it has sat in its column (`docs/specs/design_system.md`,
    /// Cards, When), since it last entered it, which its line ends with: set
    /// in Triage, Needs a person and Ready alone.
    pub sat: Option<Elapsed>,
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
            sat: None,
            changed,
        }
    }

    /// The card in Done of an item that ended as `ended` at `closed_at`.
    fn ended(
        number: Number,
        title: &str,
        ended: Ended,
        closed_at: Timestamp,
        changed: Timestamp,
    ) -> (Column, Card) {
        let line = CardLine::Ended {
            ended,
            at: closed_at,
        };
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
    ReporterAsked,
    HandedBack,
    DecisionNeeded,
    // Ready, and Working with no run in flight.
    BlockedBy(Number),
    AssignedTo(Login),
    Paused(Hold),
    Queued,
    // Working.
    Running(Running),
    // In review.
    Review {
        pull_request: Number,
        /// Who opened it, when not Linewright.
        author: Option<Login>,
        /// What stops it merging, the first that holds.
        stops: Option<Stop>,
    },
    // Done.
    Ended {
        ended: Ended,
        /// When it closed.
        at: Timestamp,
    },
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

/// How long something has gone on, kept up as it goes on: a run, or an
/// item's stay in its column.
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
    /// Its review decision, whatever it is.
    Review(ReviewDecision),
}

/// How an item in Done ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// An issue closed by the merge of the pull request.
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
    let changed = |on: &[Number]| last_change(view, recorded, on, now);
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
            Column::Ready => ready_line(&waits),
            Column::Working => working_line(run, &waits),
            Column::InReview => review_line(&managed.state, issue, run, view),
        };
        // A spec change not yet designed is its merged pull request.
        let pull_request = snapshot.merged_pull(managed.number).is_some();
        let card = Card {
            sat: sat(column, managed.number, view, now),
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
            merged_by, reason, ..
        } = item.kind
        else {
            continue;
        };
        if item.tracks(linewright).is_some() {
            continue;
        }
        let ended = merged_by.map_or(Ended::Closed(reason), Ended::ClosedIn);
        cards.push(Card::ended(
            item.number,
            &item.title,
            ended,
            item.closed_at,
            changed(&[item.number]),
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
        let line = review_line(&managed.state, None, run, view);
        let pull_request = true;
        let changed = changed(&[managed.number]);
        let card = Card::new(managed.number, &managed.title, pull_request, line, changed);
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
            Ended::ClosedUnmerged,
            item.closed_at,
            changed(&[item.number]),
        ));
    }
    for pr in &snapshot.merged {
        if &pr.author == linewright && pr.merged_at >= window_opened && closes_none(&pr.closes) {
            cards.push(Card::ended(
                pr.number,
                &pr.title,
                Ended::Merged,
                pr.merged_at,
                changed(&[pr.number]),
            ));
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
    trackers.sort_by_key(|tracker| tracker.number);

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
                Ended::NothingToBuild,
                pr.merged_at,
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
    // A tracking issue with none of its tickets on the board has no lane.
    lanes.retain(|lane| !lane.is_empty());
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
    first([
        stalled.then_some(CardLine::Stalled),
        matches!(state, ItemState::WaitingOnTheReporter).then_some(CardLine::ReporterAsked),
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

/// The line in Ready, from what the item waits for.
fn ready_line(waits: &[Wait]) -> Option<CardLine> {
    let blocker = waits
        .iter()
        .filter_map(|wait| {
            let Wait::Blocker { issue } = wait else {
                return None;
            };
            Some(*issue)
        })
        .min();
    let assignee = waits.iter().find_map(|wait| {
        let Wait::Assignee { assignee } = wait else {
            return None;
        };
        Some(assignee.clone())
    });
    first([
        blocker.map(CardLine::BlockedBy),
        assignee.map(CardLine::AssignedTo),
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

/// The line in In review: its pull request and what stops it merging.
/// Someone else's pull request the snapshot does not hold is known only by
/// `issue`'s reference to it.
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
        pull.and_then(|pull| pull.review_decision).map(Stop::Review),
    ];
    Some(CardLine::Review {
        pull_request,
        author,
        stops: first(stops),
    })
}

/// A tracking issue, open or closed in the window, heading its lane.
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
            tickets: tickets.iter().map(|s| s.linked.number).collect(),
            tickets_closed: tickets.iter().filter(|s| !s.linked.open).count(),
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

/// How long the item #`number` has sat in `column`, where its card says so:
/// since it last entered one of the column's states, by the latest of the
/// events read that move it there, or else since it was created, or for a
/// spec change since it merged.
fn sat(column: Column, number: Number, view: &View, now: Timestamp) -> Option<Elapsed> {
    let snapshot = &view.snapshot;
    let linewright = &view.policy.linewright;
    let since = if let Some(issue) = snapshot.issue(number) {
        let entered = match column {
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
        entered
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
    Some(Elapsed::of(since, now))
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
