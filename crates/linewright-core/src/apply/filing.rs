//! What a design, an implement run or an automation files and labels.

use super::{Asks, Designed, StatusTo, closing};
use crate::automation::Automation;
use crate::domain::{MergedPull, Number, StatusLabel, label};
use crate::results::{
    AutomationIssue, AutomationIssueLabel, AutomationResult, IssueWork, SpecGap, SpecGaps, Tickets,
};
use crate::spec_gap::{self, FiledGaps};
use crate::store::{NewIssue, NewPull};
use crate::tracking::{self, FiledTicket};

/// What a designed issue asks for once its design and any tickets are
/// filed: `linewright:designed`, and, once it is cut into tickets and so
/// becomes a tracking issue, no status label and no assignee.
pub fn designed_asks(cut: &IssueWork) -> Asks {
    match cut {
        IssueWork::AsItIs => Asks {
            designed: Some(Designed::Add),
            ..Asks::default()
        },
        IssueWork::Cut(_) => Asks {
            designed: Some(Designed::Add),
            status: Some(StatusTo::Clear),
            unassign: true,
            ..Asks::default()
        },
    }
}

/// The issues a cut files, in the order they are filed: each ticket after
/// those blocking it.
#[derive(Debug, PartialEq, Eq)]
pub struct Cut {
    pub tickets: Vec<NewTicket>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct NewTicket {
    pub issue: NewIssue,
    /// The positions in the cut's tickets of those blocking this one, all
    /// earlier.
    pub blocked_by: Vec<usize>,
}

/// The issues filing `tickets` makes, labeled `required`.
pub fn cut(tickets: &Tickets, required: Option<&str>) -> Cut {
    let labels: Vec<String> = required.into_iter().map(String::from).collect();
    Cut {
        tickets: tickets
            .in_order()
            .iter()
            .map(|ticket| NewTicket {
                issue: NewIssue {
                    title: ticket.title.clone(),
                    body: ticket.body.clone(),
                    labels: labels.clone(),
                },
                blocked_by: ticket.blocked_by.clone(),
            })
            .collect(),
    }
}

/// The writes that publish a cut once its tickets are filed, in the order
/// they are made.
#[derive(Debug, PartialEq, Eq)]
pub struct Published {
    /// The design, posted on the tracking issue.
    pub design_comment: String,
    /// What each ticket asks for: `ready-for-agent` and
    /// `linewright:designed`, after the design is on the tracking issue
    /// (docs/specs/implementation.md, "What the implementer reads").
    pub ticket_asks: Asks,
}

/// What publishing `design`, cut into the `filed` tickets for why `comment`
/// gives, writes.
pub fn published(design: &str, comment: &str, filed: &[FiledTicket]) -> Published {
    let in_order: String = filed
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{}. {} {}\n", i + 1, t.number, t.title))
        .collect();
    Published {
        design_comment: format!("{design}\n\n## Tickets\n\n{in_order}\n{comment}"),
        ticket_asks: Asks {
            designed: Some(Designed::Add),
            status: Some(StatusTo::Set(StatusLabel::ReadyForAgent)),
            ..Asks::default()
        },
    }
}

/// The issue taking up `spec_change` opens, labeled `required` as well
/// (`docs/specs/sources_of_work.md`, Spec changes).
pub fn spec_change_issue(spec_change: &MergedPull, required: Option<&str>) -> NewIssue {
    NewIssue {
        title: tracking::title(&spec_change.title),
        body: tracking::body(spec_change.number),
        labels: labels(label::READY_FOR_AGENT, required),
    }
}

/// The issues filing `gaps`, found while working on `item`, makes, labeled
/// `required` as well (docs/specs/runs.md, "Spec gaps").
pub fn spec_gap_issues(gaps: &[SpecGap], item: Number, required: Option<&str>) -> Vec<NewIssue> {
    gaps.iter()
        .map(|gap| NewIssue {
            title: gap.title.clone(),
            body: spec_gap::body(&gap.body, item),
            labels: labels(label::READY_FOR_HUMAN, required),
        })
        .collect()
}

/// A spec gap a session returned that is already filed, as `issue`.
#[derive(Debug, PartialEq, Eq)]
pub struct AlreadyFiled {
    pub gap: SpecGap,
    pub issue: Number,
}

/// Drops from `result`'s spec gaps those already filed and still open,
/// which are not filed again (docs/specs/runs.md, "Spec gaps"), and returns
/// them.
pub fn unfiled_gaps(result: &mut impl SpecGaps, filed: &FiledGaps<'_>) -> Vec<AlreadyFiled> {
    let Some(gaps) = result.spec_gaps_mut() else {
        return vec![];
    };
    let mut already = vec![];
    gaps.retain(|gap| match spec_gap::filed_as(filed, &gap.title) {
        Some(issue) => {
            already.push(AlreadyFiled {
                gap: gap.clone(),
                issue: issue.number,
            });
            false
        }
        None => true,
    });
    already
}

/// The pull request an implement run on `ticket` opens from `branch`.
pub fn new_pull(branch: &str, title: String, body: &str, ticket: Number) -> NewPull {
    NewPull {
        head: branch.to_string(),
        title,
        body: closing(body, &[ticket]),
    }
}

/// Drops from `result` the issues past `automation`'s `max-issues`, which
/// are not filed, and returns them.
pub fn past_max_issues(
    automation: &Automation,
    result: &mut AutomationResult,
) -> Vec<AutomationIssue> {
    let cap = (automation.max_issues as usize).min(result.issues.len());
    result.issues.split_off(cap)
}

/// The issues `automation`'s `result` files, labeled `required` as well.
pub fn automation_issues(
    automation: &Automation,
    result: &AutomationResult,
    required: Option<&str>,
) -> Vec<NewIssue> {
    result
        .issues
        .iter()
        .map(|issue| {
            let state = match issue.label {
                AutomationIssueLabel::ReadyForAgent => label::READY_FOR_AGENT,
                AutomationIssueLabel::ReadyForHuman => label::READY_FOR_HUMAN,
            };
            NewIssue {
                title: issue.title.clone(),
                body: format!(
                    "{}\n\nFiled by the automation `{}`.",
                    issue.body, automation.name
                ),
                labels: labels(state, required),
            }
        })
        .collect()
}

/// `status`, and `required` if there is one.
fn labels(status: &str, required: Option<&str>) -> Vec<String> {
    [status]
        .into_iter()
        .chain(required)
        .map(String::from)
        .collect()
}
