use std::time::{Duration, SystemTime};

use linewright::store::rate_limit::wait;
use reqwest::StatusCode;
use reqwest::header::HeaderMap;

const NOW: u64 = 1_800_000_000;

fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.insert(*name, value.parse().unwrap());
    }
    headers
}

fn waits(status: StatusCode, headers: &HeaderMap, body: &str, retries: u32) -> Option<u64> {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(NOW);
    wait(status, headers, body, retries, now).map(|d| d.as_secs())
}

const GRAPHQL_LIMITED: &str =
    r#"{"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded for user ID 1."}]}"#;

#[test]
fn a_spent_limit_waits_until_it_resets() {
    let spent = headers(&[
        ("x-ratelimit-remaining", "0"),
        ("x-ratelimit-reset", &(NOW + 600).to_string()),
    ]);

    assert_eq!(waits(StatusCode::OK, &spent, GRAPHQL_LIMITED, 0), Some(600));
    assert_eq!(waits(StatusCode::FORBIDDEN, &spent, "{}", 0), Some(600));
}

#[test]
fn a_reset_the_clock_has_passed_still_waits_a_minute() {
    let early = headers(&[
        ("x-ratelimit-remaining", "0"),
        ("x-ratelimit-reset", &(NOW - 5).to_string()),
    ]);

    assert_eq!(waits(StatusCode::OK, &early, GRAPHQL_LIMITED, 0), Some(60));
}

#[test]
fn retry_after_is_waited_out_as_given() {
    let after = headers(&[("retry-after", "30")]);

    assert_eq!(waits(StatusCode::FORBIDDEN, &after, "{}", 0), Some(30));
    assert_eq!(
        waits(StatusCode::TOO_MANY_REQUESTS, &after, "", 3),
        Some(30)
    );
}

#[test]
fn a_secondary_limit_without_a_time_waits_a_minute_doubling_with_every_retry() {
    let body = r#"{"message":"You have exceeded a secondary rate limit. Please wait a few minutes before you try again."}"#;
    let none = headers(&[("x-ratelimit-remaining", "4000")]);

    assert_eq!(waits(StatusCode::FORBIDDEN, &none, body, 0), Some(60));
    assert_eq!(waits(StatusCode::FORBIDDEN, &none, body, 2), Some(240));
    assert_eq!(waits(StatusCode::OK, &none, GRAPHQL_LIMITED, 1), Some(120));
}

#[test]
fn other_refusals_and_successes_are_not_rate_limits() {
    let none = headers(&[("x-ratelimit-remaining", "4000")]);
    let forbidden = r#"{"message":"Resource not accessible by personal access token"}"#;
    let data_mentioning_it = r#"{"data":{"repository":{"issue":{"body":"RATE_LIMITED errors"}}}}"#;

    assert_eq!(waits(StatusCode::FORBIDDEN, &none, forbidden, 0), None);
    assert_eq!(waits(StatusCode::OK, &none, data_mentioning_it, 0), None);
    assert_eq!(waits(StatusCode::OK, &none, "[]", 0), None);
    assert_eq!(waits(StatusCode::NOT_FOUND, &none, "{}", 0), None);
}
