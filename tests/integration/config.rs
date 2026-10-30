use std::collections::HashMap;
use std::time::Duration;

use clap::{CommandFactory, Parser};
use linewright::automation;
use linewright::config::{Cli, Config, LogFormat, ResolveError};
use linewright::domain::{AutoMerge, MergeMethod};
use linewright::host::HostName;
use linewright::layer::{Layer, Source, Sourced};
use linewright::reconcile::WorkKind;
use linewright::session_env::{PassEnv, SessionEnv, SetEnv};
use linewright::variable_names::VariableNames;
use linewright::work_settings::{
    Effort, Environments, RepoSettings, RunKind, SessionSettings, Setting, Setup, Split, StageSet,
    TimeoutSetting,
};

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

/// What a triage run's sessions run with under `config`.
fn session(config: &Config) -> SessionSettings {
    stage_session(config, WorkKind::Triage)
}

/// What a `stage` run's sessions run with under `config`.
fn stage_session(config: &Config, stage: WorkKind) -> SessionSettings {
    config.work.for_run(
        RunKind::Stage(stage),
        &RepoSettings::default(),
        &Environments::default(),
    )
}

#[test]
fn a_stages_table_replaces_the_whole_pipelines_model_effort_and_session_timeout_for_that_stage_only()
 {
    let file = "model = \"sonnet\"\neffort = \"low\"\nsession-timeout = \"10m\"\n\n\
                [stage.implement]\nmodel = \"opus\"\neffort = \"max\"\nsession-timeout = \"2h\"\n";

    let config = resolve(&[], &[], Some(file)).unwrap();

    let implement = stage_session(&config, WorkKind::Implement);
    assert_eq!(
        implement.model,
        Some(Sourced::new("opus".into(), Layer::Operator))
    );
    assert_eq!(
        implement.effort,
        Some(Sourced::new(Effort::Max, Layer::Operator))
    );
    assert_eq!(implement.session_timeout.value, Duration::from_hours(2));
    assert_eq!(
        implement.timeout_setting,
        TimeoutSetting::Stage(WorkKind::Implement)
    );
    let design = stage_session(&config, WorkKind::Design);
    assert_eq!(
        design.model.map(|model| model.value).as_deref(),
        Some("sonnet")
    );
    assert_eq!(design.effort.map(|effort| effort.value), Some(Effort::Low));
    assert_eq!(design.session_timeout.value, Duration::from_mins(10));
    assert_eq!(design.timeout_setting, TimeoutSetting::Pipeline);
}

#[test]
fn a_stages_table_wins_over_the_whole_pipelines_flag() {
    let config = resolve(
        &["--model", "sonnet", "--effort", "high"],
        &[("LINEWRIGHT_SESSION_TIMEOUT", "10m")],
        Some("[stage.fix-ci]\nmodel = \"haiku\"\neffort = \"medium\"\nsession-timeout = \"5m\"\n"),
    )
    .unwrap();

    let fix_ci = stage_session(&config, WorkKind::FixCi);
    assert_eq!(
        fix_ci.model.map(|model| model.value).as_deref(),
        Some("haiku")
    );
    assert_eq!(
        fix_ci.effort.map(|effort| effort.value),
        Some(Effort::Medium)
    );
    assert_eq!(fix_ci.session_timeout.value, Duration::from_mins(5));
}

#[test]
fn effort_is_a_flag_a_variable_or_a_key_and_unset_is_the_harnesss_choice() {
    let effort = |args: &[&str], env: &[(&str, &str)], file| {
        session(&resolve(args, env, file).unwrap()).effort
    };
    let file = Some("effort = \"low\"\n");

    assert_eq!(effort(&[], &[], None), None);
    assert_eq!(
        effort(&[], &[], file),
        Some(Sourced::new(Effort::Low, Layer::Operator))
    );
    assert_eq!(
        effort(&[], &[("LINEWRIGHT_EFFORT", "xhigh")], file).map(|effort| effort.value),
        Some(Effort::Xhigh)
    );
    assert_eq!(
        effort(
            &["--effort", "high"],
            &[("LINEWRIGHT_EFFORT", "xhigh")],
            file
        )
        .map(|effort| effort.value),
        Some(Effort::High)
    );
}

#[test]
fn an_effort_outside_its_levels_is_an_error_naming_the_setting_and_the_file_that_gave_it() {
    let in_file = resolve(&[], &[], Some("effort = \"huge\"\n")).unwrap_err();

    assert!(in_file.contains("linewright.toml"), "{in_file}");
    for error in [
        resolve(&["--effort", "huge"], &[], None).unwrap_err(),
        resolve(&[], &[("LINEWRIGHT_EFFORT", "huge")], None).unwrap_err(),
        in_file,
    ] {
        assert!(error.contains("`effort`"), "{error}");
        assert!(
            error.contains("`low`, `medium`, `high`, `xhigh` or `max`"),
            "{error}"
        );
    }
}

#[test]
fn a_mistake_in_a_stages_table_is_an_error_naming_the_file_and_the_key() {
    for (file, key) in [
        ("[stage.deploy]\nmodel = \"opus\"\n", "deploy"),
        ("[stage.implement]\nmax-runs = 2\n", "max-runs"),
        ("[stage.implement]\neffort = \"huge\"\n", "effort"),
        (
            "[stage.implement]\nsession-timeout = \"long\"\n",
            "session-timeout",
        ),
        (
            "[stage.implement]\nsession-pass-env = [\"GH_TOKEN\"]\n",
            "session-pass-env",
        ),
        (
            "[stage.implement]\nsession-env = { TMPDIR = \"/tmp\" }\n",
            "session-env",
        ),
        ("stage = \"implement\"\n", "stage"),
    ] {
        let error = resolve(&[], &[], Some(file)).unwrap_err();

        assert!(error.contains("linewright.toml"), "{file}: {error}");
        assert!(error.contains(key), "{file}: {error}");
        assert!(!blames_the_command_line(&[], &[], Some(file)), "{file}");
    }
}

/// What a `stage` run's sessions run with under `config` and the
/// repository's file `repo`.
fn repo_session(config: &Config, repo: &str, stage: WorkKind) -> SessionSettings {
    config.work.for_run(
        RunKind::Stage(stage),
        &RepoSettings::parse(repo).unwrap(),
        &Environments::default(),
    )
}

#[test]
fn the_repositorys_file_ranks_below_the_operators_flags_and_variables_and_above_the_operators_file()
{
    let repo = "model = \"sonnet\"\neffort = \"medium\"\nsession-timeout = \"20m\"\n\n\
                [stage.design]\neffort = \"high\"\n";
    let config = resolve(
        &["--model", "opus", "--effort", "xhigh"],
        &[],
        Some("model = \"haiku\"\nsession-timeout = \"10m\"\n\n[stage.design]\neffort = \"max\"\n"),
    )
    .unwrap();

    let design = repo_session(&config, repo, WorkKind::Design);
    assert_eq!(
        design.effort,
        Some(Sourced::new(Effort::High, Layer::Repository))
    );
    assert_eq!(
        design.model,
        Some(Sourced::new("opus".into(), Layer::Operator))
    );
    let triage = repo_session(&config, repo, WorkKind::Triage);
    assert_eq!(
        triage.effort,
        Some(Sourced::new(Effort::Xhigh, Layer::Operator))
    );
    assert_eq!(
        triage.session_timeout,
        Sourced::new(Duration::from_mins(20), Layer::Repository)
    );
    assert_eq!(triage.timeout_setting, TimeoutSetting::Pipeline);
}

#[test]
fn the_repositorys_stage_table_sets_its_stages_session_timeout() {
    let config = resolve(&[], &[], None).unwrap();

    let implement = repo_session(
        &config,
        "[stage.implement]\nsession-timeout = \"2h\"\nmodel = \"opus\"\n",
        WorkKind::Implement,
    );

    assert_eq!(
        implement.session_timeout,
        Sourced::new(Duration::from_hours(2), Layer::Repository)
    );
    assert_eq!(
        implement.timeout_setting,
        TimeoutSetting::Stage(WorkKind::Implement)
    );
    assert_eq!(
        implement.model,
        Some(Sourced::new("opus".into(), Layer::Repository))
    );
}

#[test]
fn a_key_of_the_operators_in_the_repositorys_file_is_an_error_saying_it_is_the_operators() {
    for (file, key) in [
        ("session-env = { RUST_LOG = \"info\" }\n", "session-env"),
        ("session-pass-env = [\"GH_TOKEN\"]\n", "session-pass-env"),
        ("max-runs = 9\n", "max-runs"),
        ("secrets = \"KEY\"\n", "secrets"),
        (
            "[stage.implement]\nsession-pass-env = [\"DEPLOY_KEY\"]\n",
            "session-pass-env",
        ),
        (
            "[stage.implement]\nsession-env = { RUST_LOG = \"info\" }\n",
            "session-env",
        ),
    ] {
        let error = format!("{:#}", RepoSettings::parse(file).unwrap_err());

        assert!(error.contains(&format!("`{key}`")), "{file}: {error}");
        assert!(error.contains("operator's"), "{file}: {error}");
    }
}

#[test]
fn a_mistake_in_the_repositorys_file_is_an_error_naming_the_key() {
    for (file, named) in [
        ("colour = \"blue\"\n", "`colour`"),
        ("[stage.deploy]\nmodel = \"opus\"\n", "deploy"),
        ("effort = \"huge\"\n", "`effort`"),
        (
            "[stage.design]\nsession-timeout = \"long\"\n",
            "`session-timeout`",
        ),
        ("model = [\"opus\"]\n", "`model`"),
        ("model = \"opus\n", "does not parse"),
    ] {
        let error = format!("{:#}", RepoSettings::parse(file).unwrap_err());

        assert!(error.contains(named), "{file}: {error}");
        assert!(!error.contains("operator's"), "{file}: {error}");
    }
}

#[test]
fn a_stage_has_no_flag() {
    assert!(Cli::try_parse_from(["lw", "--stage.implement.model", "opus", "status"]).is_err());
}

/// The variables a `stage` run's sessions are given, by name, under
/// `config` and the environments `split` made.
fn given(config: &Config, split: &Split, stage: WorkKind) -> Vec<(String, String)> {
    let mut given: Vec<(String, String)> = config
        .work
        .for_run(
            RunKind::Stage(stage),
            &RepoSettings::default(),
            &split.environments,
        )
        .environment
        .iter()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect();
    given.sort();
    given
}

fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
    values
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn a_stages_session_pass_env_adds_its_names_to_the_whole_pipelines_for_that_stage_only() {
    let config = resolve(
        &[],
        &[],
        Some(
            "session-pass-env = [\"CARGO_HOME\"]\n\n\
             [stage.implement]\nsession-pass-env = [\"DEPLOY_KEY\"]\n",
        ),
    )
    .unwrap();
    let environment = [
        ("CARGO_HOME", "/cargo"),
        ("DEPLOY_KEY", "k"),
        ("OTHER", "o"),
    ]
    .map(|(name, value)| (name.into(), value.into()));

    let split = config
        .work
        .environments(&config.session_env, environment, &[]);

    assert_eq!(
        given(&config, &split, WorkKind::Implement),
        pairs(&[("CARGO_HOME", "/cargo"), ("DEPLOY_KEY", "k")])
    );
    assert_eq!(
        given(&config, &split, WorkKind::Rework),
        pairs(&[("CARGO_HOME", "/cargo")])
    );
    assert_eq!(split.withheld, ["DEPLOY_KEY", "OTHER"]);
    assert_eq!(
        split.added,
        [(WorkKind::Implement, vec!["DEPLOY_KEY".to_string()])].into()
    );
}

#[test]
fn a_stages_session_env_adds_its_values_to_the_whole_pipelines_for_that_stage_only() {
    let config = resolve(
        &["--session-env", "RUST_LOG=warn"],
        &[],
        Some(
            "session-pass-env = [\"CARGO_HOME\", \"RUST_LOG\"]\n\
             session-env = { CI = \"true\" }\n\n\
             [stage.implement]\nsession-env = { RUST_LOG = \"debug\", CARGO_HOME = \"/stage\" }\n",
        ),
    )
    .unwrap();
    let environment = [("CARGO_HOME", "/cargo"), ("RUST_LOG", "info")]
        .map(|(name, value)| (name.into(), value.into()));

    let split = config
        .work
        .environments(&config.session_env, environment, &[]);

    assert_eq!(
        given(&config, &split, WorkKind::Implement),
        pairs(&[
            ("CARGO_HOME", "/stage"),
            ("CI", "true"),
            ("RUST_LOG", "debug")
        ])
    );
    assert_eq!(
        given(&config, &split, WorkKind::Rework),
        pairs(&[
            ("CARGO_HOME", "/cargo"),
            ("CI", "true"),
            ("RUST_LOG", "warn")
        ])
    );
    assert_eq!(split.set, ["CI", "RUST_LOG"]);
    assert_eq!(
        split.stages_set,
        [(
            WorkKind::Implement,
            StageSet {
                own: vec!["CARGO_HOME".into(), "RUST_LOG".into()],
                taken: vec!["CI".into()],
            }
        )]
        .into()
    );
}

#[test]
fn an_automations_model_and_effort_come_before_the_whole_pipelines() {
    let config = resolve(&["--model", "sonnet", "--effort", "low"], &[], None).unwrap();
    let automation = automation::parse(
        &"weekly".parse().unwrap(),
        "---\non:\n  schedule: \"0 9 * * 1\"\nmodel: opus\neffort: xhigh\n---\nLook around.\n",
    )
    .unwrap();

    let session = config.work.for_run(
        RunKind::Automation(&automation),
        &RepoSettings::default(),
        &Environments::default(),
    );

    assert_eq!(
        session.model,
        Some(Sourced::new("opus".into(), Layer::Repository))
    );
    assert_eq!(
        session.effort,
        Some(Sourced::new(Effort::Xhigh, Layer::Repository))
    );
    assert_eq!(session.timeout_setting, TimeoutSetting::Pipeline);
}

#[test]
fn a_runs_settings_name_the_operator_for_a_flag_or_the_operators_file_and_linewright_for_a_default()
{
    let from_flag = resolve(
        &["--model", "opus"],
        &[],
        Some("session-timeout = \"10m\"\n"),
    )
    .unwrap();
    let from_variable =
        resolve(&[], &[("LINEWRIGHT_SESSION_PASS_ENV", "CARGO_HOME")], None).unwrap();
    let set_in_file = resolve(&[], &[], Some("session-env = { CI = \"true\" }\n")).unwrap();
    let defaults = resolve(&[], &[], None).unwrap();

    let flagged = session(&from_flag);
    assert_eq!(
        flagged.model,
        Some(Sourced::new("opus".into(), Layer::Operator))
    );
    assert_eq!(
        flagged.session_timeout,
        Sourced::new(Duration::from_mins(10), Layer::Operator)
    );
    let sources = |config| session(config).sources();
    assert_eq!(
        (
            sources(&from_flag)[&Setting::SessionPassEnv],
            sources(&from_flag)[&Setting::SessionEnv]
        ),
        (
            Source::Layer(Layer::Linewright),
            Source::Layer(Layer::Linewright)
        )
    );
    assert_eq!(
        (
            sources(&from_variable)[&Setting::SessionPassEnv],
            sources(&from_variable)[&Setting::SessionEnv]
        ),
        (
            Source::Layer(Layer::Operator),
            Source::Layer(Layer::Linewright)
        )
    );
    assert_eq!(
        (
            sources(&set_in_file)[&Setting::SessionPassEnv],
            sources(&set_in_file)[&Setting::SessionEnv]
        ),
        (
            Source::Layer(Layer::Linewright),
            Source::Layer(Layer::Operator)
        )
    );
    let defaulted = session(&defaults);
    assert_eq!(defaulted.model, None);
    assert_eq!(
        defaulted.session_timeout,
        Sourced::new(Duration::from_hours(1), Layer::Linewright)
    );
}

#[test]
fn an_automations_session_timeout_comes_from_the_repository_before_the_operators_session_timeout() {
    let config = resolve(&["--session-timeout", "10m"], &[], None).unwrap();
    let automation = automation::parse(
        &"weekly".parse().unwrap(),
        "---\non:\n  schedule: \"0 9 * * 1\"\nsession-timeout: 5m\n---\nLook around.\n",
    )
    .unwrap();

    let session = config.work.for_run(
        RunKind::Automation(&automation),
        &RepoSettings::default(),
        &Environments::default(),
    );

    assert_eq!(
        session.session_timeout,
        Sourced::new(Duration::from_mins(5), Layer::Repository)
    );
    assert_eq!(session.timeout_setting, TimeoutSetting::Automation);
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
    assert_eq!(
        session(&config).session_timeout.value,
        Duration::from_hours(1)
    );
    assert_eq!(config.spec_glob.pattern(), "docs/**/*.md");
}

#[test]
fn the_done_window_is_read_from_the_file_apart_from_the_spec_lookback() {
    let file = "repo = \"acme/widgets\"\ndone-window = \"30d\"\n";

    let config = resolve(&[], &[], Some(file)).unwrap();

    assert_eq!(config.done_window, chrono::TimeDelta::days(30));
    assert_eq!(config.spec_lookback, chrono::TimeDelta::days(7));
}

#[test]
fn defaults_follow_the_spec() {
    let config = resolve(&["--repo", "acme/widgets"], &[], None).unwrap();

    assert_eq!(config.poll_interval, Duration::from_mins(1));
    assert_eq!(config.spec_lookback, chrono::TimeDelta::days(7));
    assert_eq!(config.done_window, chrono::TimeDelta::days(7));
    assert_eq!(config.max_runs, 4);
    assert_eq!(
        session(&config).session_timeout.value,
        Duration::from_hours(1)
    );
    assert_eq!(config.result_retries, 1);
    assert_eq!(config.ci_grace, chrono::TimeDelta::minutes(5));
    assert_eq!(config.ci_timeout, chrono::TimeDelta::hours(6));
    assert_eq!(config.ci_attempts, 2);
    assert_eq!(config.update_branch, None);
    assert_eq!(session(&config).model, None);
    assert_eq!(config.log_format, LogFormat::Json, "not a terminal");
    assert!(config.work_dir.ends_with("linewright"));
    assert_eq!(config.session_env, SessionEnv::default());
    assert_eq!(config.secrets, VariableNames::default());
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

#[test]
fn the_operators_layer_of_prompts_is_the_prompts_setting_from_a_flag_variable_or_the_file() {
    let from_flag = resolve(&["--prompts", "flag/"], &[], None).unwrap();
    let from_env = resolve(&[], &[("LINEWRIGHT_PROMPTS", "env/")], None).unwrap();
    let from_file = resolve(&[], &[], Some("prompts = \"file/\"\n")).unwrap();

    assert_eq!(from_flag.prompts, Some("flag/".into()));
    assert_eq!(from_env.prompts, Some("env/".into()));
    assert_eq!(from_file.prompts, Some("file/".into()));
}

#[test]
fn the_earlier_briefs_key_is_an_unknown_setting() {
    let error = resolve(&[], &[], Some("briefs = \"mine/\"\n")).unwrap_err();

    assert!(error.contains("an unknown setting `briefs`"), "{error}");
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

fn pass_env(names: &[&str]) -> PassEnv {
    PassEnv::new(names.iter().map(ToString::to_string)).unwrap()
}

fn set_env(values: &[(&str, &str)]) -> SetEnv {
    SetEnv::new(
        values
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string())),
    )
    .unwrap()
}

fn session_env(names: &[&str], values: &[(&str, &str)]) -> SessionEnv {
    SessionEnv::new(pass_env(names), set_env(values))
}

#[test]
fn session_pass_env_joins_the_names_of_the_flag_the_environment_and_the_file() {
    let file = Some("session-pass-env = [\"CARGO_HOME\", \"RUSTUP_HOME\"]\n");
    let env = [("LINEWRIGHT_SESSION_PASS_ENV", "CARGO_*,SCCACHE_DIR")];

    let from_file = resolve(&[], &[], file).unwrap();
    let joined = resolve(&["--session-pass-env", "RUSTC_WRAPPER"], &env, file).unwrap();

    assert_eq!(
        from_file.session_env,
        session_env(&["CARGO_HOME", "RUSTUP_HOME"], &[])
    );
    let given = joined.session_env.split(
        [
            "RUSTC_WRAPPER",
            "CARGO_TARGET_DIR",
            "SCCACHE_DIR",
            "RUSTUP_HOME",
            "OTHER",
        ]
        .map(|name| (name.into(), "v".into())),
        &[],
    );
    assert_eq!(given.withheld, ["OTHER"]);
}

#[test]
fn session_env_sets_each_name_from_the_flag_over_the_file() {
    let config = resolve(
        &["--session-env", "A=1"],
        &[],
        Some("session-env = { B = \"2\", A = \"3\" }\n"),
    )
    .unwrap();

    assert_eq!(
        config.session_env,
        session_env(&[], &[("A", "1"), ("B", "2")])
    );
}

#[test]
fn session_env_takes_its_values_as_written() {
    let config = resolve(
        &["--session-env", "URL=a=b,c", "--session-env", "EMPTY="],
        &[],
        Some("session-env = { HOME_TOO = \"$HOME\", LINES = \"one\\ntwo\" }\n"),
    )
    .unwrap();

    assert_eq!(
        config.session_env,
        session_env(
            &[],
            &[
                ("URL", "a=b,c"),
                ("EMPTY", ""),
                ("HOME_TOO", "$HOME"),
                ("LINES", "one\ntwo"),
            ]
        )
    );
}

#[test]
fn session_env_has_no_environment_variable() {
    let config = resolve(&[], &[("LINEWRIGHT_SESSION_ENV", "A=1")], None).unwrap();

    assert_eq!(config.session_env, SessionEnv::default());
}

#[test]
fn a_mistake_in_session_envs_flag_is_a_command_line_error_naming_the_setting() {
    for (entries, says) in [
        (&["CARGO_HOME"][..], "`session-pass-env`"),
        (&["A=1", "A=2"][..], "`A` twice"),
        (&["=1"][..], "empty name"),
        (&["A=x\0y"][..], "NUL"),
        (&["TMPDIR=/tmp"][..], "`TMPDIR`"),
    ] {
        let args: Vec<&str> = entries
            .iter()
            .flat_map(|entry| ["--session-env", entry])
            .collect();

        let error = resolve(&args, &[], None).unwrap_err();

        assert!(error.contains("`session-env`"), "{entries:?}: {error}");
        assert!(error.contains(says), "{entries:?}: {error}");
        assert!(blames_the_command_line(&args, &[], None), "{entries:?}");
    }
}

#[test]
fn a_mistake_in_session_env_in_the_file_is_an_error_naming_the_file_and_the_setting() {
    for (file, says) in [
        ("session-env = [\"CARGO_HOME\"]\n", "`session-pass-env`"),
        ("session-env = { \"\" = \"1\" }\n", "empty name"),
        ("session-env = { \"A=B\" = \"1\" }\n", "`=`"),
        ("session-env = { A = \"x\\u0000y\" }\n", "NUL"),
        ("session-env = { \"A\\u0000\" = \"y\" }\n", "NUL"),
        ("session-env = { A = 1 }\n", "string"),
        ("session-env = \"A=1\"\n", "`session-env`"),
        ("[stage.implement]\nsession-env = { A = 1 }\n", "string"),
        (
            "[stage.implement]\nsession-env = [\"A\"]\n",
            "`session-pass-env`",
        ),
    ] {
        let error = resolve(&[], &[], Some(file)).unwrap_err();

        assert!(error.contains("linewright.toml"), "{file}: {error}");
        assert!(error.contains("`session-env`"), "{file}: {error}");
        assert!(error.contains(says), "{file}: {error}");
        assert!(!blames_the_command_line(&[], &[], Some(file)), "{file}");
    }
}

#[test]
fn session_env_setting_a_variable_linewright_may_set_itself_is_an_error() {
    for name in [
        "TMPDIR",
        "CLAUDE_CODE_SHELL",
        "DISABLE_UPDATES",
        "CLAUDE_CODE_EFFORT_LEVEL",
        "GH_TOKEN",
        "GITHUB_TOKEN",
    ] {
        let error = resolve(
            &[],
            &[],
            Some(&format!("session-env = {{ {name} = \"x\" }}\n")),
        )
        .unwrap_err();

        assert!(error.contains(&format!("`{name}`")), "{error}");
        assert!(error.contains("Linewright sets"), "{error}");
    }
}

#[test]
fn session_pass_env_naming_a_credential_of_linewrights_is_an_error_saying_why() {
    for name in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "LINEWRIGHT_SESSION_GITHUB_TOKEN",
        "LINEWRIGHT_WEBHOOK_SECRET",
    ] {
        let error = resolve(
            &["--session-pass-env", &format!("CARGO_HOME,{name}")],
            &[],
            None,
        )
        .unwrap_err();

        assert!(error.contains("session-pass-env"), "{error}");
        assert!(error.contains(&format!("`{name}`")), "{error}");
        assert!(error.contains("never given"), "{error}");
    }
}

#[test]
fn a_wrong_name_in_session_pass_env_is_blamed_on_the_layer_that_gave_it() {
    let flag = ["--session-pass-env", "CARGO_HOME"];
    let bad_env = [("LINEWRIGHT_SESSION_PASS_ENV", "GH_TOKEN")];
    let bad_file = Some("session-pass-env = [\"RUSTUP_HOME\", \"GH_TOKEN\"]\n");

    assert!(blames_the_command_line(
        &["--session-pass-env", "GH_TOKEN"],
        &[],
        bad_file
    ));
    assert!(!blames_the_command_line(&flag, &bad_env, None));
    assert!(!blames_the_command_line(&flag, &[], bad_file));
    let error = resolve(&flag, &[], bad_file).unwrap_err();
    assert!(error.contains("linewright.toml"), "{error}");
    assert!(error.contains("`GH_TOKEN`"), "{error}");
}

#[test]
fn a_session_pass_env_pattern_matching_a_credential_of_linewrights_is_no_error() {
    let config = resolve(&["--session-pass-env", "GH_*,LINEWRIGHT_*"], &[], None);

    assert!(config.is_ok());
}

#[test]
fn session_pass_env_with_a_star_but_at_a_names_end_is_an_error_saying_why() {
    for name in ["CARGO_*_HOME", "*CARGO"] {
        let error = resolve(
            &["--session-pass-env", &format!("RUSTUP_HOME,{name}")],
            &[],
            None,
        )
        .unwrap_err();

        assert!(
            error.contains(&format!(
                "the setting `session-pass-env` names `{name}`: a `*` can only end a name"
            )),
            "{error}"
        );
    }
}

#[test]
fn help_lists_session_env_and_session_pass_env_with_their_forms() {
    for (flag, form) in [
        ("--session-env", "--session-env <NAME=VALUE>"),
        ("--session-pass-env", "--session-pass-env <NAMES>"),
    ] {
        let (help, line) = help_for(flag);
        assert!(line.contains(form), "{help}");
        assert!(line.contains("[default: none]"), "{help}");
    }
}

fn secrets(names: &[&str]) -> VariableNames {
    VariableNames::new("secrets", names.iter().map(ToString::to_string)).unwrap()
}

#[test]
fn secrets_joins_the_names_of_the_flag_the_environment_and_the_file() {
    let file = Some("secrets = [\"AWS_SECRET_ACCESS_KEY\", \"NPM_*\"]\n");
    let env = [("LINEWRIGHT_SECRETS", "SENTRY_*,DATABASE_URL")];

    let from_file = resolve(&[], &[], file).unwrap();
    let from_env = resolve(&[], &env, None).unwrap();
    let joined = resolve(&["--secrets", "SIGNING_KEY"], &env, file).unwrap();

    assert_eq!(
        from_file.secrets,
        secrets(&["AWS_SECRET_ACCESS_KEY", "NPM_*"])
    );
    assert_eq!(from_env.secrets, secrets(&["SENTRY_*", "DATABASE_URL"]));
    assert_eq!(
        joined.secrets,
        secrets(&[
            "SIGNING_KEY",
            "SENTRY_*",
            "DATABASE_URL",
            "AWS_SECRET_ACCESS_KEY",
            "NPM_*"
        ])
    );
}

#[test]
fn a_secret_named_at_two_layers_counts_once() {
    let joined = resolve(
        &["--secrets", "A,B"],
        &[("LINEWRIGHT_SECRETS", "B")],
        Some("secrets = [\"A\", \"C\"]\n"),
    )
    .unwrap();

    assert_eq!(joined.secrets, secrets(&["A", "B", "C"]));
}

#[test]
fn a_secret_that_is_not_a_name_is_an_error_naming_the_layer_that_gave_it() {
    let file = Some("secrets = [\"*TOKEN\"]\n");

    let error = resolve(&["--secrets", "A"], &[], file).unwrap_err();

    assert!(error.contains("linewright.toml"), "{error}");
}

#[test]
fn secrets_naming_a_variable_that_is_unset_is_no_error_resolving_the_config() {
    let config = resolve(&["--secrets", "AWS_SECRET_ACCESS_KEY"], &[], None);

    assert!(config.is_ok());
}

#[test]
fn secrets_with_a_star_but_at_a_names_end_is_an_error_saying_why() {
    for name in ["NPM_*_TOKEN", "*TOKEN"] {
        let error = resolve(&["--secrets", &format!("NPM_TOKEN,{name}")], &[], None).unwrap_err();

        assert!(
            error.contains(&format!(
                "the setting `secrets` names `{name}`: a `*` can only end a name"
            )),
            "{error}"
        );
    }
}

#[test]
fn help_lists_secrets_with_its_default() {
    let (help, line) = help_for("--secrets");
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
    let from_env = resolve(&[], &env, None).unwrap();
    let from_flag = resolve(&["--ui-hosts", "f.example,g.example"], &[], None).unwrap();

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
fn ui_hosts_joins_the_hosts_of_the_flag_the_environment_and_the_file() {
    let file = Some("ui-hosts = [\"a.example\", \"b.example\"]\n");
    let env = [("LINEWRIGHT_UI_HOSTS", "b.example,c.example")];

    let from_env_and_file = resolve(&[], &env, file).unwrap();
    let joined = resolve(&["--ui-hosts", "d.example"], &env, file).unwrap();

    for host in ["a.example", "b.example", "c.example"] {
        assert!(answers(&from_env_and_file, host), "{host}");
    }
    assert!(!answers(&from_env_and_file, "d.example"));
    for host in ["a.example", "b.example", "c.example", "d.example"] {
        assert!(answers(&joined, host), "{host}");
    }
}

#[test]
fn a_ui_hosts_entry_that_is_not_a_host_name_is_an_error_naming_the_layer_that_gave_it() {
    let file = Some("ui-hosts = [\"a.example:8081\"]\n");

    let error = resolve(&["--ui-hosts", "b.example"], &[], file).unwrap_err();

    assert!(error.contains("ui-hosts"), "{error}");
    assert!(error.contains("linewright.toml"), "{error}");
}

#[test]
fn a_ui_hosts_entry_that_is_not_a_host_name_is_an_error_naming_the_setting() {
    for bad in [
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
fn ui_hosts_in_the_file_with_no_web_ui_is_an_error_though_the_flag_names_no_host() {
    let error = resolve(
        &["--ui-address", "", "--ui-hosts", ""],
        &[],
        Some("ui-hosts = [\"a.example\"]\n"),
    )
    .unwrap_err();

    assert!(
        error.contains("the setting `ui-hosts` is set, but `ui-address` is empty"),
        "{error}"
    );
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

#[test]
fn setup_is_a_flag_a_variable_or_a_key_and_unset_runs_nothing() {
    let setup = |args: &[&str], env: &[(&str, &str)], file| {
        session(&resolve(args, env, file).unwrap()).setup
    };
    let file = Some("setup = \"make deps\"\n");
    let command = |text: &str| Setup::from(text.to_string());

    assert_eq!(
        setup(&[], &[], None),
        Sourced::new(Setup::Empty, Layer::Linewright)
    );
    assert_eq!(
        setup(&[], &[], file),
        Sourced::new(command("make deps"), Layer::Operator)
    );
    assert_eq!(
        setup(&[], &[("LINEWRIGHT_SETUP", "npm ci")], file).value,
        command("npm ci")
    );
    assert_eq!(
        setup(
            &["--setup", "cargo fetch"],
            &[("LINEWRIGHT_SETUP", "npm ci")],
            file
        )
        .value,
        command("cargo fetch")
    );
    assert_eq!(setup(&["--setup", ""], &[], file).value, Setup::Empty);
    assert_eq!(
        session(&resolve(&[], &[], file).unwrap()).sources()[&Setting::SETUP],
        Source::Layer(Layer::Operator)
    );
}

#[test]
fn a_stages_setup_replaces_the_whole_pipelines_and_an_empty_one_runs_nothing() {
    let config = resolve(
        &[],
        &[],
        Some(
            "setup = \"make deps\"\n\n[stage.implement]\nsetup = \"make all\"\n\n\
             [stage.design]\nsetup = \"\"\n",
        ),
    )
    .unwrap();

    assert_eq!(
        stage_session(&config, WorkKind::Implement).setup,
        Sourced::new(Setup::from("make all".to_string()), Layer::Operator)
    );
    assert_eq!(
        stage_session(&config, WorkKind::Design).setup,
        Sourced::new(Setup::Empty, Layer::Operator)
    );
    assert_eq!(
        stage_session(&config, WorkKind::Triage).setup.value,
        Setup::from("make deps".to_string())
    );
}

#[test]
fn the_repositorys_file_sets_setup_for_the_whole_pipeline_and_in_its_stage_tables() {
    let config = resolve(&[], &[], Some("setup = \"make deps\"\n")).unwrap();
    let repo = "setup = \"npm ci\"\n\n[stage.update]\nsetup = \"\"\n";

    assert_eq!(
        repo_session(&config, repo, WorkKind::Rework).setup,
        Sourced::new(Setup::from("npm ci".to_string()), Layer::Repository)
    );
    assert_eq!(
        repo_session(&config, repo, WorkKind::Update).setup,
        Sourced::new(Setup::Empty, Layer::Repository)
    );
}

#[test]
fn an_unknown_key_in_a_stages_table_is_refused_with_the_keys_it_holds() {
    let error = resolve(
        &[],
        &[],
        Some("[stage.implement]\nsetup-command = \"make\"\n"),
    )
    .unwrap_err();

    assert!(error.contains("`setup-command`"), "{error}");
    assert!(
        error.contains(
            "`model`, `effort`, `session-timeout`, `setup`, `session-env`, `session-pass-env`"
        ),
        "{error}"
    );
}

#[test]
fn an_automations_setup_comes_before_the_stages_and_the_pipelines() {
    let config = resolve(&["--setup", "make deps"], &[], None).unwrap();
    let parse = |front: &str| {
        automation::parse(
            &"weekly".parse().unwrap(),
            &format!("---\non:\n  schedule: \"0 9 * * 1\"\n{front}---\nLook around.\n"),
        )
        .unwrap()
    };
    let setup = |automation| {
        config
            .work
            .for_run(
                RunKind::Automation(&automation),
                &RepoSettings::parse("setup = \"npm ci\"\n").unwrap(),
                &Environments::default(),
            )
            .setup
    };

    assert_eq!(
        setup(parse("setup: make lint\n")),
        Sourced::new(Setup::from("make lint".to_string()), Layer::Repository)
    );
    assert_eq!(setup(parse("setup: \"\"\n")).value, Setup::Empty);
    assert_eq!(setup(parse("")).value, Setup::from("make deps".to_string()));
}
