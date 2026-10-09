//! The Claude Code runtime against a stand-in `claude` program: what a
//! session's environment holds.

mod common;

use common::*;
use linewright::domain::Number;
use linewright::reconcile::{Subject, WorkKind};
use linewright::runtime::claude::ClaudeCode;
use linewright::runtime::{Runtime, SessionSpec};

#[tokio::test]
async fn a_production_session_gets_the_machine_users_github_token() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_claude(dir.path(), "Done.");
    let runtime = ClaudeCode {
        program: program.to_str().unwrap().into(),
        model: None,
        github_token: Some("machine-token".into()),
    };

    runtime
        .start(&SessionSpec {
            subject: Subject::Issue(Number(1)),
            kind: WorkKind::Triage,
            dir: dir.path().to_path_buf(),
            record: dir.path().to_path_buf(),
            prompt: "Triage #1.".into(),
            deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(30),
        })
        .await
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(dir.path().join("tokens.log")).unwrap(),
        "GH_TOKEN=machine-token GITHUB_TOKEN=machine-token\n"
    );
}
