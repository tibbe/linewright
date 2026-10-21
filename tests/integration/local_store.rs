use crate::common;

use common::*;
use linewright::clock::Clock;
use linewright::domain::{
    Assignment, CheckState, IssueCloseReason, Login, Number, Pull, ReviewState, Timestamp,
};
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

/// When the snapshot says GitHub last updated the issue #`number`, open or
/// closed.
async fn updated_at(world: &World, number: Number) -> Timestamp {
    let snapshot = world.store.snapshot(Timestamp::MIN_UTC).await.unwrap();
    let closed = snapshot.closed.iter().find(|item| item.number == number);
    snapshot
        .issue(number)
        .map(|issue| issue.updated_at)
        .or(closed.map(|item| item.updated_at))
        .expect("the snapshot holds it")
}

#[tokio::test]
async fn every_write_to_an_issue_moves_when_it_was_last_updated() {
    let world = World::new().await;
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &["needs-triage"]);
    let blocker = world
        .store
        .open_issue("maintainer", "First", "Before.", &["needs-triage"]);
    let store = &world.store;

    let mut last = updated_at(&world, issue).await;
    let mut written = async |what: &str| {
        let now = updated_at(&world, issue).await;
        assert!(now > last, "{what} left it at {last}");
        last = now;
    };
    store.comment(issue, "A comment.").await.unwrap();
    written("a comment").await;
    store.comment_as("alice", issue, "Another.").unwrap();
    written("a person's comment").await;
    store.add_label(issue, "bug").await.unwrap();
    written("a label").await;
    store.remove_label(issue, "bug").await.unwrap();
    written("a label removed").await;
    store.assign(issue).await.unwrap();
    written("an assignment").await;
    store.unassign(issue).await.unwrap();
    written("an unassignment").await;
    store.edit_body(issue, "Edited.").await.unwrap();
    written("an edit").await;
    store.set_issue_type(issue, "Bug").await.unwrap();
    written("an issue type").await;
    store.add_blocked_by(issue, blocker).await.unwrap();
    written("a blocker").await;
    store
        .close_issue(issue, IssueCloseReason::NotPlanned)
        .await
        .unwrap();
    written("a close").await;
    store.reopen_issue_as(issue).unwrap();
    written("a reopening").await;
}

#[tokio::test]
async fn every_write_to_a_pull_request_moves_when_it_was_last_updated() {
    let world = World::new().await;
    let issue = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    let pr = world.persons_pull_closing(issue).await;
    let store = &world.store;

    let mut last = world.store.snapshot_pull(pr).await.updated_at;
    let mut written = async |what: &str| {
        let now = world.store.snapshot_pull(pr).await.updated_at;
        assert!(now > last, "{what} left it at {last}");
        last = now;
    };
    store.comment(pr, "A comment.").await.unwrap();
    written("a comment").await;
    store.add_label(pr, "bug").await.unwrap();
    written("a label").await;
    store.remove_label(pr, "bug").await.unwrap();
    written("a label removed").await;
    store.assign(pr).await.unwrap();
    written("an assignment").await;
    store
        .edit_body(pr, &format!("Closes {issue}, edited."))
        .await
        .unwrap();
    written("an edit").await;
    store
        .commit(
            "alice",
            &format!("alice-closes-{}", issue.0),
            &[("src/more.txt", "more\n")],
            "More",
        )
        .await
        .unwrap();
    written("a push").await;
    store
        .review_as(
            "bob",
            pr,
            ReviewState::ChangesRequested,
            "Fix it.",
            &[("src/thing.txt", "Here.")],
        )
        .unwrap();
    written("a review").await;
    let thread = format!("T{}-1", pr.0);
    store.reply_in_thread(pr, &thread, "Done.").await.unwrap();
    written("a reply").await;
    store.resolve_thread(pr, &thread).await.unwrap();
    written("a resolved thread").await;
    store
        .request_review(pr, &[Login::new("bob")])
        .await
        .unwrap();
    written("a review request").await;
    store
        .set_checks(pr, vec![check("test", CheckState::Passed, "")])
        .await
        .unwrap();
    written("a check").await;
    store.merge_pull(pr).await.unwrap();
    written("a merge").await;
    assert_eq!(
        updated_at(&world, issue).await,
        last,
        "the merge closed the issue"
    );
}

#[tokio::test]
async fn closing_a_pull_request_moves_when_it_was_last_updated() {
    let world = World::new().await;
    let issue = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    let pr = world.persons_pull_closing(issue).await;
    let before = world.store.snapshot_pull(pr).await.updated_at;

    world.store.close_pull_as(pr).unwrap();

    assert!(world.store.snapshot_pull(pr).await.updated_at > before);
}
