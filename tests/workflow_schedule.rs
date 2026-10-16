mod common;

use chrono::TimeDelta;
use common::*;
use linewright::factory::Factory;
use linewright::reconcile::WorkKind;
use linewright::runtime::SessionError;
use serde_json::json;

/// Ticks on the hour; the fake clock starts at 12:00.
const HOURLY: &str = "---\non:\n  schedule: \"0 * * * *\"\n---\nLook around.\n";

async fn add_workflow(world: &World, name: &str, text: &str) {
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
}

fn finished() -> Result<String, SessionError> {
    json(json!({"summary": "Nothing needs doing.", "issues": []}))
}

async fn restart(world: &mut World) {
    world.factory = Factory::start(
        world.store.clone(),
        world.runtime.clone(),
        world.clock.clone(),
        config(&world.dir.path().join("work")),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_workflow_runs_on_the_first_poll_after_its_schedule_ticks() {
    let mut world = World::new().await;
    add_workflow(&world, "hourly", HOURLY).await;
    world.runtime.on_workflow(|_| finished());
    world.poll().await;

    world.clock.advance(TimeDelta::hours(1));
    world.poll().await;

    let prompts = world.runtime.workflow_prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("Look around."), "{}", prompts[0]);
}

#[tokio::test]
async fn a_workflow_seen_for_the_first_time_does_not_run() {
    let mut world = World::new().await;
    add_workflow(&world, "hourly", HOURLY).await;
    world.runtime.on_workflow(|_| finished());

    world.poll().await;

    assert!(world.runtime.workflow_prompts().is_empty());
}

#[tokio::test]
async fn ticks_missed_while_linewright_was_stopped_produce_one_run_after_it_starts() {
    let mut world = World::new().await;
    add_workflow(&world, "hourly", HOURLY).await;
    world.runtime.on_workflow(|_| finished());
    world.poll().await;

    let record = world.dir.path().join("work/workflow-runs.json");
    assert!(std::fs::read_to_string(record).unwrap().contains("hourly"));
    world.clock.advance(TimeDelta::hours(5));
    restart(&mut world).await;
    world.poll().await;
    world.poll().await;

    assert_eq!(world.runtime.workflow_prompts().len(), 1);
}

#[tokio::test]
async fn a_workflow_file_that_does_not_parse_is_skipped_and_the_others_still_run() {
    let mut world = World::new().await;
    add_workflow(
        &world,
        "broken",
        "---\non:\n  schedule: \"every hour\"\n---\nLook.\n",
    )
    .await;
    add_workflow(&world, "hourly", HOURLY).await;
    world.runtime.on_workflow(|_| finished());
    world.poll().await;

    world.clock.advance(TimeDelta::hours(1));
    world.poll().await;

    let prompts = world.runtime.workflow_prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("Look around."), "{}", prompts[0]);
}

#[tokio::test]
async fn a_workflow_run_waits_for_room_under_the_ceiling_beside_the_pipelines_runs() {
    let mut world = World::with(|config| config.max_runs = 1).await;
    add_workflow(&world, "hourly", HOURLY).await;
    world.runtime.on_workflow(|_| finished());
    world.poll().await;
    world.clock.advance(TimeDelta::hours(1));
    world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    world.runtime.on(WorkKind::Implement, |call| {
        commit_in(
            &call.spec.dir,
            &[("src/thing.txt", "done\n")],
            "Do the thing",
        );
        json(json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."}))
    });

    world.poll().await;
    let while_implementing = world.runtime.workflow_prompts().len();
    world.poll().await;

    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 1);
    assert_eq!(while_implementing, 0);
    assert_eq!(world.runtime.workflow_prompts().len(), 1);
}

#[tokio::test]
async fn running_a_workflow_by_hand_leaves_its_schedule_as_it_was() {
    let mut world = World::new().await;
    add_workflow(&world, "hourly", HOURLY).await;
    world.runtime.on_workflow(|_| finished());
    world.poll().await;
    world.clock.advance(TimeDelta::hours(1));

    world
        .factory
        .run_workflow(&"hourly".parse().unwrap())
        .await
        .unwrap();
    world.poll().await;

    assert_eq!(world.runtime.workflow_prompts().len(), 2);
}
