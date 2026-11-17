use std::collections::BTreeSet;
use std::panic::Location;

use crate::common;

use common::*;
use linewright::database::Database;
use linewright::domain::{Item, TicketBranchName, Timestamp};
use linewright::pushed_branches::PushedBranches;

#[track_caller]
fn branch(name: &str) -> TicketBranchName {
    TicketBranchName::parse(name).unwrap_or_else(|| panic!("{name} names no ticket's branch"))
}

#[track_caller]
fn at(time: &str) -> Timestamp {
    time.parse().unwrap()
}

#[track_caller]
fn names(branches: &[&str]) -> BTreeSet<TicketBranchName> {
    branches.iter().map(|name| branch(name)).collect()
}

/// A record of pushed branches with none in it, in a database of its own.
#[track_caller]
fn empty_record() -> impl Future<Output = (PushedBranches, tempfile::TempDir)> {
    let caller = Location::caller();
    async move {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path())
            .await
            .unwrap_or_else(|e| panic!("opening the database at {caller}: {e:#}"));
        (PushedBranches::new(db), dir)
    }
}

#[test_log::test(tokio::test)]
async fn a_read_forgets_the_branches_it_did_not_find_but_keeps_one_pushed_after_it_began() {
    let (pushed, _dir) = empty_record().await;
    pushed
        .record(&branch("7-found"), at("2026-10-01T10:00:00Z"))
        .await
        .unwrap();
    pushed
        .record(&branch("8-deleted"), at("2026-10-01T10:00:00Z"))
        .await
        .unwrap();
    pushed
        .record(
            &branch("9-pushed-during-the-read"),
            at("2026-10-01T10:00:05Z"),
        )
        .await
        .unwrap();

    pushed
        .forget_gone(&names(&["7-found"]), at("2026-10-01T10:00:01Z"))
        .await
        .unwrap();

    assert_eq!(
        pushed.names().await.unwrap(),
        names(&["7-found", "9-pushed-during-the-read"])
    );
}

#[test_log::test(tokio::test)]
async fn a_branch_pushed_again_after_a_read_began_is_kept_though_the_read_did_not_find_it() {
    let (pushed, _dir) = empty_record().await;
    pushed
        .record(&branch("7-deleted-then-pushed"), at("2026-10-01T10:00:00Z"))
        .await
        .unwrap();
    pushed
        .record(&branch("7-deleted-then-pushed"), at("2026-10-01T10:00:05Z"))
        .await
        .unwrap();

    pushed
        .forget_gone(&BTreeSet::new(), at("2026-10-01T10:00:01Z"))
        .await
        .unwrap();

    assert_eq!(
        pushed.names().await.unwrap(),
        names(&["7-deleted-then-pushed"])
    );
}

#[test_log::test(tokio::test)]
async fn a_forgotten_branch_is_no_longer_named() {
    let (pushed, _dir) = empty_record().await;
    pushed
        .record(&branch("7-deleted"), at("2026-10-01T10:00:00Z"))
        .await
        .unwrap();
    pushed
        .record(&branch("8-kept"), at("2026-10-01T10:00:00Z"))
        .await
        .unwrap();

    pushed.forget(&branch("7-deleted")).await.unwrap();

    assert_eq!(pushed.names().await.unwrap(), names(&["8-kept"]));
}

#[test_log::test(tokio::test)]
async fn branches_are_adopted_once() {
    let (pushed, _dir) = empty_record().await;
    pushed
        .record(&branch("7-recorded"), at("2026-10-01T10:00:00Z"))
        .await
        .unwrap();
    assert!(!pushed.adopted().await.unwrap());

    pushed
        .adopt(
            &names(&["7-recorded", "8-listed"]),
            at("2026-10-01T10:00:01Z"),
        )
        .await
        .unwrap();
    pushed
        .adopt(&names(&["9-listed-again"]), at("2026-10-01T10:00:02Z"))
        .await
        .unwrap();

    assert!(pushed.adopted().await.unwrap());
    assert_eq!(
        pushed.names().await.unwrap(),
        names(&["7-recorded", "8-listed"])
    );
}

#[test_log::test(tokio::test)]
async fn an_adopted_branch_counts_as_recorded_when_the_listing_began() {
    let (pushed, _dir) = empty_record().await;
    pushed
        .adopt(&names(&["8-listed"]), at("2026-10-01T10:00:00Z"))
        .await
        .unwrap();

    pushed
        .forget_gone(&BTreeSet::new(), at("2026-10-01T10:00:01Z"))
        .await
        .unwrap();

    assert_eq!(pushed.names().await.unwrap(), BTreeSet::new());
}

#[test_log::test(tokio::test)]
async fn a_poll_after_a_restart_is_given_the_tickets_branch_linewright_pushed_before() {
    let mut world = World::new().await;
    world.implement_a_ticket().await;

    world.restart(|_| {}).await;
    world.poll().await;

    let scope = world.store.last_scope().expect("a snapshot was taken");
    assert_eq!(scope.branches, names(&["1-do-the-thing"]));
}

#[test_log::test(tokio::test)]
async fn a_poll_is_given_the_items_its_kept_snapshot_holds() {
    let mut world = World::new().await;
    let (ticket, pr) = world.implement_a_ticket().await;
    world.poll().await;

    world.poll().await;

    let scope = world.store.last_scope().expect("a snapshot was taken");
    assert_eq!(scope.items, [Item::Issue(ticket), Item::Pull(pr)].into());
}

#[test_log::test(tokio::test)]
async fn a_poll_is_given_the_issue_linewright_opened_since_the_last() {
    let mut world = World::new().await;
    tickets_script(&world);
    merge_spec_change(&world).await;
    world.poll().await;
    let opened = world
        .store
        .issues()
        .last()
        .expect("an issue was opened")
        .number;

    world.factory.cycle().await.unwrap();

    let scope = world.store.last_scope().expect("a snapshot was taken");
    assert!(
        scope.items.contains(&Item::Issue(opened)),
        "{:?}",
        scope.items
    );
}
