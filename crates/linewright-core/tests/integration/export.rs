//! Exporting a run, through the system: `lw export ID DIR` writes it as the
//! output of a trial gives its run (`docs/specs/runs.md`, `lw export`).

use std::path::Path;

use linewright_core::export;
use linewright_core::task::Task;
use linewright_core::testing::exports::*;
use linewright_core::testing::*;
use linewright_core::trial::scorer::{self, Scoreable, TrialCopy};
use serde_json::Value;

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
    std::fs::write(out.join("task"), format!("{id}\n")).unwrap();
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
