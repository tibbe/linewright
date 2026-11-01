//! An item's state (`docs/specs/repository.md`, State): the first row of
//! its table that holds, derived from what GitHub shows and the run in
//! flight on the item. Every view of an item's state asks here.

use serde::{Deserialize, Serialize};

use crate::board::Column;
use crate::domain::{
    ClosingPull, Issue, IssueEvent, Login, Merge, PullState, StatusLabel, Timestamp, label,
};
use crate::reconcile::WorkKind;

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
    } else if issue.tracks(linewright).is_some() {
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
