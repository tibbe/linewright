//! How Linewright works a pull request it opened
//! (`docs/specs/pull_requests.md`, Working a pull request): the facts its
//! rows read, worked out once ([`facts`]), and the first row that holds
//! ([`row`]). The reconcile does what the row gives.
//!
//! The facts and rows still read the earlier prose rather than the table:
//! feedback after Linewright's last push, the unmergeable update once per
//! head, the blocked-merge comment by its requirements, and a draft merged or
//! held in the step that marks it ready. The next change makes them the
//! table's.

use crate::domain::{
    Assignment, CheckState, Login, MergeMethod, Mergeability, Pull, PullState, Rebaseability,
    Requirement, ReviewState, Timestamp,
};
use crate::reconcile::{Memory, Policy, WorkKind, new_feedback, rebases_to_repair};

/// What the rows read of a pull request, as the model's `Pr` has them
/// (`docs/specs/model/pull_request.qnt`).
#[expect(
    clippy::struct_excessive_bools,
    reason = "one field per fact, as the table and the model have them"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// Comments or reviews from anyone else since Linewright's last push.
    pub feedback: bool,
    /// GitHub reports it behind the default branch, or in conflict with it.
    pub behind: bool,
    /// GitHub reports it not rebaseable, and a rebase may make it so.
    pub unmergeable: bool,
    /// An update has rebased its head since Linewright was last assigned to
    /// it again.
    pub rebase_tried: bool,
    pub checks: Checks,
    /// Its last push was less than `ci-grace` ago.
    pub within_grace: bool,
    /// `ci-timeout` has passed since its last push, or since Linewright was
    /// last assigned to it again where that is later.
    pub timed_out: bool,
    /// Fix-ci runs done on it since it was opened, last reworked or
    /// Linewright was last assigned to it again.
    pub fixes: usize,
    /// The `ci-attempts` setting.
    pub ci_attempts: usize,
    pub draft: bool,
    /// The method Linewright merges by with `--auto-merge`.
    pub auto_merge: Option<MergeMethod>,
    /// A person holds its merge (`docs/specs/pull_requests.md`, Merging).
    pub held: bool,
    /// GitHub's reasons for its failed merges since Linewright was last
    /// assigned to it again, oldest first.
    pub failed_merges: Vec<String>,
    pub github: GitHub,
    /// Linewright has commented naming the requirements GitHub blocks it by.
    pub commented: bool,
}

/// Its checks, summed up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checks {
    None,
    /// The names of those still running.
    Running(Vec<String>),
    /// None running; the names of those that failed.
    Failed(Vec<String>),
    Green,
}

/// Whether GitHub allows its merge by the method Linewright merges with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitHub {
    Allows,
    Blocks {
        requires: Vec<Requirement>,
    },
    /// It is to be merged by rebase, and GitHub reports it not rebaseable.
    NotRebaseable,
    /// It is to be merged by rebase, and GitHub has not reported whether it
    /// is rebaseable, though it reports nothing else blocking the merge.
    RebaseNotReported,
    NotReported,
}

/// The rows, in order. Rows that do the same share a variant; the order of
/// the branches of [`row`] keeps them apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Rework,
    /// An update that rebases a head GitHub reports not rebaseable.
    Rebase,
    Update,
    WaitForChecks,
    HandBackTimedOut {
        running: Vec<String>,
    },
    FixCi {
        failing: Vec<String>,
    },
    HandBackFailing {
        failing: Vec<String>,
        attempts: usize,
    },
    MarkReady,
    WaitForReview,
    HandBackMergeFailed {
        reason: String,
    },
    Merge {
        draft: bool,
        method: MergeMethod,
    },
    /// A row the table does not have; the next change drops it.
    HandBackNotRebaseable,
    /// Marks a draft ready for review too.
    CommentRequirements {
        draft: bool,
        requires: Vec<Requirement>,
    },
    WaitForRequirements {
        requires: Vec<Requirement>,
    },
    /// Marks the draft ready for review, so that GitHub reports whether it
    /// allows the merge.
    MarkReadyForMergeState,
    WaitForMergeState,
}

/// The first row that holds for a pull request with `facts`.
///
/// Where its order differs from the table's (the unmergeable update before
/// the update of one behind, and a draft merged or held in the step that
/// marks it ready), it is the earlier prose's; the next change makes it the
/// table's.
pub fn row(facts: &Facts) -> Row {
    if facts.feedback {
        Row::Rework
    } else if facts.unmergeable && (facts.behind || !facts.rebase_tried) {
        Row::Rebase
    } else if facts.behind {
        Row::Update
    } else if facts.checks == Checks::None && facts.within_grace {
        Row::WaitForChecks
    } else if let Checks::Running(running) = &facts.checks {
        if facts.timed_out {
            Row::HandBackTimedOut {
                running: running.clone(),
            }
        } else {
            Row::WaitForChecks
        }
    } else if let Checks::Failed(failing) = &facts.checks {
        if facts.fixes < facts.ci_attempts {
            Row::FixCi {
                failing: failing.clone(),
            }
        } else {
            Row::HandBackFailing {
                failing: failing.clone(),
                attempts: facts.fixes,
            }
        }
    } else if let Some(method) = facts.auto_merge.filter(|_| !facts.held) {
        merging(facts, method)
    } else if facts.draft {
        Row::MarkReady
    } else {
        Row::WaitForReview
    }
}

/// The first row that holds for a green pull request Linewright merges by
/// `method`.
fn merging(facts: &Facts, method: MergeMethod) -> Row {
    match &facts.github {
        GitHub::Allows => match facts.failed_merges.as_slice() {
            [.., _, last] => Row::HandBackMergeFailed {
                reason: last.clone(),
            },
            _ => Row::Merge {
                draft: facts.draft,
                method,
            },
        },
        GitHub::NotRebaseable => Row::HandBackNotRebaseable,
        GitHub::Blocks { requires } if facts.draft || !facts.commented => {
            Row::CommentRequirements {
                draft: facts.draft,
                requires: requires.clone(),
            }
        }
        GitHub::Blocks { requires } => Row::WaitForRequirements {
            requires: requires.clone(),
        },
        GitHub::NotReported if facts.draft => Row::MarkReadyForMergeState,
        GitHub::NotReported | GitHub::RebaseNotReported => Row::WaitForMergeState,
    }
}

/// The facts of `pr` at `now`, from it, what Linewright remembers of it and
/// `policy`.
pub fn facts(pr: &Pull, memory: &Memory, policy: &Policy, now: Timestamp) -> Facts {
    let linewright = &policy.linewright;
    let restarted = restarted_at(pr, linewright);
    let since_restart = |at: Timestamp| restarted.is_none_or(|r| at > r);
    let names = |state: CheckState| -> Vec<String> {
        pr.checks
            .iter()
            .filter(|c| c.state == state)
            .map(|c| c.name.clone())
            .collect()
    };
    let (running, failed) = (names(CheckState::Pending), names(CheckState::Failed));
    let checks = if !running.is_empty() {
        Checks::Running(running)
    } else if !failed.is_empty() {
        Checks::Failed(failed)
    } else if pr.checks.is_empty() {
        Checks::None
    } else {
        Checks::Green
    };
    let pushed = pr
        .pushes
        .iter()
        .map(|p| p.at)
        .chain([pr.created_at])
        .max()
        .unwrap_or(pr.created_at);
    let waiting_since = restarted.map_or(pushed, |r| r.max(pushed));
    let reworked = memory.last_done(pr.number, WorkKind::Rework);
    let cycle_start = [Some(pr.created_at), reworked, restarted]
        .into_iter()
        .flatten()
        .max();
    let (github, commented) = match &pr.mergeability {
        Mergeability::Clean if policy.auto_merge == Some(MergeMethod::Rebase) => {
            let github = match pr.rebaseability {
                Rebaseability::Rebaseable => GitHub::Allows,
                Rebaseability::NotRebaseable => GitHub::NotRebaseable,
                Rebaseability::Unknown => GitHub::RebaseNotReported,
            };
            (github, false)
        }
        Mergeability::Clean => (GitHub::Allows, false),
        Mergeability::Blocked { requires } => (
            GitHub::Blocks {
                requires: requires.clone(),
            },
            memory.held.is_held_for(pr.number, requires),
        ),
        // A pull request behind or conflicting is updated before any row
        // reads this.
        Mergeability::Unknown | Mergeability::Behind | Mergeability::Conflicting => {
            (GitHub::NotReported, false)
        }
    };
    Facts {
        feedback: !new_feedback(pr, linewright, reworked).is_empty(),
        behind: matches!(
            pr.mergeability,
            Mergeability::Behind | Mergeability::Conflicting
        ),
        unmergeable: rebases_to_repair(pr, policy),
        rebase_tried: memory
            .tried_rebases
            .iter()
            .any(|r| r.pr == pr.number && r.head == pr.head_sha && since_restart(r.at)),
        checks,
        within_grace: now < pushed + policy.ci_grace,
        timed_out: now > waiting_since + policy.ci_timeout,
        fixes: memory
            .done_runs_of(pr.number, WorkKind::FixCi)
            .filter(|r| Some(r.done_at) > cycle_start)
            .count(),
        ci_attempts: policy.ci_attempts as usize,
        draft: pr.state == (PullState::Open { draft: true }),
        auto_merge: policy.auto_merge,
        held: is_held(pr, linewright),
        failed_merges: memory
            .failed_merges
            .iter()
            .filter(|f| f.pr == pr.number && since_restart(f.at))
            .map(|f| f.reason.clone())
            .collect(),
        github,
        commented,
    }
}

/// When Linewright was last assigned again after an unassignment, which
/// restarts the pull request's retry counters.
fn restarted_at(pr: &Pull, linewright: &Login) -> Option<Timestamp> {
    let mut unassigned = false;
    let mut restarted = None;
    for assignment in &pr.assignments {
        match assignment {
            Assignment::Unassigned { assignee, .. } if assignee == linewright => {
                unassigned = true;
            }
            Assignment::Assigned { assignee, at, .. } if assignee == linewright && unassigned => {
                restarted = Some(*at);
            }
            Assignment::Assigned { .. } | Assignment::Unassigned { .. } => {}
        }
    }
    restarted
}

/// The inline comments submitted with an approval are made no later than it,
/// so they don't hold the merge.
fn is_held(pr: &Pull, linewright: &Login) -> bool {
    let comments = pr
        .comments
        .iter()
        .chain(pr.threads.iter().flat_map(|t| &t.comments))
        .map(|c| (&c.author, c.created_at));
    let reviews = pr.reviews.iter().map(|r| (&r.author, r.submitted_at));
    let activity: Vec<_> = comments
        .chain(reviews)
        .filter(|(author, _)| author.is_person_besides(linewright))
        .collect();
    activity.iter().any(|(person, _)| {
        let approved = pr
            .reviews
            .iter()
            .filter(|r| &r.author == *person && r.state == ReviewState::Approved)
            .map(|r| r.submitted_at)
            .max();
        approved.is_none_or(|approved| {
            activity
                .iter()
                .any(|(author, at)| author == person && *at > approved)
        })
    })
}
