//! The Claude Code runtime against a stand-in `claude` program: what a
//! session's environment holds and what configuration each turn reads.

mod common;

use std::path::Path;

use common::*;
use linewright::domain::Number;
use linewright::reconcile::{Subject, WorkKind};
use linewright::runtime::claude::ClaudeCode;
use linewright::runtime::{Purpose, Runtime, SessionError, SessionSpec};
use linewright::transcript::{Prompt, What};
use serde_json::{Value, json};

fn runtime_without_token(dir: &Path) -> ClaudeCode {
    runtime_streaming(
        dir,
        &[json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done."})],
    )
}

/// A runtime without a GitHub token whose every turn streams `events`.
fn runtime_streaming(dir: &Path, events: &[Value]) -> ClaudeCode {
    ClaudeCode {
        program: fake_claude_streaming(dir, events).to_str().unwrap().into(),
        bash: "/bin/bash".into(),
        model: None,
        github_token: None,
    }
}

/// The arguments of each turn the stand-in `claude` in `dir` ran.
fn turn_args(dir: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(dir.join("args.log"))
        .unwrap()
        .split_terminator("\n\n")
        .map(|turn| turn.lines().map(str::to_string).collect())
        .collect()
}

/// The value following `flag` in one turn's arguments.
fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let at = args.iter().position(|arg| arg == flag)?;
    args.get(at + 1).map(String::as_str)
}

/// The settings one turn passed on the command line.
fn settings(args: &[String]) -> Value {
    flag_value(args, "--settings").map_or(Value::Null, |s| serde_json::from_str(s).unwrap())
}

/// The agent instruction files one turn excluded. `globset` stands in for
/// Claude Code's own matcher, which also treats parentheses as special.
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

/// A triage session laid out under `dir` as a run lays it out: its clone in
/// `repo` with its scratch directory `tmp` beside it, and its record in `dir`.
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
        plugin: dir.join("plugin"),
        prompt: "Triage #1.".into(),
        deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(30),
    }
}

#[tokio::test]
async fn a_production_session_gets_linewrights_github_token() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_claude(dir.path(), "Done.");
    let runtime = ClaudeCode {
        program: program.to_str().unwrap().into(),
        bash: "/bin/bash".into(),
        model: None,
        github_token: Some("linewright-token".into()),
    };

    runtime.start(&triage_session(dir.path())).await.unwrap();

    assert_eq!(
        std::fs::read_to_string(dir.path().join("tokens.log")).unwrap(),
        "GH_TOKEN=linewright-token GITHUB_TOKEN=linewright-token\n"
    );
}

#[tokio::test]
async fn every_turn_of_a_session_has_its_scratch_directory_as_its_temporary_directory() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_without_token(dir.path());
    let spec = triage_session(dir.path());

    let turn = runtime.start(&spec).await.unwrap();
    runtime
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
    let runtime = runtime_without_token(dir.path());
    let spec = triage_session(dir.path());

    let turn = runtime.start(&spec).await.unwrap();
    runtime
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
    let runtime = runtime_without_token(dir.path());
    let spec = triage_session(dir.path());

    let turn = runtime.start(&spec).await.unwrap();
    runtime
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

#[tokio::test]
async fn agent_instructions_above_the_clone_do_not_reach_a_session() {
    let dir = tempfile::Builder::new()
        .prefix("work [a]{b}*?")
        .tempdir()
        .unwrap();
    let runtime = runtime_without_token(dir.path());

    runtime.start(&triage_session(dir.path())).await.unwrap();

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
    let runtime = runtime_without_token(dir.path());

    runtime.start(&triage_session(&work)).await.unwrap();

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

/// The stream of a session the runtime refused for the usage limit, as
/// Claude Code streamed one, with the `rate_limit_info` it gave.
fn refused_for_the_usage_limit(rate_limit_info: Value) -> Vec<Value> {
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

/// The `rate_limit_info` of a refusal for the usage limit that resets at
/// 7:50 UTC on 13 October 2026.
fn rejected() -> Value {
    json!({
        "status": "rejected",
        "resetsAt": 1791877800,
        "rateLimitType": "five_hour",
        "overageStatus": "rejected",
        "isUsingOverage": false,
    })
}

#[tokio::test]
async fn a_session_refused_for_the_usage_limit_ends_saying_when_it_resets() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_streaming(dir.path(), &refused_for_the_usage_limit(rejected()));

    let error = runtime
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
    let mut stream = refused_for_the_usage_limit(rejected());
    stream.insert(
        1,
        json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": "Reading the ticket."}]},
        }),
    );
    let runtime = runtime_streaming(dir.path(), &stream);

    let error = runtime
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
    let runtime = runtime_streaming(
        dir.path(),
        &refused_for_the_usage_limit(json!({"status": "rejected"})),
    );

    let error = runtime
        .start(&triage_session(dir.path()))
        .await
        .unwrap_err();

    assert!(
        matches!(error, SessionError::Infrastructure(_)),
        "expected an infrastructure failure, got {error:?}"
    );
}

#[tokio::test]
async fn a_session_that_finishes_while_warned_of_the_usage_limit_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_streaming(
        dir.path(),
        &[
            json!({
                "type": "rate_limit_event",
                "rate_limit_info": {"status": "allowed_warning", "resetsAt": 1791877800},
            }),
            json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done."}),
        ],
    );

    let turn = runtime.start(&triage_session(dir.path())).await.unwrap();

    assert_eq!(turn.message, "Done.");
}

#[tokio::test]
async fn each_turns_output_is_kept_marked_as_the_brief_or_a_follow_up_started_it() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime_streaming(
        dir.path(),
        &[
            json!({"type": "system", "subtype": "init", "cwd": "/clone"}),
            json!({
                "type": "assistant",
                "timestamp": "2026-10-13T10:09:11.268Z",
                "message": {"content": [{"type": "text", "text": "Done."}]},
            }),
            json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done."}),
        ],
    );
    let spec = triage_session(dir.path());

    let turn = runtime.start(&spec).await.unwrap();
    runtime
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
