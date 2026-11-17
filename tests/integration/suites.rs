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
async fn lw_suites_lists_each_suite_with_its_stage_and_how_many_cases() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_suite(&repo, "board", "implement").await;
    keep_suite(&repo, "triage-basics", "triage").await;
    keep_case(&repo, "board", "columns", "3/implement-1", "Names them.\n").await;
    keep_case(&repo, "board", "lanes", "4/implement-1", "Stacks them.\n").await;

    let text = lw_in(work_dir.path(), &["suites"]).await;
    let json = lw_in(work_dir.path(), &["suites", "--json"]).await;

    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert_eq!(
        text.stdout,
        "board: implement, 2 cases\ntriage-basics: triage, no cases\n"
    );
    assert_eq!(json.code, Some(0), "{}", json.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json.stdout).unwrap(),
        json!({"suites": [
            {"name": "board", "stage": "implement", "cases": 2},
            {"name": "triage-basics", "stage": "triage", "cases": 0},
        ]})
    );
}

#[test_log::test(tokio::test)]
async fn lw_suites_with_no_database_lists_none() {
    let work_dir = tempfile::tempdir().unwrap();

    let output = lw_in(work_dir.path(), &["suites"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, "suites: none\n");
}

#[test_log::test(tokio::test)]
async fn lw_suite_add_makes_a_suite_of_its_stage() {
    let work_dir = tempfile::tempdir().unwrap();

    let output = lw_in(work_dir.path(), &["suite", "add", "board", "design"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, "Made the suite board, of design\n");
    let listed = lw_in(work_dir.path(), &["suites"]).await;
    assert_eq!(listed.stdout, "board: design, no cases\n");
}

#[test_log::test(tokio::test)]
async fn lw_suite_add_of_a_name_a_suite_has_exits_1_saying_why() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_suite(&repo, "board", "implement").await;

    let output = lw_in(work_dir.path(), &["suite", "add", "board", "design"]).await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert_eq!(output.stderr, "lw: a suite is already named board\n");
    let listed = lw_in(work_dir.path(), &["suites"]).await;
    assert_eq!(listed.stdout, "board: implement, no cases\n");
}

#[test_log::test(tokio::test)]
async fn lw_suite_add_of_a_stage_no_case_has_or_a_name_not_a_suites_is_a_command_line_error() {
    let work_dir = tempfile::tempdir().unwrap();

    let rework = lw_in(work_dir.path(), &["suite", "add", "board", "rework"]).await;
    let named = lw_in(work_dir.path(), &["suite", "add", "Board", "design"]).await;

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
async fn lw_suite_shows_its_cases_with_their_criteria() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_suite(&repo, "board", "implement").await;
    keep_case(
        &repo,
        "board",
        "lanes",
        "4/implement-1",
        "Stacks them.\n\nOn a phone.\n",
    )
    .await;
    keep_case(&repo, "board", "columns", "3/implement-2", "Names them.\n").await;

    let text = lw_in(work_dir.path(), &["suite", "board"]).await;
    let json = lw_in(work_dir.path(), &["suite", "board", "--json"]).await;

    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert_eq!(
        text.stdout,
        "suite: board\nstage: implement\n\
         case columns: captured from 3/implement-2\n  Names them.\n\
         case lanes: captured from 4/implement-1\n  Stacks them.\n\n  On a phone.\n\
         experiments: none\n"
    );
    assert_eq!(json.code, Some(0), "{}", json.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json.stdout).unwrap(),
        json!({
            "name": "board",
            "stage": "implement",
            "cases": [
                {"name": "columns", "run": "3/implement-2", "criteria": "Names them.\n"},
                {"name": "lanes", "run": "4/implement-1", "criteria": "Stacks them.\n\nOn a phone.\n"},
            ],
            "experiments": [],
        })
    );
}

#[test_log::test(tokio::test)]
async fn lw_suite_says_when_a_cases_criteria_do_not_read() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_suite(&repo, "board", "implement").await;
    keep_case(&repo, "board", "lanes", "4/implement-1", " \n").await;

    let text = lw_in(work_dir.path(), &["suite", "board"]).await;
    let json = lw_in(work_dir.path(), &["suite", "board", "--json"]).await;

    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert_eq!(
        text.stdout,
        "suite: board\nstage: implement\n\
         case lanes: captured from 4/implement-1\n  \
         the criteria do not read: criteria.md holds only white space\n\
         experiments: none\n"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json.stdout).unwrap()["cases"][0],
        json!({
            "name": "lanes",
            "run": "4/implement-1",
            "unreadable": "criteria.md holds only white space",
        })
    );
}

#[test_log::test(tokio::test)]
async fn lw_suite_of_a_suite_with_no_cases_says_so() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_suite(&repo, "board", "triage").await;

    let output = lw_in(work_dir.path(), &["suite", "board"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        output.stdout,
        "suite: board\nstage: triage\ncases: none\nexperiments: none\n"
    );
}

#[test_log::test(tokio::test)]
async fn lw_suite_of_a_suite_that_does_not_exist_exits_4() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_suite(&repo, "board", "triage").await;

    let missing = lw_in(work_dir.path(), &["suite", "lanes"]).await;
    let no_database = lw_in(tempfile::tempdir().unwrap().path(), &["suite", "lanes"]).await;

    for output in [missing, no_database] {
        assert_eq!(output.code, Some(4), "{}", output.stderr);
        assert_eq!(output.stderr, "lw: there is no suite lanes\n");
    }
}

#[test_log::test(tokio::test)]
async fn lw_suite_remove_case_removes_the_case_and_its_directory() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_suite(&repo, "board", "implement").await;
    keep_case(&repo, "board", "columns", "3/implement-2", "Names them.\n").await;
    keep_case(&repo, "board", "lanes", "4/implement-1", "Stacks them.\n").await;

    let output = lw_in(work_dir.path(), &["suite", "remove-case", "board", "lanes"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        output.stdout,
        "Removed the case lanes from the suite board\n"
    );
    let listed = lw_in(work_dir.path(), &["suites"]).await;
    assert_eq!(listed.stdout, "board: implement, 1 case\n");
    let left: Vec<String> = std::fs::read_dir(repo.join("cases"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(left, ["columns"]);
}

#[test_log::test(tokio::test)]
async fn lw_suite_remove_case_of_a_suite_or_case_that_does_not_exist_exits_4() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_suite(&repo, "board", "implement").await;
    keep_suite(&repo, "designs", "design").await;
    keep_case(&repo, "board", "lanes", "4/implement-1", "Stacks them.\n").await;

    let no_suite = lw_in(
        work_dir.path(),
        &["suite", "remove-case", "nowhere", "lanes"],
    )
    .await;
    let no_case = lw_in(
        work_dir.path(),
        &["suite", "remove-case", "board", "columns"],
    )
    .await;
    let other_suite = lw_in(
        work_dir.path(),
        &["suite", "remove-case", "designs", "lanes"],
    )
    .await;

    assert_eq!(no_suite.code, Some(4), "{}", no_suite.stderr);
    assert_eq!(no_suite.stderr, "lw: there is no suite nowhere\n");
    assert_eq!(no_case.code, Some(4), "{}", no_case.stderr);
    assert_eq!(no_case.stderr, "lw: the suite board has no case columns\n");
    assert_eq!(other_suite.code, Some(4), "{}", other_suite.stderr);
    assert_eq!(
        other_suite.stderr,
        "lw: the suite designs has no case lanes\n"
    );
    assert!(
        repo.join("cases")
            .join("lanes")
            .join("criteria.md")
            .exists()
    );
    let listed = lw_in(work_dir.path(), &["suites"]).await;
    assert_eq!(
        listed.stdout,
        "board: implement, 1 case\ndesigns: design, no cases\n"
    );
}

#[test_log::test(tokio::test)]
async fn lw_suite_add_and_remove_case_answer_in_json() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_suite(&repo, "board", "implement").await;
    keep_case(&repo, "board", "lanes", "4/implement-1", "Stacks them.\n").await;

    let added = lw_in(
        work_dir.path(),
        &["suite", "add", "designs", "design", "--json"],
    )
    .await;
    let removed = lw_in(
        work_dir.path(),
        &["suite", "remove-case", "board", "lanes", "--json"],
    )
    .await;

    assert_eq!(added.code, Some(0), "{}", added.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&added.stdout).unwrap(),
        json!({"name": "designs", "stage": "design"})
    );
    assert_eq!(removed.code, Some(0), "{}", removed.stderr);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&removed.stdout).unwrap(),
        json!({"suite": "board", "case": "lanes"})
    );
}

#[test_log::test(tokio::test)]
async fn lw_suite_remove_case_removes_a_case_whose_directory_is_gone() {
    let work_dir = tempfile::tempdir().unwrap();
    let repo = made(work_dir.path()).await;
    keep_suite(&repo, "board", "implement").await;
    keep_case_row(&repo, "board", "lanes", "4/implement-1").await;

    let output = lw_in(work_dir.path(), &["suite", "remove-case", "board", "lanes"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let listed = lw_in(work_dir.path(), &["suites"]).await;
    assert_eq!(listed.stdout, "board: implement, no cases\n");
}
