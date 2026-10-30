//! What a finished pull request run writes, and the feedback that decides
//! a rework's reviewers.

use super::closing;
use crate::domain::{Login, Pull, Timestamp};
use crate::reconcile::{Feedback, WorkKind};
use crate::results::ThreadReply;

/// The writes of a finished pull request run, which the run makes in field
/// order.
#[derive(Debug, PartialEq, Eq)]
pub struct PullWorkWrites {
    /// The pull request's new description, if the session gave one.
    pub body: Option<String>,
    pub replies: Vec<Reply>,
    pub comment: String,
    /// The people to request review from; none means no request.
    pub reviewers: Vec<Login>,
}

/// A reply in one of the pull request's review threads.
#[derive(Debug, PartialEq, Eq)]
pub struct Reply {
    pub thread: String,
    pub body: String,
    /// Resolve the thread after replying.
    pub resolve: bool,
}

/// What a `kind` run on `pr` that finished with `pr_body`, `comment` and
/// `replies` writes, having shown the session the feedback of
/// `feedback_authors`.
pub fn done(
    pr: &Pull,
    kind: WorkKind,
    pr_body: Option<&str>,
    comment: &str,
    replies: &[ThreadReply],
    feedback_authors: &[Login],
) -> PullWorkWrites {
    let replies = replies
        .iter()
        .map(|reply| {
            let opened_by_bot = pr
                .threads
                .iter()
                .find(|t| t.id == reply.thread)
                .and_then(|t| t.opened_by())
                .is_some_and(Login::is_bot);
            Reply {
                thread: reply.thread.clone(),
                body: reply.body.clone(),
                resolve: reply.resolve && opened_by_bot,
            }
        })
        .collect();
    let mut reviewers = if kind == WorkKind::Rework {
        feedback_authors.to_vec()
    } else {
        vec![]
    };
    reviewers.sort();
    reviewers.dedup();
    PullWorkWrites {
        body: pr_body.map(|body| closing(body, &pr.closes)),
        replies,
        comment: comment.to_string(),
        reviewers,
    }
}

/// The part of `feedback` given after `since`.
pub fn late<'a>(feedback: &Feedback<'a>, since: Timestamp) -> Feedback<'a> {
    Feedback {
        comments: feedback
            .comments
            .iter()
            .copied()
            .filter(|c| c.created_at > since)
            .collect(),
        reviews: feedback
            .reviews
            .iter()
            .copied()
            .filter(|r| r.submitted_at > since)
            .collect(),
        threads: feedback
            .threads
            .iter()
            .filter_map(|(t, comments)| {
                let comments: Vec<_> = comments
                    .iter()
                    .copied()
                    .filter(|c| c.created_at > since)
                    .collect();
                (!comments.is_empty()).then_some((*t, comments))
            })
            .collect(),
    }
}

/// The people besides `linewright` who gave `feedback`, once for each
/// comment or review.
pub fn feedback_people(feedback: &Feedback, linewright: &Login) -> Vec<Login> {
    feedback
        .comments
        .iter()
        .map(|c| &c.author)
        .chain(feedback.reviews.iter().map(|r| &r.author))
        .chain(
            feedback
                .threads
                .iter()
                .flat_map(|(_, cs)| cs.iter().map(|c| &c.author)),
        )
        .filter(|a| a.is_person_besides(linewright))
        .cloned()
        .collect()
}
