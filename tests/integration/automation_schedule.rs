use crate::common;

use chrono::TimeDelta;
use common::*;
use linewright::factory::Factory;
use linewright::reconcile::WorkKind;
use serde_json::json;

async fn add_automation(world: &World, name: &str, text: &str) {
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
}

async fn restart(world: &mut World) {
    world.factory = Factory::start(
        world.store.clone(),
        world.harness.clone(),
        world.clock.clone(),
        config(&world.dir.path().join("work")),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn an_automation_runs_on_the_first_poll_after_its_schedule_ticks() {
    let mut world = World::new().await;
    add_automation(&world, "hourly", HOURLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world.poll().await;

    world.clock.advance(TimeDelta::hours(1));
    world.poll().await;

    let prompts = world.harness.automation_prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("Look around."), "{}", prompts[0]);
}

#[tokio::test]
async fn an_automation_seen_for_the_first_time_does_not_run() {
    let mut world = World::new().await;
    add_automation(&world, "hourly", HOURLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));

    world.poll().await;

    assert_eq!(world.harness.automation_prompts(), Vec::<String>::new());
}

#[tokio::test]
async fn ticks_missed_while_linewright_was_stopped_produce_one_run_after_it_starts() {
    let mut world = World::new().await;
    add_automation(&world, "hourly", HOURLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world.poll().await;

    let record = world.dir.path().join("work/automation-runs.json");
    assert!(std::fs::read_to_string(record).unwrap().contains("hourly"));
    world.clock.advance(TimeDelta::hours(5));
    restart(&mut world).await;
    world.poll().await;
    world.poll().await;

    assert_eq!(world.harness.automation_prompts().len(), 1);
}

#[tokio::test]
async fn an_automation_file_that_does_not_parse_is_skipped_and_the_others_still_run() {
    let mut world = World::new().await;
    add_automation(
        &world,
        "broken",
        "---\non:\n  schedule: \"every hour\"\n---\nLook.\n",
    )
    .await;
    add_automation(&world, "hourly", HOURLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world.poll().await;

    world.clock.advance(TimeDelta::hours(1));
    world.poll().await;

    let prompts = world.harness.automation_prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("Look around."), "{}", prompts[0]);
}

#[tokio::test]
async fn an_automation_run_waits_for_room_under_the_ceiling_beside_the_pipelines_runs() {
    let mut world = World::with(|config| config.max_runs = 1).await;
    add_automation(&world, "hourly", HOURLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world.poll().await;
    world.clock.advance(TimeDelta::hours(1));
    world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.harness.on(WorkKind::Implement, |call| {
        commit_in(
            &call.spec.dir,
            &[("src/thing.txt", "done\n")],
            "Do the thing",
        );
        Ok(json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."}).to_string())
    });

    world.poll().await;
    let while_implementing = world.harness.automation_prompts().len();
    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
    assert_eq!(while_implementing, 0);
    assert_eq!(world.harness.automation_prompts().len(), 1);
}

#[tokio::test]
async fn running_an_automation_by_hand_leaves_its_schedule_as_it_was() {
    let mut world = World::new().await;
    add_automation(&world, "hourly", HOURLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world.poll().await;
    world.clock.advance(TimeDelta::hours(1));

    world
        .factory
        .run_automation(&"hourly".parse().unwrap())
        .await
        .unwrap();
    world.poll().await;

    assert_eq!(world.harness.automation_prompts().len(), 2);
}

#[tokio::test]
async fn the_last_scheduled_runs_the_previous_version_recorded_are_kept() {
    let mut world = World::new().await;
    add_automation(&world, "hourly", HOURLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    // Where the previous version recorded the last scheduled runs.
    let work_dir = world.dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    std::fs::write(
        work_dir.join("workflow-runs.json"),
        r#"{"hourly": "2026-10-01T11:00:00Z"}"#,
    )
    .unwrap();
    restart(&mut world).await;

    world.poll().await;

    assert_eq!(world.harness.automation_prompts().len(), 1);
}
