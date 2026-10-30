//! Issues, as Linewright opens and changes them and reads them back.

use anyhow::{Result, bail, ensure};
use linewright::domain::{
    Assignment, ClosedKind, Issue, IssueCloseReason, IssueClosing, IssueEvent, Linked, Login,
    Number, SubIssue, SubIssueState,
};
use linewright::store::{AnyItem, IssueRead, NewIssue};

use crate::check::{contracts, equal};
use crate::sandbox::{Sandbox, recent};

contracts![
    a_created_issue_reads_back_as_linewright_opened_it,
    a_snapshot_holds_an_issue_linewright_opened,
    an_added_label_is_on_the_issue_with_who_added_it,
    a_label_added_twice_is_on_the_issue_once,
    a_removed_label_is_off_the_issue_with_its_removal_recorded,
    removing_a_label_the_issue_lacks_changes_nothing,
    a_comment_reads_back_as_linewrights,
    linewright_assigned_is_an_assignee_it_assigned_itself,
    linewright_unassigned_is_no_longer_an_assignee,
    linewright_assigned_among_the_people_is_not_refused,
    an_edited_body_reads_back,
    an_issue_closed_as_completed_reads_back_closed_as_completed,
    an_issue_closed_as_not_planned_reads_back_closed_as_not_planned,
    an_issue_closed_as_a_duplicate_reads_back_closed_as_a_duplicate,
    a_snapshot_holds_an_issue_closed_within_its_windows,
    a_sub_issue_and_its_parent_name_each_other,
    a_blocker_is_listed_open_until_it_closes,
];

/// Opens an issue, as Linewright, titled for `what`.
async fn open(sandbox: &Sandbox, what: &str) -> Result<Number> {
    sandbox
        .store()
        .create_issue(NewIssue {
            title: sandbox.unique(what),
            body: format!("Opened to check {what}."),
            labels: vec![],
        })
        .await
}

async fn linewright(sandbox: &Sandbox) -> Result<Login> {
    Ok(sandbox.store().identity().await?.login)
}

/// The open issue #`number`, as [`Store::issue`] reads it.
async fn held(sandbox: &Sandbox, number: Number) -> Result<Issue> {
    match sandbox.store().issue(number).await? {
        IssueRead::Held { issue, .. } => Ok(*issue),
        other @ (IssueRead::LeftOut { .. } | IssueRead::Closed(_) | IssueRead::Gone) => {
            bail!("{number} is not held: {other:?}")
        }
    }
}

/// The events of `issue` that label or unlabel it `label`.
fn labelings<'a>(issue: &'a Issue, label: &str) -> Vec<&'a IssueEvent> {
    issue
        .events
        .iter()
        .filter(|event| match event {
            IssueEvent::Labeled { label: l, .. } | IssueEvent::Unlabeled { label: l, .. } => {
                l == label
            }
            IssueEvent::Reopened { .. }
            | IssueEvent::Commented { .. }
            | IssueEvent::Assignment(_) => false,
        })
        .collect()
}

/// The changes to `issue`'s assignees.
fn assignments(issue: &Issue) -> Vec<&Assignment> {
    issue
        .events
        .iter()
        .filter_map(|event| match event {
            IssueEvent::Assignment(assignment) => Some(assignment),
            IssueEvent::Labeled { .. }
            | IssueEvent::Unlabeled { .. }
            | IssueEvent::Reopened { .. }
            | IssueEvent::Commented { .. } => None,
        })
        .collect()
}

async fn a_created_issue_reads_back_as_linewright_opened_it(sandbox: Sandbox) -> Result<()> {
    let label = sandbox.new_label("filed", "Made by a contract").await?;
    let title = sandbox.unique("created");

    let number = sandbox
        .store()
        .create_issue(NewIssue {
            title: title.clone(),
            body: "Created by a contract.".into(),
            labels: vec![label.clone()],
        })
        .await?;

    let linewright = linewright(&sandbox).await?;
    // GitHub writes the labeling to the timeline after the issue.
    let issue = sandbox
        .eventually(async || {
            let issue = held(&sandbox, number).await?;
            match labelings(&issue, &label)[..] {
                [IssueEvent::Labeled { by, .. }] => {
                    equal("who labeled it", by, &Some(linewright.clone()))?;
                }
                ref events => bail!("labeled by {events:?}"),
            }
            Ok(issue)
        })
        .await?;
    equal("the number", &issue.number, &number)?;
    equal("the title", &issue.title, &title)?;
    equal("the body", issue.body.as_str(), "Created by a contract.")?;
    equal("the author", &issue.author, &linewright)?;
    ensure!(issue.open, "it is closed");
    equal("the labels", &issue.labels, &vec![label.clone()])?;
    equal("the assignees", &issue.assignees.len(), &0)?;
    equal("the comments", &issue.comments.len(), &0)?;
    equal("the blockers", &issue.blocked_by.len(), &0)?;
    equal("the sub-issues", &issue.sub_issues.len(), &0)?;
    equal("the parent", &issue.parent, &None)?;
    ensure!(
        issue.created_at <= issue.updated_at,
        "updated before created"
    );
    let any = sandbox.store().any_item(number).await?;
    equal("any item", &any, &Some(AnyItem::HeldIssue(Box::new(issue))))
}

async fn a_snapshot_holds_an_issue_linewright_opened(sandbox: Sandbox) -> Result<()> {
    let number = open(&sandbox, "snapshot").await?;

    let read = held(&sandbox, number).await?;
    sandbox
        .eventually(async || {
            let snapshot = sandbox.store().snapshot(recent()).await?;
            match snapshot.issues.iter().find(|issue| issue.number == number) {
                Some(issue) => equal("the issue", issue, &read),
                None => bail!("the snapshot does not hold {number}"),
            }
        })
        .await
}

async fn an_added_label_is_on_the_issue_with_who_added_it(sandbox: Sandbox) -> Result<()> {
    let label = sandbox.new_label("added", "Made by a contract").await?;
    let number = open(&sandbox, "add-label").await?;

    sandbox.store().add_label(number, &label).await?;

    let linewright = linewright(&sandbox).await?;
    sandbox
        .eventually(async || {
            let issue = held(&sandbox, number).await?;
            equal("the labels", &issue.labels, &vec![label.clone()])?;
            match labelings(&issue, &label)[..] {
                [IssueEvent::Labeled { by, .. }] => {
                    equal("who labeled it", by, &Some(linewright.clone()))
                }
                ref events => bail!("labeled by {events:?}"),
            }
        })
        .await
}

async fn a_label_added_twice_is_on_the_issue_once(sandbox: Sandbox) -> Result<()> {
    let label = sandbox.new_label("twice", "Made by a contract").await?;
    let number = open(&sandbox, "label-twice").await?;

    sandbox.store().add_label(number, &label).await?;
    sandbox.store().add_label(number, &label).await?;

    sandbox
        .eventually(async || {
            let issue = held(&sandbox, number).await?;
            equal("the labels", &issue.labels, &vec![label.clone()])?;
            equal("the labelings", &labelings(&issue, &label).len(), &1)
        })
        .await
}

async fn a_removed_label_is_off_the_issue_with_its_removal_recorded(
    sandbox: Sandbox,
) -> Result<()> {
    let label = sandbox.new_label("removed", "Made by a contract").await?;
    let number = open(&sandbox, "remove-label").await?;
    sandbox.store().add_label(number, &label).await?;

    sandbox.store().remove_label(number, &label).await?;

    sandbox
        .eventually(async || {
            let issue = held(&sandbox, number).await?;
            equal("the labels", &issue.labels.len(), &0)?;
            // GitHub may record the labeling again after the removal, the
            // issue still unlabeled.
            match labelings(&issue, &label)[..] {
                [IssueEvent::Labeled { .. }, ref after @ ..]
                    if after
                        .iter()
                        .any(|event| matches!(event, IssueEvent::Unlabeled { .. })) =>
                {
                    Ok(())
                }
                ref events => bail!("labeled and unlabeled by {events:?}"),
            }
        })
        .await
}

async fn removing_a_label_the_issue_lacks_changes_nothing(sandbox: Sandbox) -> Result<()> {
    let label = sandbox.new_label("lacked", "Made by a contract").await?;
    let number = open(&sandbox, "remove-lacked").await?;

    sandbox.store().remove_label(number, &label).await?;

    let issue = held(&sandbox, number).await?;
    equal("the labels", &issue.labels.len(), &0)?;
    equal("the labelings", &labelings(&issue, &label).len(), &0)
}

async fn a_comment_reads_back_as_linewrights(sandbox: Sandbox) -> Result<()> {
    let number = open(&sandbox, "comment").await?;

    sandbox
        .store()
        .comment(number, "A contract's comment.")
        .await?;

    let issue = held(&sandbox, number).await?;
    let linewright = linewright(&sandbox).await?;
    match &issue.comments[..] {
        [comment] => {
            equal("the author", &comment.author, &linewright)?;
            equal("the body", comment.body.as_str(), "A contract's comment.")?;
            ensure!(
                comment.created_at >= issue.created_at,
                "commented before the issue was opened"
            );
            Ok(())
        }
        comments => bail!("the comments are {comments:?}"),
    }
}

async fn linewright_assigned_is_an_assignee_it_assigned_itself(sandbox: Sandbox) -> Result<()> {
    let number = open(&sandbox, "assign").await?;

    sandbox.store().assign(number).await?;

    let linewright = linewright(&sandbox).await?;
    sandbox
        .eventually(async || {
            let issue = held(&sandbox, number).await?;
            equal("the assignees", &issue.assignees, &vec![linewright.clone()])?;
            match assignments(&issue)[..] {
                [
                    Assignment::Assigned {
                        assignee, actor, ..
                    },
                ] => {
                    equal("the assignee", assignee, &linewright)?;
                    equal("who assigned it", actor, &linewright)
                }
                ref changes => bail!("the assignments are {changes:?}"),
            }
        })
        .await
}

async fn linewright_unassigned_is_no_longer_an_assignee(sandbox: Sandbox) -> Result<()> {
    let number = open(&sandbox, "unassign").await?;
    sandbox.store().assign(number).await?;

    sandbox.store().unassign(number).await?;

    let linewright = linewright(&sandbox).await?;
    sandbox
        .eventually(async || {
            let issue = held(&sandbox, number).await?;
            equal("the assignees", &issue.assignees.len(), &0)?;
            match assignments(&issue)[..] {
                [
                    Assignment::Assigned { .. },
                    Assignment::Unassigned {
                        assignee, actor, ..
                    },
                ] => {
                    equal("the unassignee", assignee, &linewright)?;
                    equal("who unassigned it", actor, &linewright)
                }
                ref changes => bail!("the assignments are {changes:?}"),
            }
        })
        .await
}

async fn linewright_assigned_among_the_people_is_not_refused(sandbox: Sandbox) -> Result<()> {
    let number = open(&sandbox, "assign-people").await?;
    let linewright = linewright(&sandbox).await?;

    let refused = sandbox
        .store()
        .assign_people(number, std::slice::from_ref(&linewright))
        .await?;

    equal("those refused", &refused, &vec![])?;
    let issue = held(&sandbox, number).await?;
    equal("the assignees", &issue.assignees, &vec![linewright])
}

async fn an_edited_body_reads_back(sandbox: Sandbox) -> Result<()> {
    let number = open(&sandbox, "edit-body").await?;

    sandbox
        .store()
        .edit_body(number, "Edited by a contract.")
        .await?;

    let issue = held(&sandbox, number).await?;
    equal("the body", issue.body.as_str(), "Edited by a contract.")
}

/// Closes an issue opened for `what` with `reason`, and checks that both
/// [`Store::issue`] and [`Store::any_item`] read it back closed so.
async fn closed_as(sandbox: Sandbox, what: &str, reason: IssueCloseReason) -> Result<()> {
    let number = open(&sandbox, what).await?;

    sandbox.store().close_issue(number, reason).await?;

    match sandbox.store().issue(number).await? {
        IssueRead::Closed(closed) => {
            equal("the number", &closed.number, &number)?;
            match closed.kind {
                ClosedKind::Issue {
                    reason: read,
                    merged_by,
                    ..
                } => {
                    equal("the reason", &read, &Some(reason))?;
                    equal("the merge that closed it", &merged_by, &None)?;
                }
                ClosedKind::Pull { .. } => bail!("read as a pull request"),
            }
        }
        other @ (IssueRead::Held { .. } | IssueRead::LeftOut { .. } | IssueRead::Gone) => {
            bail!("{number} is not read closed: {other:?}")
        }
    }
    match sandbox.store().any_item(number).await? {
        Some(AnyItem::ClosedIssue { issue, closing }) => {
            ensure!(!issue.open, "the issue is open");
            let IssueClosing {
                at,
                merged_by,
                reason: read,
            } = closing;
            equal("the reason", &read, &Some(reason))?;
            equal("the merge that closed it", &merged_by, &None)?;
            ensure!(at >= issue.created_at, "closed before it was opened");
            Ok(())
        }
        other => bail!("{number} is not any closed item: {other:?}"),
    }
}

async fn an_issue_closed_as_completed_reads_back_closed_as_completed(
    sandbox: Sandbox,
) -> Result<()> {
    closed_as(sandbox, "completed", IssueCloseReason::Completed).await
}

async fn an_issue_closed_as_not_planned_reads_back_closed_as_not_planned(
    sandbox: Sandbox,
) -> Result<()> {
    closed_as(sandbox, "not-planned", IssueCloseReason::NotPlanned).await
}

async fn an_issue_closed_as_a_duplicate_reads_back_closed_as_a_duplicate(
    sandbox: Sandbox,
) -> Result<()> {
    closed_as(sandbox, "duplicate", IssueCloseReason::Duplicate).await
}

async fn a_snapshot_holds_an_issue_closed_within_its_windows(sandbox: Sandbox) -> Result<()> {
    let number = open(&sandbox, "closed-snapshot").await?;
    sandbox
        .store()
        .close_issue(number, IssueCloseReason::Completed)
        .await?;

    let IssueRead::Closed(read) = sandbox.store().issue(number).await? else {
        bail!("{number} is not read closed");
    };
    sandbox
        .eventually(async || {
            let snapshot = sandbox.store().snapshot(recent()).await?;
            ensure!(
                snapshot.issues.iter().all(|issue| issue.number != number),
                "the snapshot holds {number} open"
            );
            match snapshot.closed.iter().find(|item| item.number == number) {
                Some(closed) => equal("the closed issue", closed, &*read),
                None => bail!("the snapshot does not hold {number} closed"),
            }
        })
        .await
}

async fn a_sub_issue_and_its_parent_name_each_other(sandbox: Sandbox) -> Result<()> {
    let parent = open(&sandbox, "parent").await?;
    let child = open(&sandbox, "child").await?;

    sandbox.store().add_sub_issue(parent, child).await?;

    let linewright = linewright(&sandbox).await?;
    let expected = SubIssue {
        number: child,
        author: linewright,
        state: SubIssueState::Open,
    };
    equal(
        "the sub-issues",
        &held(&sandbox, parent).await?.sub_issues,
        &vec![expected],
    )?;
    let read = held(&sandbox, child).await?.parent;
    equal("the parent", &read.map(|p| p.number), &Some(parent))
}

async fn a_blocker_is_listed_open_until_it_closes(sandbox: Sandbox) -> Result<()> {
    let issue = open(&sandbox, "blocked").await?;
    let blocker = open(&sandbox, "blocker").await?;

    sandbox.store().add_blocked_by(issue, blocker).await?;

    let open_blocker = Linked {
        number: blocker,
        open: true,
    };
    equal(
        "the blockers",
        &held(&sandbox, issue).await?.blocked_by,
        &vec![open_blocker],
    )?;
    sandbox
        .store()
        .close_issue(blocker, IssueCloseReason::Completed)
        .await?;
    let closed_blocker = Linked {
        number: blocker,
        open: false,
    };
    equal(
        "the blockers",
        &held(&sandbox, issue).await?.blocked_by,
        &vec![closed_blocker],
    )
}
