//! The Claude Code runtime against a stand-in `claude` program: what a
//! session's environment holds.

mod common;

use std::path::Path;

use common::*;
use linewright::domain::Number;
use linewright::reconcile::{Subject, WorkKind};
use linewright::runtime::claude::ClaudeCode;
use linewright::runtime::{Purpose, Runtime, SessionSpec};

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
    let program = fake_claude(dir.path(), "Done.");
    let runtime = ClaudeCode {
        program: program.to_str().unwrap().into(),
        model: None,
        github_token: None,
    };
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
