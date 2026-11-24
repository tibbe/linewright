//! The tools that show benchmarks (`docs/specs/access.md`, The MCP server,
//! What a tool shows): each returns what its screen shows.

use super::super::benchmark_runs::{launched, with_benchmark_run};
use super::super::benchmarks::with_benchmarks;
use super::super::task::with_a_task;
use super::*;

/// Sets the launch of benchmark run 1 of `pages` to half an hour before
/// the clock's now: the launch.
async fn launched_half_an_hour_ago(pages: &Pages) -> linewright_core::domain::Timestamp {
    let launch = pages.clock.now() - chrono::TimeDelta::minutes(30);
    launched_at(
        &pages.path(""),
        1,
        &launch.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
    .await;
    launch
}

/// The seconds from `launch` up to the clock's now.
fn seconds_since(pages: &Pages, launch: linewright_core::domain::Timestamp) -> i64 {
    (pages.clock.now() - launch).num_seconds()
}

/// Checks that `duration` is a time up to now from a launch half an hour
/// before a call, `up_to_now` seconds before its end.
#[track_caller]
fn up_to_now(duration: &Value, up_to_now: i64) {
    let seconds = duration.as_i64().unwrap_or_else(|| panic!("{duration}"));
    assert!(
        (1800..=up_to_now).contains(&seconds),
        "{seconds} of {up_to_now}"
    );
}

/// `docs/specs/benchmarks.md`, Benchmarks: the benchmarks by name, narrowed
/// by the screen's filters, each with its last benchmark run;
/// `docs/specs/access.md`, The MCP server, Lists.
#[test_log::test(tokio::test)]
async fn list_benchmarks_lists_them_by_name_narrowed_and_100_or_limit_at_a_time() {
    let pages = with_benchmarks().await;
    launched(&pages, "board", &[], 1, "2026-11-17T12:00:00Z").await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let first = mcp.shown("list_benchmarks", json!({ "limit": 1 })).await;
    let next = mcp
        .shown(
            "list_benchmarks",
            json!({ "limit": 1, "after": first["after"] }),
        )
        .await;
    let by_name = mcp
        .shown("list_benchmarks", json!({ "name": " BASIC " }))
        .await;
    let by_stage = mcp
        .shown("list_benchmarks", json!({ "stage": "implement" }))
        .await;
    let neither = mcp
        .shown(
            "list_benchmarks",
            json!({ "name": "basic", "stage": "implement" }),
        )
        .await;

    assert_eq!(
        first,
        json!({
            "benchmarks": [{
                "id": 1,
                "name": "board",
                "stage": "implement",
                "tasks": 2,
                "last_benchmark_run": { "id": 1, "launched_at": "2026-11-17T12:00:00Z" },
            }],
            "left_out": 1,
            "after": "1",
        })
    );
    assert_eq!(
        next,
        json!({
            "benchmarks": [{ "id": 2, "name": "triage-basics", "stage": "triage", "tasks": 0 }],
        })
    );
    assert_eq!(by_name["benchmarks"], next["benchmarks"]);
    assert_eq!(by_stage["benchmarks"], first["benchmarks"]);
    assert_eq!(neither, json!({ "benchmarks": [] }));
    assert_eq!(
        refusal(&mcp.call("list_benchmarks", json!({ "after": "9" })).await),
        "Nothing listed has the ID 9."
    );
}

/// `docs/specs/benchmarks.md`, Benchmark: its benchmark runs, latest
/// first, and its tasks; `docs/specs/ui_patterns.md`, Screen states,
/// Nothing there.
#[test_log::test(tokio::test)]
async fn get_benchmark_returns_its_benchmark_runs_and_tasks() {
    let pages = with_benchmark_run().await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let launch = launched_half_an_hour_ago(&pages).await;
    let benchmark = mcp
        .shown("get_benchmark", json!({ "benchmark_id": 1 }))
        .await;
    let ended = seconds_since(&pages, launch);

    assert_eq!(
        (&benchmark["id"], &benchmark["name"], &benchmark["stage"]),
        (&json!(1), &json!("board"), &json!("implement"))
    );
    assert_eq!(
        benchmark["tasks"][0],
        json!({
            "id": 1,
            "name": "columns",
            "run": "1/implement-1",
            "criteria": "Names the columns.\nAnd orders them.\n",
        })
    );
    let row = &benchmark["benchmark_runs"][0];
    assert_eq!(
        (&row["state"], &row["tasks"], &row["configurations"]),
        (&json!("waiting"), &json!(2), &json!(2))
    );
    assert_eq!(row["cost"], json!({ "total": 1.61, "none": 1 }));
    up_to_now(&row["duration_seconds"], ended);
    assert_eq!(
        refusal(
            &mcp.call("get_benchmark", json!({ "benchmark_id": 9 }))
                .await
        ),
        "No such benchmark 9"
    );
}

/// `docs/specs/benchmarks.md`, Task: its heading, its facts and its
/// criteria, and why its facts do not read where they do not.
#[test_log::test(tokio::test)]
async fn get_task_returns_its_facts_and_criteria_or_why_they_do_not_read() {
    let pages = with_a_task().await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let task = mcp.shown("get_task", json!({ "task_id": 3 })).await;
    let without_facts = mcp.shown("get_task", json!({ "task_id": 1 })).await;

    assert_eq!(
        task,
        json!({
            "id": 3,
            "name": "gadgets",
            "run": "1130/implement-1",
            "criteria": "Ships the gadgets.\n\nAnd tests them.\n",
            "benchmark": { "id": 1, "name": "board", "stage": "implement" },
            "item": 1130,
            "title": "Spec change: Show no working line between turns",
            "started_at": "2026-10-13T10:00:00Z",
        })
    );
    assert_eq!(without_facts["name"], "columns");
    let why = without_facts["facts_unreadable"].as_str().unwrap();
    let page = pages.page("/tasks/1").await;
    assert!(
        text(select(&page, "#task")[0]).contains(&format!("Item and Started do not read: {why}")),
        "{without_facts}"
    );
    assert_eq!(
        refusal(&mcp.call("get_task", json!({ "task_id": 9 })).await),
        "No such task 9"
    );
}

/// `docs/specs/benchmarks.md`, Benchmark run: its facts, its results and
/// each trial with its status, and its score's reasoning or its reason for
/// none.
#[test_log::test(tokio::test)]
async fn get_benchmark_run_returns_its_results_and_trials() {
    let pages = with_benchmark_run().await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let launch = launched_half_an_hour_ago(&pages).await;
    let benchmark_run = mcp
        .shown("get_benchmark_run", json!({ "benchmark_run_id": 1 }))
        .await;
    let ended = seconds_since(&pages, launch);

    let trials = benchmark_run["trials"].as_array().unwrap();
    let trial = |repetition: u64, task: u64, configuration: u64| {
        trials
            .iter()
            .find(|trial| {
                trial["repetition"] == repetition
                    && trial["task"] == task
                    && trial["configuration"] == configuration
            })
            .unwrap_or_else(|| panic!("trial {repetition} of {task} under {configuration}"))
    };
    let passed = trial(1, 1, 1);
    assert_eq!(
        (
            &passed["status"],
            &passed["correctness_reasoning"],
            &passed["leak_reasoning"]
        ),
        (
            &json!("pass"),
            &json!("It names the columns."),
            &json!("I read every transcript.")
        )
    );
    assert_eq!(passed.get("no_score"), None);
    let run_failed = trial(1, 2, 1);
    assert_eq!(
        (&run_failed["status"], &run_failed["no_score"]),
        (
            &json!("no score"),
            &json!("No score. The run's outcome: failed")
        )
    );
    assert_eq!(trial(2, 2, 1)["status"], "running");
    assert_eq!(trial(2, 2, 2)["status"], "waiting");
    up_to_now(&benchmark_run["duration_seconds"], ended);
    assert_eq!(
        refusal(
            &mcp.call("get_benchmark_run", json!({ "benchmark_run_id": 9 }))
                .await
        ),
        "No such benchmark run 9"
    );
}

/// The arguments of `get_trial` for repetition `repetition` of task `task`
/// under configuration `configuration` of benchmark run 1, with `more`.
fn trial_of(configuration: i64, task: i64, repetition: i64, more: Value) -> Value {
    let mut arguments = json!({
        "benchmark_run_id": 1,
        "configuration_id": configuration,
        "task_id": task,
        "repetition": repetition,
    });
    let Value::Object(more) = more else {
        panic!("more arguments are an object: {more}");
    };
    arguments.as_object_mut().unwrap().extend(more);
    arguments
}

/// The IDs of the entries `shown` gives.
fn entry_ids(shown: &Value) -> Vec<String> {
    shown["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("{shown}"))
        .iter()
        .map(|entry| entry["id"].as_str().unwrap().to_string())
        .collect()
}

/// The IDs of the entries of the transcript of the fixture's first turn,
/// `linewright_core::testing::TURN_1`, as Run shows it: its brief, then
/// each tool call, thought and message, then how long it worked for.
const TURN_1_ENTRIES: [&str; 13] = [
    "1/1",
    "1/1/2",
    "1/1/3",
    "1/1/4",
    "1/1/5",
    "1/1/7",
    "1/1/8",
    "1/1/9",
    "1/1/10",
    "1/1/10/2",
    "1/1/13",
    "1/1/14",
    "1/1/worked-for",
];

/// The IDs `get_trial` gives the entries of a trial whose run and scorer
/// each have the transcript of the fixture's first turn: the run's, then
/// the scorer's.
fn trial_entry_ids() -> Vec<String> {
    ["run", "scorer"]
        .into_iter()
        .flat_map(|transcript| {
            TURN_1_ENTRIES
                .iter()
                .map(move |id| format!("{transcript}/{id}"))
        })
        .collect()
}

/// `docs/specs/benchmarks.md`, Benchmark run, A trial's transcripts: the
/// run's, then the scorer's; `docs/specs/access.md`, The MCP server,
/// Transcripts: one list of entries, the latest 100 or the latest `limit`,
/// with how many were left out and the `before` that gives the ones before
/// them, in full with `verbose`.
#[test_log::test(tokio::test)]
async fn get_trial_returns_the_run_then_the_scorer_and_their_latest_entries_as_one_list() {
    let pages = with_benchmark_run().await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let all = mcp.shown("get_trial", trial_of(1, 1, 1, json!({}))).await;
    let latest = mcp
        .shown("get_trial", trial_of(1, 1, 1, json!({ "limit": 14 })))
        .await;
    let before = mcp
        .shown(
            "get_trial",
            trial_of(1, 1, 1, json!({ "before": "run/1/1/worked-for" })),
        )
        .await;
    let verbose = mcp
        .shown("get_trial", trial_of(1, 1, 1, json!({ "verbose": true })))
        .await;

    for transcript in ["run", "scorer"] {
        assert_eq!(all[transcript]["id"], "1/implement-1", "{all}");
        assert_eq!(all[transcript]["outcome"], "done", "{all}");
        assert_eq!(all[transcript]["sessions"][0]["session"], 1, "{all}");
    }
    let ids = trial_entry_ids();
    assert_eq!(entry_ids(&all), ids);
    let transcripts: Vec<&Value> = all["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["transcript"])
        .collect();
    assert_eq!(
        transcripts,
        [[&json!("run"); 13], [&json!("scorer"); 13]].concat()
    );
    assert_eq!(
        (&all["left_out"], &all["before"]),
        (&Value::Null, &Value::Null)
    );
    assert_eq!(entry_ids(&latest), ids[12..]);
    assert_eq!(
        (&latest["left_out"], &latest["before"]),
        (&json!(12), &json!("run/1/1/worked-for"))
    );
    assert_eq!(entry_ids(&before), ids[..12]);
    assert_eq!(
        (&before["left_out"], &before["before"]),
        (&Value::Null, &Value::Null)
    );
    let texts = |shown: &Value| -> Vec<Value> {
        shown["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["type"] == "brief")
            .map(|entry| entry["text"].clone())
            .collect()
    };
    assert_eq!(texts(&all), [Value::Null, Value::Null]);
    assert_eq!(
        texts(&verbose),
        [
            json!("# Your task\n\nImplement the ticket below.\n"),
            json!("# Your task\n\nScore the trial.\n"),
        ]
    );
}

/// `docs/specs/access.md`, The MCP server, A trial that does not exist;
/// `docs/specs/benchmarks.md`, Benchmark run, A trial's transcripts: each
/// once it has started.
#[test_log::test(tokio::test)]
async fn get_trial_refuses_a_trial_its_benchmark_run_lacks_and_shows_none_not_begun() {
    let pages = with_benchmark_run().await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let not_begun = mcp.shown("get_trial", trial_of(2, 2, 2, json!({}))).await;
    let mut refused = Vec::new();
    for arguments in [
        trial_of(3, 1, 1, json!({})),
        trial_of(1, 3, 1, json!({})),
        trial_of(1, 1, 3, json!({})),
        trial_of(-1, 1, 1, json!({})),
        trial_of(1, -1, 1, json!({})),
    ] {
        refused.push(refusal(&mcp.call("get_trial", arguments).await));
    }
    let mut no_benchmark_run = Vec::new();
    for configuration in [1, -1] {
        let arguments = json!({
            "benchmark_run_id": 9,
            "configuration_id": configuration,
            "task_id": 1,
            "repetition": 1,
        });
        no_benchmark_run.push(refusal(&mcp.call("get_trial", arguments).await));
    }
    let repetition_0 = mcp.call("get_trial", trial_of(1, 1, 0, json!({}))).await;

    assert_eq!(not_begun, json!({ "entries": [] }));
    assert_eq!(refused, ["No such trial."; 5]);
    assert_eq!(no_benchmark_run, ["No such benchmark run 9"; 2]);
    assert_eq!(
        refusal(&repetition_0),
        "The argument repetition is not valid."
    );
}
