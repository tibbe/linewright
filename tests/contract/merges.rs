//! Merges, as Linewright merges pull requests, and the merged pull requests
//! and their changes read back.

use anyhow::{Context, Result, bail, ensure};
use linewright::domain::{ClosedKind, MergeMethod, Mergeability, Number, PullState};
use linewright::git;
use linewright::store::{AnyItem, PushMode};

use crate::check::{contracts, equal};
use crate::sandbox::{NewCommit, Opened, Sandbox, commit, recent};

contracts![
    a_pull_request_merged_with_a_merge_commit_has_both_tips_as_parents,
    a_pull_request_of_one_commit_squashed_is_one_commit_titled_after_that_commit,
    a_pull_request_of_several_commits_squashed_is_one_commit_titled_after_it,
    a_pull_request_merged_by_rebase_keeps_its_authors_with_the_merger_as_committer,
    a_pull_request_merged_by_rebase_adds_no_commit_for_changes_already_on_the_default_branch,
    a_pull_request_that_conflicts_is_refused_merging_even_by_a_rebase_that_skips_its_change,
    a_merge_is_refused_once_the_head_has_moved,
    a_draft_is_refused_merging,
    a_merged_pull_request_reads_back_merged_with_its_files,
    a_snapshot_holds_an_issue_a_merge_closed_with_the_pull_request_that_closed_it,
    the_diff_of_a_pull_request_holds_its_changes_to_the_paths_asked,
    the_diff_of_a_merged_pull_request_holds_its_changes_to_the_paths_asked,
];

/// Who GitHub commits a merge commit or a squash as.
const GITHUB: &str = "GitHub";

/// A commit on the default branch, as `git log` gives it.
#[derive(Debug)]
struct Commit {
    id: String,
    parents: Vec<String>,
    author: String,
    author_email: String,
    committer: String,
    subject: String,
    body: String,
}

/// The fields of a [`Commit`], in `git log`'s format
/// (<https://git-scm.com/docs/git-log#_pretty_formats>).
const COMMIT_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%cn%x1f%s%x1f%b";

/// The latest commits on the default branch, newest first: other contracts
/// may merge into it too.
async fn history(sandbox: &Sandbox) -> Result<Vec<Commit>> {
    let clone = sandbox.clone_default_branch().await?;
    let log = git::git(&clone, &["log", "-z", "-n", "200", COMMIT_FORMAT]).await?;
    log.split_terminator('\0')
        .map(|commit| {
            let fields: Vec<&str> = commit.split('\x1f').collect();
            let &[id, parents, author, author_email, committer, subject, body] = &fields[..] else {
                bail!("git log gave an incomplete commit: {commit:?}");
            };
            Ok(Commit {
                id: id.into(),
                parents: parents.split_whitespace().map(String::from).collect(),
                author: author.into(),
                author_email: author_email.into(),
                committer: committer.into(),
                subject: subject.into(),
                body: body.trim_end().into(),
            })
        })
        .collect()
}

/// The commit on the default branch with `subject`.
fn find<'a>(history: &'a [Commit], subject: &str) -> Result<&'a Commit> {
    let mut found = history.iter().filter(|commit| commit.subject == subject);
    match (found.next(), found.next()) {
        (Some(commit), None) => Ok(commit),
        (None, _) => bail!("the default branch has no commit {subject:?}"),
        (Some(_), Some(_)) => bail!("the default branch has more than one commit {subject:?}"),
    }
}

/// Pushes a branch named for `what` with `commits`, and opens a pull
/// request from it ready to merge.
async fn ready(sandbox: &Sandbox, what: &str, commits: &[NewCommit<'_>]) -> Result<Opened> {
    let branch = sandbox.push_branch(what, commits).await?;
    let opened = sandbox.open_pull(branch, what).await?;
    sandbox.store().mark_ready(opened.pr).await?;
    Ok(opened)
}

/// A file of the contract's own, named for `what`.
fn file(sandbox: &Sandbox, what: &str) -> String {
    format!("{}.txt", sandbox.unique(what))
}

/// Merges #`pr`, at `head`, with `method`. GitHub works out whether a pull
/// request merges after it opens or its head moves, and refuses to merge
/// it until then, so it is asked again.
async fn merge(sandbox: &Sandbox, pr: Number, head: &str, method: MergeMethod) -> Result<()> {
    let _merging = sandbox.merging().await;
    sandbox
        .eventually(async || {
            let pull = sandbox.store().pull(pr).await?;
            if matches!(pull.state, PullState::Merged { .. }) {
                return Ok(());
            }
            equal("the head", pull.head_sha.as_str(), head)?;
            sandbox.store().merge(pr, head, method).await
        })
        .await
}

/// Whether the repository doesn't allow `method`, checking then that it
/// refuses a merge with it: a contract on a method it doesn't allow checks
/// only that.
async fn refused_unless_allowed(sandbox: &Sandbox, method: MergeMethod) -> Result<bool> {
    let settings = sandbox.store().settings().await?;
    if settings.allowed_merge_methods.contains(method) {
        return Ok(false);
    }
    let what = format!("disallowed-{method}");
    let path = file(sandbox, &what);
    let Opened { branch, pr } = ready(
        sandbox,
        &what,
        &[NewCommit {
            message: "Add a file",
            files: &[(&path, b"added\n")],
        }],
    )
    .await?;
    let merged = sandbox.store().merge(pr, &branch.head, method).await;
    ensure!(
        merged.is_err(),
        "merged with `{method}`, which the repository doesn't allow"
    );
    Ok(true)
}

/// Moves the default branch on, by merging a pull request named for `what`
/// with a method the repository allows.
async fn move_on(sandbox: &Sandbox, what: &str, files: &[(&str, &[u8])]) -> Result<()> {
    let method = sandbox.merge_method().await?;
    let move_on = NewCommit {
        message: "Move on",
        files,
    };
    let Opened { branch, pr } = ready(sandbox, what, &[move_on]).await?;
    merge(sandbox, pr, &branch.head, method).await
}

/// The contents of `path` on the default branch.
async fn on_default_branch(sandbox: &Sandbox, path: &str) -> Result<Option<String>> {
    let default_branch = sandbox.default_branch().await?;
    sandbox.store().read_file(&default_branch, path).await
}

async fn a_pull_request_merged_with_a_merge_commit_has_both_tips_as_parents(
    sandbox: Sandbox,
) -> Result<()> {
    if refused_unless_allowed(&sandbox, MergeMethod::Merge).await? {
        return Ok(());
    }
    let path = file(&sandbox, "merged");
    let Opened { branch, pr } = ready(
        &sandbox,
        "merge",
        &[NewCommit {
            message: "Add a file",
            files: &[(&path, b"merged\n")],
        }],
    )
    .await?;
    let other = file(&sandbox, "merge-other");
    move_on(&sandbox, "merge-other", &[(&other, b"other\n")]).await?;

    merge(&sandbox, pr, &branch.head, MergeMethod::Merge).await?;

    let history = history(&sandbox).await?;
    let owner = sandbox.repo().owner();
    let merged = find(
        &history,
        &format!("Merge pull request {pr} from {owner}/{}", branch.name),
    )?;
    let identity = sandbox.store().identity().await?;
    equal("the body", &merged.body, &sandbox.unique("merge"))?;
    equal(
        "the second parent",
        &merged.parents.get(1),
        &Some(&branch.head),
    )?;
    equal("the parents", &merged.parents.len(), &2)?;
    equal("the author", &merged.author, &identity.name)?;
    equal("the committer", merged.committer.as_str(), GITHUB)?;
    equal(
        "the file",
        &on_default_branch(&sandbox, &path).await?.as_deref(),
        &Some("merged\n"),
    )
}

/// Squashes a pull request named for `what` with `commits`, on a default
/// branch moved on since, and checks it is one commit by the merger titled
/// `subject` of its number, with the files of the commits.
async fn squashed(
    sandbox: &Sandbox,
    what: &str,
    commits: &[NewCommit<'_>],
    subject: impl Fn(Number) -> String,
) -> Result<()> {
    if refused_unless_allowed(sandbox, MergeMethod::Squash).await? {
        return Ok(());
    }
    let Opened { branch, pr } = ready(sandbox, what, commits).await?;
    move_on(
        sandbox,
        &format!("{what}-other"),
        &[(&file(sandbox, "other"), b"other\n")],
    )
    .await?;

    merge(sandbox, pr, &branch.head, MergeMethod::Squash).await?;

    let history = history(sandbox).await?;
    let squashed = find(&history, &subject(pr))?;
    let identity = sandbox.store().identity().await?;
    equal("the parents", &squashed.parents.len(), &1)?;
    ensure!(
        squashed.parents[0] != branch.head,
        "the squash is on the pull request's commit"
    );
    equal("the author", &squashed.author, &identity.name)?;
    equal("the committer", squashed.committer.as_str(), GITHUB)?;
    for (path, contents) in commits.iter().flat_map(|commit| commit.files) {
        let read = on_default_branch(sandbox, path).await?;
        equal(
            "a file",
            &read.as_deref().map(str::as_bytes),
            &Some(*contents),
        )?;
    }
    Ok(())
}

async fn a_pull_request_of_one_commit_squashed_is_one_commit_titled_after_that_commit(
    sandbox: Sandbox,
) -> Result<()> {
    let path = file(&sandbox, "squashed");
    let message = sandbox.unique("a squashed commit");
    squashed(
        &sandbox,
        "squash-one",
        &[NewCommit {
            message: &message,
            files: &[(&path, b"squashed\n")],
        }],
        |pr| format!("{message} ({pr})"),
    )
    .await
}

async fn a_pull_request_of_several_commits_squashed_is_one_commit_titled_after_it(
    sandbox: Sandbox,
) -> Result<()> {
    let first = file(&sandbox, "first");
    let second = file(&sandbox, "second");
    let title = sandbox.unique("squash-several");
    squashed(
        &sandbox,
        "squash-several",
        &[
            NewCommit {
                message: "Add the first file",
                files: &[(&first, b"first\n")],
            },
            NewCommit {
                message: "Add the second file",
                files: &[(&second, b"second\n")],
            },
        ],
        |pr| format!("{title} ({pr})"),
    )
    .await
}

async fn a_pull_request_merged_by_rebase_keeps_its_authors_with_the_merger_as_committer(
    sandbox: Sandbox,
) -> Result<()> {
    if refused_unless_allowed(&sandbox, MergeMethod::Rebase).await? {
        return Ok(());
    }
    let path = file(&sandbox, "rebased");
    let first = sandbox.unique("add a file to rebase");
    let second = sandbox.unique("change the file rebased");
    let Opened { branch, pr } = ready(
        &sandbox,
        "rebase",
        &[
            NewCommit {
                message: &first,
                files: &[(&path, b"rebased\n")],
            },
            NewCommit {
                message: &second,
                files: &[(&path, b"rebased again\n")],
            },
        ],
    )
    .await?;
    let other = file(&sandbox, "rebase-other");
    move_on(&sandbox, "rebase-other", &[(&other, b"other\n")]).await?;

    merge(&sandbox, pr, &branch.head, MergeMethod::Rebase).await?;

    let history = history(&sandbox).await?;
    let identity = sandbox.store().identity().await?;
    let first = find(&history, &first)?;
    let second = find(&history, &second)?;
    equal(
        "the second's parents",
        &second.parents,
        &vec![first.id.clone()],
    )?;
    ensure!(second.id != branch.head, "the commits were not replayed");
    for commit in [first, second] {
        equal("the author", &commit.author, &identity.name)?;
        equal("the author's email", &commit.author_email, &identity.email)?;
        equal("the committer", &commit.committer, &identity.name)?;
    }
    for merged in [&path, &other] {
        ensure!(
            on_default_branch(&sandbox, merged).await?.is_some(),
            "the default branch has no {merged}"
        );
    }
    Ok(())
}

/// The commit the default branch is at.
async fn default_head(sandbox: &Sandbox) -> Result<String> {
    let history = history(sandbox).await?;
    history
        .first()
        .map(|c| c.id.clone())
        .context("the default branch has no commit")
}

async fn a_pull_request_merged_by_rebase_adds_no_commit_for_changes_already_on_the_default_branch(
    sandbox: Sandbox,
) -> Result<()> {
    if refused_unless_allowed(&sandbox, MergeMethod::Rebase).await? {
        return Ok(());
    }
    let path = file(&sandbox, "upstream");
    let subject = sandbox.unique("do the thing");
    let Opened { branch, pr } = ready(
        &sandbox,
        "upstream",
        &[NewCommit {
            message: &subject,
            files: &[(&path, b"thing\n")],
        }],
    )
    .await?;
    let other = file(&sandbox, "upstream-other");
    move_on(
        &sandbox,
        "upstream-other",
        &[(&path, b"thing\n"), (&other, b"other\n")],
    )
    .await?;

    merge(&sandbox, pr, &branch.head, MergeMethod::Rebase).await?;

    let history = history(&sandbox).await?;
    ensure!(
        history.iter().all(|commit| commit.subject != subject),
        "the default branch has the pull request's commit"
    );
    Ok(())
}

async fn a_pull_request_that_conflicts_is_refused_merging_even_by_a_rebase_that_skips_its_change(
    sandbox: Sandbox,
) -> Result<()> {
    if refused_unless_allowed(&sandbox, MergeMethod::Rebase).await? {
        return Ok(());
    }
    let path = file(&sandbox, "built-on");
    let subject = sandbox.unique("do the thing");
    let Opened { branch, pr } = ready(
        &sandbox,
        "built-on",
        &[NewCommit {
            message: &subject,
            files: &[(&path, b"thing\n")],
        }],
    )
    .await?;
    move_on(&sandbox, "built-on-same", &[(&path, b"thing\n")]).await?;
    move_on(&sandbox, "built-on-more", &[(&path, b"thing\nmore\n")]).await?;
    // GitHub refuses any merge until it has worked out whether the pull
    // request merges, so the refusal checked is the one for its conflict.
    sandbox
        .eventually(async || {
            let pull = sandbox.store().pull(pr).await?;
            equal(
                "whether it merges",
                &pull.mergeability,
                &Mergeability::Conflicting,
            )
        })
        .await?;

    let merged = sandbox
        .store()
        .merge(pr, &branch.head, MergeMethod::Rebase)
        .await;

    ensure!(merged.is_err(), "merged a pull request that conflicts");
    let pull = sandbox.store().pull(pr).await?;
    equal("the state", &pull.state, &PullState::Open { draft: false })?;
    let read = on_default_branch(&sandbox, &path).await?;
    equal("the file", &read.as_deref(), &Some("thing\nmore\n"))
}

async fn a_merge_is_refused_once_the_head_has_moved(sandbox: Sandbox) -> Result<()> {
    let path = file(&sandbox, "moved");
    let Opened { branch, pr } = ready(
        &sandbox,
        "moved",
        &[NewCommit {
            message: "Add a file",
            files: &[(&path, b"added\n")],
        }],
    )
    .await?;
    let clone = sandbox.clone(&branch.name).await?;
    let head = commit(
        &clone,
        &[NewCommit {
            message: "Change it",
            files: &[(&path, b"changed\n")],
        }],
    )
    .await?;
    let expected = branch.head.clone();
    sandbox
        .push(&clone, &branch.name, PushMode::Lease { expected })
        .await?;
    // GitHub moves the head moments after the push.
    sandbox
        .eventually(async || {
            let pull = sandbox.store().pull(pr).await?;
            equal("the head", &pull.head_sha, &head)
        })
        .await?;
    let before = default_head(&sandbox).await?;

    let method = sandbox.merge_method().await?;
    let merged = sandbox.store().merge(pr, &branch.head, method).await;

    ensure!(merged.is_err(), "merged at a head it had moved from");
    let pull = sandbox.store().pull(pr).await?;
    equal("the state", &pull.state, &PullState::Open { draft: false })?;
    ensure!(
        on_default_branch(&sandbox, &path).await?.is_none(),
        "the default branch has the change, at {before} or since"
    );
    Ok(())
}

async fn a_draft_is_refused_merging(sandbox: Sandbox) -> Result<()> {
    let path = file(&sandbox, "draft");
    let branch = sandbox
        .push_branch(
            "draft",
            &[NewCommit {
                message: "Add a file",
                files: &[(&path, b"added\n")],
            }],
        )
        .await?;
    let Opened { branch, pr } = sandbox.open_pull(branch, "draft").await?;

    let method = sandbox.merge_method().await?;
    let merged = sandbox.store().merge(pr, &branch.head, method).await;

    ensure!(merged.is_err(), "merged a draft");
    let pull = sandbox.store().pull(pr).await?;
    equal("the state", &pull.state, &PullState::Open { draft: true })
}

async fn a_merged_pull_request_reads_back_merged_with_its_files(sandbox: Sandbox) -> Result<()> {
    let first = file(&sandbox, "merged-first");
    let second = file(&sandbox, "merged-second");
    let Opened { branch, pr } = ready(
        &sandbox,
        "merged",
        &[NewCommit {
            message: "Add two files",
            files: &[(&first, b"first\n"), (&second, b"second\n")],
        }],
    )
    .await?;
    let method = sandbox.merge_method().await?;

    merge(&sandbox, pr, &branch.head, method).await?;

    let identity = sandbox.store().identity().await?;
    let default_branch = sandbox.default_branch().await?;
    let merged = sandbox
        .eventually(async || {
            sandbox
                .store()
                .merged_pull(pr, recent())
                .await?
                .with_context(|| format!("{pr} is not read merged"))
        })
        .await?;
    equal("the number", &merged.number, &pr)?;
    equal("the title", &merged.title, &sandbox.unique("merged"))?;
    equal("the author", &merged.author, &identity.login)?;
    equal("the base", &merged.base_ref, &default_branch)?;
    equal("the issues it closes", &merged.closes, &vec![])?;
    let mut files = merged.files.clone();
    files.sort();
    let mut expected = vec![first, second];
    expected.sort();
    equal("the files", &files, &expected)?;
    ensure!(
        merged.merged_at <= merged.updated_at,
        "updated before it merged"
    );
    let pull = sandbox.store().pull(pr).await?;
    equal(
        "the state",
        &pull.state,
        &PullState::Merged {
            at: merged.merged_at,
        },
    )?;
    match sandbox.store().any_item(pr).await? {
        Some(AnyItem::Merged {
            pull: any,
            tracking_issues,
        }) => {
            equal("any item", &*any, &merged)?;
            equal("the tracking issues", &tracking_issues, &vec![])?;
        }
        other => bail!("any item is {other:?}"),
    }
    // A snapshot holds it among the merged pull requests only, as it closes
    // no ticket.
    let read = sandbox.store().linewright_pull(pr).await?;
    equal("Linewright's pull request", &read, &None)
}

async fn a_snapshot_holds_an_issue_a_merge_closed_with_the_pull_request_that_closed_it(
    sandbox: Sandbox,
) -> Result<()> {
    let (issue, Opened { branch, pr }) = sandbox.open_pull_closing_an_issue("merge-closes").await?;
    sandbox.store().mark_ready(pr).await?;
    // A merge before GitHub links the issue closes nothing.
    sandbox.linked(pr, issue).await?;
    let method = sandbox.merge_method().await?;

    merge(&sandbox, pr, &branch.head, method).await?;

    sandbox
        .eventually(async || {
            let snapshot = sandbox.store().snapshot(recent()).await?;
            let closed = snapshot
                .closed
                .iter()
                .find(|item| item.number == issue)
                .with_context(|| format!("the snapshot does not hold {issue} closed"))?;
            match &closed.kind {
                ClosedKind::Issue { merged_by, .. } => {
                    equal("the merge that closed it", merged_by, &Some(pr))
                }
                ClosedKind::Pull { .. } => bail!("{issue} is held as a pull request"),
            }
        })
        .await
}

/// Checks that `diff` holds the change adding `added`, which holds `line`,
/// and nothing of `left_out`.
fn holds_change(diff: &str, added: &str, line: &str, left_out: &str) -> Result<()> {
    ensure!(
        diff.contains(&format!("diff --git a/{added} b/{added}\n")),
        "the diff names no {added}: {diff}"
    );
    ensure!(
        diff.lines().any(|l| l == format!("+{line}")),
        "the diff adds no {line:?}: {diff}"
    );
    ensure!(
        !diff.contains(left_out),
        "the diff holds {left_out}: {diff}"
    );
    Ok(())
}

/// A pull request adding two files, one whose changes a diff asks for and
/// one it doesn't.
struct TwoFiles {
    opened: Opened,
    asked: String,
    unasked: String,
}

/// Opens a pull request, named for `what`, adding two files.
async fn two_files(sandbox: &Sandbox, what: &str) -> Result<TwoFiles> {
    let asked = file(sandbox, &format!("{what}-asked"));
    let unasked = file(sandbox, &format!("{what}-unasked"));
    let opened = ready(
        sandbox,
        what,
        &[NewCommit {
            message: "Add two files",
            files: &[(&asked, b"asked\n"), (&unasked, b"unasked\n")],
        }],
    )
    .await?;
    Ok(TwoFiles {
        opened,
        asked,
        unasked,
    })
}

/// Checks that the diff of `files`' pull request holds its changes to the
/// paths asked, and none to others.
async fn diffs(sandbox: &Sandbox, files: &TwoFiles) -> Result<()> {
    let store = sandbox.store();
    let pr = files.opened.pr;
    let diff = store.diff(pr, std::slice::from_ref(&files.asked)).await?;
    holds_change(&diff, &files.asked, "asked", &files.unasked)?;
    let none = store.diff(pr, &[]).await?;
    equal("the diff of no paths", none.as_str(), "")
}

async fn the_diff_of_a_pull_request_holds_its_changes_to_the_paths_asked(
    sandbox: Sandbox,
) -> Result<()> {
    let files = two_files(&sandbox, "diff").await?;

    diffs(&sandbox, &files).await
}

async fn the_diff_of_a_merged_pull_request_holds_its_changes_to_the_paths_asked(
    sandbox: Sandbox,
) -> Result<()> {
    let files = two_files(&sandbox, "merged-diff").await?;
    let Opened { branch, pr } = &files.opened;
    let method = sandbox.merge_method().await?;
    merge(&sandbox, *pr, &branch.head, method).await?;
    move_on(
        &sandbox,
        "merged-diff-other",
        &[(&file(&sandbox, "other"), b"other\n")],
    )
    .await?;

    diffs(&sandbox, &files).await
}
