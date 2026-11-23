//! The Working a pull request table (`docs/specs/pull_requests.md`): the
//! facts its rows read about a pull request Linewright opened ([`facts`]),
//! and the first row that holds ([`row`]).

use crate::domain::{
    Assignment, CheckState, Comment, Login, MergeMethod, Mergeability, Pull, PullState,
    Rebaseability, Requirement, Review, ReviewDecision, ReviewState, Thread, Timestamp,
};
use crate::reconcile::{EndedRun, Memory, Policy, WorkKind};

/// What the rows read about one pull request, worked out once at a poll.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a fact of its own that a different row of the table reads"
)]
pub struct Facts {
    /// Feedback has come: see [`feedback`].
    pub feedback: bool,
    pub branch: Branch,
    /// GitHub reports it [`Mergeability::Behind`].
    pub behind: bool,
    pub checks: Checks,
    /// Fix-ci runs that have ended on it this round, however they ended.
    pub fix_ci_runs: usize,
    pub ci_attempts: usize,
    pub draft: bool,
    /// The method Linewright merges with, where `auto-merge` is on.
    pub auto_merge: Option<MergeMethod>,
    /// A person holds its merge.
    pub held: bool,
    /// GitHub requires a review it has not had ([`Pull::review_required`]),
    /// or a reviewer requested changes.
    pub review_required: bool,
    /// A person besides Linewright has submitted an approving review.
    pub approved: bool,
    /// GitHub's reasons for the merges of it that failed this round, oldest
    /// first.
    pub failed_merges: Vec<String>,
    pub verdict: Verdict,
}

/// Whether its branch can be merged as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Branch {
    /// GitHub reports it in conflict with the default branch.
    Conflicting,
    /// It cannot be merged by a method the repository allows: GitHub reports
    /// it not rebaseable, and the repository allows rebase.
    Unmergeable {
        /// A run has ended on it since its last push.
        ended_since_push: bool,
    },
    Mergeable,
}

impl Branch {
    /// A row that starts an update run for the branch's state holds: it
    /// conflicts, or it cannot be merged and no run has ended on it since its
    /// last push.
    fn needs_update(self) -> bool {
        match self {
            Branch::Conflicting
            | Branch::Unmergeable {
                ended_since_push: false,
            } => true,
            Branch::Unmergeable {
                ended_since_push: true,
            }
            | Branch::Mergeable => false,
        }
    }
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
    Blocks {
        requires: Vec<Requirement>,
        /// Linewright has commented on it this round naming what GitHub
        /// requires.
        commented: bool,
    },
    /// Also a merge by rebase while GitHub has not reported whether it is
    /// rebaseable.
    NotReported,
}

/// What `pr` shows at `now`, with what Linewright remembers of it in
/// `memory`.
pub fn facts(pr: &Pull, memory: &Memory, policy: &Policy, now: Timestamp) -> Facts {
    let linewright = &policy.linewright;
    let restarted = restarted_at(pr, linewright);
    let ended: Vec<&EndedRun> = memory.ended_on(pr.number).collect();
    let round_start = round_start(pr, &ended, restarted);
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
    let branch = match pr.mergeability {
        Mergeability::Conflicting => Branch::Conflicting,
        _ if unmergeable(pr, policy) => Branch::Unmergeable {
            ended_since_push: ended.iter().any(|r| r.at > pushed),
        },
        Mergeability::Clean
        | Mergeability::Behind
        | Mergeability::Blocked { .. }
        | Mergeability::Unknown => Branch::Mergeable,
    };
    let draft = pr.state == (PullState::Open { draft: true });
    let auto_merge = policy.auto_merge.then_some(policy.merge_method);
    let verdict = match (&pr.mergeability, auto_merge) {
        (Mergeability::Clean, Some(MergeMethod::Rebase))
            if pr.rebaseability == Rebaseability::Unknown =>
        {
            Verdict::NotReported
        }
        (Mergeability::Clean, _) => Verdict::Allows,
        (Mergeability::Blocked { requires }, _) => Verdict::Blocks {
            requires: requires.clone(),
            commented: memory
                .pulls
                .last_commented(pr.number)
                .is_some_and(|at| at > round_start),
        },
        (Mergeability::Unknown | Mergeability::Behind | Mergeability::Conflicting, _) => {
            Verdict::NotReported
        }
    };
    Facts {
        feedback: !feedback(pr, linewright, memory.rework_answered_until(pr.number)).is_empty(),
        branch,
        behind: pr.mergeability == Mergeability::Behind,
        checks,
        fix_ci_runs: ended
            .iter()
            .filter(|r| r.kind == WorkKind::FixCi && r.at > round_start)
            .count(),
        ci_attempts: policy.ci_attempts as usize,
        draft,
        auto_merge,
        held: is_held(pr, linewright),
        review_required: pr.review_required()
            || pr.review_decision == Some(ReviewDecision::ChangesRequested),
        approved: pr
            .reviews
            .iter()
            .any(|r| r.state == ReviewState::Approved && r.author.is_person_besides(linewright)),
        failed_merges: memory
            .pulls
            .failed_merges(pr.number)
            .iter()
            .filter(|f| f.at > round_start)
            .map(|f| f.reason.clone())
            .collect(),
        verdict,
    }
}

/// A row of the Working a pull request table. Rows that do the same share a
/// variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Rework,
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
    /// Marks a draft ready for review.
    MarkReady,
    WaitForReview,
    HandBackMergeFailed {
        reason: String,
    },
    CommentRequirements {
        requires: Vec<Requirement>,
    },
    WaitForRequirements {
        requires: Vec<Requirement>,
    },
    /// Waits for GitHub to report whether it allows the merge.
    WaitForMergeState,
    Merge {
        method: MergeMethod,
    },
}

/// The first row that holds for `facts`.
pub fn row(facts: &Facts) -> Row {
    if facts.feedback {
        return Row::Rework;
    }
    if facts.branch.needs_update() {
        return Row::Update;
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
    if facts.behind && ready_to_merge(facts) {
        return Row::Update;
    }
    if facts.draft {
        return Row::MarkReady;
    }
    let Some(method) = facts.auto_merge.filter(|_| !facts.held) else {
        return Row::WaitForReview;
    };
    if let [.., _, last] = facts.failed_merges.as_slice() {
        return Row::HandBackMergeFailed {
            reason: last.clone(),
        };
    }
    match &facts.verdict {
        Verdict::Blocks {
            requires,
            commented: false,
        } => Row::CommentRequirements {
            requires: requires.clone(),
        },
        Verdict::Blocks {
            requires,
            commented: true,
        } => Row::WaitForRequirements {
            requires: requires.clone(),
        },
        Verdict::NotReported => Row::WaitForMergeState,
        Verdict::Allows => Row::Merge { method },
    }
}

/// Whether a rework run on a pull request with `facts` starts by bringing
/// its branch up to date: where the branch conflicts with the default branch,
/// or cannot be merged by a method the repository allows and no run has ended
/// on it since its last push. Being behind plays no part.
pub fn rework_starts_with_update(facts: &Facts) -> bool {
    facts.branch.needs_update()
}

/// Ready to merge, but for being behind: it is ready for review, no person
/// holds its merge, GitHub requires no review it has not had and, with
/// `auto-merge` off, a person has approved it. It reads neither its checks
/// nor its feedback.
fn ready_to_merge(facts: &Facts) -> bool {
    !facts.draft
        && !facts.held
        && !facts.review_required
        && (facts.auto_merge.is_some() || facts.approved)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Feedback<'a> {
    pub comments: Vec<&'a Comment>,
    pub reviews: Vec<&'a Review>,
    pub threads: Vec<(&'a Thread, Vec<&'a Comment>)>,
}

impl Feedback<'_> {
    pub fn is_empty(&self) -> bool {
        self.comments.is_empty() && self.reviews.is_empty() && self.threads.is_empty()
    }
}

/// The feedback on `pr`: the comments, reviews and thread comments from
/// anyone but Linewright that came after the last rework run that ended done
/// last read its feedback, at `answered_until`, leaving out a review that
/// only approves.
pub fn feedback<'a>(
    pr: &'a Pull,
    linewright: &Login,
    answered_until: Option<Timestamp>,
) -> Feedback<'a> {
    let new = |author: &Login, at: Timestamp| {
        author != linewright && answered_until.is_none_or(|answered| at > answered)
    };
    Feedback {
        comments: pr
            .comments
            .iter()
            .filter(|c| new(&c.author, c.created_at))
            .collect(),
        reviews: pr
            .reviews
            .iter()
            .filter(|r| new(&r.author, r.submitted_at))
            .filter(|r| !(r.state == ReviewState::Approved && r.body.trim().is_empty()))
            .collect(),
        threads: pr
            .threads
            .iter()
            .filter_map(|t| {
                let comments: Vec<_> = t
                    .comments
                    .iter()
                    .filter(|c| new(&c.author, c.created_at))
                    .collect();
                (!comments.is_empty()).then_some((t, comments))
            })
            .collect(),
    }
}

/// It cannot be merged by its merge method, for a reason other than being
/// behind or conflicting: GitHub reports it not rebaseable, and the merge
/// method is rebase.
fn unmergeable(pr: &Pull, policy: &Policy) -> bool {
    pr.rebaseability == Rebaseability::NotRebaseable && policy.merge_method == MergeMethod::Rebase
}

/// When the round of `pr` began: the latest of its opening, the end of the
/// last rework run on it however it ended, and Linewright's last assignment
/// to it again.
fn round_start(pr: &Pull, ended: &[&EndedRun], restarted: Option<Timestamp>) -> Timestamp {
    ended
        .iter()
        .filter(|r| r.kind == WorkKind::Rework)
        .map(|r| r.at)
        .chain(restarted)
        .fold(pr.created_at, Timestamp::max)
}

/// When Linewright was last assigned again after an unassignment, which
/// starts a round.
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

/// Whether a person holds `pr`'s merge: someone other than Linewright has
/// commented or reviewed and has not approved since. The inline comments
/// submitted with an approval, which GitHub dates no later than it
/// (<https://docs.github.com/en/rest/pulls/reviews#submit-a-review-for-a-pull-request>),
/// do not hold it.
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
