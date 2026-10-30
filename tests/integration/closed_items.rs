use crate::common;

use chrono::TimeDelta;
use common::*;
use linewright::domain::Number;

#[tokio::test]
async fn a_ticket_closed_before_the_done_window_is_held_closed_while_its_tracking_issue_is_held() {
    let world = World::new().await;
    let closed = world.tracking_with_a_closed_ticket().await;
    let alone = world
        .store
        .open_issue(LINEWRIGHT, "Old idea", "Maybe.", &["ready-for-human"]);
    world.store.close_issue_as_completed(alone).unwrap();
    world.clock.advance(TimeDelta::days(8));

    let view = view_served(world).await;

    let held: Vec<Number> = view
        .snapshot
        .closed
        .iter()
        .map(|item| item.number)
        .collect();
    assert_eq!(held, [closed]);
}
