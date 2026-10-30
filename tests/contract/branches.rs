//! Branches, as Linewright clones, pushes and deletes them.

use anyhow::{Result, bail, ensure};
use linewright::git;
use linewright::store::PushMode;

use crate::check::{contracts, equal};
use crate::sandbox::{NewCommit, Sandbox, commit};

contracts![
    a_clone_is_of_the_branch_asked_with_the_settings_given,
    a_replacing_push_moves_the_branch_to_a_commit_not_after_its_head,
    a_push_with_a_lease_moves_the_branch_from_the_commit_expected,
    a_push_with_a_stale_lease_is_refused_and_leaves_the_branch,
    a_deleted_branch_is_gone,
];

async fn a_clone_is_of_the_branch_asked_with_the_settings_given(sandbox: Sandbox) -> Result<()> {
    let pushed = sandbox
        .push_branch(
            "clone",
            &[NewCommit {
                message: "Add a file",
                files: &[("cloned.txt", b"cloned\n")],
            }],
        )
        .await?;
    let clone = sandbox.clone_dir();

    sandbox
        .store()
        .clone_repo(&clone, &pushed.name, &[("user.name", "Cloner")])
        .await?;

    equal("the head", &git::head(&clone).await?, &pushed.head)?;
    let branch = git::git(&clone, &["branch", "--show-current"]).await?;
    equal("the branch checked out", branch.trim(), &pushed.name)?;
    let contents = tokio::fs::read_to_string(clone.join("cloned.txt")).await?;
    equal("the file", contents.as_str(), "cloned\n")?;
    let name = git::git(&clone, &["config", "--local", "user.name"]).await?;
    equal("the name set", name.trim(), "Cloner")
}

/// Checks that the file `replaced.txt` at the head of `branch` holds
/// `expected`, or that there is none when `expected` is `None`.
async fn replaced_reads(sandbox: &Sandbox, branch: &str, expected: Option<&str>) -> Result<()> {
    let read = sandbox.store().read_file(branch, "replaced.txt").await?;
    equal("the file", &read.as_deref(), &expected)
}

async fn a_replacing_push_moves_the_branch_to_a_commit_not_after_its_head(
    sandbox: Sandbox,
) -> Result<()> {
    let pushed = sandbox
        .push_branch(
            "replace",
            &[NewCommit {
                message: "Add a file",
                files: &[("replaced.txt", b"before\n")],
            }],
        )
        .await?;
    let elsewhere = sandbox.clone_default_branch().await?;
    commit(
        &elsewhere,
        &[NewCommit {
            message: "Add it again",
            files: &[("other.txt", b"after\n")],
        }],
    )
    .await?;

    sandbox
        .push(&elsewhere, &pushed.name, PushMode::Replace)
        .await?;

    replaced_reads(&sandbox, &pushed.name, None).await?;
    let read = sandbox.store().read_file(&pushed.name, "other.txt").await?;
    equal("the file", &read.as_deref(), &Some("after\n"))
}

async fn a_push_with_a_lease_moves_the_branch_from_the_commit_expected(
    sandbox: Sandbox,
) -> Result<()> {
    let pushed = sandbox
        .push_branch(
            "lease",
            &[NewCommit {
                message: "Add a file",
                files: &[("replaced.txt", b"before\n")],
            }],
        )
        .await?;
    let clone = sandbox.clone(&pushed.name).await?;
    commit(
        &clone,
        &[NewCommit {
            message: "Change it",
            files: &[("replaced.txt", b"after\n")],
        }],
    )
    .await?;

    let expected = pushed.head.clone();
    sandbox
        .push(&clone, &pushed.name, PushMode::Lease { expected })
        .await?;

    replaced_reads(&sandbox, &pushed.name, Some("after\n")).await
}

async fn a_push_with_a_stale_lease_is_refused_and_leaves_the_branch(
    sandbox: Sandbox,
) -> Result<()> {
    let pushed = sandbox
        .push_branch(
            "stale",
            &[NewCommit {
                message: "Add a file",
                files: &[("replaced.txt", b"before\n")],
            }],
        )
        .await?;
    let stale = sandbox.clone(&pushed.name).await?;
    commit(
        &stale,
        &[NewCommit {
            message: "Change it here",
            files: &[("replaced.txt", b"stale\n")],
        }],
    )
    .await?;
    let moved = sandbox.clone(&pushed.name).await?;
    let head = commit(
        &moved,
        &[NewCommit {
            message: "Change it there",
            files: &[("replaced.txt", b"moved\n")],
        }],
    )
    .await?;
    sandbox
        .push(&moved, &pushed.name, PushMode::Replace)
        .await?;

    let expected = pushed.head.clone();
    let refused = sandbox
        .store()
        .push(&stale, &pushed.name, PushMode::Lease { expected })
        .await;

    if let Ok(()) = refused {
        bail!("the push was not refused");
    }
    replaced_reads(&sandbox, &pushed.name, Some("moved\n")).await?;
    let clone = sandbox.clone(&pushed.name).await?;
    equal("the head", &git::head(&clone).await?, &head)
}

async fn a_deleted_branch_is_gone(sandbox: Sandbox) -> Result<()> {
    let pushed = sandbox
        .push_branch(
            "delete",
            &[NewCommit {
                message: "Add a file",
                files: &[("deleted/file.txt", b"gone\n")],
            }],
        )
        .await?;

    sandbox.store().delete_branch(&pushed.name).await?;

    // GitHub may answer a read with the branch it just deleted.
    sandbox
        .eventually(async || {
            let read = sandbox
                .store()
                .read_file(&pushed.name, "deleted/file.txt")
                .await?;
            equal("the file", &read, &None)?;
            let listed = sandbox.store().list_files(&pushed.name, "deleted").await?;
            equal("the files listed", &listed, &vec![])
        })
        .await?;
    let clone = sandbox.clone_dir();
    let cloned = sandbox.store().clone_repo(&clone, &pushed.name, &[]).await;
    ensure!(cloned.is_err(), "the deleted branch clones");
    sandbox
        .store()
        .delete_branch(&pushed.name)
        .await
        .map_err(|error| error.context("deleting it again"))
}
