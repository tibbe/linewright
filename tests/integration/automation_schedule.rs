use crate::common;

use chrono::TimeDelta;
use common::*;
use linewright::automation::LastScheduledRuns;
use linewright::database::Database;
use linewright::factory::Factory;

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
        config(&world.dir.path().join("work")).await,
    )
    .await
    .unwrap();
}

#[test_log::test(tokio::test)]
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

#[test_log::test(tokio::test)]
async fn ticks_missed_while_linewright_was_stopped_produce_one_run_after_it_starts() {
    let mut world = World::new().await;
    add_automation(&world, "hourly", HOURLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world.poll().await;

    let db = Database::open(&world.dir.path().join("work"))
        .await
        .unwrap();
    let recorded = LastScheduledRuns::load(&db).await.unwrap();
    assert!(recorded.get(&"hourly".parse().unwrap()).is_some());
    world.clock.advance(TimeDelta::hours(5));
    restart(&mut world).await;
    world.poll().await;
    world.poll().await;

    assert_eq!(world.harness.automation_prompts().len(), 1);
}

#[test_log::test(tokio::test)]
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

#[test_log::test(tokio::test)]
async fn after_an_upgrade_from_files_an_automation_keeps_the_last_scheduled_run_an_earlier_release_recorded()
 {
    let mut world = World::new().await;
    add_automation(&world, "hourly", HOURLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    let upgraded = tempfile::tempdir().unwrap();
    write_files(
        upgraded.path(),
        &[(
            "automation-runs.json",
            r#"{"hourly": "2026-10-01T12:00:00Z"}"#,
        )],
    );
    world.clock.advance(TimeDelta::minutes(30));

    world
        .restart(|config| config.work_dir = upgraded.path().to_path_buf())
        .await;
    world.poll().await;

    assert_eq!(
        world.harness.automation_prompts().len(),
        0,
        "not run at once"
    );
    let db = Database::open(upgraded.path()).await.unwrap();
    let recorded = LastScheduledRuns::load(&db).await.unwrap();
    assert_eq!(
        recorded.get(&"hourly".parse().unwrap()),
        Some("2026-10-01T12:00:00Z".parse().unwrap()),
        "not counted as new at 12:30"
    );
    world.clock.advance(TimeDelta::minutes(30));
    world.poll().await;
    assert_eq!(world.harness.automation_prompts().len(), 1);
}
