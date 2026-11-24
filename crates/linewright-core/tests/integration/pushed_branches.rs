use std::collections::BTreeSet;

use linewright_core::clock::Clock as _;
use linewright_core::database::Database;
use linewright_core::domain::{Item, Number, TicketBranchName, Timestamp};
use linewright_core::pushed_branches::PushedBranches;
use linewright_core::store::Store as _;
use linewright_core::testing::*;

fn branch(name: &str) -> TicketBranchName {
    TicketBranchName::parse(name).unwrap_or_else(|| panic!("{name} names no ticket's branch"))
}

fn at(time: &str) -> Timestamp {
    time.parse().unwrap()
}

fn names(branches: &[&str]) -> BTreeSet<TicketBranchName> {
    branches.iter().map(|name| branch(name)).collect()
}

/// A record of pushed branches with none in it, in a database of its own.
async fn empty_record() -> (PushedBranches, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path())
        .await
        .unwrap_or_else(|e| panic!("opening the database: {e:#}"));
    (PushedBranches::new(db), dir)
}

#[test_log::test(tokio::test)]
async fn a_read_forgets_the_branches_it_found_gone_but_keeps_one_pushed_after_it_began() {
    let (pushed, _dir) = empty_record().await;
    for name in ["7-found", "8-deleted", "10-recorded-unread"] {
        pushed
            .record(&branch(name), at("2026-10-01T10:00:00Z"))
            .await
            .unwrap();
    }
    pushed
        .record(
            &branch("9-pushed-during-the-read"),
            at("2026-10-01T10:00:05Z"),
        )
        .await
        .unwrap();

    pushed
        .forget_gone(
            &names(&["8-deleted", "9-pushed-during-the-read"]),
            at("2026-10-01T10:00:01Z"),
        )
        .await
        .unwrap();

    assert_eq!(
        pushed.names().await.unwrap(),
        names(&["10-recorded-unread", "7-found", "9-pushed-during-the-read"])
    );
}

#[test_log::test(tokio::test)]
async fn a_branch_pushed_again_after_a_read_began_is_kept_though_the_read_found_it_gone() {
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
        .forget_gone(
            &names(&["7-deleted-then-pushed"]),
            at("2026-10-01T10:00:01Z"),
        )
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
        .forget_gone(&names(&["8-listed"]), at("2026-10-01T10:00:01Z"))
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
    let (ticket, pr) = world.ticket_in_review().await;
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

/// The record of `world`'s factory.
async fn world_record(world: &World) -> PushedBranches {
    PushedBranches::new(
        Database::open(&world.dir.path().join("work"))
            .await
            .unwrap(),
    )
}

/// A branch for `ticket` whose head Linewright committed, pushed without
/// Linewright recording it, as before the record began or before the push
/// path records it.
async fn branch_pushed_unrecorded(world: &World, ticket: Number, slug: &str) -> String {
    let branch = format!("{}-{slug}", ticket.0);
    world
        .store
        .commit(LINEWRIGHT, &branch, &[("work.txt", "work\n")], "Work")
        .await
        .unwrap();
    branch
}

/// `docs/specs/sources_of_work.md`, Polling: what a poll reads grows with
/// the branches Linewright pushed, so every branch is listed only once, to
/// adopt those pushed before the record began.
#[test_log::test(tokio::test)]
async fn polls_after_the_first_read_the_recorded_branches_without_listing_every_branch() {
    let mut world = World::new().await;
    let ticket = open_designed_gadgets(&world);
    let branch = branch_pushed_unrecorded(&world, ticket, "earlier").await;

    world.poll().await;
    world.poll().await;

    assert_eq!(world.store.branch_listings(), 1);
    let scope = world.store.last_scope().unwrap();
    assert_eq!(
        scope
            .branches
            .iter()
            .map(linewright_core::domain::TicketBranchName::as_str)
            .collect::<Vec<_>>(),
        [branch.as_str()]
    );
}

#[test_log::test(tokio::test)]
async fn the_branch_of_a_ticket_closed_without_a_pull_request_is_deleted_when_adopted() {
    let mut world = World::new().await;
    let ticket = open_designed_gadgets(&world);
    world.store.assign(ticket).await.unwrap();
    let branch = branch_pushed_unrecorded(&world, ticket, "earlier").await;
    world.store.close_issue_as_completed(ticket).unwrap();

    world.poll().await;

    let listed = world.store.ticket_branches_pushed().await.unwrap();
    assert!(listed.iter().all(|b| b.as_str() != branch), "{listed:?}");
    assert_eq!(
        world_record(&world).await.names().await.unwrap(),
        names(&[])
    );
}

#[test_log::test(tokio::test)]
async fn the_branch_of_a_ticket_closed_without_a_pull_request_is_deleted_when_recorded_at_its_push()
{
    let mut world = World::new().await;
    world.poll().await;
    let ticket = open_designed_gadgets(&world);
    world.store.assign(ticket).await.unwrap();
    let branch = branch_pushed_unrecorded(&world, ticket, "pushed").await;
    let record = world_record(&world).await;
    record
        .record(&self::branch(&branch), world.clock.now())
        .await
        .unwrap();
    world.store.close_issue_as_completed(ticket).unwrap();

    world.poll().await;

    let listed = world.store.ticket_branches_pushed().await.unwrap();
    assert!(listed.iter().all(|b| b.as_str() != branch), "{listed:?}");
    assert_eq!(record.names().await.unwrap(), names(&[]));
}

#[test_log::test(tokio::test)]
async fn a_branch_a_person_deleted_is_forgotten_at_the_next_poll() {
    let mut world = World::new().await;
    let ticket = open_designed_gadgets(&world);
    let branch = branch_pushed_unrecorded(&world, ticket, "earlier").await;
    world.poll().await;
    let record = world_record(&world).await;
    assert_eq!(record.names().await.unwrap(), names(&[&branch]));
    world.clock.advance(chrono::TimeDelta::minutes(1));

    world.store.delete_branch(&branch).await.unwrap();
    world.poll().await;

    assert_eq!(record.names().await.unwrap(), names(&[]));
}

/// The factory forgets a branch a poll found gone only when it was recorded
/// before the poll began.
#[test_log::test(tokio::test)]
async fn a_branch_recorded_once_a_poll_began_is_kept_though_the_poll_found_it_gone() {
    let mut world = World::new().await;
    world.poll().await;
    let record = world_record(&world).await;
    let poll_began = world.clock.now();

    record
        .record(
            &branch("7-pushed-during-the-poll"),
            poll_began + chrono::TimeDelta::seconds(30),
        )
        .await
        .unwrap();
    world.poll().await;

    assert_eq!(
        record.names().await.unwrap(),
        names(&["7-pushed-during-the-poll"])
    );
}
