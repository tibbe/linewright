//! Showing and rebuilding the environment: `environment`, `environment
//! rebuild`, `status` and the server page (`docs/specs/sessions.md`, `lw
//! environment`, and `docs/specs/sessions.md`, Building).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use linewright_core::clock::Clock as _;
use linewright_core::database::Database;
use linewright_core::environment::{
    BuildEnded, Builder, Environment, RebuildTaken, SetupRead, Tallied,
};
use linewright_core::redaction::Redaction;
use linewright_core::testing::*;
use linewright_core::work_settings::{SetupCommand, SetupSettings};

/// The environment of `world`'s repository, kept in the work directory
/// `dir`, as `serve` drives it.
async fn environment_of(world: &World, dir: &Path) -> Environment {
    let builder = Builder {
        store: world.store.clone(),
        harness: world.harness.clone(),
        bash: bash(),
        clock: world.clock.clone(),
        redaction: Redaction::default(),
        given: Arc::default(),
        default_branch: "main".into(),
        winding_up: linewright_core::winding_up::WindingUp::default(),
        database: Database::open(dir)
            .await
            .unwrap_or_else(|e| panic!("opening the database failed: {e:#}")),
    };
    Environment::open(linewright_core::environment::dir(dir), builder)
        .await
        .unwrap_or_else(|e| panic!("opening the environment failed: {e:#}"))
}

fn setting_up(command: &str) -> SetupRead {
    SetupRead::Given(Some(SetupSettings {
        command: SetupCommand::new(command.into()).unwrap(),
        max_age: Duration::from_hours(24),
        timeout: Duration::from_mins(60),
    }))
}

#[test_log::test(tokio::test)]
async fn a_rebuild_starts_now_or_after_the_build_in_progress_and_a_drain_refuses_it_and_ends_one_queued()
 {
    let world = World::new().await;
    let dir = tempfile::tempdir().unwrap();
    let mut environment = environment_of(&world, dir.path()).await;
    environment
        .poll(setting_up("true"), world.clock.now())
        .await;
    environment.settle().await;

    let started = environment.rebuild();
    let queued = environment.rebuild();
    environment.drain();
    let refused = environment.rebuild();
    environment.settle().await;

    assert_eq!(
        [started, queued, refused],
        [
            RebuildTaken::Started { build: tallied(2) },
            RebuildTaken::Queued { build: tallied(3) },
            RebuildTaken::Refused {
                reason: "`serve` is draining, and a drain starts no build".into()
            },
        ]
    );
    assert_eq!(
        environment.tally().ended.get(&tallied(3)),
        Some(&BuildEnded::NotStarted {
            reason: "a drain started no more builds".into()
        })
    );
}

#[test_log::test(tokio::test)]
async fn a_config_file_linewright_cannot_use_refuses_a_rebuild_even_from_the_start_and_without_a_setup_command_there_is_none()
 {
    let world = World::new().await;
    let dir = tempfile::tempdir().unwrap();
    let mut environment = environment_of(&world, dir.path()).await;

    environment
        .poll(SetupRead::Given(None), world.clock.now())
        .await;
    let none = environment.rebuild();
    environment
        .poll(setting_up("true"), world.clock.now())
        .await;
    environment.settle().await;
    environment
        .poll(SetupRead::Unusable, world.clock.now())
        .await;
    let refused = environment.rebuild();
    let other_dir = tempfile::tempdir().unwrap();
    let mut unusable_from_the_start = environment_of(&world, other_dir.path()).await;
    unusable_from_the_start
        .poll(SetupRead::Unusable, world.clock.now())
        .await;
    let refused_from_the_start = unusable_from_the_start.rebuild();

    let reason = "`.linewright/config.toml` cannot be used, which holds the builds";
    assert_eq!(
        [none, refused, refused_from_the_start],
        [
            RebuildTaken::NoSetupCommand,
            RebuildTaken::Refused {
                reason: reason.into()
            },
            RebuildTaken::Refused {
                reason: reason.into()
            },
        ]
    );
}

/// The `n`th build since the environment was opened.
fn tallied(n: u64) -> Tallied {
    serde_json::from_value(serde_json::json!(n)).unwrap()
}
