use std::collections::HashMap;
use std::time::Duration;

use crate::common::model;
use clap::{CommandFactory, Parser};
use linewright::access::providers::Providers;
use linewright::automation;
use linewright::config::{Cli, Config, LogFormat, ResolveError};
use linewright::domain::MergeMethod;
use linewright::layer::{Layer, Sourced};
use linewright::reconcile::WorkKind;
use linewright::session_env::{PassEnv, SessionEnv, SetEnv};
use linewright::variable_names::VariableNames;
use linewright::work_settings::{
    Effort, Environments, Maintenance, Partial, RepoSettings, RunKind, SessionSettings,
    SessionWork, Setting, SetupCommand, SetupSettings, Split, StageKey, StageSet, TimeoutSetting,
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

/// The operator's settings for sessions under `config`, its file's model
/// `opus` where its layers set none.
fn sessions(config: &Config) -> SessionWork {
    let mut work = config.work.clone();
    if work.flags_and_env.model.is_none() {
        work.file.model.get_or_insert_with(|| model("opus"));
    }
    work.for_sessions()
        .expect("the operator's layers set a model")
}

/// What a triage run's sessions run with under `config`.
fn session(config: &Config) -> SessionSettings {
    stage_session(config, WorkKind::Triage)
}

/// What a `stage` run's sessions run with under `config`.
fn stage_session(config: &Config, stage: WorkKind) -> SessionSettings {
    sessions(config).for_run(
        RunKind::Stage(stage),
        &RepoSettings::default(),
        &Environments::default(),
    )
}

#[test_log::test]
fn a_stages_table_replaces_the_whole_pipelines_model_effort_and_session_timeout_for_that_stage_only()
 {
    let file = "model = \"sonnet\"\neffort = \"low\"\nsession-timeout = \"10m\"\n\n\
                [stage.implement]\nmodel = \"opus\"\neffort = \"max\"\nsession-timeout = \"2h\"\n";

    let config = resolve(&[], &[], Some(file)).unwrap();

    let implement = stage_session(&config, WorkKind::Implement);
    assert_eq!(
        implement.model,
        Sourced::new(model("opus"), Layer::Operator)
    );
    assert_eq!(implement.effort, Sourced::new(Effort::Max, Layer::Operator));
    assert_eq!(implement.session_timeout.value, Duration::from_hours(2));
    assert_eq!(
        implement.timeout_setting,
        TimeoutSetting::Stage(StageKey::Work(WorkKind::Implement))
    );
    let design = stage_session(&config, WorkKind::Design);
    assert_eq!(design.model.value, model("sonnet"));
    assert_eq!(design.effort.value, Effort::Low);
    assert_eq!(design.session_timeout.value, Duration::from_mins(10));
    assert_eq!(design.timeout_setting, TimeoutSetting::Pipeline);
}

#[test_log::test]
fn a_configuration_replaces_what_it_sets_over_a_stages_table_and_leaves_the_rest() {
    let config = resolve(
        &["--model", "sonnet", "--effort", "high"],
        &[],
        Some("[stage.triage]\nmodel = \"haiku\"\nsession-timeout = \"5m\"\n"),
    )
    .unwrap();
    let by = Partial {
        model: Some(model("opus")),
        effort: None,
        session_timeout: Some(Duration::from_hours(2)),
    };

    let overridden = session(&config).overridden(&by);

    assert_eq!(
        overridden.model,
        Sourced::new(model("opus"), Layer::Configuration)
    );
    assert_eq!(
        overridden.effort,
        Sourced::new(Effort::High, Layer::Operator)
    );
    assert_eq!(
        overridden.session_timeout,
        Sourced::new(Duration::from_hours(2), Layer::Configuration)
    );
    assert_eq!(
        overridden.timeout_setting.to_string(),
        "`session-timeout` in the configuration"
    );
    assert_eq!(overridden.sources()[&Setting::MODEL], Layer::Configuration);
}

#[test_log::test]
fn a_configuration_that_sets_nothing_changes_nothing() {
    let config = resolve(
        &["--model", "sonnet"],
        &[],
        Some("[stage.triage]\nmodel = \"haiku\"\nsession-timeout = \"5m\"\n"),
    )
    .unwrap();

    let overridden = session(&config).overridden(&Partial::default());

    assert_eq!(
        overridden.model,
        Sourced::new(model("haiku"), Layer::Operator)
    );
    assert_eq!(
        overridden.effort,
        Sourced::new(Effort::Medium, Layer::Linewright)
    );
    assert_eq!(
        overridden.session_timeout,
        Sourced::new(Duration::from_mins(5), Layer::Operator)
    );
    assert_eq!(
        overridden.timeout_setting,
        TimeoutSetting::Stage(StageKey::Work(WorkKind::Triage))
    );
}

#[test_log::test]
fn a_stages_table_wins_over_the_whole_pipelines_flag() {
    let config = resolve(
        &["--model", "sonnet", "--effort", "high"],
        &[("LINEWRIGHT_SESSION_TIMEOUT", "10m")],
        Some("[stage.fix-ci]\nmodel = \"haiku\"\neffort = \"medium\"\nsession-timeout = \"5m\"\n"),
    )
    .unwrap();

    let fix_ci = stage_session(&config, WorkKind::FixCi);
    assert_eq!(fix_ci.model.value, model("haiku"));
    assert_eq!(fix_ci.effort.value, Effort::Medium);
    assert_eq!(fix_ci.session_timeout.value, Duration::from_mins(5));
}

#[test_log::test]
fn without_a_model_in_the_operators_layers_sessions_cannot_start_and_the_error_says_where_to_set_it()
 {
    let config = resolve(&[], &[], Some("[stage.triage]\nmodel = \"haiku\"\n")).unwrap();

    let error = config.work.for_sessions().unwrap_err().to_string();

    assert_eq!(
        error,
        "`model` is required to start a session: set it with `--model`, `LINEWRIGHT_MODEL` \
         or `model` in the operator's config file"
    );
}

#[test_log::test]
fn the_operators_model_is_a_flag_a_variable_or_a_key() {
    let model_of = |args: &[&str], env: &[(&str, &str)], file| {
        session(&resolve(args, env, file).unwrap()).model
    };
    let file = Some("model = \"haiku\"\n");

    assert_eq!(
        model_of(&[], &[], file),
        Sourced::new(model("haiku"), Layer::Operator)
    );
    assert_eq!(
        model_of(&[], &[("LINEWRIGHT_MODEL", "sonnet")], file).value,
        model("sonnet")
    );
    assert_eq!(
        model_of(
            &["--model", "opus"],
            &[("LINEWRIGHT_MODEL", "sonnet")],
            file
        )
        .value,
        model("opus")
    );
}

#[test_log::test]
fn an_empty_model_is_an_error_naming_the_setting_and_where_it_was_given() {
    let in_file = resolve(&[], &[], Some("model = \"\"\n")).unwrap_err();
    let in_stage = resolve(&[], &[], Some("[stage.triage]\nmodel = \"\"\n")).unwrap_err();
    let in_repo = format!("{:#}", RepoSettings::parse("model = \"\"\n").unwrap_err());

    assert!(in_file.contains("linewright.toml"), "{in_file}");
    assert!(in_stage.contains("linewright.toml"), "{in_stage}");
    for error in [
        resolve(&["--model", ""], &[], None).unwrap_err(),
        resolve(&[], &[("LINEWRIGHT_MODEL", "")], None).unwrap_err(),
        resolve(&[], &[("LINEWRIGHT_MODEL", " ")], None).unwrap_err(),
        in_file,
        in_stage,
        in_repo,
    ] {
        assert!(error.contains("the setting `model` is empty"), "{error}");
    }
}

#[test_log::test]
fn effort_is_a_flag_a_variable_or_a_key_and_unset_is_linewrights_medium() {
    let effort = |args: &[&str], env: &[(&str, &str)], file| {
        let args: Vec<&str> = ["--model", "opus"].iter().chain(args).copied().collect();
        session(&resolve(&args, env, file).unwrap()).effort
    };
    let file = Some("effort = \"low\"\n");

    assert_eq!(
        effort(&[], &[], None),
        Sourced::new(Effort::Medium, Layer::Linewright)
    );
    assert_eq!(
        effort(&[], &[], file),
        Sourced::new(Effort::Low, Layer::Operator)
    );
    assert_eq!(
        effort(&[], &[("LINEWRIGHT_EFFORT", "xhigh")], file).value,
        Effort::Xhigh
    );
    assert_eq!(
        effort(
            &["--effort", "high"],
            &[("LINEWRIGHT_EFFORT", "xhigh")],
            file
        )
        .value,
        Effort::High
    );
}

#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
fn the_scorers_table_sets_the_scorers_sessions_and_no_stages() {
    let file = "model = \"sonnet\"\neffort = \"low\"\nsession-timeout = \"10m\"\n\n\
                [stage.scorer]\nmodel = \"opus\"\neffort = \"max\"\nsession-timeout = \"2h\"\n";

    let config = resolve(&["--model", "haiku"], &[], Some(file)).unwrap();

    let scorer = sessions(&config).for_run(
        RunKind::Scorer,
        &RepoSettings::default(),
        &Environments::default(),
    );
    assert_eq!(scorer.model, Sourced::new(model("opus"), Layer::Operator));
    assert_eq!(scorer.effort, Sourced::new(Effort::Max, Layer::Operator));
    assert_eq!(scorer.session_timeout.value, Duration::from_hours(2));
    assert_eq!(
        scorer.timeout_setting,
        TimeoutSetting::Stage(StageKey::Scorer)
    );
    assert_eq!(
        scorer.timeout_setting.to_string(),
        "`session-timeout` in `[stage.scorer]`"
    );
    for stage in WorkKind::ALL {
        let session = stage_session(&config, stage);
        assert_eq!(session.model.value, model("haiku"));
        assert_eq!(session.effort.value, Effort::Low);
        assert_eq!(session.timeout_setting, TimeoutSetting::Pipeline);
    }
}

#[test_log::test]
fn the_repositorys_scorer_table_ranks_above_the_operators() {
    let config = resolve(
        &["--model", "opus"],
        &[],
        Some("[stage.scorer]\nmodel = \"haiku\"\neffort = \"max\"\n"),
    )
    .unwrap();
    let repo = RepoSettings::parse(
        "model = \"sonnet\"\n\n[stage.scorer]\neffort = \"low\"\nsession-timeout = \"5m\"\n",
    )
    .unwrap();

    let scorer = sessions(&config).for_run(RunKind::Scorer, &repo, &Environments::default());

    assert_eq!(scorer.effort, Sourced::new(Effort::Low, Layer::Repository));
    assert_eq!(scorer.model, Sourced::new(model("haiku"), Layer::Operator));
    assert_eq!(
        scorer.session_timeout,
        Sourced::new(Duration::from_mins(5), Layer::Repository)
    );
    assert_eq!(
        scorer.timeout_setting,
        TimeoutSetting::Stage(StageKey::Scorer)
    );
    assert_eq!(
        repo_session(
            &config,
            "[stage.scorer]\neffort = \"low\"\n",
            WorkKind::Design
        )
        .effort,
        Sourced::new(Effort::Medium, Layer::Linewright)
    );
}

#[test_log::test]
fn the_scorers_table_adds_to_the_scorers_environment() {
    let config = resolve(
        &[],
        &[],
        Some(
            "session-pass-env = [\"CARGO_HOME\"]\n\n\
             [stage.scorer]\nsession-pass-env = [\"SCORER_KEY\"]\nsession-env = { CI = \"scorer\" }\n",
        ),
    )
    .unwrap();
    let environment = [("CARGO_HOME", "/cargo"), ("SCORER_KEY", "k")]
        .map(|(name, value)| (name.into(), value.into()));

    let split = config
        .work
        .environments(&config.session_env, environment, &[]);

    let mut scorer: Vec<(String, String)> = sessions(&config)
        .for_run(
            RunKind::Scorer,
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
    scorer.sort();
    assert_eq!(
        scorer,
        pairs(&[
            ("CARGO_HOME", "/cargo"),
            ("CI", "scorer"),
            ("SCORER_KEY", "k")
        ])
    );
    assert_eq!(
        given(&config, &split, WorkKind::Implement),
        pairs(&[("CARGO_HOME", "/cargo")])
    );
    assert_eq!(
        split.added,
        [(StageKey::Scorer, vec!["SCORER_KEY".to_string()])].into()
    );
}

#[test_log::test]
fn an_unknown_stages_table_is_an_error_naming_the_table_and_the_scorer_among_the_names() {
    let operator_error = resolve(&[], &[], Some("[stage.deploy]\nmodel = \"opus\"\n")).unwrap_err();
    let repo_error = format!(
        "{:#}",
        RepoSettings::parse("[stage.deploy]\nmodel = \"opus\"\n").unwrap_err()
    );

    for error in [operator_error, repo_error] {
        assert!(error.contains("`[stage.deploy]`"), "{error}");
        assert!(error.contains("update, scorer"), "{error}");
    }
}

/// What a `stage` run's sessions run with under `config` and the
/// repository's file `repo`.
fn repo_session(config: &Config, repo: &str, stage: WorkKind) -> SessionSettings {
    sessions(config).for_run(
        RunKind::Stage(stage),
        &RepoSettings::parse(repo).unwrap(),
        &Environments::default(),
    )
}

#[test_log::test]
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
    assert_eq!(design.effort, Sourced::new(Effort::High, Layer::Repository));
    assert_eq!(design.model, Sourced::new(model("opus"), Layer::Operator));
    let triage = repo_session(&config, repo, WorkKind::Triage);
    assert_eq!(triage.effort, Sourced::new(Effort::Xhigh, Layer::Operator));
    assert_eq!(
        triage.session_timeout,
        Sourced::new(Duration::from_mins(20), Layer::Repository)
    );
    assert_eq!(triage.timeout_setting, TimeoutSetting::Pipeline);
}

#[test_log::test]
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
        TimeoutSetting::Stage(StageKey::Work(WorkKind::Implement))
    );
    assert_eq!(
        implement.model,
        Sourced::new(model("opus"), Layer::Repository)
    );
}

#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
fn the_earlier_names_of_the_benchmarking_commands_and_flags_are_unknown() {
    for (args, unknown) in [
        (&["replay", "task", "--out", "out"][..], "'replay'"),
        (&["judge", "task", "out"], "'judge'"),
        (&["suites"], "'suites'"),
        (&["suite", "show", "board"], "'suite'"),
        (&["experiment", "show", "1"], "'experiment'"),
        (
            &["benchmark", "remove-case", "board", "lanes"],
            "'remove-case'",
        ),
        (
            &[
                "capture",
                "43/implement-2",
                "--name",
                "lanes",
                "--suite",
                "1",
                "--criteria",
                "c.md",
            ],
            "'--suite'",
        ),
        (&["--max-replays", "2", "status"], "'--max-replays'"),
    ] {
        let error = Cli::try_parse_from(["lw"].iter().chain(args)).unwrap_err();

        assert!(error.to_string().contains(unknown), "{args:?}: {error}");
    }
}

#[test_log::test]
fn a_stage_has_no_flag() {
    assert!(Cli::try_parse_from(["lw", "--stage.implement.model", "opus", "status"]).is_err());
}

/// The variables a `stage` run's sessions are given, by name, under
/// `config` and the environments `split` made.
fn given(config: &Config, split: &Split, stage: WorkKind) -> Vec<(String, String)> {
    let mut given: Vec<(String, String)> = sessions(config)
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

#[test_log::test]
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
        [(
            StageKey::Work(WorkKind::Implement),
            vec!["DEPLOY_KEY".to_string()]
        )]
        .into()
    );
}

#[test_log::test]
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
            StageKey::Work(WorkKind::Implement),
            StageSet {
                own: vec!["CARGO_HOME".into(), "RUST_LOG".into()],
                taken: vec!["CI".into()],
            }
        )]
        .into()
    );
}

#[test_log::test]
fn an_automations_model_and_effort_come_before_the_whole_pipelines() {
    let config = resolve(&["--model", "sonnet", "--effort", "low"], &[], None).unwrap();
    let automation = automation::parse(
        &"weekly.md".parse().unwrap(),
        "---\non:\n  schedule: \"0 9 * * 1\"\nmodel: opus\neffort: xhigh\n---\nLook around.\n",
    )
    .unwrap();

    let session = sessions(&config).for_run(
        RunKind::Automation(&automation),
        &RepoSettings::default(),
        &Environments::default(),
    );

    assert_eq!(
        session.model,
        Sourced::new(model("opus"), Layer::Repository)
    );
    assert_eq!(
        session.effort,
        Sourced::new(Effort::Xhigh, Layer::Repository)
    );
    assert_eq!(session.timeout_setting, TimeoutSetting::Pipeline);
}

#[test_log::test]
fn a_runs_settings_name_the_operator_for_a_flag_or_the_operators_file_and_linewright_for_a_default()
{
    let from_flag = resolve(
        &["--model", "opus"],
        &[],
        Some("session-timeout = \"10m\"\n"),
    )
    .unwrap();
    let from_variable = resolve(
        &["--model", "opus"],
        &[("LINEWRIGHT_SESSION_PASS_ENV", "CARGO_HOME")],
        None,
    )
    .unwrap();
    let set_in_file = resolve(
        &["--model", "opus"],
        &[],
        Some("session-env = { CI = \"true\" }\n"),
    )
    .unwrap();
    let defaults = resolve(&["--model", "opus"], &[], None).unwrap();

    let flagged = session(&from_flag);
    assert_eq!(flagged.model, Sourced::new(model("opus"), Layer::Operator));
    assert_eq!(
        flagged.session_timeout,
        Sourced::new(Duration::from_mins(10), Layer::Operator)
    );
    let sources = |config| session(config).sources();
    assert_eq!(
        (
            sources(&from_flag)[&Setting::SESSION_PASS_ENV],
            sources(&from_flag)[&Setting::SESSION_ENV]
        ),
        (Layer::Linewright, Layer::Linewright)
    );
    assert_eq!(
        (
            sources(&from_variable)[&Setting::SESSION_PASS_ENV],
            sources(&from_variable)[&Setting::SESSION_ENV]
        ),
        (Layer::Operator, Layer::Linewright)
    );
    assert_eq!(
        (
            sources(&set_in_file)[&Setting::SESSION_PASS_ENV],
            sources(&set_in_file)[&Setting::SESSION_ENV]
        ),
        (Layer::Linewright, Layer::Operator)
    );
    let defaulted = session(&defaults);
    assert_eq!(
        defaulted.effort,
        Sourced::new(Effort::Medium, Layer::Linewright)
    );
    assert_eq!(
        defaulted.session_timeout,
        Sourced::new(Duration::from_hours(6), Layer::Linewright)
    );
}

#[test_log::test]
fn an_automations_session_timeout_comes_from_the_repository_before_the_operators_session_timeout() {
    let config = resolve(&["--session-timeout", "10m"], &[], None).unwrap();
    let automation = automation::parse(
        &"weekly.md".parse().unwrap(),
        "---\non:\n  schedule: \"0 9 * * 1\"\nsession-timeout: 5m\n---\nLook around.\n",
    )
    .unwrap();

    let session = sessions(&config).for_run(
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

#[test_log::test]
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
        session(&config).session_timeout,
        Sourced::new(Duration::from_hours(6), Layer::Linewright)
    );
    assert_eq!(config.spec_glob.pattern(), "docs/**/*.md");
}

#[test_log::test]
fn max_trials_is_read_from_the_file() {
    let config = resolve(&["--repo", "acme/widgets"], &[], Some("max-trials = 3\n")).unwrap();

    assert_eq!(config.max_trials, 3);
}

#[test_log::test]
fn max_trials_is_read_from_its_flag_and_defaults_to_1() {
    let flagged = resolve(&["--repo", "acme/widgets", "--max-trials", "2"], &[], None).unwrap();
    let unset = resolve(&["--repo", "acme/widgets"], &[], None).unwrap();

    assert_eq!(flagged.max_trials, 2);
    assert_eq!(unset.max_trials, 1);
}

#[test_log::test]
fn the_earlier_max_replays_key_and_scorers_table_are_errors_naming_them() {
    let max_replays =
        resolve(&["--repo", "acme/widgets"], &[], Some("max-replays = 3\n")).unwrap_err();
    let judge = resolve(&[], &[], Some("[stage.judge]\nmodel = \"opus\"\n")).unwrap_err();

    assert!(
        max_replays.contains("an unknown setting `max-replays`"),
        "{max_replays}"
    );
    assert!(judge.contains("`[stage.judge]`"), "{judge}");
}

#[test_log::test]
fn the_done_window_is_read_from_the_file_apart_from_the_spec_lookback() {
    let file = "repo = \"acme/widgets\"\ndone-window = \"30d\"\n";

    let config = resolve(&[], &[], Some(file)).unwrap();

    assert_eq!(config.done_window, chrono::TimeDelta::days(30));
    assert_eq!(config.spec_lookback, chrono::TimeDelta::days(7));
}

#[test_log::test]
fn defaults_follow_the_spec() {
    let config = resolve(&["--repo", "acme/widgets"], &[], None).unwrap();

    assert_eq!(config.poll_interval, Duration::from_mins(1));
    assert_eq!(config.spec_lookback, chrono::TimeDelta::days(7));
    assert_eq!(config.done_window, chrono::TimeDelta::days(7));
    assert_eq!(config.max_runs, 4);
    assert_eq!(config.max_trials, 1);
    assert!(
        config.work.clone().for_sessions().is_err(),
        "model has none"
    );
    let with_model = resolve(&["--repo", "acme/widgets", "--model", "opus"], &[], None).unwrap();
    assert_eq!(
        session(&with_model).session_timeout,
        Sourced::new(Duration::from_hours(6), Layer::Linewright)
    );
    assert_eq!(session(&with_model).effort.value, Effort::Medium);
    assert_eq!(config.result_retries, 1);
    assert_eq!(config.ci_grace, chrono::TimeDelta::minutes(5));
    assert_eq!(config.ci_timeout, chrono::TimeDelta::hours(6));
    assert_eq!(config.ci_attempts, 2);
    assert!(!config.auto_merge);
    assert_eq!(config.merge_method, None);
    assert_eq!(config.log_format, LogFormat::Json, "not a terminal");
    assert!(config.work_dir.ends_with("linewright"));
    assert_eq!(config.session_env, SessionEnv::default());
    assert_eq!(config.secrets, VariableNames::default());
}

#[test_log::test]
fn a_bad_value_or_an_unknown_key_is_an_error_naming_the_setting() {
    assert!(
        resolve(&["--max-runs", "many"], &[], None)
            .unwrap_err()
            .contains("max-runs")
    );
    assert!(
        resolve(&[], &[("LINEWRIGHT_LOG_FORMAT", "xml")], None)
            .unwrap_err()
            .contains("log-format")
    );
    assert!(
        resolve(&[], &[], Some("max-wokers = 2\n"))
            .unwrap_err()
            .contains("max-wokers")
    );
}

#[test_log::test]
fn the_operators_layer_of_briefs_is_the_briefs_setting_from_a_flag_variable_or_the_file() {
    let from_flag = resolve(&["--briefs", "flag/"], &[], None).unwrap();
    let from_env = resolve(&[], &[("LINEWRIGHT_BRIEFS", "env/")], None).unwrap();
    let from_file = resolve(&[], &[], Some("briefs = \"file/\"\n")).unwrap();

    assert_eq!(from_flag.briefs, Some("flag/".into()));
    assert_eq!(from_env.briefs, Some("env/".into()));
    assert_eq!(from_file.briefs, Some("file/".into()));
}

#[test_log::test]
fn the_earlier_prompts_key_is_an_unknown_setting() {
    let error = resolve(&[], &[], Some("prompts = \"mine/\"\n")).unwrap_err();

    assert!(error.contains("an unknown setting `prompts`"), "{error}");
}

fn blames_the_command_line(args: &[&str], env: &[(&str, &str)], file: Option<&str>) -> bool {
    match resolve_blaming(args, env, file) {
        Err(ResolveError::CommandLine(_)) => true,
        Err(ResolveError::Elsewhere(_)) => false,
        Ok(_) => panic!("{args:?}, {env:?} and {file:?} resolve"),
    }
}

#[test_log::test]
fn a_wrong_value_is_blamed_on_the_command_line_only_when_a_flag_gave_it() {
    let bad_env = [("LINEWRIGHT_LOG_FORMAT", "xml")];

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

#[test_log::test]
fn a_config_file_the_command_line_names_that_cannot_be_read_is_blamed_on_it() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let cli = Cli::try_parse_from(["lw", "--config", missing.to_str().unwrap(), "status"]).unwrap();

    let resolved = Config::resolve(&cli.settings, dir.path(), &|_| None, false);

    assert!(matches!(resolved, Err(ResolveError::CommandLine(_))));
}

#[test_log::test]
fn max_workers_is_no_longer_a_flag() {
    assert!(Cli::try_parse_from(["lw", "--max-workers", "2", "status"]).is_err());
}

#[test_log::test]
fn a_ci_timeout_no_longer_than_the_grace_period_is_an_error() {
    let error = resolve(&["--ci-grace", "10m", "--ci-timeout", "5m"], &[], None).unwrap_err();

    assert!(error.contains("ci-timeout"), "{error}");
}

#[test_log::test]
fn the_earlier_update_branch_key_is_an_unknown_setting() {
    let error = resolve(&[], &[], Some("update-branch = \"rebase\"\n")).unwrap_err();

    assert!(
        error.contains("an unknown setting `update-branch`"),
        "{error}"
    );
    assert!(Cli::try_parse_from(["lw", "--update-branch", "rebase", "status"]).is_err());
}

#[test_log::test]
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

#[test_log::test]
fn the_merge_method_follows_the_repository_unless_set_with_auto_merge_on_or_off() {
    let merge_method =
        |args: &[&str], env: &[(&str, &str)], file| resolve(args, env, file).unwrap().merge_method;
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
    assert_eq!(merge_method(&[], &[], None), None);
    assert_eq!(
        merge_method(&["--merge-method", "squash"], &[], None),
        Some(MergeMethod::Squash)
    );
    assert_eq!(
        merge_method(
            &["--auto-merge=false"],
            &[],
            Some("auto-merge = true\nmerge-method = \"rebase\"\n"),
        ),
        Some(MergeMethod::Rebase)
    );
}

#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
fn webhooks_are_off_without_the_secret() {
    let config = resolve(&[], &[], None).unwrap();

    assert!(config.webhooks.is_none());
}

#[test_log::test]
fn the_secret_turns_webhooks_on_at_the_default_address() {
    let config = resolve(&[], &[("LINEWRIGHT_WEBHOOK_SECRET", "s3cret")], None).unwrap();

    let webhooks = config.webhooks.unwrap();
    assert_eq!(webhooks.address.to_string(), "0.0.0.0:8080");
}

#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
fn a_webhook_address_without_the_secret_is_an_error_naming_the_secret() {
    let flag = resolve(&["--webhook-address", "127.0.0.1:9000"], &[], None).unwrap_err();
    let env = resolve(
        &[],
        &[("LINEWRIGHT_WEBHOOK_ADDRESS", "127.0.0.1:9000")],
        None,
    )
    .unwrap_err();
    let file = resolve(&[], &[], Some("webhook-address = \"127.0.0.1:9000\"\n")).unwrap_err();
    let the_default = resolve(&["--webhook-address", "0.0.0.0:8080"], &[], None).unwrap_err();

    for error in [flag, env, file, the_default] {
        assert!(error.contains("LINEWRIGHT_WEBHOOK_SECRET"), "{error}");
    }
}

#[test_log::test]
fn the_webhook_secret_is_not_a_flag_or_a_file_key() {
    let flag = Cli::try_parse_from(["lw", "--webhook-secret", "s3cret", "status"]);
    let file = resolve(&[], &[], Some("webhook-secret = \"s3cret\"\n")).unwrap_err();

    assert!(flag.is_err());
    assert!(file.contains("webhook-secret"), "{file}");
}

#[test_log::test]
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

#[test_log::test]
fn help_lists_the_webhook_address_with_its_default() {
    let (help, line) = help_for("--webhook-address");
    assert!(
        line.contains("[default: 0.0.0.0:8080 with `LINEWRIGHT_WEBHOOK_SECRET` set"),
        "{help}"
    );
}

#[test_log::test]
fn help_lists_the_session_timeout_with_its_default() {
    let (help, line) = help_for("--session-timeout");
    assert!(line.contains("[default: 6h]"), "{help}");
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

#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
fn session_env_has_no_environment_variable() {
    let config = resolve(&[], &[("LINEWRIGHT_SESSION_ENV", "A=1")], None).unwrap();

    assert_eq!(config.session_env, SessionEnv::default());
}

#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
fn session_env_setting_a_variable_linewright_may_set_itself_is_an_error() {
    for name in [
        "TMPDIR",
        "CLAUDE_CODE_SHELL",
        "DISABLE_UPDATES",
        "CLAUDE_CODE_EFFORT_LEVEL",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "CLAUDE_CODE_STARTUP_FAILURE_RESULTS",
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

#[test_log::test]
fn session_pass_env_naming_a_credential_of_linewrights_is_an_error_saying_why() {
    for name in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "LINEWRIGHT_SESSION_GITHUB_TOKEN",
        "LINEWRIGHT_WEBHOOK_SECRET",
        "LINEWRIGHT_GOOGLE_CLIENT_SECRET",
        "LINEWRIGHT_GITHUB_CLIENT_SECRET",
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

#[test_log::test]
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

#[test_log::test]
fn a_session_pass_env_pattern_matching_a_credential_of_linewrights_is_no_error() {
    let config = resolve(&["--session-pass-env", "GH_*,LINEWRIGHT_*"], &[], None);

    assert!(config.is_ok());
}

#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
fn a_secret_named_at_two_layers_counts_once() {
    let joined = resolve(
        &["--secrets", "A,B"],
        &[("LINEWRIGHT_SECRETS", "B")],
        Some("secrets = [\"A\", \"C\"]\n"),
    )
    .unwrap();

    assert_eq!(joined.secrets, secrets(&["A", "B", "C"]));
}

#[test_log::test]
fn a_secret_that_is_not_a_name_is_an_error_naming_the_layer_that_gave_it() {
    let file = Some("secrets = [\"*TOKEN\"]\n");

    let error = resolve(&["--secrets", "A"], &[], file).unwrap_err();

    assert!(error.contains("linewright.toml"), "{error}");
}

#[test_log::test]
fn secrets_naming_a_variable_that_is_unset_is_no_error_resolving_the_config() {
    let config = resolve(&["--secrets", "AWS_SECRET_ACCESS_KEY"], &[], None);

    assert!(config.is_ok());
}

#[test_log::test]
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

#[test_log::test]
fn help_lists_secrets_with_its_default() {
    let (help, line) = help_for("--secrets");
    assert!(line.contains("[default: none]"), "{help}");
}

#[test_log::test]
fn only_the_list_settings_are_lists_in_the_file() {
    let error = resolve(&[], &[], Some("repo = [\"acme/widgets\"]\n")).unwrap_err();

    assert!(error.contains("`repo`"), "{error}");
}

/// The variables of a web UI at `https://lw.example.com`, and of its
/// providers' client IDs and secrets.
const PUBLIC_URL: (&str, &str) = ("LINEWRIGHT_PUBLIC_URL", "https://lw.example.com");
const GOOGLE_ID: (&str, &str) = ("LINEWRIGHT_GOOGLE_CLIENT_ID", "google-id");
const GOOGLE_SECRET: (&str, &str) = ("LINEWRIGHT_GOOGLE_CLIENT_SECRET", "google-s3cret");
const GITHUB_ID: (&str, &str) = ("LINEWRIGHT_GITHUB_CLIENT_ID", "github-id");
const GITHUB_SECRET: (&str, &str) = ("LINEWRIGHT_GITHUB_CLIENT_SECRET", "github-s3cret");

/// The variables of a web UI signing people in with Google.
const GOOGLE_UI: [(&str, &str); 3] = [PUBLIC_URL, GOOGLE_ID, GOOGLE_SECRET];

/// GitHub's client ID and secret, as variables.
const GITHUB: [(&str, &str); 2] = [GITHUB_ID, GITHUB_SECRET];

/// `GOOGLE_UI` with `more` after it.
fn google_ui_and<'a>(more: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    GOOGLE_UI.iter().chain(more).copied().collect()
}

#[test_log::test]
fn without_public_url_serve_serves_no_web_ui() {
    let config = resolve(&[], &[], None).unwrap();
    let at_its_default = resolve(&["--ui-address", "127.0.0.1:8081"], &[], None).unwrap();

    assert!(config.ui.is_none());
    assert!(at_its_default.ui.is_none());
}

#[test_log::test]
fn the_web_ui_is_served_on_127_0_0_1_8081_at_public_url_by_default() {
    let config = resolve(&[], &GOOGLE_UI, None).unwrap();

    let ui = config.ui.unwrap();
    assert_eq!(ui.address.to_string(), "127.0.0.1:8081");
    assert_eq!(ui.public_url.to_string(), "https://lw.example.com/");
    let google = ui.providers.google().unwrap();
    assert_eq!(google.client_id.as_str(), "google-id");
    assert_eq!(google.secret.expose(), "google-s3cret");
    assert!(ui.providers.github().is_none());
    assert!(ui.manager.is_none());
}

#[test_log::test]
fn the_ui_settings_are_keys_in_the_file_or_flags_that_add_to_its_lists() {
    let file = "public-url = \"https://lw.example.com\"\n\
                google-client-id = \"google-id\"\n\
                google-signup-domains = [\"example.com\", \"Example.ORG.\"]\n\
                github-client-id = \"github-id\"\n\
                github-signup-orgs = [\"acme\", \"Widgets/Core-Team\"]\n\
                manager = \"github:Octo-Cat\"\n";
    let secrets = [GOOGLE_SECRET, GITHUB_SECRET];

    let from_file = resolve(&[], &secrets, Some(file)).unwrap();
    let from_flags = resolve(
        &[
            "--public-url",
            "http://127.0.0.1:9000",
            "--google-client-id",
            "flag-id",
            "--google-signup-domains",
            "flag.example",
        ],
        &secrets,
        Some(file),
    )
    .unwrap();

    let in_file = from_file.ui.unwrap();
    let Providers::Both(google, github) = &in_file.providers else {
        panic!("{:?}", in_file.providers);
    };
    assert_eq!(google.client_id.as_str(), "google-id");
    assert_eq!(google.signup.domains(), ["example.com", "example.org"]);
    assert_eq!(github.client_id.as_str(), "github-id");
    assert_eq!(github.secret.expose(), "github-s3cret");
    let memberships: Vec<(&str, Option<&str>)> = github
        .signup
        .memberships()
        .iter()
        .map(|membership| (membership.organization(), membership.team()))
        .collect();
    assert_eq!(
        memberships,
        [("acme", None), ("widgets", Some("core-team"))]
    );
    assert_eq!(in_file.manager.unwrap().to_string(), "github:octo-cat");
    let by_flags = from_flags.ui.unwrap();
    assert_eq!(by_flags.public_url.to_string(), "http://127.0.0.1:9000/");
    let flagged = by_flags.providers.google().unwrap();
    assert_eq!(flagged.client_id.as_str(), "flag-id");
    assert_eq!(
        flagged.signup.domains(),
        ["flag.example", "example.com", "example.org"]
    );
}

#[test_log::test]
fn public_url_is_https_or_http_to_a_loopback_address() {
    for (given, url) in [
        ("https://lw.example.com", "https://lw.example.com/"),
        (
            "HTTPS://LW.Example.com:8443/",
            "https://lw.example.com:8443/",
        ),
        ("https://192.0.2.7", "https://192.0.2.7/"),
        ("http://127.0.0.1:8081", "http://127.0.0.1:8081/"),
        ("http://127.1.2.3", "http://127.1.2.3/"),
        ("http://[::1]:8081", "http://[::1]:8081/"),
    ] {
        let env = [("LINEWRIGHT_PUBLIC_URL", given), GOOGLE_ID, GOOGLE_SECRET];

        let ui = resolve(&[], &env, None).unwrap().ui.unwrap();

        assert_eq!(ui.public_url.to_string(), url, "{given}");
    }
}

#[test_log::test]
fn a_public_url_that_is_not_https_or_http_to_a_loopback_address_is_an_error_naming_it() {
    for bad in [
        "http://lw.example.com",
        "http://localhost:8081",
        "http://192.0.2.7",
        "ftp://lw.example.com",
        "lw.example.com",
        "https://lw.example.com/linewright",
        "https://lw.example.com/?a=b",
        "https://lw.example.com/#top",
        "https://someone@lw.example.com",
        "https://.",
        "https://lw_example.com",
    ] {
        let error = resolve(&["--public-url", bad], &[GOOGLE_ID, GOOGLE_SECRET], None).unwrap_err();

        assert!(
            error.starts_with("the setting `public-url` is "),
            "{bad}: {error}"
        );
        assert!(error.contains(bad), "{bad}: {error}");
    }
}

#[test_log::test]
fn ui_address_away_from_its_default_without_public_url_is_an_error_naming_both() {
    for ui_address in ["0.0.0.0:9000", ""] {
        let error = resolve(&["--ui-address", ui_address], &[], None).unwrap_err();

        assert_eq!(
            error,
            "the setting `ui-address` is set, but `public-url` is not: \
             set `public-url` to serve the web UI, or unset `ui-address`",
            "{ui_address:?}"
        );
    }
}

#[test_log::test]
fn a_ui_setting_without_public_url_is_an_error_naming_it() {
    for (setting, variable, value) in [
        (
            "google-client-id",
            "LINEWRIGHT_GOOGLE_CLIENT_ID",
            "google-id",
        ),
        (
            "google-signup-domains",
            "LINEWRIGHT_GOOGLE_SIGNUP_DOMAINS",
            "example.com",
        ),
        (
            "github-client-id",
            "LINEWRIGHT_GITHUB_CLIENT_ID",
            "github-id",
        ),
        (
            "github-signup-orgs",
            "LINEWRIGHT_GITHUB_SIGNUP_ORGS",
            "acme",
        ),
        ("manager", "LINEWRIGHT_MANAGER", "google:alice@example.com"),
    ] {
        let error = resolve(&[], &[(variable, value)], None).unwrap_err();

        assert_eq!(
            error,
            format!(
                "the setting `{setting}` is set, but `public-url` is not: \
                 set `public-url` to serve the web UI, or unset `{setting}`"
            )
        );
    }
}

#[test_log::test]
fn a_sign_up_rule_without_its_providers_client_id_is_an_error_naming_both() {
    let google = resolve(
        &[],
        &[
            PUBLIC_URL,
            GITHUB_ID,
            GITHUB_SECRET,
            ("LINEWRIGHT_GOOGLE_SIGNUP_DOMAINS", "example.com"),
        ],
        None,
    )
    .unwrap_err();
    let github = resolve(
        &[],
        &google_ui_and(&[("LINEWRIGHT_GITHUB_SIGNUP_ORGS", "acme")]),
        None,
    )
    .unwrap_err();

    assert_eq!(
        google,
        "the setting `google-signup-domains` is set, but `google-client-id` is not: \
         set `google-client-id` to sign people in with Google, or unset `google-signup-domains`"
    );
    assert_eq!(
        github,
        "the setting `github-signup-orgs` is set, but `github-client-id` is not: \
         set `github-client-id` to sign people in with GitHub, or unset `github-signup-orgs`"
    );
}

#[test_log::test]
fn a_client_secret_without_its_providers_client_id_is_an_error_naming_both() {
    let google = resolve(
        &[],
        &[PUBLIC_URL, GITHUB_ID, GITHUB_SECRET, GOOGLE_SECRET],
        None,
    )
    .unwrap_err();
    let github = resolve(&[], &google_ui_and(&[GITHUB_SECRET]), None).unwrap_err();
    let without_public_url = resolve(&[], &[GITHUB_SECRET], None).unwrap_err();

    assert_eq!(
        google,
        "`LINEWRIGHT_GOOGLE_CLIENT_SECRET` is set, but the setting `google-client-id` is not: \
         set `google-client-id` to sign people in with Google, or unset \
         `LINEWRIGHT_GOOGLE_CLIENT_SECRET`"
    );
    for error in [github, without_public_url] {
        assert_eq!(
            error,
            "`LINEWRIGHT_GITHUB_CLIENT_SECRET` is set, but the setting `github-client-id` is \
             not: set `github-client-id` to sign people in with GitHub, or unset \
             `LINEWRIGHT_GITHUB_CLIENT_SECRET`"
        );
    }
}

#[test_log::test]
fn manager_naming_a_provider_without_a_client_id_is_an_error_naming_both() {
    let github = resolve(&["--manager", "github:octocat"], &GOOGLE_UI, None).unwrap_err();
    let google = resolve(
        &["--manager", "google:alice@example.com"],
        &[PUBLIC_URL, GITHUB_ID, GITHUB_SECRET],
        None,
    )
    .unwrap_err();

    assert_eq!(
        github,
        "the setting `manager` names a GitHub identity, but `github-client-id` is not set: \
         set `github-client-id` to sign people in with GitHub, or name a Google identity"
    );
    assert_eq!(
        google,
        "the setting `manager` names a Google identity, but `google-client-id` is not set: \
         set `google-client-id` to sign people in with Google, or name a GitHub identity"
    );
}

#[test_log::test]
fn manager_names_a_google_email_or_a_github_login_ignoring_case() {
    let env = google_ui_and(&GITHUB);
    let manager = |name: &str| {
        resolve(&["--manager", name], &env, None)
            .unwrap()
            .ui
            .unwrap()
            .manager
            .unwrap()
            .to_string()
    };

    assert_eq!(
        manager("google:Alice@Example.com"),
        "google:alice@example.com"
    );
    assert_eq!(manager("GitHub:Octo-Cat"), "github:octo-cat");
}

#[test_log::test]
fn a_manager_that_names_no_identity_is_an_error_naming_it() {
    let env = google_ui_and(&GITHUB);
    for bad in [
        "alice@example.com",
        "google:alice",
        "google:@example.com",
        "google:alice@",
        "github:",
        "github:octo cat",
        "github:octo@cat",
        "github:-octocat",
        "github:octo--cat",
        "github:a123456789a123456789a123456789a123456789",
        "google:alice@bob@example.com",
        "gitlab:octocat",
    ] {
        let error = resolve(&["--manager", bad], &env, None).unwrap_err();

        assert!(
            error.starts_with(&format!("the setting `manager` is {bad:?}: ")),
            "{bad}: {error}"
        );
    }
}

#[test_log::test]
fn a_sign_up_rule_entry_that_names_no_domain_or_organization_is_an_error_naming_it() {
    let env = google_ui_and(&GITHUB);
    for (flag, bad) in [
        ("--google-signup-domains", "@example.com"),
        ("--google-signup-domains", "example.com/x"),
        ("--google-signup-domains", "*.example.com"),
        ("--google-signup-domains", "example.com:443"),
        ("--github-signup-orgs", "acme/core/x"),
        ("--github-signup-orgs", "/core"),
        ("--github-signup-orgs", "acme/"),
        ("--github-signup-orgs", "ac me"),
        ("--google-signup-domains", "example-.com"),
    ] {
        let error = resolve(&[flag, bad], &env, None).unwrap_err();

        assert!(
            error.contains(&format!("the setting `{}` names {bad:?}", &flag[2..])),
            "{bad}: {error}"
        );
    }
}

#[test_log::test]
fn public_url_with_an_empty_ui_address_is_an_error_naming_both() {
    let error = resolve(&["--ui-address", ""], &GOOGLE_UI, None).unwrap_err();

    assert_eq!(
        error,
        "the setting `public-url` is set, but `ui-address` is empty: \
         set `ui-address` to serve the web UI on it, or unset `public-url`"
    );
}

#[test_log::test]
fn public_url_without_a_client_id_is_an_error_naming_the_providers() {
    let error = resolve(&[], &[PUBLIC_URL], None).unwrap_err();

    assert_eq!(
        error,
        "the setting `public-url` is set, but no identity provider is: \
         set `google-client-id`, `github-client-id` or both"
    );
}

#[test_log::test]
fn a_client_id_without_its_secret_is_an_error_naming_both() {
    let google = resolve(&[], &[PUBLIC_URL, GOOGLE_ID], None).unwrap_err();
    let github = resolve(&[], &google_ui_and(&[GITHUB_ID]), None).unwrap_err();

    assert_eq!(
        google,
        "the setting `google-client-id` is set, but `LINEWRIGHT_GOOGLE_CLIENT_SECRET` is not: \
         set it to the client secret Google gave"
    );
    assert_eq!(
        github,
        "the setting `github-client-id` is set, but `LINEWRIGHT_GITHUB_CLIENT_SECRET` is not: \
         set it to the client secret GitHub gave"
    );
}

#[test_log::test]
fn an_empty_client_secret_is_no_client_secret() {
    let env = [
        PUBLIC_URL,
        GOOGLE_ID,
        ("LINEWRIGHT_GOOGLE_CLIENT_SECRET", ""),
    ];

    let error = resolve(&[], &env, None).unwrap_err();

    assert!(
        error.contains("`LINEWRIGHT_GOOGLE_CLIENT_SECRET` is not"),
        "{error}"
    );
}

#[test_log::test]
fn a_ui_setting_is_blamed_on_the_command_line_only_when_a_flag_is_at_fault() {
    assert!(blames_the_command_line(
        &["--manager", "github:octocat"],
        &GOOGLE_UI,
        None
    ));
    assert!(!blames_the_command_line(
        &[],
        &google_ui_and(&[("LINEWRIGHT_MANAGER", "github:octocat")]),
        None
    ));
    assert!(blames_the_command_line(
        &["--ui-address", ""],
        &GOOGLE_UI,
        None
    ));
    assert!(blames_the_command_line(
        &["--google-client-id", "google-id"],
        &[],
        None
    ));
    assert!(!blames_the_command_line(
        &[],
        &[PUBLIC_URL, GOOGLE_ID],
        None
    ));
}

#[test_log::test]
fn ui_hosts_is_an_unknown_setting() {
    let error = resolve(&[], &GOOGLE_UI, Some("ui-hosts = [\"a.example\"]\n")).unwrap_err();

    assert!(error.contains("unknown setting `ui-hosts`"), "{error}");
    assert!(Cli::try_parse_from(["lw", "--ui-hosts", "a.example", "status"]).is_err());
}

#[test_log::test]
fn the_ui_address_is_a_flag_the_environment_or_the_file() {
    let address = |args: &[&str], env: &[(&str, &str)], file| {
        resolve(args, &[&GOOGLE_UI[..], env].concat(), file)
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

#[test_log::test]
fn a_ui_address_that_is_not_a_host_and_port_is_an_error_naming_the_setting() {
    for bad in [
        "8081",
        "localhost",
        ":8081",
        "localhost:http",
        "localhost:99999",
    ] {
        let error = resolve(&["--ui-address", bad], &GOOGLE_UI, None).unwrap_err();

        assert!(error.contains("ui-address"), "{bad}: {error}");
    }
}

#[test_log::test]
fn a_malformed_merge_method_or_a_webhook_address_with_webhooks_off_is_an_error_naming_it() {
    let merge_method = resolve(&["--merge-method", "fast-forward"], &[], None).unwrap_err();
    let webhook_address = resolve(&["--webhook-address", "8080"], &[], None).unwrap_err();

    assert!(merge_method.contains("\"fast-forward\""), "{merge_method}");
    assert!(webhook_address.contains("\"8080\""), "{webhook_address}");
}

#[test_log::test]
fn help_lists_the_ui_address_with_its_default() {
    let (help, line) = help_for("--ui-address");
    assert!(line.contains("127.0.0.1:8081"), "{help}");
}

#[test_log::test]
fn maintenance_is_a_flag_a_variable_or_a_key_and_unset_runs_nothing() {
    let maintenance = |args: &[&str], env: &[(&str, &str)], file| {
        session(&resolve(args, env, file).unwrap()).maintenance
    };
    let file = Some("maintenance = \"make deps\"\n");
    let command = |text: &str| Maintenance::from(text.to_string());

    assert_eq!(
        maintenance(&[], &[], None),
        Sourced::new(Maintenance::Empty, Layer::Linewright)
    );
    assert_eq!(
        maintenance(&[], &[], file),
        Sourced::new(command("make deps"), Layer::Operator)
    );
    assert_eq!(
        maintenance(&[], &[("LINEWRIGHT_MAINTENANCE", "npm ci")], file).value,
        command("npm ci")
    );
    assert_eq!(
        maintenance(
            &["--maintenance", "cargo fetch"],
            &[("LINEWRIGHT_MAINTENANCE", "npm ci")],
            file
        )
        .value,
        command("cargo fetch")
    );
    assert_eq!(
        maintenance(&["--maintenance", ""], &[], file).value,
        Maintenance::Empty
    );
    assert_eq!(
        session(&resolve(&[], &[], file).unwrap()).sources()[&Setting::Maintenance],
        Layer::Operator
    );
}

#[test_log::test]
fn the_repositorys_maintenance_ranks_below_the_operators_flag_and_variable_and_above_the_operators_file()
 {
    let repo = "maintenance = \"npm ci\"\n";
    let maintenance = |args: &[&str], env: &[(&str, &str)]| {
        let config = resolve(args, env, Some("maintenance = \"make deps\"\n")).unwrap();
        repo_session(&config, repo, WorkKind::Implement).maintenance
    };

    assert_eq!(
        maintenance(&[], &[]),
        Sourced::new(Maintenance::from("npm ci".to_string()), Layer::Repository)
    );
    assert_eq!(
        maintenance(&[], &[("LINEWRIGHT_MAINTENANCE", "cargo fetch")]),
        Sourced::new(
            Maintenance::from("cargo fetch".to_string()),
            Layer::Operator
        )
    );
    assert_eq!(
        maintenance(&["--maintenance", ""], &[]),
        Sourced::new(Maintenance::Empty, Layer::Operator)
    );
}

#[test_log::test]
fn maintenance_in_a_stages_table_or_front_matter_is_an_error_naming_it_the_whole_pipelines() {
    let operators = resolve(
        &[],
        &[],
        Some("[stage.implement]\nmaintenance = \"make all\"\n"),
    )
    .unwrap_err();
    let repositorys = format!(
        "{:#}",
        RepoSettings::parse("[stage.implement]\nmaintenance = \"make all\"\n").unwrap_err()
    );
    let front_matter = format!(
        "{:#}",
        automation::parse(
            &"weekly.md".parse().unwrap(),
            "---\non:\n  schedule: \"0 9 * * 1\"\nmaintenance: make lint\n---\nLook around.\n",
        )
        .unwrap_err()
    );

    for error in [operators, repositorys, front_matter] {
        assert!(error.contains("`maintenance`"), "{error}");
        assert!(error.contains("whole pipeline's"), "{error}");
    }
}

#[test_log::test]
fn setup_as_a_flag_or_variable_is_an_error_naming_setup_command_and_maintenance() {
    let flag = resolve_blaming(&["--setup", "npm ci"], &[], None).unwrap_err();
    let variable = resolve(&[], &[("LINEWRIGHT_SETUP", "npm ci")], None).unwrap_err();

    assert!(matches!(flag, ResolveError::CommandLine(_)), "{flag}");
    for (error, named) in [
        (
            flag.to_string(),
            ["`--setup`", "`--setup-command`", "`--maintenance`"],
        ),
        (
            variable,
            [
                "`LINEWRIGHT_SETUP`",
                "`LINEWRIGHT_SETUP_COMMAND`",
                "`LINEWRIGHT_MAINTENANCE`",
            ],
        ),
    ] {
        for name in named {
            assert!(error.contains(name), "{error}");
        }
    }
}

#[test_log::test]
fn setup_as_a_plain_value_is_an_error_saying_it_is_a_table_holding_command() {
    let operators_file = resolve(&[], &[], Some("setup = \"npm ci\"\n")).unwrap_err();
    let repositorys = format!(
        "{:#}",
        RepoSettings::parse("setup = \"npm ci\"\n").unwrap_err()
    );

    for error in [operators_file, repositorys] {
        assert!(
            error.contains("`setup` is a table holding `command`"),
            "{error}"
        );
        assert!(error.contains("`maintenance`"), "{error}");
    }
}

#[track_caller]
fn setup_of(config: &Config, repo: &str) -> Option<SetupSettings> {
    config.work.setup(&RepoSettings::parse(repo).unwrap())
}

#[track_caller]
fn setup(command: &str, max_age: Duration, timeout: Duration) -> SetupSettings {
    SetupSettings {
        command: SetupCommand::new(command.to_string()).unwrap(),
        max_age,
        timeout,
    }
}

#[test_log::test]
fn the_setup_table_is_a_table_in_the_file_and_flags_and_variables_named_with_a_dash() {
    let file = Some("[setup]\ncommand = \"make deps\"\nmax-age = \"12h\"\n");
    let setup_with = |args: &[&str], env: &[(&str, &str)], given| {
        setup_of(&resolve(args, env, given).unwrap(), "")
    };

    assert_eq!(setup_with(&[], &[], None), None);
    assert_eq!(
        setup_with(&[], &[], file),
        Some(setup(
            "make deps",
            Duration::from_hours(12),
            Duration::from_mins(60)
        ))
    );
    assert_eq!(
        setup_with(
            &["--setup-command", "npm ci", "--setup-max-age", "2h"],
            &[("LINEWRIGHT_SETUP_TIMEOUT", "10m")],
            file
        ),
        Some(setup(
            "npm ci",
            Duration::from_hours(2),
            Duration::from_mins(10)
        ))
    );
    assert_eq!(
        setup_with(&[], &[("LINEWRIGHT_SETUP_COMMAND", "cargo fetch")], None),
        Some(setup(
            "cargo fetch",
            Duration::from_hours(24),
            Duration::from_mins(60)
        ))
    );
}

#[test_log::test]
fn the_repositorys_setup_table_ranks_below_the_operators_flags_and_variables_and_above_the_operators_file()
 {
    let file = Some("[setup]\ncommand = \"make deps\"\ntimeout = \"5m\"\n");
    let repo = "[setup]\ncommand = \"npm ci\"\nmax-age = \"1h\"\n";

    assert_eq!(
        setup_of(&resolve(&[], &[], file).unwrap(), repo),
        Some(setup(
            "npm ci",
            Duration::from_hours(1),
            Duration::from_mins(5)
        ))
    );
    assert_eq!(
        setup_of(
            &resolve(&[], &[("LINEWRIGHT_SETUP_COMMAND", "cargo fetch")], file).unwrap(),
            repo
        ),
        Some(setup(
            "cargo fetch",
            Duration::from_hours(1),
            Duration::from_mins(5)
        ))
    );
    assert_eq!(
        setup_of(
            &resolve(&[], &[], file).unwrap(),
            "[setup]\ncommand = \"\"\n"
        ),
        None
    );
}

#[test_log::test]
fn setup_in_a_stages_table_or_front_matter_is_an_error_naming_it_the_whole_pipelines() {
    let operators = resolve(
        &[],
        &[],
        Some("[stage.implement]\nsetup = { command = \"make\" }\n"),
    )
    .unwrap_err();
    let repositorys = format!(
        "{:#}",
        RepoSettings::parse("[stage.implement.setup]\ncommand = \"make\"\n").unwrap_err()
    );
    let front_matter = format!(
        "{:#}",
        automation::parse(
            &"weekly.md".parse().unwrap(),
            "---\non:\n  schedule: \"0 9 * * 1\"\nsetup:\n  command: make\n---\nLook around.\n",
        )
        .unwrap_err()
    );

    for error in [operators, repositorys, front_matter] {
        assert!(error.contains("`setup`"), "{error}");
        assert!(error.contains("whole pipeline's"), "{error}");
    }
}

#[test_log::test]
fn an_unknown_key_in_the_setup_table_is_refused_with_the_keys_it_holds() {
    let operators = resolve(&[], &[], Some("[setup]\nscript = \"make\"\n")).unwrap_err();
    let repositorys = format!(
        "{:#}",
        RepoSettings::parse("[setup]\nscript = \"make\"\n").unwrap_err()
    );

    for error in [operators, repositorys] {
        assert!(error.contains("`script`"), "{error}");
        assert!(error.contains("`command`, `max-age`, `timeout`"), "{error}");
    }
}

#[test_log::test]
fn a_setup_duration_that_does_not_parse_is_an_error_naming_it() {
    let operators = resolve(&[], &[], Some("[setup]\nmax-age = \"soon\"\n")).unwrap_err();
    let repositorys = format!(
        "{:#}",
        RepoSettings::parse("[setup]\ntimeout = \"later\"\n").unwrap_err()
    );

    assert!(operators.contains("`setup.max-age`"), "{operators}");
    assert!(repositorys.contains("`setup.timeout`"), "{repositorys}");
}

#[test_log::test]
fn an_unknown_key_in_a_stages_table_is_refused_with_the_keys_it_holds() {
    let error = resolve(
        &[],
        &[],
        Some("[stage.implement]\nsetup-command = \"make\"\n"),
    )
    .unwrap_err();

    assert!(error.contains("`setup-command`"), "{error}");
    assert!(
        error.contains("`model`, `effort`, `session-timeout`, `session-env`, `session-pass-env`"),
        "{error}"
    );
}

#[test_log::test]
fn an_automations_front_matter_holds_no_maintenance_so_it_runs_the_whole_pipelines() {
    let config = resolve(&["--maintenance", "make deps"], &[], None).unwrap();
    let automation = automation::parse(
        &"weekly.md".parse().unwrap(),
        "---\non:\n  schedule: \"0 9 * * 1\"\n---\nLook around.\n",
    )
    .unwrap();

    let settings = sessions(&config).for_run(
        RunKind::Automation(&automation),
        &RepoSettings::parse("maintenance = \"npm ci\"\n").unwrap(),
        &Environments::default(),
    );

    assert_eq!(
        settings.maintenance,
        Sourced::new(Maintenance::from("make deps".to_string()), Layer::Operator)
    );
}
