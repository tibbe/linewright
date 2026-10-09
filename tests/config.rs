//! Configuration as the operator gives it: flags, `LINEWRIGHT_*` environment
//! variables and a TOML file, with that precedence over the defaults
//! (`docs/spec.md`, "Operation").

use std::collections::HashMap;
use std::time::Duration;

use clap::Parser;
use linewright::config::{Cli, Config, LogFormat};
use linewright::domain::MergeMethod;

fn resolve(args: &[&str], env: &[(&str, &str)], file: Option<&str>) -> Result<Config, String> {
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
    .map_err(|e| format!("{e:#}"))
}

#[test]
fn a_flag_beats_the_environment_which_beats_the_file_which_beats_the_default() {
    let file = "repo = \"acme/file\"\nmax-workers = 2\npoll-interval = \"5m\"\nci-attempts = 7\n";
    let env = [
        ("LINEWRIGHT_REPO", "acme/env"),
        ("LINEWRIGHT_MAX_WORKERS", "3"),
    ];

    let config = resolve(&["--repo", "acme/flag"], &env, Some(file)).unwrap();

    assert_eq!(config.repo.as_deref(), Some("acme/flag"));
    assert_eq!(config.max_workers, 3);
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
    assert_eq!(config.max_workers, 4);
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
        resolve(&["--max-workers", "many"], &[], None)
            .unwrap_err()
            .contains("max-workers")
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
