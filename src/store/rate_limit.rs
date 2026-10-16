//! <https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#exceeding-the-rate-limit>,
//! <https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api#exceeding-the-rate-limit>

use std::time::{Duration, SystemTime};

use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use serde::Deserialize;

pub const RETRIES: u32 = 5;

/// The shortest wait: GitHub asks for at least a minute when it gives no
/// time, and a reset time is only as good as the clocks agree.
const MINIMUM: Duration = Duration::from_secs(60);

/// `retries` is how many times it was tried again already.
pub fn wait(
    status: StatusCode,
    headers: &HeaderMap,
    body: &str,
    retries: u32,
    now: SystemTime,
) -> Option<Duration> {
    let seconds = |name: &str| -> Option<u64> { headers.get(name)?.to_str().ok()?.parse().ok() };
    let spent = seconds("x-ratelimit-remaining") == Some(0);
    let limited = match status {
        StatusCode::TOO_MANY_REQUESTS => true,
        StatusCode::FORBIDDEN => {
            spent || headers.contains_key("retry-after") || says_rate_limit(body)
        }
        // GraphQL reports a limit as an error in a successful response.
        StatusCode::OK => graphql_rate_limited(body),
        _ => false,
    };
    if !limited {
        return None;
    }
    if let Some(after) = seconds("retry-after") {
        return Some(Duration::from_secs(after));
    }
    if spent && let Some(reset) = seconds("x-ratelimit-reset") {
        let until = SystemTime::UNIX_EPOCH + Duration::from_secs(reset);
        return Some(until.duration_since(now).unwrap_or_default().max(MINIMUM));
    }
    Some(MINIMUM * 2u32.pow(retries))
}

fn says_rate_limit(body: &str) -> bool {
    #[derive(Deserialize)]
    struct Error {
        message: String,
    }
    serde_json::from_str::<Error>(body)
        .is_ok_and(|e| e.message.to_lowercase().contains("rate limit"))
}

fn graphql_rate_limited(body: &str) -> bool {
    #[derive(Deserialize)]
    struct Response {
        #[serde(default)]
        errors: Vec<Error>,
    }
    #[derive(Deserialize)]
    struct Error {
        #[serde(rename = "type")]
        kind: Option<String>,
    }
    serde_json::from_str::<Response>(body).is_ok_and(|r| {
        r.errors
            .iter()
            .any(|e| e.kind.as_deref() == Some("RATE_LIMITED"))
    })
}
