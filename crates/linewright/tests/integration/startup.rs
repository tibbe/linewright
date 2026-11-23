use linewright_core::domain::label;
use linewright_core::factory::SessionToken;
use linewright_core::testing::*;

use crate::programs::*;

/// A variable that fails every connection to GitHub at once.
const GITHUB_UNREACHABLE: (&str, &str) = ("HTTPS_PROXY", "http://127.0.0.1:1");

/// Runs `lw` with `args` and `env`, `path` as its path, a runtime directory
/// of its own, the model `MODEL`, the web UI on a port the system chooses
/// and none of GitHub's tokens but `env`'s. Returns whether
/// it succeeded and what it wrote to stderr.
fn lw(path: &std::path::Path, args: &[&str], env: &[(&str, &str)]) -> (bool, String) {
    let (status, stderr) = lw_with_status(path, args, env);
    (status.success(), stderr)
}

/// As `lw`, but returns its exit status.
fn lw_with_status(
    path: &std::path::Path,
    args: &[&str],
    env: &[(&str, &str)],
) -> (std::process::ExitStatus, String) {
    lw_with_model(path, args, env, Some(MODEL))
}

/// As `lw`, but with no model set.
fn lw_without_model(path: &std::path::Path, args: &[&str], env: &[(&str, &str)]) -> (bool, String) {
    let (status, stderr) = lw_with_model(path, args, env, None);
    (status.success(), stderr)
}

/// As `lw_with_status`, but with the model `model`, or none.
fn lw_with_model(
    path: &std::path::Path,
    args: &[&str],
    env: &[(&str, &str)],
    model: Option<&str>,
) -> (std::process::ExitStatus, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_lw"));
    match model {
        Some(model) => command.env("LINEWRIGHT_MODEL", model),
        None => command.env_remove("LINEWRIGHT_MODEL"),
    };
    let output = command
        .args(args)
        .current_dir(dir.path())
        .env("PATH", path)
        .env("XDG_RUNTIME_DIR", dir.path())
        .env("LINEWRIGHT_WORK_DIR", dir.path().join("work"))
        .env_remove("LINEWRIGHT_UI_ADDRESS")
        .env_remove("LINEWRIGHT_PUBLIC_URL")
        .env_remove("LINEWRIGHT_GOOGLE_CLIENT_ID")
        .env_remove("LINEWRIGHT_GOOGLE_SIGNUP_DOMAINS")
        .env_remove("LINEWRIGHT_GOOGLE_CLIENT_SECRET")
        .env_remove("LINEWRIGHT_GITHUB_CLIENT_ID")
        .env_remove("LINEWRIGHT_GITHUB_SIGNUP_ORGS")
        .env_remove("LINEWRIGHT_GITHUB_CLIENT_SECRET")
        .env_remove("LINEWRIGHT_MANAGER")
        .env_remove("LINEWRIGHT_WEBHOOK_SECRET")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("LINEWRIGHT_SESSION_GITHUB_TOKEN")
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (
        output.status,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test_log::test]
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

#[test_log::test]
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
            GITHUB_UNREACHABLE,
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
    assert!(!withheld.contains(&"DISABLE_UPDATES"), "{log}");
    assert!(!withheld.contains(&"GH_TOKEN"), "{log}");
    assert!(!log.contains("hunter2"), "{log}");
}

#[test_log::test]
fn startup_logs_the_variables_a_stages_table_gives_its_sessions() {
    let bin = path_to_start_on();
    let config = tempfile::tempdir().unwrap();
    let file = config.path().join("linewright.toml");
    std::fs::write(
        &file,
        "[stage.implement]\nsession-pass-env = [\"DEPLOY_KEY\"]\n",
    )
    .unwrap();

    let (_, log) = lw(
        bin.path(),
        &[
            "--repo",
            "owner/name",
            "--log-format",
            "json",
            "--config",
            file.to_str().unwrap(),
            "once",
        ],
        &[
            ("GH_TOKEN", "linewright-token"),
            ("DEPLOY_KEY", "hunter2"),
            ("MY_SECRET", "hunter3"),
            GITHUB_UNREACHABLE,
        ],
    );

    let logged: Vec<serde_json::Value> = log
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|line: &serde_json::Value| line["fields"]["variables"].is_string())
        .collect();
    let withheld = logged[0]["fields"]["variables"].as_str().unwrap();
    assert!(
        withheld.split(' ').any(|name| name == "DEPLOY_KEY"),
        "{log}"
    );
    let staged: Vec<(&str, &str)> = logged[1..]
        .iter()
        .map(|line| {
            (
                line["fields"]["stage"].as_str().unwrap(),
                line["fields"]["variables"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(staged, [("implement", "DEPLOY_KEY")], "{log}");
    assert!(!log.contains("hunter2"), "{log}");
}

#[test_log::test]
fn startup_logs_the_names_but_not_the_values_session_env_sets() {
    let bin = path_to_start_on();
    let config = tempfile::tempdir().unwrap();
    let file = config.path().join("linewright.toml");
    std::fs::write(
        &file,
        "session-env = { CI = \"value-1\", RUST_LOG = \"value-2\" }\n\n\
         [stage.implement]\nsession-env = { RUST_LOG = \"value-3\", DEPLOY = \"value-4\" }\n\n\
         [stage.design]\nsession-pass-env = [\"OTHER\"]\n",
    )
    .unwrap();

    let (_, log) = lw(
        bin.path(),
        &[
            "--repo",
            "owner/name",
            "--log-format",
            "json",
            "--config",
            file.to_str().unwrap(),
            "--session-env",
            "FLAGGED=value-5",
            "once",
        ],
        &[("GH_TOKEN", "linewright-token"), GITHUB_UNREACHABLE],
    );

    let text = |value: &serde_json::Value| value.as_str().map(str::to_string);
    let set: Vec<(Option<String>, String, Option<String>)> = log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|line| {
            let fields = &line["fields"];
            Some((
                text(&fields["stage"]),
                text(&fields["set"])?,
                text(&fields["taken"]),
            ))
        })
        .collect();
    assert_eq!(
        set,
        [
            (None, "CI FLAGGED RUST_LOG".into(), None),
            (
                Some("implement".into()),
                "DEPLOY RUST_LOG".into(),
                Some("CI FLAGGED".into())
            ),
        ],
        "{log}"
    );
    assert!(!log.contains("value-"), "{log}");
}

/// A path holding a Claude Code that would finish, `git` and `bash`, so
/// that `once` gets as far as connecting to GitHub.
fn path_to_start_on() -> tempfile::TempDir {
    let bin = tempfile::tempdir().unwrap();
    fake_claude(bin.path(), "Done.");
    link_from_path(bin.path(), "git");
    link_from_path(bin.path(), "bash");
    bin
}

#[rstest::rstest]
#[case::serve(&["serve"])]
#[case::once(&["once"])]
#[case::automation_run(&["automation", "run", "weekly.md"])]
#[case::environment_rebuild(&["environment", "rebuild"])]
#[test_log::test]
fn a_command_that_runs_sessions_without_linewrights_token_is_an_error_at_startup_naming_its_variables(
    #[case] command: &[&str],
) {
    let bin = path_to_start_on();
    let args: Vec<&str> = ["--repo", "owner/name"]
        .iter()
        .chain(command)
        .copied()
        .collect();

    let (succeeded, error) = lw(
        bin.path(),
        &args,
        &[("GH_TOKEN", ""), ("GITHUB_TOKEN", ""), GITHUB_UNREACHABLE],
    );

    assert!(!succeeded, "{error}");
    assert!(
        error.contains("set GH_TOKEN or GITHUB_TOKEN to the token of Linewright's account"),
        "{error}"
    );
}

#[rstest::rstest]
#[case::serve(&["serve"])]
#[case::once(&["once"])]
#[case::automation_run(&["automation", "run", "weekly.md"])]
#[case::environment_rebuild(&["environment", "rebuild"])]
#[test_log::test]
fn a_command_that_runs_sessions_without_a_model_is_an_error_at_startup_saying_where_to_set_it(
    #[case] command: &[&str],
) {
    let bin = path_to_start_on();
    let args: Vec<&str> = ["--repo", "owner/name"]
        .iter()
        .chain(command)
        .copied()
        .collect();

    let (succeeded, error) = lw_without_model(
        bin.path(),
        &args,
        &[("GH_TOKEN", "linewright-token"), GITHUB_UNREACHABLE],
    );

    assert!(!succeeded, "{error}");
    assert!(
        error.contains(
            "`model` is required to start a session: set it with `--model`, \
             `LINEWRIGHT_MODEL` or `model` in the operator's config file"
        ),
        "{error}"
    );
}

#[test_log::test]
fn status_runs_without_a_model() {
    let empty = tempfile::tempdir().unwrap();

    let (_, error) = lw_without_model(empty.path(), &["--repo", "owner/name", "status"], &[]);

    assert!(error.contains("not running"), "{error}");
    assert!(!error.contains("`model`"), "{error}");
}

#[test_log::test]
fn secrets_naming_a_variable_that_is_unset_or_empty_is_an_error_at_startup_naming_it() {
    let bin = path_to_start_on();
    for value in [None, Some("")] {
        let mut env = vec![
            ("GH_TOKEN", "linewright-token"),
            ("LINEWRIGHT_SECRETS", "NPM_*,AWS_SECRET_ACCESS_KEY"),
            ("NPM_TOKEN", "npm-token"),
            GITHUB_UNREACHABLE,
        ];
        env.extend(value.map(|value| ("AWS_SECRET_ACCESS_KEY", value)));

        let (succeeded, error) = lw(bin.path(), &["--repo", "owner/name", "once"], &env);

        assert!(!succeeded, "{error}");
        assert!(
            error.contains(
                "the setting `secrets` names `AWS_SECRET_ACCESS_KEY`, which is unset or empty: \
                 set it, or take it out of `secrets`"
            ),
            "{error}"
        );
    }
}

#[test_log::test]
fn secrets_naming_a_prefix_no_variable_has_is_no_error_at_startup() {
    let bin = path_to_start_on();

    let (_, log) = lw(
        bin.path(),
        &["--repo", "owner/name", "once"],
        &[
            ("GH_TOKEN", "linewright-token"),
            ("LINEWRIGHT_SECRETS", "UNSET_PREFIX_*"),
            GITHUB_UNREACHABLE,
        ],
    );

    assert!(!log.contains("`secrets`"), "{log}");
}

#[test_log::test]
fn status_is_not_affected_by_secrets_naming_an_unset_variable() {
    let empty = tempfile::tempdir().unwrap();

    let (_, error) = lw(
        empty.path(),
        &["--repo", "owner/name", "status"],
        &[("LINEWRIGHT_SECRETS", "AWS_SECRET_ACCESS_KEY")],
    );

    assert!(error.contains("not running"), "{error}");
    assert!(!error.contains("`secrets`"), "{error}");
}

#[test_log::test]
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

/// `serve` for `owner/name` with `args` before it, with nothing on the
/// path; returns its exit status.
fn serve_with_status(args: &[&str], env: &[(&str, &str)]) -> (std::process::ExitStatus, String) {
    let empty = tempfile::tempdir().unwrap();
    let mut args = [&["--repo", "owner/name"], args].concat();
    args.push("serve");
    lw_with_status(empty.path(), &args, env)
}

/// The settings of a web UI at a loopback address signing people in with
/// Google, as variables.
const UI: [(&str, &str); 3] = [
    ("LINEWRIGHT_PUBLIC_URL", "https://127.0.0.1:8081"),
    ("LINEWRIGHT_GOOGLE_CLIENT_ID", "google-id"),
    ("LINEWRIGHT_GOOGLE_CLIENT_SECRET", "google-s3cret"),
];

#[test_log::test]
fn serve_refuses_a_ui_setting_without_public_url() {
    let (flag, flag_error) = serve_with_status(&["--google-client-id", "google-id"], &[]);
    let (env, env_error) = serve_with_status(&[], &[("LINEWRIGHT_MANAGER", "github:octocat")]);

    assert_eq!(flag.code(), Some(2), "{flag_error}");
    assert_eq!(env.code(), Some(1), "{env_error}");
    for error in [flag_error, env_error] {
        assert!(error.contains("`public-url` is not"), "{error}");
    }
}

#[test_log::test]
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

#[test_log::test]
fn serve_refuses_a_ui_address_it_cannot_listen_on_before_it_talks_to_github() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap().to_string();

    let empty = tempfile::tempdir().unwrap();
    let (succeeded, error) = lw(
        empty.path(),
        &["--repo", "owner/name", "--ui-address", &address, "serve"],
        &[&UI[..], &[("GH_TOKEN", ""), ("GITHUB_TOKEN", "")]].concat(),
    );

    assert!(!succeeded, "{error}");
    assert!(error.contains("ui-address"), "{error}");
    assert!(error.contains(&address), "{error}");
}

/// What `serve` logs as JSON lines, with `ui_address` as its `ui-address`
/// and the variables `env`, until startup fails for want of `git`.
#[track_caller]
fn serve_log_lines(ui_address: &str, env: &[(&str, &str)]) -> Vec<serde_json::Value> {
    json_log_with(&["--ui-address", ui_address, "serve"], env).0
}

/// The lines of the JSON log of `lw` on `owner/name` with `args` until it
/// fails for want of `git`, and the log as it is.
#[track_caller]
fn json_log(args: &[&str]) -> (Vec<serde_json::Value>, String) {
    json_log_with(args, &[])
}

/// As [`json_log`], with the variables `env`.
#[track_caller]
fn json_log_with(args: &[&str], env: &[(&str, &str)]) -> (Vec<serde_json::Value>, String) {
    let empty = tempfile::tempdir().unwrap();
    let args: Vec<&str> = ["--repo", "owner/name", "--log-format", "json"]
        .iter()
        .chain(args)
        .copied()
        .collect();
    let (_, log) = lw(empty.path(), &args, env);
    let lines = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (lines, log)
}

#[test_log::test]
fn serve_logs_the_address_it_serves_the_web_ui_on() {
    let lines = serve_log_lines("127.0.0.1:0", &UI);

    let serving: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|line| line["fields"]["message"] == "serving the web UI")
        .collect();
    assert_eq!(serving.len(), 1, "{lines:?}");
    let address = serving[0]["fields"]["address"].as_str().unwrap();
    let port = address.strip_prefix("127.0.0.1:").unwrap();
    assert_ne!(port, "0", "{address}");
}

#[test_log::test]
fn without_public_url_serve_logs_that_it_serves_no_web_ui_and_why() {
    let (lines, log) = json_log(&["serve"]);

    assert!(
        lines
            .iter()
            .all(|line| line["fields"]["message"] != "serving the web UI"),
        "{log}"
    );
    let none: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|line| line["fields"]["message"] == "serving no web UI")
        .collect();
    assert_eq!(none.len(), 1, "{log}");
    assert_eq!(none[0]["fields"]["reason"], "`public-url` is not set");
    assert!(
        lines.iter().any(
            |line| line["fields"]["message"] == "answering the monitoring commands and `drain`"
        ),
        "{log}"
    );
}

#[test_log::test]
fn without_public_url_serve_starts_though_it_cannot_listen_on_ui_address() {
    let _held_here_unless_held_already = std::net::TcpListener::bind("127.0.0.1:8081");

    let (_, error) = serve_with_status(&[], &[]);

    assert!(error.contains("`git` is not on the path"), "{error}");
}

#[test_log::test]
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

    assert!(!succeeded, "{error}");
    assert!(error.contains("`git` is not on the path"), "{error}");
}

fn version() -> String {
    let manifest: toml::Table =
        std::fs::read_to_string(linewright_core::testing::repository().join("Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap();
    manifest["workspace"]["package"]["version"]
        .as_str()
        .unwrap()
        .to_owned()
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

#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
fn help_lists_drain_and_serve_as_polling_until_drained_or_stopped() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .arg("--help")
        .output()
        .unwrap();

    let printed = String::from_utf8(output.stdout).unwrap();
    let described = |command: &str| {
        printed
            .lines()
            .find_map(|line| line.trim_start().strip_prefix(command))
            .map(str::trim)
    };
    assert_eq!(
        described("serve "),
        Some("Poll the repository until drained or stopped"),
        "{printed}"
    );
    assert_eq!(
        described("drain "),
        Some("Tell the running `serve` to drain"),
        "{printed}"
    );
}

#[test_log::test]
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

#[test_log::test]
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

/// `docs/specs/operation.md`, The log: "The first line" and "The last line",
/// for a command that does work other than `serve`, and one that learns only
/// as it runs that it does the work.
#[rstest::rstest]
#[case::once(&["once"])]
#[case::environment_rebuild_without_serve(&["environment", "rebuild"])]
#[test_log::test]
fn a_command_that_does_work_logs_the_version_first_and_the_exit_last(#[case] args: &[&str]) {
    let (lines, log) = json_log(args);

    let first = &lines[0]["fields"];
    assert_eq!(first["message"], "starting", "{log}");
    assert_eq!(first["version"], version().as_str(), "{log}");
    assert!(first["commit"].as_str().is_some_and(names_head), "{log}");
    assert_eq!(
        lines.last().unwrap()["fields"],
        serde_json::json!({ "message": "exiting", "status": 1 }),
        "{log}"
    );
}

#[test_log::test]
fn serve_first_logs_the_version_and_the_commit_as_text_too() {
    let log = without_colors(&serve_log("text"));

    let first = log.lines().next().unwrap_or_default();
    assert!(first.contains(&format!("version={}", version())), "{log}");
    let commit = first
        .split_once("commit=")
        .and_then(|(_, rest)| rest.split_whitespace().next());
    assert!(commit.is_some_and(names_head), "{log}");
}

#[test_log::test(tokio::test)]
async fn startup_creates_the_designed_label_with_what_it_says_an_item_has_had() {
    let world = World::new().await;

    assert_eq!(
        world.store.label_description("linewright:designed"),
        Some("Designed, and cut into tickets where it needed cutting".to_string())
    );
}

#[test_log::test(tokio::test)]
async fn startup_creates_the_stalled_label_saying_the_item_was_handed_back_in_a_stall() {
    let world = World::new().await;

    assert_eq!(
        world.store.label_description("linewright:stalled"),
        Some("Handed back in a stall".to_string())
    );
}

#[test_log::test(tokio::test)]
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
            "Its assessment waits on a fact only a person knows",
            "Decided: an agent could build it",
            "Needs a person's decision or action",
        ]
        .map(|description| Some(description.to_string()))
    );
}

#[test_log::test(tokio::test)]
async fn startup_leaves_the_labels_the_repository_has_as_they_are_whatever_their_case() {
    let repository = Repository {
        labels: vec![
            label::Label {
                name: "Needs-Triage",
                description: "Waiting for us to look",
            },
            label::Label {
                name: "needs-info",
                description: "Waiting on the reporter",
            },
            label::Label {
                name: "bug",
                description: "Broken",
            },
        ],
        ..Repository::default()
    };

    let world = World::with_repository(repository, |_| {}).await;

    assert!(!world.store.has_label("needs-triage"));
    assert_eq!(
        world.store.label_description("Needs-Triage"),
        Some("Waiting for us to look".to_string())
    );
    assert_eq!(
        world.store.label_description("needs-info"),
        Some("Waiting on the reporter".to_string())
    );
    assert_eq!(
        world.store.label_description("bug"),
        Some("Broken".to_string())
    );
}

#[test_log::test(tokio::test)]
async fn startup_sets_a_linewright_label_with_another_description_back_to_its_meaning() {
    let repository = Repository {
        labels: vec![label::Label {
            name: "Linewright:Stalled",
            description: "Stuck",
        }],
        ..Repository::default()
    };

    let world = World::with_repository(repository, |_| {}).await;

    assert!(!world.store.has_label("linewright:stalled"));
    assert_eq!(
        world.store.label_description("Linewright:Stalled"),
        Some("Handed back in a stall".to_string())
    );
}

#[test_log::test(tokio::test)]
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

#[test_log::test(tokio::test)]
async fn a_session_token_that_reads_the_repository_starts_without_a_warning() {
    let (log, _guard) = Log::capture();

    World::start(Repository::default(), |config| {
        config.session_token = SessionToken::Separate("session-token".into());
    })
    .await
    .unwrap();

    assert_eq!(log.lines("WARN"), Vec::<String>::new());
}

/// The messages `lw once` logs as warnings, with Linewright's token and
/// `env`, until it fails to reach GitHub.
#[track_caller]
fn warnings_at_startup(env: &[(&str, &str)]) -> Vec<String> {
    let bin = path_to_start_on();
    let env: Vec<(&str, &str)> = [("GH_TOKEN", "linewright-token"), GITHUB_UNREACHABLE]
        .iter()
        .chain(env)
        .copied()
        .collect();
    let (succeeded, log) = lw(
        bin.path(),
        &["--repo", "owner/name", "--log-format", "json", "once"],
        &env,
    );
    assert!(!succeeded, "{log}");
    json_messages(&log, "WARN")
}

#[test_log::test]
fn sessions_given_linewrights_token_are_warned_of_at_startup() {
    let warnings = warnings_at_startup(&[("LINEWRIGHT_SESSION_GITHUB_TOKEN", "")]);

    let warned: Vec<&String> = warnings
        .iter()
        .filter(|warning| warning.contains("sessions can write to GitHub"))
        .collect();
    assert_eq!(warned.len(), 1, "{warnings:?}");
    assert!(
        warned[0].contains("LINEWRIGHT_SESSION_GITHUB_TOKEN"),
        "{}",
        warned[0]
    );
}

#[test_log::test]
fn sessions_given_the_separate_token_are_not_warned_of_at_startup() {
    let warnings = warnings_at_startup(&[("LINEWRIGHT_SESSION_GITHUB_TOKEN", "session-token")]);

    assert!(
        !warnings
            .iter()
            .any(|warning| warning.contains("sessions can write to GitHub")),
        "{warnings:?}"
    );
}

#[test_log::test]
fn sessions_are_given_the_separate_token_never_linewrights() {
    let session_token = SessionToken::new("linewright-token".into(), Some("session-token".into()));

    assert_eq!(session_token.given(), "session-token");
}

#[test_log::test]
fn without_a_separate_token_or_with_an_empty_one_sessions_are_given_linewrights() {
    let unset = SessionToken::new("linewright-token".into(), None);
    let empty = SessionToken::new("linewright-token".into(), Some(String::new()));

    assert_eq!(unset.given(), "linewright-token");
    assert_eq!(empty.given(), "linewright-token");
}

#[test_log::test]
fn a_prompt_with_a_placeholder_linewright_does_not_fill_is_an_error_naming_it() {
    let briefs = briefs_with("prompts/update.md", |text| {
        text.replace("{{ticket}}", "{{tikcet}}")
    });

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
        error.contains("- prompts/update.md: it has unknown placeholders: `{{tikcet}}`"),
        "{error}"
    );
}

#[test_log::test]
fn a_prompt_without_a_placeholder_linewright_fills_is_an_error_naming_it() {
    let briefs = briefs_with("prompts/triage.md", |text| text.replace("{{issue}}", ""));

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
        error.contains("- prompts/triage.md: it lacks the placeholders `{{issue}}`"),
        "{error}"
    );
}

#[test_log::test]
fn startup_logs_the_files_the_operators_briefs_provide() {
    let briefs = briefs_with("prompts/implement.md", |text| text);
    std::fs::write(briefs.path().join(".DS_Store"), "Finder's.").unwrap();

    let (succeeded, log) = lw(
        std::path::Path::new(""),
        &[
            "--repo",
            "owner/name",
            "--briefs",
            briefs.path().to_str().unwrap(),
            "--log-format",
            "json",
            "once",
        ],
        &[],
    );

    assert!(!succeeded, "fails later, without `git`: {log}");
    let logged = |message: &str| {
        log.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|line| line["fields"]["message"] == message)
            .and_then(|line| line["fields"]["files"].as_str().map(String::from))
            .unwrap_or_else(|| panic!("no {message:?} logged: {log}"))
    };
    assert_eq!(
        logged("the operator's briefs provide these files"),
        "prompts/implement.md"
    );
}

#[test_log::test]
fn linewrights_own_briefs_can_be_the_operators_briefs() {
    let shipped = linewright_core::testing::repository().join("briefs");
    let dir = tempfile::tempdir().unwrap();
    for path in linewright_core::briefs::Briefs::shipped().layers().keys() {
        let copy = dir.path().join(path);
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        std::fs::copy(shipped.join(path), copy).unwrap();
    }
    let briefs = dir.path();

    let (succeeded, log) = lw(
        std::path::Path::new(""),
        &[
            "--repo",
            "owner/name",
            "--briefs",
            briefs.to_str().unwrap(),
            "--log-format",
            "json",
            "once",
        ],
        &[],
    );

    assert!(!succeeded, "fails later, without `git`: {log}");
    assert!(
        log.contains("the operator's briefs provide these files"),
        "{log}"
    );
}

#[test_log::test]
fn files_in_the_operators_briefs_that_a_layer_may_not_hold_are_an_error_naming_each() {
    let briefs = briefs_with("prompts/implement.md", |text| text);
    std::fs::write(briefs.path().join("prompts/implment.md"), "A misspelling.").unwrap();
    let schema = briefs.path().join("results/implement.schema.json");
    std::fs::create_dir_all(schema.parent().unwrap()).unwrap();
    std::fs::write(schema, "{}").unwrap();

    let (succeeded, error) = lw(
        std::path::Path::new(""),
        &[
            "--repo",
            "owner/name",
            "--briefs",
            briefs.path().to_str().unwrap(),
            "serve",
        ],
        &[],
    );

    assert!(!succeeded, "{error}");
    assert!(
        error.contains("- prompts/implment.md: not a file a layer may hold"),
        "{error}"
    );
    assert!(
        error.contains("- results/implement.schema.json: not a file a layer may hold"),
        "{error}"
    );
}

#[test_log::test]
fn sigterm_while_serve_is_starting_drains_it_with_status_0_saying_the_next_signal_stops_and_logs_exiting_last()
 {
    let bin = tempfile::tempdir().unwrap();
    link_from_path(bin.path(), "git");
    link_from_path(bin.path(), "bash");
    let started = bin.path().join("claude.started");
    fake_claude_running(
        bin.path(),
        &Scenario {
            version: vec![Step::Touch(started.clone()), Step::Hang],
            ..Scenario::default()
        },
    );
    let runtime_dir = tempfile::tempdir().unwrap();
    let log = runtime_dir.path().join("serve.log");
    let mut serve = std::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .args(["--repo", "owner/name", "--log-format", "json", "serve"])
        .current_dir(runtime_dir.path())
        .env("PATH", bin.path())
        .env("XDG_RUNTIME_DIR", runtime_dir.path())
        .env("LINEWRIGHT_WORK_DIR", runtime_dir.path().join("work"))
        .env("GH_TOKEN", "token")
        .env("LINEWRIGHT_MODEL", MODEL)
        .env_remove("LINEWRIGHT_UI_ADDRESS")
        .env_remove("LINEWRIGHT_PUBLIC_URL")
        .env_remove("LINEWRIGHT_GOOGLE_CLIENT_ID")
        .env_remove("LINEWRIGHT_GOOGLE_SIGNUP_DOMAINS")
        .env_remove("LINEWRIGHT_GOOGLE_CLIENT_SECRET")
        .env_remove("LINEWRIGHT_GITHUB_CLIENT_ID")
        .env_remove("LINEWRIGHT_GITHUB_SIGNUP_ORGS")
        .env_remove("LINEWRIGHT_GITHUB_CLIENT_SECRET")
        .env_remove("LINEWRIGHT_MANAGER")
        .env_remove("LINEWRIGHT_WEBHOOK_SECRET")
        .stderr(std::fs::File::create(&log).unwrap())
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
        .args(["-TERM", &serve.id().to_string()])
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
    let log = std::fs::read_to_string(&log).unwrap();
    let drained = log
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|line| line["fields"]["by"] == "SIGTERM")
        .unwrap_or_else(|| panic!("no line by SIGTERM in {log}"));
    assert_eq!(
        drained["fields"]["message"],
        "draining: exiting once no run is in flight; the next signal stops instead"
    );
    let last: serde_json::Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    assert_eq!(
        (&last["level"], &last["fields"]),
        (
            &serde_json::json!("INFO"),
            &serde_json::json!({ "message": "exiting", "status": 0 })
        ),
        "{log}"
    );
}

/// How many times `issue` was labeled `linewright:designed`.
fn designed_labelings(issue: &linewright_core::domain::Issue) -> usize {
    issue
        .events
        .iter()
        .filter(|event| {
            matches!(event, linewright_core::domain::IssueEvent::Labeled { label: name, .. }
                if name == label::DESIGNED)
        })
        .count()
}

#[test_log::test(tokio::test)]
async fn startup_labels_an_old_open_spec_change_tracking_issue_designed_once_and_its_cut_goes_on() {
    let mut world = World::new().await;
    let cut = world.old_spec_change_cut().await;
    let before = designed_labelings(&world.issue(cut.tracking));

    for _ in 0..2 {
        world.restart(|_| {}).await;
        world.poll().await;
    }

    let tracking = world.issue(cut.tracking);
    assert!(tracking.has_label(label::DESIGNED), "{tracking:?}");
    assert_eq!(designed_labelings(&tracking), before + 1);
    for ticket in cut.tickets {
        assert!(world.issue(ticket).open, "#{ticket} was closed");
    }
}
