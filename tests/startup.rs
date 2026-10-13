//! Startup, end to end at the operator's level: the repository's settings
//! and Linewright's in, a factory or an error out, against the local store
//! (`docs/specs/repository.md`). `serve`, `once` and `workflow run` all start
//! this way.

mod common;

use common::*;
use linewright::domain::{MergeMethod, UpdateMethod};
use linewright::factory::FactoryConfig;

/// The error startup with auto-merge and `adjust` applied fails with.
async fn refused(repository: Repository, adjust: impl FnOnce(&mut FactoryConfig)) -> String {
    let started = World::start(repository, |config| {
        config.auto_merge = true;
        adjust(config);
    })
    .await;
    match started {
        Ok(_) => panic!("startup went ahead"),
        Err(e) => format!("{e:#}"),
    }
}

#[tokio::test]
async fn with_auto_merge_a_merge_method_the_repository_disallows_is_an_error_naming_what_it_allows()
{
    let repository = Repository::allowing([MergeMethod::Merge, MergeMethod::Rebase]);

    let error = refused(repository, |config| {
        config.merge_method = Some(MergeMethod::Squash);
    })
    .await;

    assert!(error.contains("`squash`"), "{error}");
    assert!(error.contains("`merge` and `rebase`"), "{error}");
}

#[tokio::test]
async fn with_auto_merge_and_linear_history_a_default_rebase_the_repository_disallows_is_an_error()
{
    let repository = Repository {
        linear_history: true,
        ..Repository::allowing([MergeMethod::Merge, MergeMethod::Squash])
    };

    let error = refused(repository, |_| {}).await;

    assert!(error.contains("`rebase`"), "{error}");
    assert!(error.contains("`merge` and `squash`"), "{error}");
}

#[tokio::test]
async fn with_auto_merge_update_branch_merge_with_rebase_merges_is_an_error() {
    let repository = Repository::allowing([MergeMethod::Rebase]);

    let error = refused(repository, |config| {
        config.update_method = Some(UpdateMethod::Merge);
    })
    .await;

    assert!(error.contains("`update-branch` `merge`"), "{error}");
    assert!(error.contains("rebase-mergeable"), "{error}");
}

#[tokio::test]
async fn with_auto_merge_the_merge_method_in_effect_is_logged() {
    let (log, _guard) = Log::capture();
    let repository = Repository::allowing([MergeMethod::Squash, MergeMethod::Rebase]);

    World::start(repository, |config| config.auto_merge = true)
        .await
        .unwrap();

    let started: Vec<String> = log
        .lines("INFO")
        .into_iter()
        .filter(|line| line.contains("started"))
        .collect();
    assert_eq!(started.len(), 1, "{started:?}");
    assert!(started[0].contains("merge_method=squash"), "{}", started[0]);
}

#[tokio::test]
async fn without_auto_merge_neither_the_merge_method_nor_update_branch_is_checked() {
    let rebase_only = Repository::allowing([MergeMethod::Rebase]);
    let squash_only = Repository::allowing([MergeMethod::Squash]);

    let merge_updates_with_rebase_merges = World::start(rebase_only, |config| {
        config.update_method = Some(UpdateMethod::Merge);
    })
    .await;
    let disallowed_merge_method = World::start(squash_only, |config| {
        config.merge_method = Some(MergeMethod::Rebase);
    })
    .await;

    assert!(merge_updates_with_rebase_merges.is_ok());
    assert!(disallowed_merge_method.is_ok());
}

/// Runs `linewright` with `args`, `env` and only `path` on the path, and
/// returns whether it succeeded and what it wrote to stderr.
fn linewright(path: &std::path::Path, args: &[&str], env: &[(&str, &str)]) -> (bool, String) {
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_linewright"))
        .args(args)
        .current_dir(dir.path())
        .env("PATH", path)
        .env_remove("LINEWRIGHT_WEBHOOK_SECRET")
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn without_bash_on_the_path_startup_is_an_error_naming_bash() {
    let bin = tempfile::tempdir().unwrap();
    fake_claude(bin.path(), "Done.");
    let git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("git"))
        .find(|git| git.is_file())
        .unwrap();
    std::os::unix::fs::symlink(git, bin.path().join("git")).unwrap();

    let (succeeded, error) = linewright(
        bin.path(),
        &["--repo", "owner/name", "once"],
        &[("GH_TOKEN", "token")],
    );

    assert!(!succeeded, "{error}");
    assert!(error.contains("`bash` is not on the path"), "{error}");
}

#[test]
fn serve_refuses_a_webhook_address_without_the_secret() {
    let empty = tempfile::tempdir().unwrap();
    let (succeeded, error) = linewright(
        empty.path(),
        &[
            "--repo",
            "owner/name",
            "--webhook-address",
            "127.0.0.1:0",
            "serve",
        ],
        &[],
    );

    assert!(!succeeded, "{error}");
    assert!(error.contains("LINEWRIGHT_WEBHOOK_SECRET"), "{error}");
}

#[test]
fn serve_refuses_a_webhook_address_it_cannot_listen_on() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap().to_string();

    let empty = tempfile::tempdir().unwrap();
    let (succeeded, error) = linewright(
        empty.path(),
        &[
            "--repo",
            "owner/name",
            "--webhook-address",
            &address,
            "serve",
        ],
        &[("LINEWRIGHT_WEBHOOK_SECRET", "s3cret")],
    );

    assert!(!succeeded, "{error}");
    assert!(error.contains("webhook-address"), "{error}");
    assert!(error.contains(&address), "{error}");
}

#[test]
fn once_does_not_listen_even_with_the_secret() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap().to_string();

    let empty = tempfile::tempdir().unwrap();
    let (succeeded, error) = linewright(
        empty.path(),
        &[
            "--repo",
            "owner/name",
            "--webhook-address",
            &address,
            "once",
        ],
        &[("LINEWRIGHT_WEBHOOK_SECRET", "s3cret")],
    );

    // It goes on to the startup checks, which fail here for want of `git`.
    assert!(!succeeded, "{error}");
    assert!(error.contains("`git` is not on the path"), "{error}");
}
