//! Making a session's clone, the first step of its session
//! (`docs/specs/runs.md`, Stages).

use std::time::Duration;

use linewright_core::factory::FactoryConfig;
use linewright_core::testing::session_clones::*;
use linewright_core::testing::*;

use crate::programs::*;

/// A world whose work dir is `REPO`'s, with `configure`d.
async fn world_in_work_dir_with(configure: impl FnOnce(&mut FactoryConfig)) -> World {
    World::with(|config| {
        config.work_dir = config.work_dir.join(REPO);
        configure(config);
    })
    .await
}

/// What `lw logs` with `args` prints of `world`.
async fn logs(world: &World, args: &[&str]) -> String {
    let output = lw_in(&world.dir.path().join("work"), &[&["logs"], args].concat()).await;
    assert_eq!(output.code, Some(0), "{}", output.stderr);
    output.stdout
}

/// `docs/specs/runs.md`, Transcripts, The clone.
#[test_log::test(tokio::test)]
async fn logs_shows_the_making_of_a_sessions_clone_before_its_maintenance_command() {
    let mut world = world_in_work_dir_with(|config| maintaining_with(config, "true")).await;
    crate::logs::implement_as_recorded(&world);
    world.store.open_issue(
        "maintainer",
        "Ship gh",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.poll().await;

    let shown = logs(&world, &["1"]).await;

    let shown: Vec<&str> = shown.lines().collect();
    assert!(shown[2].starts_with(" 0:00  clone → "), "{shown:#?}");
    assert!(
        shown[3].contains("maintenance: true → exit 0"),
        "{shown:#?}"
    );
}

/// `docs/specs/runs.md`, Stages, 1a; `docs/specs/runs.md`, `lw logs`, The
/// clone.
#[test_log::test(tokio::test)]
async fn logs_shows_why_a_sessions_clone_could_not_be_made() {
    let mut world = world_in_work_dir_with(|_| {}).await;
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    let mut first = world.store.pause_after(clone_of("main"));
    world.factory.cycle().await.unwrap();
    reached(&mut first).await;
    let mut second = world.store.pause_after(clone_of("main"));
    first.fail();
    reached(&mut second).await;
    second.fail();
    world.factory.settle().await;

    let shown = logs(&world, &["1"]).await;
    let json = json_objects(&logs(&world, &["1", "--json"]).await);

    let error = "the store call failed after Clone { branch: \"main\" }";
    let shown: Vec<&str> = shown.lines().collect();
    assert_eq!(shown.len(), 7, "{shown:#?}");
    for session in [&shown[1..4], &shown[4..7]] {
        assert!(session[1].starts_with(" 0:00  clone → "), "{shown:#?}");
        assert_eq!(session[2], format!("         {error}"));
    }
    assert_eq!(json[2]["type"], "clone");
    assert_eq!(json[2]["error"], error);
}

/// `docs/specs/runs.md`, Stages; Ending.
#[test_log::test(tokio::test)]
async fn a_clone_still_being_made_at_its_sessions_deadline_or_when_its_run_is_cancelled_says_so() {
    let mut world = world_in_work_dir_with(|config| {
        edit_work(config, |work| {
            work.flags_and_env.session_timeout = Some(Duration::from_secs(1));
        });
    })
    .await;
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    let _hanging = world.store.pause_after(clone_of("main"));
    world.poll().await;
    implement(&world);
    let ticket = open_ticket(&world, "Do the thing");
    let mut cancelling = world.store.pause_after(clone_of("main"));
    world.factory.cycle().await.unwrap();
    reached(&mut cancelling).await;
    world.store.unlabel_as(ticket, "ready-for-agent").unwrap();
    world.factory.cycle().await.unwrap();
    world.factory.settle().await;

    let timed_out = logs(&world, &["1"]).await;
    let cancelled = logs(&world, &[&ticket.0.to_string()]).await;

    assert!(timed_out.ends_with("\n         timed out\n"), "{timed_out}");
    assert!(cancelled.ends_with("\n         cancelled\n"), "{cancelled}");
}
