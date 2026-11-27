use linewright_core::harness::UsageLimit;
use linewright_core::testing::*;
use rstest::rstest;

#[rstest]
#[case::a_reset_to_come_is_waited_for("2026-10-13T07:50:00Z", "2026-10-13T07:50:00Z")]
#[case::a_reset_that_has_passed_is_waited_out_for_a_minute(
    "2026-10-13T05:00:00Z",
    "2026-10-13T06:01:00Z"
)]
#[case::a_reset_now_is_waited_out_for_a_minute("2026-10-13T06:00:00Z", "2026-10-13T06:01:00Z")]
#[test_log::test]
fn a_refused_session_waits_until(#[case] resets_at: &str, #[case] until: &str) {
    let limit = UsageLimit {
        resets_at: at(resets_at),
    };

    assert_eq!(limit.wait_until(at("2026-10-13T06:00:00Z")), at(until));
}
