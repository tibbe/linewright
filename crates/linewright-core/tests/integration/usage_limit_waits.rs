use std::time::Duration;

use linewright_core::domain::Timestamp;
use linewright_core::usage_limit::{Deadline, UsageLimitHeld, UsageLimitWaits};
use serde_json::{Value, json};
use tokio::time::Instant;

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

/// Whether `deadline` is reached within `within`, the paused clock moving
/// on by as much.
async fn reached_within(deadline: &Deadline, within: Duration) -> bool {
    tokio::time::timeout(within, deadline.reached())
        .await
        .is_ok()
}

#[test_log::test(tokio::test(start_paused = true))]
async fn a_deadline_is_reached_once_its_timeout_has_passed() {
    let waits = UsageLimitWaits::default().session(Instant::now(), Duration::from_mins(60));
    let deadline = waits.deadline().clone();

    assert!(!reached_within(&deadline, Duration::from_mins(59)).await);
    assert!(reached_within(&deadline, Duration::from_mins(2)).await);
    assert_eq!(deadline.counted(Instant::now()), Duration::from_mins(60));
}

/// `docs/specs/runs.md`, Usage limit, The session timeout.
#[test_log::test(tokio::test(start_paused = true))]
async fn a_wait_does_not_count_toward_the_session_timeout() {
    let start = Instant::now();
    let waits = UsageLimitWaits::default().session(start, Duration::from_mins(60));
    let deadline = waits.deadline().clone();
    tokio::time::advance(Duration::from_mins(20)).await;

    let (waiting, _) = waits.begin(at("2026-10-13T06:20:00Z"), None);
    assert!(!reached_within(&deadline, Duration::from_mins(120)).await);
    assert_eq!(deadline.counted(Instant::now()), Duration::from_mins(20));
    drop(waiting);

    assert!(!reached_within(&deadline, Duration::from_mins(39)).await);
    assert_eq!(deadline.counted(Instant::now()), Duration::from_mins(59));
    assert!(reached_within(&deadline, Duration::from_mins(2)).await);
    assert_eq!(Instant::now() - start, Duration::from_mins(180));
}

#[test_log::test(tokio::test(start_paused = true))]
async fn a_deadline_reached_during_a_wait_holds_off_until_it_ends() {
    let waits = UsageLimitWaits::default().session(Instant::now(), Duration::from_mins(60));
    let deadline = waits.deadline().clone();
    let reached = tokio::spawn(async move { deadline.reached().await });
    tokio::time::advance(Duration::from_mins(30)).await;

    let (waiting, _) = waits.begin(at("2026-10-13T06:30:00Z"), None);
    tokio::time::sleep(Duration::from_mins(90)).await;
    assert!(!reached.is_finished());
    drop(waiting);
    tokio::time::sleep(Duration::from_mins(31)).await;

    assert!(reached.is_finished());
}

#[test_log::test(tokio::test(start_paused = true))]
async fn a_wait_pauses_only_its_own_sessions_deadline() {
    let registry = UsageLimitWaits::default();
    let waits = registry.session(Instant::now(), Duration::from_mins(60));
    let other = registry.session(Instant::now(), Duration::from_mins(60));

    let _waiting = waits.begin(at("2026-10-13T06:00:00Z"), None);

    assert!(reached_within(other.deadline(), Duration::from_mins(61)).await);
    assert_eq!(waits.deadline().counted(Instant::now()), Duration::ZERO);
}

/// `docs/specs/runs.md`, Holds: the usage limit is in force while a session
/// waits for it, and ends when none does.
#[test_log::test(tokio::test)]
async fn the_hold_is_in_force_while_a_session_waits_with_its_stated_reset() {
    let registry = UsageLimitWaits::default();
    let held = registry.held();
    let waits = registry.session(Instant::now(), Duration::from_mins(60));
    assert_eq!(*held.borrow(), None);

    let (waiting, _) = waits.begin(at("2026-10-13T06:00:00Z"), Some(at("2026-10-13T07:50:00Z")));
    assert_eq!(
        *held.borrow(),
        Some(UsageLimitHeld {
            resets_at: Some(at("2026-10-13T07:50:00Z"))
        })
    );

    drop(waiting);
    assert_eq!(*held.borrow(), None);
}

#[test_log::test(tokio::test)]
async fn a_wait_with_no_reset_time_holds_with_none() {
    let registry = UsageLimitWaits::default();
    let waits = registry.session(Instant::now(), Duration::from_mins(60));

    let _waiting = waits.begin(at("2026-10-13T06:00:00Z"), None);

    assert_eq!(
        *registry.held().borrow(),
        Some(UsageLimitHeld { resets_at: None })
    );
}

/// `docs/specs/runs.md`, Usage limit, The reset time.
#[test_log::test(tokio::test)]
async fn two_sessions_waiting_hold_until_both_stop_with_the_reset_stated_last() {
    let registry = UsageLimitWaits::default();
    let held = |resets_at: &str| {
        Some(UsageLimitHeld {
            resets_at: Some(at(resets_at)),
        })
    };
    let first = registry.session(Instant::now(), Duration::from_mins(60));
    let second = registry.session(Instant::now(), Duration::from_mins(60));

    let (first_wait, _) = first.begin(at("2026-10-13T06:00:00Z"), Some(at("2026-10-13T08:00:00Z")));
    let (mut second_wait, _) =
        second.begin(at("2026-10-13T06:01:00Z"), Some(at("2026-10-13T07:00:00Z")));
    assert_eq!(*registry.held().borrow(), held("2026-10-13T07:00:00Z"));

    second_wait.restate(at("2026-10-13T06:02:00Z"), at("2026-10-13T07:30:00Z"));
    assert_eq!(*registry.held().borrow(), held("2026-10-13T07:30:00Z"));

    drop(second_wait);
    assert_eq!(*registry.held().borrow(), held("2026-10-13T08:00:00Z"));

    drop(first_wait);
    assert_eq!(*registry.held().borrow(), None);
}

#[test_log::test(tokio::test)]
async fn a_change_of_the_hold_wakes_who_watches_it() {
    let registry = UsageLimitWaits::default();
    let mut held = registry.held();
    let waits = registry.session(Instant::now(), Duration::from_mins(60));

    let (waiting, _) = waits.begin(at("2026-10-13T06:00:00Z"), None);
    held.changed().await.unwrap();
    assert!(held.borrow_and_update().is_some());

    waiting.end(at("2026-10-13T06:10:00Z"));
    held.changed().await.unwrap();
    assert!(held.borrow_and_update().is_none());
}

#[test_log::test(tokio::test)]
async fn a_wait_gives_the_transcript_lines_of_its_start_its_restatements_and_its_end() {
    let registry = UsageLimitWaits::default();
    let waits = registry.session(Instant::now(), Duration::from_mins(60));

    let (mut waiting, began) =
        waits.begin(at("2026-10-13T06:00:00Z"), Some(at("2026-10-13T07:00:00Z")));
    let restated = waiting.restate(at("2026-10-13T06:05:00Z"), at("2026-10-13T07:30:00Z"));
    let ended = waiting.end(at("2026-10-13T07:30:00Z"));

    let (_, unstated) = waits.begin(at("2026-10-13T08:00:00Z"), None);

    assert_eq!(
        [began, restated, ended, unstated]
            .map(|line| serde_json::from_str::<Value>(&line).unwrap()),
        [
            json!({"type": "linewright-usage-limit", "at": "2026-10-13T06:00:00Z", "until": "2026-10-13T07:00:00Z"}),
            json!({"type": "linewright-usage-limit", "at": "2026-10-13T06:05:00Z", "until": "2026-10-13T07:30:00Z"}),
            json!({"type": "linewright-usage-limit-ended", "at": "2026-10-13T07:30:00Z"}),
            json!({"type": "linewright-usage-limit", "at": "2026-10-13T08:00:00Z"}),
        ]
    );
}
