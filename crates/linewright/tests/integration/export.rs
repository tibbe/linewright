//! Exporting a run, through the system: `lw export ID DIR` writes it as the
//! output of a trial gives its run (`docs/specs/runs.md`, `lw export`).

use std::collections::BTreeSet;
use std::path::PathBuf;

use linewright_core::testing::exports::*;
use linewright_core::testing::*;
use serde_json::json;

use crate::programs::*;

/// A work directory for `lw_in`, whose repository's work directory keeps
/// the run 1/triage-1, ended done, with one session holding a brief.
async fn triaged_in_work_dir() -> tempfile::TempDir {
    let base = tempfile::tempdir().unwrap();
    let work = base.path().join(REPO);
    runs_in(&work).await;
    keep_record(
        &work,
        &serde_json::from_str(&item_run_record(
            1,
            "triage",
            1,
            Some("done"),
            &json!(["sessions/1/triage-1"]),
        ))
        .unwrap(),
    )
    .await;
    let session = work.join("sessions/1/triage-1");
    std::fs::create_dir_all(&session)
        .and_then(|()| std::fs::write(session.join("brief.md"), "Triage #1.\n"))
        .unwrap_or_else(|e| panic!("writing the session: {e}"));
    base
}

#[test_log::test(tokio::test)]
async fn lw_export_writes_the_run_without_serve_and_says_where() {
    let work_dir = triaged_in_work_dir().await;
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().join("triage");

    let ran = lw_in(
        work_dir.path(),
        &["export", "1/triage-1", dir.to_str().unwrap()],
    )
    .await;

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    assert_eq!(
        ran.stdout,
        format!("Exported 1/triage-1 into {}\n", dir.display())
    );
    assert_eq!(
        files_under(&dir),
        BTreeSet::from([
            PathBuf::from("outcome"),
            PathBuf::from("runs/items/1/triage-1.json"),
            PathBuf::from("sessions/1/triage-1/brief.md"),
        ])
    );
}

#[test_log::test(tokio::test)]
async fn lw_export_refuses_a_directory_that_holds_anything_as_a_trial_does() {
    let work_dir = triaged_in_work_dir().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("kept"), "").unwrap();

    let ran = lw_in(
        work_dir.path(),
        &["export", "1/triage-1", dir.path().to_str().unwrap()],
    )
    .await;

    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert!(
        ran.stderr.contains(&format!(
            "the output's directory {} holds something: give an empty or missing one",
            dir.path().display()
        )),
        "{}",
        ran.stderr
    );
}

#[test_log::test(tokio::test)]
async fn lw_export_of_an_unknown_run_exits_4_with_or_without_a_database() {
    let kept = triaged_in_work_dir().await;
    let empty = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().join("export");

    for work_dir in [kept.path(), empty.path()] {
        let ran = lw_in(work_dir, &["export", "1/rework-1", dir.to_str().unwrap()]).await;

        assert_eq!(ran.code, Some(4), "{}", ran.stderr);
        assert!(
            ran.stderr.contains("there is no run 1/rework-1"),
            "{}",
            ran.stderr
        );
    }
}

#[test_log::test(tokio::test)]
async fn lw_export_without_a_directory_exits_2() {
    let work_dir = tempfile::tempdir().unwrap();

    let ran = lw_in(work_dir.path(), &["export", "1/triage-1"]).await;

    assert_eq!(ran.code, Some(2), "{}", ran.stderr);
}

#[test_log::test(tokio::test)]
async fn lw_help_lists_export_after_logs() {
    let work_dir = tempfile::tempdir().unwrap();

    let ran = lw_in(work_dir.path(), &["--help"]).await;

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    let commands: Vec<&str> = ran
        .stdout
        .lines()
        .filter_map(|line| line.strip_prefix("  "))
        .filter(|line| !line.starts_with(' ') && !line.starts_with('-'))
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    let logs = commands.iter().position(|command| *command == "logs");
    assert_eq!(
        logs.map(|logs| commands.get(logs + 1)),
        Some(Some(&"export")),
        "{}",
        ran.stdout
    );
}
