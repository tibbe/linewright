use std::path::Path;
use std::sync::Arc;

use super::{FakeClock, REPO, write_files};
use crate::api::{AutomationsLink, BenchmarkRunsLink, EnvironmentLink, Sources};
use crate::clock::Clock;
use crate::database::Database;
use crate::ending::Ends;
use crate::runs::{RepoWorkDir, Runs};
use crate::status::Serving;
use crate::winding_up::WindingUp;

/// A work directory whose repository's directory holds each of `files`, a
/// path under it and what it holds, as runs left them.
pub fn work_dir_holding(files: &[(&str, &str)]) -> (tempfile::TempDir, RepoWorkDir) {
    let base = tempfile::tempdir().unwrap();
    write_files(&base.path().join(REPO), files);
    let work_dir = RepoWorkDir {
        base: base.path().to_path_buf(),
        repo: REPO.parse().unwrap(),
    };
    (base, work_dir)
}

/// A work directory as [`work_dir_holding`] makes it, whose database keeps
/// each of `records`, a run's record in the form a run's record file has.
pub async fn work_dir_keeping(
    records: &[&str],
    files: &[(&str, &str)],
) -> (tempfile::TempDir, RepoWorkDir) {
    let (base, work_dir) = work_dir_holding(files);
    let records: Vec<serde_json::Value> = records
        .iter()
        .map(|record| {
            serde_json::from_str(record).unwrap_or_else(|e| panic!("the record {record}: {e}"))
        })
        .collect();
    let dir = work_dir.resolve();
    drop(
        Database::open(&dir)
            .await
            .unwrap_or_else(|e| panic!("opening the database: {e:#}")),
    );
    for record in &records {
        keep_record(&dir, record).await;
    }
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

/// The runs of the database of `work_dir`, made if there is none.
pub async fn runs_in(work_dir: &Path) -> Runs {
    let db = Database::open(work_dir)
        .await
        .unwrap_or_else(|e| panic!("opening the database: {e:#}"));
    Runs::new(db)
}

/// The database of `work_dir`, which must exist, opened as no handle of
/// Linewright's: for the back door.
pub async fn back_door(work_dir: &Path) -> sqlx::SqlitePool {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(work_dir.join("linewright.db"))
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    sqlx::SqlitePool::connect_with(options)
        .await
        .unwrap_or_else(|e| panic!("opening the database: {e}"))
}

/// Keeps `record`, a run's record in the form a run's record file has, in
/// the database of `work_dir` as Linewright keeps it, in place of any
/// record of the same run: as the run would have kept it.
pub async fn keep_record(work_dir: &Path, record: &serde_json::Value) {
    let unusable = |what: &str| -> ! { panic!("the record's {what}: {record}") };
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
    let pool = back_door(work_dir).await;
    let mut tx = pool
        .begin()
        .await
        .unwrap_or_else(|e| panic!("keeping a run's record: {e}"));
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
    .unwrap_or_else(|e| panic!("removing a run's record: {e}"));
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
    .unwrap_or_else(|e| panic!("keeping a run's record: {e}"));
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
            .unwrap_or_else(|e| panic!("keeping a session's record: {e}"));
    }
    tx.commit()
        .await
        .unwrap_or_else(|e| panic!("keeping a run's record: {e}"));
    pool.close().await;
}

/// Keeps `input` as the input the run `id`, kept in the database of
/// `work_dir`, recorded.
pub async fn keep_input(work_dir: &Path, id: &str, input: &serde_json::Value) {
    let id: crate::runs::RunId = id.parse().unwrap_or_else(|e| panic!("the run {id}: {e}"));
    let crate::runs::RunOf::Work { item, stage } = id.of else {
        panic!("only an item's run records its input");
    };
    let input = input.to_string();
    let pool = back_door(work_dir).await;
    sqlx::query(
        "INSERT OR REPLACE INTO run_inputs (run, input)
        SELECT id, ? FROM runs WHERE kind = 'item' AND item = ? AND stage = ? AND n = ?",
    )
    .bind(input)
    .bind(i64::try_from(item.0).unwrap_or_else(|e| panic!("#{item}: {e}")))
    .bind(stage.name())
    .bind(i64::from(id.n))
    .execute(&pool)
    .await
    .unwrap_or_else(|e| panic!("keeping a run's input: {e}"));
    pool.close().await;
}

/// How deep a run's record file is under its work directory, as
/// `runs/items/N/<stage>-<n>.json` or `runs/automations/NAME/<n>.json`.
const RECORD_DEPTH: usize = 4;

/// Holds the lock of the run whose record file would be `record`, under
/// `runs/` in a work directory, as its process does while the run is in
/// flight, until it is dropped.
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
pub async fn session_records(world: &super::World) -> Vec<crate::transcript::SessionRecord> {
    let runs = runs_in(&world.dir.path().join("work"))
        .await
        .read()
        .await
        .unwrap_or_else(|e| panic!("reading the runs: {e:#}"));
    runs.iter()
        .flat_map(|run| &run.sessions)
        .map(|session| {
            crate::transcript::SessionRecord::read(&session.dir).unwrap_or_else(|e| {
                panic!("reading the session in {}: {e:#}", session.dir.display())
            })
        })
        .collect()
}

/// The events of each session of each run `world` recorded, in the order of
/// its runs' records, each as its session's transcript holds it.
pub async fn session_events(world: &super::World) -> Vec<Vec<serde_json::Value>> {
    session_records(world)
        .await
        .into_iter()
        .map(|record| {
            record
                .output
                .lines()
                .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("an event: {e}")))
                .collect()
        })
        .collect()
}

/// Keeps the benchmark `name` of `stage` in the database of `work_dir`, with
/// the next benchmark's ID, and gives the ID.
pub async fn keep_benchmark(work_dir: &Path, name: &str, stage: &str) -> i64 {
    let pool = back_door(work_dir).await;
    let mut tx = pool.begin().await.unwrap();
    let id: i64 =
        sqlx::query_scalar("UPDATE ids SET last = last + 1 WHERE of = 'benchmarks' RETURNING last")
            .fetch_one(&mut *tx)
            .await
            .unwrap_or_else(|e| panic!("giving a benchmark's ID: {e}"));
    sqlx::query("INSERT INTO benchmarks (id, name, stage) VALUES (?, ?, ?)")
        .bind(id)
        .bind(name)
        .bind(stage)
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|e| panic!("keeping a benchmark: {e}"));
    tx.commit().await.unwrap();
    pool.close().await;
    id
}

/// Keeps the open benchmark run `id` on the benchmark `benchmark` in the
/// database of `work_dir`, with the tasks `tasks` and the configurations
/// named `configurations`, `as-is` first, as a launch keeps its rows, without
/// its trials or its directory.
pub async fn keep_benchmark_run(
    work_dir: &Path,
    id: i64,
    benchmark: i64,
    tasks: &[i64],
    configurations: &[&str],
) {
    let pool = back_door(work_dir).await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO benchmark_runs (id, benchmark, launched_at, repetitions, state)
         VALUES (?, ?, '2026-11-17T12:00:00Z', 1, 'open')",
    )
    .bind(id)
    .bind(benchmark)
    .execute(&mut *tx)
    .await
    .unwrap_or_else(|e| panic!("keeping a benchmark run: {e}"));
    for (seq, task) in tasks.iter().enumerate() {
        sqlx::query("INSERT INTO benchmark_run_tasks (benchmark_run, seq, task) VALUES (?, ?, ?)")
            .bind(id)
            .bind(i64::try_from(seq).unwrap())
            .bind(task)
            .execute(&mut *tx)
            .await
            .unwrap_or_else(|e| panic!("keeping a benchmark run's task: {e}"));
    }
    for (seq, name) in configurations.iter().enumerate() {
        sqlx::query(
            "INSERT INTO benchmark_run_configurations (benchmark_run, seq, name) VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(i64::try_from(seq).unwrap())
        .bind(name)
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|e| panic!("keeping a benchmark run's configuration: {e}"));
    }
    tx.commit().await.unwrap();
    pool.close().await;
}

/// Keeps the task `name` of the benchmark named `benchmark`, captured from
/// `run`, in the database of `work_dir`, with the next task's ID, without its
/// directory, last in the benchmark's order as a capture keeps it, and gives
/// its ID.
pub async fn keep_task_row(work_dir: &Path, benchmark: &str, name: &str, run: &str) -> i64 {
    let pool = back_door(work_dir).await;
    let mut tx = pool.begin().await.unwrap();
    let id: i64 =
        sqlx::query_scalar("UPDATE ids SET last = last + 1 WHERE of = 'tasks' RETURNING last")
            .fetch_one(&mut *tx)
            .await
            .unwrap_or_else(|e| panic!("giving a task's ID: {e}"));
    sqlx::query(
        "INSERT INTO tasks (id, name, benchmark, run, seq)
         SELECT ?1, ?2, benchmarks.id, ?3,
             (SELECT COALESCE(MAX(seq), 0) + 1 FROM tasks WHERE benchmark = benchmarks.id)
         FROM benchmarks WHERE benchmarks.name = ?4",
    )
    .bind(id)
    .bind(name)
    .bind(run)
    .bind(benchmark)
    .execute(&mut *tx)
    .await
    .unwrap_or_else(|e| panic!("keeping a task: {e}"));
    tx.commit().await.unwrap();
    pool.close().await;
    id
}

/// Keeps the task `name` of the benchmark named `benchmark`, captured from
/// `run`, in the database of `work_dir`, with its directory holding
/// `criteria`, and gives its ID.
pub async fn keep_task(
    work_dir: &Path,
    benchmark: &str,
    name: &str,
    run: &str,
    criteria: &str,
) -> i64 {
    let id = keep_task_row(work_dir, benchmark, name, run).await;
    let dir = work_dir.join("tasks").join(id.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("criteria.md"), criteria).unwrap();
    id
}

/// Writes into the directory of the task `id` of `work_dir` what a capture of
/// the run `run`, started at `start`, leaves of its facts: its `task.json`,
/// and its `items.json` holding the run's item with its issue `issue`, as
/// GitHub gave it.
pub fn keep_task_facts(
    work_dir: &Path,
    id: i64,
    run: &str,
    start: &str,
    issue: &serde_json::Value,
) {
    let parsed: crate::runs::RunId = run.parse().unwrap_or_else(|e| panic!("the run {run}: {e}"));
    let crate::runs::RunOf::Work { item, stage } = parsed.of else {
        panic!("only an item's run is captured");
    };
    let dir = work_dir.join("tasks").join(id.to_string());
    let task = serde_json::json!({
        "stage": stage.name(),
        "item": item.0,
        "start": start,
        "run": run,
        "api_version": crate::store::API_VERSION,
    });
    let items = serde_json::json!([{
        "number": item.0,
        "resources": {format!("issues/{}", item.0): issue},
    }]);
    for (file, json) in [("task.json", task), ("items.json", items)] {
        std::fs::write(dir.join(file), json.to_string())
            .unwrap_or_else(|e| panic!("writing the task's {file}: {e}"));
    }
}

/// Sets when the benchmark run `id` of `work_dir` was launched, through the
/// back door, so that what shows it is known.
pub async fn launched_at(work_dir: &Path, id: i64, at: &str) {
    let pool = back_door(work_dir).await;
    sqlx::query("UPDATE benchmark_runs SET launched_at = ? WHERE id = ?")
        .bind(at)
        .bind(id)
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("setting the launch: {e}"));
    pool.close().await;
}

/// Sets the trial `(repetition, task, configuration)` of the benchmark run
/// `id` of `work_dir` to the columns `set`, through the back door, as a
/// run of it under `serve` would leave it.
pub async fn trial(
    work_dir: &Path,
    id: i64,
    (repetition, task, configuration): (i64, i64, i64),
    set: &str,
) {
    let pool = back_door(work_dir).await;
    let statement = format!(
        "UPDATE trials SET {set} WHERE benchmark_run = ? AND repetition = ? AND task_seq = ? AND configuration_seq = ?"
    );
    let updated = sqlx::query(sqlx::AssertSqlSafe(statement))
        .bind(id)
        .bind(repetition)
        .bind(task)
        .bind(configuration)
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("setting the trial: {e}"));
    assert_eq!(updated.rows_affected(), 1, "the trial set");
    pool.close().await;
}

/// What `serve`'s local API answers from before its first poll, over
/// `work_dir`.
pub async fn sources_before_polling(work_dir: RepoWorkDir) -> Sources {
    let runs = runs_in(&work_dir.resolve()).await;
    let clock = Arc::new(FakeClock::new());
    let (_, served) = tokio::sync::watch::channel(None);
    let (_, polling) = tokio::sync::watch::channel(None);
    Sources {
        serving: Serving {
            since: clock.now(),
            max_runs: 4,
        },
        served,
        polling,
        deliveries: None,
        clock,
        work_dir,
        runs,
        ends: Ends::new(),
        environment: EnvironmentLink::default(),
        automations: AutomationsLink::default(),
        benchmark_runs: BenchmarkRunsLink::default(),
        exiting: WindingUp::default().exiting(),
    }
}
