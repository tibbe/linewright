//! Making a session's clone, the first step of its session
//! (`docs/specs/runs.md`, Stages).

use crate::common;

use std::panic::Location;
use std::time::Duration;

use common::*;
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
async fn a_clone_that_cannot_be_made_twice_hands_the_item_back_from_a_run_with_a_session_for_each_try()
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
    let issue = world.issue(issue);
    assert!(labels(&issue).contains(&"ready-for-human"));
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
async fn cancelling_a_run_while_its_clone_is_being_made_ends_it_cancelled() {
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
