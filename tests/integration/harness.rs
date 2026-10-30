use crate::common;

use std::path::Path;
use std::sync::{Arc, Mutex};

use common::*;
use linewright::domain::Number;
use linewright::harness::claude::ClaudeCode;
use linewright::harness::{Harness, Purpose, Reported, Reporter, SessionError, SessionSpec};
use linewright::reconcile::{Subject, WorkKind};
use linewright::redaction::Redaction;
use linewright::session_env::{Given, PassEnv, SessionEnv, SetEnv};
use linewright::transcript::{Prompt, What};
use linewright::work_settings::Effort;
use serde_json::{Value, json};

/// Claude Code as `program`, with no GitHub token and nothing to redact.
fn claude_code(program: &Path) -> ClaudeCode {
    ClaudeCode {
        program: program.to_str().unwrap().into(),
        bash: "/bin/bash".into(),
        github_token: None,
        redaction: Redaction::default(),
        container: env!("CARGO_BIN_EXE_lw").into(),
    }
}

fn harness_without_token(dir: &Path) -> ClaudeCode {
    harness_streaming(dir, &[done()])
}

fn harness_streaming(dir: &Path, events: &[Value]) -> ClaudeCode {
    claude_code(&fake_claude_streaming(dir, events))
}

/// Claude Code as the fake `claude`, carrying out `turn` on each turn.
fn harness_running(dir: &Path, turn: Vec<Step>) -> ClaudeCode {
    claude_code(&fake_claude_turning(dir, turn))
}

/// A turn's result, `Done.`.
fn done() -> Value {
    json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done."})
}

/// An assistant message saying `text`.
fn saying(text: &str) -> Value {
    json!({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}})
}

/// Waits until the session whose record is `record` has streamed `text`.
async fn until_said(record: &Path, text: &str) {
    let output = record.join(linewright::transcript::OUTPUT);
    eventually(|| std::fs::read_to_string(&output).is_ok_and(|output| output.contains(text))).await;
}

/// Runs `session` on `harness` and times it out once it has streamed `said`,
/// returning the error it ends with.
///
/// A session can take long to start on a loaded machine, so the deadline is
/// set far off and comes, with the clock paused, only once the session has
/// spoken. Pausing the clock means a test may call this at most once, on the
/// default current-thread runtime.
async fn timed_out_once_said(
    harness: ClaudeCode,
    mut session: SessionSpec,
    said: &str,
) -> SessionError {
    session.deadline = tokio::time::Instant::now() + std::time::Duration::from_hours(1);
    let record = session.record.clone();
    let turn = tokio::spawn(async move { harness.start(&session).await });
    until_said(&record, said).await;
    tokio::time::pause();
    match turn.await.unwrap() {
        Ok(_) => panic!("expected the session to time out, but it finished"),
        Err(error) => error,
    }
}

fn turn_args(dir: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(dir.join("args.log"))
        .unwrap()
        .split_terminator("\n\n")
        .map(|turn| turn.lines().map(str::to_string).collect())
        .collect()
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let at = args.iter().position(|arg| arg == flag)?;
    args.get(at + 1).map(String::as_str)
}

fn settings(args: &[String]) -> Value {
    flag_value(args, "--settings").map_or(Value::Null, |s| serde_json::from_str(s).unwrap())
}

/// `globset` stands in for Claude Code's own matcher, which also treats
/// parentheses as special.
fn instruction_excludes(args: &[String]) -> globset::GlobSet {
    let mut excludes = globset::GlobSetBuilder::new();
    for pattern in settings(args)["claudeMdExcludes"].as_array().unwrap() {
        let glob = globset::GlobBuilder::new(pattern.as_str().unwrap())
            .literal_separator(true)
            .build()
            .unwrap();
        excludes.add(glob);
    }
    excludes.build().unwrap()
}

fn triage_session(dir: &Path) -> SessionSpec {
    for sub in ["repo", "tmp"] {
        std::fs::create_dir(dir.join(sub)).unwrap();
    }
    SessionSpec {
        purpose: Purpose::Work {
            subject: Subject::Issue(Number(1)),
            kind: WorkKind::Triage,
        },
        dir: dir.join("repo"),
        scratch: dir.join("tmp"),
        record: dir.to_path_buf(),
        attachments: dir.join("attachments"),
        plugin: dir.join("plugin"),
        prompt: "Triage #1.".into(),
        model: None,
        effort: None,
        environment: Arc::new(Given::default()),
        deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(30),
        report: Reporter::new(|_| {}),
    }
}

/// What the harness reports of `session`, as it reports it.
fn reports_of(session: &mut SessionSpec) -> Arc<Mutex<Vec<Reported>>> {
    let reports = Arc::new(Mutex::new(vec![]));
    let kept = reports.clone();
    session.report = Reporter::new(move |reported| kept.lock().unwrap().push(reported));
    reports
}

/// An `init` event as Claude Code streams it:
/// <https://code.claude.com/docs/en/agent-sdk/typescript#sdksystemmessage>.
fn init_naming(model: &str) -> Value {
    json!({
        "type": "system",
        "subtype": "init",
        "session_id": "0d5b1b8e-6a0c-4e0e-9f0e-4b3c6f1e2a7d",
        "cwd": "/clone",
        "tools": ["Bash", "Read", "Edit"],
        "mcp_servers": [],
        "model": model,
        "permissionMode": "bypassPermissions",
        "apiKeySource": "none",
        "slash_commands": [],
        "output_style": "default",
    })
}

#[tokio::test]
async fn a_session_reports_the_model_its_init_event_names() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_streaming(dir.path(), &[init_naming("claude-opus-5-5"), done()]);
    let mut session = triage_session(dir.path());
    session.model = Some("opus".into());
    let reports = reports_of(&mut session);

    harness.start(&session).await.unwrap();

    assert_eq!(
        *reports.lock().unwrap(),
        [Reported {
            model: "claude-opus-5-5".into()
        }]
    );
}

#[tokio::test]
async fn a_session_that_times_out_has_reported_the_model_it_ran() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_running(
        dir.path(),
        vec![
            Step::Stream(vec![
                init_naming("claude-sonnet-5-5"),
                saying("Still reading."),
            ]),
            Step::Hang,
        ],
    );
    let mut session = triage_session(dir.path());
    let reports = reports_of(&mut session);

    let error = timed_out_once_said(harness, session, "Still reading.").await;

    assert!(matches!(error, SessionError::TimedOut { .. }), "{error:?}");
    assert_eq!(
        *reports.lock().unwrap(),
        [Reported {
            model: "claude-sonnet-5-5".into()
        }]
    );
}

#[tokio::test]
async fn a_production_session_gets_linewrights_github_token() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_claude(dir.path(), "Done.");
    let harness = ClaudeCode {
        github_token: Some("linewright-token".into()),
        ..claude_code(&program)
    };

    harness.start(&triage_session(dir.path())).await.unwrap();

    assert_eq!(
        std::fs::read_to_string(dir.path().join("tokens.log")).unwrap(),
        "GH_TOKEN=linewright-token GITHUB_TOKEN=linewright-token\n"
    );
}

/// The names of the variables the session's harness was started with.
fn session_variables(dir: &Path) -> Vec<String> {
    let env = std::fs::read_to_string(dir.join("env.log")).unwrap();
    let mut names: Vec<String> = env
        .lines()
        .filter_map(|line| Some(line.split_once('=')?.0.to_string()))
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn a_session_is_given_only_the_variables_the_table_and_session_pass_env_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = std::env::var("PATH").unwrap();
    let linewrights = [
        ("PATH", path.as_str()),
        ("HOME", "/home/linewright"),
        ("LC_ALL", "C.UTF-8"),
        ("LC_TIME", "C"),
        ("https_proxy", "http://proxy:4750"),
        ("ANTHROPIC_API_KEY", "sk-ant"),
        ("ANTHROPIC_MODEL", "opus"),
        ("CLAUDE_CONFIG_DIR", "/home/linewright/.claude"),
        ("CARGO_HOME", "/usr/local/cargo"),
        ("CARGO_TARGET_DIR", "/tmp/target"),
        ("RUSTUP_HOME", "/usr/local/rustup"),
        ("SCCACHE_DIR", "/var/cache/sccache"),
        ("LINEWRIGHT_WEBHOOK_SECRET", SECRET),
        ("LINEWRIGHT_SESSION_GITHUB_TOKEN", "session-token"),
        ("MY_SECRET", "hunter2"),
        ("TMPDIR", "/tmp"),
        ("GH_TOKEN", "linewright-token"),
    ];
    let session_env = SessionEnv::new(
        PassEnv::new(["CARGO_*", "RUSTUP_HOME", "LINEWRIGHT_*"].map(String::from)).unwrap(),
        SetEnv::default(),
    );
    let harness = harness_without_token(dir.path());
    let mut session = triage_session(dir.path());
    session.environment = Arc::new(
        session_env
            .split(
                linewrights.map(|(name, value)| (name.into(), value.into())),
                &[],
            )
            .given,
    );

    harness.start(&session).await.unwrap();

    assert_eq!(
        session_variables(dir.path()),
        [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_MODEL",
            "CARGO_HOME",
            "CARGO_TARGET_DIR",
            "CLAUDE_CODE_SHELL",
            "CLAUDE_CONFIG_DIR",
            "DISABLE_UPDATES",
            "HOME",
            "LC_ALL",
            "LC_TIME",
            "PATH",
            "RUSTUP_HOME",
            "TMPDIR",
            "https_proxy",
        ]
    );
    let env = std::fs::read_to_string(dir.path().join("env.log")).unwrap();
    assert!(!env.contains(SECRET), "{env}");
    assert!(
        env.contains(&format!("TMPDIR={}\n", dir.path().join("tmp").display())),
        "{env}"
    );
    assert!(env.contains("DISABLE_UPDATES=1\n"), "{env}");
}

#[tokio::test]
async fn a_session_is_given_the_values_session_env_sets_over_passed_ones_and_below_linewrights() {
    let dir = tempfile::tempdir().unwrap();
    let path = std::env::var("PATH").unwrap();
    let linewrights = [
        ("PATH", path.as_str()),
        ("CARGO_HOME", "/usr/local/cargo"),
        ("TMPDIR", "/tmp"),
    ];
    let session_env = SessionEnv::new(
        PassEnv::new(["CARGO_HOME", "TMPDIR"].map(String::from)).unwrap(),
        SetEnv::new(
            [
                ("CARGO_HOME", "/opt/cargo"),
                ("RUST_LOG", "$RUST_LOG"),
                ("EMPTY", ""),
            ]
            .map(|(name, value)| (name.to_string(), value.to_string())),
        )
        .unwrap(),
    );
    let harness = harness_without_token(dir.path());
    let mut session = triage_session(dir.path());
    session.environment = Arc::new(
        session_env
            .split(
                linewrights.map(|(name, value)| (name.into(), value.into())),
                &[],
            )
            .given,
    );

    harness.start(&session).await.unwrap();

    let env = std::fs::read_to_string(dir.path().join("env.log")).unwrap();
    assert_eq!(session_variable(&env, "CARGO_HOME"), Some("/opt/cargo"));
    assert_eq!(session_variable(&env, "RUST_LOG"), Some("$RUST_LOG"));
    assert_eq!(session_variable(&env, "EMPTY"), Some(""));
    let scratch = dir.path().join("tmp");
    assert_eq!(
        session_variable(&env, "TMPDIR"),
        Some(scratch.to_str().unwrap())
    );
}

#[tokio::test]
async fn a_sessions_turns_are_spawned_with_the_environment_the_harness_exposes() {
    let dir = tempfile::tempdir().unwrap();
    let path = std::env::var("PATH").unwrap();
    let session_env = SessionEnv::new(
        PassEnv::new(["CARGO_HOME", "TMPDIR"].map(String::from)).unwrap(),
        SetEnv::default(),
    );
    let harness = harness_without_token(dir.path());
    let mut session = triage_session(dir.path());
    session.effort = Some(Effort::High);
    session.environment = Arc::new(
        session_env
            .split(
                [
                    ("PATH", path.as_str()),
                    ("CARGO_HOME", "/usr/local/cargo"),
                    ("TMPDIR", "/tmp"),
                ]
                .map(|(name, value)| (name.into(), value.into())),
                &[],
            )
            .given,
    );

    let environment = harness.environment(&session.environment, &session.scratch, session.effort);
    harness.start(&session).await.unwrap();

    let scratch = session.scratch.to_str().unwrap();
    let exposed: Vec<(&str, &str)> = environment
        .iter()
        .map(|(name, value)| (name.to_str().unwrap(), value.to_str().unwrap()))
        .collect();
    assert_eq!(
        exposed,
        [
            ("PATH", path.as_str()),
            ("CARGO_HOME", "/usr/local/cargo"),
            ("TMPDIR", "/tmp"),
            ("TMPDIR", scratch),
            ("CLAUDE_CODE_SHELL", "/bin/bash"),
            ("DISABLE_UPDATES", "1"),
            ("CLAUDE_CODE_EFFORT_LEVEL", "high"),
        ]
    );
    let env = std::fs::read_to_string(dir.path().join("env.log")).unwrap();
    let mut spawned: Vec<&str> = env
        .lines()
        .filter(|line| {
            !["PWD=", "OLDPWD=", "SHLVL=", "_="]
                .iter()
                .any(|own| line.starts_with(own))
        })
        .collect();
    spawned.sort_unstable();
    let mut expected = vec![
        format!("PATH={path}"),
        "CARGO_HOME=/usr/local/cargo".to_string(),
        format!("TMPDIR={scratch}"),
        "CLAUDE_CODE_SHELL=/bin/bash".to_string(),
        "DISABLE_UPDATES=1".to_string(),
        "CLAUDE_CODE_EFFORT_LEVEL=high".to_string(),
    ];
    expected.sort_unstable();
    assert_eq!(spawned, expected);
}

#[tokio::test]
async fn every_turn_of_a_session_has_its_scratch_directory_as_its_temporary_directory() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_without_token(dir.path());
    let spec = triage_session(dir.path());

    let turn = harness.start(&spec).await.unwrap();
    harness
        .resume(&spec, &turn.session, "Answer again.")
        .await
        .unwrap();

    let scratch = dir.path().join("tmp");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("tmpdir.log")).unwrap(),
        format!("TMPDIR={0}\nTMPDIR={0}\n", scratch.display())
    );
}

#[tokio::test]
async fn every_turn_of_a_session_is_given_its_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_without_token(dir.path());
    let spec = triage_session(dir.path());

    let turn = harness.start(&spec).await.unwrap();
    harness
        .resume(&spec, &turn.session, "Answer again.")
        .await
        .unwrap();

    let plugin = dir.path().join("plugin");
    assert_eq!(
        turn_args(dir.path())
            .iter()
            .map(|args| flag_value(args, "--plugin-dir"))
            .collect::<Vec<_>>(),
        [plugin.to_str(), plugin.to_str()]
    );
}

#[tokio::test]
async fn every_turn_reads_only_the_repositorys_configuration_and_keeps_no_memory() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_without_token(dir.path());
    let spec = triage_session(dir.path());

    let turn = harness.start(&spec).await.unwrap();
    harness
        .resume(&spec, &turn.session, "Your answer did not parse.")
        .await
        .unwrap();

    let configuration: Vec<(Option<String>, Value, Value)> = turn_args(dir.path())
        .iter()
        .map(|args| {
            let passed = settings(args);
            (
                flag_value(args, "--setting-sources").map(str::to_string),
                passed["autoMemoryEnabled"].clone(),
                passed["disableClaudeAiConnectors"].clone(),
            )
        })
        .collect();
    let isolated = (Some("project,local".to_string()), json!(false), json!(true));
    assert_eq!(configuration, vec![isolated.clone(), isolated]);
}

/// A session whose environment chooses a model and effort of its own.
fn session_choosing_in_its_environment(dir: &Path) -> SessionSpec {
    let mut session = triage_session(dir);
    session.environment = Arc::new(
        SessionEnv::default()
            .split(
                [
                    ("ANTHROPIC_MODEL", "haiku"),
                    ("CLAUDE_CODE_EFFORT_LEVEL", "low"),
                ]
                .map(|(name, value)| (name.into(), value.into())),
                &[],
            )
            .given,
    );
    session
}

fn session_variable<'a>(env: &'a str, name: &str) -> Option<&'a str> {
    env.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
}

#[tokio::test]
async fn a_sessions_model_and_effort_win_over_its_environment_and_the_harnesss_settings() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = session_choosing_in_its_environment(dir.path());
    session.model = Some("opus".into());
    session.effort = Some(Effort::High);
    let harness = harness_without_token(dir.path());

    let turn = harness.start(&session).await.unwrap();
    harness
        .resume(&session, &turn.session, "Answer again.")
        .await
        .unwrap();

    for args in turn_args(dir.path()) {
        assert_eq!(flag_value(&args, "--model"), Some("opus"));
        assert_eq!(flag_value(&args, "--effort"), Some("high"));
        // The settings Linewright passes outrank the repository's.
        let passed = settings(&args);
        assert_eq!(
            (&passed["model"], &passed["effortLevel"]),
            (&json!("opus"), &json!("high"))
        );
    }
    let env = std::fs::read_to_string(dir.path().join("env.log")).unwrap();
    // The harness ranks the variable above every other choice of effort.
    assert_eq!(
        session_variable(&env, "CLAUDE_CODE_EFFORT_LEVEL"),
        Some("high")
    );
}

#[tokio::test]
async fn an_effort_of_max_is_set_but_not_in_the_settings_which_do_not_take_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = triage_session(dir.path());
    session.effort = Some(Effort::Max);

    harness_without_token(dir.path())
        .start(&session)
        .await
        .unwrap();

    let args = &turn_args(dir.path())[0];
    assert_eq!(flag_value(args, "--effort"), Some("max"));
    assert_eq!(settings(args)["effortLevel"], Value::Null);
    let env = std::fs::read_to_string(dir.path().join("env.log")).unwrap();
    assert_eq!(
        session_variable(&env, "CLAUDE_CODE_EFFORT_LEVEL"),
        Some("max")
    );
}

#[tokio::test]
async fn without_a_model_or_effort_a_session_runs_on_the_harnesss_choice() {
    let dir = tempfile::tempdir().unwrap();

    harness_without_token(dir.path())
        .start(&session_choosing_in_its_environment(dir.path()))
        .await
        .unwrap();

    let args = &turn_args(dir.path())[0];
    assert_eq!(flag_value(args, "--model"), None);
    assert_eq!(flag_value(args, "--effort"), None);
    let passed = settings(args);
    assert_eq!(
        (&passed["model"], &passed["effortLevel"]),
        (&Value::Null, &Value::Null)
    );
    let env = std::fs::read_to_string(dir.path().join("env.log")).unwrap();
    assert_eq!(
        session_variable(&env, "CLAUDE_CODE_EFFORT_LEVEL"),
        Some("low")
    );
    assert_eq!(session_variable(&env, "ANTHROPIC_MODEL"), Some("haiku"));
}

#[tokio::test]
async fn every_turn_leaves_out_what_no_brief_uses() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_without_token(dir.path());

    harness.start(&triage_session(dir.path())).await.unwrap();

    let passed = settings(&turn_args(dir.path())[0]);
    assert_eq!(
        [
            "disableBundledSkills",
            "disableWorkflows",
            "disableRemoteControl",
            "disableArtifact"
        ]
        .map(|key| passed[key].clone()),
        [json!(true), json!(true), json!(true), json!(true)]
    );
    let denied = passed["permissions"]["deny"].as_array().unwrap();
    assert_eq!(
        [
            "AskUserQuestion",
            "EnterPlanMode",
            "CronCreate",
            "SendMessage"
        ]
        .map(|tool| denied.contains(&json!(tool))),
        [true, true, true, true]
    );
    assert_eq!(
        [
            "Task",
            "Agent",
            "Skill",
            "Bash",
            "Read",
            "Edit",
            "Write",
            "WebFetch",
            "WebSearch"
        ]
        .map(|tool| denied.contains(&json!(tool))),
        [false; 9]
    );
}

#[tokio::test]
async fn every_turn_leaves_out_the_skills_the_harness_ships() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_without_token(dir.path());

    harness.start(&triage_session(dir.path())).await.unwrap();

    let overrides = &settings(&turn_args(dir.path())[0])["skillOverrides"];
    // The skills Claude Code ships that turning off bundled skills keeps.
    assert_eq!(
        overrides,
        &json!({ "design": "off", "doctor": "off", "plugin-authoring": "off" })
    );
    assert_eq!(
        [
            "linewright:triage",
            "linewright:software-design",
            "linewright:to-tickets",
            "linewright:implement",
            "linewright:code-review",
            "linewright:pr",
            "linewright:tdd",
            "prior-art"
        ]
        .map(|skill| overrides.get(skill).is_some()),
        [false; 8]
    );
}

#[tokio::test]
async fn agent_instructions_above_the_clone_do_not_reach_a_session() {
    let dir = tempfile::Builder::new()
        .prefix("work [a]{b}*?")
        .tempdir()
        .unwrap();
    let harness = harness_without_token(dir.path());

    harness.start(&triage_session(dir.path())).await.unwrap();

    let excludes = instruction_excludes(&turn_args(dir.path())[0]);
    let parent = dir.path().canonicalize().unwrap();
    let clone = parent.join("repo");
    assert_eq!(
        [
            parent.join("CLAUDE.md"),
            parent.join("CLAUDE.local.md"),
            parent.join(".claude/CLAUDE.md"),
            parent.join(".claude/rules/style.md"),
            "/CLAUDE.md".into(),
            clone.join("CLAUDE.md"),
            clone.join("AGENTS.md"),
            clone.join("CLAUDE.local.md"),
            clone.join(".claude/rules/style.md"),
            clone.join("src/CLAUDE.md"),
        ]
        .map(|path| excludes.is_match(path)),
        [
            true, true, true, true, true, false, false, false, false, false
        ]
    );
}

#[tokio::test]
async fn a_clone_under_a_claude_directory_keeps_its_own_agent_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join(".claude/work");
    std::fs::create_dir_all(&work).unwrap();
    let harness = harness_without_token(dir.path());

    harness.start(&triage_session(&work)).await.unwrap();

    let excludes = instruction_excludes(&turn_args(dir.path())[0]);
    let parent = dir.path().canonicalize().unwrap();
    let clone = work.canonicalize().unwrap().join("repo");
    assert_eq!(
        [
            parent.join("CLAUDE.md"),
            parent.join(".claude/CLAUDE.md"),
            clone.join("CLAUDE.md"),
            clone.join("AGENTS.md"),
            clone.join(".claude/rules/style.md"),
        ]
        .map(|path| excludes.is_match(path)),
        [true, true, false, false, false]
    );
}

fn refused_for_the_usage_limit(rate_limit_info: &Value) -> Vec<Value> {
    let session = "9f4fb233-b170-4a09-8ce1-34e0a9c68e36";
    vec![
        json!({"type": "system", "subtype": "init", "session_id": session}),
        json!({"type": "rate_limit_event", "rate_limit_info": rate_limit_info, "session_id": session}),
        json!({
            "type": "result",
            "subtype": "success",
            "is_error": true,
            "terminal_reason": "api_error",
            "api_error_status": 429,
            "result": "You've hit your session limit \u{b7} resets 7:50am (UTC)",
            "session_id": session,
        }),
    ]
}

fn rejected() -> Value {
    json!({
        "status": "rejected",
        "resetsAt": 1_791_877_800,
        "rateLimitType": "five_hour",
        "overageStatus": "rejected",
        "overageDisabledReason": "out_of_credits",
        "isUsingOverage": false,
    })
}

#[tokio::test]
async fn a_session_refused_for_the_usage_limit_ends_saying_when_it_resets() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_streaming(dir.path(), &refused_for_the_usage_limit(&rejected()));

    let error = harness
        .start(&triage_session(dir.path()))
        .await
        .unwrap_err();

    let SessionError::UsageLimit { limit, underway } = error else {
        panic!("expected the usage limit, got {error:?}");
    };
    assert_eq!(limit.resets_at.to_rfc3339(), "2026-10-13T07:50:00+00:00");
    assert_eq!(underway, None, "refused before the agent said anything");
}

#[tokio::test]
async fn a_turn_refused_part_way_for_the_usage_limit_names_its_session_to_resume() {
    let dir = tempfile::tempdir().unwrap();
    let mut stream = refused_for_the_usage_limit(&rejected());
    stream.insert(1, saying("Reading the ticket."));
    let harness = harness_streaming(dir.path(), &stream);

    let error = harness
        .resume(&triage_session(dir.path()), "c0ffee", "Go on.")
        .await
        .unwrap_err();

    let SessionError::UsageLimit { underway, .. } = error else {
        panic!("expected the usage limit, got {error:?}");
    };
    assert_eq!(underway.as_deref(), Some("c0ffee"));
}

#[tokio::test]
async fn a_refusal_that_does_not_say_when_the_limit_resets_is_an_infrastructure_failure() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_streaming(
        dir.path(),
        &refused_for_the_usage_limit(&json!({"status": "rejected"})),
    );

    let error = harness
        .start(&triage_session(dir.path()))
        .await
        .unwrap_err();

    let SessionError::Infrastructure(reason) = error else {
        panic!("expected an infrastructure failure, got {error:?}");
    };
    assert_eq!(
        reason,
        "the harness's API failed (You've hit your session limit \u{b7} resets 7:50am (UTC))"
    );
}

#[tokio::test]
async fn a_session_that_ends_without_a_result_says_the_harness_exited() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_streaming(dir.path(), &[]);

    let error = harness
        .start(&triage_session(dir.path()))
        .await
        .unwrap_err();

    let SessionError::Failed { reason, .. } = error else {
        panic!("expected a failure, got {error:?}");
    };
    assert_eq!(
        reason,
        "the harness exited (exit status: 0) without a result"
    );
}

#[tokio::test]
async fn a_session_that_runs_past_its_deadline_times_out_with_what_it_said() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_running(
        dir.path(),
        vec![Step::Stream(vec![saying("Still reading.")]), Step::Hang],
    );
    let session = triage_session(dir.path());

    let error = timed_out_once_said(harness, session, "Still reading.").await;

    let SessionError::TimedOut { log_tail } = error else {
        panic!("expected a timeout, got {error:?}");
    };
    assert_eq!(log_tail, "Still reading.");
}

#[tokio::test]
async fn a_session_that_finishes_while_warned_of_the_usage_limit_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_streaming(
        dir.path(),
        &[
            json!({
                "type": "rate_limit_event",
                "rate_limit_info": {"status": "allowed_warning", "resetsAt": 1_791_877_800},
            }),
            done(),
        ],
    );

    let turn = harness.start(&triage_session(dir.path())).await.unwrap();

    assert_eq!(turn.message, "Done.");
}

#[tokio::test]
async fn each_turns_output_is_kept_marked_as_the_brief_or_a_follow_up_started_it() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_streaming(
        dir.path(),
        &[
            json!({"type": "system", "subtype": "init", "cwd": "/clone"}),
            json!({
                "type": "assistant",
                "timestamp": "2026-10-13T10:09:11.268Z",
                "message": {"content": [{"type": "text", "text": "Done."}]},
            }),
            done(),
        ],
    );
    let spec = triage_session(dir.path());

    let turn = harness.start(&spec).await.unwrap();
    harness
        .resume(&spec, &turn.session, "Answer again.")
        .await
        .unwrap();

    let record = linewright::transcript::SessionRecord::read(dir.path()).unwrap();
    let session = linewright::transcript::parse(&record);
    let turns: Vec<(&Prompt, Vec<&What>)> = session
        .turns
        .iter()
        .map(|kept| {
            let messages = kept
                .entries
                .iter()
                .map(|entry| &entry.what)
                .filter(|what| matches!(what, What::Message(_)))
                .collect();
            (&kept.prompt, messages)
        })
        .collect();
    let done = What::Message("Done.".into());
    assert_eq!(
        turns,
        [
            (&Prompt::Brief(None), vec![&done]),
            (&Prompt::FollowUp(None), vec![&done])
        ]
    );
    assert!(session.turns.iter().all(|kept| kept.started_at.is_some()));
}

#[tokio::test]
async fn each_line_the_agent_streams_is_in_the_sessions_record_while_it_runs() {
    let dir = tempfile::tempdir().unwrap();
    let go = dir.path().join("go");
    let harness = harness_running(
        dir.path(),
        vec![
            Step::Stream(vec![saying("Looking.")]),
            Step::WaitFor(go.clone()),
            Step::Stream(vec![done()]),
        ],
    );
    let spec = triage_session(dir.path());

    let turn = tokio::spawn(async move { harness.start(&spec).await });
    until_said(dir.path(), "Looking.").await;
    std::fs::write(&go, "").unwrap();

    assert_eq!(turn.await.unwrap().unwrap().message, "Done.");
}

const CREDENTIAL: &str = "ghp_s3cr3t";

/// A session that prints `CREDENTIAL` in all it says, as one that ran `env`
/// would, and ends with `last`, if any.
fn harness_printing_a_secret(dir: &Path, last: Option<Value>) -> ClaudeCode {
    let said = saying(&format!("GH_TOKEN={CREDENTIAL}"));
    let harness = harness_running(
        dir,
        vec![
            Step::Stderr(format!("GH_TOKEN={CREDENTIAL}\n")),
            Step::Stream([said].into_iter().chain(last).collect()),
        ],
    );
    ClaudeCode {
        redaction: redaction_of(&[("GH_TOKEN", CREDENTIAL)]),
        ..harness
    }
}

#[tokio::test]
async fn a_secret_a_session_prints_is_kept_and_answered_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_printing_a_secret(
        dir.path(),
        Some(json!({
            "type": "result", "subtype": "success", "is_error": false,
            "result": format!("It is {CREDENTIAL}."),
        })),
    );

    let turn = harness.start(&triage_session(dir.path())).await.unwrap();

    assert_eq!(turn.message, "It is [redacted].");
    for file in [linewright::transcript::OUTPUT, "stderr.log"] {
        let kept = std::fs::read_to_string(dir.path().join(file)).unwrap();
        assert!(
            !kept.contains(CREDENTIAL) && kept.contains("GH_TOKEN=[redacted]"),
            "{file}: {kept}"
        );
    }
}

#[tokio::test]
async fn a_failed_sessions_log_tail_has_its_secrets_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_printing_a_secret(dir.path(), None);

    let error = harness
        .start(&triage_session(dir.path()))
        .await
        .unwrap_err();

    let SessionError::Failed { log_tail, .. } = error else {
        panic!("expected a failure, got {error:?}");
    };
    assert_eq!(log_tail, "GH_TOKEN=[redacted]");
}

#[tokio::test]
async fn a_failed_sessions_reason_has_its_secrets_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_printing_a_secret(
        dir.path(),
        Some(json!({
            "type": "result", "subtype": "error_max_turns", "is_error": true,
            "result": format!("stuck on {CREDENTIAL}"),
        })),
    );

    let error = harness
        .start(&triage_session(dir.path()))
        .await
        .unwrap_err();

    let SessionError::Failed { reason, .. } = error else {
        panic!("expected a failure, got {error:?}");
    };
    assert_eq!(
        reason,
        "the session ended with `error_max_turns`: stuck on [redacted]"
    );
}

#[tokio::test]
async fn a_secret_that_holds_another_is_redacted_whole() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness_printing_a_secret(dir.path(), None);
    harness.redaction = redaction_of(&[("GH_TOKEN", CREDENTIAL), ("GH_PREFIX_TOKEN", "ghp_s3")]);

    let error = harness
        .start(&triage_session(dir.path()))
        .await
        .unwrap_err();

    let SessionError::Failed { log_tail, .. } = error else {
        panic!("expected a failure, got {error:?}");
    };
    assert_eq!(log_tail, "GH_TOKEN=[redacted]");
}

/// So the SIGINT a terminal sends `serve`'s process group on Ctrl-C, which
/// drains, does not also end the sessions in flight.
#[tokio::test]
async fn a_session_runs_in_a_process_group_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let group = dir.path().join("pgid.log");
    let harness = harness_running(
        dir.path(),
        vec![
            Step::WriteProcessGroup(group.clone()),
            Step::Stream(vec![done()]),
        ],
    );

    harness.start(&triage_session(dir.path())).await.unwrap();

    assert_in_a_process_group_of_its_own(&std::fs::read_to_string(&group).unwrap());
}

/// A Claude Code that, like the shell commands Claude Code runs, starts a
/// process in a session of its own and in the background, writes its PID to
/// `dir/left.pid`, says `Started.` and then carries out `then`.
fn harness_leaving_a_process(dir: &Path, then: Vec<Step>) -> ClaudeCode {
    let turn = [
        Step::LeaveInOwnSession {
            pid: dir.join("left.pid"),
        },
        Step::Stream(vec![saying("Started.")]),
    ];
    harness_running(dir, turn.into_iter().chain(then).collect())
}

fn left_process(dir: &Path) -> rustix::process::Pid {
    let pid = std::fs::read_to_string(dir.join("left.pid")).unwrap();
    rustix::process::Pid::from_raw(pid.trim().parse().unwrap()).unwrap()
}

fn has_ended(pid: rustix::process::Pid) -> bool {
    rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH)
}

/// Without a subreaper, which only Linux has, a process whose parent has
/// exited is beyond reach.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn what_a_session_started_has_ended_once_it_ends_with_a_result() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_leaving_a_process(dir.path(), vec![Step::Stream(vec![done()])]);

    harness.start(&triage_session(dir.path())).await.unwrap();

    assert!(has_ended(left_process(dir.path())));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn what_a_session_started_has_ended_once_it_crashes() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_leaving_a_process(dir.path(), vec![Step::Exit(3)]);

    let error = harness
        .start(&triage_session(dir.path()))
        .await
        .unwrap_err();

    assert!(matches!(error, SessionError::Failed { .. }), "{error:?}");
    assert!(has_ended(left_process(dir.path())));
}

#[tokio::test]
async fn what_a_session_started_has_ended_once_it_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_leaving_a_process(dir.path(), vec![Step::Hang]);
    let session = triage_session(dir.path());

    let error = timed_out_once_said(harness, session, "Started.").await;

    assert!(matches!(error, SessionError::TimedOut { .. }), "{error:?}");
    assert!(has_ended(left_process(dir.path())));
}

/// As when its run is cancelled.
#[tokio::test]
async fn what_a_session_started_ends_once_its_turn_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_leaving_a_process(dir.path(), vec![Step::Hang]);
    let session = triage_session(dir.path());

    let turn = tokio::spawn(async move { harness.start(&session).await });
    until_said(dir.path(), "Started.").await;
    turn.abort();
    let left = left_process(dir.path());

    eventually(|| has_ended(left)).await;
}

#[tokio::test]
async fn a_harness_that_cannot_be_started_is_an_infrastructure_failure() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("missing");
    let harness = claude_code(&program);

    let error = harness
        .start(&triage_session(dir.path()))
        .await
        .unwrap_err();

    let SessionError::Infrastructure(reason) = error else {
        panic!("expected an infrastructure failure, got {error:?}");
    };
    assert_eq!(
        reason,
        format!(
            "starting {}: No such file or directory (os error 2)",
            program.display()
        )
    );
}

/// Claude Code's background commands share its standard output, and a turn
/// reads that output to its end.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_process_a_session_left_holding_its_output_does_not_hold_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness_running(
        dir.path(),
        vec![Step::LeaveHoldingOutput, Step::Stream(vec![done()])],
    );

    let turn = harness.start(&triage_session(dir.path())).await.unwrap();

    assert_eq!(turn.message, "Done.");
}
