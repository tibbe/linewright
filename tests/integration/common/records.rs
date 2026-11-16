use std::panic::Location;
use std::path::Path;

use linewright::database::Database;
use linewright::runs::{RepoWorkDir, Runs};

use super::{REPO, copy_dir, write_files};

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

/// The runs of the database of `work_dir`, made if there is none, which
/// imports the runs kept there as files.
#[track_caller]
pub fn runs_in(work_dir: &Path) -> impl Future<Output = Runs> + use<> {
    let caller = Location::caller();
    let work_dir = work_dir.to_path_buf();
    async move {
        let db = Database::open(&work_dir)
            .await
            .unwrap_or_else(|e| panic!("opening the database at {caller}: {e:#}"));
        Runs::new(db)
    }
}

/// The database of `work_dir`, which must exist, opened as no handle of
/// Linewright's: for the back door.
#[track_caller]
pub fn back_door(work_dir: &Path) -> impl Future<Output = sqlx::SqlitePool> + use<> {
    let caller = Location::caller();
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(work_dir.join("linewright.db"))
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    async move {
        sqlx::SqlitePool::connect_with(options)
            .await
            .unwrap_or_else(|e| panic!("opening the database at {caller}: {e}"))
    }
}

/// Keeps `record`, a run's record in the form a run's record file has, in
/// the database of `work_dir` as Linewright keeps it, in place of any
/// record of the same run: as the run would have kept it.
#[track_caller]
pub fn keep_record(
    work_dir: &Path,
    record: &serde_json::Value,
) -> impl Future<Output = ()> + use<> {
    let caller = Location::caller();
    let pool = back_door(work_dir);
    let record = record.clone();
    async move {
        let unusable = |what: &str| -> ! { panic!("the record's {what} at {caller}: {record}") };
        let text = |key: &str| record[key].as_str().map(str::to_owned);
        let n = record["n"].as_i64().unwrap_or_else(|| unusable("n"));
        let (kind, item, stage, started_by) = match record["automation"].as_str() {
            Some(_) => (
                "automation",
                None,
                None,
                Some(text("started_by").unwrap_or_else(|| "schedule".into())),
            ),
            None => ("item", record["item"].as_i64(), text("stage"), None),
        };
        let pool = pool.await;
        let mut tx = pool
            .begin()
            .await
            .unwrap_or_else(|e| panic!("keeping a run's record at {caller}: {e}"));
        sqlx::query(
            "DELETE FROM runs WHERE kind = ? AND item IS ? AND stage IS ? AND automation IS ? AND n = ?",
        )
        .bind(kind)
        .bind(item)
        .bind(&stage)
        .bind(record["automation"].as_str())
        .bind(n)
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|e| panic!("removing a run's record at {caller}: {e}"));
        let end = &record["end"];
        let row: i64 = sqlx::query_scalar(
            "INSERT INTO runs
                (kind, item, stage, automation, n, started_by, started_at, ended_at, ending, answered_until)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING id",
        )
        .bind(kind)
        .bind(item)
        .bind(&stage)
        .bind(record["automation"].as_str())
        .bind(n)
        .bind(started_by)
        .bind(text("started_at"))
        .bind(end["at"].as_str())
        .bind(end["ending"].as_str())
        .bind(text("answered_until"))
        .fetch_one(&mut *tx)
        .await
        .unwrap_or_else(|e| panic!("keeping a run's record at {caller}: {e}"));
        let sessions = record["sessions"]
            .as_array()
            .unwrap_or_else(|| unusable("sessions"));
        for (seq, session) in sessions.iter().enumerate() {
            let (dir, ran) = match session.as_str() {
                Some(dir) => (dir.to_owned(), None),
                None => {
                    let mut ran = session.clone();
                    let dir = ran
                        .as_object_mut()
                        .and_then(|ran| ran.remove("dir"))
                        .unwrap_or_else(|| unusable("session's directory"));
                    (
                        dir.as_str()
                            .unwrap_or_else(|| unusable("session's directory"))
                            .to_owned(),
                        Some(ran.to_string()),
                    )
                }
            };
            sqlx::query("INSERT INTO sessions (run, seq, dir, ran) VALUES (?, ?, ?, ?)")
                .bind(row)
                .bind(i64::try_from(seq).unwrap_or_else(|_| unusable("sessions")))
                .bind(dir)
                .bind(ran)
                .execute(&mut *tx)
                .await
                .unwrap_or_else(|e| panic!("keeping a session's record at {caller}: {e}"));
        }
        tx.commit()
            .await
            .unwrap_or_else(|e| panic!("keeping a run's record at {caller}: {e}"));
        pool.close().await;
    }
}

/// Keeps `input` as the input the run `id`, kept in the database of
/// `work_dir`, recorded.
#[track_caller]
pub fn keep_input(
    work_dir: &Path,
    id: &str,
    input: &serde_json::Value,
) -> impl Future<Output = ()> + use<> {
    let caller = Location::caller();
    let id: linewright::runs::RunId = id
        .parse()
        .unwrap_or_else(|e| panic!("the run {id} at {caller}: {e}"));
    let linewright::runs::RunOf::Work { item, stage } = id.of else {
        panic!("only an item's run records its input, at {caller}");
    };
    let pool = back_door(work_dir);
    let input = input.to_string();
    async move {
        let pool = pool.await;
        sqlx::query(
            "INSERT OR REPLACE INTO run_inputs (run, input)
            SELECT id, ? FROM runs WHERE kind = 'item' AND item = ? AND stage = ? AND n = ?",
        )
        .bind(input)
        .bind(i64::try_from(item.0).unwrap_or_else(|e| panic!("#{item} at {caller}: {e}")))
        .bind(stage.name())
        .bind(i64::from(id.n))
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("keeping a run's input at {caller}: {e}"));
        pool.close().await;
    }
}

/// How deep a run's record file is under its work directory, as
/// `runs/items/N/<stage>-<n>.json` or `runs/automations/NAME/<n>.json`.
const RECORD_DEPTH: usize = 4;

/// Holds the lock of the run whose record file would be `record`, under
/// `runs/` in a work directory, as its process does while the run is in
/// flight, until it is dropped.
#[track_caller]
pub fn lock_of(record: &Path) -> std::fs::File {
    let work_dir = record
        .ancestors()
        .nth(RECORD_DEPTH)
        .unwrap_or_else(|| panic!("{} is no run's record", record.display()));
    let lock = work_dir
        .join("locks")
        .join(record.strip_prefix(work_dir).unwrap())
        .with_extension("lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let lock = std::fs::File::create(lock).unwrap();
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
        let runs = runs_in(&world.dir.path().join("work"))
            .await
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

/// The build that is the environment in `dir`, a directory under the work
/// directory `work_dir`, as the database there names it: its number, commit,
/// time and fingerprint, as an earlier release kept them in `state.json`.
#[track_caller]
pub fn environment_state(
    work_dir: &Path,
    dir: &str,
) -> impl Future<Output = serde_json::Value> + use<> {
    let caller = Location::caller();
    let pool = back_door(work_dir);
    let dir = dir.to_string();
    async move {
        let (id, commit, at, fingerprint): (i64, String, String, String) = sqlx::query_as(
            "SELECT build, built_from, built_at, fingerprint FROM environments WHERE dir = ?",
        )
        .bind(&dir)
        .fetch_one(&pool.await)
        .await
        .unwrap_or_else(|e| panic!("reading the state of {dir} at {caller}: {e}"));
        serde_json::json!({"id": id, "commit": commit, "at": at, "fingerprint": fingerprint})
    }
}

/// Leaves the environment in `dir` under the work directory `from` in the
/// work directory `to`, as an earlier release that kept its state as files
/// left it: its directory, holding the state `from`'s database names, and
/// that its latest poll gave a setup command.
#[track_caller]
pub fn environment_as_an_earlier_release_left(
    from: &Path,
    dir: &str,
    to: &Path,
) -> impl Future<Output = ()> + use<> {
    let state = environment_state(from, dir);
    let (from, to) = (from.join(dir), to.join(dir));
    async move {
        let state = state.await;
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        copy_dir(&from, &to);
        write_files(
            &to,
            &[
                ("state.json", &state.to_string()),
                ("setup.json", r#"{"setup_command": true}"#),
            ],
        );
    }
}
