use std::collections::HashMap;
use std::time::Duration;

use clap::{CommandFactory, Parser};
use linewright::config::{Cli, Config, LogFormat, ResolveError};
use linewright::domain::{AutoMerge, MergeMethod};
use linewright::host::HostName;
use linewright::session_env::SessionEnv;

fn resolve(args: &[&str], env: &[(&str, &str)], file: Option<&str>) -> Result<Config, String> {
    resolve_blaming(args, env, file).map_err(|e| e.to_string())
}

fn resolve_blaming(
    args: &[&str],
    env: &[(&str, &str)],
    file: Option<&str>,
) -> Result<Config, ResolveError> {
    let dir = tempfile::tempdir().unwrap();
    let mut argv = vec!["lw"];
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
    assert_eq!(config.poll_interval, Duration::from_mins(5));
    assert_eq!(config.ci_attempts, 7);
    assert_eq!(config.session_timeout, Duration::from_hours(1));
    assert_eq!(config.spec_glob.pattern(), "docs/**/*.md");
}

#[test]
fn defaults_follow_the_spec() {
    let config = resolve(&["--repo", "acme/widgets"], &[], None).unwrap();

    assert_eq!(config.poll_interval, Duration::from_mins(1));
    assert_eq!(config.spec_lookback, chrono::TimeDelta::days(7));
    assert_eq!(config.max_runs, 4);
    assert_eq!(config.session_timeout, Duration::from_hours(1));
    assert_eq!(config.result_retries, 1);
    assert_eq!(config.ci_grace, chrono::TimeDelta::minutes(5));
    assert_eq!(config.ci_timeout, chrono::TimeDelta::hours(6));
    assert_eq!(config.ci_attempts, 2);
    assert_eq!(config.update_branch, None);
    assert_eq!(config.model, None);
    assert_eq!(config.log_format, LogFormat::Json, "not a terminal");
    assert!(config.work_dir.ends_with("linewright"));
    assert_eq!(config.session_env, SessionEnv::default());
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
    let cli = Cli::try_parse_from(["lw", "--config", missing.to_str().unwrap(), "status"]).unwrap();

    let resolved = Config::resolve(&cli.settings, dir.path(), &|_| None, false);

    assert!(matches!(resolved, Err(ResolveError::CommandLine(_))));
}

#[test]
fn max_workers_is_no_longer_a_flag() {
    assert!(Cli::try_parse_from(["lw", "--max-workers", "2", "status"]).is_err());
}

#[test]
fn a_ci_timeout_no_longer_than_the_grace_period_is_an_error() {
    let error = resolve(&["--ci-grace", "10m", "--ci-timeout", "5m"], &[], None).unwrap_err();

    assert!(error.contains("ci-timeout"), "{error}");
}

#[test]
fn auto_merge_is_off_by_default_and_a_bare_flag_turns_it_on() {
    let on = Some(AutoMerge { merge_method: None });
    let off = resolve(&[], &[], None).unwrap();
    let flag = resolve(&["--auto-merge"], &[], None).unwrap();
    let env = resolve(&[], &[("LINEWRIGHT_AUTO_MERGE", "true")], None).unwrap();
    let file = resolve(&[], &[], Some("auto-merge = true\n")).unwrap();
    let flag_over_file =
        resolve(&["--auto-merge=false"], &[], Some("auto-merge = true\n")).unwrap();

    assert_eq!(off.auto_merge, None);
    assert_eq!(flag.auto_merge, on);
    assert_eq!(env.auto_merge, on);
    assert_eq!(file.auto_merge, on);
    assert_eq!(flag_over_file.auto_merge, None);
}

#[test]
fn with_auto_merge_the_merge_method_follows_the_repository_unless_set() {
    let merge_method = |args: &[&str], env: &[(&str, &str)], file| {
        resolve(args, env, file)
            .unwrap()
            .auto_merge
            .unwrap()
            .merge_method
    };
    let on = ("LINEWRIGHT_AUTO_MERGE", "true");

    assert_eq!(merge_method(&[], &[on], None), None);
    assert_eq!(
        merge_method(&["--merge-method", "squash"], &[on], None),
        Some(MergeMethod::Squash)
    );
    assert_eq!(
        merge_method(
            &[],
            &[on, ("LINEWRIGHT_MERGE_METHOD", "rebase")],
            Some("merge-method = \"merge\"\n"),
        ),
        Some(MergeMethod::Rebase)
    );
    assert_eq!(
        merge_method(&[], &[on], Some("merge-method = \"merge\"\n")),
        Some(MergeMethod::Merge)
    );
}

#[test]
fn a_merge_method_without_auto_merge_is_an_error_naming_both() {
    let flag = resolve(&["--merge-method", "squash"], &[], None).unwrap_err();
    let env = resolve(&[], &[("LINEWRIGHT_MERGE_METHOD", "squash")], None).unwrap_err();
    let file = resolve(&[], &[], Some("merge-method = \"squash\"\n")).unwrap_err();
    let turned_off = resolve(
        &["--auto-merge=false"],
        &[],
        Some("auto-merge = true\nmerge-method = \"squash\"\n"),
    )
    .unwrap_err();

    for error in [flag, env, file, turned_off] {
        assert_eq!(
            error,
            "the setting `merge-method` is set, but `auto-merge` is off: \
             turn `auto-merge` on, or unset `merge-method`"
        );
    }
}

#[test]
fn a_merge_method_without_auto_merge_is_blamed_on_the_command_line_only_when_a_flag_turned_it_off()
{
    let file = Some("merge-method = \"squash\"\n");

    assert!(blames_the_command_line(&["--auto-merge=false"], &[], file));
    assert!(!blames_the_command_line(
        &["--merge-method", "squash"],
        &[],
        None
    ));
    assert!(!blames_the_command_line(
        &[],
        &[("LINEWRIGHT_AUTO_MERGE", "false")],
        file
    ));
}

#[test]
fn a_bad_auto_merge_or_merge_method_is_an_error_naming_the_setting() {
    assert!(
        resolve(
            &["--auto-merge"],
            &[("LINEWRIGHT_MERGE_METHOD", "fast-forward")],
            None
        )
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
    let env = resolve(
        &[],
        &[("LINEWRIGHT_WEBHOOK_ADDRESS", "127.0.0.1:9000")],
        None,
    )
    .unwrap_err();
    let file = resolve(&[], &[], Some("webhook-address = \"127.0.0.1:9000\"\n")).unwrap_err();
    // The default only with the secret set.
    let the_default = resolve(&["--webhook-address", "0.0.0.0:8080"], &[], None).unwrap_err();

    for error in [flag, env, file, the_default] {
        assert!(error.contains("LINEWRIGHT_WEBHOOK_SECRET"), "{error}");
    }
}

#[test]
fn the_webhook_secret_is_not_a_flag_or_a_file_key() {
    let flag = Cli::try_parse_from(["lw", "--webhook-secret", "s3cret", "status"]);
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

/// `lw --help` and the lines that start at `flag`'s.
fn help_for(flag: &str) -> (String, String) {
    let help = Cli::command().render_long_help().to_string();
    let lines = help
        .lines()
        .skip_while(|line| !line.contains(flag))
        .take(4)
        .collect::<Vec<_>>()
        .join("\n");
    (help, lines)
}

#[test]
fn help_lists_the_webhook_address_with_its_default() {
    let (help, line) = help_for("--webhook-address");
    assert!(
        line.contains("[default: 0.0.0.0:8080 with `LINEWRIGHT_WEBHOOK_SECRET` set"),
        "{help}"
    );
}

fn session_env(names: &[&str]) -> SessionEnv {
    SessionEnv::new(names.iter().map(ToString::to_string)).unwrap()
}

#[test]
fn session_env_is_a_flag_the_environment_or_the_file_naming_variables() {
    let file = Some("session-env = [\"CARGO_HOME\", \"RUSTUP_HOME\"]\n");
    let env = [("LINEWRIGHT_SESSION_ENV", "CARGO_*,SCCACHE_DIR")];

    let from_file = resolve(&[], &[], file).unwrap();
    let from_env = resolve(&[], &env, file).unwrap();
    let from_flag = resolve(&["--session-env", "RUSTC_WRAPPER"], &env, file).unwrap();

    assert_eq!(
        from_file.session_env,
        session_env(&["CARGO_HOME", "RUSTUP_HOME"])
    );
    assert_eq!(
        from_env.session_env,
        session_env(&["CARGO_*", "SCCACHE_DIR"])
    );
    assert_eq!(from_flag.session_env, session_env(&["RUSTC_WRAPPER"]));
}

#[test]
fn session_env_naming_a_credential_of_linewrights_is_an_error_saying_why() {
    for name in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "LINEWRIGHT_SESSION_GITHUB_TOKEN",
        "LINEWRIGHT_WEBHOOK_SECRET",
    ] {
        let error =
            resolve(&["--session-env", &format!("CARGO_HOME,{name}")], &[], None).unwrap_err();

        assert!(error.contains("session-env"), "{error}");
        assert!(error.contains(&format!("`{name}`")), "{error}");
        assert!(error.contains("never given"), "{error}");
    }
}

#[test]
fn a_session_env_pattern_matching_a_credential_of_linewrights_is_no_error() {
    let config = resolve(&["--session-env", "GH_*,LINEWRIGHT_*"], &[], None);

    assert!(config.is_ok());
}

#[test]
fn session_env_with_a_star_but_at_a_names_end_is_an_error_saying_why() {
    for name in ["CARGO_*_HOME", "*CARGO"] {
        let error = resolve(
            &["--session-env", &format!("RUSTUP_HOME,{name}")],
            &[],
            None,
        )
        .unwrap_err();

        assert!(
            error.contains(&format!(
                "the setting `session-env` names `{name}`: a `*` can only end a name"
            )),
            "{error}"
        );
    }
}

#[test]
fn help_lists_session_env_with_its_default() {
    let (help, line) = help_for("--session-env");
    assert!(line.contains("[default: none]"), "{help}");
}

#[test]
fn only_the_list_settings_are_lists_in_the_file() {
    let error = resolve(&[], &[], Some("repo = [\"acme/widgets\"]\n")).unwrap_err();

    assert!(error.contains("`repo`"), "{error}");
}

#[test]
fn the_web_ui_is_served_on_127_0_0_1_8081_answering_only_local_names_by_default() {
    let config = resolve(&[], &[], None).unwrap();

    let ui = config.ui.unwrap();
    assert_eq!(ui.address.to_string(), "127.0.0.1:8081");
    assert!(ui.hosts.answers(&HostName::parse("localhost").unwrap()));
    assert!(
        !ui.hosts
            .answers(&HostName::parse("linewright.example.com").unwrap())
    );
}

#[test]
fn an_empty_ui_address_serves_no_web_ui() {
    let from_flag = resolve(&["--ui-address", ""], &[], None).unwrap();
    let from_env = resolve(&[], &[("LINEWRIGHT_UI_ADDRESS", "")], None).unwrap();
    let from_file = resolve(&[], &[], Some("ui-address = \"\"\n")).unwrap();

    assert!(from_flag.ui.is_none());
    assert!(from_env.ui.is_none());
    assert!(from_file.ui.is_none());
}

#[test]
fn the_ui_address_is_a_flag_the_environment_or_the_file() {
    let address = |args: &[&str], env: &[(&str, &str)], file| {
        resolve(args, env, file)
            .unwrap()
            .ui
            .unwrap()
            .address
            .to_string()
    };
    let file = Some("ui-address = \"0.0.0.0:9000\"\n");
    let env = [("LINEWRIGHT_UI_ADDRESS", "localhost:9001")];

    assert_eq!(address(&[], &[], file), "0.0.0.0:9000");
    assert_eq!(address(&[], &env, file), "localhost:9001");
    assert_eq!(
        address(&["--ui-address", "[::1]:9002"], &env, file),
        "[::1]:9002"
    );
}

#[test]
fn a_ui_address_that_is_not_a_host_and_port_is_an_error_naming_the_setting() {
    for bad in [
        "8081",
        "localhost",
        ":8081",
        "localhost:http",
        "localhost:99999",
    ] {
        let error = resolve(&["--ui-address", bad], &[], None).unwrap_err();

        assert!(error.contains("ui-address"), "{bad}: {error}");
    }
}

/// Whether the web UI `config` sets up answers a request whose `Host` is
/// `host`.
fn answers(config: &Config, host: &str) -> bool {
    let host = HostName::parse(host).unwrap();
    config.ui.as_ref().unwrap().hosts.answers(&host)
}

#[test]
fn ui_hosts_is_a_comma_separated_flag_or_variable_or_a_list_or_string_in_the_file() {
    let list = Some("ui-hosts = [\"a.example\", \"B.example.\"]\n");
    let string = Some("ui-hosts = \"c.example\"\n");
    let env = [("LINEWRIGHT_UI_HOSTS", "d.example,e.example")];

    let from_list = resolve(&[], &[], list).unwrap();
    let from_string = resolve(&[], &[], string).unwrap();
    let from_env = resolve(&[], &env, list).unwrap();
    let from_flag = resolve(&["--ui-hosts", "f.example,g.example"], &env, list).unwrap();

    assert!(answers(&from_list, "a.example"));
    assert!(answers(&from_list, "b.example"));
    assert!(!answers(&from_list, "c.example"));
    assert!(answers(&from_string, "c.example"));
    assert!(answers(&from_env, "d.example"));
    assert!(answers(&from_env, "e.example"));
    assert!(!answers(&from_env, "a.example"));
    assert!(answers(&from_flag, "f.example"));
    assert!(answers(&from_flag, "g.example"));
    assert!(!answers(&from_flag, "d.example"));
}

#[test]
fn a_ui_hosts_entry_that_is_not_a_host_name_is_an_error_naming_the_setting() {
    for bad in [
        "a.example,",
        "a.example,,b.example",
        "a.example:8081",
        "https://a.example",
        "*.example",
        "a example",
        ".",
    ] {
        let error = resolve(&["--ui-hosts", bad], &[], None).unwrap_err();

        assert!(error.contains("ui-hosts"), "{bad:?}: {error}");
    }
    for bad in ["[\"a.example\", 7]", "[\"\"]", "[\"a.example,b.example\"]"] {
        let file = format!("ui-hosts = {bad}\n");
        let error = resolve(&[], &[], Some(&file)).unwrap_err();

        assert!(error.contains("ui-hosts"), "{bad}: {error}");
    }
}

#[test]
fn ui_hosts_empty_names_no_host() {
    let from_flag = resolve(&["--ui-hosts", ""], &[], None).unwrap();
    let from_file = resolve(&[], &[], Some("ui-hosts = []\n")).unwrap();

    assert!(!answers(&from_flag, "a.example"));
    assert!(!answers(&from_file, "a.example"));
    assert!(answers(&from_file, "localhost"));
}

#[test]
fn ui_hosts_with_no_web_ui_is_an_error_naming_both() {
    let no_ui = [("LINEWRIGHT_UI_ADDRESS", "")];
    let flag = resolve(&["--ui-address", "", "--ui-hosts", "a.example"], &[], None).unwrap_err();
    let env = resolve(&[], &[no_ui[0], ("LINEWRIGHT_UI_HOSTS", "a.example")], None).unwrap_err();
    let file = resolve(&[], &no_ui, Some("ui-hosts = [\"a.example\"]\n")).unwrap_err();

    for error in [flag, env, file] {
        assert_eq!(
            error,
            "the setting `ui-hosts` is set, but `ui-address` is empty: \
             set `ui-address` to serve the web UI, or unset `ui-hosts`"
        );
    }
}

#[test]
fn ui_hosts_naming_no_host_is_at_its_default_so_allowed_with_no_web_ui() {
    let no_ui = [("LINEWRIGHT_UI_ADDRESS", "")];
    let flag = resolve(&["--ui-hosts", ""], &no_ui, None);
    let env = resolve(&[], &[no_ui[0], ("LINEWRIGHT_UI_HOSTS", "")], None);
    let file = resolve(&[], &[], Some("ui-address = \"\"\nui-hosts = []\n"));

    for config in [flag, env, file] {
        assert!(config.unwrap().ui.is_none());
    }
}

#[test]
fn a_malformed_ui_hosts_with_no_web_ui_is_an_error_naming_what_is_wrong() {
    let error = resolve(
        &["--ui-hosts", "a/b"],
        &[("LINEWRIGHT_UI_ADDRESS", "")],
        None,
    )
    .unwrap_err();

    assert!(error.contains("names \"a/b\""), "{error}");
}

#[test]
fn a_malformed_merge_method_or_webhook_address_for_a_feature_that_is_off_is_an_error_naming_it() {
    let merge_method = resolve(&["--merge-method", "fast-forward"], &[], None).unwrap_err();
    let webhook_address = resolve(&["--webhook-address", "8080"], &[], None).unwrap_err();

    assert!(merge_method.contains("\"fast-forward\""), "{merge_method}");
    assert!(webhook_address.contains("\"8080\""), "{webhook_address}");
}

#[test]
fn ui_hosts_with_no_web_ui_is_blamed_on_the_command_line_only_when_a_flag_emptied_ui_address() {
    let hosts = [("LINEWRIGHT_UI_HOSTS", "a.example")];

    assert!(blames_the_command_line(&["--ui-address", ""], &hosts, None));
    assert!(!blames_the_command_line(
        &["--ui-hosts", "a.example"],
        &[("LINEWRIGHT_UI_ADDRESS", "")],
        None
    ));
}

#[test]
fn help_lists_the_ui_address_with_its_default() {
    let (help, line) = help_for("--ui-address");
    assert!(line.contains("127.0.0.1:8081"), "{help}");
}
