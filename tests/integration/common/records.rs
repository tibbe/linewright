use std::panic::Location;
use std::path::Path;

use linewright::runs::RepoWorkDir;

use super::{REPO, write_files};

/// A work directory whose repository's directory holds each of `files`, a
/// path under it and what it holds, as runs, or an earlier version, left
/// them.
pub fn work_dir_holding(files: &[(&str, &str)]) -> (tempfile::TempDir, RepoWorkDir) {
    let base = tempfile::tempdir().unwrap();
    write_files(&base.path().join(REPO), files);
    let work_dir = RepoWorkDir {
        base: base.path().to_path_buf(),
        repo: REPO.parse().unwrap(),
    };
    (base, work_dir)
}

/// The record of the run of `stage` on `item` numbered `n`, started at
/// 2026-10-14 12:00 and, if `ending` is given, ended so a minute later,
/// whose sessions are `sessions`.
pub fn item_run_record(
    item: u64,
    stage: &str,
    n: u32,
    ending: Option<&str>,
    sessions: &serde_json::Value,
) -> String {
    serde_json::json!({
        "item": item,
        "stage": stage,
        "n": n,
        "started_at": "2026-10-14T12:00:00Z",
        "end": ending.map(|ending| serde_json::json!({"at": "2026-10-14T12:01:00Z", "ending": ending})),
        "sessions": sessions,
    })
    .to_string()
}

/// Holds the lock of the run whose record is `record`, as its process does
/// while the run is in flight, until it is dropped.
pub fn lock_of(record: &Path) -> std::fs::File {
    let lock = std::fs::File::create(record.with_extension("lock")).unwrap();
    lock.try_lock().unwrap();
    lock
}

/// `events` as a session's output holds them, one to a line.
pub fn output_of(events: &[serde_json::Value]) -> String {
    events.iter().map(|event| format!("{event}\n")).collect()
}

/// The record of each session of each run `world` recorded, in the order of
/// its runs' records.
#[track_caller]
pub fn session_records(
    world: &super::World,
) -> impl Future<Output = Vec<linewright::transcript::SessionRecord>> + '_ {
    let caller = Location::caller();
    async move {
        let runs = linewright::runs::Runs::new(&world.dir.path().join("work"))
            .read()
            .await
            .unwrap_or_else(|e| panic!("reading the runs at {caller}: {e:#}"));
        runs.iter()
            .flat_map(|run| &run.sessions)
            .map(|session| {
                linewright::transcript::SessionRecord::read(&session.dir).unwrap_or_else(|e| {
                    panic!(
                        "reading the session in {} at {caller}: {e:#}",
                        session.dir.display()
                    )
                })
            })
            .collect()
    }
}

/// The events of each session of each run `world` recorded, in the order of
/// its runs' records, each as its session's transcript holds it.
#[track_caller]
pub fn session_events(
    world: &super::World,
) -> impl Future<Output = Vec<Vec<serde_json::Value>>> + '_ {
    let caller = Location::caller();
    let records = session_records(world);
    async move {
        records
            .await
            .into_iter()
            .map(|record| {
                record
                    .output
                    .lines()
                    .map(|line| {
                        serde_json::from_str(line)
                            .unwrap_or_else(|e| panic!("an event at {caller}: {e}"))
                    })
                    .collect()
            })
            .collect()
    }
}
