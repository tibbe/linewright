use crate::common;

use std::path::{Path, PathBuf};

use common::*;
use serde_json::json;

/// The work directory of `REPO` under `work_dir`, its database made, with
/// the design benchmark `board` holding the tasks `columns` and `lanes`.
#[track_caller]
fn board(work_dir: &Path) -> impl Future<Output = PathBuf> + use<> {
    let repo = work_dir.join(REPO);
    let runs = Box::pin(runs_in(&repo));
    let benchmark = Box::pin(keep_benchmark(&repo, "board", "design"));
    let lanes = Box::pin(keep_task(
        &repo,
        "board",
        "lanes",
        "4/design-1",
        "Stacks them.\n",
    ));
    let columns = Box::pin(keep_task(
        &repo,
        "board",
        "columns",
        "3/design-2",
        "Names them.\n",
    ));
    let git = repo.join("tasks/columns/commits.git");
    std::fs::create_dir_all(git.join("objects")).unwrap();
    std::fs::write(git.join("HEAD"), "ref\n").unwrap();
    async move {
        runs.await;
        benchmark.await;
        lanes.await;
        columns.await;
        repo
    }
}

/// A configuration in a directory of `dir`, setting the model.
#[track_caller]
fn short(dir: &Path) -> PathBuf {
    let configuration = dir.join("short");
    std::fs::create_dir_all(configuration.join("briefs/prompts")).unwrap();
    std::fs::write(configuration.join("config.toml"), "model = \"haiku\"\n").unwrap();
    configuration
}

#[track_caller]
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_run_launch_keeps_a_copy_of_each_task_and_configuration() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    let configurations = tempfile::tempdir().unwrap();
    let short = short(configurations.path());

    let output = lw_in(
        work_dir.path(),
        &[
            "experiment",
            "launch",
            "board",
            "--configuration",
            &format!("short={}", short.display()),
            "--repetitions",
            "2",
        ],
    )
    .await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        output.stdout,
        "Launched experiment 1 on the suite board: 8 replays\n"
    );
    let kept = repo.join("benchmark-runs/1/.kept");
    assert_eq!(names_in(&repo.join("benchmark-runs")), ["1"]);
    assert_eq!(names_in(&kept.join("tasks")), ["columns", "lanes"]);
    assert_eq!(
        std::fs::read_to_string(kept.join("tasks/columns/criteria.md")).unwrap(),
        "Names them.\n"
    );
    assert_eq!(
        std::fs::read_to_string(kept.join("tasks/columns/commits.git/HEAD")).unwrap(),
        "ref\n"
    );
    assert_eq!(names_in(&kept.join("configurations")), ["as-is", "short"]);
    assert_eq!(
        names_in(&kept.join("configurations/as-is")),
        Vec::<String>::new()
    );
    assert_eq!(
        std::fs::read_to_string(kept.join("configurations/short/config.toml")).unwrap(),
        "model = \"haiku\"\n"
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_run_launch_takes_an_id_past_a_directory_a_stopped_launch_left() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    std::fs::create_dir_all(repo.join("benchmark-runs/1/.kept")).unwrap();

    let output = lw_in(work_dir.path(), &["experiment", "launch", "board"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        output.stdout,
        "Launched experiment 2 on the suite board: 2 replays\n"
    );
    assert_eq!(
        names_in(&repo.join("benchmark-runs/2/.kept/tasks")),
        ["columns", "lanes"]
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_run_launch_with_no_configuration_runs_as_is_once() {
    let work_dir = tempfile::tempdir().unwrap();
    board(work_dir.path()).await;

    let output = lw_in(
        work_dir.path(),
        &["experiment", "launch", "board", "--json"],
    )
    .await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output.stdout).unwrap(),
        json!({"id": 1, "suite": "board", "replays": 2})
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_run_launch_refuses_writing_nothing() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    keep_benchmark(&repo, "empty", "design").await;
    let configurations = tempfile::tempdir().unwrap();
    let short = short(configurations.path());
    let broken = configurations.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("config.toml"), "max-runs = 9\n").unwrap();
    let short_named = |name: &str| format!("{name}={}", short.display());

    let no_benchmark = lw_in(
        work_dir.path(),
        &[
            "experiment",
            "launch",
            "lanes",
            "--configuration",
            &format!("broken={}", broken.display()),
        ],
    )
    .await;
    let no_tasks = lw_in(work_dir.path(), &["experiment", "launch", "empty"]).await;
    let with_an_error = lw_in(
        work_dir.path(),
        &[
            "experiment",
            "launch",
            "board",
            "--configuration",
            &format!("broken={}", broken.display()),
        ],
    )
    .await;
    let twice = lw_in(
        work_dir.path(),
        &[
            "experiment",
            "launch",
            "board",
            "--configuration",
            &short_named("short"),
            "--configuration",
            &short_named("short"),
        ],
    )
    .await;
    let as_is = lw_in(
        work_dir.path(),
        &[
            "experiment",
            "launch",
            "board",
            "--configuration",
            &short_named("as-is"),
        ],
    )
    .await;

    assert_eq!(no_benchmark.code, Some(4), "{}", no_benchmark.stderr);
    assert_eq!(no_benchmark.stderr, "lw: there is no suite lanes\n");
    assert_eq!(no_tasks.code, Some(1), "{}", no_tasks.stderr);
    assert_eq!(no_tasks.stderr, "lw: the suite empty has no cases\n");
    assert_eq!(with_an_error.code, Some(1), "{}", with_an_error.stderr);
    assert!(
        with_an_error.stderr.contains("cannot be used")
            && with_an_error.stderr.contains("config.toml"),
        "{}",
        with_an_error.stderr
    );
    assert_eq!(twice.code, Some(1), "{}", twice.stderr);
    assert_eq!(twice.stderr, "lw: two configurations are named short\n");
    assert_eq!(as_is.code, Some(1), "{}", as_is.stderr);
    assert_eq!(as_is.stderr, "lw: two configurations are named as-is\n");
    let benchmark_runs = repo.join("benchmark-runs");
    assert!(!benchmark_runs.exists() || names_in(&benchmark_runs).is_empty());
    let shown = lw_in(work_dir.path(), &["suite", "show", "board"]).await;
    assert!(
        shown.stdout.ends_with("experiments: none\n"),
        "{}",
        shown.stdout
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_run_launch_of_a_wrong_configuration_or_repetitions_is_a_command_line_error() {
    let work_dir = tempfile::tempdir().unwrap();
    board(work_dir.path()).await;

    let no_dir = lw_in(
        work_dir.path(),
        &["experiment", "launch", "board", "--configuration", "short"],
    )
    .await;
    let named = lw_in(
        work_dir.path(),
        &[
            "experiment",
            "launch",
            "board",
            "--configuration",
            "Short=x",
        ],
    )
    .await;
    let none = lw_in(
        work_dir.path(),
        &["experiment", "launch", "board", "--repetitions", "0"],
    )
    .await;

    for output in [no_dir, named, none] {
        assert_eq!(output.code, Some(2), "{}", output.stderr);
    }
}

#[test_log::test(tokio::test)]
async fn a_benchmark_run_keeps_its_tasks_as_they_were_at_its_launch() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    let launched = lw_in(work_dir.path(), &["experiment", "launch", "board"]).await;
    assert_eq!(launched.code, Some(0), "{}", launched.stderr);

    std::fs::write(repo.join("tasks/lanes/criteria.md"), "Hides them.\n").unwrap();
    let removed = lw_in(
        work_dir.path(),
        &["suite", "remove-case", "board", "columns"],
    )
    .await;
    assert_eq!(removed.code, Some(0), "{}", removed.stderr);

    let kept = repo.join("benchmark-runs/1/.kept/tasks");
    assert_eq!(names_in(&kept), ["columns", "lanes"]);
    assert_eq!(
        std::fs::read_to_string(kept.join("lanes/criteria.md")).unwrap(),
        "Stacks them.\n"
    );
    assert_eq!(
        std::fs::read_to_string(kept.join("columns/criteria.md")).unwrap(),
        "Names them.\n"
    );
    let shown = lw_in(work_dir.path(), &["experiment", "show", "1", "--json"]).await;
    assert_eq!(shown.code, Some(0), "{}", shown.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&shown.stdout).unwrap()["cases"],
        json!(["lanes", "columns"])
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_run_show_shows_its_state_its_results_and_each_trial() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    let configurations = tempfile::tempdir().unwrap();
    let short = short(configurations.path());
    let launched = lw_in(
        work_dir.path(),
        &[
            "experiment",
            "launch",
            "board",
            "--configuration",
            &format!("short={}", short.display()),
        ],
    )
    .await;
    assert_eq!(launched.code, Some(0), "{}", launched.stderr);
    launched_at(&repo, 1, "2026-11-17T12:00:00Z").await;
    trial(
        &repo,
        1,
        (1, 0, 0),
        "state = 'scored', outcome = 'done', money = 0.3, seconds = 90, \
         correctness = 'pass', leak = 0, scorer_money = 0.05",
    )
    .await;
    trial(
        &repo,
        1,
        (1, 0, 1),
        "state = 'unscored', outcome = 'failed', seconds = 30, unscored = 'outcome'",
    )
    .await;
    trial(&repo, 1, (1, 1, 0), "state = 'running'").await;

    let text = lw_in(work_dir.path(), &["experiment", "show", "1"]).await;
    let json = lw_in(work_dir.path(), &["experiment", "show", "1", "--json"]).await;

    assert_eq!(text.code, Some(0), "{}", text.stderr);
    for line in [
        "launched: 2026-11-17 12:00:00 UTC",
        "state: waiting for serve",
        "replays ended: 2 of 4",
        "replay as-is/lanes/1: judged pass, run done, cost $0.30, took 1m 30s, judge's cost $0.05",
        "replay short/columns/1: pending",
    ] {
        assert!(
            text.stdout.lines().any(|shown| shown == line),
            "{line:?} is not in {}",
            text.stdout
        );
    }
    assert_eq!(json.code, Some(0), "{}", json.stderr);
    let json: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    assert_eq!(json["state"], "waiting");
    assert_eq!(json["waiting_for"], "serve");
    assert_eq!(json.get("ended_at"), None);
    assert_eq!(json["configurations"], json!(["as-is", "short"]));
    assert_eq!(
        json["replays"][0],
        json!({
            "repetition": 1,
            "case": "lanes",
            "configuration": "as-is",
            "state": "judged",
            "outcome": "done",
            "cost": 0.3,
            "seconds": 90.0,
            "correctness": "pass",
            "leak": false,
            "judge_cost": 0.05,
        })
    );
    assert_eq!(
        json["replays"][1],
        json!({
            "repetition": 1,
            "case": "lanes",
            "configuration": "short",
            "state": "unjudged",
            "outcome": "failed",
            "seconds": 30.0,
            "unjudged": "outcome",
        })
    );
    let none = json!({"mean": null, "total": 0.0, "none": 0});
    assert_eq!(
        json["results"]["configurations"][0],
        json!({
            "name": "as-is",
            "pass_rate": 1.0,
            "cost": {"mean": 0.3, "total": 0.3, "none": 0},
            "time": {"mean": 90.0, "total": 90.0, "none": 0},
            "leaks": 0,
            "unjudged": 0,
            "cases": [
                {
                    "case": "lanes",
                    "passes": {"pass": 1, "of": 1},
                    "cost": {"mean": 0.3, "total": 0.3, "none": 0},
                    "time": {"mean": 90.0, "total": 90.0, "none": 0},
                    "leaks": 0,
                    "unjudged": 0,
                },
                {
                    "case": "columns",
                    "passes": null,
                    "cost": none,
                    "time": none,
                    "leaks": 0,
                    "unjudged": 0,
                },
            ],
        })
    );
    assert_eq!(json["results"]["replays_ended"], 2);
    assert_eq!(json["results"]["all"], 4);
    assert_eq!(json["results"]["cost"], json!({"total": 0.35, "none": 1}));
    assert_eq!(json["results"]["judges"], json!({"total": 0.05, "none": 0}));
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_run_show_of_a_benchmark_run_that_does_not_exist_exits_4() {
    let work_dir = tempfile::tempdir().unwrap();
    board(work_dir.path()).await;

    let missing = lw_in(work_dir.path(), &["experiment", "show", "3"]).await;
    let no_database = lw_in(
        tempfile::tempdir().unwrap().path(),
        &["experiment", "show", "3"],
    )
    .await;
    let cancel = lw_in(work_dir.path(), &["experiment", "cancel", "3"]).await;

    for output in [missing, no_database, cancel] {
        assert_eq!(output.code, Some(4), "{}", output.stderr);
        assert_eq!(output.stderr, "lw: there is no experiment 3\n");
    }
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_run_cancel_cancels_an_open_benchmark_run_once() {
    let work_dir = tempfile::tempdir().unwrap();
    board(work_dir.path()).await;
    let launched = lw_in(work_dir.path(), &["experiment", "launch", "board"]).await;
    assert_eq!(launched.code, Some(0), "{}", launched.stderr);

    let cancelled = lw_in(work_dir.path(), &["experiment", "cancel", "1"]).await;
    let again = lw_in(work_dir.path(), &["experiment", "cancel", "1", "--json"]).await;

    assert_eq!(cancelled.code, Some(0), "{}", cancelled.stderr);
    assert_eq!(cancelled.stdout, "Cancelled experiment 1\n");
    assert_eq!(again.code, Some(1), "{}", again.stderr);
    assert_eq!(again.stderr, "lw: experiment 1 is cancelled\n");
    let shown = lw_in(work_dir.path(), &["experiment", "show", "1"]).await;
    assert!(
        shown.stdout.contains("\nstate: cancelled\n"),
        "{}",
        shown.stdout
    );
    assert!(
        shown.stdout.contains("\nreplay as-is/columns/1: pending\n"),
        "{}",
        shown.stdout
    );
    let json = lw_in(work_dir.path(), &["experiment", "show", "1", "--json"]).await;
    let json: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    assert!(json["ended_at"].is_string(), "{json}");
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_lists_its_benchmark_runs_latest_first() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    for _ in 0..2 {
        let launched = lw_in(work_dir.path(), &["experiment", "launch", "board"]).await;
        assert_eq!(launched.code, Some(0), "{}", launched.stderr);
    }
    launched_at(&repo, 1, "2026-11-16T09:00:00Z").await;
    launched_at(&repo, 2, "2026-11-17T12:00:00Z").await;
    trial(
        &repo,
        1,
        (1, 0, 0),
        "state = 'scored', outcome = 'done', seconds = 9, correctness = 'fail', leak = 0",
    )
    .await;
    let cancelled = lw_in(work_dir.path(), &["experiment", "cancel", "1"]).await;
    assert_eq!(cancelled.code, Some(0), "{}", cancelled.stderr);

    let text = lw_in(work_dir.path(), &["suite", "show", "board"]).await;
    let json = lw_in(work_dir.path(), &["suite", "show", "board", "--json"]).await;

    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert!(
        text.stdout.ends_with(
            "experiment 2: waiting for serve, launched 2026-11-17 12:00:00 UTC, pass rates as-is none\n\
             experiment 1: cancelled, launched 2026-11-16 09:00:00 UTC, pass rates as-is 0%\n"
        ),
        "{}",
        text.stdout
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json.stdout).unwrap()["experiments"],
        json!([
            {
                "id": 2,
                "launched_at": "2026-11-17T12:00:00Z",
                "state": "waiting",
                "waiting_for": "serve",
                "pass_rates": [{"configuration": "as-is", "pass_rate": null}],
            },
            {
                "id": 1,
                "launched_at": "2026-11-16T09:00:00Z",
                "state": "cancelled",
                "pass_rates": [{"configuration": "as-is", "pass_rate": 0.0}],
            },
        ])
    );
}
