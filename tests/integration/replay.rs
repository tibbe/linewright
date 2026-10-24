use crate::common;

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use chrono::TimeDelta;
use common::*;
use linewright::case::{Case, Event, copy_sessions, replay};
use linewright::domain::{AutoMerge, Login, MergeMethod, Number, UpdateMethod};
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
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Design, |call| {
        assert!(call.spec.prompt.contains("+Gadgets exist."));
        Ok(json!({
            "outcome": "designed", "design": "A gadget store.", "tickets": [{"id": "a", "title": "Gadgets", "body": "Build gadgets.", "blocked_by": []}],
            "comment": "One slice."
        }).to_string())
    });
    harness.on(WorkKind::Implement, |call| {
        assert!(call.spec.prompt.contains("+Gadgets exist."));
        commit_in(
            &call.spec.dir,
            &[("src/gadgets.txt", "gadget\n")],
            "Gadgets",
        );
        Ok(
            json!({"outcome": "done", "pr_title": "Add gadgets", "pr_body": "Adds gadgets."})
                .to_string(),
        )
    });
    let out = dir.path().join("out");

    replay(
        &dir.path().join("case"),
        &out,
        harness,
        Arc::new(FakeClock::new()),
        config(&out),
    )
    .await
    .unwrap();

    let read = |path: &str| std::fs::read_to_string(out.join(path)).unwrap();
    assert!(read("issues/2.md").starts_with("# #2 Spec change: Gadgets"));
    assert!(read("issues/3.md").contains("- Labels: linewright:designed, ready-for-agent"));
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
    let bin = case_with_fake_claude(dir);
    replay_with(dir, &bin, env);
    bin
}

/// Makes a case in `dir` and a Claude Code that finds nothing to build,
/// returning the fake's directory.
fn case_with_fake_claude(dir: &Path) -> std::path::PathBuf {
    spec_change_case(dir, &[]);
    let bin = dir.join("bin");
    std::fs::create_dir(&bin).unwrap();
    fake_claude(
        &bin,
        r#"{"outcome": "nothing-to-build", "reason": "Nothing to build."}"#,
    );
    bin
}

/// Replays the case in `dir` with the Claude Code in `bin`, returning what
/// it logged.
fn replay_with(dir: &Path, bin: &Path, env: &[(&str, &str)]) -> String {
    let output = replay_output(dir, bin, env.iter().copied());
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "{log}");
    log
}

/// As `replay_with`, but returns how the replay ended, whether it
/// succeeded or not.
fn replay_output(
    dir: &Path,
    bin: &Path,
    env: impl IntoIterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>,
) -> std::process::Output {
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    Command::new(env!("CARGO_BIN_EXE_lw"))
        .args(["replay", "case", "--out", "out"])
        .current_dir(dir)
        .env("PATH", path)
        .env("CLAUDE_CONFIG_DIR", bin)
        .envs(env)
        .output()
        .unwrap()
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
    let result = json!({"outcome": "nothing-to-build", "reason": format!("Saw {env}")}).to_string();
    let result =
        json!({"type": "result", "subtype": "success", "is_error": false, "result": result});
    fake_claude_saying(&bin, &[said.clone(), result], &said, &env);

    replay_with(dir.path(), &bin, &CREDENTIALS);

    let out = files_under(&dir.path().join("out"));
    for (path, contents) in &out {
        assert!(
            !contents.contains("s3cr3t"),
            "a secret in {}: {contents}",
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

/// Replays a session that says `saw`, with `env`, returning the pull
/// request it leaves and what it logged.
fn replay_saying(dir: &Path, env: &[(&str, &str)], saw: &str) -> (String, String) {
    spec_change_case(dir, &[]);
    let bin = dir.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let result = json!({"outcome": "nothing-to-build", "reason": format!("Saw {saw}")}).to_string();
    let result =
        json!({"type": "result", "subtype": "success", "is_error": false, "result": result});
    let said =
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": saw}]}});
    fake_claude_saying(&bin, &[said.clone(), result], &said, saw);

    let log = replay_with(dir, &bin, env);

    for (path, contents) in files_under(&dir.join("out")) {
        assert!(
            !contents.contains("s3cr3t"),
            "a secret in {}: {contents}",
            path.display()
        );
    }
    let pull = std::fs::read_to_string(dir.join("out/pulls/1.md")).unwrap();
    (pull, log)
}

#[test]
fn a_replayed_session_leaves_redacted_the_variables_secrets_names_and_none_that_only_look_secret() {
    let dir = tempfile::tempdir().unwrap();
    let env = [
        ("LINEWRIGHT_SECRETS", "AWS_SECRET_ACCESS_KEY,NPM_*,UNSET_*"),
        ("AWS_SECRET_ACCESS_KEY", "aws-s3cr3t"),
        ("NPM_TOKEN", "npm-s3cr3t"),
        ("NPM_REGISTRY", "registry-s3cr3t.example.com"),
        ("NPM_EMPTY", ""),
        ("DATABASE_PASSWORD", "database-password"),
        ("SIGNING_KEY", "signing-key"),
        ("WEBHOOK_SECRET", "webhook-secret"),
        ("CI_TOKEN", "ci-token"),
    ];
    let saw = "AWS_SECRET_ACCESS_KEY=aws-s3cr3t NPM_TOKEN=npm-s3cr3t \
               NPM_REGISTRY=registry-s3cr3t.example.com NPM_EMPTY= \
               DATABASE_PASSWORD=database-password SIGNING_KEY=signing-key \
               WEBHOOK_SECRET=webhook-secret CI_TOKEN=ci-token";

    let (pull, _) = replay_saying(dir.path(), &env, saw);

    assert!(
        pull.contains(
            "Saw AWS_SECRET_ACCESS_KEY=[redacted] NPM_TOKEN=[redacted] \
             NPM_REGISTRY=[redacted] NPM_EMPTY= \
             DATABASE_PASSWORD=database-password SIGNING_KEY=signing-key \
             WEBHOOK_SECRET=webhook-secret CI_TOKEN=ci-token"
        ),
        "{pull}"
    );
}

#[test]
fn a_replayed_session_leaves_a_secret_of_1_to_5_characters_in_what_it_said_and_warns_of_it() {
    let dir = tempfile::tempdir().unwrap();
    let env = [
        ("LINEWRIGHT_SECRETS", "PIN,LONG_PIN"),
        // Five characters in ten bytes.
        ("PIN", "ééééé"),
        ("LONG_PIN", "s3cr3t"),
        ("ANTHROPIC_API_KEY", "12345"),
    ];
    let saw = "PIN=ééééé LONG_PIN=s3cr3t ANTHROPIC_API_KEY=12345";

    let (pull, log) = replay_saying(dir.path(), &env, saw);

    assert!(
        pull.contains("Saw PIN=ééééé LONG_PIN=[redacted] ANTHROPIC_API_KEY=12345"),
        "{pull}"
    );
    let warned: Vec<String> = log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|line| line["level"] == "WARN")
        .filter_map(|line| line["fields"]["variable"].as_str().map(String::from))
        .collect();
    assert_eq!(warned, ["ANTHROPIC_API_KEY", "PIN"], "{log}");
}

#[test]
fn a_replayed_session_leaves_redacted_the_credentials_the_table_passes_unnamed() {
    let dir = tempfile::tempdir().unwrap();
    let env = [
        ("LINEWRIGHT_SESSION_ENV", "AWS_BEARER_TOKEN_BEDROCK"),
        ("CLAUDE_CODE_OAUTH_TOKEN", "oauth-s3cr3t"),
        ("ANTHROPIC_AUTH_TOKEN", "auth-s3cr3t"),
        ("HTTPS_PROXY", "http://bot:p%40s3cr3t@proxy:3128"),
        ("http_proxy", "http://bot:p%40s@proxy:3128"),
        ("AWS_BEARER_TOKEN_BEDROCK", "bedrock-token"),
    ];
    let saw = "oauth-s3cr3t auth-s3cr3t http://bot:p%40s3cr3t@proxy:3128 p@s3cr3t \
               bedrock-token";

    let (pull, log) = replay_saying(dir.path(), &env, saw);

    assert!(
        pull.contains(
            "Saw [redacted] [redacted] http://bot:[redacted]@proxy:3128 [redacted] bedrock-token"
        ),
        "{pull}"
    );
    let warned: Vec<String> = log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|line| line["level"] == "WARN")
        .filter_map(|line| line["fields"]["variable"].as_str().map(String::from))
        .collect();
    assert_eq!(warned, ["http_proxy"], "{log}");
}

#[test]
fn a_replay_with_secrets_naming_an_unset_variable_runs_no_session() {
    let dir = tempfile::tempdir().unwrap();
    let bin = case_with_fake_claude(dir.path());

    let output = replay_output(
        dir.path(),
        &bin,
        [("LINEWRIGHT_SECRETS", "LINEWRIGHT_TEST_UNSET_SECRET")],
    );

    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{error}");
    assert!(
        error.contains(
            "the setting `secrets` names `LINEWRIGHT_TEST_UNSET_SECRET`, which is unset or \
             empty: set it, or take it out of `secrets`"
        ),
        "{error}"
    );
    assert!(!dir.path().join("out/sessions").exists());
}

#[test]
fn a_replay_with_secrets_naming_a_variable_set_to_a_value_that_is_not_utf_8_runs() {
    let dir = tempfile::tempdir().unwrap();
    let bin = case_with_fake_claude(dir.path());
    let not_utf_8 = OsStr::from_bytes(b"s3cr3t\xff").to_os_string();

    let output = replay_output(
        dir.path(),
        &bin,
        [
            (
                OsString::from("LINEWRIGHT_SECRETS"),
                OsString::from("BINARY_SECRET"),
            ),
            (OsString::from("BINARY_SECRET"), not_utf_8),
        ],
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
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

#[test]
fn a_replayed_session_cannot_update_the_harness_whatever_session_env_passes() {
    let dir = tempfile::tempdir().unwrap();

    let bin = replay_with_fake_claude(
        dir.path(),
        &[
            ("DISABLE_UPDATES", "0"),
            ("LINEWRIGHT_SESSION_ENV", "DISABLE_UPDATES"),
        ],
    );

    let env = std::fs::read_to_string(bin.join("env.log")).unwrap();
    let values: Vec<&str> = env
        .lines()
        .filter(|line| line.starts_with("DISABLE_UPDATES="))
        .collect();
    assert_eq!(values, ["DISABLE_UPDATES=1"], "{env}");
}

#[tokio::test]
async fn a_replay_runs_no_automations() {
    let dir = tempfile::tempdir().unwrap();
    spec_change_case(
        dir.path(),
        &[(
            ".linewright/automations/hourly.md",
            "---\non:\n  schedule: \"0 * * * *\"\n---\nLook around.\n",
        )],
    );
    let clock = Arc::new(FakeClock::new());
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Design, {
        let clock = clock.clone();
        move |_| {
            // Past the automation's next tick, while the replay goes on.
            clock.advance(TimeDelta::hours(2));
            Ok(json!({"outcome": "designed", "design": "A gadget store.", "tickets": [{"id": "a", "title": "Gadgets", "body": "Build gadgets.", "blocked_by": []}], "comment": "One slice."}).to_string())
        }
    });
    harness.on(WorkKind::Implement, |call| {
        commit_in(
            &call.spec.dir,
            &[("src/gadgets.txt", "gadget\n")],
            "Gadgets",
        );
        Ok(
            json!({"outcome": "done", "pr_title": "Add gadgets", "pr_body": "Adds gadgets."})
                .to_string(),
        )
    });
    harness.on_automation(|_| {
        Ok(json!({"summary": "Nothing needs doing.", "issues": []}).to_string())
    });
    let out = dir.path().join("out");

    replay(
        &dir.path().join("case"),
        &out,
        harness.clone(),
        clock,
        config(&out),
    )
    .await
    .unwrap();

    assert_eq!(harness.automation_prompts(), Vec::<String>::new());
}

#[tokio::test]
async fn a_replay_with_auto_merge_merges_each_pull_request_and_runs_the_tickets_it_blocked() {
    let dir = tempfile::tempdir().unwrap();
    spec_change_case(dir.path(), &[]);
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Design, |_| {
        Ok(json!({
            "outcome": "designed", "design": "A gadget store.", "tickets": [
                {"id": "store", "title": "Store gadgets", "body": "Gadgets persist.", "blocked_by": []},
                {"id": "list", "title": "List gadgets", "body": "List them.", "blocked_by": ["store"]}
            ],
            "comment": "Two slices."
        }).to_string())
    });
    harness.on(WorkKind::Implement, |call| {
        let file = format!("src/{}.txt", subject_file(call));
        commit_in(&call.spec.dir, &[(&file, "gadget\n")], "Gadgets");
        Ok(
            json!({"outcome": "done", "pr_title": "Add gadgets", "pr_body": "Adds gadgets."})
                .to_string(),
        )
    });
    let out = dir.path().join("out");
    let mut config = config(&out);
    config.auto_merge = AUTO_MERGE;

    replay(
        &dir.path().join("case"),
        &out,
        harness,
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
    config.auto_merge = Some(AutoMerge {
        merge_method: Some(MergeMethod::Merge),
    });

    let error = replay(
        &dir.path().join("case"),
        &out,
        Arc::new(ScriptedHarness::default()),
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
