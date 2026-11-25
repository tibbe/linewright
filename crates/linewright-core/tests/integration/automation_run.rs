use std::sync::{Arc, Mutex};

use clap::{CommandFactory, Parser};
use linewright_core::by_hand::{HandRefusal, HandTaken};
use linewright_core::config::Cli;
use linewright_core::harness::SessionError;
use linewright_core::reconcile::HandWait;
use linewright_core::testing::*;
use linewright_core::work_settings::Effort;
use serde_json::json;

const WEEKLY: &str = "---
on:
  schedule: \"0 6 * * 1\"
session-timeout: 60m
max-issues: 3
---
Look at the bugs fixed in the past week ...
";

/// A world with the automation `name` on the default branch, polled once.
async fn with_automation(name: &str, text: &str) -> World {
    let mut world = World::new().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(&format!(".linewright/automations/{name}"), text)],
            "Add an automation",
        )
        .await
        .unwrap();
    world.poll().await;
    world
}

#[test_log::test(tokio::test)]
async fn an_automation_run_files_the_issues_its_result_returns_as_linewright() {
    let mut world = with_automation(
        "team/weekly.md",
        "---\non:\n  schedule: \"0 6 * * 1\"\nmodel: opus\neffort: low\n---\nLook.\n",
    )
    .await;
    let given = Arc::new(Mutex::new(None));
    world.harness.on_automation({
        let given = given.clone();
        move |call| {
            *given.lock().unwrap() = Some((
                call.spec
                    .dir
                    .join(".linewright/automations/team/weekly.md")
                    .exists(),
                call.spec.model.clone(),
                call.spec.effort,
            ));
            Ok(json!({
            "summary": "One bug looks unfixed, one needs a decision.",
            "issues": [
                {"title": "Fix the widget count", "body": "Count them all.\n\n- [ ] All counted", "label": "ready-for-agent"},
                {"title": "Should gadgets expire?", "body": "Recommend: yes.", "label": "ready-for-human"}
            ]
            }).to_string())
        }
    });

    world.run_by_hand("team/weekly.md").await.unwrap();

    assert_eq!(
        given.lock().unwrap().clone(),
        Some((true, model("opus"), Effort::Low))
    );
    let sessions = world
        .dir
        .path()
        .join("work/sessions/automation-team%2Fweekly.md");
    assert_eq!(std::fs::read_dir(sessions).unwrap().count(), 1);
    let issues = world.store.issues();
    let filed: Vec<(&str, Vec<&str>, &str)> = issues
        .iter()
        .map(|i| (i.title.as_str(), labels(i), i.author.as_str()))
        .collect();
    assert_eq!(
        filed,
        [
            ("Fix the widget count", vec!["ready-for-agent"], LINEWRIGHT),
            (
                "Should gadgets expire?",
                vec!["ready-for-human"],
                LINEWRIGHT
            ),
        ]
    );
    assert!(
        issues[0]
            .body
            .starts_with("Count them all.\n\n- [ ] All counted"),
        "{}",
        issues[0].body
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

#[test_log::test(tokio::test)]
async fn the_issues_beyond_max_issues_are_not_filed_and_are_logged_as_dropped() {
    let mut world = with_automation("weekly.md", CAPPED).await;
    world.harness.on_automation(|_| Ok(three_issues()));
    let (log, _guard) = Log::capture();

    world.run_by_hand("weekly.md").await.unwrap();

    assert_eq!(world.store.issues().len(), 2);
    let dropped: Vec<String> = log
        .lines("WARN")
        .into_iter()
        .filter(|line| line.contains("dropped"))
        .collect();
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert!(dropped[0].contains("Third"), "{}", dropped[0]);
}

/// The errors logged as runs of automations failed.
fn failures(log: &Log) -> Vec<String> {
    log.lines("ERROR")
        .into_iter()
        .filter(|line| line.contains("automation run failed"))
        .collect()
}

#[test_log::test(tokio::test)]
async fn a_session_that_times_out_fails_the_run_without_a_retry_saying_it_timed_out() {
    let mut world = with_automation("weekly.md", WEEKLY).await;
    world.harness.on_automation(|_| {
        Err(SessionError::TimedOut {
            log_tail: String::new(),
        })
    });
    let (log, _guard) = Log::capture();

    world.run_by_hand("weekly.md").await.unwrap();

    let failed = failures(&log);
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("it timed out"), "{}", failed[0]);
    assert!(
        failed[0].contains(
            "or `session-timeout` in the automation's front matter may need to be longer"
        ),
        "{}",
        failed[0]
    );
    assert_eq!(world.harness.automation_prompts().len(), 1);
}

#[test_log::test(tokio::test)]
async fn a_session_that_fails_fails_the_run() {
    let mut world = with_automation("weekly.md", WEEKLY).await;
    world.harness.on_automation(|_| {
        Err(SessionError::NoResult {
            why: "it crashed".into(),
            log_tail: String::new(),
        })
    });
    let (log, _guard) = Log::capture();

    world.run_by_hand("weekly.md").await.unwrap();

    let failed = failures(&log);
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(
        failed[0].contains("The session ended unexpectedly: the harness stopped without a result."),
        "{}",
        failed[0]
    );
}

#[test_log::test(tokio::test)]
async fn a_result_that_still_does_not_parse_after_the_retries_fails_the_run() {
    let mut world = with_automation("weekly.md", WEEKLY).await;
    world
        .harness
        .on_automation(|_| Ok("I looked; all is well.".into()));

    let (log, _guard) = Log::capture();

    world.run_by_hand("weekly.md").await.unwrap();

    assert_eq!(failures(&log).len(), 1);
    let the_first_answer_and_one_follow_up = 2;
    assert_eq!(
        world.harness.automation_prompts().len(),
        the_first_answer_and_one_follow_up
    );
}

#[test_log::test]
fn the_help_says_how_to_run_one_now() {
    let help = Cli::command().render_long_help().to_string();

    assert!(help.contains("`automation run NAME`"), "{help}");
}

#[test_log::test]
fn the_command_that_runs_one_now_is_named_after_automations() {
    assert!(Cli::try_parse_from(["lw", "automation", "run", "weekly.md"]).is_ok());
    assert!(Cli::try_parse_from(["lw", "workflow", "run", "weekly.md"]).is_err());
}

/// `docs/specs/runs.md`, Usage limit: an automation's run waits it out as
/// every run does.
#[test_log::test(tokio::test)]
async fn a_run_refused_for_the_usage_limit_waits_it_out_and_ends_done() {
    let mut world = with_automation("weekly.md", WEEKLY).await;
    let refused = std::sync::atomic::AtomicBool::new(false);
    world.harness.on_automation(move |call| {
        if refused.swap(true, std::sync::atomic::Ordering::Relaxed) {
            Ok(nothing_needs_doing())
        } else {
            refused_for_the_usage_limit(call)
        }
    });

    world
        .factory
        .ask_by_hand("weekly.md".parse().unwrap())
        .await
        .unwrap();
    assert_eq!(world.clock.next_sleep().await, usage_limit_resets_at());
    let now = linewright_core::clock::Clock::now(world.clock.as_ref());
    world.clock.advance(usage_limit_resets_at() - now);
    world.factory.settle().await;

    assert_eq!(
        ended_runs(&world).await,
        [linewright_core::runs::Ending::Done]
    );
}

#[test_log::test(tokio::test)]
async fn a_run_by_hand_is_answered_with_its_id() {
    let mut world = with_automation("team/weekly.md", WEEKLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));

    let taken = world.run_by_hand("team/weekly.md").await.unwrap();

    assert_eq!(
        taken,
        HandTaken::Started {
            run: "team/weekly.md/1".parse().unwrap()
        }
    );
    assert_eq!(taken.render(), "team/weekly.md/1\n");
}

#[test_log::test(tokio::test)]
async fn requests_while_a_run_is_in_flight_wait_for_it_and_then_start_one_run() {
    let mut world = with_automation("weekly.md", WEEKLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    let agent = world.harness.hold();
    let weekly = || "weekly.md".parse().unwrap();

    let first = world.factory.ask_by_hand(weekly()).await.unwrap();
    let second = world.factory.ask_by_hand(weekly()).await.unwrap();
    let third = world.factory.ask_by_hand(weekly()).await.unwrap();
    drop(agent);
    world.factory.settle().await;
    world.poll().await;
    world.poll().await;

    let waiting = HandTaken::Waiting {
        waits_for: HandWait::RunInFlight("weekly.md/1".parse().unwrap()),
    };
    assert_eq!(
        first,
        HandTaken::Started {
            run: "weekly.md/1".parse().unwrap()
        }
    );
    assert_eq!([second.clone(), third], [waiting.clone(), waiting]);
    assert_eq!(
        second.render(),
        "waiting for a run of weekly.md in flight: weekly.md/1\n"
    );
    assert_eq!(world.harness.automation_prompts().len(), 2);
}

#[test_log::test(tokio::test)]
async fn a_restart_forgets_a_request_whose_run_has_not_started() {
    let mut world = with_automation("weekly.md", WEEKLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    let agent = world.harness.hold();
    world
        .factory
        .ask_by_hand("weekly.md".parse().unwrap())
        .await
        .unwrap();
    let waiting = world
        .factory
        .ask_by_hand("weekly.md".parse().unwrap())
        .await
        .unwrap();

    world.restart(|_| {}).await;
    drop(agent);
    world.poll().await;
    world.poll().await;

    assert_eq!(
        waiting,
        HandTaken::Waiting {
            waits_for: HandWait::RunInFlight("weekly.md/1".parse().unwrap())
        }
    );
    let runs: Vec<String> = runs_in(&world.dir.path().join("work"))
        .await
        .read()
        .await
        .unwrap()
        .into_iter()
        .map(|run| run.id.to_string())
        .collect();
    assert_eq!(runs, ["weekly.md/1"]);
}

#[test_log::test(tokio::test)]
async fn a_request_after_a_poll_that_could_not_list_the_automations_is_refused_saying_why() {
    let mut world = with_automation("weekly.md", WEEKLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world
        .store
        .fail_listing(".linewright/automations", "GitHub answered 502");
    world.poll().await;

    let refused = world.run_by_hand("weekly.md").await.unwrap_err();

    assert_eq!(
        refused,
        HandRefusal::Unlisted {
            why: "GitHub answered 502".into()
        }
    );
    world.store.restore_listing(".linewright/automations");
    world.poll().await;
    assert_eq!(
        world.run_by_hand("weekly.md").await,
        Ok(HandTaken::Started {
            run: "weekly.md/1".parse().unwrap()
        })
    );
}

#[test_log::test(tokio::test)]
async fn a_request_after_a_poll_that_failed_before_it_listed_the_automations_is_refused() {
    let mut world = with_automation("weekly.md", WEEKLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world.store.fail_reads("bad credentials");
    world.factory.cycle().await.unwrap_err();
    world.store.restore_reads();

    let refused = world.run_by_hand("weekly.md").await.unwrap_err();

    assert_eq!(
        refused,
        HandRefusal::Unlisted {
            why: "taking a snapshot: bad credentials".into()
        }
    );
    world.poll().await;
    assert_eq!(
        world.run_by_hand("weekly.md").await,
        Ok(HandTaken::Started {
            run: "weekly.md/1".parse().unwrap()
        })
    );
}

#[test_log::test(tokio::test)]
async fn a_poll_that_fails_before_it_lists_the_automations_leaves_them_shown() {
    let World {
        store,
        factory,
        dir,
        ..
    } = with_automation("weekly.md", WEEKLY).await;
    let serving = Serving::start(
        factory,
        std::time::Duration::from_millis(100),
        linewright_core::webhook::Pending::default(),
    );
    eventually(|| serving.served.borrow().is_some()).await;
    store.fail_reads("GitHub is down");
    eventually(|| serving.failed_polls().is_some()).await;

    let view = serving.served.borrow().as_ref().unwrap().view();

    let shown: Vec<String> = view
        .automations
        .parsed
        .iter()
        .map(|automation| automation.name.to_string())
        .collect();
    assert_eq!(shown, ["weekly.md"]);
    serving.stop().await;
    drop(dir);
}
