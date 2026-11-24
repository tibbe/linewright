use chrono::TimeDelta;
use linewright_core::automation::LastScheduledRuns;
use linewright_core::database::Database;
use linewright_core::factory::Factory;
use linewright_core::testing::*;

async fn add_automation(world: &World, name: &str, text: &str) {
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
    add_automation(&world, "hourly.md", HOURLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world.poll().await;

    world.clock.advance(TimeDelta::hours(1));
    world.poll().await;

    let prompts = world.harness.automation_prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("Look around."), "{}", prompts[0]);
}

#[test_log::test(tokio::test)]
async fn an_automation_keeps_its_schedule_across_a_restart() {
    let mut world = World::new().await;
    add_automation(&world, "hourly.md", HOURLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world.poll().await;

    let db = Database::open(&world.dir.path().join("work"))
        .await
        .unwrap();
    let recorded = LastScheduledRuns::load(&db).await.unwrap();
    assert!(recorded.get(&"hourly.md".parse().unwrap()).is_some());
    world.clock.advance(TimeDelta::hours(1));
    restart(&mut world).await;
    world.poll().await;

    assert_eq!(world.harness.automation_prompts().len(), 1);
}

#[test_log::test(tokio::test)]
async fn running_an_automation_by_hand_leaves_its_schedule_as_it_was() {
    let mut world = World::new().await;
    add_automation(&world, "hourly.md", HOURLY).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world.poll().await;
    world.clock.advance(TimeDelta::hours(1));

    world.run_by_hand("hourly.md").await.unwrap();
    world.poll().await;

    assert_eq!(world.harness.automation_prompts().len(), 2);
}
