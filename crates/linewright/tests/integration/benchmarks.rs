use std::path::{Path, PathBuf};

use linewright_core::testing::*;
use serde_json::json;

use crate::fake_serve::FakeServe;
use crate::programs::*;

/// The work directory of `REPO` under `work_dir`, its database made.
async fn made(work_dir: &Path) -> PathBuf {
    let repo = work_dir.join(REPO);
    runs_in(&repo).await;
    repo
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

/// `docs/specs/operation.md`, Commands, `serve` does the work.
#[test_log::test(tokio::test)]
async fn lw_benchmark_add_asks_serve_which_makes_a_benchmark_of_its_stage() {
    let work_dir = tempfile::tempdir().unwrap();
    let serve = FakeServe::over(work_dir.path()).await;

    let output = serve.lw(&["benchmark", "add", "board", "design"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, "Made the benchmark 1 (board), of design\n");
    let listed = serve.lw(&["benchmarks"]).await;
    assert_eq!(listed.stdout, "1 (board): design, no tasks\n");
    serve.stop().await;
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_add_of_a_name_a_benchmark_has_exits_1_saying_why() {
    let work_dir = tempfile::tempdir().unwrap();
    let serve = FakeServe::over(work_dir.path()).await;
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;

    let output = serve.lw(&["benchmark", "add", "board", "design"]).await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert_eq!(output.stderr, "lw: a benchmark is already named board\n");
    let listed = serve.lw(&["benchmarks"]).await;
    assert_eq!(listed.stdout, "1 (board): implement, no tasks\n");
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_add_of_a_stage_no_task_has_or_no_name_is_a_command_line_error() {
    let work_dir = tempfile::tempdir().unwrap();

    let rework = lw_in(work_dir.path(), &["benchmark", "add", "board", "rework"]).await;
    let named = lw_in(
        work_dir.path(),
        &["benchmark", "add", " \t\u{200b}", "design"],
    )
    .await;

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
            .contains("it holds nothing once it is clean, which is no name"),
        "{}",
        named.stderr
    );
}

/// `docs/specs/benchmarks.md`, Names and IDs.
#[test_log::test(tokio::test)]
async fn lw_benchmark_add_tells_names_of_other_letter_case_apart() {
    let work_dir = tempfile::tempdir().unwrap();
    let serve = FakeServe::over(work_dir.path()).await;

    let upper = serve.lw(&["benchmark", "add", "Board", "triage"]).await;
    let lower = serve.lw(&["benchmark", "add", "board", "triage"]).await;
    let again = serve.lw(&["benchmark", "add", "Board", "triage"]).await;

    assert_eq!(upper.code, Some(0), "{}", upper.stderr);
    assert_eq!(lower.code, Some(0), "{}", lower.stderr);
    assert_eq!(again.code, Some(1), "{}", again.stderr);
    assert_eq!(again.stderr, "lw: a benchmark is already named Board\n");
    let listed = serve.lw(&["benchmarks"]).await;
    assert_eq!(
        listed.stdout,
        "2 (board): triage, no tasks\n\
         1 (Board): triage, no tasks\n"
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
    let serve = FakeServe::over(work_dir.path()).await;
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_task(&repo, "board", "columns", "3/implement-2", "Names them.\n").await;
    keep_task(&repo, "board", "lanes", "4/implement-1", "Stacks them.\n").await;

    let output = serve.lw(&["benchmark", "remove-task", "1", "2"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        output.stdout,
        "Removed the task 2 (lanes) from the benchmark 1\n"
    );
    let listed = serve.lw(&["benchmarks"]).await;
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
    let serve = FakeServe::over(work_dir.path()).await;
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_benchmark(&repo, "designs", "design").await;
    keep_task(&repo, "board", "lanes", "4/implement-1", "Stacks them.\n").await;

    let no_benchmark = serve.lw(&["benchmark", "remove-task", "3", "1"]).await;
    let no_task = serve.lw(&["benchmark", "remove-task", "1", "2"]).await;
    let other_benchmark = serve.lw(&["benchmark", "remove-task", "2", "1"]).await;

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
    let listed = serve.lw(&["benchmarks"]).await;
    assert_eq!(
        listed.stdout,
        "1 (board): implement, 1 task\n2 (designs): design, no tasks\n"
    );
}

#[test_log::test(tokio::test)]
async fn lw_benchmark_add_and_remove_task_answer_in_json() {
    let work_dir = tempfile::tempdir().unwrap();
    let serve = FakeServe::over(work_dir.path()).await;
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_task(&repo, "board", "lanes", "4/implement-1", "Stacks them.\n").await;

    let added = serve
        .lw(&["benchmark", "add", "designs", "design", "--json"])
        .await;
    let removed = serve
        .lw(&["benchmark", "remove-task", "1", "1", "--json"])
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
    let serve = FakeServe::over(work_dir.path()).await;
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_task_row(&repo, "board", "lanes", "4/implement-1").await;

    let output = serve.lw(&["benchmark", "remove-task", "1", "1"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let listed = serve.lw(&["benchmarks"]).await;
    assert_eq!(listed.stdout, "1 (board): implement, no tasks\n");
}

/// `docs/specs/benchmarks.md`, Benchmarks, Changing.
#[test_log::test(tokio::test)]
async fn lw_benchmark_rename_changes_a_benchmarks_name_refusing_one_another_benchmark_has() {
    let work_dir = tempfile::tempdir().unwrap();
    let serve = FakeServe::over(work_dir.path()).await;
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_benchmark(&repo, "designs", "design").await;

    let renamed = serve
        .lw(&["benchmark", "rename", "1", "  Board games  "])
        .await;
    let taken = serve.lw(&["benchmark", "rename", "2", "Board games"]).await;
    let same = serve.lw(&["benchmark", "rename", "1", "Board games"]).await;
    let missing = serve.lw(&["benchmark", "rename", "3", "Cards"]).await;
    let blank = serve.lw(&["benchmark", "rename", "1", " "]).await;

    assert_eq!(renamed.code, Some(0), "{}", renamed.stderr);
    assert_eq!(renamed.stdout, "Renamed the benchmark 1 to Board games\n");
    assert_eq!(taken.code, Some(1), "{}", taken.stderr);
    assert_eq!(
        taken.stderr,
        "lw: a benchmark is already named Board games\n"
    );
    assert_eq!(same.code, Some(0), "{}", same.stderr);
    assert_eq!(same.stdout, "Renamed the benchmark 1 to Board games\n");
    assert_eq!(missing.code, Some(4), "{}", missing.stderr);
    assert_eq!(missing.stderr, "lw: there is no benchmark 3\n");
    assert_eq!(blank.code, Some(2), "{}", blank.stderr);
    let listed = serve.lw(&["benchmarks"]).await;
    assert_eq!(
        listed.stdout,
        "1 (Board games): implement, no tasks\n2 (designs): design, no tasks\n"
    );
}

/// `docs/specs/benchmarks.md`, Benchmarks, Changing.
#[test_log::test(tokio::test)]
async fn lw_benchmark_edit_task_changes_a_tasks_name_its_criteria_or_both() {
    let work_dir = tempfile::tempdir().unwrap();
    let serve = FakeServe::over(work_dir.path()).await;
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_task(&repo, "board", "lanes", "4/implement-1", "Stacks them.\n").await;
    keep_task(&repo, "board", "columns", "3/implement-2", "Names them.\n").await;
    let files = tempfile::tempdir().unwrap();
    let hides = files.path().join("hides.md");
    std::fs::write(&hides, "Hides them.\n").unwrap();
    let sorts = files.path().join("sorts.md");
    std::fs::write(&sorts, "Sorts them.\n").unwrap();

    let named = serve
        .lw(&[
            "benchmark",
            "edit-task",
            "1",
            "1",
            "--name",
            "Stack the lanes",
        ])
        .await;
    let criteria = serve
        .lw(&[
            "benchmark",
            "edit-task",
            "1",
            "2",
            "--criteria",
            hides.to_str().unwrap(),
        ])
        .await;
    let both = serve
        .lw(&[
            "benchmark",
            "edit-task",
            "1",
            "1",
            "--name",
            "Sort the lanes",
            "--criteria",
            sorts.to_str().unwrap(),
            "--json",
        ])
        .await;

    assert_eq!(named.code, Some(0), "{}", named.stderr);
    assert_eq!(named.stdout, "Changed the task 1 of the benchmark 1\n");
    assert_eq!(criteria.code, Some(0), "{}", criteria.stderr);
    assert_eq!(both.code, Some(0), "{}", both.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&both.stdout).unwrap(),
        json!({"benchmark": 1, "task": 1})
    );
    let shown = serve.lw(&["benchmark", "show", "1"]).await;
    assert_eq!(
        shown.stdout,
        "benchmark: 1 (board)\nstage: implement\n\
         task 1 (Sort the lanes): captured from 4/implement-1\n  Sorts them.\n\
         task 2 (columns): captured from 3/implement-2\n  Hides them.\n\
         benchmark runs: none\n"
    );
}

/// `docs/specs/benchmarks.md`, Benchmarks, Changing.
#[test_log::test(tokio::test)]
async fn lw_benchmark_edit_task_refused_changes_neither_the_name_nor_the_criteria() {
    let work_dir = tempfile::tempdir().unwrap();
    let serve = FakeServe::over(work_dir.path()).await;
    let repo = made(work_dir.path()).await;
    keep_benchmark(&repo, "board", "implement").await;
    keep_benchmark(&repo, "designs", "design").await;
    keep_task(&repo, "board", "lanes", "4/implement-1", "Stacks them.\n").await;
    keep_task(&repo, "board", "columns", "3/implement-2", "Names them.\n").await;
    let files = tempfile::tempdir().unwrap();
    let hides = files.path().join("hides.md");
    std::fs::write(&hides, "Hides them.\n").unwrap();
    let hides = hides.to_str().unwrap();
    let blank = files.path().join("blank.md");
    std::fs::write(&blank, " \n\t\n").unwrap();
    let missing_file = files.path().join("missing.md");

    let neither = serve.lw(&["benchmark", "edit-task", "1", "1"]).await;
    let taken = serve
        .lw(&[
            "benchmark",
            "edit-task",
            "1",
            "1",
            "--name",
            "columns",
            "--criteria",
            hides,
        ])
        .await;
    let white_space = serve
        .lw(&[
            "benchmark",
            "edit-task",
            "1",
            "1",
            "--name",
            "Stack the lanes",
            "--criteria",
            blank.to_str().unwrap(),
        ])
        .await;
    let unread = serve
        .lw(&[
            "benchmark",
            "edit-task",
            "1",
            "1",
            "--name",
            "Stack the lanes",
            "--criteria",
            missing_file.to_str().unwrap(),
        ])
        .await;
    let no_benchmark = serve
        .lw(&["benchmark", "edit-task", "3", "1", "--name", "Lanes"])
        .await;
    let other_benchmark = serve
        .lw(&["benchmark", "edit-task", "2", "1", "--name", "Lanes"])
        .await;
    let no_task = serve
        .lw(&["benchmark", "edit-task", "1", "3", "--criteria", hides])
        .await;

    assert_eq!(neither.code, Some(2), "{}", neither.stderr);
    assert_eq!(taken.code, Some(1), "{}", taken.stderr);
    assert_eq!(
        taken.stderr,
        "lw: a task named columns already exists in board\n"
    );
    assert_eq!(white_space.code, Some(1), "{}", white_space.stderr);
    assert_eq!(
        white_space.stderr,
        "lw: the criteria are required: what a good result is, not only white space\n"
    );
    assert_eq!(unread.code, Some(1), "{}", unread.stderr);
    assert!(
        unread.stderr.starts_with("lw: reading the criteria: "),
        "{}",
        unread.stderr
    );
    assert_eq!(no_benchmark.code, Some(4), "{}", no_benchmark.stderr);
    assert_eq!(no_benchmark.stderr, "lw: there is no benchmark 3\n");
    assert_eq!(other_benchmark.code, Some(4), "{}", other_benchmark.stderr);
    assert_eq!(
        other_benchmark.stderr,
        "lw: the benchmark 2 has no task 1\n"
    );
    assert_eq!(no_task.code, Some(4), "{}", no_task.stderr);
    assert_eq!(no_task.stderr, "lw: the benchmark 1 has no task 3\n");
    let shown = serve.lw(&["benchmark", "show", "1"]).await;
    assert_eq!(
        shown.stdout,
        "benchmark: 1 (board)\nstage: implement\n\
         task 1 (lanes): captured from 4/implement-1\n  Stacks them.\n\
         task 2 (columns): captured from 3/implement-2\n  Names them.\n\
         benchmark runs: none\n"
    );
    let mut left: Vec<String> = std::fs::read_dir(repo.join("tasks/1"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with('.'))
        .collect();
    left.sort();
    assert_eq!(left, Vec::<String>::new());
}
