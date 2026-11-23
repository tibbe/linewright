//! The MCP server (`docs/specs/access.md`, The MCP server and The
//! authorization server, No token), driven at `/mcp` through the router
//! with access tokens that the authorization server issued.

use std::panic::Location;

use serde_json::{Value, json};

use super::oauth::{Tokens, authorized_by_back_door, query_value};
use super::sign_in::{Browsing, signing};
use super::*;

mod benchmark_reads;
mod reads;

/// The protocol version these tests' client asks for.
const PROTOCOL: &str = "2025-06-18";

/// An MCP client of [`Pages`]' router, acting with an access token that
/// it sends with the `Authorization` scheme `scheme`.
struct Mcp<'a> {
    router: &'a Router,
    scheme: &'static str,
    token: String,
}

/// What `/mcp` answered: the status, the headers and the body.
struct Answered {
    status: u16,
    headers: axum::http::HeaderMap,
    body: String,
}

impl<'a> Mcp<'a> {
    fn new(pages: &'a Pages, token: &str) -> Mcp<'a> {
        Mcp {
            router: &pages.router,
            scheme: "Bearer",
            token: token.to_string(),
        }
    }

    /// Posts the JSON-RPC message `message` to `/mcp`: at [`PROTOCOL`] but
    /// for an `initialize`, which negotiates the version.
    /// <https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#protocol-version-header>
    #[track_caller]
    fn post(&self, message: &Value) -> impl Future<Output = Answered> + use<> {
        let caller = Location::caller();
        let mut request = axum::http::Request::post("/mcp")
            .header("Host", UI_HOST)
            .header("Authorization", format!("{} {}", self.scheme, self.token))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        if message["method"] != "initialize" {
            request = request.header("MCP-Protocol-Version", PROTOCOL);
        }
        let request = request
            .body(Body::from(message.to_string()))
            .unwrap_or_else(|e| panic!("building the request at {caller}: {e}"));
        let router = self.router.clone();
        async move {
            let Ok(response) = router.oneshot(request).await;
            let headers = response.headers().clone();
            let (status, body) = status_and_body(response).await;
            Answered {
                status,
                headers,
                body,
            }
        }
    }

    /// The result of the request `method` with `params`, or else its error.
    #[track_caller]
    fn request(&self, method: &str, params: Value) -> impl Future<Output = Value> + use<> {
        let caller = Location::caller();
        let mut asked = json!({ "jsonrpc": "2.0", "id": 1, "method": method });
        asked["params"] = params;
        let posted = self.post(&asked);
        async move {
            let answered = posted.await;
            assert_eq!(answered.status, 200, "at {caller}: {}", answered.body);
            let message: Value = serde_json::from_str(&answered.body)
                .unwrap_or_else(|e| panic!("at {caller}: {e}: {}", answered.body));
            match (message.get("result"), message.get("error")) {
                (Some(result), _) => result.clone(),
                (None, Some(error)) => json!({ "error": error }),
                (None, None) => panic!("at {caller}: neither a result nor an error: {message}"),
            }
        }
    }

    /// The names of the tools `tools/list` lists.
    #[track_caller]
    fn tool_names(&self) -> impl Future<Output = Vec<String>> + use<> {
        let caller = Location::caller();
        let listed = self.request("tools/list", json!({}));
        async move {
            let listed = listed.await;
            listed["tools"]
                .as_array()
                .unwrap_or_else(|| panic!("a list of tools at {caller}: {listed}"))
                .iter()
                .map(|tool| tool["name"].as_str().unwrap().to_string())
                .collect()
        }
    }

    /// What calling the tool `name` with `arguments` returned, its
    /// structured content, where it is not an error, once checked against
    /// the output schema `tools/list` gives the tool.
    #[track_caller]
    fn call(&self, name: &str, arguments: Value) -> impl Future<Output = Value> + use<> {
        let caller = Location::caller();
        let mut params = json!({ "name": name });
        params["arguments"] = arguments;
        let listed = self.request("tools/list", json!({}));
        let called = self.request("tools/call", params);
        let name = name.to_string();
        async move {
            let listed = listed.await;
            let result = called.await;
            if result["isError"] != false {
                return result;
            }
            let schema = &listed["tools"]
                .as_array()
                .and_then(|tools| tools.iter().find(|tool| tool["name"] == name))
                .unwrap_or_else(|| panic!("{name} listed at {caller}: {listed}"))["outputSchema"];
            let validator = jsonschema::validator_for(schema)
                .unwrap_or_else(|e| panic!("{name}'s output schema at {caller}: {e}: {schema}"));
            let errors: Vec<String> = validator
                .iter_errors(&result["structuredContent"])
                .map(|e| format!("{}: {e}", e.instance_path()))
                .collect();
            assert!(
                errors.is_empty(),
                "{name} at {caller} does not match its output schema: {errors:?}: {result}"
            );
            result
        }
    }

    /// What calling the tool `name` with `arguments` showed, as [`done`]
    /// gives it.
    #[track_caller]
    fn shown(&self, name: &str, arguments: Value) -> impl Future<Output = Value> + use<> {
        let called = self.call(name, arguments);
        async move { done(&called.await) }
    }
}

/// A new person of `role` named `name`, as [`signed_in_as`] adds them, and
/// the access token the authorization server issued to a client they
/// allowed.
#[track_caller]
fn token_of<'a>(pages: &'a Pages, role: Role, name: &'a str) -> impl Future<Output = String> + 'a {
    let signed_in = signed_in_as(&pages.sources, role, name);
    async move {
        let cookie = signed_in.await;
        let (_, tokens) = authorized_by_back_door(&pages.sources, &cookie, "My agent").await;
        tokens.access
    }
}

/// `docs/specs/access.md`, The authorization server, No token;
/// `docs/specs/operation.md`, The log: each request it refuses and why.
#[test_log::test(tokio::test)]
async fn a_request_without_a_valid_access_token_gets_401_pointing_to_the_resource_metadata() {
    let pages = Pages::of(a_ticket()).await;
    let metadata = r#"Bearer resource_metadata="http://127.0.0.1:8081/.well-known/oauth-protected-resource/mcp""#;
    let (log, _guard) = Log::capture();

    for (token, challenge) in [
        ("", metadata.to_string()),
        (
            "not-a-token",
            format!(r#"{metadata}, error="invalid_token""#),
        ),
    ] {
        let answered = Mcp::new(&pages, token)
            .post(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
            .await;

        assert_eq!(answered.status, 401, "{token:?}: {}", answered.body);
        assert_eq!(answered.headers["www-authenticate"], challenge, "{token:?}");
    }
    let logged = log.lines("INFO");
    for reason in ["it has no access token", "its access token is not valid"] {
        assert!(
            logged
                .iter()
                .any(|line| line.contains("refused a request to the MCP server")
                    && line.contains(reason)),
            "{reason}: {logged:?}"
        );
    }
}

/// `docs/specs/access.md`, The MCP server: the versions from 2025-06-18 on.
#[test_log::test(tokio::test)]
async fn a_client_initializes_at_2025_06_18_and_later() {
    let pages = Pages::of(a_ticket()).await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    for version in ["2025-06-18", "2025-11-25"] {
        let initialized = mcp
            .request(
                "initialize",
                json!({
                    "protocolVersion": version,
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1" },
                }),
            )
            .await;

        assert_eq!(initialized["protocolVersion"], version, "{initialized}");
        assert_eq!(initialized["serverInfo"]["name"], "linewright");
        assert!(
            initialized["capabilities"]["tools"].is_object(),
            "{initialized}"
        );
    }
    let older = mcp
        .request(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" },
            }),
        )
        .await;
    assert_ne!(older["protocolVersion"], "2025-03-26", "{older}");
}

/// The tools a viewer is listed.
const VIEWERS: [&str; 13] = [
    "get_board",
    "get_item",
    "list_runs",
    "get_run",
    "list_automations",
    "list_benchmarks",
    "get_benchmark",
    "get_task",
    "get_benchmark_run",
    "get_server",
    "list_people",
    "list_authorizations",
    "end_authorization",
];

/// The tools a contributor is listed besides a viewer's.
const CONTRIBUTORS: [&str; 8] = [
    "add_task",
    "create_benchmark",
    "edit_benchmark",
    "edit_task",
    "remove_task",
    "run_benchmark",
    "edit_configuration",
    "cancel_benchmark_run",
];

/// The tools a manager is listed besides a contributor's.
const MANAGERS: [&str; 3] = ["edit_person", "disable_person", "enable_person"];

/// `docs/specs/access.md`, The MCP server, Listed tools.
#[test_log::test(tokio::test)]
async fn the_tools_listed_are_those_the_persons_role_allows() {
    let pages = Pages::of(a_ticket()).await;

    for (role, name, expected) in [
        (Role::Viewer, "Vera Viewer", VIEWERS.to_vec()),
        (
            Role::Contributor,
            "Cody Contributor",
            [VIEWERS.as_slice(), &CONTRIBUTORS].concat(),
        ),
        (
            Role::Manager,
            "Mona Manager",
            [VIEWERS.as_slice(), &CONTRIBUTORS, &MANAGERS].concat(),
        ),
    ] {
        let token = token_of(&pages, role, name).await;

        assert_eq!(
            Mcp::new(&pages, &token).tool_names().await,
            expected,
            "{role:?}"
        );
    }
}

/// `docs/specs/access.md`, The MCP server, Tools and Arguments.
#[test_log::test(tokio::test)]
async fn each_tool_has_the_action_as_its_title_and_its_screens_arguments() {
    let pages = Pages::of(a_ticket()).await;
    let token = token_of(&pages, Role::Manager, "Mona Manager").await;

    let listed = Mcp::new(&pages, &token)
        .request("tools/list", json!({}))
        .await;

    let tool = |name: &str| {
        listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} in {listed}"))
            .clone()
    };
    let arguments = |described: &Value| -> Vec<String> {
        described["inputSchema"]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{described}"))
            .keys()
            .cloned()
            .collect()
    };
    let people = tool("list_people");
    assert_eq!(people["title"], "See the people");
    assert_eq!(people["annotations"]["readOnlyHint"], true);
    assert_eq!(arguments(&people), ["after", "limit"]);
    assert_eq!(people["outputSchema"]["type"], "object");
    let capture = tool("add_task");
    assert_eq!(capture["title"], "Capture a run as a task");
    assert_ne!(capture["annotations"]["readOnlyHint"], true);
    assert_eq!(
        arguments(&capture),
        [
            "benchmark_id",
            "benchmark_name",
            "criteria",
            "name",
            "run_id"
        ]
    );
    assert_eq!(
        arguments(&tool("run_benchmark")),
        ["benchmark_id", "configurations", "repetitions"]
    );
    assert_eq!(arguments(&tool("edit_person")), ["person_id", "role"]);
    assert_eq!(
        tool("edit_person")["inputSchema"]["properties"]["role"]["enum"],
        json!(["viewer", "contributor", "manager"])
    );
    assert_eq!(arguments(&tool("end_authorization")), ["authorization_id"]);
    assert_eq!(arguments(&tool("get_board")), ["person"]);
    assert_eq!(arguments(&tool("get_item")), ["number"]);
    assert_eq!(
        arguments(&tool("list_runs")),
        ["after", "item", "limit", "since", "stage"]
    );
    assert_eq!(
        arguments(&tool("get_run")),
        ["before", "limit", "run_id", "verbose"]
    );
    assert_eq!(arguments(&tool("list_automations")), ["after", "limit"]);
    assert_eq!(
        arguments(&tool("list_benchmarks")),
        ["after", "limit", "name", "stage"]
    );
    assert_eq!(arguments(&tool("get_benchmark")), ["benchmark_id"]);
    assert_eq!(arguments(&tool("get_task")), ["task_id"]);
    assert_eq!(arguments(&tool("get_benchmark_run")), ["benchmark_run_id"]);
    assert_eq!(
        tool("get_server")["inputSchema"],
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "additionalProperties": false,
        })
    );
    for read in VIEWERS
        .iter()
        .filter(|name| name.starts_with("get_") || name.starts_with("list_"))
    {
        let read = tool(read);
        assert_eq!(read["annotations"]["readOnlyHint"], true, "{read}");
        assert_eq!(read["outputSchema"]["type"], "object", "{read}");
    }
}

/// What a call that changed something returned, as structured content,
/// once checked to be the same JSON as its text.
#[track_caller]
fn done(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap())
        .unwrap_or_else(|e| panic!("{e}: {result}"));
    assert_eq!(text, result["structuredContent"], "{result}");
    result["structuredContent"].clone()
}

/// The keys `shown` and `answered`, both objects, have in common, each
/// checked to hold the same value in both.
#[track_caller]
fn shared(shown: &Value, answered: &Value) -> Vec<String> {
    let (shown, answered) = (shown.as_object().unwrap(), answered.as_object().unwrap());
    shown
        .iter()
        .filter_map(|(key, value)| {
            let other = answered.get(key)?;
            assert_eq!(value, other, "{key}: {shown:?} against {answered:?}");
            Some(key.clone())
        })
        .collect()
}

/// The message of a refused call.
#[track_caller]
fn refusal(result: &Value) -> String {
    assert_eq!(result["isError"], true, "{result}");
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("{result}"))
        .to_string()
}

/// `docs/specs/access.md`, The MCP server, What a tool changes; Actions,
/// Logged.
#[test_log::test(tokio::test)]
async fn a_contributor_makes_a_benchmark_and_gets_it_back_which_is_logged() {
    let pages = super::benchmarks::with_benchmarks().await;
    let token = token_of(&pages, Role::Contributor, "Cody Contributor").await;
    let (log, _guard) = Log::capture();

    let made = Mcp::new(&pages, &token)
        .call(
            "create_benchmark",
            json!({ "name": "gadgets", "stage": "design" }),
        )
        .await;

    assert_eq!(
        done(&made),
        json!({ "id": 3, "name": "gadgets", "stage": "design" })
    );
    let logged = log.lines("INFO");
    assert!(
        logged
            .iter()
            .any(|line| line.contains("Make a benchmark: done")
                && line.contains(r#"identity="google:cody@example.com""#)
                && line.contains("benchmark 3 named gadgets of design")),
        "{logged:?}"
    );
}

/// `docs/specs/access.md`, Actions, An access level that is too low; The
/// MCP server, A refused call.
#[test_log::test(tokio::test)]
async fn a_call_above_the_persons_access_level_is_refused_with_the_level_it_needs() {
    let pages = super::benchmarks::with_benchmarks().await;
    let viewer = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let contributor = token_of(&pages, Role::Contributor, "Cody Contributor").await;
    let (log, _guard) = Log::capture();

    let made = Mcp::new(&pages, &viewer)
        .call(
            "create_benchmark",
            json!({ "name": "gadgets", "stage": "design" }),
        )
        .await;
    let edited = Mcp::new(&pages, &contributor)
        .call("edit_person", json!({ "person_id": 1, "role": "viewer" }))
        .await;

    assert_eq!(refusal(&made), "This needs the act access level.");
    assert_eq!(refusal(&edited), "This needs the manage access level.");
    let logged = log.lines("INFO");
    assert!(
        logged
            .iter()
            .any(|line| line.contains("Make a benchmark: refused")
                && line.contains(r#"identity="google:vera@example.com""#)),
        "{logged:?}"
    );
    let html = pages.page("/benchmarks").await;
    assert!(!html.html().contains("gadgets"));
}

/// `docs/specs/access.md`, The MCP server, A refused call:
/// `docs/specs/benchmarks.md`, New benchmark and Edit benchmark, Refusals.
#[test_log::test(tokio::test)]
async fn a_refused_change_is_a_tool_error_with_the_message_the_ui_shows() {
    let pages = super::benchmarks::with_benchmarks().await;
    let token = token_of(&pages, Role::Contributor, "Cody Contributor").await;
    let mcp = Mcp::new(&pages, &token);

    let taken = mcp
        .call(
            "edit_benchmark",
            json!({ "benchmark_id": 1, "name": "triage-basics" }),
        )
        .await;
    let gone = mcp
        .call("edit_benchmark", json!({ "benchmark_id": 9, "name": "x" }))
        .await;
    let blank = mcp
        .call(
            "create_benchmark",
            json!({ "name": " ", "stage": "triage" }),
        )
        .await;

    assert_eq!(
        refusal(&taken),
        "A benchmark named triage-basics already exists."
    );
    assert_eq!(refusal(&gone), "No such benchmark 9");
    assert_eq!(refusal(&blank), "name: Required.");
}

/// `docs/specs/access.md`, The MCP server, What a tool changes;
/// `docs/specs/benchmarks.md`, Edit benchmark, Edit task and Remove task.
#[test_log::test(tokio::test)]
async fn a_contributor_renames_a_benchmark_changes_a_task_and_removes_one() {
    let pages = super::benchmarks::with_benchmarks().await;
    let token = token_of(&pages, Role::Contributor, "Cody Contributor").await;
    let mcp = Mcp::new(&pages, &token);

    let renamed = mcp
        .call(
            "edit_benchmark",
            json!({ "benchmark_id": 1, "name": "boards" }),
        )
        .await;
    let changed = mcp
        .call(
            "edit_task",
            json!({ "task_id": 1, "name": "cols", "criteria": "Names them.\n" }),
        )
        .await;
    let removed = mcp.call("remove_task", json!({ "task_id": 2 })).await;

    assert_eq!(
        done(&renamed),
        json!({ "id": 1, "name": "boards", "stage": "implement" })
    );
    assert_eq!(
        done(&changed),
        json!({
            "id": 1,
            "name": "cols",
            "run": "1/implement-1",
            "criteria": "Names them.\n",
            "benchmark": { "id": 1, "name": "boards", "stage": "implement" },
        })
    );
    assert_eq!(
        done(&removed),
        json!({ "benchmark": 1, "task": 2, "name": "lanes" })
    );
    assert_eq!(
        refusal(&mcp.call("remove_task", json!({ "task_id": 2 })).await),
        "No such task 2"
    );
}

/// `docs/specs/access.md`, The MCP server, What a tool changes;
/// `docs/specs/benchmarks.md`, Capturing.
#[test_log::test(tokio::test)]
async fn a_contributor_captures_a_run_into_a_benchmark_or_a_new_one() {
    use super::capture::{IMPLEMENTS, Recorded, implemented};
    let pages = implemented(Recorded::Whole).await;
    let token = token_of(&pages, Role::Contributor, "Cody Contributor").await;
    let mcp = Mcp::new(&pages, &token);

    let captured = mcp
        .call(
            "add_task",
            json!({
                "run_id": "1/implement-1",
                "benchmark_id": 1,
                "name": "gadgets",
                "criteria": "Ships gh.\n",
            }),
        )
        .await;
    let into_new = mcp
        .call(
            "add_task",
            json!({
                "run_id": "1/implement-1",
                "benchmark_name": "fresh",
                "name": "gadgets",
                "criteria": "Ships gh.\n",
            }),
        )
        .await;
    let taken = mcp
        .call(
            "add_task",
            json!({
                "run_id": "1/implement-1",
                "benchmark_id": 1,
                "name": "gadgets",
                "criteria": "Ships gh.\n",
            }),
        )
        .await;

    assert_eq!(
        done(&captured),
        json!({
            "id": 1,
            "name": "gadgets",
            "run": "1/implement-1",
            "criteria": "Ships gh.\n",
            "benchmark": { "id": 1, "name": IMPLEMENTS, "stage": "implement" },
        })
    );
    assert_eq!(
        done(&into_new)["benchmark"],
        json!({ "id": 3, "name": "fresh", "stage": "implement" })
    );
    assert_eq!(
        refusal(&taken),
        format!("A task named gadgets already exists in {IMPLEMENTS}.")
    );
}

/// `docs/specs/access.md`, The MCP server, What a tool changes;
/// `docs/specs/benchmarks.md`, New benchmark run.
#[test_log::test(tokio::test)]
async fn a_contributor_launches_a_benchmark_run_with_configurations_and_repetitions() {
    let pages = super::benchmarks::with_benchmarks().await;
    let token = token_of(&pages, Role::Contributor, "Cody Contributor").await;
    let mcp = Mcp::new(&pages, &token);

    let launched = mcp
        .call(
            "run_benchmark",
            json!({
                "benchmark_id": 1,
                "configurations": [{ "name": "short", "model": "claude-haiku-5-5" }],
                "repetitions": 2,
            }),
        )
        .await;
    let twice = mcp
        .call(
            "run_benchmark",
            json!({
                "benchmark_id": 1,
                "configurations": [
                    { "name": "short", "effort": "low" },
                    { "name": "short", "session_timeout": "soon" },
                ],
            }),
        )
        .await;
    let empty = mcp
        .call("run_benchmark", json!({ "benchmark_id": 2 }))
        .await;

    assert_eq!(
        done(&launched),
        json!({ "id": 1, "benchmark": 1, "trials": 8 })
    );
    assert_eq!(
        refusal(&twice),
        "configurations[0].name: Another configuration is named short.\n\
         configurations[1].name: Another configuration is named short.\n\
         configurations[1].session_timeout: A session timeout is a duration, such as 6h."
    );
    assert_eq!(refusal(&empty), "The benchmark triage-basics has no tasks.");
}

/// `docs/specs/access.md`, The MCP server, What a tool changes;
/// `docs/specs/benchmarks.md`, Edit configuration and Cancel benchmark run.
#[test_log::test(tokio::test)]
async fn a_contributor_renames_a_configuration_and_cancels_a_benchmark_run() {
    let pages = super::benchmarks::with_benchmarks().await;
    super::benchmark_runs::launched(&pages, "board", &["short"], 1, "2026-11-17T12:00:00Z").await;
    let token = token_of(&pages, Role::Contributor, "Cody Contributor").await;
    let mcp = Mcp::new(&pages, &token);

    let renamed = mcp
        .call(
            "edit_configuration",
            json!({ "benchmark_run_id": 1, "configuration_id": 2, "name": "long" }),
        )
        .await;
    let as_is = mcp
        .call(
            "edit_configuration",
            json!({ "benchmark_run_id": 1, "configuration_id": 1, "name": "plain" }),
        )
        .await;
    let cancelled = mcp
        .call("cancel_benchmark_run", json!({ "benchmark_run_id": 1 }))
        .await;

    assert_eq!(
        done(&renamed),
        json!({ "benchmark_run": 1, "id": 2, "name": "long" })
    );
    assert_eq!(refusal(&as_is), "The configuration as-is keeps its name.");
    let cancelled = done(&cancelled);
    assert_eq!(cancelled["id"], 1);
    assert_eq!(cancelled["state"], "cancelled");
    assert_eq!(cancelled["benchmark"], json!({ "id": 1, "name": "board" }));
    assert_eq!(cancelled["stage"], "implement");
    assert_eq!(cancelled["launched_at"], "2026-11-17T12:00:00Z");
}

/// `docs/specs/access.md`, The MCP server, Lists; People.
#[test_log::test(tokio::test)]
async fn people_are_listed_100_or_limit_at_a_time_in_the_order_of_their_names() {
    let pages = Pages::of(a_ticket()).await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    signed_in_as(&pages.sources, Role::Contributor, "Bea Builder").await;
    let mcp = Mcp::new(&pages, &token);

    let first = done(&mcp.call("list_people", json!({ "limit": 2 })).await);
    let next = done(
        &mcp.call(
            "list_people",
            json!({ "limit": 2, "after": first["after"] }),
        )
        .await,
    );
    let all = done(&mcp.call("list_people", json!({})).await);

    let names = |page: &Value| -> Vec<String> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|person| person["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names(&first), ["Ada Lovelace", "Bea Builder"]);
    assert_eq!(first["left_out"], 1);
    assert_eq!(first["after"], "3");
    assert_eq!(names(&next), ["Vera Viewer"]);
    assert_eq!(
        (&next["left_out"], &next["after"]),
        (&Value::Null, &Value::Null)
    );
    assert_eq!(all["items"].as_array().unwrap().len(), 3);
    assert_eq!(
        all["items"][0],
        json!({
            "id": 1,
            "name": "Ada Lovelace",
            "provider": "google",
            "identity": "ada@example.com",
            "role": "manager",
            "state": "enabled",
            "signed_in_at": "2026-10-01T12:00:00Z",
        })
    );
    assert_eq!(
        refusal(&mcp.call("list_people", json!({ "limt": 2 })).await),
        "The arguments do not read: unknown field `limt`, expected `limit` or `after`."
    );
}

/// `docs/specs/access.md`, People: the named manager and the last manager,
/// whose "Edit" and "Disable" say why only a manager may use them.
#[test_log::test(tokio::test)]
async fn a_manager_is_listed_why_a_person_cannot_lose_the_role() {
    let pages = Pages::of(a_ticket()).await;
    let token = token_of(&pages, Role::Manager, "Mona Manager").await;
    let mcp = Mcp::new(&pages, &token);
    done(&mcp.call("disable_person", json!({ "person_id": 1 })).await);

    let listed = done(&mcp.call("list_people", json!({})).await);

    assert_eq!(
        listed["items"][1]["protected"],
        "Mona Manager is the last manager."
    );
}

/// `docs/specs/access.md`, The MCP server, What a tool changes; Edit
/// person, Disable person and People, "Enable"; Roles, At each request.
#[test_log::test(tokio::test)]
async fn a_manager_changes_a_role_and_disables_and_enables_a_person_at_once() {
    let pages = Pages::of(a_ticket()).await;
    let manager = token_of(&pages, Role::Manager, "Mona Manager").await;
    let viewer = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &manager);

    let promoted = done(
        &mcp.call(
            "edit_person",
            json!({ "person_id": 3, "role": "contributor" }),
        )
        .await,
    );

    assert_eq!(promoted["role"], "contributor");
    assert_eq!(promoted["identity"], "vera@example.com");
    assert_eq!(
        Mcp::new(&pages, &viewer).tool_names().await,
        [VIEWERS.as_slice(), &CONTRIBUTORS].concat(),
        "a role change counts at the next call"
    );
    let disabled = done(&mcp.call("disable_person", json!({ "person_id": 3 })).await);
    assert_eq!(disabled["state"], "disabled");
    let refused = Mcp::new(&pages, &viewer)
        .post(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .await;
    assert_eq!(refused.status, 401, "{}", refused.body);
    let enabled = done(&mcp.call("enable_person", json!({ "person_id": 3 })).await);
    assert_eq!(enabled["state"], "enabled");
    assert_eq!(
        refusal(&mcp.call("disable_person", json!({ "person_id": 9 })).await),
        "No such person 9"
    );
}

/// `docs/specs/access.md`, Edit person, Refusals: a tool that disables such
/// a person gets the same message.
#[test_log::test(tokio::test)]
async fn disabling_the_last_manager_is_refused_with_edit_persons_message() {
    let pages = Pages::of(a_ticket()).await;
    let manager = token_of(&pages, Role::Manager, "Mona Manager").await;
    let mcp = Mcp::new(&pages, &manager);
    done(&mcp.call("disable_person", json!({ "person_id": 1 })).await);

    let refused = mcp.call("disable_person", json!({ "person_id": 2 })).await;

    assert_eq!(refusal(&refused), "Mona Manager is the last manager.");
}

/// `docs/specs/access.md`, Authorizations and End authorization; The
/// authorization server, How long it lasts.
#[test_log::test(tokio::test)]
async fn a_viewer_lists_their_authorizations_and_ends_one_which_stops_its_token_at_once() {
    let pages = Pages::of(a_ticket()).await;
    let cookie = signed_in_as(&pages.sources, Role::Viewer, "Vera Viewer").await;
    let (_, kept) = authorized_by_back_door(&pages.sources, &cookie, "Kept").await;
    let (_, ending) = authorized_by_back_door(&pages.sources, &cookie, "").await;
    let other = token_of(&pages, Role::Viewer, "Otto Other").await;
    let mcp = Mcp::new(&pages, &kept.access);

    let listed = done(&mcp.call("list_authorizations", json!({})).await);
    let others = mcp
        .call("end_authorization", json!({ "authorization_id": 3 }))
        .await;
    let ended = done(
        &mcp.call("end_authorization", json!({ "authorization_id": 2 }))
            .await,
    );

    let clients: Vec<(&Value, &Value)> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|authorization| (&authorization["id"], &authorization["client"]))
        .collect();
    assert_eq!(
        clients,
        [
            (&json!(2), &json!("an unnamed client")),
            (&json!(1), &json!("Kept"))
        ]
    );
    assert_eq!(listed["items"][0]["sends_back_to"], "127.0.0.1");
    assert_eq!(refusal(&others), "No such authorization 3");
    assert_eq!(ended["id"], 2);
    assert_eq!(ended["client"], "an unnamed client");
    let after = Mcp::new(&pages, &ending.access)
        .post(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .await;
    assert_eq!(after.status, 401, "{}", after.body);
    assert_eq!(Mcp::new(&pages, &other).tool_names().await, VIEWERS);
}

/// `docs/specs/access.md`, The authorization server, 1 to 6, and The MCP
/// server: a token issued at the token endpoint acts as the person who
/// signed in and allowed the client, with their role at each call.
#[test_log::test(tokio::test)]
async fn tokens_issued_through_sign_in_and_consent_list_the_tools_of_each_role() {
    let signing = signing(Some("google:boss@elsewhere.example")).await;
    let mut browser = Browsing::new(&signing);
    let client = browser.register("My agent").await;
    let mut token_of = async |email: &str| {
        let consent = browser.reach_consent(&client, "s", email).await;
        let sent = browser.decide(&consent, "allow").await;
        let code = query_value(&sent, "code").expect("a code");
        Tokens::of(&browser.exchange_code(&client, &code).await).access
    };
    let viewer = token_of("nameless@example.com").await;
    let manager = token_of("boss@elsewhere.example").await;
    let pages = &signing.pages;

    assert_eq!(Mcp::new(pages, &viewer).tool_names().await, VIEWERS);
    let managing = Mcp::new(pages, &manager);
    assert_eq!(
        managing.tool_names().await,
        [VIEWERS.as_slice(), &CONTRIBUTORS, &MANAGERS].concat()
    );
    let listed = done(&managing.call("list_people", json!({})).await);
    let nameless = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|person| person["identity"] == "nameless@example.com")
        .unwrap_or_else(|| panic!("{listed}"))["id"]
        .clone();
    done(
        &managing
            .call(
                "edit_person",
                json!({ "person_id": nameless, "role": "contributor" }),
            )
            .await,
    );
    assert_eq!(
        Mcp::new(pages, &viewer).tool_names().await,
        [VIEWERS.as_slice(), &CONTRIBUTORS].concat()
    );
}

/// `docs/specs/access.md`, The MCP server: the `Bearer` scheme whatever its
/// letter case.
/// <https://www.rfc-editor.org/rfc/rfc7235#section-2.1>
#[test_log::test(tokio::test)]
async fn an_access_token_is_taken_whatever_the_letter_case_of_its_scheme() {
    let pages = Pages::of(a_ticket()).await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;

    let lower = Mcp {
        scheme: "bearer",
        ..Mcp::new(&pages, &token)
    };

    assert_eq!(lower.tool_names().await, VIEWERS);
}
