use std::path::Path;

use anyhow::{Context, Result, bail};
use tokio::process::Command;

use crate::domain::{UpdateMethod, UpdateRule};

pub type GitEnv = [(String, String)];

pub async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    git_with(dir, args, &[]).await
}

pub async fn git_with(dir: &Path, args: &[&str], env: &GitEnv) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub async fn clone(url: &str, dir: &Path, branch: &str, env: &GitEnv) -> Result<()> {
    let parent = dir.parent().context("a clone directory has a parent")?;
    tokio::fs::create_dir_all(parent).await?;
    let target = dir.to_str().context("clone paths are UTF-8")?;
    git_with(
        parent,
        &[
            "clone",
            "--quiet",
            "--no-tags",
            "--branch",
            branch,
            url,
            target,
        ],
        env,
    )
    .await?;
    Ok(())
}

pub async fn set_identity(dir: &Path, name: &str, email: &str) -> Result<()> {
    git(dir, &["config", "user.name", name]).await?;
    git(dir, &["config", "user.email", email]).await?;
    Ok(())
}

pub async fn head(dir: &Path) -> Result<String> {
    Ok(git(dir, &["rev-parse", "HEAD"]).await?.trim().to_string())
}

pub async fn create_branch(dir: &Path, branch: &str) -> Result<()> {
    git(dir, &["checkout", "--quiet", "-B", branch]).await?;
    Ok(())
}

/// What a branch [`check_out_branch`] checked out starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchStart {
    /// The branch as the clone's remote has it, with the `unfinished`
    /// commits it holds.
    Existing {
        unfinished: Option<UnfinishedCommits>,
    },
    /// A new branch at the commit checked out before.
    New,
}

/// Checks out `branch` as the remote had it when cloned, or as a new branch
/// at `HEAD` when the remote had no such branch. `base` is what the
/// branch's unfinished commits are counted past.
pub async fn check_out_branch(dir: &Path, branch: &str, base: &str) -> Result<BranchStart> {
    let remote = format!("refs/remotes/origin/{branch}");
    if git(dir, &["rev-parse", "--verify", "--quiet", &remote])
        .await
        .is_ok()
    {
        git(dir, &["checkout", "--quiet", "-B", branch, &remote]).await?;
        let unfinished = unfinished_commits(dir, base).await?;
        Ok(BranchStart::Existing { unfinished })
    } else {
        create_branch(dir, branch).await?;
        Ok(BranchStart::New)
    }
}

pub enum Updated {
    UpToDate,
    Updated,
    Conflict(Conflict),
}

/// A rebase or merge of the default branch that stopped on conflicts in
/// `files` and is left in progress.
pub struct Conflict {
    pub method: UpdateMethod,
    pub files: Vec<String>,
}

/// Brings the checked-out branch up to date with `origin/<default_branch>`,
/// as fresh as the clone, with the method `rule` picks.
pub async fn update(dir: &Path, default_branch: &str, rule: UpdateRule) -> Result<Updated> {
    let upstream = format!("origin/{default_branch}");
    if git(dir, &["merge-base", "--is-ancestor", &upstream, "HEAD"])
        .await
        .is_ok()
    {
        return Ok(Updated::UpToDate);
    }
    match rule {
        UpdateRule::Always(method) => update_with(dir, &upstream, method).await,
        UpdateRule::RebaseOnConflict => {
            match update_with(dir, &upstream, UpdateMethod::Merge).await? {
                Updated::Conflict(_) => {
                    git(dir, &["merge", "--abort"]).await?;
                    update_with(dir, &upstream, UpdateMethod::Rebase).await
                }
                updated => Ok(updated),
            }
        }
    }
}

/// Rebases the checked-out branch onto `origin/<default_branch>`, as fresh
/// as the clone, even when it already contains it: replaying the branch's
/// commits drops the merge commits on it. A branch with none that contains
/// it is left as it is.
pub async fn rebase(dir: &Path, default_branch: &str) -> Result<Updated> {
    update_with(
        dir,
        &format!("origin/{default_branch}"),
        UpdateMethod::Rebase,
    )
    .await
}

async fn update_with(dir: &Path, upstream: &str, method: UpdateMethod) -> Result<Updated> {
    let attempt = match method {
        UpdateMethod::Rebase => git(dir, &["rebase", "--quiet", upstream]).await,
        UpdateMethod::Merge => git(dir, &["merge", "--quiet", "--no-edit", upstream]).await,
    };
    match attempt {
        Ok(_) => Ok(Updated::Updated),
        Err(error) => {
            let files: Vec<String> = git(dir, &["diff", "--name-only", "--diff-filter=U"])
                .await?
                .lines()
                .map(str::to_string)
                .collect();
            if files.is_empty() {
                Err(error)
            } else {
                Ok(Updated::Conflict(Conflict { method, files }))
            }
        }
    }
}

pub async fn changed_files(dir: &Path, base: &str) -> Result<Vec<String>> {
    Ok(
        git(dir, &["diff", "--name-only", &format!("{base}...HEAD")])
            .await?
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

pub async fn commits_ahead(dir: &Path, base: &str) -> Result<usize> {
    Ok(git(dir, &["rev-list", "--count", &format!("{base}..HEAD")])
        .await?
        .trim()
        .parse()?)
}

/// A rebase or merge left in progress.
#[derive(Clone, Copy)]
enum InProgress {
    Rebase,
    Merge,
}

impl InProgress {
    fn describe(self) -> &'static str {
        match self {
            InProgress::Rebase => "a rebase is in progress",
            InProgress::Merge => "a merge is in progress",
        }
    }

    async fn abort(self, dir: &Path) -> Result<()> {
        let command = match self {
            InProgress::Rebase => "rebase",
            InProgress::Merge => "merge",
        };
        git(dir, &[command, "--abort"]).await?;
        Ok(())
    }
}

async fn in_progress(dir: &Path) -> Result<Option<InProgress>> {
    for (path, what) in [
        ("rebase-merge", InProgress::Rebase),
        ("rebase-apply", InProgress::Rebase),
        ("MERGE_HEAD", InProgress::Merge),
    ] {
        let location = git(dir, &["rev-parse", "--git-path", path]).await?;
        if dir.join(location.trim()).exists() {
            return Ok(Some(what));
        }
    }
    Ok(None)
}

/// What keeps the work in `dir` from being finished: a rebase or merge in
/// progress, or uncommitted changes to tracked files.
pub async fn loose_ends(dir: &Path) -> Result<Option<String>> {
    if let Some(operation) = in_progress(dir).await? {
        return Ok(Some(format!("{}; finish it.", operation.describe())));
    }
    let status = git(dir, &["status", "--porcelain", "--untracked-files=no"]).await?;
    if !status.trim().is_empty() {
        return Ok(Some(format!(
            "there are uncommitted changes; commit them:\n\n```\n{}\n```",
            status.trim_end()
        )));
    }
    Ok(None)
}

/// The trailer that marks an unfinished commit.
const UNFINISHED_TRAILER: &str = "Linewright-Unfinished";

/// Commits all that is uncommitted in `dir`, untracked files included and
/// ignored ones not, as one unfinished commit, after aborting a rebase or
/// merge in progress. The commit amends `HEAD` when that is an unfinished
/// commit already, so the work a run leaves stays in one.
pub async fn commit_unfinished(dir: &Path) -> Result<()> {
    if let Some(operation) = in_progress(dir).await? {
        operation.abort(dir).await?;
    }
    git(dir, &["add", "--all"]).await?;
    if git(dir, &["diff", "--cached", "--quiet"]).await.is_ok() {
        return Ok(());
    }
    let amend = is_unfinished(dir, "HEAD").await?;
    let trailer = format!("{UNFINISHED_TRAILER}: true");
    let mut args = vec![
        "commit",
        "--quiet",
        "--no-verify",
        "-m",
        "Unfinished work, kept when its run stopped",
        "-m",
        "Linewright committed what the session left uncommitted when its run \
         stopped. The next run folds it into its own commits.",
        "-m",
        &trailer,
    ];
    if amend {
        args.push("--amend");
    }
    git(dir, &args).await?;
    Ok(())
}

async fn is_unfinished(dir: &Path, commit: &str) -> Result<bool> {
    let format = format!("--format=%(trailers:key={UNFINISHED_TRAILER},valueonly)");
    Ok(git(dir, &["log", "-1", &format, commit])
        .await?
        .lines()
        .any(|line| !line.trim().is_empty()))
}

/// The abbreviated names of one or more unfinished commits, oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnfinishedCommits {
    pub first: String,
    pub rest: Vec<String>,
}

/// The unfinished commits on `HEAD` past `base`, if any.
pub async fn unfinished_commits(dir: &Path, base: &str) -> Result<Option<UnfinishedCommits>> {
    let format =
        format!("--format=%h %(trailers:key={UNFINISHED_TRAILER},valueonly,separator=%x2C)");
    let mut commits = git(
        dir,
        &["log", "--reverse", &format, &format!("{base}..HEAD")],
    )
    .await?
    .lines()
    .filter_map(|line| {
        let (commit, marked) = line.split_once(' ')?;
        (!marked.trim().is_empty()).then(|| commit.to_string())
    })
    .collect::<Vec<_>>()
    .into_iter();
    Ok(commits.next().map(|first| UnfinishedCommits {
        first,
        rest: commits.collect(),
    }))
}
