mod common;

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use chrono::TimeDelta;
use common::*;
use linewright::case::{Case, Event, copy_sessions, replay};
use linewright::domain::{Login, MergeMethod, Number, UpdateMethod};
use linewright::reconcile::{Subject, WorkKind};
use serde_json::json;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn spec_change_case(dir: &Path, files: &[(&str, &str)]) {
    spec_change_case_allowing(dir, files, &MergeMethod::ALL);
}

fn spec_change_case_allowing(
    dir: &Path,
    files: &[(&str, &str)],
    allowed_merge_methods: &[MergeMethod],
) {
    let repo = dir.join("source");
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    git(&repo, &["init", "--quiet", "-b", "main"]);
    git(&repo, &["config", "user.name", "maintainer"]);
    git(&repo, &["config", "user.email", "maintainer@example.com"]);
    std::fs::write(repo.join("docs/spec.md"), "# Spec\n").unwrap();
    for (path, contents) in files {
        let path = repo.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    git(&repo, &["add", "--all"]);
    git(&repo, &["commit", "--quiet", "-m", "Start"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    std::fs::write(repo.join("docs/spec.md"), "# Spec\n\nGadgets exist.\n").unwrap();
    git(&repo, &["commit", "--quiet", "-am", "Gadgets"]);
    let merge = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["update-ref", "refs/heads/linewright-base", &base]);
    git(
        &repo,
        &["update-ref", "refs/heads/linewright-merge", &merge],
    );
    let case = dir.join("case");
    std::fs::create_dir_all(&case).unwrap();
    let bundle = case.join("repo.bundle");
    git(
        &repo,
        &[
            "bundle",
            "create",
            "--quiet",
            bundle.to_str().unwrap(),
            "refs/heads/linewright-base",
            "refs/heads/linewright-merge",
        ],
    );
    let case_json = Case {
        repo: "acme/widgets".into(),
        default_branch: "main".into(),
        linewright: Login::new(LINEWRIGHT),
        issue_types: None,
        update_method: UpdateMethod::Merge,
        allowed_merge_methods: allowed_merge_methods.iter().copied().collect(),
        base,
        event: Event::SpecChange {
            number: Number(41),
            title: "Gadgets".into(),
            body: "Adds gadgets.".into(),
            author: Login::new("maintainer"),
            merge_commit: merge,
            commits: vec![],
        },
    };
    std::fs::write(
        case.join("case.json"),
        serde_json::to_string_pretty(&case_json).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn a_replayed_spec_change_leaves_its_tickets_pull_requests_and_session_records_as_files() {
    let dir = tempfile::tempdir().unwrap();
    spec_change_case(dir.path(), &[]);
    let runtime = Arc::new(ScriptedRuntime::default());
    runtime.on(WorkKind::Tickets, |call| {
        assert!(call.spec.prompt.contains("+Gadgets exist."));
        json(json!({
            "tickets": [{"id": "a", "title": "Gadgets", "body": "Build gadgets.", "blocked_by": []}],
            "comment": "One slice."
        }))
    });
    runtime.on(WorkKind::Implement, |call| {
        assert!(call.spec.prompt.contains("+Gadgets exist."));
        commit_in(
            &call.spec.dir,
            &[("src/gadgets.txt", "gadget\n")],
            "Gadgets",
        );
        json(json!({"outcome": "done", "pr_title": "Add gadgets", "pr_body": "Adds gadgets."}))
    });
    let out = dir.path().join("out");

    replay(
        &dir.path().join("case"),
        &out,
        runtime,
        Arc::new(FakeClock::new()),
        config(&out),
    )
    .await
    .unwrap();

    let read = |path: &str| std::fs::read_to_string(out.join(path)).unwrap();
    assert!(read("issues/2.md").starts_with("# #2 Spec change: Gadgets"));
    assert!(read("issues/3.md").contains("- Labels: ready-for-agent"));
    assert!(read("issues/3.md").contains("- Parent: #2\n"));
    let pull = read("pulls/4.md");
    assert!(pull.contains("- State: ready for review"), "{pull}");
    assert!(
        pull.contains(&format!("- Assignees: {LINEWRIGHT}\n")),
        "{pull}"
    );
    assert!(read("pulls/4.diff").contains("+gadget"));
    let sessions: Vec<_> = std::fs::read_dir(out.join("sessions/pr-1"))
        .unwrap()
        .collect();
    let session = sessions[0].as_ref().unwrap().path();
    for file in ["brief.md", "result.json", "transcript.jsonl"] {
        assert!(
            session.join(file).exists(),
            "{file} in {}",
            session.display()
        );
    }
    assert!(out.join("sessions/issue-3").is_dir());
}

/// Returns the fake's directory, which is also Claude Code's config
/// directory.
fn replay_with_fake_claude(dir: &Path, env: &[(&str, &str)]) -> std::path::PathBuf {
    spec_change_case(dir, &[]);
    let bin = dir.join("bin");
    std::fs::create_dir(&bin).unwrap();
    fake_claude(&bin, r#"{"tickets": [], "comment": "Nothing to build."}"#);
    replay_with(dir, &bin, env);
    bin
}

/// Replays the case in `dir` with the Claude Code in `bin`.
fn replay_with(dir: &Path, bin: &Path, env: &[(&str, &str)]) {
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());

    let output = Command::new(env!("CARGO_BIN_EXE_lw"))
        .args(["replay", "case", "--out", "out"])
        .current_dir(dir)
        .env("PATH", path)
        .env("CLAUDE_CONFIG_DIR", bin)
        .envs(env.iter().copied())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Each value holds `s3cr3t`, so that none is found in any form, escaped
/// or not.
const CREDENTIALS: [(&str, &str); 5] = [
    ("GH_TOKEN", "gh-s3cr3t"),
    ("GITHUB_TOKEN", "github-s3cr3t"),
    ("LINEWRIGHT_SESSION_GITHUB_TOKEN", "session-s3cr3t"),
    ("LINEWRIGHT_WEBHOOK_SECRET", r#"webhook "s3cr3t" \ too"#),
    ("ANTHROPIC_API_KEY", "anthropic-s3cr3t"),
];

/// Every file under `dir`, with its contents.
fn files_under(dir: &Path) -> Vec<(std::path::PathBuf, String)> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            let contents = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
            files.push((path, contents));
        }
    }
    files
}

#[test]
fn a_replayed_session_that_prints_linewrights_credentials_leaves_them_redacted() {
    let dir = tempfile::tempdir().unwrap();
    spec_change_case(dir.path(), &[]);
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    // What the session saw when it ran `env`.
    let env: String = CREDENTIALS
        .iter()
        .map(|(variable, value)| format!("{variable}={value} "))
        .collect();
    let said =
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": env}]}});
    let result = json!({"tickets": [], "comment": format!("Saw {env}")}).to_string();
    let result =
        json!({"type": "result", "subtype": "success", "is_error": false, "result": result});
    fake_claude_saying(&bin, &[said.clone(), result], &said, &env);

    replay_with(dir.path(), &bin, &CREDENTIALS);

    let out = files_under(&dir.path().join("out"));
    for (path, contents) in &out {
        assert!(
            !contents.contains("s3cr3t"),
            "a credential in {}: {contents}",
            path.display()
        );
    }
    let redacted = |name: &str| {
        out.iter()
            .filter(|(path, _)| path.file_name().unwrap() == name)
            .all(|(_, contents)| contents.contains("GH_TOKEN=[redacted] "))
    };
    assert!(
        ["output.jsonl", "transcript.jsonl", "stderr.log"].map(redacted) == [true; 3],
        "{out:#?}"
    );
    let pull = std::fs::read_to_string(dir.path().join("out/pulls/1.md")).unwrap();
    assert!(
        pull.contains(
            "Saw GH_TOKEN=[redacted] GITHUB_TOKEN=[redacted] \
             LINEWRIGHT_SESSION_GITHUB_TOKEN=[redacted] \
             LINEWRIGHT_WEBHOOK_SECRET=[redacted] ANTHROPIC_API_KEY=[redacted]"
        ),
        "{pull}"
    );
}

#[test]
fn a_replayed_session_gets_no_github_token_whatever_the_operators_environment_holds() {
    let dir = tempfile::tempdir().unwrap();

    let bin = replay_with_fake_claude(
        dir.path(),
        &[
            ("GH_TOKEN", "operator-token"),
            ("GITHUB_TOKEN", "operator-token"),
        ],
    );

    assert_eq!(
        std::fs::read_to_string(bin.join("tokens.log")).unwrap(),
        "GH_TOKEN= GITHUB_TOKEN=\n"
    );
}

#[test]
fn a_replayed_session_with_linewrights_credentials_empty_keeps_all_it_said() {
    let dir = tempfile::tempdir().unwrap();
    let empty = CREDENTIALS.map(|(variable, _)| (variable, ""));

    replay_with_fake_claude(dir.path(), &empty);

    let pull = std::fs::read_to_string(dir.path().join("out/pulls/1.md")).unwrap();
    assert!(pull.contains("Nothing to build."), "{pull}");
}

#[test]
fn a_replayed_session_runs_its_commands_in_bash_whatever_the_operators_shell() {
    let dir = tempfile::tempdir().unwrap();

    let bin = replay_with_fake_claude(
        dir.path(),
        &[("SHELL", "/bin/zsh"), ("CLAUDE_CODE_SHELL", "/bin/zsh")],
    );

    let shell = std::fs::read_to_string(bin.join("shell.log")).unwrap();
    let shell = Path::new(shell.trim_end());
    let version = Command::new(shell).arg("--version").output().unwrap();
    assert!(
        String::from_utf8_lossy(&version.stdout).starts_with("GNU bash"),
        "{}",
        shell.display()
    );
}

#[tokio::test]
async fn a_replay_runs_no_workflows() {
    let dir = tempfile::tempdir().unwrap();
    spec_change_case(
        dir.path(),
        &[(
            ".linewright/workflows/hourly.md",
            "---\non:\n  schedule: \"0 * * * *\"\n---\nLook around.\n",
        )],
    );
    let clock = Arc::new(FakeClock::new());
    let runtime = Arc::new(ScriptedRuntime::default());
    runtime.on(WorkKind::Tickets, {
        let clock = clock.clone();
        move |_| {
            // Past the workflow's next tick, while the replay goes on.
            clock.advance(TimeDelta::hours(2));
            json(json!({"tickets": [{"id": "a", "title": "Gadgets", "body": "Build gadgets.", "blocked_by": []}], "comment": "One slice."}))
        }
    });
    runtime.on(WorkKind::Implement, |call| {
        commit_in(
            &call.spec.dir,
            &[("src/gadgets.txt", "gadget\n")],
            "Gadgets",
        );
        json(json!({"outcome": "done", "pr_title": "Add gadgets", "pr_body": "Adds gadgets."}))
    });
    runtime.on_workflow(|_| json(json!({"summary": "Nothing needs doing.", "issues": []})));
    let out = dir.path().join("out");

    replay(
        &dir.path().join("case"),
        &out,
        runtime.clone(),
        clock,
        config(&out),
    )
    .await
    .unwrap();

    assert!(runtime.workflow_prompts().is_empty());
}

#[tokio::test]
async fn a_replay_with_auto_merge_merges_each_pull_request_and_runs_the_tickets_it_blocked() {
    let dir = tempfile::tempdir().unwrap();
    spec_change_case(dir.path(), &[]);
    let runtime = Arc::new(ScriptedRuntime::default());
    runtime.on(WorkKind::Tickets, |_| {
        json(json!({
            "tickets": [
                {"id": "store", "title": "Store gadgets", "body": "Gadgets persist.", "blocked_by": []},
                {"id": "list", "title": "List gadgets", "body": "List them.", "blocked_by": ["store"]}
            ],
            "comment": "Two slices."
        }))
    });
    runtime.on(WorkKind::Implement, |call| {
        let file = format!("src/{}.txt", subject_file(call));
        commit_in(&call.spec.dir, &[(&file, "gadget\n")], "Gadgets");
        json(json!({"outcome": "done", "pr_title": "Add gadgets", "pr_body": "Adds gadgets."}))
    });
    let out = dir.path().join("out");
    let mut config = config(&out);
    config.auto_merge = true;

    replay(
        &dir.path().join("case"),
        &out,
        runtime,
        Arc::new(FakeClock::new()),
        config,
    )
    .await
    .unwrap();

    let read = |path: &str| std::fs::read_to_string(out.join(path)).unwrap();
    for pull in ["pulls/5.md", "pulls/6.md"] {
        assert!(read(pull).contains("- State: merged at"), "{pull}");
    }
    for issue in ["issues/2.md", "issues/3.md", "issues/4.md"] {
        assert!(read(issue).contains("- State: closed"), "{issue}");
    }
}

#[tokio::test]
async fn a_replay_with_auto_merge_refuses_a_merge_method_the_case_repository_does_not_allow() {
    let dir = tempfile::tempdir().unwrap();
    spec_change_case_allowing(dir.path(), &[], &[MergeMethod::Rebase]);
    let out = dir.path().join("out");
    let mut config = config(&out);
    config.auto_merge = true;
    config.merge_method = Some(MergeMethod::Merge);

    let error = replay(
        &dir.path().join("case"),
        &out,
        Arc::new(ScriptedRuntime::default()),
        Arc::new(FakeClock::new()),
        config,
    )
    .await
    .unwrap_err();

    assert!(format!("{error:#}").contains("only `rebase`"), "{error:#}");
}

#[test]
fn a_replayed_sessions_record_keeps_its_transcript() {
    let dir = tempfile::tempdir().unwrap();

    replay_with_fake_claude(dir.path(), &[]);

    let sessions: Vec<_> = std::fs::read_dir(dir.path().join("out/sessions/pr-1"))
        .unwrap()
        .collect();
    let session = sessions[0].as_ref().unwrap().path();
    assert_eq!(
        std::fs::read_to_string(session.join("transcript.jsonl")).unwrap(),
        "{\"type\":\"said\"}\n"
    );
    assert!(!session.join("trace.jsonl").exists());
}

#[test]
fn a_captured_case_keeps_the_transcripts_of_the_sessions_that_handled_it() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let session = sessions.join("issue-3/20260101T000000Z-triage");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("transcript.jsonl"), "{\"type\":\"said\"}\n").unwrap();
    std::fs::create_dir_all(sessions.join("issue-4/20260101T000000Z-triage")).unwrap();
    let case = dir.path().join("case");

    copy_sessions(&[Subject::Issue(Number(3))], &sessions, &case).unwrap();

    assert_eq!(
        std::fs::read_to_string(
            case.join("transcripts/issue-3/20260101T000000Z-triage/transcript.jsonl")
        )
        .unwrap(),
        "{\"type\":\"said\"}\n"
    );
    assert!(!case.join("transcripts/issue-4").exists());
    assert!(!case.join("traces").exists());
}
