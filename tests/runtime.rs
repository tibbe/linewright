//! The Claude Code runtime against a stand-in `claude` program: what a
//! session's environment holds and what configuration each turn reads.

mod common;

use std::path::Path;

use common::*;
use linewright::domain::Number;
use linewright::reconcile::{Subject, WorkKind};
use linewright::runtime::claude::ClaudeCode;
use linewright::runtime::{Purpose, Runtime, SessionSpec};
use serde_json::{Value, json};

fn runtime_without_token(dir: &Path) -> ClaudeCode {
    ClaudeCode {
        program: fake_claude(dir, "Done.").to_str().unwrap().into(),
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
        prompt: "Triage #1.".into(),
        deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(30),
    }
}

#[tokio::test]
async fn a_production_session_gets_the_machine_users_github_token() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_claude(dir.path(), "Done.");
    let runtime = ClaudeCode {
        program: program.to_str().unwrap().into(),
        bash: "/bin/bash".into(),
        model: None,
        github_token: Some("machine-token".into()),
    };

    runtime.start(&triage_session(dir.path())).await.unwrap();

    assert_eq!(
        std::fs::read_to_string(dir.path().join("tokens.log")).unwrap(),
        "GH_TOKEN=machine-token GITHUB_TOKEN=machine-token\n"
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
