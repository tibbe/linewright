mod common;

use common::*;
use linewright::clock::Clock;
use linewright::domain::{Assignment, Login, Number, Pull, Timestamp};
use linewright::store::Store;

async fn snapshot_pull(world: &World, pr: Number) -> Pull {
    let snapshot = world.store.snapshot(Timestamp::MIN_UTC).await.unwrap();
    snapshot.pull(pr).expect("the snapshot holds it").clone()
}

#[tokio::test]
async fn a_pull_request_lists_its_assignees_and_who_assigned_and_unassigned_whom_when() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    let opened = snapshot_pull(&world, pr).await.assignments.len();

    let start = world.clock.now();
    world
        .store
        .unassign_as("maintainer", pr, "factory")
        .unwrap();
    let unassigned = world.clock.now();
    world.store.assign_as("lead", pr, "maintainer").unwrap();
    let end = world.clock.now();

    let pull = snapshot_pull(&world, pr).await;
    assert_eq!(pull.assignees, [Login::new("maintainer")]);
    let changes = &pull.assignments[opened..];
    assert!(
        matches!(
            changes,
            [
                Assignment::Unassigned { assignee, actor, at: first },
                Assignment::Assigned { assignee: second_assignee, actor: second_actor, at: second },
            ] if assignee.as_str() == "factory"
                && actor.as_str() == "maintainer"
                && second_assignee.as_str() == "maintainer"
                && second_actor.as_str() == "lead"
                && start < *first && *first < unassigned
                && unassigned < *second && *second < end
        ),
        "{changes:?}"
    );
}

#[tokio::test]
async fn a_pull_request_read_again_lists_its_assignments_as_a_snapshot_does() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.store.unassign(pr).await.unwrap();
    world.store.assign(pr).await.unwrap();

    let read = world.store.linewright_pull(pr).await.unwrap();

    assert_eq!(read, Some(snapshot_pull(&world, pr).await));
    assert_eq!(read.unwrap().assignees, [Login::new("factory")]);
}

#[tokio::test]
async fn unassigning_someone_a_pull_request_is_not_assigned_to_records_nothing() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    let before = snapshot_pull(&world, pr).await.assignments;

    world.store.unassign_as("maintainer", pr, "alice").unwrap();

    assert_eq!(snapshot_pull(&world, pr).await.assignments, before);
}

#[tokio::test]
async fn branches_whose_names_differ_only_in_case_are_kept_apart() {
    let world = World::new().await;
    let branches = ["1-Gadgets", "1-gadgets"];

    for branch in branches {
        let contents = format!("{branch}\n");
        world
            .store
            .commit(
                LINEWRIGHT,
                branch,
                &[("src/gadgets.txt", &contents)],
                "Gadgets",
            )
            .await
            .unwrap();
    }

    for branch in branches {
        let contents = world.store.file(branch, "src/gadgets.txt").await.unwrap();
        assert_eq!(contents, format!("{branch}\n"));
    }
}
