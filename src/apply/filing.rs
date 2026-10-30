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
/// filed.
pub fn designed_asks(cut: &IssueWork) -> Asks {
    match cut {
        IssueWork::AsItIs => Asks {
            designed: Some(Designed::Add),
            ..Asks::default()
        },
        // A tracking issue carries no status label, and is worked through
        // its tickets.
        IssueWork::Cut(_) => Asks {
            designed: Some(Designed::Add),
            status: Some(StatusTo::Clear),
            unassign: true,
            ..Asks::default()
        },
    }
}

/// What a designed spec change asks for. Its label goes on last, so that a
/// design stopped before it is discarded and made again
/// (docs/specs/design.md, "Discarding a cut").
pub fn spec_change_designed() -> Asks {
    Asks {
        designed: Some(Designed::Add),
        unassign: true,
        ..Asks::default()
    }
}

/// What a cut's tickets are filed under.
#[derive(Clone, Copy)]
pub enum CutUnder<'a> {
    /// A merged spec change, whose tickets get a new tracking issue.
    SpecChange(&'a MergedPull),
    /// An issue, the tracking issue of its own tickets.
    Issue,
}

/// The issues a cut files, which the run files in order: the tracking
/// issue, then each ticket after those blocking it.
#[derive(Debug, PartialEq, Eq)]
pub struct Cut {
    /// The tracking issue to file, if the tickets get a new one.
    pub tracking_issue: Option<TrackingIssue>,
    pub tickets: Vec<NewTicket>,
}

/// A new tracking issue, for the tickets cut from `spec_change`.
#[derive(Debug, PartialEq, Eq)]
pub struct TrackingIssue {
    pub issue: NewIssue,
    pub spec_change: Number,
}

#[derive(Debug, PartialEq, Eq)]
pub struct NewTicket {
    pub issue: NewIssue,
    /// The positions in the cut's tickets of those blocking this one, all
    /// earlier.
    pub blocked_by: Vec<usize>,
}

/// The issues filing `tickets` under `under` makes, labeled `required`.
pub fn cut(tickets: &Tickets, under: CutUnder<'_>, required: Option<&str>) -> Cut {
    let labels: Vec<String> = required.into_iter().map(String::from).collect();
    Cut {
        tracking_issue: match under {
            CutUnder::SpecChange(pr) => Some(TrackingIssue {
                issue: NewIssue {
                    title: tracking::title(&pr.title),
                    body: tracking::body(pr.number, &[]),
                    labels: labels.clone(),
                },
                spec_change: pr.number,
            }),
            CutUnder::Issue => None,
        },
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

/// The writes that publish a cut once its tickets are filed, which the run
/// makes in field order.
#[derive(Debug, PartialEq, Eq)]
pub struct Published {
    /// The new tracking issue's body, listing the tickets.
    pub tracking_body: Option<String>,
    /// The design, posted on the tracking issue.
    pub design_comment: String,
    /// What each ticket asks for. They get `ready-for-agent` only after the
    /// design is on the tracking issue, so that their implementers read it
    /// (docs/specs/implementation.md, "What the implementer reads"), and
    /// only once they are designed, so that none is taken for an issue to
    /// design.
    pub ticket_asks: Asks,
    /// The comment on the spec change the tickets were cut from.
    pub spec_change_comment: Option<Comment>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Comment {
    /// The item to post it on.
    pub on: Number,
    pub body: String,
}

/// What publishing `design`, cut into the `filed` tickets under
/// `tracking_issue` for why `comment` gives, writes; `spec_change` is the
/// spec change they were cut from, if any.
pub fn published(
    design: &str,
    comment: &str,
    tracking_issue: Number,
    filed: &[FiledTicket],
    spec_change: Option<Number>,
) -> Published {
    let in_order: String = filed
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{}. {} {}\n", i + 1, t.number, t.title))
        .collect();
    Published {
        tracking_body: spec_change.map(|pr| tracking::body(pr, filed)),
        design_comment: format!("{design}\n\n## Tickets\n\n{in_order}\n{comment}"),
        ticket_asks: Asks {
            designed: Some(Designed::Add),
            status: Some(StatusTo::Set(StatusLabel::ReadyForAgent)),
            ..Asks::default()
        },
        spec_change_comment: spec_change.map(|pr| Comment {
            on: pr,
            body: format!("{comment}\n\nTracked in {tracking_issue}:\n\n{in_order}"),
        }),
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
