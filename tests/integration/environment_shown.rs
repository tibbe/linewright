//! Showing and rebuilding the environment: `environment`, `environment
//! rebuild`, `status` and the server page (`docs/specs/monitoring.md`,
//! environment, and `docs/specs/sessions.md`, Building).

use crate::common;
use crate::environment::set_up_with;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use linewright::clock::Clock as _;
use linewright::environment::{BuildEnded, Builder, Environment, RebuildTaken, SetupRead, Tallied};
use linewright::redaction::Redaction;
use linewright::work_settings::{SetupCommand, SetupSettings};

/// The lines of `text`, each account's duration, such as `0s`, as `Ns`.
fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| match line.rsplit_once(", ") {
            Some((before, after))
                if after.ends_with('s')
                    && after[..after.len() - 1].bytes().all(|b| b.is_ascii_digit()) =>
            {
                format!("{before}, Ns")
            }
            _ => line.to_string(),
        })
        .collect()
}

/// An RFC 3339 time in UTC, as the commands print it.
fn shown_time(rfc3339: &str) -> String {
    rfc3339.replace('T', " ").replace('Z', " UTC")
}

/// A commit, as builds name it.
fn short(commit: &str) -> &str {
    &commit[..12]
}

#[test_log::test(tokio::test)]
async fn without_serve_environment_prints_the_latest_build_then_the_latest_that_failed() {
    let (mut world, work_dir) = world_in_work_dir().await;
    let in_work_dir = |config: &mut linewright::factory::FactoryConfig| {
        config.work_dir = work_dir.path().join(REPO);
    };
    set_up_with(&world, "echo 'npm ERR! missing lockfile'; exit 3").await;
    world.factory.prepare_environment().await.unwrap_err();
    set_up_with(&world, "echo installed").await;
    world.restart(in_work_dir).await;
    world.factory.prepare_environment().await.unwrap();

    let shown = lw_in(work_dir.path(), &["environment"]).await;
    let json = lw_in(work_dir.path(), &["environment", "--json"]).await;

    assert_eq!(shown.code, Some(0), "{}", shown.stderr);
    let answer: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    let [(latest, latest_at), (failed, failed_at)] = [0, 1].map(|i| {
        let build = &answer["environment"]["builds"][i];
        (
            build["commit"].as_str().unwrap().to_string(),
            shown_time(build["at"].as_str().unwrap()),
        )
    });
    assert_eq!(
        lines(&shown.stdout),
        [
            format!("environment: built from `{}` {latest_at}", short(&latest)),
            String::new(),
            format!("build 2: started {latest_at}, from `{}`", short(&latest)),
            "setup: echo installed → exit 0, Ns".into(),
            "  installed".into(),
            String::new(),
            format!("build 1: started {failed_at}, from `{}`", short(&failed)),
            "setup: echo 'npm ERR! missing lockfile'; exit 3 → exit 3, Ns".into(),
            "  npm ERR! missing lockfile".into(),
        ]
    );
    let build = &answer["environment"]["builds"][1];
    assert_eq!(
        (
            &build["command"],
            &build["status"],
            &build["output"],
            &build["commit"]
        ),
        (
            &serde_json::json!("echo 'npm ERR! missing lockfile'; exit 3"),
            &serde_json::json!({"exited": 3}),
            &serde_json::json!("npm ERR! missing lockfile\n"),
            &serde_json::json!(failed),
        )
    );
}

#[test_log::test(tokio::test)]
async fn without_serve_and_with_no_setup_command_environment_says_there_is_none() {
    let (mut world, work_dir) = world_in_work_dir().await;
    world.factory.prepare_environment().await.unwrap();

    let shown = lw_in(work_dir.path(), &["environment"]).await;

    assert_eq!(
        (shown.code, shown.stdout.as_str()),
        (Some(0), "environment: there is no setup command\n")
    );
}

#[test_log::test(tokio::test)]
async fn without_serve_a_rebuild_builds_the_environment_and_says_how_the_build_ended() {
    let marks = crate::environment::Marks::new();
    let fail = marks.path("fail");
    let mut world = World::new().await;
    set_up_with(&world, &format!("test ! -e '{}'", fail.display())).await;

    let succeeded = world.factory.rebuild_environment().await.unwrap();
    std::fs::write(&fail, "").unwrap();
    world.restart(|_| {}).await;
    let failed = world.factory.rebuild_environment().await.unwrap();

    assert!(
        matches!(succeeded, Some(BuildEnded::Succeeded { .. })),
        "{succeeded:?}"
    );
    assert!(
        matches!(&failed, Some(BuildEnded::Failed { error }) if error.starts_with("the setup command exited with status 1;")),
        "{failed:?}"
    );
}

#[test_log::test(tokio::test)]
async fn without_serve_and_with_no_setup_command_a_rebuild_builds_nothing() {
    let mut world = World::new().await;

    let rebuilt = world.factory.rebuild_environment().await.unwrap();

    assert_eq!(rebuilt, None);
}

/// The environment of `world`'s repository, kept in `dir`, as `serve` drives
/// it.
#[track_caller]
fn environment_of<'a>(world: &'a World, dir: &'a Path) -> impl Future<Output = Environment> + 'a {
    let caller = std::panic::Location::caller();
    async move {
        let builder = Builder {
            store: world.store.clone(),
            harness: world.harness.clone(),
            bash: bash(),
            clock: world.clock.clone(),
            redaction: Redaction::default(),
            given: Arc::default(),
            default_branch: "main".into(),
        };
        Environment::open(dir.to_path_buf(), builder)
            .await
            .unwrap_or_else(|e| panic!("opening the environment at {caller} failed: {e:#}"))
    }
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
    environment.poll(setting_up("true"), world.clock.now());
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

    environment.poll(SetupRead::Given(None), world.clock.now());
    let none = environment.rebuild();
    environment.poll(setting_up("true"), world.clock.now());
    environment.settle().await;
    environment.poll(SetupRead::Unusable, world.clock.now());
    let refused = environment.rebuild();
    let other_dir = tempfile::tempdir().unwrap();
    let mut unusable_from_the_start = environment_of(&world, other_dir.path()).await;
    unusable_from_the_start.poll(SetupRead::Unusable, world.clock.now());
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

#[test_log::test(tokio::test)]
async fn without_serve_a_config_file_linewright_cannot_use_refuses_a_rebuild_before_there_is_a_setup_command()
 {
    let world = World::new().await;
    let dir = tempfile::tempdir().unwrap();
    let mut environment = environment_of(&world, dir.path()).await;

    let refused = environment
        .rebuild_now(SetupRead::Unusable)
        .await
        .unwrap_err();

    assert_eq!(
        format!("{refused:#}"),
        "no build starts: `.linewright/config.toml` cannot be used, which holds the builds"
    );
}

/// The `n`th build since the environment was opened.
fn tallied(n: u64) -> Tallied {
    serde_json::from_value(serde_json::json!(n)).unwrap()
}
