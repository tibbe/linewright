use crate::common;

use std::path::{Path, PathBuf};

use common::*;
use serde_json::json;

/// The work directory of `REPO` under `work_dir`, its database made, with
/// the design benchmark `board`, 1, holding the tasks `lanes`, 1, and
/// `columns`, 2.
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
    async move {
        runs.await;
        benchmark.await;
        lanes.await;
        let columns = columns.await;
        let git = repo.join(format!("tasks/{columns}/commits.git"));
        std::fs::create_dir_all(git.join("objects")).unwrap();
        std::fs::write(git.join("HEAD"), "ref\n").unwrap();
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
    let short = short(&configurations.path().join("a=b"));

    let output = lw_in(
        work_dir.path(),
        &[
            "benchmark-run",
            "launch",
            "1",
            "--configuration",
            &format!("Haiku, für <b>={}", short.display()),
            "--repetitions",
            "2",
        ],
    )
    .await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        output.stdout,
        "Launched benchmark run 1 on the benchmark 1 (board): 8 trials\n"
    );
    let kept = repo.join("benchmark-runs/1/.kept");
    assert_eq!(names_in(&repo.join("benchmark-runs")), ["1"]);
    assert_eq!(names_in(&kept.join("tasks")), ["1", "2"]);
    assert_eq!(
        std::fs::read_to_string(kept.join("tasks/2/criteria.md")).unwrap(),
        "Names them.\n"
    );
    assert_eq!(
        std::fs::read_to_string(kept.join("tasks/2/commits.git/HEAD")).unwrap(),
        "ref\n"
    );
    assert_eq!(names_in(&kept.join("configurations")), ["1", "2"]);
    assert_eq!(
        names_in(&kept.join("configurations/1")),
        Vec::<String>::new()
    );
    assert_eq!(
        std::fs::read_to_string(kept.join("configurations/2/config.toml")).unwrap(),
        "model = \"haiku\"\n"
    );
    let shown = lw_in(work_dir.path(), &["benchmark-run", "show", "1", "--json"]).await;
    let shown: serde_json::Value = serde_json::from_str(&shown.stdout).unwrap();
    assert_eq!(
        shown["configurations"],
        json!([{"id": 1, "name": "as-is"}, {"id": 2, "name": "Haiku, für <b>"}])
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_run_launch_takes_an_id_past_a_directory_a_stopped_launch_left() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    std::fs::create_dir_all(repo.join("benchmark-runs/1/.kept")).unwrap();

    let output = lw_in(work_dir.path(), &["benchmark-run", "launch", "1"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        output.stdout,
        "Launched benchmark run 2 on the benchmark 1 (board): 2 trials\n"
    );
    assert_eq!(
        names_in(&repo.join("benchmark-runs/2/.kept/tasks")),
        ["1", "2"]
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_run_launch_with_no_configuration_runs_as_is_once() {
    let work_dir = tempfile::tempdir().unwrap();
    board(work_dir.path()).await;

    let output = lw_in(work_dir.path(), &["benchmark-run", "launch", "1", "--json"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output.stdout).unwrap(),
        json!({"id": 1, "benchmark": 1, "trials": 2})
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
            "benchmark-run",
            "launch",
            "9",
            "--configuration",
            &format!("broken={}", broken.display()),
        ],
    )
    .await;
    let no_tasks = lw_in(work_dir.path(), &["benchmark-run", "launch", "2"]).await;
    let with_an_error = lw_in(
        work_dir.path(),
        &[
            "benchmark-run",
            "launch",
            "1",
            "--configuration",
            &format!("broken={}", broken.display()),
        ],
    )
    .await;
    let twice = lw_in(
        work_dir.path(),
        &[
            "benchmark-run",
            "launch",
            "1",
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
            "benchmark-run",
            "launch",
            "1",
            "--configuration",
            &short_named("as-is"),
        ],
    )
    .await;

    assert_eq!(no_benchmark.code, Some(4), "{}", no_benchmark.stderr);
    assert_eq!(no_benchmark.stderr, "lw: there is no benchmark 9\n");
    assert_eq!(no_tasks.code, Some(1), "{}", no_tasks.stderr);
    assert_eq!(no_tasks.stderr, "lw: the benchmark empty has no tasks\n");
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
    let shown = lw_in(work_dir.path(), &["benchmark", "show", "1"]).await;
    assert!(
        shown.stdout.ends_with("benchmark runs: none\n"),
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
        &["benchmark-run", "launch", "1", "--configuration", "short"],
    )
    .await;
    let named = lw_in(
        work_dir.path(),
        &["benchmark-run", "launch", "1", "--configuration", " =x"],
    )
    .await;
    let none = lw_in(
        work_dir.path(),
        &["benchmark-run", "launch", "1", "--repetitions", "0"],
    )
    .await;

    for output in [no_dir, named, none] {
        assert_eq!(output.code, Some(2), "{}", output.stderr);
    }
}

#[test_log::test(tokio::test)]
async fn a_benchmark_run_keeps_its_tasks_as_they_were_at_its_launch_but_their_names() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    let launched = lw_in(work_dir.path(), &["benchmark-run", "launch", "1"]).await;
    assert_eq!(launched.code, Some(0), "{}", launched.stderr);

    std::fs::write(repo.join("tasks/1/criteria.md"), "Hides them.\n").unwrap();
    let removed = lw_in(work_dir.path(), &["benchmark", "remove-task", "1", "2"]).await;
    assert_eq!(removed.code, Some(0), "{}", removed.stderr);
    let pool = back_door(&repo).await;
    sqlx::query("UPDATE tasks SET name = 'stacks' WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let kept = repo.join("benchmark-runs/1/.kept/tasks");
    assert_eq!(names_in(&kept), ["1", "2"]);
    assert_eq!(
        std::fs::read_to_string(kept.join("1/criteria.md")).unwrap(),
        "Stacks them.\n"
    );
    assert_eq!(
        std::fs::read_to_string(kept.join("2/criteria.md")).unwrap(),
        "Names them.\n"
    );
    let shown = lw_in(work_dir.path(), &["benchmark-run", "show", "1", "--json"]).await;
    assert_eq!(shown.code, Some(0), "{}", shown.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&shown.stdout).unwrap()["tasks"],
        json!([{"id": 1, "name": "stacks"}, {"id": 2, "name": "columns"}])
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
            "benchmark-run",
            "launch",
            "1",
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

    let text = lw_in(work_dir.path(), &["benchmark-run", "show", "1"]).await;
    let json = lw_in(work_dir.path(), &["benchmark-run", "show", "1", "--json"]).await;

    assert_eq!(text.code, Some(0), "{}", text.stderr);
    for line in [
        "launched: 2026-11-17 12:00:00 UTC",
        "state: waiting for serve",
        "trials ended: 2 of 4",
        "benchmark: 1 (board)",
        "configuration 1 (as-is):",
        "configuration 2 (short):",
        "  task 1 (lanes):",
        "  task 2 (columns):",
        "trial 1 of lanes under as-is (1/1/1): scored pass, run done, cost $0.30, took 1m 30s, scorer's cost $0.05",
        "trial 1 of columns under short (2/2/1): pending",
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
    assert_eq!(json["benchmark"], json!({"id": 1, "name": "board"}));
    assert_eq!(
        json["tasks"],
        json!([{"id": 1, "name": "lanes"}, {"id": 2, "name": "columns"}])
    );
    assert_eq!(
        json["configurations"],
        json!([{"id": 1, "name": "as-is"}, {"id": 2, "name": "short"}])
    );
    assert_eq!(
        json["trials"][0],
        json!({
            "repetition": 1,
            "task": 1,
            "configuration": 1,
            "state": "scored",
            "outcome": "done",
            "cost": 0.3,
            "seconds": 90.0,
            "correctness": "pass",
            "leak": false,
            "scorer_cost": 0.05,
        })
    );
    assert_eq!(
        json["trials"][1],
        json!({
            "repetition": 1,
            "task": 1,
            "configuration": 2,
            "state": "unscored",
            "outcome": "failed",
            "seconds": 30.0,
            "unscored": "outcome",
        })
    );
    let none = json!({"mean": null, "total": 0.0, "none": 0});
    assert_eq!(
        json["results"]["configurations"][0],
        json!({
            "id": 1,
            "name": "as-is",
            "pass_rate": 1.0,
            "cost": {"mean": 0.3, "total": 0.3, "none": 0},
            "time": {"mean": 90.0, "total": 90.0, "none": 0},
            "leaks": 0,
            "unscored": 0,
            "tasks": [
                {
                    "id": 1,
                    "name": "lanes",
                    "passes": {"pass": 1, "of": 1},
                    "cost": {"mean": 0.3, "total": 0.3, "none": 0},
                    "time": {"mean": 90.0, "total": 90.0, "none": 0},
                    "leaks": 0,
                    "unscored": 0,
                },
                {
                    "id": 2,
                    "name": "columns",
                    "passes": null,
                    "cost": none,
                    "time": none,
                    "leaks": 0,
                    "unscored": 0,
                },
            ],
        })
    );
    assert_eq!(json["results"]["trials_ended"], 2);
    assert_eq!(json["results"]["all"], 4);
    assert_eq!(json["results"]["cost"], json!({"total": 0.35, "none": 1}));
    assert_eq!(
        json["results"]["scorers"],
        json!({"total": 0.05, "none": 0})
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_run_show_of_a_benchmark_run_that_does_not_exist_exits_4() {
    let work_dir = tempfile::tempdir().unwrap();
    board(work_dir.path()).await;

    let missing = lw_in(work_dir.path(), &["benchmark-run", "show", "3"]).await;
    let no_database = lw_in(
        tempfile::tempdir().unwrap().path(),
        &["benchmark-run", "show", "3"],
    )
    .await;
    let cancel = lw_in(work_dir.path(), &["benchmark-run", "cancel", "3"]).await;

    for output in [missing, no_database, cancel] {
        assert_eq!(output.code, Some(4), "{}", output.stderr);
        assert_eq!(output.stderr, "lw: there is no benchmark run 3\n");
    }
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_run_cancel_cancels_an_open_benchmark_run_once() {
    let work_dir = tempfile::tempdir().unwrap();
    board(work_dir.path()).await;
    let launched = lw_in(work_dir.path(), &["benchmark-run", "launch", "1"]).await;
    assert_eq!(launched.code, Some(0), "{}", launched.stderr);

    let cancelled = lw_in(work_dir.path(), &["benchmark-run", "cancel", "1"]).await;
    let again = lw_in(work_dir.path(), &["benchmark-run", "cancel", "1", "--json"]).await;

    assert_eq!(cancelled.code, Some(0), "{}", cancelled.stderr);
    assert_eq!(cancelled.stdout, "Cancelled benchmark run 1\n");
    assert_eq!(again.code, Some(1), "{}", again.stderr);
    assert_eq!(again.stderr, "lw: benchmark run 1 is cancelled\n");
    let shown = lw_in(work_dir.path(), &["benchmark-run", "show", "1"]).await;
    assert!(
        shown.stdout.contains("\nstate: cancelled\n"),
        "{}",
        shown.stdout
    );
    assert!(
        shown
            .stdout
            .contains("\ntrial 1 of columns under as-is (1/2/1): pending\n"),
        "{}",
        shown.stdout
    );
    let json = lw_in(work_dir.path(), &["benchmark-run", "show", "1", "--json"]).await;
    let json: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    assert!(json["ended_at"].is_string(), "{json}");
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_lists_its_benchmark_runs_latest_first() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    for _ in 0..2 {
        let launched = lw_in(work_dir.path(), &["benchmark-run", "launch", "1"]).await;
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
    let cancelled = lw_in(work_dir.path(), &["benchmark-run", "cancel", "1"]).await;
    assert_eq!(cancelled.code, Some(0), "{}", cancelled.stderr);

    let text = lw_in(work_dir.path(), &["benchmark", "show", "1"]).await;
    let json = lw_in(work_dir.path(), &["benchmark", "show", "1", "--json"]).await;

    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert!(
        text.stdout.ends_with(
            "benchmark run 2: waiting for serve, launched 2026-11-17 12:00:00 UTC, pass rates as-is none\n\
             benchmark run 1: cancelled, launched 2026-11-16 09:00:00 UTC, pass rates as-is 0%\n"
        ),
        "{}",
        text.stdout
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json.stdout).unwrap()["benchmark_runs"],
        json!([
            {
                "id": 2,
                "launched_at": "2026-11-17T12:00:00Z",
                "state": "waiting",
                "waiting_for": "serve",
                "pass_rates": [{"configuration": {"id": 1, "name": "as-is"}, "pass_rate": null}],
            },
            {
                "id": 1,
                "launched_at": "2026-11-16T09:00:00Z",
                "state": "cancelled",
                "pass_rates": [{"configuration": {"id": 1, "name": "as-is"}, "pass_rate": 0.0}],
            },
        ])
    );
}

/// `docs/specs/benchmarks.md`, Benchmark runs, At its launch.
#[test_log::test(tokio::test)]
async fn a_criteria_edit_reaches_the_benchmark_runs_launched_after_it_and_not_before() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    let before = lw_in(work_dir.path(), &["benchmark-run", "launch", "1"]).await;
    assert_eq!(before.code, Some(0), "{}", before.stderr);
    let files = tempfile::tempdir().unwrap();
    let hides = files.path().join("hides.md");
    std::fs::write(&hides, "Hides them.\n").unwrap();

    let edited = lw_in(
        work_dir.path(),
        &[
            "benchmark",
            "edit-task",
            "1",
            "1",
            "--name",
            "Hide the lanes",
            "--criteria",
            hides.to_str().unwrap(),
        ],
    )
    .await;
    let after = lw_in(work_dir.path(), &["benchmark-run", "launch", "1"]).await;

    assert_eq!(edited.code, Some(0), "{}", edited.stderr);
    assert_eq!(after.code, Some(0), "{}", after.stderr);
    assert_eq!(
        std::fs::read_to_string(repo.join("benchmark-runs/1/.kept/tasks/1/criteria.md")).unwrap(),
        "Stacks them.\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("benchmark-runs/2/.kept/tasks/1/criteria.md")).unwrap(),
        "Hides them.\n"
    );
    let shown = lw_in(work_dir.path(), &["benchmark-run", "show", "1", "--json"]).await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&shown.stdout).unwrap()["tasks"],
        json!([{"id": 1, "name": "Hide the lanes"}, {"id": 2, "name": "columns"}])
    );
}

/// `docs/specs/benchmarks.md`, Benchmark runs, The configurations.
#[test_log::test(tokio::test)]
async fn lw_benchmark_run_rename_configuration_changes_its_name_after_the_launch() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    keep_benchmark_run(&repo, 1, 1, &[1, 2], &["as-is", "short", "long"]).await;

    let renamed = lw_in(
        work_dir.path(),
        &[
            "benchmark-run",
            "rename-configuration",
            "1",
            "2",
            " Haiku, short ",
        ],
    )
    .await;
    let json = lw_in(
        work_dir.path(),
        &[
            "benchmark-run",
            "rename-configuration",
            "1",
            "3",
            "Long",
            "--json",
        ],
    )
    .await;

    assert_eq!(renamed.code, Some(0), "{}", renamed.stderr);
    assert_eq!(
        renamed.stdout,
        "Renamed the configuration 2 of benchmark run 1 to Haiku, short\n"
    );
    assert_eq!(json.code, Some(0), "{}", json.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json.stdout).unwrap(),
        json!({"id": 1, "configuration": 3, "name": "Long"})
    );
    let text = lw_in(work_dir.path(), &["benchmark-run", "show", "1"]).await;
    assert!(
        text.stdout
            .lines()
            .any(|line| line == "configuration 2 (Haiku, short):"),
        "{}",
        text.stdout
    );
    let shown = lw_in(work_dir.path(), &["benchmark-run", "show", "1", "--json"]).await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&shown.stdout).unwrap()["configurations"],
        json!([
            {"id": 1, "name": "as-is"},
            {"id": 2, "name": "Haiku, short"},
            {"id": 3, "name": "Long"},
        ])
    );
}

/// `docs/specs/benchmarks.md`, Benchmark runs, The configurations.
#[test_log::test(tokio::test)]
async fn lw_benchmark_run_rename_configuration_refuses_as_is_a_taken_name_and_one_that_does_not_exist()
 {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    keep_benchmark_run(&repo, 1, 1, &[1, 2], &["as-is", "short", "long"]).await;

    let as_is = lw_in(
        work_dir.path(),
        &["benchmark-run", "rename-configuration", "1", "1", "plain"],
    )
    .await;
    let taken = lw_in(
        work_dir.path(),
        &["benchmark-run", "rename-configuration", "1", "2", "long"],
    )
    .await;
    let taken_by_as_is = lw_in(
        work_dir.path(),
        &["benchmark-run", "rename-configuration", "1", "2", "as-is"],
    )
    .await;
    let no_configuration = lw_in(
        work_dir.path(),
        &["benchmark-run", "rename-configuration", "1", "4", "long"],
    )
    .await;
    let no_benchmark_run = lw_in(
        work_dir.path(),
        &["benchmark-run", "rename-configuration", "2", "2", "more"],
    )
    .await;

    assert_eq!(as_is.code, Some(1), "{}", as_is.stderr);
    assert_eq!(
        as_is.stderr,
        "lw: the configuration 1, as-is, keeps its name\n"
    );
    assert_eq!(taken.code, Some(1), "{}", taken.stderr);
    assert_eq!(taken.stderr, "lw: another configuration is named long\n");
    assert_eq!(taken_by_as_is.code, Some(1), "{}", taken_by_as_is.stderr);
    assert_eq!(
        taken_by_as_is.stderr,
        "lw: another configuration is named as-is\n"
    );
    assert_eq!(
        no_configuration.code,
        Some(4),
        "{}",
        no_configuration.stderr
    );
    assert_eq!(
        no_configuration.stderr,
        "lw: benchmark run 1 has no configuration 4\n"
    );
    assert_eq!(
        no_benchmark_run.code,
        Some(4),
        "{}",
        no_benchmark_run.stderr
    );
    assert_eq!(no_benchmark_run.stderr, "lw: there is no benchmark run 2\n");
    let shown = lw_in(work_dir.path(), &["benchmark-run", "show", "1", "--json"]).await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&shown.stdout).unwrap()["configurations"],
        json!([
            {"id": 1, "name": "as-is"},
            {"id": 2, "name": "short"},
            {"id": 3, "name": "long"},
        ])
    );
}
