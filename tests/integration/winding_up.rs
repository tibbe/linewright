//! What Linewright waits for before it exits, and its logging of the wait
//! (`docs/specs/operation.md`, Stopping, "Exiting" and "Logging the exit").

use crate::common;

use common::*;
use linewright::winding_up::{Remaining, WindingUp};

fn clone(name: &str) -> Remaining {
    Remaining::Clone { name: name.into() }
}

/// Spawns, as `what`, work that blocks until the sender returned is used or
/// dropped.
fn held(winding_up: &WindingUp, what: Remaining) -> std::sync::mpsc::Sender<()> {
    let (release, held) = std::sync::mpsc::channel::<()>();
    winding_up.spawn_blocking(what, move || {
        let _released = held.recv();
    });
    release
}

/// The INFO lines of `log` naming `what`, without their timestamps and from
/// `took=` on.
fn logged_of(log: &Log, what: &str) -> Vec<String> {
    log.lines("INFO")
        .into_iter()
        .filter(|line| line.contains(what))
        .map(|line| {
            let line = line.split_once(" INFO ").unwrap().1;
            line.split_once(" took=")
                .map_or(line, |(before, _)| before)
                .to_string()
        })
        .collect()
}

#[test_log::test(tokio::test)]
async fn the_wait_logs_each_thing_pending_as_it_begins_and_how_long_it_took_as_it_ends() {
    let (log, _guard) = Log::capture();
    let winding_up = WindingUp::default();
    let release = held(&winding_up, clone("issue-1-AbC123"));
    let mut exiting = winding_up.exiting();
    let published_before = exiting.borrow().clone();

    let waiting = tokio::spawn({
        let winding_up = winding_up.clone();
        async move { winding_up.wait().await }
    });
    exiting.wait_for(|waits| !waits.is_empty()).await.unwrap();
    let published_while = exiting.borrow().clone();
    eventually(|| !logged_of(&log, "issue-1-AbC123").is_empty()).await;
    let logged_while = logged_of(&log, "issue-1-AbC123");
    release.send(()).unwrap();
    waiting.await.unwrap();

    assert_eq!(published_before, []);
    assert_eq!(published_while, [clone("issue-1-AbC123")]);
    assert_eq!(exiting.borrow().clone(), []);
    assert_eq!(
        logged_while,
        [
            "linewright::winding_up: waiting before exiting waiting_for=the removal of clone issue-1-AbC123"
        ]
    );
    assert_eq!(
        logged_of(&log, "issue-1-AbC123"),
        [
            "linewright::winding_up: waiting before exiting waiting_for=the removal of clone issue-1-AbC123",
            "linewright::winding_up: waited before exiting waited_for=the removal of clone issue-1-AbC123",
        ]
    );
    assert!(
        log.text()
            .contains("waited_for=the removal of clone issue-1-AbC123 took="),
        "{}",
        log.text()
    );
}

#[test_log::test(tokio::test)]
async fn the_wait_waits_for_and_logs_what_is_spawned_while_it_waits() {
    let (log, _guard) = Log::capture();
    let winding_up = WindingUp::default();
    let first = held(&winding_up, clone("issue-1-AbC123"));
    let mut exiting = winding_up.exiting();
    let waiting = tokio::spawn({
        let winding_up = winding_up.clone();
        async move { winding_up.wait().await }
    });
    exiting.wait_for(|waits| !waits.is_empty()).await.unwrap();

    let late = held(&winding_up, Remaining::LeftClones);
    first.send(()).unwrap();
    exiting
        .wait_for(|waits| *waits == [Remaining::LeftClones])
        .await
        .unwrap();
    tokio::task::yield_now().await;
    let ended_before_the_late = waiting.is_finished();
    late.send(()).unwrap();
    waiting.await.unwrap();

    assert!(!ended_before_the_late);
    assert_eq!(
        logged_of(&log, "clones left behind"),
        [
            "linewright::winding_up: waiting before exiting waiting_for=the removal of the clones left behind",
            "linewright::winding_up: waited before exiting waited_for=the removal of the clones left behind",
        ]
    );
}

#[test_log::test(tokio::test)]
async fn with_nothing_pending_the_wait_logs_nothing() {
    let (log, _guard) = Log::capture();
    let winding_up = WindingUp::default();

    winding_up.wait().await;

    assert_eq!(log.text(), "");
}
