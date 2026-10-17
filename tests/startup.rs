mod common;

use common::*;
use linewright::domain::{MergeMethod, UpdateMethod};
use linewright::factory::FactoryConfig;

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

/// Returns whether it succeeded and what it wrote to stderr. It gets a
/// runtime directory of its own, so that `serve`s run by tests at the same
/// time don't claim the same socket.
fn lw(path: &std::path::Path, args: &[&str], env: &[(&str, &str)]) -> (bool, String) {
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .args(args)
        .current_dir(dir.path())
        .env("PATH", path)
        .env("XDG_RUNTIME_DIR", dir.path())
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

    let (succeeded, error) = lw(
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
    let (succeeded, error) = lw(
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
    let (succeeded, error) = lw(
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
    let (succeeded, error) = lw(
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

fn version() -> String {
    let manifest: toml::Table =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap();
    manifest["package"]["version"].as_str().unwrap().to_owned()
}

fn names_head(commit: &str) -> bool {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(output.status.success());
    commit.len() == 12
        && commit.chars().all(|c| c.is_ascii_hexdigit())
        && String::from_utf8(output.stdout)
            .unwrap()
            .starts_with(commit)
}

fn serve_log(format: &str) -> String {
    let empty = tempfile::tempdir().unwrap();
    let (_, log) = lw(
        empty.path(),
        &[
            "--repo",
            "owner/name",
            "--webhook-address",
            "127.0.0.1:0",
            "--log-format",
            format,
            "serve",
        ],
        &[("LINEWRIGHT_WEBHOOK_SECRET", "s3cret")],
    );
    log
}

fn without_colors(text: &str) -> String {
    text.split('\x1b')
        .map(|part| part.split_once('m').map_or(part, |(_, rest)| rest))
        .collect()
}

#[test]
fn version_names_the_version_and_the_commit_built_from() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    let printed = String::from_utf8(output.stdout).unwrap();
    let commit = printed
        .strip_prefix(&format!("lw {} (", version()))
        .and_then(|rest| rest.strip_suffix(")\n"));
    assert!(commit.is_some_and(names_head), "{printed}");
}

#[test]
fn help_shows_the_command_as_lw() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    let printed = String::from_utf8(output.stdout).unwrap();
    assert!(
        printed.contains("\nUsage: lw [OPTIONS] <COMMAND>\n"),
        "{printed}"
    );
}

#[test]
fn an_error_the_program_prints_itself_starts_with_lw() {
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .arg("status")
        .current_dir(dir.path())
        .env_remove("LINEWRIGHT_REPO")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.starts_with("lw: set `repo`"), "{error}");
}

#[test]
fn serve_first_logs_the_version_and_the_commit() {
    let log = serve_log("json");

    let lines: Vec<serde_json::Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let fields = &lines[0]["fields"];
    assert_eq!(fields["version"], version().as_str(), "{log}");
    assert!(fields["commit"].as_str().is_some_and(names_head), "{log}");
    assert_eq!(
        lines[1]["fields"]["message"], "listening for webhook deliveries",
        "{log}"
    );
}

#[test]
fn serve_first_logs_the_version_and_the_commit_as_text_too() {
    let log = without_colors(&serve_log("text"));

    let first = log.lines().next().unwrap_or_default();
    assert!(first.contains(&format!("version={}", version())), "{log}");
    let commit = first
        .split_once("commit=")
        .and_then(|(_, rest)| rest.split_whitespace().next());
    assert!(commit.is_some_and(names_head), "{log}");
}

#[tokio::test]
async fn startup_creates_the_status_labels_with_what_each_says_an_issue_needs() {
    let world = World::new().await;

    assert_eq!(
        [
            "needs-triage",
            "needs-info",
            "ready-for-agent",
            "ready-for-human"
        ]
        .map(|name| world.store.label_description(name)),
        [
            "Not yet assessed",
            "Its assessment waits on a fact from the reporter",
            "Decided: an agent could build it",
            "Needs a person's decision or action",
        ]
        .map(|description| Some(description.to_string()))
    );
}
