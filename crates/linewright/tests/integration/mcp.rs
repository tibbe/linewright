//! The MCP tools beside `lw`, through the system: a tool gives the facts
//! its command gives by the same names (`docs/specs/access.md`, The MCP
//! server, What a tool shows).

use std::net::SocketAddr;

use linewright_core::access::Role;
use linewright_core::benchmarks::BenchmarkRunState;
use linewright_core::clock::Clock as _;
use linewright_core::runs::RepoWorkDir;
use linewright_core::serving::UiListener;
use linewright_core::testing::benchmark_runs::*;
use linewright_core::testing::oauth::authorized_by_back_door;
use linewright_core::testing::*;
use linewright_core::trial::Configuration;
use serde_json::{Value, json};

use crate::local_api::sources_of;
use crate::programs::*;

/// `serve` at rest over [`with_benchmark`]'s work directory once benchmark
/// run 1, of its two tasks twice under `as-is` and `short`, has ended with
/// every trial scored.
struct Rested {
    ui: UiListener,
    /// An access token of a viewer's client.
    token: String,
    serving: Serving,
    work_dir: tempfile::TempDir,
    _store: tempfile::TempDir,
}

impl Rested {
    async fn start() -> Rested {
        let (world, work_dir) = with_benchmark(2).await;
        let repo = work_dir.path().join(REPO);
        let short = tempfile::tempdir().unwrap();
        std::fs::write(short.path().join("config.toml"), "model = \"haiku\"\n").unwrap();
        let configuration = Configuration::read(short.path()).unwrap();
        let (benchmark_runs, id) =
            launch(&repo, vec![("short".parse().unwrap(), configuration)], 2).await;
        let World {
            clock,
            factory,
            dir,
            ..
        } = world;
        let (ends, winding_up) = (factory.ends(), factory.winding_up());
        let since = clock.now();
        let serving = serve(factory);
        rests(&benchmark_runs, id, |record| {
            matches!(record.state, BenchmarkRunState::Ended { at: Some(_) })
        })
        .await;
        let served = RepoWorkDir {
            base: work_dir.path().to_path_buf(),
            repo: REPO.parse().unwrap(),
        };
        let sources = sources_of(&serving, clock, since, served, &ends, &winding_up).await;
        let ui = UiListener::bind(
            &ui_on("127.0.0.1:0"),
            sources.clone(),
            linewright_web::router,
        )
        .await
        .unwrap();
        let cookie = signed_in_as(&sources, Role::Viewer, "Vera Viewer").await;
        let (_, tokens) = authorized_by_back_door(&sources, &cookie, "My agent").await;
        Rested {
            ui,
            token: tokens.access,
            serving,
            work_dir,
            _store: dir,
        }
    }

    /// What the tool `name` shows the viewer's client, called with
    /// `arguments`.
    async fn shown(&self, name: &str, arguments: Value) -> Value {
        let result = shown_by(self.ui.local_address(), &self.token, name, arguments).await;
        assert_eq!(result["isError"], false, "{name}: {result}");
        result["structuredContent"].clone()
    }

    /// What `lw` with `args` prints as JSON on the work directory.
    async fn printed(&self, args: &[&str]) -> Value {
        let printed = lw_in(self.work_dir.path(), args).await;
        assert_eq!(printed.code, Some(0), "{args:?}: {}", printed.stderr);
        serde_json::from_str(&printed.stdout)
            .unwrap_or_else(|e| panic!("{args:?}: {e}: {}", printed.stdout))
    }

    async fn stop(self) {
        self.ui.stop().await;
        self.serving.stop().await;
    }
}

/// The result of calling the tool `name` of the MCP server at `address`
/// with `arguments`, as the bearer of `token`.
/// <https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#sending-messages-to-the-server>
async fn shown_by(address: SocketAddr, token: &str, name: &str, arguments: Value) -> Value {
    let call = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let answer = reqwest::Client::new()
        .post(format!("http://{address}/mcp"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-06-18")
        .body(call.to_string())
        .send()
        .await
        .unwrap();
    let status = answer.status();
    let body = answer.text().await.unwrap();
    assert_eq!(status, 200, "{body}");
    let message: Value = serde_json::from_str(&body).unwrap_or_else(|e| panic!("{e}: {body}"));
    message["result"].clone()
}

#[test_log::test(tokio::test)]
async fn get_benchmark_and_get_task_give_their_facts_by_the_names_benchmark_show_gives() {
    let rested = Rested::start().await;

    let benchmark = rested
        .shown("get_benchmark", json!({ "benchmark_id": 1 }))
        .await;
    let task = rested.shown("get_task", json!({ "task_id": 2 })).await;
    let printed = rested.printed(&["benchmark", "show", "1", "--json"]).await;
    rested.stop().await;

    assert_eq!(benchmark["tasks"], printed["tasks"]);
    assert_eq!(
        shared(
            &benchmark["benchmark_runs"][0],
            &printed["benchmark_runs"][0]
        ),
        ["id", "launched_at", "state"]
    );
    assert_eq!(
        shared(&task, &printed["tasks"][1]),
        ["criteria", "id", "name", "run"]
    );
}

#[test_log::test(tokio::test)]
async fn get_benchmark_run_gives_its_facts_and_trials_by_the_names_benchmark_run_show_gives() {
    let rested = Rested::start().await;

    let benchmark_run = rested
        .shown("get_benchmark_run", json!({ "benchmark_run_id": 1 }))
        .await;
    let printed = rested
        .printed(&["benchmark-run", "show", "1", "--json"])
        .await;
    rested.stop().await;

    let mut facts = benchmark_run.clone();
    facts.as_object_mut().unwrap().remove("trials");
    assert_eq!(
        shared(&facts, &printed),
        [
            "benchmark",
            "configurations",
            "ended_at",
            "id",
            "launched_at",
            "repetitions",
            "results",
            "stage",
            "state",
            "tasks",
        ]
    );
    let trials = benchmark_run["trials"].as_array().unwrap();
    let printed_trials = printed["trials"].as_array().unwrap();
    assert_eq!(trials.len(), 8);
    assert_eq!(trials.len(), printed_trials.len());
    for (trial, printed_trial) in trials.iter().zip(printed_trials) {
        let mut keys = shared(trial, printed_trial);
        keys.sort();
        let mut printed_keys: Vec<String> =
            printed_trial.as_object().unwrap().keys().cloned().collect();
        printed_keys.sort();
        assert_eq!(keys, printed_keys, "{trial}");
    }
}
