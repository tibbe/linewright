//! The local store, as the factory reads it through the store interface:
//! what it reports of the changes made to it (`docs/specs/architecture.md`,
//! "The store is an interface"; `CODING_STANDARDS.md`, "Internal
//! libraries").

mod common;

use common::*;
use linewright::clock::Clock;
use linewright::domain::{LabelChange, Number, Pull, Timestamp};
use linewright::store::Store;

/// Linewright's pull request as a snapshot taken now holds it.
async fn snapshot_pull(world: &World, pr: Number) -> Pull {
    let snapshot = world.store.snapshot(Timestamp::MIN_UTC).await.unwrap();
    snapshot.pull(pr).expect("the snapshot holds it").clone()
}

#[tokio::test]
async fn a_pull_request_lists_its_labels_and_when_each_was_applied_and_removed() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;

    let start = world.clock.now();
    world.store.label_as(pr, "ready-for-human").unwrap();
    let applied = world.clock.now();
    world.store.unlabel_as(pr, "ready-for-human").unwrap();
    let removed = world.clock.now();
    world.store.add_label(pr, "ready-for-agent").await.unwrap();
    let end = world.clock.now();

    let pull = snapshot_pull(&world, pr).await;
    assert_eq!(pull.labels, ["ready-for-agent"]);
    let changes: Vec<(&str, LabelChange)> = pull
        .label_events
        .iter()
        .map(|e| (e.label.as_str(), e.change))
        .collect();
    assert_eq!(
        changes,
        [
            ("ready-for-human", LabelChange::Applied),
            ("ready-for-human", LabelChange::Removed),
            ("ready-for-agent", LabelChange::Applied),
        ]
    );
    let t: Vec<Timestamp> = pull.label_events.iter().map(|e| e.at).collect();
    assert!(start < t[0] && t[0] < applied, "{t:?}");
    assert!(applied < t[1] && t[1] < removed, "{t:?}");
    assert!(removed < t[2] && t[2] < end, "{t:?}");
}

#[tokio::test]
async fn a_pull_request_read_again_lists_its_labels_as_a_snapshot_does() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.store.label_as(pr, "ready-for-human").unwrap();
    world
        .store
        .remove_label(pr, "ready-for-human")
        .await
        .unwrap();
    world.store.label_as(pr, "ready-for-agent").unwrap();

    let read = world.store.linewright_pull(pr).await.unwrap();

    assert_eq!(read, Some(snapshot_pull(&world, pr).await));
    assert_eq!(read.unwrap().label_events.len(), 3);
}

#[tokio::test]
async fn removing_a_label_a_pull_request_lacks_records_nothing() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;

    world
        .store
        .remove_label(pr, "ready-for-human")
        .await
        .unwrap();

    assert_eq!(snapshot_pull(&world, pr).await.label_events, []);
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
