//! Making a session's clone, the first step of its session
//! (`docs/specs/runs.md`, Stages).

use std::panic::Location;
use std::time::Duration;

use linewright_core::reconcile::WorkKind;
use linewright_core::runs::Ending;
use linewright_core::store::local::Write;
use linewright_core::testing;
use linewright_core::testing::session_clones::*;
use linewright_core::testing::*;

/// How many sessions each of `world`'s runs started.
#[track_caller]
fn sessions_per_run(world: &World) -> impl Future<Output = Vec<usize>> + '_ {
    let caller = Location::caller();
    async move {
        runs_in(&world.dir.path().join("work"))
            .await
            .read()
            .await
            .unwrap_or_else(|e| panic!("reading the runs at {caller}: {e:#}"))
            .iter()
            .map(|run| run.sessions.len())
            .collect()
    }
}

/// The clones left in `world`'s work directory once every removal pending
/// has ended.
#[track_caller]
fn clones_left(world: &World) -> impl Future<Output = Vec<std::ffi::OsString>> + '_ {
    let caller = Location::caller();
    async move {
        world.factory.winding_up().wait().await;
        let clones = world.dir.path().join("work").join("clones");
        match std::fs::read_dir(clones) {
            Ok(entries) => entries
                .map(|entry| {
                    entry
                        .unwrap_or_else(|e| panic!("reading the clones at {caller}: {e}"))
                        .file_name()
                })
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(e) => panic!("reading the clones at {caller}: {e}"),
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
    assert_eq!(sessions_per_run(&world).await, [2]);
    assert_eq!(clones_left(&world).await, Vec::<std::ffi::OsString>::new());
    let issue = world.issue(issue);
    assert!(labels(&issue).contains(&"ready-for-human"));
    let comment = last_comment(&issue);
    assert!(
        comment.contains("it failed on its infrastructure: "),
        "{comment}"
    );
}

#[test_log::test(tokio::test)]
async fn a_fetch_into_the_copy_of_the_environment_that_fails_twice_hands_the_item_back() {
    let mut world = World::new().await;
    testing::set_up_with(&world, "mkdir -p node_modules").await;
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
    assert_eq!(sessions_per_run(&world).await, [2]);
    let issue = world.issue(ticket);
    let comment = last_comment(&issue);
    assert!(
        comment.contains("it failed on its infrastructure: "),
        "{comment}"
    );
}

#[test_log::test(tokio::test)]
async fn a_clone_still_being_made_at_its_sessions_deadline_is_the_session_timing_out() {
    let mut world = World::with(|config| {
        edit_work(config, |work| {
            work.flags_and_env.session_timeout = Some(Duration::from_secs(1));
        });
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
    assert_eq!(sessions_per_run(&world).await, [1], "not retried");
    let issue = world.issue(issue);
    assert!(labels(&issue).contains(&"ready-for-human"));
    let comment = last_comment(&issue);
    assert!(
        comment.contains(
            "The session ended unexpectedly: it timed out after 1s. The work may need cutting \
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

    assert_eq!(ended_runs(&world).await, [Ending::Cancelled]);
    assert_eq!(sessions_per_run(&world).await, [1]);
    assert_eq!(clones_left(&world).await, Vec::<std::ffi::OsString>::new());
    assert_eq!(
        world.harness.prompts(WorkKind::Implement),
        Vec::<String>::new()
    );
}

#[test_log::test(tokio::test)]
async fn an_update_with_a_conflict_works_in_a_clone_of_its_own() {
    let (mut world, pr) = testing::moved_past_with(Repository::default(), |_| {}, true).await;
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
    assert_eq!(*sessions_per_run(&world).await.last().unwrap(), 1);
}

/// `docs/specs/runs.md`, Transcripts, Running.
#[test_log::test(tokio::test)]
async fn a_session_whose_clone_could_not_be_made_ends_with_its_end() {
    let mut world = World::new().await;
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

    let sessions = session_events(&world).await;

    let kinds: Vec<Vec<&str>> = sessions
        .iter()
        .map(|session| {
            session
                .iter()
                .map(|event| event["type"].as_str().unwrap())
                .collect()
        })
        .collect();
    let ended = [
        "linewright-clone",
        "linewright-clone-ended",
        "linewright-session-ended",
    ];
    assert_eq!(kinds, [ended, ended]);
}

#[test_log::test(tokio::test)]
async fn a_factory_starting_removes_a_clone_left_behind() {
    let mut world = World::new().await;
    world.factory.winding_up().wait().await;
    let left = world
        .dir
        .path()
        .join("work")
        .join("clones")
        .join("issue-12-AbC123");
    std::fs::create_dir_all(left.join("repo")).unwrap();
    std::fs::File::create(left.with_extension("lock")).unwrap();

    world.restart(|_| {}).await;

    assert_eq!(clones_left(&world).await, Vec::<std::ffi::OsString>::new());
}
