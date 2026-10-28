use crate::common;

use std::time::Duration;

use chrono::TimeDelta;
use common::*;
use linewright::domain::{ClosedItem, ClosedKind, Number};
use linewright::webhook::Pending;

/// The closed items `serve` holds once its first poll took its snapshot.
async fn closed_items_served(world: World) -> Vec<ClosedItem> {
    let mut serving = Serving::start(world.factory, Duration::from_hours(1), Pending::default());
    let served = serving.served.wait_for(Option::is_some).await.unwrap();
    let closed = served.as_ref().unwrap().view().snapshot.closed.clone();
    drop(served);
    serving.stop().await;
    closed
}

/// Each closed item's number, with how it closed: merged by a pull request,
/// closed without one, or, for a pull request, the issues it closes.
fn endings(closed: &[ClosedItem]) -> Vec<(Number, String)> {
    closed
        .iter()
        .map(|item| {
            let ending = match &item.kind {
                ClosedKind::Issue {
                    merged_by: Some(pr),
                    ..
                } => format!("merged by {pr}"),
                ClosedKind::Issue {
                    merged_by: None, ..
                } => "closed".to_string(),
                ClosedKind::Pull { closes } => {
                    let closes: Vec<String> = closes.iter().map(ToString::to_string).collect();
                    format!("pull request closing {}", closes.join(", "))
                }
            };
            (item.number, ending)
        })
        .collect()
}

#[tokio::test]
async fn a_ticket_merged_within_the_done_window_is_held_closed_and_one_merged_before_is_not() {
    let mut world = World::new().await;
    let (_, before_pr) = world.ticket_in_review().await;
    world.store.merge_pull(before_pr).await.unwrap();
    world.poll().await;
    world.clock.advance(TimeDelta::days(8));
    let (within, within_pr) = world.ticket_in_review().await;
    world.store.merge_pull(within_pr).await.unwrap();

    let closed = closed_items_served(world).await;

    assert_eq!(
        endings(&closed),
        [(within, format!("merged by {within_pr}"))]
    );
}

#[tokio::test]
async fn an_issue_a_person_closed_and_linewrights_pull_request_closed_unmerged_are_held_closed() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world.store.close_pull_as(pr).unwrap();
    world.store.close_issue_as_completed(ticket).unwrap();

    let closed = closed_items_served(world).await;

    assert_eq!(
        endings(&closed),
        [
            (ticket, "closed".to_string()),
            (pr, format!("pull request closing {ticket}")),
        ]
    );
}

#[tokio::test]
async fn a_ticket_closed_before_the_done_window_is_held_closed_while_its_tracking_issue_is_held() {
    let world = World::new().await;
    let Tracking { closed, .. } = world.tracking_with_a_closed_ticket().await;
    let alone = world
        .store
        .open_issue(LINEWRIGHT, "Old idea", "Maybe.", &["ready-for-human"]);
    world.store.close_issue_as_completed(alone).unwrap();
    world.clock.advance(TimeDelta::days(8));

    let closed_items = closed_items_served(world).await;

    assert_eq!(endings(&closed_items), [(closed, "closed".to_string())]);
}
