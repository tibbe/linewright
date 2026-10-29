use crate::common;

use common::*;
use linewright::domain::{Login, Marking};

#[tokio::test]
async fn an_open_issue_linewright_labeled_and_claimed_is_read_and_managed_after_a_person_removed_both()
 {
    let world = World::new().await;
    let issue = world
        .store
        .open_issue("maintainer", "Odd crash", "It crashed.", &[]);
    world
        .store
        .label_as(LINEWRIGHT, issue, "ready-for-human")
        .unwrap();
    world
        .store
        .assign_as(LINEWRIGHT, issue, LINEWRIGHT)
        .unwrap();
    world.store.unlabel_as(issue, "ready-for-human").unwrap();
    world
        .store
        .unassign_as("maintainer", issue, LINEWRIGHT)
        .unwrap();

    let view = view_served(world).await;

    let [read] = view.snapshot.issues.as_slice() else {
        panic!("one issue read, not {:?}", view.snapshot.issues);
    };
    assert_eq!(read.number, issue);
    assert!(read.managed(&Login::new(LINEWRIGHT), &read.closed_by, Marking::Marked));
}
