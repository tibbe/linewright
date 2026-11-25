//! The tools that show benchmarks (`docs/specs/access.md`, The MCP server,
//! What a tool shows): each returns what its screen shows.

use super::super::benchmark_runs::{board_run, with_benchmark_run};
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
    board_run(&pages, 1, &[], 1, "2026-11-17T12:00:00Z").await;
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
/// first, and its tasks; `docs/specs/shell.md`, No such thing.
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

/// The arguments of `get_trial` for the trial of benchmark run
/// `benchmark_run` that is `repetition` of task `task` under configuration
/// `configuration`, with `more`.
fn trial(
    benchmark_run: i64,
    configuration: i64,
    task: i64,
    repetition: i64,
    more: &Value,
) -> Value {
    let mut arguments = json!({
        "benchmark_run_id": benchmark_run,
        "configuration_id": configuration,
        "task_id": task,
        "repetition": repetition,
    });
    arguments
        .as_object_mut()
        .unwrap()
        .extend(more.as_object().unwrap().clone());
    arguments
}

/// The entries `shown` gives.
fn entries(shown: &Value) -> Vec<Value> {
    shown["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("{shown}"))
        .clone()
}

/// `docs/specs/access.md`, The MCP server, Arguments and Transcripts: a
/// trial's transcripts with their headings and sessions, and their entries as one list,
/// each with its transcript and an ID of its own among them.
#[test_log::test(tokio::test)]
async fn get_trial_returns_its_transcripts_and_their_entries_each_with_its_transcript() {
    let pages = with_benchmark_run().await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    let scored = mcp.shown("get_trial", trial(1, 1, 1, 1, &json!({}))).await;
    let waiting = mcp.shown("get_trial", trial(1, 2, 2, 2, &json!({}))).await;

    for (i, transcript) in ["run", "scorer"].into_iter().enumerate() {
        let shown = &scored["transcripts"][i];
        assert_eq!(shown["transcript"], transcript, "{scored}");
        assert_eq!(shown["id"], "1/implement-1", "{scored}");
        assert_eq!(shown["outcome"], "done", "{scored}");
        assert_eq!(shown["sessions"][0]["session"], 1, "{scored}");
    }
    let entries = entries(&scored);
    assert_eq!(
        (&entries[0]["transcript"], &entries[0]["id"]),
        (&json!("run"), &json!("run/1/1"))
    );
    let last = entries.last().unwrap();
    assert_eq!(last["transcript"], "scorer");
    assert!(
        last["id"].as_str().unwrap().starts_with("scorer/1/"),
        "{last}"
    );
    assert_eq!(last["session"], 1);
    assert_eq!(
        (&scored["left_out"], &scored["before"]),
        (&Value::Null, &Value::Null)
    );
    assert_eq!(waiting, json!({ "transcripts": [], "entries": [] }));
}

/// `docs/specs/access.md`, The MCP server, Transcripts: the latest
/// `limit`, with how many were left out and the `before` that gives the
/// ones before them, from the scorer's into the run's, in full with
/// `verbose`.
#[test_log::test(tokio::test)]
async fn get_trial_pages_its_entries_from_the_latest_across_its_transcripts() {
    let pages = with_benchmark_run().await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);
    let all = entries(&mcp.shown("get_trial", trial(1, 1, 1, 1, &json!({}))).await);
    let n = all.len();
    let scorers = all
        .iter()
        .filter(|entry| entry["transcript"] == "scorer")
        .count();
    let limit = scorers + 1 - scorers / 2;
    assert!(limit < scorers && scorers < 2 * limit, "{scorers} of {n}");

    let mut paged = vec![];
    let mut before = Value::Null;
    let mut crossed = false;
    loop {
        let page = mcp
            .shown(
                "get_trial",
                trial(1, 1, 1, 1, &json!({ "limit": limit, "before": before })),
            )
            .await;
        let shown = entries(&page);
        crossed |= shown.iter().any(|entry| entry["transcript"] == "run")
            && shown.iter().any(|entry| entry["transcript"] == "scorer");
        paged.splice(0..0, shown);
        before = page["before"].clone();
        if before.is_null() {
            assert_eq!(page["left_out"], Value::Null, "{page}");
            break;
        }
        assert_eq!(page["left_out"], n - paged.len(), "{page}");
        assert_eq!(before, paged[0]["id"], "{page}");
    }
    let verbose = mcp
        .shown("get_trial", trial(1, 1, 1, 1, &json!({ "verbose": true })))
        .await;

    assert_eq!(paged, all);
    assert!(crossed, "a page holds the run's entries and the scorer's");
    let has_input = |entries: &[Value]| entries.iter().any(|entry| entry.get("input").is_some());
    assert!(!has_input(&all));
    assert!(has_input(&entries(&verbose)), "{verbose}");
    assert_eq!(entries(&verbose).len(), n);
    assert_eq!(
        refusal(
            &mcp.call("get_trial", trial(1, 1, 1, 1, &json!({ "before": "1/1" })))
                .await
        ),
        "Nothing listed has the ID 1/1."
    );
}

/// `docs/specs/access.md`, The MCP server, A trial that does not exist.
#[test_log::test(tokio::test)]
async fn get_trial_of_a_trial_its_benchmark_run_does_not_have_is_refused() {
    let pages = with_benchmark_run().await;
    let token = token_of(&pages, Role::Viewer, "Vera Viewer").await;
    let mcp = Mcp::new(&pages, &token);

    for (arguments, refused) in [
        (trial(1, 3, 1, 1, &json!({})), "No such trial."),
        (trial(1, 1, 3, 1, &json!({})), "No such trial."),
        (trial(1, 1, 1, 3, &json!({})), "No such trial."),
        (trial(1, 1, 1, 0, &json!({})), "No such trial."),
        (trial(1, -1, 1, 1, &json!({})), "No such trial."),
        (trial(9, 1, 1, 1, &json!({})), "No such benchmark run 9"),
        (trial(9, 0, 1, 1, &json!({})), "No such benchmark run 9"),
        (trial(-1, 1, 1, 1, &json!({})), "No such benchmark run -1"),
    ] {
        assert_eq!(
            refusal(&mcp.call("get_trial", arguments.clone()).await),
            refused,
            "{arguments}"
        );
    }
}
