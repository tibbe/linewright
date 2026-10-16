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

pub async fn unfinished(dir: &Path) -> Result<Option<String>> {
    for (path, what) in [
        ("rebase-merge", "a rebase is in progress"),
        ("rebase-apply", "a rebase is in progress"),
        ("MERGE_HEAD", "a merge is in progress"),
    ] {
        let location = git(dir, &["rev-parse", "--git-path", path]).await?;
        if dir.join(location.trim()).exists() {
            return Ok(Some(format!("{what}; finish it.")));
        }
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
