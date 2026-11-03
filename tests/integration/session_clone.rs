//! Making a session's clone, the first step of its session
//! (`docs/specs/runs.md`, Stages).

use crate::common;

use std::panic::Location;
use std::time::Duration;

use common::*;
use linewright::factory::FactoryConfig;
use linewright::reconcile::WorkKind;
use linewright::runs::{Ending, Runs};
use linewright::store::local::{Paused, Write};

/// How many sessions each of `world`'s runs started.
#[track_caller]
fn sessions_per_run(world: &World) -> Vec<usize> {
    Runs::new(&world.dir.path().join("work"))
        .read()
        .unwrap()
        .iter()
        .map(|run| run.sessions.len())
        .collect()
}

/// The clones left in `world`'s work directory once every removal pending
/// has ended.
async fn clones_left(world: &World) -> Vec<std::ffi::OsString> {
    world.factory.winding_up().wait().await;
    match std::fs::read_dir(world.dir.path().join("work").join("clones")) {
        Ok(entries) => entries.map(|entry| entry.unwrap().file_name()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(e) => panic!("reading the clones: {e}"),
    }
}

fn clone_of(branch: &str) -> Write {
    Write::Clone {
        branch: branch.into(),
    }
}

/// Waits until `paused`'s clone is made, failing past 30 seconds.
#[track_caller]
fn reached(paused: &mut Paused) -> impl Future<Output = ()> + '_ {
    let caller = Location::caller();
    async move {
        if tokio::time::timeout(Duration::from_secs(30), paused.reached())
            .await
            .is_err()
        {
            panic!("the clone awaited at {caller} was never made");
        }
    }
}

#[test_log::test(tokio::test)]
async fn a_clone_that_cannot_be_made_twice_hands_the_item_back_from_a_run_with_a_session_for_each_try_and_is_removed()
 {
    let mut world = World::new().await;
    let issue =
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

    assert_eq!(
        world.harness.prompts(WorkKind::Triage),
        Vec::<String>::new()
    );
    assert_eq!(sessions_per_run(&world), [2]);
    assert_eq!(clones_left(&world).await, Vec::<std::ffi::OsString>::new());
    let issue = world.issue(issue);
    assert!(labels(&issue).contains(&"ready-for-human"));
    let comment = last_comment(&issue);
    assert!(
        comment.contains("The run failed twice on its infrastructure"),
        "{comment}"
    );
}

#[test_log::test(tokio::test)]
async fn a_fetch_into_the_copy_of_the_environment_that_fails_twice_hands_the_item_back() {
    let mut world = World::new().await;
    crate::environment::set_up_with(&world, "mkdir -p node_modules").await;
    world.poll().await;
    implement(&world);
    let ticket = open_ticket(&world, "Do the thing");
    let mut first = world.store.pause_after(Write::Fetch);

    world.factory.cycle().await.unwrap();
    reached(&mut first).await;
    let mut second = world.store.pause_after(Write::Fetch);
    first.fail();
    reached(&mut second).await;
    second.fail();
    world.factory.settle().await;

    assert_eq!(
        world.harness.prompts(WorkKind::Implement),
        Vec::<String>::new()
    );
    assert_eq!(sessions_per_run(&world), [2]);
    let issue = world.issue(ticket);
    let comment = last_comment(&issue);
    assert!(
        comment.contains("The run failed twice on its infrastructure"),
        "{comment}"
    );
}

#[test_log::test(tokio::test)]
async fn a_clone_still_being_made_at_its_sessions_deadline_is_the_session_timing_out() {
    let mut world = World::with(|config| {
        config.work.flags_and_env.session_timeout = Some(Duration::from_secs(1));
    })
    .await;
    let issue =
        world
            .store
            .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    let _hanging = world.store.pause_after(clone_of("main"));
    let started = std::time::Instant::now();

    world.poll().await;

    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(sessions_per_run(&world), [1], "not retried");
    let issue = world.issue(issue);
    assert!(labels(&issue).contains(&"ready-for-human"));
    let comment = last_comment(&issue);
    assert!(
        comment.contains(
            "The session ended unexpectedly: it timed out. The work may need cutting \
             smaller, or `session-timeout` may need to be longer."
        ),
        "{comment}"
    );
}

#[test_log::test(tokio::test)]
async fn cancelling_a_run_while_its_clone_is_being_made_ends_it_cancelled_and_removes_the_clone() {
    let mut world = World::new().await;
    implement(&world);
    let ticket = open_ticket(&world, "Do the thing");
    let mut hanging = world.store.pause_after(clone_of("main"));
    world.factory.cycle().await.unwrap();
    reached(&mut hanging).await;

    world.store.unlabel_as(ticket, "ready-for-agent").unwrap();
    world.factory.cycle().await.unwrap();
    world.factory.settle().await;

    assert_eq!(ended_runs(&world), [Ending::Cancelled]);
    assert_eq!(sessions_per_run(&world), [1]);
    assert_eq!(clones_left(&world).await, Vec::<std::ffi::OsString>::new());
    assert_eq!(
        world.harness.prompts(WorkKind::Implement),
        Vec::<String>::new()
    );
}

#[test_log::test(tokio::test)]
async fn an_update_with_a_conflict_works_in_a_clone_of_its_own() {
    let (mut world, pr) =
        crate::pipeline::moved_past_with(Repository::default(), |_| {}, true).await;
    let branch = world.store.snapshot_pull(pr).await.head_ref;
    let mut tried = world.store.pause_after(clone_of(&branch));

    world.factory.cycle().await.unwrap();
    reached(&mut tried).await;
    let mut sessions = world.store.pause_after(clone_of(&branch));
    tried.release();
    reached(&mut sessions).await;
    sessions.release();
    world.factory.settle().await;

    assert_eq!(world.harness.prompts(WorkKind::Update).len(), 1);
    assert_eq!(*sessions_per_run(&world).last().unwrap(), 1);
}

/// A world whose work dir is `REPO`'s, with `configure`d.
async fn world_in_work_dir_with(configure: impl FnOnce(&mut FactoryConfig)) -> World {
    World::with(|config| {
        config.work_dir = config.work_dir.join(REPO);
        configure(config);
    })
    .await
}

/// What `lw logs` with `args` prints of `world`.
#[track_caller]
fn logs<'a>(world: &'a World, args: &'a [&str]) -> impl Future<Output = String> + 'a {
    let caller = Location::caller();
    async move {
        let output = lw_in(&world.dir.path().join("work"), &[&["logs"], args].concat()).await;
        assert_eq!(output.code, Some(0), "{}\nat {caller}", output.stderr);
        output.stdout
    }
}

/// `docs/specs/design_system.md`, Transcripts, The clone.
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

/// `docs/specs/runs.md`, Stages, 1a; `docs/specs/monitoring.md`, logs, The
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
        config.work.flags_and_env.session_timeout = Some(Duration::from_secs(1));
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
