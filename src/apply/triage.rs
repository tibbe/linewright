//! What a triage result writes (docs/specs/triage.md).

use super::{Asks, StatusTo};
use crate::domain::{Issue, Login, StatusLabel, addressed_to, label};
use crate::results::{TriageOutcome, TriageResult};
use crate::store::Categories;

/// The writes of a triage result, which the run makes in field order.
#[derive(Debug, PartialEq, Eq)]
pub struct Triage {
    /// The people to assign; none means no assignment.
    pub assign: Vec<Login>,
    pub comment: String,
    pub category: CategoryChange,
    pub asks: Asks,
}

/// How the issue takes its category, by the repository's scheme.
#[derive(Debug, PartialEq, Eq)]
pub enum CategoryChange {
    IssueType(String),
    Labels {
        /// The other category labels the issue holds.
        remove: Vec<&'static str>,
        add: String,
    },
}

/// What triaging `issue` into `result` writes, with `categories` the
/// repository's scheme and `linewright` Linewright's login.
pub fn triage(
    issue: &Issue,
    result: &TriageResult,
    categories: &Categories,
    linewright: &Login,
) -> Triage {
    let (status, assign, comment) = match &result.outcome {
        TriageOutcome::ReadyForAgent => {
            (StatusLabel::ReadyForAgent, vec![], result.comment.clone())
        }
        TriageOutcome::NeedsInfo { ask } => {
            let people = issue.people_asked(ask, linewright);
            let comment = addressed_to(&people, &result.comment);
            (StatusLabel::NeedsInfo, people, comment)
        }
        TriageOutcome::ReadyForHuman => (
            StatusLabel::ReadyForHuman,
            vec![],
            format!(
                "{}\n\nAnswer with a label: {}. A comment alone starts nothing.",
                result.comment,
                label::ANSWERS
                    .map(|(answer, asks)| format!("`{answer}` {asks}"))
                    .join(" or ")
            ),
        ),
    };
    let category = match categories {
        Categories::IssueTypes(_) => CategoryChange::IssueType(result.category.clone()),
        Categories::Labels => CategoryChange::Labels {
            remove: [label::BUG, label::ENHANCEMENT]
                .into_iter()
                .filter(|l| *l != result.category && issue.has_label(l))
                .collect(),
            add: result.category.clone(),
        },
    };
    Triage {
        assign,
        comment,
        category,
        asks: Asks {
            status: Some(StatusTo::Set(status)),
            unassign: !result.outcome.asks_for_linewright(),
            ..Asks::default()
        },
    }
}
