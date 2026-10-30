use crate::common;

use std::sync::Arc;
use std::time::Duration;

use chrono::TimeDelta;

use common::*;
use linewright::domain::{Login, Number, Snapshot};
use linewright::store::local::LocalStore;
use linewright::webhook::{Listener, Pending};
use serde_json::json;

/// A poll interval no test waits out: what such a test sees, deliveries did.
const NEVER: Duration = Duration::from_hours(1);

struct Served {
    store: Arc<LocalStore>,
    listener: Listener,
    serving: Serving,
    _dir: tempfile::TempDir,
}

/// Serves `world`, polling every `poll_interval`, once its first poll took
/// its snapshot and `serve` published it.
async fn serve(world: World, poll_interval: Duration) -> Served {
    let World {
        store,
        factory,
        dir,
        ..
    } = world;
    let pending = Pending::default();
    let listener = Listener::bind(
        &webhooks("127.0.0.1:0"),
        pending.clone(),
        std::sync::Arc::new(linewright::clock::SystemClock),
    )
    .await
    .unwrap();
    let polled = store.snapshots_taken();
    let serving = Serving::start(factory, poll_interval, pending);
    eventually(|| store.snapshots_taken() > polled).await;
    // The store counts the snapshot before `serve` publishes it.
    eventually(|| serving.served.borrow().is_some()).await;
    Served {
        store,
        listener,
        serving,
        _dir: dir,
    }
}

impl Served {
    async fn deliver(&self, event: &str, body: serde_json::Value) {
        let status = signed(&self.listener, event, &body.to_string()).await;
        assert!(status.is_success(), "{event}: {status}");
    }

    /// The snapshot `serve` holds now.
    fn snapshot(&self) -> Arc<Snapshot> {
        let served = self.serving.served.borrow();
        served.as_ref().expect("served").view().snapshot
    }

    async fn stop(self) {
        self.serving.stop().await;
        self.listener.stop().await;
    }
}

fn claimed(store: &LocalStore, issue: Number) -> bool {
    let issue = store.issue(issue).unwrap();
    issue.assignees.contains(&Login::new(LINEWRIGHT))
}

#[tokio::test]
async fn an_issue_labeled_ready_for_agent_is_claimed_on_the_delivery() {
    let world = World::new().await;
    implement(&world);
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    let served = serve(world, NEVER).await;

    served
        .store
        .label_as("maintainer", issue, "ready-for-agent")
        .unwrap();
    served
        .deliver(
            "issues",
            json!({"action": "labeled", "issue": {"number": issue.0}}),
        )
        .await;

    eventually(|| claimed(&served.store, issue)).await;
    assert_eq!(served.store.snapshots_taken(), 1);
    served.stop().await;
}

fn about_issue(number: Number) -> serde_json::Value {
    json!({"issue": {"number": number.0}})
}

#[tokio::test]
async fn a_refresh_holds_the_closed_issues_a_fresh_snapshot_would() {
    let world = World::new().await;
    let ticket = world.tracking_with_a_closed_ticket().await;
    let alone = world
        .store
        .open_issue(LINEWRIGHT, "Old idea", "Maybe.", &["ready-for-human"]);
    world.store.close_issue_as_completed(alone).unwrap();
    world.clock.advance(TimeDelta::days(8));
    let marker = world
        .store
        .open_issue(LINEWRIGHT, "Marker", "Now.", &["ready-for-human"]);
    let served = serve(world, NEVER).await;

    served.store.close_issue_as_completed(marker).unwrap();
    for issue in [ticket, alone, marker] {
        served.deliver("issues", about_issue(issue)).await;
    }

    let held = || -> Vec<Number> {
        let snapshot = served.snapshot();
        snapshot.closed.iter().map(|item| item.number).collect()
    };
    eventually(|| held().contains(&marker)).await;
    assert_eq!(held(), [ticket, marker]);
    served.stop().await;
}

#[tokio::test]
async fn polls_go_on_at_the_poll_interval_while_deliveries_arrive() {
    let world = World::new().await;
    let issue = world
        .store
        .open_issue("maintainer", "Leave it", "Later.", &["ready-for-human"]);
    let served = serve(world, Duration::from_millis(100)).await;

    let polling = tokio::time::timeout(Duration::from_secs(10), async {
        while served.store.snapshots_taken() < 4 {
            served.deliver("issues", about_issue(issue)).await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;

    assert!(polling.is_ok(), "polls stopped while deliveries arrived");
    served.stop().await;
}

#[tokio::test]
async fn a_failed_read_for_a_delivery_is_logged_and_the_next_poll_catches_up() {
    let (log, _guard) = Log::capture();
    let world = World::new().await;
    implement(&world);
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    let served = serve(world, Duration::from_millis(200)).await;

    // Polls read no single item, so only the delivery's read fails.
    served.store.fail_item_reads("GitHub is down");
    served
        .store
        .label_as("maintainer", issue, "ready-for-agent")
        .unwrap();
    served.deliver("issues", about_issue(issue)).await;
    let failed = || {
        log.lines("ERROR")
            .iter()
            .any(|line| line.contains("deliveries failed") && line.contains("GitHub is down"))
    };
    eventually(failed).await;
    assert_eq!(served.serving.failed_polls(), None);
    served.store.restore_reads();

    eventually(|| claimed(&served.store, issue)).await;
    served.stop().await;
}
