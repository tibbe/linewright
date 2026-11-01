//! An item's state (`docs/specs/repository.md`, State): the first row of
//! its table that holds, derived from what GitHub shows and the run in
//! flight on the item. Every view of an item's state asks here, and the
//! reconcile works an item by the row of the Working an item table that
//! holds for it ([`row`]) and cancels its runs as Steering says
//! ([`cancels`]).

use serde::{Deserialize, Serialize};

use crate::board::Column;
use crate::domain::{
    Asked, Asks, Assignment, ClosingPull, Issue, IssueEvent, Login, Merge, Number, PullState,
    StatusLabel, Timestamp, label, last_assignment, last_labeling, last_unassignment,
};
use crate::reconcile::{Discard, NotSpecChange, Policy, WorkKind};

/// An item's state: ten of an issue, three of a pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Done,
    InReview,
    Tracking,
    BeingTriaged,
    BeingDesigned,
    BeingImplemented,
    NeedsTriage,
    NeedsInfo,
    ReadyForHuman,
    ReadyForAgent,
    Open,
    Merged,
    Closed,
}

impl State {
    /// The column of a card in this state (`docs/specs/design_system.md`,
    /// The board): none for tracking, since a tracking issue has no card.
    pub fn column(self) -> Option<Column> {
        Some(match self {
            State::NeedsTriage => Column::Triage,
            State::NeedsInfo | State::ReadyForHuman => Column::NeedsAPerson,
            State::ReadyForAgent => Column::Ready,
            State::BeingTriaged | State::BeingDesigned | State::BeingImplemented => Column::Working,
            State::InReview | State::Open => Column::InReview,
            State::Done | State::Merged | State::Closed => Column::Done,
            State::Tracking => return None,
        })
    }

    /// Whether it is a pull request's state.
    pub fn of_pull_request(self) -> bool {
        match self {
            State::Open | State::Merged | State::Closed => true,
            State::Done
            | State::InReview
            | State::Tracking
            | State::BeingTriaged
            | State::BeingDesigned
            | State::BeingImplemented
            | State::NeedsTriage
            | State::NeedsInfo
            | State::ReadyForHuman
            | State::ReadyForAgent => false,
        }
    }

    /// How it is shown (`docs/specs/design_system.md`, States).
    pub fn shown(self) -> &'static str {
        match self {
            State::NeedsTriage => "Waiting for triage",
            State::BeingTriaged => "Being triaged",
            State::NeedsInfo => "Waiting for an answer",
            State::ReadyForHuman => "Waiting for a decision",
            State::ReadyForAgent => "Waiting for Linewright",
            State::BeingDesigned => "Being designed",
            State::BeingImplemented => "Being implemented",
            State::InReview => "In review",
            State::Tracking => "Tracking",
            State::Done => "Done",
            State::Open => "Open",
            State::Merged => "Merged",
            State::Closed => "Closed",
        }
    }

    /// The state of an issue holding `status`.
    fn of_label(status: StatusLabel) -> State {
        match status {
            StatusLabel::NeedsTriage => State::NeedsTriage,
            StatusLabel::NeedsInfo => State::NeedsInfo,
            StatusLabel::ReadyForHuman => State::ReadyForHuman,
            StatusLabel::ReadyForAgent => State::ReadyForAgent,
        }
    }

    /// The stage of the run an issue in this state is worked by, if it is
    /// one of the working states.
    pub fn stage(self) -> Option<IssueStage> {
        match self {
            State::BeingTriaged => Some(IssueStage::Triage),
            State::BeingDesigned => Some(IssueStage::Design),
            State::BeingImplemented => Some(IssueStage::Implement),
            State::Done
            | State::InReview
            | State::Tracking
            | State::NeedsTriage
            | State::NeedsInfo
            | State::ReadyForHuman
            | State::ReadyForAgent
            | State::Open
            | State::Merged
            | State::Closed => None,
        }
    }

    /// The state an issue was in when a run of `stage` started on it: none
    /// for a stage that runs on a pull request.
    fn of_stage(stage: WorkKind) -> Option<State> {
        match stage {
            WorkKind::Triage => Some(State::BeingTriaged),
            WorkKind::Design => Some(State::BeingDesigned),
            WorkKind::Implement => Some(State::BeingImplemented),
            WorkKind::Rework | WorkKind::FixCi | WorkKind::Update => None,
        }
    }
}

/// The state of `issue`, closed by `closing`, with `merge` the merge that
/// made it done, as [`done_by_merge`] gives it, and a run of `run` in flight
/// on it or none, where `linewright` is Linewright's login.
pub fn issue_state(
    issue: &Issue,
    closing: &[ClosingPull],
    merge: Option<Merge>,
    run: Option<WorkKind>,
    linewright: &Login,
) -> Option<State> {
    let assigned = issue.is_assigned(linewright);
    let status = issue.status_label();
    if !issue.open || merge.is_some() {
        Some(State::Done)
    } else if closing.iter().any(ClosingPull::is_open) {
        Some(State::InReview)
    } else if issue.is_tracking(linewright) {
        Some(State::Tracking)
    } else if status == Some(StatusLabel::NeedsTriage) && assigned {
        Some(State::BeingTriaged)
    } else if status == Some(StatusLabel::ReadyForAgent) && assigned {
        Some(if issue.has_label(label::DESIGNED) {
            State::BeingImplemented
        } else {
            State::BeingDesigned
        })
    } else if let Some(status) = status {
        Some(State::of_label(status))
    } else {
        run.and_then(State::of_stage)
    }
}

/// The state of a pull request in `state`.
pub fn pull_state(state: &PullState) -> State {
    match state {
        PullState::Open { .. } => State::Open,
        PullState::Merged { .. } => State::Merged,
        PullState::Closed { .. } => State::Closed,
    }
}

/// Of `closing`, the pull requests that close an issue with `events`, the
/// first to merge into `default_branch` since the issue was last opened or
/// given a status label, by number where two merged at once: the merge that
/// made it done. A pull request whose base is not known merged into no
/// branch it can tell.
pub fn done_by_merge(
    events: &[IssueEvent],
    closing: &[ClosingPull],
    default_branch: &str,
) -> Option<Merge> {
    let since = opened_or_labeled(events);
    closing
        .iter()
        .filter(|pull| !pull.base_ref.is_empty() && pull.base_ref == default_branch)
        .filter_map(|pull| match pull.state {
            PullState::Merged { at } => Some(Merge {
                pull_request: pull.number,
                at,
            }),
            PullState::Open { .. } | PullState::Closed { .. } => None,
        })
        .filter(|merge| since.is_none_or(|since| merge.at > since))
        .min_by_key(|merge| (merge.at, merge.pull_request))
}

/// When an issue became done: when `merge`, the merge that made it done,
/// merged, or when it closed at `closed_at`, whichever is earlier.
pub fn done_at(merge: Option<Merge>, closed_at: Option<Timestamp>) -> Option<Timestamp> {
    [merge.map(|merge| merge.at), closed_at]
        .into_iter()
        .flatten()
        .min()
}

/// A stage of a run on an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueStage {
    Triage,
    Design,
    Implement,
}

/// What the Working an item table reads of an issue, besides its state.
#[derive(Clone, Copy, Debug)]
pub struct IssueFacts<'a> {
    pub issue: &'a Issue,
    /// The pull requests that close it.
    pub closing: &'a [ClosingPull],
    pub run_in_flight: bool,
    /// The discard of its cut, while a ticket cut from it is among its
    /// sub-issues.
    pub discard: Option<&'a Discard>,
}

/// A row of the Working an item table (`docs/specs/repository.md`) for an
/// issue, with what it writes. The rows for how a run ends are the run's to
/// apply, and a pull request's are [`PullRow`]s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// Discards the cut (`docs/specs/design.md`, "Discarding a cut").
    DiscardCut(Discard),
    /// Hands the issue back: a person closed `pull`, Linewright's, unmerged.
    HandBack {
        pull: Number,
    },
    /// Unassigns Linewright, posting `comment` first where there is one.
    LetGo {
        comment: Option<String>,
    },
    /// Unassigns the people Linewright asked.
    UnassignAsked {
        people: Vec<Login>,
    },
    /// Removes `labels` from a tracking issue, then unassigns Linewright
    /// where `let_go`.
    CleanUpTracking {
        labels: Vec<StatusLabel>,
        let_go: bool,
    },
    CloseTracking,
    /// Takes a reply: labels the issue `needs-triage`.
    TakeReply,
    Claim,
    Start(IssueStage),
    RemoveStaleStatusLabels(Vec<StatusLabel>),
    ClearStall,
}

/// The first row of the Working an item table that holds for the issue of
/// `facts` in `state`, other than those for how a run ends: none while a run
/// is in flight on it.
pub fn row(facts: &IssueFacts, state: Option<State>, policy: &Policy) -> Option<Row> {
    let IssueFacts {
        issue,
        closing,
        run_in_flight,
        discard,
    } = *facts;
    let linewright = &policy.linewright;
    if run_in_flight {
        return None;
    }
    let in_state = |states: &[State]| state.is_some_and(|state| states.contains(&state));
    let assigned = issue.is_assigned(linewright);
    if let Some(discard) = discard
        && !issue.has_label(label::DESIGNED)
        && issue.tickets_cut(linewright).next().is_some()
    {
        return Some(Row::DiscardCut(discard.clone()));
    }
    // Linewright leaves a closed item's labels and assignees as they are.
    if !issue.open {
        return None;
    }
    if in_state(&[State::BeingDesigned, State::BeingImplemented])
        && let Some(pull) = refused(issue, closing, linewright)
    {
        return Some(Row::HandBack { pull });
    }
    if matches!(state, None | Some(State::NeedsInfo | State::ReadyForHuman)) && assigned {
        let comment = answers_hand_assignment(issue, linewright)
            .then(|| let_go_comment(issue.status_label(), linewright));
        return Some(Row::LetGo { comment });
    }
    if in_state(&[State::NeedsTriage, State::ReadyForAgent]) {
        let people = issue.assigned_by(linewright);
        if !people.is_empty() {
            return Some(Row::UnassignAsked { people });
        }
    }
    if state == Some(State::Tracking) {
        let labels: Vec<StatusLabel> = StatusLabel::ALL
            .into_iter()
            .filter(|status| issue.has_label(status.label()))
            .collect();
        if !labels.is_empty() || assigned {
            return Some(Row::CleanUpTracking {
                labels,
                let_go: assigned,
            });
        }
        if !issue.sub_issues.is_empty() && issue.sub_issues.iter().all(|s| !s.is_open()) {
            return Some(Row::CloseTracking);
        }
    }
    if state == Some(State::NeedsInfo) && has_reply(issue, linewright) {
        return Some(Row::TakeReply);
    }
    if let Some(status) = issue.status_label()
        && in_state(&[State::NeedsTriage, State::ReadyForAgent])
        && may_claim(issue, status, policy).is_ok()
    {
        return Some(Row::Claim);
    }
    if let Some(stage) = state.and_then(State::stage)
        && issue.open_blockers().is_empty()
    {
        return Some(Row::Start(stage));
    }
    let stale = issue.stale_status_labels();
    if !stale.is_empty() {
        return Some(Row::RemoveStaleStatusLabels(stale));
    }
    if issue.is_stalled() && assigned {
        return Some(Row::ClearStall);
    }
    None
}

/// Why Linewright may not claim an issue on its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotClaimed {
    /// These open issues block it.
    Blocked(Vec<Number>),
    /// These people are assigned to it.
    Assigned(Vec<Login>),
    /// Linewright was unassigned from it since its status label or the
    /// required label was last applied.
    Unassigned,
    /// It is not marked for Linewright.
    Unmarked,
}

/// Whether Linewright may claim `issue`, holding `status`, on its own
/// (`docs/specs/repository.md`, Working an item, "Claiming"), or why not.
pub fn may_claim(issue: &Issue, status: StatusLabel, policy: &Policy) -> Result<(), NotClaimed> {
    let blockers = issue.open_blockers();
    if !blockers.is_empty() {
        Err(NotClaimed::Blocked(blockers))
    } else if !issue.assignees.is_empty() {
        Err(NotClaimed::Assigned(issue.assignees.clone()))
    } else if unassigned_since(issue, policy, status) {
        Err(NotClaimed::Unassigned)
    } else if !policy.marks(&issue.labels) {
        Err(NotClaimed::Unmarked)
    } else {
        Ok(())
    }
}

/// Whether Linewright was unassigned from `issue`, by anyone, after it was
/// last labeled with `status` or with the required label.
fn unassigned_since(issue: &Issue, policy: &Policy, status: StatusLabel) -> bool {
    let Some(unassigned) = last_unassignment(&issue.events, &policy.linewright) else {
        return false;
    };
    [Some(status.label()), policy.require_label.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(|l| last_labeling(&issue.events, l))
        .max()
        .is_none_or(|labeled| unassigned > labeled)
}

/// Of `closing`, the pull request Linewright opened that was closed without
/// merging since `issue` was last opened or given a status label, the first
/// by number. GitHub does not say who closed it; Linewright closes its own
/// only when it discards a cut, which leaves their tickets alone until it
/// closes them too, so it is taken as a person's no.
fn refused(issue: &Issue, closing: &[ClosingPull], linewright: &Login) -> Option<Number> {
    let since = opened_or_labeled(&issue.events);
    closing
        .iter()
        .filter(|pull| &pull.author == linewright)
        .filter(|pull| match pull.state {
            PullState::Closed { at } => since.is_none_or(|since| at > since),
            PullState::Open { .. } | PullState::Merged { .. } => false,
        })
        .map(|pull| pull.number)
        .min()
}

/// Whether a reply to Linewright's question has come on `issue`
/// (`docs/specs/triage.md`, Answers): a comment from anyone other than
/// Linewright posted after its question, the later of its last comment and
/// the last `needs-info` labeling.
fn has_reply(issue: &Issue, linewright: &Login) -> bool {
    let Some(last) = issue
        .comments
        .iter()
        .filter(|c| &c.author == linewright)
        .map(|c| c.created_at)
        .max()
    else {
        return false;
    };
    let asked = issue
        .last_labeled(StatusLabel::NeedsInfo)
        .map_or(last, |labeled| labeled.max(last));
    issue
        .comments
        .iter()
        .any(|c| &c.author != linewright && c.created_at > asked)
}

/// Whether letting go of `issue`, whose label asks nothing of Linewright,
/// answers a person's assignment of Linewright: a person made the last one,
/// its label asked nothing of Linewright then either, and Linewright has not
/// commented since.
fn answers_hand_assignment(issue: &Issue, linewright: &Login) -> bool {
    let Some((assigned, actor)) = last_assignment(&issue.events, linewright) else {
        return false;
    };
    let unasked_then = !matches!(
        issue.status_label_as_of(assigned).map(StatusLabel::asks),
        Some(Asks::Linewright(_))
    );
    // A comment since means this one was posted, and a stop came before
    // the unassignment.
    let commented = issue
        .comments
        .iter()
        .any(|c| &c.author == linewright && c.created_at >= assigned.at);
    actor != linewright && unasked_then && !commented
}

/// The comment letting go of an issue assigned to Linewright by hand whose
/// label, `status`, asks nothing of it.
fn let_go_comment(status: Option<StatusLabel>, linewright: &Login) -> String {
    let waits = match status {
        Some(StatusLabel::NeedsInfo) => format!(
            "This issue waits for an answer to its question (`{}`)",
            label::NEEDS_INFO
        ),
        Some(held) => format!("This issue waits for a person's decision (`{held}`)"),
        None => "This issue has no status label".into(),
    };
    format!(
        "{waits}, so @{linewright} has let go of it. Label it `{}` to have it implemented or \
         `{}` to have it assessed, and assign @{linewright} again.",
        label::READY_FOR_AGENT,
        label::NEEDS_TRIAGE,
    )
}

/// Linewright's assignment to a pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullClaim {
    Claimed,
    /// Neither assigned nor unassigned ever, as a pull request Linewright
    /// opened before it assigned itself to those it opens.
    NeverAssigned,
    Unassigned,
}

impl PullClaim {
    /// Linewright's assignment to a pull request with `assignees`, after
    /// `assignments`, where `linewright` is Linewright's login.
    pub fn of(assignees: &[Login], assignments: &[Assignment], linewright: &Login) -> PullClaim {
        if assignees.contains(linewright) {
            PullClaim::Claimed
        } else if assignments.iter().any(|a| a.assignee() == linewright) {
            PullClaim::Unassigned
        } else {
            PullClaim::NeverAssigned
        }
    }
}

/// Whose a pull request is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opener {
    Linewright,
    Person,
}

/// Where the work on a pull request Linewright opened is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullWorking {
    RunInFlight,
    /// Pull requests (`docs/specs/pull_requests.md`) asks for a run, a merge
    /// or another write on it.
    Asked,
    /// Pull requests asks for nothing yet, or it is not Linewright's.
    Waiting,
}

/// What the Working an item table reads of a pull request, besides its
/// state.
#[derive(Clone, Copy, Debug)]
pub struct PullFacts {
    pub opener: Opener,
    pub claim: PullClaim,
    pub working: PullWorking,
    pub stalled: bool,
}

/// A row of the Working an item table, or of Steering, for a pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PullRow {
    /// Assigns Linewright to a pull request it opened.
    Claim,
    /// Does what Pull requests asks.
    Work,
    /// Unassigns Linewright from a pull request it did not open, posting
    /// `comment` first.
    LetGo {
        comment: String,
    },
    ClearStall,
}

/// The row of the Working an item table, or of Steering, that takes up a
/// merged spec change: Linewright opens its issue, then unassigns itself
/// from the spec change where `let_go`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TakeUp {
    pub let_go: bool,
}

/// The first row that holds for the open pull request of `facts` in
/// `state`, other than those for how a run ends: none while a run is in
/// flight on it. A claim leaves the pull request to its work in
/// the same poll.
pub fn pull_row(facts: &PullFacts, state: State, linewright: &Login) -> Option<PullRow> {
    let PullFacts {
        opener,
        claim,
        working,
        stalled,
    } = *facts;
    let linewrights_open = opener == Opener::Linewright && state == State::Open;
    let claimed = claim == PullClaim::Claimed;
    if working == PullWorking::RunInFlight {
        None
    } else if linewrights_open && claim == PullClaim::NeverAssigned {
        Some(PullRow::Claim)
    } else if linewrights_open && claimed && working == PullWorking::Asked {
        Some(PullRow::Work)
    } else if opener == Opener::Person && claimed {
        Some(PullRow::LetGo {
            comment: not_its_own(linewright),
        })
    } else if linewrights_open && claimed && stalled {
        Some(PullRow::ClearStall)
    } else {
        None
    }
}

/// The comment letting go of an open pull request Linewright did not open.
fn not_its_own(linewright: &Login) -> String {
    format!(
        "@{linewright} works only on the pull requests it opens and on spec changes, so it has \
         let go of this one."
    )
}

/// The first row of the Working an item table, or of Steering, that holds
/// for a merged pull request a person opened, `as_spec_change` the spec
/// change Linewright may take up or why not, with Linewright `assigned` to
/// it or not. Assigned to a spec change not yet taken up, Linewright takes
/// it up whether it may on its own or not. Only taking up holds for a
/// merged pull request: any other is left as it is, whoever is assigned.
pub fn merged_row(as_spec_change: &Result<(), NotSpecChange>, assigned: bool) -> Option<TakeUp> {
    match as_spec_change {
        Ok(()) => Some(TakeUp { let_go: assigned }),
        Err(NotSpecChange::BeforeLookback { .. } | NotSpecChange::RequiredLabel { .. }) => {
            assigned.then_some(TakeUp { let_go: true })
        }
        Err(
            NotSpecChange::Linewrights
            | NotSpecChange::TakenUp { .. }
            | NotSpecChange::Designed { .. }
            | NotSpecChange::OtherBranch { .. }
            | NotSpecChange::NoSpecFile { .. },
        ) => None,
    }
}

/// Why a run on an issue is cancelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cancel {
    Closed,
    /// The status label that asks for the run was changed or removed.
    Relabeled(StatusLabel),
    Unassigned,
}

/// Whether a run of `stage` on `issue` is cancelled, and why
/// (`docs/specs/repository.md`, Steering): the issue was closed, its status
/// label no longer asks for the run, or Linewright was unassigned from it.
/// `ready-for-agent` asks for both the design and the implement run, so
/// `linewright:designed` changing cancels neither.
pub fn cancels(stage: WorkKind, issue: &Issue, linewright: &Login) -> Option<Cancel> {
    let wanted = match stage {
        WorkKind::Triage => Asked::Triage,
        WorkKind::Design
        | WorkKind::Implement
        | WorkKind::Rework
        | WorkKind::FixCi
        | WorkKind::Update => Asked::DesignAndImplementation,
    };
    if !issue.open {
        Some(Cancel::Closed)
    } else if issue.status_label().map(StatusLabel::asks) != Some(Asks::Linewright(wanted)) {
        Some(Cancel::Relabeled(wanted.label()))
    } else if !issue.is_assigned(linewright) {
        Some(Cancel::Unassigned)
    } else {
        None
    }
}

/// When an issue with `events` was last opened again or given a status
/// label, as far as its events tell.
fn opened_or_labeled(events: &[IssueEvent]) -> Option<Timestamp> {
    events
        .iter()
        .filter_map(|event| match event {
            IssueEvent::Reopened { at } => Some(*at),
            IssueEvent::Labeled { label, at, .. }
                if StatusLabel::ALL
                    .iter()
                    .any(|status| status.label() == label) =>
            {
                Some(*at)
            }
            IssueEvent::Labeled { .. }
            | IssueEvent::Unlabeled { .. }
            | IssueEvent::Commented { .. }
            | IssueEvent::Assignment(_) => None,
        })
        .max()
}
