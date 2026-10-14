//! Configuration as the operator gives it: flags, `LINEWRIGHT_*` environment
//! variables and a TOML file, with that precedence over the defaults
//! (`docs/specs/operation.md`).

use std::collections::HashMap;
use std::time::Duration;

use clap::{CommandFactory, Parser};
use linewright::config::{Cli, Config, LogFormat, ResolveError};
use linewright::domain::MergeMethod;

fn resolve(args: &[&str], env: &[(&str, &str)], file: Option<&str>) -> Result<Config, String> {
    resolve_blaming(args, env, file).map_err(|e| e.to_string())
}

/// Resolves the settings as [`resolve`] does, keeping the error's layer.
fn resolve_blaming(
    args: &[&str],
    env: &[(&str, &str)],
    file: Option<&str>,
) -> Result<Config, ResolveError> {
    let dir = tempfile::tempdir().unwrap();
    let mut argv = vec!["linewright"];
    argv.extend(args);
    argv.push("status");
    let mut cli = Cli::try_parse_from(argv).unwrap();
    if let Some(file) = file {
        let path = dir.path().join("linewright.toml");
        std::fs::write(&path, file).unwrap();
        cli.settings.config = Some(path);
    }
    let env: HashMap<String, String> = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Config::resolve(
        &cli.settings,
        dir.path(),
        &|key| env.get(key).cloned(),
        false,
    )
}

#[test]
fn a_flag_beats_the_environment_which_beats_the_file_which_beats_the_default() {
    let file = "repo = \"acme/file\"\nmax-runs = 2\npoll-interval = \"5m\"\nci-attempts = 7\n";
    let env = [
        ("LINEWRIGHT_REPO", "acme/env"),
        ("LINEWRIGHT_MAX_RUNS", "3"),
    ];

    let config = resolve(&["--repo", "acme/flag"], &env, Some(file)).unwrap();

    assert_eq!(config.repo.as_deref(), Some("acme/flag"));
    assert_eq!(config.max_runs, 3);
    assert_eq!(config.poll_interval, Duration::from_secs(300));
    assert_eq!(config.ci_attempts, 7);
    assert_eq!(config.session_timeout, Duration::from_secs(30 * 60));
    assert_eq!(config.spec_glob.pattern(), "docs/**/*.md");
}

#[test]
fn defaults_follow_the_spec() {
    let config = resolve(&["--repo", "acme/widgets"], &[], None).unwrap();

    assert_eq!(config.poll_interval, Duration::from_secs(60));
    assert_eq!(config.spec_lookback, chrono::TimeDelta::days(7));
    assert_eq!(config.max_runs, 4);
    assert_eq!(config.result_retries, 1);
    assert_eq!(config.ci_grace, chrono::TimeDelta::minutes(5));
    assert_eq!(config.ci_timeout, chrono::TimeDelta::hours(6));
    assert_eq!(config.ci_attempts, 2);
    assert_eq!(config.update_branch, None);
    assert_eq!(config.model, None);
    assert_eq!(config.log_format, LogFormat::Json, "not a terminal");
    assert!(config.work_dir.ends_with("linewright"));
}

#[test]
fn a_bad_value_or_an_unknown_key_is_an_error_naming_the_setting() {
    assert!(
        resolve(&["--max-runs", "many"], &[], None)
            .unwrap_err()
            .contains("max-runs")
    );
    assert!(
        resolve(&[], &[("LINEWRIGHT_UPDATE_BRANCH", "squash")], None)
            .unwrap_err()
            .contains("update-branch")
    );
    assert!(
        resolve(&[], &[], Some("max-wokers = 2\n"))
            .unwrap_err()
            .contains("max-wokers")
    );
}

/// Whether resolving with `args`, `env` and `file` blames the command line.
fn blames_the_command_line(args: &[&str], env: &[(&str, &str)], file: Option<&str>) -> bool {
    match resolve_blaming(args, env, file) {
        Err(ResolveError::CommandLine(_)) => true,
        Err(ResolveError::Elsewhere(_)) => false,
        Ok(_) => panic!("{args:?}, {env:?} and {file:?} resolve"),
    }
}

#[test]
fn a_wrong_value_is_blamed_on_the_command_line_only_when_a_flag_gave_it() {
    let bad_env = [("LINEWRIGHT_UPDATE_BRANCH", "squash")];

    assert!(blames_the_command_line(&["--max-runs", "many"], &[], None));
    assert!(blames_the_command_line(
        &["--ci-grace", "10m"],
        &[("LINEWRIGHT_CI_TIMEOUT", "5m")],
        None
    ));
    assert!(!blames_the_command_line(&[], &bad_env, None));
    assert!(!blames_the_command_line(
        &[],
        &[],
        Some("max-runs = \"many\"\n")
    ));
    assert!(!blames_the_command_line(
        &["--max-runs", "2"],
        &bad_env,
        None
    ));
    assert!(
        !blames_the_command_line(&["--webhook-address", "127.0.0.1:9000"], &[], None),
        "the secret is missing from the environment"
    );
}

#[test]
fn a_config_file_the_command_line_names_that_cannot_be_read_is_blamed_on_it() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let cli = Cli::try_parse_from([
        "linewright",
        "--config",
        missing.to_str().unwrap(),
        "status",
    ])
    .unwrap();

    let resolved = Config::resolve(&cli.settings, dir.path(), &|_| None, false);

    assert!(matches!(resolved, Err(ResolveError::CommandLine(_))));
}

#[test]
fn max_workers_is_no_longer_a_flag() {
    assert!(Cli::try_parse_from(["linewright", "--max-workers", "2", "status"]).is_err());
}

#[test]
fn a_ci_timeout_no_longer_than_the_grace_period_is_an_error() {
    let error = resolve(&["--ci-grace", "10m", "--ci-timeout", "5m"], &[], None).unwrap_err();

    assert!(error.contains("ci-timeout"), "{error}");
}

#[test]
fn auto_merge_is_off_by_default_and_a_bare_flag_turns_it_on() {
    let off = resolve(&[], &[], None).unwrap();
    let flag = resolve(&["--auto-merge"], &[], None).unwrap();
    let env = resolve(&[], &[("LINEWRIGHT_AUTO_MERGE", "true")], None).unwrap();
    let file = resolve(&[], &[], Some("auto-merge = true\n")).unwrap();
    let flag_over_file =
        resolve(&["--auto-merge=false"], &[], Some("auto-merge = true\n")).unwrap();

    assert!(!off.auto_merge);
    assert!(flag.auto_merge);
    assert!(env.auto_merge);
    assert!(file.auto_merge);
    assert!(!flag_over_file.auto_merge);
}

#[test]
fn the_merge_method_follows_the_repository_unless_set() {
    let unset = resolve(&[], &[], None).unwrap();
    let flag = resolve(&["--merge-method", "squash"], &[], None).unwrap();
    let env = resolve(
        &[],
        &[("LINEWRIGHT_MERGE_METHOD", "rebase")],
        Some("merge-method = \"merge\"\n"),
    )
    .unwrap();

    assert_eq!(unset.merge_method, None);
    assert_eq!(flag.merge_method, Some(MergeMethod::Squash));
    assert_eq!(env.merge_method, Some(MergeMethod::Rebase));
}

#[test]
fn a_bad_auto_merge_or_merge_method_is_an_error_naming_the_setting() {
    assert!(
        resolve(&[], &[("LINEWRIGHT_MERGE_METHOD", "fast-forward")], None)
            .unwrap_err()
            .contains("merge-method")
    );
    assert!(
        resolve(&[], &[], Some("auto-merge = \"yes\"\n"))
            .unwrap_err()
            .contains("auto-merge")
    );
}

#[test]
fn every_issue_is_in_scope_unless_a_label_is_required() {
    let unset = resolve(&[], &[], None).unwrap();
    let flag = resolve(&["--require-label", "linewright"], &[], None).unwrap();
    let env = resolve(
        &[],
        &[("LINEWRIGHT_REQUIRE_LABEL", "from-env")],
        Some("require-label = \"from-file\"\n"),
    )
    .unwrap();
    let file = resolve(&[], &[], Some("require-label = \"from-file\"\n")).unwrap();

    assert_eq!(unset.require_label, None);
    assert_eq!(flag.require_label.as_deref(), Some("linewright"));
    assert_eq!(env.require_label.as_deref(), Some("from-env"));
    assert_eq!(file.require_label.as_deref(), Some("from-file"));
}

#[test]
fn webhooks_are_off_without_the_secret() {
    let config = resolve(&[], &[], None).unwrap();

    assert!(config.webhooks.is_none());
}

#[test]
fn the_secret_turns_webhooks_on_at_the_default_address() {
    let config = resolve(&[], &[("LINEWRIGHT_WEBHOOK_SECRET", "s3cret")], None).unwrap();

    let webhooks = config.webhooks.unwrap();
    assert_eq!(webhooks.address.to_string(), "0.0.0.0:8080");
}

#[test]
fn the_webhook_address_is_a_flag_the_environment_or_the_file() {
    let secret = ("LINEWRIGHT_WEBHOOK_SECRET", "s3cret");
    let address = |args: &[&str], env: &[(&str, &str)], file| {
        let mut env = env.to_vec();
        env.push(secret);
        resolve(args, &env, file)
            .unwrap()
            .webhooks
            .unwrap()
            .address
            .to_string()
    };
    let file = Some("webhook-address = \"127.0.0.1:9000\"\n");

    assert_eq!(address(&[], &[], file), "127.0.0.1:9000");
    assert_eq!(
        address(
            &[],
            &[("LINEWRIGHT_WEBHOOK_ADDRESS", "localhost:9001")],
            file
        ),
        "localhost:9001"
    );
    assert_eq!(
        address(
            &["--webhook-address", "[::1]:9002"],
            &[("LINEWRIGHT_WEBHOOK_ADDRESS", "localhost:9001")],
            file
        ),
        "[::1]:9002"
    );
}

#[test]
fn a_webhook_address_that_is_not_a_host_and_port_is_an_error_naming_the_setting() {
    let secret = [("LINEWRIGHT_WEBHOOK_SECRET", "s3cret")];
    for bad in [
        "8080",
        "localhost",
        ":8080",
        "localhost:http",
        "localhost:99999",
    ] {
        let error = resolve(&["--webhook-address", bad], &secret, None).unwrap_err();

        assert!(error.contains("webhook-address"), "{bad}: {error}");
    }
}

#[test]
fn a_webhook_address_without_the_secret_is_an_error_naming_the_secret() {
    let flag = resolve(&["--webhook-address", "127.0.0.1:9000"], &[], None).unwrap_err();
    let file = resolve(&[], &[], Some("webhook-address = \"127.0.0.1:9000\"\n")).unwrap_err();

    assert!(flag.contains("LINEWRIGHT_WEBHOOK_SECRET"), "{flag}");
    assert!(file.contains("LINEWRIGHT_WEBHOOK_SECRET"), "{file}");
}

#[test]
fn the_webhook_secret_is_not_a_flag_or_a_file_key() {
    let flag = Cli::try_parse_from(["linewright", "--webhook-secret", "s3cret", "status"]);
    let file = resolve(&[], &[], Some("webhook-secret = \"s3cret\"\n")).unwrap_err();

    assert!(flag.is_err());
    assert!(file.contains("webhook-secret"), "{file}");
}

#[test]
fn the_webhook_secret_is_not_shown_in_the_config() {
    let config = resolve(&[], &[("LINEWRIGHT_WEBHOOK_SECRET", "s3cret")], None).unwrap();

    let shown = format!("{config:?}");
    assert!(!shown.contains("s3cret"), "{shown}");
}

#[test]
fn help_lists_the_webhook_address_with_its_default() {
    let help = Cli::command().render_long_help().to_string();

    let line = help
        .lines()
        .skip_while(|line| !line.contains("--webhook-address"))
        .take(4)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(line.contains("0.0.0.0:8080"), "{help}");
}
