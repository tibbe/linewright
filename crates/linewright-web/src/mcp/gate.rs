//! What only an HTTP status can say of a tool call (`docs/specs/access.md`,
//! The MCP server, Too many calls and An access level that is too low),
//! said before rmcp reads the call, since a `ServerHandler` sets no status.
//! <https://docs.rs/rmcp/3.11.0/rmcp/handler/server/trait.ServerHandler.html>

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::TimeDelta;
use http_body_util::LengthLimitError;
use serde_json::{Value, json};
use tracing::info;

use linewright_core::access::Caller;
use linewright_core::access::people::PersonId;
use linewright_core::clock::Clock;
use linewright_core::domain::Timestamp;
use linewright_core::host::PublicUrl;

use super::tools;

/// How many tools a person's clients can call in a window.
const CALLS: u32 = 120;

/// How long a window lasts from a person's first call in it.
const WINDOW: TimeDelta = TimeDelta::minutes(1);

/// The JSON-RPC error code of a call that the gate refuses: one of those
/// reserved for a server's own errors.
/// <https://www.jsonrpc.org/specification#error_object>
const REFUSED: i64 = -32000;

/// What the gate of each request to the MCP server holds.
#[derive(Clone)]
pub(super) struct Gate {
    clock: Arc<dyn Clock>,
    limit: Arc<CallLimit>,
    public_url: PublicUrl,
    max_body: usize,
}

impl Gate {
    /// The gate of the MCP server at `public_url`, counting on `clock`,
    /// with no call counted yet. It reads at most `max_body` bytes of a
    /// body, the most that rmcp reads.
    /// <https://docs.rs/rmcp/3.11.0/rmcp/transport/streamable_http_server/struct.StreamableHttpServerConfig.html#structfield.max_request_body_bytes>
    pub(super) fn new(clock: Arc<dyn Clock>, public_url: PublicUrl, max_body: usize) -> Gate {
        Gate {
            clock,
            limit: Arc::default(),
            public_url,
            max_body,
        }
    }

    /// Whether rmcp takes a request with `headers` as far as its `Origin`
    /// goes: one with none, or with the public URL's.
    fn takes_origin(&self, headers: &HeaderMap) -> bool {
        headers.get(header::ORIGIN).is_none_or(|origin| {
            origin
                .to_str()
                .is_ok_and(|origin| self.public_url.is_origin(origin))
        })
    }
}

/// Answers a `tools/call` by a person whose clients have called tools
/// [`CALLS`] times in the window with 429, and then one to a tool their role
/// does not allow with 403, after counting it, and passes on every other
/// request as it came: an unknown tool's call, and one from an `Origin` that
/// rmcp refuses, included. A body longer than `max_body` bytes gets 413, and
/// one that does not read 400.
pub(super) async fn gate(State(gate): State<Gate>, request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let body = match axum::body::to_bytes(body, gate.max_body).await {
        Ok(body) => body,
        Err(e) if e.source().is_some_and(<dyn Error>::is::<LengthLimitError>) => {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    };
    if gate.takes_origin(&parts.headers)
        && let (Some(call), Some(caller)) = (ToolCall::of(&body), parts.extensions.get::<Caller>())
        && let Some(response) = refusal(&gate, &call, caller)
    {
        return response;
    }
    next.run(Request::from_parts(parts, Body::from(body))).await
}

/// The answer to `call` by `caller` where the gate refuses it.
fn refusal(gate: &Gate, call: &ToolCall, caller: &Caller) -> Option<Response> {
    let person = caller.person();
    if let Err(retry_after) = gate.limit.count(person.id, gate.clock.now()) {
        info!(
            person = %person.id,
            identity = person.logged(),
            tool = call.name,
            retry_after,
            "refused a tool call over the limit"
        );
        let body = error(&call.id, "Too many tool calls in a minute.");
        return Some(
            (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, retry_after.to_string())],
                body,
            )
                .into_response(),
        );
    }
    let needs = tools::needs_level(&call.name, call.arguments.clone(), caller)?;
    Some((StatusCode::FORBIDDEN, error(&call.id, &needs.to_string())).into_response())
}

/// The JSON-RPC error with `message` that answers the request `id`.
fn error(id: &Value, message: &str) -> impl IntoResponse + use<> {
    (
        [(header::CONTENT_TYPE, "application/json")],
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": REFUSED, "message": message },
        })
        .to_string(),
    )
}

/// A `tools/call` request, as far as the gate reads it.
/// <https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools>,
/// <https://www.jsonrpc.org/specification#request_object>
struct ToolCall {
    /// A number or a string.
    id: Value,
    name: String,
    arguments: Option<serde_json::Map<String, Value>>,
}

impl ToolCall {
    /// The `tools/call` request `body` holds, `None` where it holds another
    /// message or none.
    fn of(body: &Bytes) -> Option<ToolCall> {
        let message: Value = serde_json::from_slice(body).ok()?;
        if message.get("jsonrpc")? != "2.0" || message.get("method")? != "tools/call" {
            return None;
        }
        let id = message.get("id")?;
        if !(id.is_i64() || id.is_u64() || id.is_string()) {
            return None;
        }
        let params = message.get("params")?;
        Some(ToolCall {
            id: id.clone(),
            name: params.get("name")?.as_str()?.to_string(),
            arguments: params.get("arguments").and_then(Value::as_object).cloned(),
        })
    }
}

/// How many tools each person's clients called in their window, which opens
/// at their first call and lasts [`WINDOW`].
#[derive(Default)]
pub(super) struct CallLimit {
    windows: Mutex<BTreeMap<PersonId, Window>>,
}

/// A person's window: when it opened, and how many calls it has counted.
#[derive(Clone, Copy)]
struct Window {
    opened: Timestamp,
    calls: u32,
}

impl CallLimit {
    /// Counts a call by `person` at `now`, or else gives the seconds to the
    /// end of their window, rounded up, where it has counted [`CALLS`]
    /// already. It forgets each window that has closed by `now`.
    fn count(&self, person: PersonId, now: Timestamp) -> Result<(), i64> {
        let mut windows = self.windows.lock().expect("no count panics");
        windows.retain(|_, window| now < window.opened + WINDOW);
        let window = windows.entry(person).or_insert(Window {
            opened: now,
            calls: 0,
        });
        if window.calls < CALLS {
            window.calls += 1;
            return Ok(());
        }
        let left = window.opened + WINDOW - now;
        let whole = left.num_seconds();
        Err(if left > TimeDelta::seconds(whole) {
            whole + 1
        } else {
            whole
        })
    }
}
