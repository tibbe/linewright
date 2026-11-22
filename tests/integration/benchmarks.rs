use crate::common;

use std::path::{Path, PathBuf};

use common::*;
use serde_json::json;

/// The work directory of `REPO` under `work_dir`, its database made.
#[track_caller]
fn made(work_dir: &Path) -> impl Future<Output = PathBuf> + use<> {
    let repo = work_dir.join(REPO);
    let runs = runs_in(&repo);
    async move {
        runs.await;
        repo
    }
}

#[test_log::test(tokio::test)]
async fn lw_benchmarks_lists_each_benchmark_with_its_stage_and_how_many_tasks() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_benchmark(&repo, "triage-basics", "triage").await;
    keep_task(&repo, "board", "columns", "3/implement-1", "Names them.\n").await;
    keep_task(&repo, "board", "lanes", "4/implement-1", "Stacks them.\n").await;

    let text = lw_in(work_dir.path(), &["benchmarks"]).await;
    let json = lw_in(work_dir.path(), &["benchmarks", "--json"]).await;

    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert_eq!(
        text.stdout,
        "1 (board): implement, 2 tasks\n2 (triage-basics): triage, no tasks\n"
    );
    assert_eq!(json.code, Some(0), "{}", json.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json.stdout).unwrap(),
        json!({"benchmarks": [
            {"id": 1, "name": "board", "stage": "implement", "tasks": 2},
            {"id": 2, "name": "triage-basics", "stage": "triage", "tasks": 0},
        ]})
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmarks_with_no_database_lists_none() {
    let work_dir = tempfile::tempdir().unwrap();

    let output = lw_in(work_dir.path(), &["benchmarks"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, "benchmarks: none\n");
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_add_makes_a_benchmark_of_its_stage() {
    let work_dir = tempfile::tempdir().unwrap();

    let output = lw_in(work_dir.path(), &["benchmark", "add", "board", "design"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, "Made the benchmark 1 (board), of design\n");
    let listed = lw_in(work_dir.path(), &["benchmarks"]).await;
    assert_eq!(listed.stdout, "1 (board): design, no tasks\n");
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_add_of_a_name_a_benchmark_has_exits_1_saying_why() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;

    let output = lw_in(work_dir.path(), &["benchmark", "add", "board", "design"]).await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert_eq!(output.stderr, "lw: a benchmark is already named board\n");
    let listed = lw_in(work_dir.path(), &["benchmarks"]).await;
    assert_eq!(listed.stdout, "1 (board): implement, no tasks\n");
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_add_of_a_stage_no_task_has_or_a_name_not_a_benchmarks_is_a_command_line_error()
 {
    let work_dir = tempfile::tempdir().unwrap();

    let rework = lw_in(work_dir.path(), &["benchmark", "add", "board", "rework"]).await;
    let named = lw_in(work_dir.path(), &["benchmark", "add", "Board", "design"]).await;

    assert_eq!(rework.code, Some(2), "{}", rework.stderr);
    assert!(
        rework.stderr.contains("one of triage, design, implement"),
        "{}",
        rework.stderr
    );
    assert_eq!(named.code, Some(2), "{}", named.stderr);
    assert!(
        named
            .stderr
            .contains("lowercase letters, digits and hyphens"),
        "{}",
        named.stderr
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_show_shows_its_tasks_with_their_criteria_in_the_order_they_were_captured() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_task(
        &repo,
        "board",
        "lanes",
        "4/implement-1",
        "Stacks them.\n\nOn a phone.\n",
    )
    .await;
    keep_task(&repo, "board", "columns", "3/implement-2", "Names them.\n").await;

    let text = lw_in(work_dir.path(), &["benchmark", "show", "1"]).await;
    let json = lw_in(work_dir.path(), &["benchmark", "show", "1", "--json"]).await;

    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert_eq!(
        text.stdout,
        "benchmark: 1 (board)\nstage: implement\n\
         task 1 (lanes): captured from 4/implement-1\n  Stacks them.\n\n  On a phone.\n\
         task 2 (columns): captured from 3/implement-2\n  Names them.\n\
         benchmark runs: none\n"
    );
    assert_eq!(json.code, Some(0), "{}", json.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json.stdout).unwrap(),
        json!({
            "id": 1,
            "name": "board",
            "stage": "implement",
            "tasks": [
                {"id": 1, "name": "lanes", "run": "4/implement-1", "criteria": "Stacks them.\n\nOn a phone.\n"},
                {"id": 2, "name": "columns", "run": "3/implement-2", "criteria": "Names them.\n"},
            ],
            "benchmark_runs": [],
        })
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_show_says_when_a_tasks_criteria_do_not_read() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_task(&repo, "board", "lanes", "4/implement-1", " \n").await;

    let text = lw_in(work_dir.path(), &["benchmark", "show", "1"]).await;
    let json = lw_in(work_dir.path(), &["benchmark", "show", "1", "--json"]).await;

    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert_eq!(
        text.stdout,
        "benchmark: 1 (board)\nstage: implement\n\
         task 1 (lanes): captured from 4/implement-1\n  \
         the criteria do not read: criteria.md holds only white space\n\
         benchmark runs: none\n"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json.stdout).unwrap()["tasks"][0],
        json!({
            "id": 1,
            "name": "lanes",
            "run": "4/implement-1",
            "unreadable": "criteria.md holds only white space",
        })
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_show_of_a_benchmark_with_no_tasks_says_so() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "triage").await;

    let output = lw_in(work_dir.path(), &["benchmark", "show", "1"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        output.stdout,
        "benchmark: 1 (board)\nstage: triage\ntasks: none\nbenchmark runs: none\n"
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_show_takes_a_benchmarks_id_and_not_its_name() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "add", "triage").await;

    let output = lw_in(work_dir.path(), &["benchmark", "show", "1"]).await;
    let named = lw_in(work_dir.path(), &["benchmark", "show", "add"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        output.stdout,
        "benchmark: 1 (add)\nstage: triage\ntasks: none\nbenchmark runs: none\n"
    );
    assert_eq!(named.code, Some(2), "{}", named.stderr);
    assert!(
        named.stderr.contains("is not a benchmark's ID"),
        "{}",
        named.stderr
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_show_of_a_benchmark_that_does_not_exist_exits_4() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "triage").await;

    let missing = lw_in(work_dir.path(), &["benchmark", "show", "2"]).await;
    let no_database = lw_in(
        tempfile::tempdir().unwrap().path(),
        &["benchmark", "show", "2"],
    )
    .await;

    for output in [missing, no_database] {
        assert_eq!(output.code, Some(4), "{}", output.stderr);
        assert_eq!(output.stderr, "lw: there is no benchmark 2\n");
    }
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_remove_task_removes_the_task_and_its_directory() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_task(&repo, "board", "columns", "3/implement-2", "Names them.\n").await;
    keep_task(&repo, "board", "lanes", "4/implement-1", "Stacks them.\n").await;

    let output = lw_in(work_dir.path(), &["benchmark", "remove-task", "1", "2"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        output.stdout,
        "Removed the task 2 (lanes) from the benchmark 1\n"
    );
    let listed = lw_in(work_dir.path(), &["benchmarks"]).await;
    assert_eq!(listed.stdout, "1 (board): implement, 1 task\n");
    let left: Vec<String> = std::fs::read_dir(repo.join("tasks"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(left, ["1"]);
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_remove_task_of_a_benchmark_or_task_that_does_not_exist_exits_4() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_benchmark(&repo, "designs", "design").await;
    keep_task(&repo, "board", "lanes", "4/implement-1", "Stacks them.\n").await;

    let no_benchmark = lw_in(work_dir.path(), &["benchmark", "remove-task", "3", "1"]).await;
    let no_task = lw_in(work_dir.path(), &["benchmark", "remove-task", "1", "2"]).await;
    let other_benchmark = lw_in(work_dir.path(), &["benchmark", "remove-task", "2", "1"]).await;

    assert_eq!(no_benchmark.code, Some(4), "{}", no_benchmark.stderr);
    assert_eq!(no_benchmark.stderr, "lw: there is no benchmark 3\n");
    assert_eq!(no_task.code, Some(4), "{}", no_task.stderr);
    assert_eq!(no_task.stderr, "lw: the benchmark 1 has no task 2\n");
    assert_eq!(other_benchmark.code, Some(4), "{}", other_benchmark.stderr);
    assert_eq!(
        other_benchmark.stderr,
        "lw: the benchmark 2 has no task 1\n"
    );
    assert!(repo.join("tasks").join("1").join("criteria.md").exists());
    let listed = lw_in(work_dir.path(), &["benchmarks"]).await;
    assert_eq!(
        listed.stdout,
        "1 (board): implement, 1 task\n2 (designs): design, no tasks\n"
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_add_and_remove_task_answer_in_json() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_task(&repo, "board", "lanes", "4/implement-1", "Stacks them.\n").await;

    let added = lw_in(
        work_dir.path(),
        &["benchmark", "add", "designs", "design", "--json"],
    )
    .await;
    let removed = lw_in(
        work_dir.path(),
        &["benchmark", "remove-task", "1", "1", "--json"],
    )
    .await;

    assert_eq!(added.code, Some(0), "{}", added.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&added.stdout).unwrap(),
        json!({"id": 2, "name": "designs", "stage": "design"})
    );
    assert_eq!(removed.code, Some(0), "{}", removed.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&removed.stdout).unwrap(),
        json!({"benchmark": 1, "task": 1, "name": "lanes"})
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_remove_task_removes_a_task_whose_directory_is_gone() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_task_row(&repo, "board", "lanes", "4/implement-1").await;

    let output = lw_in(work_dir.path(), &["benchmark", "remove-task", "1", "1"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let listed = lw_in(work_dir.path(), &["benchmarks"]).await;
    assert_eq!(listed.stdout, "1 (board): implement, no tasks\n");
}
