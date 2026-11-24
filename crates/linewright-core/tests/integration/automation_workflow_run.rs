use std::num::NonZeroU32;

use chrono::TimeDelta;
use linewright_core::clock::Clock;
use linewright_core::domain::Timestamp;
use linewright_core::testing::*;
use linewright_core::webhook::Subject;
use linewright_core::workflow_run::{
    AttemptConclusion, AttemptId, CompletedAttempt, Conclusion, WorkflowRunId,
};

const FIX_CI: &str = "---
on:
  workflow_run:
    workflows: [CI]
    branches: [main]
    conclusions: [failure]
---
Fix what failed.
";

/// A world with the automation `fix-ci.md`, which a poll has seen.
async fn with_fix_ci() -> World {
    let mut world = World::new().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(".linewright/automations/fix-ci.md", FIX_CI)],
            "Add an automation",
        )
        .await
        .unwrap_or_else(|e| panic!("adding the automation: {e:#}"));
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world.poll().await;
    world
}

/// The first attempt of the run `run` of CI on main, which GitHub created
/// at `created`, and which failed at `completed`.
fn failed_on_main(run: u64, created: Timestamp, completed: Timestamp) -> CompletedAttempt {
    CompletedAttempt {
        id: AttemptId {
            run: WorkflowRunId(run),
            attempt: NonZeroU32::MIN,
        },
        workflow: "CI".into(),
        run_created_at: created,
        completed_at: completed,
        conclusion: AttemptConclusion::Known(Conclusion::Failure),
        branch: Some("main".into()),
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
        link: format!("https://github.com/owner/repo/actions/runs/{run}/attempts/1"),
    }
}

/// The run `run`'s first attempt, created 10 minutes ago, which failed on
/// main now.
fn fails_now(world: &World, run: u64) -> CompletedAttempt {
    let now = world.clock.now();
    failed_on_main(run, now - TimeDelta::minutes(10), now)
}

#[test_log::test(tokio::test)]
async fn a_failing_attempt_on_main_runs_the_automation_once_whatever_the_polls_and_deliveries() {
    let mut world = with_fix_ci().await;
    world.clock.advance(TimeDelta::minutes(5));
    world.store.complete_attempt(fails_now(&world, 7));

    world
        .factory
        .refresh([Subject::WorkflowRuns])
        .await
        .unwrap();
    world.factory.settle().await;
    world.poll().await;
    world
        .factory
        .refresh([Subject::WorkflowRuns])
        .await
        .unwrap();
    world.factory.settle().await;
    world.clock.advance(TimeDelta::minutes(5));
    world.poll().await;

    let prompts = world.harness.automation_prompts();
    assert_eq!(prompts.len(), 1, "{prompts:#?}");
    assert!(
        prompts[0].contains(
            "trigger=«[parts/trigger-workflow-run.md workflow=«CI» attempt=«1» \
             conclusion=«failure» branch=«`main`» \
             commit=«0123456789abcdef0123456789abcdef01234567» \
             link=«https://github.com/owner/repo/actions/runs/7/attempts/1»]»"
        ),
        "{}",
        prompts[0]
    );
}

#[test_log::test(tokio::test)]
async fn an_attempt_that_completed_while_linewright_was_down_runs_the_automation_after_it_starts() {
    let mut world = with_fix_ci().await;
    world.clock.advance(TimeDelta::hours(1));
    world.store.complete_attempt(fails_now(&world, 7));
    world.clock.advance(TimeDelta::hours(1));

    world.restart(|_| {}).await;
    world.poll().await;
    world.poll().await;

    assert_eq!(world.harness.automation_prompts().len(), 1);
}

#[test_log::test(tokio::test)]
async fn an_attempt_read_after_ci_timeout_does_not_run_the_automation() {
    let mut world = with_fix_ci().await;
    world.clock.advance(TimeDelta::minutes(5));
    let created_at_12_00 = at("2026-10-01T12:00:00Z");
    world
        .store
        .complete_attempt(failed_on_main(7, created_at_12_00, world.clock.now()));
    world.clock.advance(TimeDelta::hours(6));

    world.restart(|_| {}).await;
    world.poll().await;

    assert_eq!(world.harness.automation_prompts().len(), 0);
}

#[test_log::test(tokio::test)]
async fn running_the_automation_by_hand_leaves_the_attempts_that_have_run_it_as_they_were() {
    let mut world = with_fix_ci().await;
    world.clock.advance(TimeDelta::minutes(5));
    world.store.complete_attempt(fails_now(&world, 7));

    world
        .factory
        .run_automation(&"fix-ci.md".parse().unwrap())
        .await
        .unwrap();
    world.poll().await;
    world.poll().await;

    let prompts = world.harness.automation_prompts();
    assert_eq!(prompts.len(), 2, "{prompts:#?}");
    assert!(prompts[0].contains("trigger=«»"), "by hand: {}", prompts[0]);
    assert!(
        prompts[1].contains("/actions/runs/7/attempts/1"),
        "{}",
        prompts[1]
    );
}
