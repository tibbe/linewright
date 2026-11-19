//! Exporting a run, through the system: `lw export ID DIR` writes it as the
//! output of a trial gives its run (`docs/specs/runs.md`, `lw export`).

use crate::common;
use crate::trial::triage_task;

use std::collections::BTreeSet;
use std::panic::Location;
use std::path::{Path, PathBuf};

use common::*;
use linewright::export;
use linewright::task::Task;
use linewright::trial::scorer::{self, Scoreable, TrialCopy};
use serde_json::{Value, json};

/// The paths of the files under `dir`, relative to it.
#[track_caller]
fn files_under(dir: &Path) -> BTreeSet<PathBuf> {
    let mut files = BTreeSet::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        for entry in std::fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(path.strip_prefix(dir).unwrap().to_path_buf());
            }
        }
    }
    files
}

#[track_caller]
fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test_log::test(tokio::test)]
async fn an_exported_run_is_read_by_the_scorers_copy_as_a_trials_output() {
    let mut world = World::new().await;
    let (issue, task_dir) = triage_task(&mut world).await;
    let work = world.dir.path().join("work");
    let under = tempfile::tempdir().unwrap();
    let out = under.path().join("out");
    let id = format!("{}/triage-1", issue.0);

    export::run(Some(&runs_in(&work).await), &id.parse().unwrap(), &out)
        .await
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(out.join("outcome")).unwrap(),
        "done\n"
    );
    let record = read_json(&out.join(format!("runs/items/{}/triage-1.json", issue.0)));
    assert_eq!(record["end"]["ending"], "done");
    let [session] = record["sessions"].as_array().unwrap().as_slice() else {
        panic!("one session: {record:#}");
    };
    let session = session["dir"].as_str().unwrap();
    assert!(session.starts_with("sessions/"), "{session}");
    let exported = files_under(&out.join(session));
    assert!(!exported.is_empty());
    assert_eq!(exported, files_under(&work.join(session)));
    std::fs::write(out.join("case"), format!("{id}\n")).unwrap();
    let task = Task::open(&task_dir).await.unwrap();
    let scoreable = scorer::ScoreableTrial::open(&out, &task).await.unwrap();
    assert_eq!(scoreable.outcome(), Scoreable::Done);
    let into = tempfile::tempdir().unwrap();
    let copy = TrialCopy::make(&scoreable, into.path()).await.unwrap();
    assert_eq!(std::fs::read_to_string(copy.outcome()).unwrap(), "done\n");
    let copied = copy
        .sessions()
        .join(session.strip_prefix("sessions/").unwrap());
    assert_eq!(files_under(&copied), exported);
}

/// A work directory for `lw_in`, whose repository's work directory keeps
/// the run 1/triage-1, ended done, with one session holding a brief.
#[track_caller]
fn triaged_in_work_dir() -> impl Future<Output = tempfile::TempDir> {
    let caller = Location::caller();
    async move {
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
            .unwrap_or_else(|e| panic!("writing the session at {caller}: {e}"));
        base
    }
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
