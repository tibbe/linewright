use crate::common;

use std::path::{Path, PathBuf};

use common::*;
use serde_json::json;

/// The work directory of `REPO` under `work_dir`, its database made, with
/// the design suite `board` holding the cases `columns` and `lanes`.
#[track_caller]
fn board(work_dir: &Path) -> impl Future<Output = PathBuf> + use<> {
    let repo = work_dir.join(REPO);
    let runs = Box::pin(runs_in(&repo));
    let suite = Box::pin(keep_suite(&repo, "board", "design"));
    let lanes = Box::pin(keep_case(
        &repo,
        "board",
        "lanes",
        "4/design-1",
        "Stacks them.\n",
    ));
    let columns = Box::pin(keep_case(
        &repo,
        "board",
        "columns",
        "3/design-2",
        "Names them.\n",
    ));
    let git = repo.join("cases/columns/commits.git");
    std::fs::create_dir_all(git.join("objects")).unwrap();
    std::fs::write(git.join("HEAD"), "ref\n").unwrap();
    async move {
        runs.await;
        suite.await;
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
async fn lw_experiment_launch_keeps_a_copy_of_each_case_and_configuration() {
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
    let kept = repo.join("experiments/1/.kept");
    assert_eq!(names_in(&repo.join("experiments")), ["1"]);
    assert_eq!(names_in(&kept.join("cases")), ["columns", "lanes"]);
    assert_eq!(
        std::fs::read_to_string(kept.join("cases/columns/criteria.md")).unwrap(),
        "Names them.\n"
    );
    assert_eq!(
        std::fs::read_to_string(kept.join("cases/columns/commits.git/HEAD")).unwrap(),
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
async fn lw_experiment_launch_takes_an_id_past_a_directory_a_stopped_launch_left() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    std::fs::create_dir_all(repo.join("experiments/1/.kept")).unwrap();

    let output = lw_in(work_dir.path(), &["experiment", "launch", "board"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        output.stdout,
        "Launched experiment 2 on the suite board: 2 replays\n"
    );
    assert_eq!(
        names_in(&repo.join("experiments/2/.kept/cases")),
        ["columns", "lanes"]
    );
}

#[test_log::test(tokio::test)]
async fn lw_experiment_launch_with_no_configuration_replays_as_is_once() {
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
async fn lw_experiment_launch_refuses_writing_nothing() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    keep_suite(&repo, "empty", "design").await;
    let configurations = tempfile::tempdir().unwrap();
    let short = short(configurations.path());
    let broken = configurations.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("config.toml"), "max-runs = 9\n").unwrap();
    let short_named = |name: &str| format!("{name}={}", short.display());

    let no_suite = lw_in(
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
    let no_cases = lw_in(work_dir.path(), &["experiment", "launch", "empty"]).await;
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

    assert_eq!(no_suite.code, Some(4), "{}", no_suite.stderr);
    assert_eq!(no_suite.stderr, "lw: there is no suite lanes\n");
    assert_eq!(no_cases.code, Some(1), "{}", no_cases.stderr);
    assert_eq!(no_cases.stderr, "lw: the suite empty has no cases\n");
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
    let experiments = repo.join("experiments");
    assert!(!experiments.exists() || names_in(&experiments).is_empty());
    let shown = lw_in(work_dir.path(), &["suite", "board"]).await;
    assert!(
        shown.stdout.ends_with("experiments: none\n"),
        "{}",
        shown.stdout
    );
}

#[test_log::test(tokio::test)]
async fn lw_experiment_launch_of_a_wrong_configuration_or_repetitions_is_a_command_line_error() {
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
async fn an_experiment_keeps_its_cases_as_they_were_at_its_launch() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = board(work_dir.path()).await;
    let launched = lw_in(work_dir.path(), &["experiment", "launch", "board"]).await;
    assert_eq!(launched.code, Some(0), "{}", launched.stderr);

    std::fs::write(repo.join("cases/lanes/criteria.md"), "Hides them.\n").unwrap();
    let removed = lw_in(
        work_dir.path(),
        &["suite", "remove-case", "board", "columns"],
    )
    .await;
    assert_eq!(removed.code, Some(0), "{}", removed.stderr);

    let kept = repo.join("experiments/1/.kept/cases");
    assert_eq!(names_in(&kept), ["columns", "lanes"]);
    assert_eq!(
        std::fs::read_to_string(kept.join("lanes/criteria.md")).unwrap(),
        "Stacks them.\n"
    );
    assert_eq!(
        std::fs::read_to_string(kept.join("columns/criteria.md")).unwrap(),
        "Names them.\n"
    );
    let shown = lw_in(work_dir.path(), &["experiment", "1", "--json"]).await;
    assert_eq!(shown.code, Some(0), "{}", shown.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&shown.stdout).unwrap()["cases"],
        json!(["columns", "lanes"])
    );
}

#[test_log::test(tokio::test)]
async fn lw_experiment_shows_its_state_its_results_and_each_replay() {
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
        "state = 'judged', outcome = 'done', money = 0.3, seconds = 90, \
         correctness = 'pass', leak = 0, judge_money = 0.05",
    )
    .await;
    trial(
        &repo,
        1,
        (1, 0, 1),
        "state = 'unjudged', outcome = 'failed', seconds = 30, unjudged = 'outcome'",
    )
    .await;
    trial(&repo, 1, (1, 1, 0), "state = 'replaying'").await;

    let text = lw_in(work_dir.path(), &["experiment", "1"]).await;
    let json = lw_in(work_dir.path(), &["experiment", "1", "--json"]).await;

    assert_eq!(text.code, Some(0), "{}", text.stderr);
    for line in [
        "launched: 2026-11-17 12:00:00 UTC",
        "state: waiting for serve",
        "replays ended: 2 of 4",
        "replay as-is/columns/1: judged pass, run done, cost $0.30, took 1m 30s, judge's cost $0.05",
        "replay short/lanes/1: pending",
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
    assert_eq!(json["configurations"], json!(["as-is", "short"]));
    assert_eq!(
        json["replays"][0],
        json!({
            "repetition": 1,
            "case": "columns",
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
            "case": "columns",
            "configuration": "short",
            "state": "unjudged",
            "outcome": "failed",
            "seconds": 30.0,
            "unjudged": "outcome",
        })
    );
    assert_eq!(json["results"]["configurations"][0]["pass_rate"], 1.0);
    assert_eq!(json["results"]["ended"], 2);
    assert_eq!(json["results"]["all"], 4);
}

#[test_log::test(tokio::test)]
async fn lw_experiment_of_an_experiment_that_does_not_exist_exits_4() {
    let work_dir = tempfile::tempdir().unwrap();
    board(work_dir.path()).await;

    let missing = lw_in(work_dir.path(), &["experiment", "3"]).await;
    let no_database = lw_in(tempfile::tempdir().unwrap().path(), &["experiment", "3"]).await;
    let cancel = lw_in(work_dir.path(), &["experiment", "cancel", "3"]).await;

    for output in [missing, no_database, cancel] {
        assert_eq!(output.code, Some(4), "{}", output.stderr);
        assert_eq!(output.stderr, "lw: there is no experiment 3\n");
    }
}

#[test_log::test(tokio::test)]
async fn lw_experiment_cancel_cancels_an_open_experiment_once() {
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
    let shown = lw_in(work_dir.path(), &["experiment", "1"]).await;
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
}

#[test_log::test(tokio::test)]
async fn lw_suite_lists_its_experiments_latest_first() {
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
        "state = 'judged', outcome = 'done', seconds = 9, correctness = 'fail', leak = 0",
    )
    .await;
    let cancelled = lw_in(work_dir.path(), &["experiment", "cancel", "1"]).await;
    assert_eq!(cancelled.code, Some(0), "{}", cancelled.stderr);

    let text = lw_in(work_dir.path(), &["suite", "board"]).await;
    let json = lw_in(work_dir.path(), &["suite", "board", "--json"]).await;

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
