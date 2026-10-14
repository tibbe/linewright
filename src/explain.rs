//! `linewright explain N`: where issue or pull request #N stands and what
//! Linewright waits for before it acts on it next
//! (`docs/specs/monitoring.md`, "explain"). `serve` answers it from its own
//! view, with the waits the reconcile step derives; the command prints the
//! answer.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::domain::{Item, Number, Timestamp};
use crate::reconcile::Wait;
use crate::status::{InReview, ItemState, Paint, View, describe, describe_wait, tracked, utc};

/// The answer to `explain N`, as the local API sends it. Its fields keep
/// their names and meanings (`docs/specs/monitoring.md`, "Output").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    pub number: Number,
    pub title: String,
    #[serde(flatten)]
    pub standing: Standing,
    /// When `serve` last read #N.
    pub last_read: Timestamp,
    pub waiting_for: Vec<Wait>,
}

/// Where #N is in the pipeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Standing {
    /// Linewright's pull request for the issue `pull_request_for`, which is
    /// in review with it; the waits are the issue's.
    PullRequest {
        pull_request_for: Number,
        #[serde(flatten)]
        review: InReview,
    },
    /// An item Linewright tracks.
    Item(ItemState),
}

/// The answer about #`number` at `now`: an item Linewright tracks, or
/// Linewright's open pull request for one. `None` for any other number.
pub fn explain(view: &View, number: Number, now: Timestamp) -> Option<Explanation> {
    let items = tracked(view, now);
    let last_read = |item: Item| view.read_at.get(&item).copied().unwrap_or(view.polls.last);
    if let Some(tracked) = items.iter().find(|t| t.number == number) {
        // A tracked spec change is a merged pull request; the rest are issues.
        let read = if view.snapshot.issue(number).is_some() {
            Item::Issue(number)
        } else {
            Item::Pull(number)
        };
        return Some(Explanation {
            number,
            title: tracked.title.clone(),
            standing: Standing::Item(tracked.state.clone()),
            last_read: last_read(read),
            waiting_for: tracked.waiting_for.clone(),
        });
    }
    items.into_iter().find_map(|tracked| match tracked.state {
        ItemState::InReview(review) if review.pull_request == number => Some(Explanation {
            number,
            title: view.snapshot.pull(number)?.title.clone(),
            standing: Standing::PullRequest {
                pull_request_for: tracked.number,
                review,
            },
            last_read: last_read(Item::Pull(number)),
            waiting_for: tracked.waiting_for,
        }),
        _ => None,
    })
}

/// The answer as text: one fact per line as `label: value`, coloured if
/// `colour` (`docs/specs/monitoring.md`, "Output").
pub fn render(explanation: &Explanation, colour: bool) -> String {
    let paint = Paint(colour);
    let mut lines = vec![format!(
        "{}: {}",
        paint.bold(&explanation.number.to_string()),
        explanation.title
    )];
    let state = match &explanation.standing {
        Standing::PullRequest {
            pull_request_for,
            review,
        } => {
            lines.push(format!("pull request for: {pull_request_for}"));
            review.describe()
        }
        Standing::Item(state) => describe(state),
    };
    lines.push(format!("state: {state}"));
    lines.push(format!("last read: {}", utc(explanation.last_read)));
    if explanation.waiting_for.is_empty() {
        lines.push("waiting for: nothing: Linewright acts on it at its next poll".into());
    }
    for wait in &explanation.waiting_for {
        lines.push(format!("waiting for: {}", describe_wait(wait)));
    }
    let mut text = String::new();
    for line in lines {
        let _ = writeln!(text, "{line}");
    }
    text
}
