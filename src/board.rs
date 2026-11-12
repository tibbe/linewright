//! The board (`docs/specs/design_system.md`, The board, Cards): every managed
//! item as a card in a column, in a lane per piece of work, from a view and
//! the time alone.

use std::cmp::Reverse;
use std::collections::BTreeSet;

use crate::domain::{
    CheckState, ClosedKind, Issue, Login, Marking, Mergeability, Number, Pull, PullState,
    ReviewDecision, StatusLabel, SubIssue, Timestamp, last_labeling, last_unassigned, tickets_cut,
};
use crate::holds::Hold;
use crate::lifecycle::{self, done_at, done_by_merge};
pub use crate::managed::Ended;
use crate::managed::{self, Detail, Managed, Tickets};
use crate::party;
use crate::reconcile::{Policy, Wait, WorkKind};
use crate::runs::{Ending, Recorded, RunOf, State};
use crate::status::View;

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
    /// The managed items, of which it shows those with a card on it.
    managed: Vec<Managed>,
}

impl Board {
    /// The managed items it was built from.
    pub fn managed(&self) -> &[Managed] {
        &self.managed
    }

    /// The card #`number` has on the board.
    pub fn card(&self, number: Number) -> Option<(Column, &Card)> {
        self.lanes
            .iter()
            .flat_map(|lane| {
                Column::ALL.into_iter().flat_map(move |column| {
                    lane.cards(column).iter().map(move |card| (column, card))
                })
            })
            .find(|(_, card)| card.number == number)
    }

    /// It with only the cards `person` is party to, and the lanes left with
    /// a card, each lane's head, finished state and place, and the count of
    /// cards in Needs a person, as they are on it
    /// (`docs/specs/design_system.md`, Filtering by person).
    #[must_use]
    pub fn narrowed(&self, person: &Login) -> Board {
        Board {
            lanes: self
                .lanes
                .iter()
                .filter_map(|lane| lane.narrowed(person))
                .collect(),
            needing_a_person: self.needing_a_person,
            managed: self.managed.clone(),
        }
    }

    /// Each person some card on it matches, sorted by login and each once.
    pub fn people(&self) -> Vec<Login> {
        let mut people: Vec<Login> = self
            .lanes
            .iter()
            .flat_map(|lane| lane.columns.iter().flatten())
            .flat_map(|card| card.people.iter().cloned())
            .collect();
        people.sort();
        people.dedup();
        people
    }

    /// The lane the tracking issue #`number` heads.
    pub fn lane_headed_by(&self, number: Number) -> Option<&Lane> {
        self.lanes.iter().find(
            |lane| matches!(lane.head, LaneHead::Tracking { number: head, .. } if head == number),
        )
    }
}

/// A lane (`GLOSSARY.md`, **Lane**), its head, its finished state and its
/// place on the board those of the whole lane, however it is narrowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lane {
    pub head: LaneHead,
    /// By column, each ascending by number.
    columns: [Vec<Card>; Column::ALL.len()],
    finished: Option<Timestamp>,
    place: Place,
}

/// A lane's place on the board (`docs/specs/design_system.md`, The board,
/// Order of lanes): the unfinished first, latest changed first, then the
/// finished, latest finished first, then by number.
type Place = (bool, Reverse<Timestamp>, Number);

impl Lane {
    pub fn cards(&self, column: Column) -> &[Card] {
        &self.columns[column as usize]
    }

    /// When it finished, once every card is in Done and its tracking issue,
    /// if it has one, is done: when the last of its cards entered Done or
    /// its tracking issue became done, whichever is later
    /// (`docs/specs/design_system.md`, The board, Finished lanes).
    pub fn finished(&self) -> Option<Timestamp> {
        self.finished
    }

    /// What its head shows.
    pub fn heading(&self) -> LaneHeading {
        LaneHeading {
            head: self.head.clone(),
            finished: self.finished,
        }
    }

    /// It with only the cards that match `person`, if any does.
    fn narrowed(&self, person: &Login) -> Option<Lane> {
        let columns = self.columns.clone().map(|cards| {
            cards
                .into_iter()
                .filter(|card| card.people.contains(person))
                .collect::<Vec<_>>()
        });
        columns.iter().any(|cards| !cards.is_empty()).then(|| Lane {
            columns,
            ..self.clone()
        })
    }
}

/// A lane as its cards gather, before it is built.
struct Gathering {
    head: LaneHead,
    columns: [Vec<Card>; Column::ALL.len()],
}

impl Gathering {
    fn of(head: LaneHead) -> Gathering {
        Gathering {
            head,
            columns: Default::default(),
        }
    }

    fn cards(&self, column: Column) -> &[Card] {
        &self.columns[column as usize]
    }

    /// Whether it has no cards.
    fn is_empty(&self) -> bool {
        self.columns.iter().all(Vec::is_empty)
    }

    /// [`Lane::finished`].
    fn finished(&self) -> Option<Timestamp> {
        let open = Column::ALL
            .into_iter()
            .filter(|&column| column != Column::Done)
            .any(|column| !self.cards(column).is_empty());
        if open {
            return None;
        }
        let done = match self.head {
            LaneHead::Tracking { done: None, .. } => return None,
            LaneHead::Tracking { done, .. } => done,
            LaneHead::Item { .. } => None,
        };
        self.cards(Column::Done)
            .iter()
            .filter_map(|card| match card.line {
                Some(CardLine::Ended { at, .. }) => Some(at),
                _ => None,
            })
            .chain(done)
            .max()
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

    fn built(self) -> Lane {
        let finished = self.finished();
        let latest = finished.unwrap_or_else(|| self.last_change());
        Lane {
            place: (finished.is_some(), Reverse(latest), self.head.number()),
            finished,
            head: self.head,
            columns: self.columns,
        }
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
        /// When the tracking issue became done, if it is done.
        done: Option<Timestamp>,
    },
    /// An item worked as one, the lane's one card: an issue, or a pull
    /// request that closes no issue with a card.
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
    /// When it last changed.
    changed: Timestamp,
    /// The people it matches (`docs/specs/design_system.md`, Filtering by
    /// person): those party to its item
    /// ([`party::parties`]) and, for a ticket on the board, to its tracking
    /// issue ([`party::party_to`]).
    pub people: BTreeSet<Login>,
}

impl Card {
    /// Its card status, that of its line.
    pub fn status(&self) -> Option<CardStatus> {
        self.line.as_ref().and_then(CardLine::status)
    }
}

/// The line under a card's title (`docs/specs/design_system.md`, Cards):
/// what sets the item apart from the rest of its column, or what a person
/// acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardLine {
    /// Handed back in a stall (`GLOSSARY.md`, **Stall**).
    Stalled,
    /// The people asked (`docs/specs/design_system.md`, Cards).
    Asked(Vec<Login>),
    HandedBack,
    DecisionNeeded,
    BlockedBy(Number),
    AssignedTo(Login),
    /// Linewright was unassigned from it.
    Unassigned,
    /// It is not marked for Linewright (`GLOSSARY.md`, **Marked for
    /// Linewright**).
    NotMarked,
    Held(Held),
    Running(Running),
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
            CardLine::Held(held) => Some(held.status()),
            CardLine::Running(_) => Some(CardStatus::InFlight),
            CardLine::Review { stops, .. } => stops.as_ref().and_then(Stop::status),
            CardLine::MergeStop(stop) => stop.status(),
            CardLine::BlockedBy(_)
            | CardLine::AssignedTo(_)
            | CardLine::Unassigned
            | CardLine::NotMarked
            | CardLine::Ended { .. } => None,
        }
    }
}

/// What holds back the run Linewright would start on an item: a hold, or
/// the runs in flight at `max-runs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Held {
    Paused(Hold),
    Queued,
}

impl Held {
    /// Its card status (`docs/specs/design_system.md`, Cards, Card status).
    pub fn status(&self) -> CardStatus {
        CardStatus::PausedOrQueued
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
    /// Linewright opened it and is not assigned to it, and its last act on it
    /// was letting go of it.
    HandedBack,
    /// Linewright opened it and is not assigned to it otherwise.
    Unassigned,
    Running(Running),
    Held(Held),
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
            Stop::Held(held) => Some(held.status()),
            Stop::HandedBack | Stop::ChangesRequested | Stop::ReviewRequired => {
                Some(CardStatus::WaitingForAPerson)
            }
            Stop::Unassigned | Stop::Behind { .. } | Stop::Draft | Stop::Approved => None,
        }
    }
}

/// The column and card of `managed`, the card it would have on its own,
/// on the board or off it: `None` for a tracking issue, which heads its
/// lane. `recorded` is every run's record, which tells whether a run handed
/// an issue back.
pub fn card(
    managed: &Managed,
    view: &View,
    recorded: &[Recorded],
    now: Timestamp,
) -> Option<(Column, Card)> {
    let seen = Seen {
        view,
        recorded,
        now,
    };
    card_among(managed, &managed::managed(view, now), &seen)
}

/// What the board is built from: the view, every run's record, which tells
/// whether a run handed an item back, and the time.
struct Seen<'a> {
    view: &'a View,
    recorded: &'a [Recorded],
    now: Timestamp,
}

/// [`card`], with `all` the managed items: the card shows those of them that
/// are pull requests closing `managed`, unless `managed` is itself a pull
/// request or in Done.
fn card_among(managed: &Managed, all: &[Managed], seen: &Seen) -> Option<(Column, Card)> {
    let view = seen.view;
    let snapshot = &view.snapshot;
    let column = managed.column()?;
    let folded: Vec<&Managed> = if managed.is_pull_request() || column == Column::Done {
        vec![]
    } else {
        all.iter()
            .filter(|pull| {
                matches!(pull.detail, Some(Detail::PullRequest { .. }))
                    && pull.closes.contains(&managed.number)
            })
            .collect()
    };
    let mut on: Vec<Number> = vec![managed.number];
    on.extend(folded.iter().map(|pull| pull.number));
    let run = run_on(view, &on, seen.now);
    let waits: Vec<Wait> = managed
        .waiting_for
        .iter()
        .chain(folded.iter().flat_map(|pull| &pull.waiting_for))
        .cloned()
        .collect();
    let issue = snapshot.issue(managed.number);
    let line = match column {
        Column::Triage | Column::Ready => ready_line(issue, &view.policy, &waits),
        Column::NeedsAPerson => needs_a_person_line(
            managed.state,
            snapshot.is_stalled(managed.number),
            issue,
            seen,
        ),
        Column::Working => working_line(issue, run, &waits),
        Column::InReview => review_line(managed, &folded, issue, run, seen),
        Column::Done => done_line(managed.detail.as_ref()),
    };
    let card = Card {
        number: managed.number,
        title: managed.title.clone(),
        pull_request: managed.is_pull_request(),
        line,
        entered: entered(column, managed.number, view),
        changed: last_change(seen, &on),
        people: party::parties(snapshot, &view.policy.linewright, managed.number),
    };
    Some((column, card))
}

/// The board at `now` (`docs/specs/design_system.md`, The board). A pull
/// request shows on the card of each issue it closes that has a card, and
/// has a card of its own otherwise. Each open tracking issue, and each
/// managed one, heads the lane of its tickets. A lane with no cards, or that finished
/// before the Done window opened, is left off. `recorded` is every run's
/// record, which tells whether a run handed an issue back.
pub fn board(view: &View, recorded: &[Recorded], now: Timestamp) -> Board {
    let all = managed::managed(view, now);
    let seen = Seen {
        view,
        recorded,
        now,
    };
    let mut cards: Vec<(Column, Card)> = vec![];
    let mut pulls: Vec<(&[Number], (Column, Card))> = vec![];
    for managed in &all {
        let Some(card) = card_among(managed, &all, &seen) else {
            continue;
        };
        match folds_onto(managed) {
            Some(closes) => pulls.push((closes, card)),
            None => cards.push(card),
        }
    }
    let on_the_board: Vec<Number> = cards.iter().map(|(_, card)| card.number).collect();
    cards.extend(
        pulls
            .into_iter()
            .filter(|(closes, _)| !closes.iter().any(|n| on_the_board.contains(n)))
            .map(|(_, card)| card),
    );

    let open_trackers = view
        .snapshot
        .issues
        .iter()
        .filter(|issue| issue.open)
        .map(|issue| issue.number);
    let mut trackers: Vec<Tracker> = all
        .iter()
        .filter(|managed| managed.tickets.is_some())
        .map(|managed| managed.number)
        .chain(open_trackers)
        .filter_map(|number| Tracker::of(number, view))
        .collect();
    trackers.sort_by_key(|tracker| tracker.number);
    trackers.dedup_by_key(|tracker| tracker.number);

    cards.sort_by_key(|(_, card)| card.number);
    let mut lanes: Vec<Gathering> = trackers
        .iter()
        .map(|tracker| Gathering::of(tracker.head()))
        .collect();
    for (column, mut card) in cards {
        let tracker = trackers
            .iter()
            .position(|tracker| tracker.tickets.contains(&card.number));
        match tracker {
            Some(i) => {
                card.people.extend(trackers[i].parties.iter().cloned());
                lanes[i].columns[column as usize].push(card);
            }
            None => {
                let mut lane = Gathering::of(LaneHead::Item {
                    number: card.number,
                    title: card.title.clone(),
                });
                lane.columns[column as usize].push(card);
                lanes.push(lane);
            }
        }
    }
    let windows = view.policy.windows(now);
    let mut lanes: Vec<Lane> = lanes
        .into_iter()
        .filter(|lane| !lane.is_empty())
        .map(Gathering::built)
        .filter(|lane| lane.finished.is_none_or(|at| windows.in_done_window(at)))
        .collect();
    lanes.sort_by_key(|lane| lane.place);
    let needing_a_person = lanes
        .iter()
        .map(|lane| lane.cards(Column::NeedsAPerson).len())
        .sum();
    Board {
        lanes,
        needing_a_person,
        managed: all,
    }
}

/// The issues `managed` closes, if it is a pull request, which shows on
/// their cards.
fn folds_onto(managed: &Managed) -> Option<&[Number]> {
    managed.is_pull_request().then_some(&managed.closes[..])
}

/// The line in Done: how the item ended.
fn done_line(detail: Option<&Detail>) -> Option<CardLine> {
    match *detail? {
        Detail::Ended { ended, at } => Some(CardLine::Ended { ended, at }),
        Detail::InReview(_) | Detail::InReviewElsewhere { .. } | Detail::PullRequest { .. } => None,
    }
}

/// The first of `rules` that holds.
fn first<T>(rules: impl IntoIterator<Item = Option<T>>) -> Option<T> {
    rules.into_iter().flatten().next()
}

/// The line in Needs a person, of an item `stalled` or not.
fn needs_a_person_line(
    state: lifecycle::State,
    stalled: bool,
    issue: Option<&Issue>,
    seen: &Seen,
) -> Option<CardLine> {
    let linewright = &seen.view.policy.linewright;
    let let_go = issue.is_some_and(|issue| let_go_of(issue, seen.recorded, linewright));
    let asked = || CardLine::Asked(issue.map_or(vec![], |issue| issue.assignees_but(linewright)));
    first([
        stalled.then_some(CardLine::Stalled),
        (state == lifecycle::State::NeedsInfo).then(asked),
        let_go.then_some(CardLine::HandedBack),
        Some(CardLine::DecisionNeeded),
    ])
}

/// Whether Linewright's last act on `issue` was letting go of it: of its
/// runs on the issue, by when each ended, and its pull requests closing the
/// issue that were closed unmerged, the latest is a run that handed it back
/// or such a closing.
fn let_go_of(issue: &Issue, recorded: &[Recorded], linewright: &Login) -> bool {
    let closings = issue
        .closed_by
        .iter()
        .filter(|pull| &pull.author == linewright)
        .filter_map(|pull| match pull.state {
            PullState::Closed { at } => Some((at, true)),
            PullState::Open { .. } | PullState::Merged { .. } => None,
        });
    last_lets_go(runs_ended(issue.number, recorded).chain(closings))
}

/// Linewright's runs on item #`number` that ended, each by when it ended
/// and whether it handed the item back.
fn runs_ended(
    number: Number,
    recorded: &[Recorded],
) -> impl Iterator<Item = (Timestamp, bool)> + '_ {
    recorded
        .iter()
        .filter(move |run| run.id.of.item() == Some(number))
        .filter_map(|run| match run.state {
            State::Ended { at, ending } => Some((at, ending == Ending::HandedBack)),
            State::InFlight | State::Interrupted => None,
        })
}

/// Whether the latest of Linewright's `acts` on an item, each by when it
/// happened and whether it let go of the item, let go of it.
fn last_lets_go(acts: impl Iterator<Item = (Timestamp, bool)>) -> bool {
    acts.max_by_key(|(at, _)| *at)
        .is_some_and(|(_, let_go)| let_go)
}

/// The line in Ready, and as in Ready in Triage, from its issue's open
/// blockers and whether `policy` marks it for Linewright, if it is an issue,
/// and what it waits for. An issue waiting for someone else or for its
/// label again waits for nothing past that, so its blockers come from the
/// issue itself.
fn ready_line(issue: Option<&Issue>, policy: &Policy, waits: &[Wait]) -> Option<CardLine> {
    let marking = issue.map(|issue| policy.marking(&issue.labels));
    let assignee = waits.iter().find_map(|wait| {
        let Wait::Assignee { assignee } = wait else {
            return None;
        };
        Some(assignee.clone())
    });
    let unassigned = waits.iter().any(Wait::is_unassigned);
    first([
        lowest_blocker(issue),
        assignee.map(CardLine::AssignedTo),
        unassigned.then_some(CardLine::Unassigned),
        (marking == Some(Marking::Unmarked)).then_some(CardLine::NotMarked),
        held(waits).map(CardLine::Held),
    ])
}

/// The line in Working: its run in flight, or else its issue's lowest open
/// blocker, if it is an issue, or else as in Ready, what holds back the run
/// that would start.
fn working_line(issue: Option<&Issue>, run: Option<Running>, waits: &[Wait]) -> Option<CardLine> {
    first([
        run.map(CardLine::Running),
        lowest_blocker(issue),
        held(waits).map(CardLine::Held),
    ])
}

/// The lowest of `issue`'s open blockers, if it is an issue.
fn lowest_blocker(issue: Option<&Issue>) -> Option<CardLine> {
    let blocker = issue?.open_blockers().into_iter().min()?;
    Some(CardLine::BlockedBy(blocker))
}

/// The first hold among `waits` by rank (`docs/specs/runs.md`, Holds,
/// Rank), the first noted among files, or, where none is a hold, the runs in
/// flight at `max-runs`, holding back the run that would start.
fn held(waits: &[Wait]) -> Option<Held> {
    let first_hold = waits.iter().filter_map(Hold::of).min_by(Hold::cmp_rank);
    first([
        first_hold.map(Held::Paused),
        waits
            .iter()
            .any(|wait| matches!(wait, Wait::FreeRun { .. }))
            .then_some(Held::Queued),
    ])
}

/// The line in In review: of an item other than its pull request, its pull
/// request and what stops it merging; of a pull request, what stops it
/// merging. `folded` is the pull requests the card shows, among which the
/// shown pull request's own waits are found.
fn review_line(
    managed: &Managed,
    folded: &[&Managed],
    issue: Option<&Issue>,
    run: Option<Running>,
    seen: &Seen,
) -> Option<CardLine> {
    let (pull_request, author) = match managed.detail.as_ref()? {
        Detail::PullRequest { .. } => {
            return merge_stop(managed.number, None, run, &managed.waiting_for, seen)
                .map(CardLine::MergeStop);
        }
        Detail::InReview(review) => (review.pull_request, None),
        Detail::InReviewElsewhere {
            pull_request,
            author,
        } => (
            *pull_request,
            Some(author.clone()).filter(|author| author != &seen.view.policy.linewright),
        ),
        Detail::Ended { .. } => return None,
    };
    let waits = folded
        .iter()
        .find(|pull| pull.number == pull_request)
        .map_or(&[][..], |pull| &pull.waiting_for);
    Some(CardLine::Review {
        pull_request,
        author,
        stops: merge_stop(pull_request, issue, run, waits, seen),
    })
}

/// What stops `pull_request` merging, the first that holds, with `waits` its
/// own waits. Someone else's pull request the snapshot does not hold is known
/// only by `issue`'s reference to it.
fn merge_stop(
    pull_request: Number,
    issue: Option<&Issue>,
    run: Option<Running>,
    waits: &[Wait],
    seen: &Seen,
) -> Option<Stop> {
    let view = seen.view;
    let snapshot = &view.snapshot;
    let linewright = &view.policy.linewright;
    let pull = snapshot.any_pull(pull_request);
    let unassigned =
        pull.filter(|pull| &pull.author == linewright && !pull.is_assigned(linewright));
    let let_go = || last_lets_go(runs_ended(pull_request, seen.recorded));
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
        unassigned.map(|_| {
            if let_go() {
                Stop::HandedBack
            } else {
                Stop::Unassigned
            }
        }),
        run.map(Stop::Running),
        held(waits).map(Stop::Held),
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
    /// Its tickets, the cards of its lane.
    tickets: Vec<Number>,
    counted: Tickets,
    /// When it became done, if it is done.
    done: Option<Timestamp>,
    /// The people party to it ([`party::party_to`]), whom its tickets'
    /// cards match.
    parties: BTreeSet<Login>,
}

impl Tracker {
    /// The tracking issue #`number`, open or closed, as the snapshot holds
    /// it. It became done when it closed, or when a pull request closing it
    /// merged, if that was earlier (`docs/specs/repository.md`, State).
    fn of(number: Number, view: &View) -> Option<Tracker> {
        let snapshot = &view.snapshot;
        let linewright = &view.policy.linewright;
        let default_branch = &snapshot.default_branch;
        if let Some(issue) = snapshot.issue(number).filter(|issue| issue.open) {
            if !issue.is_tracking(linewright) {
                return None;
            }
            let merge = done_by_merge(
                &issue.events,
                &snapshot.pulls_closing(issue),
                default_branch,
            );
            return Some(Tracker::new(
                number,
                &issue.title,
                &issue.sub_issues,
                done_at(merge, None),
                party::party_to(snapshot, linewright, number),
                linewright,
            ));
        }
        let item = snapshot.closed.iter().find(|item| item.number == number)?;
        if !item.is_tracking(linewright) {
            return None;
        }
        let ClosedKind::Issue { facts, .. } = &item.kind else {
            return None;
        };
        let merge = done_by_merge(&facts.work.events, &facts.work.closed_by, default_branch);
        Some(Tracker::new(
            number,
            &item.title,
            &facts.sub_issues,
            done_at(merge, Some(item.closed_at)),
            party::party_to(snapshot, linewright, number),
            linewright,
        ))
    }

    fn new(
        number: Number,
        title: &str,
        sub_issues: &[SubIssue],
        done: Option<Timestamp>,
        parties: BTreeSet<Login>,
        linewright: &Login,
    ) -> Tracker {
        Tracker {
            number,
            title: title.to_string(),
            tickets: tickets_cut(sub_issues, linewright)
                .map(|ticket| ticket.number)
                .collect(),
            counted: Tickets::of(sub_issues, linewright),
            done,
            parties,
        }
    }

    fn head(&self) -> LaneHead {
        LaneHead::Tracking {
            number: self.number,
            title: self.title.clone(),
            tickets_closed: self.counted.tickets_closed,
            tickets: self.counted.tickets,
            done: self.done,
        }
    }
}

/// When anything last happened to `items`, a card's item and its pull
/// requests (`docs/specs/design_system.md`, The board, A lane's last
/// change): on GitHub, as it last updated them and every pull request
/// closing them, and as those merged or closed, and in Linewright, as its
/// runs on them started and ended. A run in flight is happening `now`.
fn last_change(seen: &Seen, items: &[Number]) -> Timestamp {
    let Seen {
        view,
        recorded,
        now,
    } = *seen;
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
