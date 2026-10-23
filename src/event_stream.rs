//! The response of every event stream, so that each passes through a reverse
//! proxy in front unbuffered, unchanged and uncached (docs/specs/operation.md,
//! Running: Behind a proxy).

#![allow(clippy::disallowed_types)]

use std::convert::Infallible;
use std::time::Duration;

use axum::http::{HeaderName, HeaderValue, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::Stream;

/// The longest a stream goes without sending anything: "at least every 15
/// seconds" (docs/specs/operation.md, Running: Behind a proxy).
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// Asks nginx to pass each event on as it is sent
/// (<https://nginx.org/en/docs/http/ngx_http_proxy_module.html#proxy_buffering>).
const X_ACCEL_BUFFERING: HeaderName = HeaderName::from_static("x-accel-buffering");

/// Asks every cache to keep no copy and leave the stream unchanged (RFC 9111,
/// section 5.2.2).
const NO_STORE_NO_TRANSFORM: HeaderValue = HeaderValue::from_static("no-store, no-transform");

/// An event stream's response, sending `events` and, after each quiet
/// `KEEP_ALIVE`, a comment line.
pub fn respond<S>(events: S) -> Response
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    let mut response = Sse::new(events)
        .keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
        .into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, NO_STORE_NO_TRANSFORM);
    headers.insert(X_ACCEL_BUFFERING, HeaderValue::from_static("no"));
    response
}
