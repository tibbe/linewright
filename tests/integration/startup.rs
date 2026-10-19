use crate::common;

use common::*;
use linewright::domain::{MergeMethod, UpdateMethod};
use linewright::factory::{FactoryConfig, SessionToken};

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

/// Puts the `tool` on the tests' path in `bin`.
fn link_from_path(bin: &std::path::Path, tool: &str) {
    let found = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join(tool))
        .find(|path| path.is_file())
        .unwrap();
    std::os::unix::fs::symlink(found, bin.join(tool)).unwrap();
}

#[test]
fn without_bash_on_the_path_startup_is_an_error_naming_bash() {
    let bin = tempfile::tempdir().unwrap();
    fake_claude(bin.path(), "Done.");
    link_from_path(bin.path(), "git");

    let (succeeded, error) = lw(
        bin.path(),
        &["--repo", "owner/name", "once"],
        &[("GH_TOKEN", "token")],
    );

    assert!(!succeeded, "{error}");
    assert!(error.contains("`bash` is not on the path"), "{error}");
}

#[test]
fn startup_logs_the_names_but_not_the_values_of_the_variables_sessions_are_not_given() {
    let bin = tempfile::tempdir().unwrap();
    fake_claude(bin.path(), "Done.");
    link_from_path(bin.path(), "git");
    link_from_path(bin.path(), "bash");

    let (_, log) = lw(
        bin.path(),
        &["--repo", "owner/name", "--log-format", "json", "once"],
        &[
            ("GH_TOKEN", "linewright-token"),
            ("MY_SECRET", "hunter2"),
            ("LC_ALL", "C"),
            ("DISABLE_UPDATES", "0"),
            // Fails the connection to GitHub at once, after the log.
            ("HTTPS_PROXY", "http://127.0.0.1:1"),
        ],
    );

    let withheld = log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|line| line["fields"]["variables"].as_str().map(String::from))
        .unwrap_or_else(|| panic!("no variables logged: {log}"));
    let withheld: Vec<&str> = withheld.split(' ').collect();
    assert!(withheld.contains(&"MY_SECRET"), "{log}");
    assert!(!withheld.contains(&"LC_ALL"), "{log}");
    // Linewright sets it for every session.
    assert!(!withheld.contains(&"DISABLE_UPDATES"), "{log}");
    // The session is given Linewright's token.
    assert!(!withheld.contains(&"GH_TOKEN"), "{log}");
    assert!(!log.contains("hunter2"), "{log}");
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

#[tokio::test]
async fn a_session_token_that_cannot_read_the_repository_is_an_error_naming_its_variable() {
    let repository = Repository {
        unreadable_by: vec!["session-token"],
        ..Repository::default()
    };

    let started = World::start(repository, |config| {
        config.session_token = SessionToken::Separate("session-token".into());
    })
    .await;

    let error = format!("{:#}", started.err().expect("startup went ahead"));
    assert!(error.contains("LINEWRIGHT_SESSION_GITHUB_TOKEN"), "{error}");
    assert!(error.contains("read access"), "{error}");
}

#[tokio::test]
async fn a_session_token_that_reads_the_repository_starts_without_a_warning() {
    let (log, _guard) = Log::capture();

    World::start(Repository::default(), |config| {
        config.session_token = SessionToken::Separate("session-token".into());
    })
    .await
    .unwrap();

    assert_eq!(log.lines("WARN"), Vec::<String>::new());
}

#[tokio::test]
async fn sessions_given_linewrights_token_are_warned_of_at_startup() {
    let (log, _guard) = Log::capture();

    World::start(Repository::default(), |config| {
        config.session_token = SessionToken::Linewright("linewright-token".into());
    })
    .await
    .unwrap();

    let warnings = log.lines("WARN");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("sessions can write to GitHub"),
        "{}",
        warnings[0]
    );
    assert!(
        warnings[0].contains("LINEWRIGHT_SESSION_GITHUB_TOKEN"),
        "{}",
        warnings[0]
    );
}

#[test]
fn sessions_are_given_the_separate_token_never_linewrights() {
    let session_token = SessionToken::new("linewright-token".into(), Some("session-token".into()));

    assert_eq!(session_token.given(), Some("session-token"));
}

#[test]
fn without_a_separate_token_or_with_an_empty_one_sessions_are_given_linewrights() {
    let unset = SessionToken::new("linewright-token".into(), None);
    let empty = SessionToken::new("linewright-token".into(), Some(String::new()));

    assert_eq!(unset.given(), Some("linewright-token"));
    assert_eq!(empty.given(), Some("linewright-token"));
}

#[test]
fn a_prompt_with_a_placeholder_linewright_does_not_fill_is_an_error_naming_it() {
    let briefs = briefs_with("update.md", |text| text.replace("{{ticket}}", "{{tikcet}}"));

    let (succeeded, error) = lw(
        std::path::Path::new(""),
        &[
            "--repo",
            "owner/name",
            "--briefs",
            briefs.path().to_str().unwrap(),
            "once",
        ],
        &[],
    );

    assert!(!succeeded, "{error}");
    assert!(
        error.contains("reading the prompt update.md: it has unknown placeholders: `{{tikcet}}`"),
        "{error}"
    );
}

#[test]
fn a_prompt_without_a_placeholder_linewright_fills_is_an_error_naming_it() {
    let briefs = briefs_with("triage.md", |text| text.replace("{{issue}}", ""));

    let (succeeded, error) = lw(
        std::path::Path::new(""),
        &[
            "--repo",
            "owner/name",
            "--briefs",
            briefs.path().to_str().unwrap(),
            "once",
        ],
        &[],
    );

    assert!(!succeeded, "{error}");
    assert!(
        error.contains("reading the prompt triage.md: it lacks the placeholders `{{issue}}`"),
        "{error}"
    );
}

#[test]
fn sigquit_while_serve_is_starting_ends_it_with_status_0() {
    let bin = tempfile::tempdir().unwrap();
    link_from_path(bin.path(), "git");
    link_from_path(bin.path(), "bash");
    let started = bin.path().join("claude.started");
    // Startup waits on `claude --version`, which hangs.
    write_program(
        &bin.path().join("claude"),
        &format!(
            "#!/bin/sh\n: > '{}'\nexec /bin/sleep 30\n",
            started.display()
        ),
    );
    let runtime_dir = tempfile::tempdir().unwrap();
    let mut serve = std::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .args(["--repo", "owner/name", "serve"])
        .current_dir(runtime_dir.path())
        .env("PATH", bin.path())
        .env("XDG_RUNTIME_DIR", runtime_dir.path())
        .env("GH_TOKEN", "token")
        .env_remove("LINEWRIGHT_WEBHOOK_SECRET")
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let starting = std::time::Instant::now();
    while !started.exists() {
        assert!(
            starting.elapsed().as_secs() < 10,
            "startup never ran claude"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let sent = std::process::Command::new("kill")
        .args(["-QUIT", &serve.id().to_string()])
        .status()
        .unwrap();

    assert!(sent.success());
    let sent_at = std::time::Instant::now();
    let status = loop {
        if let Some(status) = serve.try_wait().unwrap() {
            break status;
        }
        if sent_at.elapsed().as_secs() >= 10 {
            serve.kill().unwrap();
            panic!("serve did not end");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(0), "{status}");
}
