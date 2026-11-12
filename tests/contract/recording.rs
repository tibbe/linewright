//! Items as GitHub's REST API gives them, and the heads of branches, as a
//! run records its input.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use linewright::domain::Number;
use linewright::git;
use linewright::store::{NewIssue, RecordedItem, Resource};
use serde_json::Value;

use crate::check::{contracts, equal};
use crate::sandbox::{NewCommit, Sandbox};

contracts![
    an_issue_is_recorded_with_its_comments_parent_and_blockers,
    an_issue_without_a_parent_is_recorded_without_one,
    a_pull_request_is_recorded_with_its_own_resource,
    the_heads_are_of_the_branches_the_repository_has,
    the_commits_are_fetched_as_branches_with_their_history_alone,
    a_commit_the_repository_lacks_fails_the_fetch_naming_it_and_fetches_nothing,
];

/// Opens an issue, as Linewright, titled for `what`.
async fn open(sandbox: &Sandbox, what: &str) -> Result<Number> {
    let number = sandbox
        .store()
        .create_issue(NewIssue {
            title: sandbox.unique(what),
            body: format!("Opened to record {what}."),
            labels: vec![],
        })
        .await?;
    sandbox.made_issue(number);
    Ok(number)
}

/// The one item `number` recorded.
async fn recorded(sandbox: &Sandbox, number: Number) -> Result<RecordedItem> {
    let items = sandbox
        .store()
        .record_items(&BTreeSet::from([number]))
        .await?;
    match <[RecordedItem; 1]>::try_from(items) {
        Ok([item]) => Ok(item),
        Err(items) => anyhow::bail!("recorded {items:?}, not #{number} alone"),
    }
}

/// The item's `resource`.
fn at(item: &RecordedItem, resource: Resource) -> Result<&Value> {
    item.resource(resource)
        .with_context(|| format!("no {resource:?} among {:?}", item.resources.keys()))
}

/// The `number` of each issue listed as the item's `resource`.
fn numbers_at(item: &RecordedItem, resource: Resource) -> Result<Vec<u64>> {
    at(item, resource)?
        .as_array()
        .with_context(|| format!("{resource:?} is not a list"))?
        .iter()
        .map(|listed| listed["number"].as_u64().context("a listed issue's number"))
        .collect()
}

async fn an_issue_is_recorded_with_its_comments_parent_and_blockers(
    sandbox: Sandbox,
) -> Result<()> {
    let parent = open(&sandbox, "recorded-parent").await?;
    let issue = open(&sandbox, "recorded").await?;
    let blocker = open(&sandbox, "recorded-blocker").await?;
    sandbox.store().add_sub_issue(parent, issue).await?;
    sandbox.store().add_blocked_by(issue, blocker).await?;
    sandbox
        .store()
        .comment(issue, "A contract's comment.")
        .await?;
    let linewright = sandbox.store().identity().await?.login;
    let n = issue.0;

    sandbox
        .eventually(async || {
            let item = recorded(&sandbox, issue).await?;
            equal("the number", &item.number, &issue)?;
            equal(
                "the paths",
                &item.resources.keys().cloned().collect::<Vec<_>>(),
                &vec![
                    format!("issues/{n}"),
                    format!("issues/{n}/comments"),
                    format!("issues/{n}/dependencies/blocked_by"),
                    format!("issues/{n}/parent"),
                ],
            )?;
            let read = at(&item, Resource::Issue)?;
            equal("the number", &read["number"].as_u64(), &Some(n))?;
            equal(
                "the title",
                &read["title"].as_str(),
                &Some(sandbox.unique("recorded").as_str()),
            )?;
            equal(
                "the body",
                &read["body"].as_str(),
                &Some("Opened to record recorded."),
            )?;
            equal(
                "the author",
                &read["user"]["login"].as_str(),
                &Some(linewright.as_str()),
            )?;
            equal("the state", &read["state"].as_str(), &Some("open"))?;
            let comments = at(&item, Resource::Comments)?;
            equal(
                "the comments' bodies",
                &comments[0]["body"].as_str(),
                &Some("A contract's comment."),
            )?;
            equal(
                "the comments' authors",
                &comments[0]["user"]["login"].as_str(),
                &Some(linewright.as_str()),
            )?;
            equal(
                "the parent",
                &at(&item, Resource::Parent)?["number"].as_u64(),
                &Some(parent.0),
            )?;
            equal(
                "the blockers",
                &numbers_at(&item, Resource::BlockedBy)?,
                &vec![blocker.0],
            )
        })
        .await
}

async fn an_issue_without_a_parent_is_recorded_without_one(sandbox: Sandbox) -> Result<()> {
    let issue = open(&sandbox, "parentless").await?;
    let n = issue.0;

    let item = recorded(&sandbox, issue).await?;

    equal(
        "the paths",
        &item.resources.keys().cloned().collect::<Vec<_>>(),
        &vec![
            format!("issues/{n}"),
            format!("issues/{n}/comments"),
            format!("issues/{n}/dependencies/blocked_by"),
        ],
    )?;
    equal(
        "the comments",
        at(&item, Resource::Comments)?,
        &Value::Array(vec![]),
    )
}

async fn a_pull_request_is_recorded_with_its_own_resource(sandbox: Sandbox) -> Result<()> {
    let branch = sandbox
        .push_branch(
            "recorded-pull",
            &[NewCommit {
                message: "Add a file",
                files: &[("recorded.txt", b"recorded\n")],
            }],
        )
        .await?;
    let head = branch.name.clone();
    let opened = sandbox.open_pull(branch, "recorded-pull").await?;
    let default_branch = sandbox.default_branch().await?;
    let n = opened.pr.0;

    let item = recorded(&sandbox, opened.pr).await?;

    let issue = at(&item, Resource::Issue)?;
    equal("the number", &issue["number"].as_u64(), &Some(n))?;
    equal(
        "whether the issues API calls it a pull request",
        &issue.get("pull_request").is_some(),
        &true,
    )?;
    let pull = at(&item, Resource::Pull)?;
    equal(
        "the pull request's number",
        &pull["number"].as_u64(),
        &Some(n),
    )?;
    equal(
        "the head",
        &pull["head"]["ref"].as_str(),
        &Some(head.as_str()),
    )?;
    equal(
        "the base",
        &pull["base"]["ref"].as_str(),
        &Some(default_branch.as_str()),
    )?;
    equal(
        "the paths",
        &item.resources.keys().cloned().collect::<Vec<_>>(),
        &vec![
            format!("issues/{n}"),
            format!("issues/{n}/comments"),
            format!("issues/{n}/dependencies/blocked_by"),
            format!("pulls/{n}"),
        ],
    )
}

async fn the_heads_are_of_the_branches_the_repository_has(sandbox: Sandbox) -> Result<()> {
    let default_branch = sandbox.default_branch().await?;
    let clone = sandbox.clone_default_branch().await?;
    let default_head = git::head(&clone).await?;
    let pushed = sandbox
        .push_branch(
            "heads",
            &[NewCommit {
                message: "Add a file",
                files: &[("heads.txt", b"heads\n")],
            }],
        )
        .await?;
    let missing = sandbox.unique("missing");

    let heads = sandbox
        .store()
        .heads(&[default_branch.clone(), pushed.name.clone(), missing])
        .await?;

    equal(
        "the heads",
        &heads,
        &BTreeMap::from([
            (default_branch, default_head),
            (pushed.name, git::CommitId::from(pushed.head.as_str())),
        ]),
    )
}

/// The branches of the repository at `bare`, with their heads.
async fn branches_of(bare: &Path) -> Result<BTreeMap<String, git::CommitId>> {
    let refs = git::git(
        bare,
        &["for-each-ref", "--format=%(refname)%09%(objectname)"],
    )
    .await?;
    Ok(refs
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(name, head)| (name.to_string(), git::CommitId::from(head)))
        .collect())
}

/// The names of the objects `args` of `git` list in `bare`, one per line,
/// each first on its line.
async fn objects(bare: &Path, args: &[&str]) -> Result<BTreeSet<String>> {
    Ok(git::git(bare, args)
        .await?
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect())
}

async fn the_commits_are_fetched_as_branches_with_their_history_alone(
    sandbox: Sandbox,
) -> Result<()> {
    let default_branch = sandbox.default_branch().await?;
    let default_head = git::head(&sandbox.clone_default_branch().await?).await?;
    let fetched = sandbox
        .push_branch(
            "fetched",
            &[NewCommit {
                message: "Add a fetched file",
                files: &[("fetched.txt", b"fetched\n")],
            }],
        )
        .await?;
    let left = sandbox
        .push_branch(
            "left",
            &[NewCommit {
                message: "Add a file left out",
                files: &[("left.txt", b"left\n")],
            }],
        )
        .await?;
    let bare = sandbox.scratch().join("fetched.git");
    let heads = BTreeMap::from([
        (default_branch.clone(), default_head.clone()),
        (
            fetched.name.clone(),
            git::CommitId::from(fetched.head.as_str()),
        ),
    ]);

    sandbox.store().fetch_commits(&bare, &heads).await?;

    equal(
        "whether the repository is bare",
        &git::git(&bare, &["rev-parse", "--is-bare-repository"]).await?,
        &"true\n".to_string(),
    )?;
    equal(
        "the branches",
        &branches_of(&bare).await?,
        &BTreeMap::from([
            (format!("refs/heads/{default_branch}"), default_head),
            (
                format!("refs/heads/{}", fetched.name),
                git::CommitId::from(fetched.head.as_str()),
            ),
        ]),
    )?;
    let history = objects(&bare, &["rev-list", "--objects", "--all"]).await?;
    equal(
        "the history holds the fetched commit's file",
        &git::git(&bare, &["show", &format!("{}:fetched.txt", fetched.head)])
            .await
            .ok(),
        &Some("fetched\n".to_string()),
    )?;
    equal(
        "the commit left out is",
        &history.contains(&left.head),
        &false,
    )?;
    equal(
        "the objects",
        &objects(&bare, &["cat-file", "--batch-all-objects", "--batch-check"]).await?,
        &history,
    )
}

async fn a_commit_the_repository_lacks_fails_the_fetch_naming_it_and_fetches_nothing(
    sandbox: Sandbox,
) -> Result<()> {
    let default_branch = sandbox.default_branch().await?;
    let default_head = git::head(&sandbox.clone_default_branch().await?).await?;
    let lacked = "0123456789abcdef0123456789abcdef01234567";
    let bare = sandbox.scratch().join("lacking.git");
    let heads = BTreeMap::from([
        (default_branch, default_head),
        (sandbox.unique("lacked"), git::CommitId::from(lacked)),
    ]);

    let fetched = sandbox.store().fetch_commits(&bare, &heads).await;

    let error = match fetched {
        Ok(()) => anyhow::bail!("fetched a commit the repository lacks"),
        Err(error) => format!("{error:#}"),
    };
    equal(
        "whether the error names the commit",
        &error.contains(lacked),
        &true,
    )?;
    equal(
        "the branches",
        &branches_of(&bare).await.unwrap_or_default(),
        &BTreeMap::new(),
    )
}
