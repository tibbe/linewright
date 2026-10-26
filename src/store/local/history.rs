//! The only writer of the local store's history: every commit it makes to its
//! repository and every ref it moves there.

use std::path::Path;

use anyhow::{Context, Result, bail};

use super::url;
use crate::domain::{MergeMethod, Number};
use crate::git;

/// A commit to write on `branch`, which starts from the default branch when
/// it doesn't exist yet.
pub struct NewCommit<'a> {
    pub branch: &'a str,
    pub default_branch: &'a str,
    pub author: &'a str,
    pub message: &'a str,
    pub files: &'a [(&'a str, &'a str)],
}

/// Writes `commit` and moves its branch to it, returning the commit.
pub async fn write_commit(repo: &Path, commit: NewCommit<'_>) -> Result<String> {
    let NewCommit {
        branch,
        default_branch,
        author,
        message,
        files,
    } = commit;
    let work = tempfile::tempdir()?;
    let dir = work.path().join("clone");
    let url = url(repo)?;
    let empty = git::git(repo, &["rev-parse", "--verify", "--quiet", default_branch])
        .await
        .is_err();
    if empty {
        git::git(work.path(), &["init", "--quiet", "-b", branch, "clone"]).await?;
        git::git(&dir, &["remote", "add", "origin", url]).await?;
    } else {
        let has_branch = git::git(repo, &["rev-parse", "--verify", "--quiet", branch])
            .await
            .is_ok();
        git::clone(
            url,
            &dir,
            if has_branch { branch } else { default_branch },
            &[],
        )
        .await?;
        git::create_branch(&dir, branch).await?;
    }
    git::set_identity(&dir, author, &format!("{author}@example.com")).await?;
    for (path, contents) in files {
        let path = dir.join(path);
        tokio::fs::create_dir_all(path.parent().expect("files live in a directory")).await?;
        tokio::fs::write(&path, contents).await?;
    }
    git::git(&dir, &["add", "--all"]).await?;
    git::git(&dir, &["commit", "--quiet", "-m", message]).await?;
    git::git(
        &dir,
        &["push", "--quiet", "origin", &format!("HEAD:{branch}")],
    )
    .await?;
    git::head(&dir).await
}

/// A merge of pull request `pr`, from branch `head` into branch `into` at
/// `before`, by `by`.
pub struct MergeOf<'a> {
    pub into: &'a str,
    pub before: &'a str,
    pub head: &'a str,
    pub method: MergeMethod,
    pub by: &'a str,
    pub title: &'a str,
    pub pr: Number,
}

/// Merges as GitHub merges
/// (<https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/about-merge-methods-on-github>),
/// returning the new tip of the branch merged into.
pub async fn merge(repo: &Path, merge: MergeOf<'_>) -> Result<String> {
    let MergeOf {
        into,
        before,
        head,
        method,
        by,
        title,
        pr,
    } = merge;
    let remote_head = format!("origin/{head}");
    match method {
        MergeMethod::Merge => {
            let message = format!("Merge pull request {pr} from {head}");
            merge_in_clone(
                repo,
                into,
                by,
                &[&["merge", "--quiet", "--no-ff", "-m", &message, &remote_head]],
            )
            .await
        }
        MergeMethod::Squash => {
            let message = format!("{title} ({pr})");
            merge_in_clone(
                repo,
                into,
                by,
                &[
                    &["merge", "--quiet", "--squash", &remote_head],
                    &["commit", "--quiet", "-m", &message],
                ],
            )
            .await
        }
        MergeMethod::Rebase => {
            let Replayed::Onto(after) = replay(repo, into, head, by).await? else {
                bail!("the commits of {head} don't rebase onto {into} without conflicts");
            };
            move_ref(repo, into, &after, before).await?;
            Ok(after)
        }
    }
}

/// Runs `commands` in a clone of `into` with `by` as its identity, and
/// pushes what they leave checked out to `into`, returning it.
async fn merge_in_clone(repo: &Path, into: &str, by: &str, commands: &[&[&str]]) -> Result<String> {
    let work = tempfile::tempdir()?;
    let dir = work.path().join("clone");
    git::clone(url(repo)?, &dir, into, &[]).await?;
    git::set_identity(&dir, by, &format!("{by}@example.com")).await?;
    for command in commands {
        git::git(&dir, command).await?;
    }
    git::git(
        &dir,
        &["push", "--quiet", "origin", &format!("HEAD:{into}")],
    )
    .await?;
    git::head(&dir).await
}

/// Where replaying a branch's commits ended.
pub enum Replayed {
    /// On this commit, the last one replayed.
    Onto(String),
    Conflict,
}

/// Replays the commits of `head` onto `onto`, as `git rebase` replays them,
/// with `committer` as their committer. The commits it writes are in `repo`,
/// and it moves no ref there.
pub async fn replay(repo: &Path, onto: &str, head: &str, committer: &str) -> Result<Replayed> {
    let work = tempfile::tempdir()?;
    let dir = work.path().join("clone");
    let target = dir.to_str().context("clone paths are UTF-8")?;
    let name = format!("user.name={committer}");
    let email = format!("user.email={committer}@example.com");
    git::git(
        work.path(),
        &[
            "clone",
            "--quiet",
            "--no-tags",
            "--branch",
            head,
            "--config",
            &name,
            "--config",
            &email,
            url(repo)?,
            target,
        ],
    )
    .await?;
    // The clone holds only objects `repo` has, so the rebase can read from
    // and write to `repo`'s own.
    let objects = repo.join("objects");
    let objects = objects
        .to_str()
        .context("the local store's path is UTF-8")?;
    let env = [("GIT_OBJECT_DIRECTORY".to_string(), objects.to_string())];
    let rebased = git::git_with(
        &dir,
        &["rebase", "--quiet", &format!("origin/{onto}")],
        &env,
    )
    .await;
    match rebased {
        Ok(_) => Ok(Replayed::Onto(git::head(&dir).await?)),
        // A rebase that stops on a conflict leaves its state behind.
        Err(_) if dir.join(".git/rebase-merge").exists() => Ok(Replayed::Conflict),
        Err(error) => Err(error),
    }
}

/// Moves `branch` to `new`, provided it is at `old`.
pub async fn move_ref(repo: &Path, branch: &str, new: &str, old: &str) -> Result<()> {
    let refname = format!("refs/heads/{branch}");
    git::git(repo, &["update-ref", &refname, new, old]).await?;
    Ok(())
}

/// Deletes `branch`, which succeeds when there is no such branch.
pub async fn delete_branch(repo: &Path, branch: &str) -> Result<()> {
    let refname = format!("refs/heads/{branch}");
    git::git(repo, &["update-ref", "-d", &refname]).await?;
    Ok(())
}

/// Fetches all of `bundle`'s refs under `refs/case/` and moves
/// `default_branch` to `base`, wherever it was.
pub async fn import_bundle(
    repo: &Path,
    bundle: &Path,
    default_branch: &str,
    base: &str,
) -> Result<()> {
    let bundle = bundle.to_str().context("bundle paths are UTF-8")?;
    git::git(repo, &["fetch", "--quiet", bundle, "refs/*:refs/case/*"]).await?;
    let refname = format!("refs/heads/{default_branch}");
    git::git(repo, &["update-ref", &refname, base]).await?;
    Ok(())
}
