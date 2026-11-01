//! The Working a pull request table (`docs/specs/pull_requests.md`): the
//! facts its rows read about a pull request Linewright opened ([`facts`]),
//! and the first row that holds ([`row`]).
//!
//! Each fact keeps the reading Linewright gave it before the table, and
//! where that reading's order or outcome differs from the table's, the rows
//! keep it.

use crate::domain::{
    Assignment, CheckState, Login, MergeMethod, Mergeability, Pull, PullState, Rebaseability,
    Requirement, ReviewState, Timestamp,
};
use crate::reconcile::{Memory, Policy, WorkKind, new_feedback, rebases_to_repair};

/// What the rows read about one pull request, worked out once at a poll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// Feedback has come: a comment or review since Linewright's last push,
    /// the opening or the last rework run.
    pub feedback: bool,
    pub branch: Branch,
    pub checks: Checks,
    /// Fix-ci runs done on it since its opening, the last done rework run
    /// and Linewright's last assignment again.
    pub fix_ci_runs: usize,
    pub ci_attempts: usize,
    pub draft: bool,
    /// The method Linewright merges with, where `auto-merge` is on.
    pub auto_merge: Option<MergeMethod>,
    /// A person holds its merge.
    pub held: bool,
    /// GitHub's reasons for the merges that failed since Linewright was last
    /// assigned to it again, oldest first.
    pub failed_merges: Vec<String>,
    pub verdict: Verdict,
}

/// Whether its branch can be merged as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Branch {
    /// GitHub reports it behind the default branch, or in conflict with it,
    /// and `unmergeable` too.
    Behind {
        unmergeable: bool,
    },
    /// GitHub reports it not rebaseable, and an update would rebase it.
    Unmergeable {
        /// Linewright has started an update that rebases this head since
        /// it was last assigned to it again, as far as this process
        /// remembers.
        updated_at_head: bool,
    },
    Mergeable,
}

/// Its checks: the check runs and commit statuses on its head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checks {
    NoneAppeared {
        /// Its last push was less than `ci-grace` ago.
        within_grace: bool,
    },
    Running {
        names: Vec<String>,
        /// More than `ci-timeout` has passed since its last push, or since
        /// Linewright was last assigned to it again where that is later.
        past_timeout: bool,
    },
    /// The names of those that failed, none still running.
    Failed(Vec<String>),
    Green,
}

/// Whether GitHub allows the merge by `auto-merge`'s method.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allows,
    /// GitHub allows a merge, but reports it not rebaseable, and the method
    /// is rebase.
    NotRebaseable,
    Blocks {
        requires: Vec<Requirement>,
        /// Linewright has commented on what GitHub requires, for the same
        /// requirements.
        commented: bool,
    },
    NotReported,
}

/// What `pr` shows at `now`, with what Linewright remembers of it in
/// `memory`.
pub fn facts(pr: &Pull, memory: &Memory, policy: &Policy, now: Timestamp) -> Facts {
    let linewright = &policy.linewright;
    let restarted = restarted_at(pr, linewright);
    let reworked = memory.last_done(pr.number, WorkKind::Rework);
    let names = |state: CheckState| -> Vec<String> {
        pr.checks
            .iter()
            .filter(|c| c.state == state)
            .map(|c| c.name.clone())
            .collect()
    };
    let pushed = pr
        .pushes
        .iter()
        .map(|p| p.at)
        .chain([pr.created_at])
        .max()
        .unwrap_or(pr.created_at);
    let waiting_since = restarted.map_or(pushed, |r| r.max(pushed));
    let (running, failed) = (names(CheckState::Pending), names(CheckState::Failed));
    let checks = if !running.is_empty() {
        Checks::Running {
            names: running,
            past_timeout: now > waiting_since + policy.ci_timeout,
        }
    } else if !failed.is_empty() {
        Checks::Failed(failed)
    } else if pr.checks.is_empty() {
        Checks::NoneAppeared {
            within_grace: now < pushed + policy.ci_grace,
        }
    } else {
        Checks::Green
    };
    let unmergeable = rebases_to_repair(pr, policy);
    let branch = match pr.mergeability {
        Mergeability::Behind | Mergeability::Conflicting => Branch::Behind { unmergeable },
        _ if unmergeable => Branch::Unmergeable {
            updated_at_head: memory.tried_rebases.iter().any(|r| {
                r.pr == pr.number && r.head == pr.head_sha && restarted.is_none_or(|at| at < r.at)
            }),
        },
        Mergeability::Clean | Mergeability::Blocked { .. } | Mergeability::Unknown => {
            Branch::Mergeable
        }
    };
    let round_start = [Some(pr.created_at), reworked, restarted]
        .into_iter()
        .flatten()
        .max();
    let auto_merge = policy.auto_merge;
    let verdict = match (&pr.mergeability, auto_merge) {
        (Mergeability::Clean, Some(MergeMethod::Rebase)) => match pr.rebaseability {
            Rebaseability::Rebaseable => Verdict::Allows,
            Rebaseability::NotRebaseable => Verdict::NotRebaseable,
            Rebaseability::Unknown => Verdict::NotReported,
        },
        (Mergeability::Clean, _) => Verdict::Allows,
        (Mergeability::Blocked { requires }, _) => Verdict::Blocks {
            requires: requires.clone(),
            commented: memory.held.is_held_for(pr.number, requires),
        },
        // Behind and conflicting have a fact of their own, read first.
        (Mergeability::Unknown | Mergeability::Behind | Mergeability::Conflicting, _) => {
            Verdict::NotReported
        }
    };
    Facts {
        feedback: !new_feedback(pr, linewright, reworked).is_empty(),
        branch,
        checks,
        fix_ci_runs: memory
            .done_runs_of(pr.number, WorkKind::FixCi)
            .filter(|r| Some(r.done_at) > round_start)
            .count(),
        ci_attempts: policy.ci_attempts as usize,
        draft: pr.state == (PullState::Open { draft: true }),
        auto_merge,
        held: is_held(pr, linewright),
        failed_merges: memory
            .failed_merges
            .iter()
            .filter(|f| f.pr == pr.number && restarted.is_none_or(|r| f.at > r))
            .map(|f| f.reason.clone())
            .collect(),
        verdict,
    }
}

/// A row of the Working a pull request table. Rows that do the same share a
/// variant, and the order of [`row`]'s branches keeps them apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Rework,
    /// An update run, which Linewright remembers rebasing the head where
    /// `rebase`.
    Update {
        rebase: bool,
    },
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
    /// Marks a draft ready for review.
    MarkReady,
    WaitForReview,
    HandBackMergeFailed {
        reason: String,
    },
    /// Hands it back as GitHub reports it not rebaseable, though it was
    /// rebased: a row of its own, outside the table.
    HandBackNotRebaseable,
    /// Comments naming what GitHub requires, marking a `draft` ready for
    /// review first.
    CommentRequirements {
        requires: Vec<Requirement>,
        draft: bool,
    },
    WaitForRequirements {
        requires: Vec<Requirement>,
    },
    /// Marks the draft ready for review, so that GitHub reports whether it
    /// allows the merge.
    MarkReadyForMergeState,
    WaitForMergeState,
    /// Merges it with `method`, marking a `draft` ready for review first.
    Merge {
        method: MergeMethod,
        draft: bool,
    },
}

/// The first row that holds for `facts`.
pub fn row(facts: &Facts) -> Row {
    if facts.feedback {
        return Row::Rework;
    }
    match facts.branch {
        Branch::Behind { unmergeable } => {
            return Row::Update {
                rebase: unmergeable,
            };
        }
        Branch::Unmergeable {
            updated_at_head: false,
        } => return Row::Update { rebase: true },
        Branch::Unmergeable {
            updated_at_head: true,
        }
        | Branch::Mergeable => {}
    }
    match &facts.checks {
        Checks::NoneAppeared { within_grace: true } => return Row::WaitForChecks,
        Checks::Running {
            names,
            past_timeout: true,
        } => {
            return Row::HandBackTimedOut {
                running: names.clone(),
            };
        }
        Checks::Running { .. } => return Row::WaitForChecks,
        Checks::Failed(failing) if facts.fix_ci_runs < facts.ci_attempts => {
            return Row::FixCi {
                failing: failing.clone(),
            };
        }
        Checks::Failed(failing) => {
            return Row::HandBackFailing {
                failing: failing.clone(),
                attempts: facts.fix_ci_runs,
            };
        }
        Checks::NoneAppeared {
            within_grace: false,
        }
        | Checks::Green => {}
    }
    let Some(method) = facts.auto_merge.filter(|_| !facts.held) else {
        return if facts.draft {
            Row::MarkReady
        } else {
            Row::WaitForReview
        };
    };
    let draft = facts.draft;
    match &facts.verdict {
        Verdict::Allows => match facts.failed_merges.as_slice() {
            [.., _, last] => Row::HandBackMergeFailed {
                reason: last.clone(),
            },
            _ => Row::Merge { method, draft },
        },
        Verdict::NotRebaseable => Row::HandBackNotRebaseable,
        Verdict::Blocks {
            requires,
            commented,
        } if draft || !commented => Row::CommentRequirements {
            requires: requires.clone(),
            draft,
        },
        Verdict::Blocks { requires, .. } => Row::WaitForRequirements {
            requires: requires.clone(),
        },
        Verdict::NotReported if draft => Row::MarkReadyForMergeState,
        Verdict::NotReported => Row::WaitForMergeState,
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
