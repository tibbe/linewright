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
