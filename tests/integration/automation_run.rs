use crate::common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Parser;
use common::*;
use linewright::config::Cli;
use linewright::harness::SessionError;
use linewright::reconcile::WorkKind;
use linewright::work_settings::Effort;
use serde_json::json;

const WEEKLY: &str = "---
on:
  schedule: \"0 6 * * 1\"
timeout: 60m
max-issues: 3
---
Look at the bugs fixed in the past week ...
";

async fn with_automation(name: &str, text: &str) -> World {
    let world = World::new().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(&format!(".linewright/automations/{name}.md"), text)],
            "Add an automation",
        )
        .await
        .unwrap();
    world
}

#[tokio::test]
async fn an_automation_runs_its_instructions_in_one_session_on_a_fresh_clone_of_the_default_branch()
{
    let world = with_automation("weekly", WEEKLY).await;
    world.harness.on_automation(|call| {
        assert!(
            call.spec
                .dir
                .join(".linewright/automations/weekly.md")
                .exists()
        );
        Ok(nothing_needs_doing())
    });

    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();

    let prompts = world.harness.automation_prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains("Look at the bugs fixed in the past week ..."),
        "{}",
        prompts[0]
    );
    assert!(
        prompts[0].contains("### The automation `weekly`"),
        "{}",
        prompts[0]
    );
    assert!(
        prompts[0].contains("one of the maintainer's automations"),
        "{}",
        prompts[0]
    );
}

#[tokio::test]
async fn an_operators_automation_prompt_is_the_one_the_session_gets() {
    let operators = prompts_with("automation.md", |text| {
        format!("Mind the schedule.\n\n{text}")
    });
    let world = World::with(|config| {
        config.prompts = linewright::briefs::OperatorPrompts::read(Some(operators.path())).unwrap();
    })
    .await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(".linewright/automations/weekly.md", WEEKLY)],
            "Add an automation",
        )
        .await
        .unwrap();
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));

    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();

    let prompts = world.harness.automation_prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].starts_with("Mind the schedule.\n\n"),
        "{}",
        prompts[0]
    );
}

#[tokio::test]
async fn an_automation_run_files_the_issues_its_result_returns_as_linewright() {
    let world = with_automation("weekly", WEEKLY).await;
    world.harness.on_automation(|_| {
        Ok(json!({
            "summary": "One bug looks unfixed, one needs a decision.",
            "issues": [
                {"title": "Fix the widget count", "body": "Count them all.\n\n- [ ] All counted", "label": "ready-for-agent"},
                {"title": "Should gadgets expire?", "body": "Recommend: yes.", "label": "ready-for-human"}
            ]
        }).to_string())
    });

    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();

    let issues = world.store.issues();
    let filed: Vec<(&str, Vec<&str>, &str, &str)> = issues
        .iter()
        .map(|i| {
            (
                i.title.as_str(),
                labels(i),
                i.author.as_str(),
                i.body.as_str(),
            )
        })
        .collect();
    assert_eq!(
        filed,
        [
            (
                "Fix the widget count",
                vec!["ready-for-agent"],
                LINEWRIGHT,
                "Count them all.\n\n- [ ] All counted\n\nFiled by the automation `weekly`."
            ),
            (
                "Should gadgets expire?",
                vec!["ready-for-human"],
                LINEWRIGHT,
                "Recommend: yes.\n\nFiled by the automation `weekly`."
            ),
        ]
    );
}

const CAPPED: &str = "---
on:
  schedule: \"0 6 * * 1\"
max-issues: 2
---
Look.
";

/// A result returning three issues, one more than [`CAPPED`] files.
fn three_issues() -> String {
    let issue = |title: &str| json!({"title": title, "body": "Do it.", "label": "ready-for-agent"});
    json!({
        "summary": "Three things need doing.",
        "issues": [issue("First"), issue("Second"), issue("Third")]
    })
    .to_string()
}

#[tokio::test]
async fn a_run_files_only_the_first_max_issues_issues() {
    let world = with_automation("weekly", CAPPED).await;
    world.harness.on_automation(|_| Ok(three_issues()));

    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();

    let titles: Vec<String> = world.store.issues().into_iter().map(|i| i.title).collect();
    assert_eq!(titles, ["First", "Second"]);
}

#[tokio::test]
async fn the_issues_beyond_max_issues_are_logged_as_dropped() {
    let world = with_automation("weekly", CAPPED).await;
    world.harness.on_automation(|_| Ok(three_issues()));
    let (log, _guard) = Log::capture();

    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();

    let dropped: Vec<String> = log
        .lines("WARN")
        .into_iter()
        .filter(|line| line.contains("dropped"))
        .collect();
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert!(dropped[0].contains("Third"), "{}", dropped[0]);
}

#[tokio::test]
async fn a_result_with_a_label_other_than_the_two_goes_back_to_the_session_with_the_error() {
    let world = with_automation("weekly", WEEKLY).await;
    world.harness.on_automation(|call| {
        let label = match call.followup {
            None => "bug",
            Some(_) => "ready-for-agent",
        };
        Ok(json!({
            "summary": "One bug.",
            "issues": [{"title": "Fix it", "body": "It is broken.", "label": label}]
        })
        .to_string())
    });

    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();

    let prompts = world.harness.automation_prompts();
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[1]
            .contains("unknown variant `bug`, expected `ready-for-agent` or `ready-for-human`"),
        "{}",
        prompts[1]
    );
}

#[tokio::test]
async fn a_ready_for_agent_automation_issue_is_designed_then_implemented_without_triage() {
    let mut world = with_automation("weekly", WEEKLY).await;
    world.harness.on_automation(|_| {
        Ok(json!({
            "summary": "One bug.",
            "issues": [{"title": "Fix it", "body": "It is broken.", "label": "ready-for-agent"}]
        })
        .to_string())
    });
    world.harness.on(WorkKind::Implement, |call| {
        commit_in(&call.spec.dir, &[("src/fixed.txt", "fixed\n")], "Fix it");
        Ok(json!({"outcome": "done", "pr_title": "Fix it", "pr_body": "Fixes it."}).to_string())
    });
    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();
    let issue = world.store.issues()[0].number;
    design_issues(&world);

    world.poll().await;
    world.poll().await;

    let pr = *world
        .store
        .pulls()
        .last()
        .expect("a pull request was opened");
    assert_eq!(world.store.snapshot_pull(pr).await.closes, [issue]);
    assert_eq!(
        world.harness.prompts(WorkKind::Triage),
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn the_brief_tells_the_agent_to_search_the_existing_issues_and_not_return_open_ones() {
    let world = with_automation("weekly", WEEKLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));

    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();

    let prompt = &world.harness.automation_prompts()[0];
    assert!(
        prompt.contains(
            "Search the existing issues first, and don't return one that is already open."
        ),
        "{prompt}"
    );
}

#[tokio::test]
async fn an_automation_runs_kept_its_brief_result_and_transcript_under_the_work_directory() {
    let world = with_automation("weekly", WEEKLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));

    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();

    let sessions = world.dir.path().join("work/sessions/automation-weekly");
    let records: Vec<_> = std::fs::read_dir(&sessions).unwrap().collect();
    assert_eq!(records.len(), 1);
    let record = records[0].as_ref().unwrap().path();
    for file in ["brief.md", "result.json", "transcript.jsonl"] {
        assert!(record.join(file).exists(), "{file} in {}", record.display());
    }
}

async fn time_given(text: &str) -> Duration {
    let world = with_automation("weekly", text).await;
    let left = Arc::new(Mutex::new(None));
    world.harness.on_automation({
        let left = left.clone();
        move |call| {
            *left.lock().unwrap() = Some(call.spec.deadline - tokio::time::Instant::now());
            Ok(nothing_needs_doing())
        }
    });
    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();
    left.lock().unwrap().expect("the session ran")
}

#[tokio::test]
async fn an_automations_model_and_effort_reach_its_session() {
    let world = with_automation(
        "weekly",
        "---\non:\n  schedule: \"0 6 * * 1\"\nmodel: opus\neffort: low\n---\nLook.\n",
    )
    .await;
    let given = Arc::new(Mutex::new(None));
    world.harness.on_automation({
        let given = given.clone();
        move |call| {
            *given.lock().unwrap() = Some((call.spec.model.clone(), call.spec.effort));
            Ok(nothing_needs_doing())
        }
    });

    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();

    assert_eq!(
        given.lock().unwrap().clone(),
        Some((Some("opus".to_string()), Some(Effort::Low)))
    );
}

#[tokio::test]
async fn the_session_times_out_after_the_automations_timeout() {
    let left = time_given(WEEKLY).await;

    assert!(
        left > Duration::from_mins(59) && left <= Duration::from_hours(1),
        "{left:?}"
    );
}

#[tokio::test]
async fn without_a_timeout_the_session_times_out_after_the_session_timeout() {
    let left = time_given("---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n").await;

    // The `session-timeout` the test factory runs with.
    assert!(
        left > Duration::from_mins(29) && left <= Duration::from_mins(30),
        "{left:?}"
    );
}

#[tokio::test]
async fn an_unknown_automation_fails_naming_its_file() {
    let world = with_automation("weekly", WEEKLY).await;

    let error = format!(
        "{:#}",
        world
            .factory
            .run_automation(&"daily".parse().unwrap())
            .await
            .unwrap_err()
    );

    assert!(error.contains("there is no automation `daily`"), "{error}");
    assert!(
        error.contains(".linewright/automations/daily.md"),
        "{error}"
    );
    assert_eq!(world.harness.automation_prompts(), Vec::<String>::new());
}

#[tokio::test]
async fn an_automation_file_that_does_not_parse_fails_saying_why() {
    let world = with_automation(
        "weekly",
        "---\non:\n  schedule: \"every monday\"\n---\nLook.\n",
    )
    .await;

    let error = format!(
        "{:#}",
        world
            .factory
            .run_automation(&"weekly".parse().unwrap())
            .await
            .unwrap_err()
    );

    assert!(error.contains("every monday"), "{error}");
    assert_eq!(world.harness.automation_prompts(), Vec::<String>::new());
}

#[tokio::test]
async fn a_session_that_times_out_fails_the_run_without_a_retry_saying_it_timed_out() {
    let world = with_automation("weekly", WEEKLY).await;
    world.harness.on_automation(|_| {
        Err(SessionError::TimedOut {
            log_tail: String::new(),
        })
    });

    let error = format!(
        "{:#}",
        world
            .factory
            .run_automation(&"weekly".parse().unwrap())
            .await
            .unwrap_err()
    );

    assert!(error.contains("it timed out"), "{error}");
    assert!(
        error.contains("or the automation's `timeout` may need to be longer"),
        "{error}"
    );
    assert_eq!(world.harness.automation_prompts().len(), 1);
}

#[tokio::test]
async fn a_session_that_fails_fails_the_run() {
    let world = with_automation("weekly", WEEKLY).await;
    world.harness.on_automation(|_| {
        Err(SessionError::Failed {
            reason: "it crashed".into(),
            log_tail: String::new(),
        })
    });

    let error = format!(
        "{:#}",
        world
            .factory
            .run_automation(&"weekly".parse().unwrap())
            .await
            .unwrap_err()
    );

    assert!(error.contains("it crashed"), "{error}");
}

#[tokio::test]
async fn a_result_that_still_does_not_parse_after_the_retries_fails_the_run() {
    let world = with_automation("weekly", WEEKLY).await;
    world
        .harness
        .on_automation(|_| Ok("I looked; all is well.".into()));

    let error = world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await;

    assert!(error.is_err());
    // The first answer and one follow-up, the test factory's `result-retries`.
    assert_eq!(world.harness.automation_prompts().len(), 2);
}

#[test]
fn the_help_says_how_to_run_one_now() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .arg("--help")
        .output()
        .unwrap();

    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("`automation run NAME`"), "{help}");
}

#[test]
fn the_command_that_runs_one_now_is_named_after_automations() {
    assert!(Cli::try_parse_from(["lw", "automation", "run", "weekly"]).is_ok());
    // The command's old name, which the spec gives no alias.
    assert!(Cli::try_parse_from(["lw", "workflow", "run", "weekly"]).is_err());
}

#[tokio::test]
async fn a_run_refused_for_the_usage_limit_ends_saying_so_not_failing() {
    let world = with_automation("weekly", WEEKLY).await;
    world
        .harness
        .on_automation(|_| refused_for_the_usage_limit());
    let (log, _guard) = Log::capture();

    // A run that waited for the reset would never return: the fake clock
    // stands still.
    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();

    let ended = log.usage_limit_warnings();
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert!(log.lines("ERROR").is_empty(), "{:?}", log.lines("ERROR"));
    assert_eq!(world.harness.automation_prompts().len(), 1, "not retried");
    assert_eq!(world.store.issues(), []);
}
