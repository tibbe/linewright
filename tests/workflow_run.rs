//! Running a workflow by hand, `linewright workflow run NAME`, end to end at
//! the operator's level: a workflow file on the default branch in, one agent
//! session and its record out, against the local store and a scripted
//! runtime (`docs/specs/product.md`, "Workflows").

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use linewright::runtime::SessionError;
use serde_json::json;

const WEEKLY: &str = "---
on:
  schedule: \"0 6 * * 1\"
timeout: 60m
max-issues: 3
---
Look at the bugs fixed in the past week ...
";

async fn with_workflow(name: &str, text: &str) -> World {
    let world = World::new().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(&format!(".linewright/workflows/{name}.md"), text)],
            "Add a workflow",
        )
        .await
        .unwrap();
    world
}

fn finished() -> Result<String, SessionError> {
    json(json!({"summary": "Nothing needs doing.", "issues": []}))
}

#[tokio::test]
async fn a_workflow_runs_its_instructions_in_one_session_on_a_fresh_clone_of_the_default_branch() {
    let world = with_workflow("weekly", WEEKLY).await;
    world.runtime.on_workflow(|call| {
        assert!(
            call.spec
                .dir
                .join(".linewright/workflows/weekly.md")
                .exists()
        );
        finished()
    });

    world
        .factory
        .run_workflow(&"weekly".parse().unwrap())
        .await
        .unwrap();

    let prompts = world.runtime.workflow_prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains("Look at the bugs fixed in the past week ..."),
        "{}",
        prompts[0]
    );
}

#[tokio::test]
async fn a_workflow_run_hands_no_work_to_the_pipeline_yet() {
    let world = with_workflow("weekly", WEEKLY).await;
    world.runtime.on_workflow(|_| {
        json(json!({
            "summary": "One bug looks unfixed.",
            "issues": [{"title": "Fix it", "body": "It is broken.", "label": "ready-for-agent"}]
        }))
    });

    world
        .factory
        .run_workflow(&"weekly".parse().unwrap())
        .await
        .unwrap();

    assert!(world.store.issues().is_empty());
}

#[tokio::test]
async fn a_workflow_runs_kept_its_brief_result_and_trace_under_the_work_directory() {
    let world = with_workflow("weekly", WEEKLY).await;
    world.runtime.on_workflow(|_| finished());

    world
        .factory
        .run_workflow(&"weekly".parse().unwrap())
        .await
        .unwrap();

    let sessions = world.dir.path().join("work/sessions/workflow-weekly");
    let records: Vec<_> = std::fs::read_dir(&sessions).unwrap().collect();
    assert_eq!(records.len(), 1);
    let record = records[0].as_ref().unwrap().path();
    for file in ["brief.md", "result.json", "trace.jsonl"] {
        assert!(record.join(file).exists(), "{file} in {}", record.display());
    }
}

/// How long the workflow's session had left when it started.
async fn time_given(text: &str) -> Duration {
    let world = with_workflow("weekly", text).await;
    let left = Arc::new(Mutex::new(None));
    world.runtime.on_workflow({
        let left = left.clone();
        move |call| {
            *left.lock().unwrap() = Some(call.spec.deadline - tokio::time::Instant::now());
            finished()
        }
    });
    world
        .factory
        .run_workflow(&"weekly".parse().unwrap())
        .await
        .unwrap();
    left.lock().unwrap().expect("the session ran")
}

#[tokio::test]
async fn the_session_times_out_after_the_workflows_timeout() {
    let left = time_given(WEEKLY).await;

    assert!(
        left > Duration::from_secs(59 * 60) && left <= Duration::from_secs(60 * 60),
        "{left:?}"
    );
}

#[tokio::test]
async fn without_a_timeout_the_session_times_out_after_the_session_timeout() {
    let left = time_given("---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n").await;

    // The `session-timeout` the test factory runs with.
    assert!(
        left > Duration::from_secs(29 * 60) && left <= Duration::from_secs(30 * 60),
        "{left:?}"
    );
}

#[tokio::test]
async fn an_unknown_workflow_fails_naming_its_file() {
    let world = with_workflow("weekly", WEEKLY).await;

    let error = format!(
        "{:#}",
        world
            .factory
            .run_workflow(&"daily".parse().unwrap())
            .await
            .unwrap_err()
    );

    assert!(error.contains(".linewright/workflows/daily.md"), "{error}");
    assert!(world.runtime.workflow_prompts().is_empty());
}

#[tokio::test]
async fn a_workflow_file_that_does_not_parse_fails_saying_why() {
    let world = with_workflow(
        "weekly",
        "---\non:\n  schedule: \"every monday\"\n---\nLook.\n",
    )
    .await;

    let error = format!(
        "{:#}",
        world
            .factory
            .run_workflow(&"weekly".parse().unwrap())
            .await
            .unwrap_err()
    );

    assert!(error.contains("every monday"), "{error}");
    assert!(world.runtime.workflow_prompts().is_empty());
}

#[tokio::test]
async fn a_session_that_fails_fails_the_run() {
    let world = with_workflow("weekly", WEEKLY).await;
    world.runtime.on_workflow(|_| {
        Err(SessionError::Failed {
            reason: "it timed out".into(),
            log_tail: String::new(),
        })
    });

    let error = format!(
        "{:#}",
        world
            .factory
            .run_workflow(&"weekly".parse().unwrap())
            .await
            .unwrap_err()
    );

    assert!(error.contains("it timed out"), "{error}");
}

#[tokio::test]
async fn a_result_that_still_does_not_parse_after_the_retries_fails_the_run() {
    let world = with_workflow("weekly", WEEKLY).await;
    world
        .runtime
        .on_workflow(|_| Ok("I looked; all is well.".into()));

    let error = world.factory.run_workflow(&"weekly".parse().unwrap()).await;

    assert!(error.is_err());
    // The first answer and one follow-up, the test factory's `result-retries`.
    assert_eq!(world.runtime.workflow_prompts().len(), 2);
}

#[test]
fn the_help_lists_workflow_run_name() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_linewright"))
        .arg("--help")
        .output()
        .unwrap();

    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("`workflow run NAME`"), "{help}");
}
