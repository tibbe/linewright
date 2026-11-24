use std::time::Duration;

use linewright_core::status::Polls;
use linewright_core::testing::*;
use linewright_core::webhook::Pending;

/// A poll interval no test waits out.
const INTERVAL: Duration = Duration::from_hours(1);

fn polls(serving: &Serving) -> Polls {
    serving.served.borrow().as_ref().unwrap().view().polls
}

#[test_log::test(tokio::test)]
async fn the_next_poll_published_is_one_interval_after_the_poll_started() {
    let world = World::new().await;
    let mut serving = Serving::start(world.factory, INTERVAL, Pending::default());

    let polling = serving.polling.wait_for(Option::is_some).await.unwrap();
    let next = polling.as_ref().unwrap().next;
    drop(polling);

    assert_eq!(next, polls(&serving).last + INTERVAL);
    assert_eq!(polls(&serving).next, next);
    serving.stop().await;
}
