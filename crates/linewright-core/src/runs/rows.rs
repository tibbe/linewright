//! The rows that keep the work directory's runs in its database: a run in
//! `runs`, each of its sessions in `sessions`, and its input in
//! `run_inputs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row as _, SqliteConnection, SqliteExecutor, SqlitePool};

use super::files::{End, Record, RecordedSession};
use super::{Ran, Recorded, RunId, RunOf, SessionEntry, SessionKept, Started, StartedBy, State};
use crate::cost::SessionCost;
use crate::database::{Database, parse_time, time_text};
use crate::domain::Number;
use crate::input::Input;
use crate::workflow_run::AttemptId;

/// What a write of a run changes of its rows.
#[derive(Clone, Copy)]
pub(super) enum Change {
    /// Its session at this index among its record's.
    Session(usize),
    Answered,
    /// Its end, and every session as the record has it.
    EndAndSessions,
}

/// A run's row in `runs`, in the database `db`.
#[derive(Clone)]
pub(super) struct RunRow {
    db: Database,
    id: i64,
}

impl RunRow {
    pub(super) fn new(db: Database, id: i64) -> RunRow {
        RunRow { db, id }
    }

    /// Writes what `change` changes of the run, as `record` has it.
    pub(super) async fn write(&self, record: &Record, change: Change) -> Result<()> {
        let pool = self.db.pool();
        match change {
            Change::Session(index) => {
                let mut conn = pool.acquire().await.context("recording a session")?;
                write_session(&mut conn, self.id, record, index).await
            }
            Change::Answered => {
                sqlx::query("UPDATE runs SET answered_until = ? WHERE id = ?")
                    .bind(record.answered_until.map(time_text))
                    .bind(self.id)
                    .execute(pool)
                    .await
                    .context("recording what the run answered")?;
                Ok(())
            }
            Change::EndAndSessions => {
                let mut tx = pool.begin().await.context("recording the run's end")?;
                let (ended_at, ending) = end_columns(record.end)?;
                sqlx::query("UPDATE runs SET ended_at = ?, ending = ? WHERE id = ?")
                    .bind(ended_at)
                    .bind(ending)
                    .bind(self.id)
                    .execute(&mut *tx)
                    .await
                    .context("recording the run's end")?;
                for index in 0..record.sessions.len() {
                    write_session(&mut tx, self.id, record, index).await?;
                }
                tx.commit().await.context("recording the run's end")
            }
        }
    }

    /// Keeps `input` as what the run was given.
    pub(super) async fn write_input(&self, input: &Input) -> Result<()> {
        let mut conn = self
            .db
            .pool()
            .acquire()
            .await
            .context("recording a run's input")?;
        write_input(&mut conn, self.id, input).await
    }
}

/// What a run is of, as the columns of `runs` hold it.
struct OfColumns {
    kind: &'static str,
    item: Option<i64>,
    stage: Option<&'static str>,
    automation: Option<String>,
}

const ITEM: &str = "item";
const AUTOMATION: &str = "automation";

/// Picks the runs of what [`OfColumns`] binds, in its order.
const WHERE_OF: &str = "kind = ? AND item IS ? AND stage IS ? AND automation IS ?";

impl OfColumns {
    fn of(of: &RunOf) -> Result<OfColumns> {
        Ok(match of {
            RunOf::Work { item, stage } => OfColumns {
                kind: ITEM,
                item: Some(
                    i64::try_from(item.0)
                        .with_context(|| format!("{item} is beyond what the database keeps"))?,
                ),
                stage: Some(stage.name()),
                automation: None,
            },
            RunOf::Automation { automation } => OfColumns {
                kind: AUTOMATION,
                item: None,
                stage: None,
                automation: Some(automation.to_string()),
            },
        })
    }

    /// What the run of `row` is of.
    fn read(row: &SqliteRow) -> Result<RunOf> {
        match row.try_get::<&str, _>("kind")? {
            ITEM => Ok(RunOf::Work {
                item: Number(u64::try_from(row.try_get::<i64, _>("item")?)?),
                stage: row
                    .try_get::<&str, _>("stage")?
                    .parse()
                    .map_err(anyhow::Error::msg)?,
            }),
            AUTOMATION => Ok(RunOf::Automation {
                automation: row
                    .try_get::<&str, _>("automation")?
                    .parse()
                    .map_err(anyhow::Error::msg)?,
            }),
            other => anyhow::bail!("{other:?} is not what a run is of"),
        }
    }
}

/// How an automation's run started, as the columns of `runs` hold it.
struct StartedByColumns {
    started_by: Option<&'static str>,
    /// `workflow_run` and `run_attempt`, of a run an attempt started.
    attempt: Option<(i64, i64)>,
}

const WORKFLOW_RUN: &str = "workflow-run";

impl StartedByColumns {
    fn of(by: Option<StartedBy>) -> Result<StartedByColumns> {
        let (started_by, attempt) = match by {
            None => (None, None),
            Some(StartedBy::Schedule) => (Some("schedule"), None),
            Some(StartedBy::Hand) => (Some("hand"), None),
            Some(StartedBy::WorkflowRun(attempt)) => (Some(WORKFLOW_RUN), Some(attempt.columns()?)),
        };
        Ok(StartedByColumns {
            started_by,
            attempt,
        })
    }

    /// How the run of `row` started.
    fn read(row: &SqliteRow) -> Result<Option<StartedBy>> {
        Ok(match row.try_get::<Option<&str>, _>("started_by")? {
            None => None,
            Some(WORKFLOW_RUN) => Some(StartedBy::WorkflowRun(AttemptId::of_columns(
                row.try_get("workflow_run")?,
                row.try_get("run_attempt")?,
            )?)),
            Some(by) => Some(parse_enum(by)?),
        })
    }
}

/// The next number of a run of `of`: one after every run of it kept.
pub(super) async fn next_n(conn: &mut SqliteConnection, of: &RunOf) -> Result<u32> {
    let of = OfColumns::of(of)?;
    let last: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT max(n) FROM runs WHERE {WHERE_OF}"
    )))
    .bind(of.kind)
    .bind(of.item)
    .bind(of.stage)
    .bind(of.automation)
    .fetch_one(conn)
    .await
    .context("numbering a run")?;
    u32::try_from(last.unwrap_or(0) + 1).context("numbering a run")
}

/// Inserts the run `record` and its sessions, and gives its row's ID.
pub(super) async fn insert(conn: &mut SqliteConnection, record: &Record) -> Result<i64> {
    let of = OfColumns::of(&record.of)?;
    let (ended_at, ending) = end_columns(record.end)?;
    let by = StartedByColumns::of(record.started_by)?;
    let row: i64 = sqlx::query_scalar(
        "INSERT INTO runs
            (kind, item, stage, automation, n, started_by, workflow_run, run_attempt, started_at,
                ended_at, ending, answered_until)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        RETURNING id",
    )
    .bind(of.kind)
    .bind(of.item)
    .bind(of.stage)
    .bind(of.automation)
    .bind(record.n)
    .bind(by.started_by)
    .bind(by.attempt.map(|(run, _)| run))
    .bind(by.attempt.map(|(_, attempt)| attempt))
    .bind(time_text(record.started_at))
    .bind(ended_at)
    .bind(ending)
    .bind(record.answered_until.map(time_text))
    .fetch_one(&mut *conn)
    .await
    .context("inserting a run")?;
    for index in 0..record.sessions.len() {
        write_session(&mut *conn, row, record, index).await?;
    }
    Ok(row)
}

/// The `ended_at` and `ending` of `end`.
fn end_columns(end: Option<End>) -> Result<(Option<String>, Option<String>)> {
    Ok(match end {
        Some(End { at, ending }) => (Some(time_text(at)), Some(enum_text(&ending)?)),
        None => (None, None),
    })
}

/// Writes the session at `index` of `record`, the run kept at `row`.
async fn write_session(
    conn: &mut SqliteConnection,
    row: i64,
    record: &Record,
    index: usize,
) -> Result<()> {
    let (dir, ran, cost) = match &record.sessions[index] {
        RecordedSession::Ran { dir, ran, cost } => (
            dir,
            Some(serde_json::to_string(ran)?),
            cost.as_ref().map(serde_json::to_string).transpose()?,
        ),
        RecordedSession::Dir(dir) => (dir, None, None),
    };
    let dir = dir
        .to_str()
        .with_context(|| format!("the session's directory {} is not UTF-8", dir.display()))?;
    sqlx::query(
        "INSERT INTO sessions (run, seq, dir, ran, cost) VALUES (?, ?, ?, ?, ?)
        ON CONFLICT (run, seq) DO UPDATE
            SET dir = excluded.dir, ran = excluded.ran, cost = excluded.cost",
    )
    .bind(row)
    .bind(i64::try_from(index)?)
    .bind(dir)
    .bind(ran)
    .bind(cost)
    .execute(conn)
    .await
    .context("recording a session")?;
    Ok(())
}

async fn write_input(conn: &mut SqliteConnection, row: i64, input: &Input) -> Result<()> {
    sqlx::query("INSERT OR REPLACE INTO run_inputs (run, input) VALUES (?, ?)")
        .bind(row)
        .bind(serde_json::to_string(input)?)
        .execute(conn)
        .await
        .context("recording a run's input")?;
    Ok(())
}

/// A run's row, with what it is of.
const SELECT_RUNS: &str = "SELECT id, kind, item, stage, automation, n, started_by, workflow_run, \
    run_attempt, started_at, ended_at, ending, answered_until FROM runs";

/// Every run kept, each with no end in flight when `in_flight` says so of
/// it, and interrupted otherwise. Session directories are under `work_dir`.
pub(super) async fn read_all(
    pool: &SqlitePool,
    work_dir: &Path,
    in_flight: impl Fn(&RunId) -> bool,
) -> Result<Vec<Recorded>> {
    let runs = sqlx::query(SELECT_RUNS)
        .fetch_all(pool)
        .await
        .context("reading the runs")?;
    let mut sessions: BTreeMap<i64, Vec<SessionEntry>> = BTreeMap::new();
    for row in sqlx::query("SELECT run, dir, ran, cost FROM sessions ORDER BY run, seq")
        .fetch_all(pool)
        .await
        .context("reading the runs' sessions")?
    {
        sessions
            .entry(row.try_get("run")?)
            .or_default()
            .push(session(&row, work_dir)?);
    }
    runs.iter()
        .map(|row| {
            let sessions = sessions.remove(&row.try_get("id")?).unwrap_or_default();
            recorded(row, sessions, &in_flight)
        })
        .collect()
}

/// Run `id`, as [`read_all`] gives it, `None` if it is not kept: its row
/// and its sessions read in one transaction, so that the two agree.
pub(super) async fn read_one(
    pool: &SqlitePool,
    work_dir: &Path,
    id: &RunId,
    in_flight: impl Fn(&RunId) -> bool,
) -> Result<Option<Recorded>> {
    let mut tx = pool
        .begin()
        .await
        .with_context(|| format!("reading the run {id}"))?;
    let Some(row) = run_row(&mut *tx, id)
        .await
        .with_context(|| format!("reading the run {id}"))?
    else {
        return Ok(None);
    };
    let sessions = sqlx::query("SELECT dir, ran, cost FROM sessions WHERE run = ? ORDER BY seq")
        .bind(row.try_get::<i64, _>("id")?)
        .fetch_all(&mut *tx)
        .await
        .with_context(|| format!("reading the sessions of {id}"))?
        .iter()
        .map(|row| session(row, work_dir))
        .collect::<Result<_>>()?;
    recorded(&row, sessions, &in_flight).map(Some)
}

/// The input run `id` recorded, `None` when it recorded none or is not
/// kept.
pub(super) async fn read_input(pool: &SqlitePool, id: &RunId) -> Result<Option<Input>> {
    let Some(row) = run_row(pool, id).await? else {
        return Ok(None);
    };
    let input: Option<String> = sqlx::query_scalar("SELECT input FROM run_inputs WHERE run = ?")
        .bind(row.try_get::<i64, _>("id")?)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("reading the input of {id}"))?;
    input
        .map(|input| serde_json::from_str(&input))
        .transpose()
        .with_context(|| format!("reading the input of {id}"))
}

async fn run_row(conn: impl SqliteExecutor<'_>, id: &RunId) -> Result<Option<SqliteRow>> {
    let of = OfColumns::of(&id.of)?;
    Ok(sqlx::query(sqlx::AssertSqlSafe(format!(
        "{SELECT_RUNS} WHERE {WHERE_OF} AND n = ?"
    )))
    .bind(of.kind)
    .bind(of.item)
    .bind(of.stage)
    .bind(of.automation)
    .bind(i64::from(id.n))
    .fetch_optional(conn)
    .await?)
}

fn recorded(
    row: &SqliteRow,
    sessions: Vec<SessionEntry>,
    in_flight: impl Fn(&RunId) -> bool,
) -> Result<Recorded> {
    let id = RunId {
        of: OfColumns::read(row)?,
        n: u32::try_from(row.try_get::<i64, _>("n")?)?,
    };
    let started_by = StartedByColumns::read(row)?;
    let ended_at = row.try_get::<Option<&str>, _>("ended_at")?;
    let ending = row.try_get::<Option<&str>, _>("ending")?;
    let state = match (ended_at, ending) {
        (Some(at), Some(ending)) => State::Ended {
            at: parse_time(at)?,
            ending: parse_enum(ending)?,
        },
        _ if in_flight(&id) => State::InFlight,
        _ => State::Interrupted,
    };
    Ok(Recorded::new(
        Started::new(id, started_by).map_err(anyhow::Error::msg)?,
        parse_time(row.try_get("started_at")?)?,
        state,
        sessions,
        row.try_get::<Option<&str>, _>("answered_until")?
            .map(parse_time)
            .transpose()?,
    ))
}

fn session(row: &SqliteRow, work_dir: &Path) -> Result<SessionEntry> {
    let dir = work_dir.join(PathBuf::from(row.try_get::<&str, _>("dir")?));
    let ran = row
        .try_get::<Option<&str>, _>("ran")?
        .map(serde_json::from_str::<Ran>)
        .transpose()
        .with_context(|| format!("reading what the session in {} ran with", dir.display()))?;
    let cost = row
        .try_get::<Option<&str>, _>("cost")?
        .map(serde_json::from_str::<SessionCost>)
        .transpose()
        .with_context(|| format!("reading what the session in {} cost", dir.display()))?;
    let kept = ran.map(|ran| SessionKept { ran, cost });
    Ok(SessionEntry { dir, kept })
}

/// The text a unit variant of an enum serializes as.
fn enum_text(value: &impl Serialize) -> Result<String> {
    let value = serde_json::to_value(value)?;
    let serde_json::Value::String(text) = value else {
        anyhow::bail!("{value} does not serialize as text");
    };
    Ok(text)
}

/// The enum whose unit variant serializes as `text`.
fn parse_enum<T: DeserializeOwned>(text: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .with_context(|| format!("{text:?} does not read"))
}
