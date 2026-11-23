use std::time::Duration;

use linewright_core::harness::{Deadline, UsageLimit};
use linewright_core::testing::*;
use rstest::rstest;

#[rstest]
#[case::a_reset_to_come_is_waited_for("2026-10-13T07:50:00Z", "2026-10-13T07:50:00Z")]
#[case::a_reset_that_has_passed_is_waited_out_for_a_minute(
    "2026-10-13T05:00:00Z",
    "2026-10-13T06:01:00Z"
)]
#[case::a_reset_now_is_waited_out_for_a_minute("2026-10-13T06:00:00Z", "2026-10-13T06:01:00Z")]
fn a_refused_session_waits_until(#[case] resets_at: &str, #[case] until: &str) {
    let limit = UsageLimit {
        resets_at: at(resets_at),
    };

    assert_eq!(limit.wait_until(at("2026-10-13T06:00:00Z")), at(until));
}

#[rstest]
#[case::the_time_left_at_the_refusal_is_carried_over(30, 20, 10)]
#[case::a_deadline_passed_at_the_refusal_leaves_no_time(30, 40, 0)]
fn a_session_resumed_after_waiting_out_the_usage_limit_has(
    #[case] deadline_minute: u64,
    #[case] refused_minute: u64,
    #[case] left_minutes: u64,
) {
    let start = tokio::time::Instant::now();
    let minute = |n: u64| start + Duration::from_mins(n);
    let woke_at = minute(120);

    let set = Deadline::after(start, Duration::from_mins(deadline_minute));

    let deadline = UsageLimit::deadline_after_wait(set, minute(refused_minute), woke_at);

    assert_eq!(deadline.at() - woke_at, Duration::from_mins(left_minutes));
}

#[test_log::test]
fn the_time_a_session_timeout_counts_leaves_out_the_waits_for_the_usage_limit() {
    let start = tokio::time::Instant::now();
    let minute = |n: u64| start + Duration::from_mins(n);
    let set = Deadline::after(start, Duration::from_mins(60));

    let deadline = UsageLimit::deadline_after_wait(set, minute(20), minute(120));

    assert_eq!(
        [set.counted(minute(10)), deadline.counted(minute(130))],
        [Duration::from_mins(10), Duration::from_mins(30)]
    );
}
