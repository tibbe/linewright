//! Pull requests, as Linewright opens, readies and closes them and reads
//! them back.

use anyhow::{Context, Result, bail, ensure};
use linewright::domain::{
    Assignment, ClosedKind, IssueCloseReason, Mergeability, Number, Pull, PullState, Rebaseability,
};
use linewright::store::{AnyItem, IssueRead, NewPull, PeoplesPullRead, PushMode};

use crate::check::{contracts, equal};
use crate::sandbox::{NewCommit, Opened, Sandbox, commit, recent};

contracts![
    an_opened_pull_request_is_a_draft_of_linewrights_into_the_default_branch,
    a_snapshot_holds_an_open_pull_request_of_linewrights,
    a_pull_request_marked_ready_is_no_longer_a_draft,
    a_closed_pull_request_reads_back_closed_unmerged,
    a_snapshot_holds_linewrights_pull_request_closed_unmerged_as_closing_its_closed_ticket,
    a_pull_request_lists_the_issues_it_closes,
    a_push_to_a_pull_requests_branch_is_listed_with_who_pushed_it,
    a_pull_request_read_again_lists_its_assignments_as_a_snapshot_does,
    opening_a_pull_request_from_a_missing_branch_is_refused,
    no_pull_request_is_read_for_a_number_the_store_lacks,
];

/// Pushes a branch named for `what`, adding a file of its own, and opens a
/// pull request from it.
async fn opened(sandbox: &Sandbox, what: &str) -> Result<Opened> {
    let path = format!("{}.txt", sandbox.unique(what));
    let branch = sandbox
        .push_branch(
            what,
            &[NewCommit {
                message: "Add a file",
                files: &[(&path, b"added\n")],
            }],
        )
        .await?;
    sandbox.open_pull(branch, what).await
}

/// `pull`, but for whether it merges or rebases, which GitHub works out
/// when asked, so that two reads may differ on it.
fn worked_out(pull: &Pull) -> Pull {
    Pull {
        mergeability: Mergeability::Unknown,
        rebaseability: Rebaseability::Unknown,
        ..pull.clone()
    }
}

/// The pull request #`pr`, once its head is `head`: GitHub moves a pull
/// request's head moments after a push.
async fn at(sandbox: &Sandbox, pr: Number, head: &str) -> Result<Pull> {
    sandbox
        .eventually(async || {
            let pull = sandbox.store().pull(pr).await?;
            equal("the head", pull.head_sha.as_str(), head)?;
            Ok(pull)
        })
        .await
}

async fn an_opened_pull_request_is_a_draft_of_linewrights_into_the_default_branch(
    sandbox: Sandbox,
) -> Result<()> {
    let branch = sandbox
        .push_branch(
            "open",
            &[NewCommit {
                message: "Add a file",
                files: &[("opened.txt", b"opened\n")],
            }],
        )
        .await?;
    let title = sandbox.unique("opened");

    let pr = sandbox
        .store()
        .open_pull(NewPull {
            head: branch.name.clone(),
            title: title.clone(),
            body: "Opened by a contract.".into(),
        })
        .await?;
    sandbox.made_pull(pr);

    let linewright = sandbox.store().identity().await?.login;
    let default_branch = sandbox.default_branch().await?;
    let pull = at(&sandbox, pr, &branch.head).await?;
    equal("the number", &pull.number, &pr)?;
    equal("the title", &pull.title, &title)?;
    equal("the body", pull.body.as_str(), "Opened by a contract.")?;
    equal("the author", &pull.author, &linewright)?;
    equal("the base", &pull.base_ref, &default_branch)?;
    equal("the head branch", &pull.head_ref, &branch.name)?;
    equal("the state", &pull.state, &PullState::Open { draft: true })?;
    equal("the issues it closes", &pull.closes, &vec![])?;
    equal("the labels", &pull.labels, &Vec::<String>::new())?;
    equal("the assignees", &pull.assignees, &vec![])?;
    equal("the comments", &pull.comments, &vec![])?;
    equal("the reviews", &pull.reviews, &vec![])?;
    ensure!(pull.created_at <= pull.updated_at, "updated before created");
    let pull = worked_out(&pull);
    let read = sandbox.store().linewright_pull(pr).await?;
    equal(
        "Linewright's pull request",
        &read.as_ref().map(worked_out),
        &Some(pull.clone()),
    )?;
    match sandbox.store().any_item(pr).await? {
        Some(AnyItem::Pull(any)) => equal("any item", &worked_out(&any), &pull),
        other => bail!("any item is {other:?}"),
    }
}

async fn a_snapshot_holds_an_open_pull_request_of_linewrights(sandbox: Sandbox) -> Result<()> {
    let Opened { branch, pr } = opened(&sandbox, "snapshot-pull").await?;

    at(&sandbox, pr, &branch.head).await?;
    sandbox
        .eventually(async || {
            let read = sandbox.store().pull(pr).await?;
            let snapshot = sandbox.store().snapshot(recent()).await?;
            ensure!(
                snapshot.peoples_pulls.iter().all(|p| p.number != pr),
                "the snapshot holds {pr} among the people's"
            );
            match snapshot.pull(pr) {
                Some(pull) => equal("the pull request", &worked_out(pull), &worked_out(&read)),
                None => bail!("the snapshot does not hold {pr}"),
            }
        })
        .await
}

async fn a_pull_request_marked_ready_is_no_longer_a_draft(sandbox: Sandbox) -> Result<()> {
    let Opened { pr, .. } = opened(&sandbox, "ready").await?;

    sandbox.store().mark_ready(pr).await?;

    // GitHub may answer a read with the pull request as it was.
    sandbox
        .eventually(async || {
            let pull = sandbox.store().pull(pr).await?;
            equal("the state", &pull.state, &PullState::Open { draft: false })
        })
        .await
}

async fn a_closed_pull_request_reads_back_closed_unmerged(sandbox: Sandbox) -> Result<()> {
    let Opened { pr, .. } = opened(&sandbox, "close").await?;

    sandbox.store().close_pull(pr).await?;

    // GitHub may answer a read with the pull request as it was.
    sandbox
        .eventually(async || {
            let pull = sandbox.store().pull(pr).await?;
            let PullState::Closed { at } = pull.state else {
                bail!("the state is {:?}", pull.state);
            };
            ensure!(at >= pull.created_at, "closed before it was opened");
            let merged = sandbox.store().merged_pull(pr, recent()).await?;
            equal("the merged pull request", &merged, &None)?;
            let peoples = sandbox.store().peoples_pull(pr).await?;
            equal(
                "the people's pull request",
                &peoples,
                &PeoplesPullRead::default(),
            )?;
            // Unlike an open one, a snapshot holds it only while it closes a
            // ticket.
            let read = sandbox.store().linewright_pull(pr).await?;
            equal("Linewright's pull request", &read, &None)?;
            match sandbox.store().any_item(pr).await? {
                Some(AnyItem::Pull(any)) => equal("any item", &any.state, &pull.state),
                other => bail!("any item is {other:?}"),
            }
        })
        .await
}

async fn a_snapshot_holds_linewrights_pull_request_closed_unmerged_as_closing_its_closed_ticket(
    sandbox: Sandbox,
) -> Result<()> {
    let (issue, Opened { pr, .. }) = sandbox.open_pull_closing_an_issue("closes-ticket").await?;
    sandbox.linked(pr, issue).await?;

    sandbox.store().close_pull(pr).await?;
    sandbox
        .store()
        .close_issue(issue, IssueCloseReason::Completed)
        .await?;

    let linewright = sandbox.store().identity().await?.login;
    sandbox
        .eventually(async || {
            let snapshot = sandbox.store().snapshot(recent()).await?;
            ensure!(
                snapshot.closed.iter().any(|item| item.number == issue),
                "the snapshot does not hold {issue} closed"
            );
            let closed = snapshot
                .closed
                .iter()
                .find(|item| item.number == pr)
                .with_context(|| format!("the snapshot does not hold {pr} closed"))?;
            equal(
                "the closed pull request",
                &closed.kind,
                &ClosedKind::Pull {
                    closes: vec![issue],
                    author: Some(linewright.clone()),
                },
            )
        })
        .await
}

async fn a_pull_request_lists_the_issues_it_closes(sandbox: Sandbox) -> Result<()> {
    let (issue, Opened { pr, .. }) = sandbox.open_pull_closing_an_issue("closes").await?;

    // GitHub links the issue moments after the pull request opens.
    sandbox
        .eventually(async || {
            let pull = sandbox.store().pull(pr).await?;
            equal("the issues it closes", &pull.closes, &vec![issue])?;
            let peoples = sandbox.store().peoples_pull(pr).await?;
            equal(
                "the issues it closes, read as the people's",
                &peoples.closes,
                &vec![issue],
            )?;
            equal("the people's pull request", &peoples.pull, &None)?;
            match sandbox.store().issue(issue).await? {
                IssueRead::Held { pulls, .. } => {
                    let numbers: Vec<Number> = pulls.iter().map(|p| p.number).collect();
                    equal("the pull requests closing the issue", &numbers, &vec![pr])
                }
                other @ (IssueRead::LeftOut { .. } | IssueRead::Closed(_) | IssueRead::Gone) => {
                    bail!("{issue} is not held: {other:?}")
                }
            }
        })
        .await
}

async fn a_push_to_a_pull_requests_branch_is_listed_with_who_pushed_it(
    sandbox: Sandbox,
) -> Result<()> {
    let Opened { branch, pr } = opened(&sandbox, "pushes").await?;
    let opened_at = at(&sandbox, pr, &branch.head).await?.created_at;
    let clone = sandbox.clone(&branch.name).await?;
    let head = commit(
        &clone,
        &[NewCommit {
            message: "Change it",
            files: &[("pushed.txt", b"pushed\n")],
        }],
    )
    .await?;

    let expected = branch.head.clone();
    sandbox
        .push(&clone, &branch.name, PushMode::Lease { expected })
        .await?;

    let linewright = sandbox.store().identity().await?.login;
    at(&sandbox, pr, &head).await?;
    // GitHub lists a push moments after it.
    sandbox
        .eventually(async || {
            let pull = sandbox.store().pull(pr).await?;
            let since: Vec<_> = pull.pushes.iter().filter(|p| p.at >= opened_at).collect();
            match since[..] {
                [push] => equal("who pushed", &push.by, &linewright),
                ref pushes => bail!("the pushes since it opened are {pushes:?}"),
            }
        })
        .await
}

async fn a_pull_request_read_again_lists_its_assignments_as_a_snapshot_does(
    sandbox: Sandbox,
) -> Result<()> {
    let Opened { pr, .. } = opened(&sandbox, "assigned").await?;
    sandbox.store().assign(pr).await?;
    sandbox.store().unassign(pr).await?;

    sandbox.store().assign(pr).await?;

    let linewright = sandbox.store().identity().await?.login;
    // GitHub writes the assignments to the timeline moments after.
    sandbox
        .eventually(async || {
            let read = sandbox
                .store()
                .linewright_pull(pr)
                .await?
                .with_context(|| format!("{pr} is not read as Linewright's"))?;
            equal("the assignees", &read.assignees, &vec![linewright.clone()])?;
            let changes: Vec<_> = read
                .assignments
                .iter()
                .map(|change| match change {
                    Assignment::Assigned {
                        assignee, actor, ..
                    } => (true, assignee, actor),
                    Assignment::Unassigned {
                        assignee, actor, ..
                    } => (false, assignee, actor),
                })
                .collect();
            let by = (&linewright, &linewright);
            equal(
                "the assignments",
                &changes,
                &vec![(true, by.0, by.1), (false, by.0, by.1), (true, by.0, by.1)],
            )?;
            let snapshot = sandbox.store().snapshot(recent()).await?;
            match snapshot.pull(pr) {
                Some(pull) => equal("the pull request", &worked_out(pull), &worked_out(&read)),
                None => bail!("the snapshot does not hold {pr}"),
            }
        })
        .await
}

async fn opening_a_pull_request_from_a_missing_branch_is_refused(sandbox: Sandbox) -> Result<()> {
    let opened = sandbox
        .store()
        .open_pull(NewPull {
            head: sandbox.unique("no-such-branch"),
            title: sandbox.unique("missing-branch"),
            body: "Opened by a contract.".into(),
        })
        .await;

    match opened {
        Ok(pr) => {
            sandbox.made_pull(pr);
            bail!("{pr} opened")
        }
        Err(_) => Ok(()),
    }
}

async fn no_pull_request_is_read_for_a_number_the_store_lacks(sandbox: Sandbox) -> Result<()> {
    let lacked = Number(999_999_999);

    let store = sandbox.store();
    equal("any item", &store.any_item(lacked).await?, &None)?;
    equal(
        "Linewright's pull request",
        &store.linewright_pull(lacked).await?,
        &None,
    )?;
    equal(
        "the people's pull request",
        &store.peoples_pull(lacked).await?,
        &PeoplesPullRead::default(),
    )?;
    equal(
        "the merged pull request",
        &store.merged_pull(lacked, recent()).await?,
        &None,
    )
}
