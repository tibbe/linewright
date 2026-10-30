use crate::common;

use common::*;
use linewright::domain::{ClosedKind, Login, Marking, Number, Pull};

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
    assert!(read.candidate(&Login::new(LINEWRIGHT), &read.closed_by, Marking::Marked));
}

/// A person's open pull request, as `alice` opens it.
async fn persons_pull(world: &World, branch: &str) -> Number {
    world
        .store
        .open_pull_as(
            "alice",
            branch,
            "Tidy up",
            "Tidies up.",
            &[(&format!("src/{branch}.txt"), "tidy\n")],
        )
        .await
        .unwrap()
}

fn numbers(pulls: &[Pull]) -> Vec<Number> {
    pulls.iter().map(|pull| pull.number).collect()
}

#[tokio::test]
async fn a_persons_open_pull_requests_linewright_is_assigned_to_or_commented_on_are_held_apart_from_its_own()
 {
    let world = World::new().await;
    let assigned = persons_pull(&world, "assigned").await;
    world
        .store
        .assign_as("maintainer", assigned, LINEWRIGHT)
        .unwrap();
    let commented = persons_pull(&world, "commented").await;
    world
        .store
        .comment_as(LINEWRIGHT, commented, "Looks good.")
        .unwrap();
    let untouched = persons_pull(&world, "untouched").await;
    world
        .store
        .comment_as("maintainer", untouched, "Thanks.")
        .unwrap();

    let view = view_served(world).await;

    assert_eq!(numbers(&view.snapshot.peoples_pulls), [assigned, commented]);
    assert_eq!(numbers(&view.snapshot.pulls), []);
}

#[tokio::test]
async fn a_persons_pull_request_closed_unmerged_after_linewright_worked_it_is_held_closed_with_its_author()
 {
    let world = World::new().await;
    let worked = persons_pull(&world, "worked").await;
    world
        .store
        .label_as(LINEWRIGHT, worked, "linewright:stalled")
        .unwrap();
    world.store.close_pull_as(worked).unwrap();
    let untouched = persons_pull(&world, "untouched").await;
    world.store.close_pull_as(untouched).unwrap();

    let view = view_served(world).await;

    let closed: Vec<(Number, &ClosedKind)> = view
        .snapshot
        .closed
        .iter()
        .map(|item| (item.number, &item.kind))
        .collect();
    assert_eq!(
        closed,
        [(
            worked,
            &ClosedKind::Pull {
                closes: vec![],
                author: Some(Login::new("alice")),
            }
        )]
    );
    assert_eq!(numbers(&view.snapshot.peoples_pulls), []);
}

#[tokio::test]
async fn an_open_issue_linewright_only_commented_on_is_worked() {
    let world = World::new().await;
    let commented = world
        .store
        .open_issue("maintainer", "Odd crash", "It crashed.", &[]);
    world
        .store
        .comment_as(LINEWRIGHT, commented, "Which version?")
        .unwrap();
    let theirs = world
        .store
        .open_issue("maintainer", "Another crash", "It crashed too.", &[]);
    world.store.comment_as("alice", theirs, "Me too.").unwrap();

    let view = view_served(world).await;

    let worked: Vec<(Number, bool)> = view
        .snapshot
        .issues
        .iter()
        .map(|issue| {
            let worked = issue.worked_by(&Login::new(LINEWRIGHT), &issue.closed_by);
            (issue.number, worked)
        })
        .collect();
    assert_eq!(worked, [(commented, true), (theirs, false)]);
}

#[tokio::test]
async fn a_closed_issue_linewright_only_commented_on_is_worked_and_one_only_assigned_to_it_by_hand_is_not()
 {
    let world = World::new().await;
    let open = |title: &str| {
        world
            .store
            .open_issue("maintainer", title, "It crashed.", &["ready-for-agent"])
    };
    let commented = open("Commented on");
    world
        .store
        .comment_as(LINEWRIGHT, commented, "Fixed upstream.")
        .unwrap();
    let assigned = open("Assigned by hand");
    world
        .store
        .assign_as("maintainer", assigned, LINEWRIGHT)
        .unwrap();
    for issue in [commented, assigned] {
        world.store.close_issue_as_completed(issue).unwrap();
    }

    let view = view_served(world).await;

    let worked: Vec<(Number, bool)> = view
        .snapshot
        .closed
        .iter()
        .map(|item| (item.number, item.worked_by(&Login::new(LINEWRIGHT))))
        .collect();
    assert_eq!(worked, [(commented, true), (assigned, false)]);
}
