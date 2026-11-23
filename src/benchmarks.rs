//! Benchmarks, the tasks they hold and the benchmark runs on them, kept in the
//! database (`docs/specs/benchmarks.md`, "Benchmarks" and "Benchmark runs"):
//! every write to them, each in one transaction.

mod answers;
pub mod results;
mod runs;
mod trial_shown;

pub use answers::{
    BenchmarkRunAnswer, BenchmarkRunSummary, Shown, WaitingFor, dollars, duration, mean, percent,
    render_benchmark_run, without,
};
pub use runs::{
    BENCHMARK_RUNS, BenchmarkRunId, BenchmarkRunRecord, BenchmarkRunState, BenchmarkRuns,
    CancelRefusal, Cancelled, ConfigurationId, LaunchRefusal, Launched, NoScore,
    RenameConfigurationError, Repetitions, RunTask, ScoreFigures, TrialChange, TrialEnd,
    TrialFigures, TrialKey, TrialName, TrialOf, TrialRef, TrialState, as_is_name,
};
pub use trial_shown::TrialShown;

use std::fmt;
use std::fmt::Write as _;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Serialize;
use sqlx::SqliteConnection;

use crate::database::Database;
use crate::name::{self, Name};
use crate::runs::RunId;
use crate::task::{self, CaptureError, Criteria, TaskFacts, TaskRefusal, TaskStage};

/// An ID that Linewright gives: a positive number, the next of its counter
/// (`docs/specs/benchmarks.md`, Names and IDs).
macro_rules! given_id {
    ($(#[$doc:meta])* $id:ident, $whose:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $id(i64);

        impl $id {
            /// The ID a row holds.
            fn from_row(id: i64) -> Result<$id> {
                (id > 0)
                    .then_some($id(id))
                    .with_context(|| format!("{id} is not {} ID", $whose))
            }

            /// The ID as a row holds it.
            fn row(self) -> i64 {
                self.0
            }
        }

        impl std::str::FromStr for $id {
            type Err = String;

            fn from_str(id: &str) -> Result<$id, String> {
                positive(id, $whose).map($id)
            }
        }

        impl fmt::Display for $id {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

/// `id`, an ID of `whose`, such as "a task's": a positive number in digits
/// alone.
fn positive<T: std::str::FromStr + Default + PartialOrd>(
    id: &str,
    whose: &str,
) -> Result<T, String> {
    id.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| id.parse::<T>().ok())
        .flatten()
        .filter(|id| *id > T::default())
        .ok_or_else(|| format!("{id:?} is not {whose} ID: it is a positive number"))
}

given_id!(
    /// A benchmark's ID.
    BenchmarkId,
    "a benchmark's"
);

given_id!(
    /// A task's ID, never given again, also once the task is removed.
    TaskId,
    "a task's"
);

/// A named set of tasks of one stage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Benchmark {
    pub id: BenchmarkId,
    pub name: Name,
    pub stage: TaskStage,
}

/// The benchmark a capture puts its task into, resolved: one there is, or one
/// to make with the task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChosenBenchmark {
    Existing(Benchmark),
    New { name: Name, stage: TaskStage },
}

impl ChosenBenchmark {
    pub fn name(&self) -> &Name {
        match self {
            ChosenBenchmark::Existing(benchmark) => &benchmark.name,
            ChosenBenchmark::New { name, .. } => name,
        }
    }

    pub fn stage(&self) -> TaskStage {
        match self {
            ChosenBenchmark::Existing(benchmark) => benchmark.stage,
            ChosenBenchmark::New { stage, .. } => *stage,
        }
    }
}

/// A benchmark, with how many tasks it holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BenchmarkListed {
    #[serde(flatten)]
    pub benchmark: Benchmark,
    pub tasks: usize,
}

/// A benchmark, with its tasks in its order and its benchmark runs, latest
/// first.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchmarkShown {
    pub benchmark: Benchmark,
    pub tasks: Vec<TaskEntry>,
    pub benchmark_runs: Vec<BenchmarkRunRecord>,
}

/// A task of a benchmark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskEntry {
    pub id: TaskId,
    pub name: Name,
    /// The run it was captured from.
    pub run: RunId,
    /// Its criteria, or why they do not read, such as once a person
    /// emptied them.
    pub criteria: Result<Criteria, TaskRefusal>,
}

/// A task, with its benchmark and what its directory gives of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskShown {
    pub id: TaskId,
    pub name: Name,
    pub benchmark: Benchmark,
    /// The run it was captured from.
    pub run: RunId,
    /// Its stage, item and start, or why they do not read.
    pub facts: Result<TaskFacts, TaskRefusal>,
    /// Its criteria, or why they do not read.
    pub criteria: Result<Criteria, TaskRefusal>,
}

/// A task that was removed: the benchmark it was removed from, and its name
/// then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovedTask {
    pub benchmark: BenchmarkId,
    pub name: Name,
}

/// A task just kept in its benchmark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeptTask {
    pub id: TaskId,
    /// Its benchmark, made with it when it is new.
    pub benchmark: Benchmark,
    /// Its directory.
    pub dir: PathBuf,
}

/// Why a benchmark was not made.
#[derive(Debug, thiserror::Error)]
pub enum MakeError {
    #[error("a benchmark is already named {0}")]
    Taken(Name),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// Why a task was not removed.
#[derive(Debug, thiserror::Error)]
pub enum RemoveError {
    #[error("there is no benchmark {0}")]
    NoSuchBenchmark(BenchmarkId),
    #[error("the benchmark {benchmark} has no task {task}")]
    NoSuchTask {
        benchmark: BenchmarkId,
        task: TaskId,
    },
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// Why a benchmark was not renamed.
#[derive(Debug, thiserror::Error)]
pub enum RenameError {
    #[error("there is no benchmark {0}")]
    NoSuchBenchmark(BenchmarkId),
    #[error("a benchmark is already named {0}")]
    Taken(Name),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// A change to a task: its name, its criteria, or both, never neither.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskChange {
    Name(Name),
    Criteria(Criteria),
    Both(Name, Criteria),
}

impl TaskChange {
    /// The change of `name` and `criteria`, `None` when both are absent.
    pub fn of(name: Option<Name>, criteria: Option<Criteria>) -> Option<TaskChange> {
        match (name, criteria) {
            (Some(name), Some(criteria)) => Some(TaskChange::Both(name, criteria)),
            (Some(name), None) => Some(TaskChange::Name(name)),
            (None, Some(criteria)) => Some(TaskChange::Criteria(criteria)),
            (None, None) => None,
        }
    }

    pub fn name(&self) -> Option<&Name> {
        match self {
            TaskChange::Name(name) | TaskChange::Both(name, _) => Some(name),
            TaskChange::Criteria(_) => None,
        }
    }

    pub fn criteria(&self) -> Option<&Criteria> {
        match self {
            TaskChange::Criteria(criteria) | TaskChange::Both(_, criteria) => Some(criteria),
            TaskChange::Name(_) => None,
        }
    }
}

/// Why a task was not changed.
#[derive(Debug, thiserror::Error)]
pub enum EditTaskError {
    #[error("there is no benchmark {0}")]
    NoSuchBenchmark(BenchmarkId),
    #[error("the benchmark {benchmark} has no task {task}")]
    NoSuchTask {
        benchmark: BenchmarkId,
        task: TaskId,
    },
    #[error("a task named {task} already exists in {benchmark}")]
    Taken { task: Name, benchmark: Name },
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// The benchmarks of a work directory, kept in its database: every handle on
/// one database sees the same benchmarks.
#[derive(Clone, Debug)]
pub struct Benchmarks {
    db: Database,
}

impl Benchmarks {
    pub fn new(db: Database) -> Benchmarks {
        Benchmarks { db }
    }

    /// The directory that holds the tasks, under the work directory.
    pub fn tasks_dir(&self) -> PathBuf {
        self.db.work_dir().join(task::TASKS)
    }

    /// The directory of the task `id`.
    pub fn task_dir(&self, id: TaskId) -> PathBuf {
        self.tasks_dir().join(id.to_string())
    }

    /// Makes the benchmark `name` of `stage`, with the next benchmark's ID,
    /// refused when a benchmark has the name.
    pub async fn make(&self, name: &Name, stage: TaskStage) -> Result<Benchmark, MakeError> {
        let making = || format!("making the benchmark {name}");
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .with_context(making)?;
        let benchmark = insert_benchmark(&mut tx, name, stage).await?;
        tx.commit().await.with_context(making)?;
        Ok(benchmark)
    }

    /// Whether the benchmark `benchmark` holds a task named `name`.
    pub async fn has_task(&self, benchmark: BenchmarkId, name: &Name) -> Result<bool> {
        let row: Option<i64> =
            sqlx::query_scalar("SELECT id FROM tasks WHERE benchmark = ? AND name = ?")
                .bind(benchmark.row())
                .bind(name.as_str())
                .fetch_optional(self.db.pool())
                .await
                .context(READING)?;
        Ok(row.is_some())
    }

    /// Every benchmark, in the order of their names by the Unicode Collation
    /// Algorithm, with how many tasks it holds.
    pub async fn list(&self) -> Result<Vec<BenchmarkListed>> {
        let rows: Vec<(i64, String, String, i64)> = sqlx::query_as(
            "SELECT benchmarks.id, benchmarks.name, benchmarks.stage, COUNT(tasks.id)
             FROM benchmarks
             LEFT JOIN tasks ON tasks.benchmark = benchmarks.id
             GROUP BY benchmarks.id",
        )
        .fetch_all(self.db.pool())
        .await
        .context(READING)?;
        let mut listed = rows
            .into_iter()
            .map(|(id, name, stage, tasks)| {
                Ok(BenchmarkListed {
                    benchmark: benchmark_of(id, &name, &stage)?,
                    tasks: usize::try_from(tasks).context(READING)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        name::collated(&mut listed, |listed| &listed.benchmark.name);
        Ok(listed)
    }

    /// The benchmark `id`, if there is one.
    pub async fn find(&self, id: BenchmarkId) -> Result<Option<Benchmark>> {
        let mut conn = self.db.pool().acquire().await.context(READING)?;
        find_benchmark(&mut conn, id).await
    }

    /// The benchmark named `name`, if there is one.
    pub async fn named(&self, name: &Name) -> Result<Option<Benchmark>> {
        let row: Option<(i64, String)> =
            sqlx::query_as("SELECT id, stage FROM benchmarks WHERE name = ?")
                .bind(name.as_str())
                .fetch_optional(self.db.pool())
                .await
                .context(READING)?;
        row.map(|(id, stage)| benchmark_of(id, name.as_str(), &stage))
            .transpose()
    }

    /// The benchmark `id` with its tasks, in its order, each with its
    /// criteria as its directory holds them now, and its benchmark runs, latest
    /// first; `None` when there is no such benchmark.
    pub async fn show(&self, id: BenchmarkId) -> Result<Option<BenchmarkShown>> {
        let Some(benchmark) = self.find(id).await? else {
            return Ok(None);
        };
        let rows: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT id, name, run FROM tasks WHERE benchmark = ? ORDER BY seq")
                .bind(id.row())
                .fetch_all(self.db.pool())
                .await
                .context(READING)?;
        let tasks = rows
            .into_iter()
            .map(|(task, name, run)| {
                let task = TaskId::from_row(task)?;
                Ok(TaskEntry {
                    id: task,
                    name: Name::parse(&name)?,
                    run: run.parse().map_err(anyhow::Error::msg)?,
                    criteria: task::read_criteria(&self.task_dir(task)),
                })
            })
            .collect::<Result<_>>()
            .context(READING)?;
        let benchmark_runs = BenchmarkRuns::new(self.db.clone()).of_benchmark(id).await?;
        Ok(Some(BenchmarkShown {
            benchmark,
            tasks,
            benchmark_runs,
        }))
    }

    /// The task `id` with its benchmark, and what its directory gives of it
    /// now; `None` when there is no such task, such as once it is removed.
    pub async fn task(&self, id: TaskId) -> Result<Option<TaskShown>> {
        let row: Option<(String, String, i64, String, String)> = sqlx::query_as(
            "SELECT tasks.name, tasks.run, benchmarks.id, benchmarks.name, benchmarks.stage
             FROM tasks JOIN benchmarks ON benchmarks.id = tasks.benchmark
             WHERE tasks.id = ?",
        )
        .bind(id.row())
        .fetch_optional(self.db.pool())
        .await
        .context(READING)?;
        let Some((name, run, benchmark, benchmark_name, stage)) = row else {
            return Ok(None);
        };
        let dir = self.task_dir(id);
        Ok(Some(TaskShown {
            id,
            name: Name::parse(&name).context(READING)?,
            benchmark: benchmark_of(benchmark, &benchmark_name, &stage)?,
            run: run.parse().map_err(anyhow::Error::msg).context(READING)?,
            facts: task::read_facts(&dir),
            criteria: task::read_criteria(&dir),
        }))
    }

    /// The task `id` as it was when it was removed; `None` when no task with
    /// that ID was removed.
    pub async fn removed_task(&self, id: TaskId) -> Result<Option<RemovedTask>> {
        let row: Option<(i64, String)> =
            sqlx::query_as("SELECT benchmark, name FROM removed_tasks WHERE id = ?")
                .bind(id.row())
                .fetch_optional(self.db.pool())
                .await
                .context(READING)?;
        row.map(|(benchmark, name)| -> Result<RemovedTask> {
            Ok(RemovedTask {
                benchmark: BenchmarkId::from_row(benchmark)?,
                name: Name::parse(&name)?,
            })
        })
        .transpose()
        .context(READING)
    }

    /// Removes the task `task` from the benchmark `benchmark`, and with it the
    /// task, and gives its name: its row goes to the removed tasks, with its
    /// name, as its directory goes, or neither does.
    pub async fn remove_task(
        &self,
        benchmark: BenchmarkId,
        task: TaskId,
    ) -> Result<Name, RemoveError> {
        let removing = format!("removing the task {task} from the benchmark {benchmark}");
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .context(removing.clone())?;
        if find_benchmark(&mut tx, benchmark)
            .await
            .context(removing.clone())?
            .is_none()
        {
            return Err(RemoveError::NoSuchBenchmark(benchmark));
        }
        let name: Option<String> =
            sqlx::query_scalar("DELETE FROM tasks WHERE id = ? AND benchmark = ? RETURNING name")
                .bind(task.row())
                .bind(benchmark.row())
                .fetch_optional(&mut *tx)
                .await
                .context(removing.clone())?;
        let Some(name) = name else {
            return Err(RemoveError::NoSuchTask { benchmark, task });
        };
        sqlx::query("INSERT INTO removed_tasks (id, benchmark, name) VALUES (?, ?, ?)")
            .bind(task.row())
            .bind(benchmark.row())
            .bind(&name)
            .execute(&mut *tx)
            .await
            .context(removing.clone())?;
        let name = Name::parse(&name).context(removing.clone())?;
        let tasks_dir = self.tasks_dir();
        let place = self.task_dir(task);
        std::fs::create_dir_all(&tasks_dir)
            .context("creating the tasks' directory")
            .context(removing.clone())?;
        let bin = tempfile::Builder::new()
            .prefix(".removing-")
            .tempdir_in(&tasks_dir)
            .context(removing.clone())?;
        let binned = bin.path().join(task.to_string());
        match std::fs::rename(&place, &binned) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                return Err(anyhow::Error::from(e)
                    .context(format!("moving {} out of the tasks", place.display()))
                    .context(removing)
                    .into());
            }
        }
        if let Err(e) = tx.commit().await {
            if binned.exists()
                && let Err(back) = std::fs::rename(&binned, &place)
            {
                tracing::warn!(task = %task, error = %back, "putting back a task whose removal failed failed");
                let _kept = bin.keep();
            }
            return Err(anyhow::Error::from(e).context(removing).into());
        }
        Ok(name)
    }

    /// Changes the name of the benchmark `id` to `name`, refused when
    /// another benchmark has it. Of two changes at once, the last is kept.
    pub async fn rename(&self, id: BenchmarkId, name: &Name) -> Result<(), RenameError> {
        let renaming = || format!("renaming the benchmark {id} to {name}");
        let updated = sqlx::query("UPDATE benchmarks SET name = ? WHERE id = ?")
            .bind(name.as_str())
            .bind(id.row())
            .execute(self.db.pool())
            .await;
        match updated {
            Ok(done) if done.rows_affected() == 0 => Err(RenameError::NoSuchBenchmark(id)),
            Ok(_) => Ok(()),
            Err(e) if unique_violation(&e) => Err(RenameError::Taken(name.clone())),
            Err(e) => Err(anyhow::Error::from(e).context(renaming()).into()),
        }
    }

    /// Changes the task `task` of the benchmark `benchmark` as `change` says:
    /// in one transaction, its name changes and its criteria file is replaced
    /// by a rename, so that both are kept or neither is. Refused as
    /// [`EditTaskError::Taken`] when another task of the benchmark has the
    /// name. Of two changes at once, the last is kept.
    pub async fn edit_task(
        &self,
        benchmark: BenchmarkId,
        task: TaskId,
        change: &TaskChange,
    ) -> Result<(), EditTaskError> {
        let editing = format!("changing the task {task} of the benchmark {benchmark}");
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .context(editing.clone())?;
        let Some(found) = find_benchmark(&mut tx, benchmark)
            .await
            .context(editing.clone())?
        else {
            return Err(EditTaskError::NoSuchBenchmark(benchmark));
        };
        let held: Option<i64> =
            sqlx::query_scalar("SELECT id FROM tasks WHERE id = ? AND benchmark = ?")
                .bind(task.row())
                .bind(benchmark.row())
                .fetch_optional(&mut *tx)
                .await
                .context(editing.clone())?;
        if held.is_none() {
            return Err(EditTaskError::NoSuchTask { benchmark, task });
        }
        if let Some(name) = change.name() {
            let updated = sqlx::query("UPDATE tasks SET name = ? WHERE id = ?")
                .bind(name.as_str())
                .bind(task.row())
                .execute(&mut *tx)
                .await;
            match updated {
                Ok(_) => {}
                Err(e) if unique_violation(&e) => {
                    return Err(EditTaskError::Taken {
                        task: name.clone(),
                        benchmark: found.name,
                    });
                }
                Err(e) => return Err(anyhow::Error::from(e).context(editing).into()),
            }
        }
        let replaced = change
            .criteria()
            .map(|criteria| task::replace_criteria(&self.task_dir(task), criteria))
            .transpose()
            .context(editing.clone())?;
        if let Err(e) = tx.commit().await {
            if let Some(replaced) = replaced
                && let Err(back) = replaced.put_back()
            {
                tracing::warn!(task = %task, error = %back, "the criteria of a task whose change failed were not put back");
            }
            return Err(anyhow::Error::from(e).context(editing).into());
        }
        Ok(())
    }

    /// Keeps the task `name`, captured from `run` and built in the
    /// directory `built`, in `chosen`, made first when it is new, with the
    /// next task's ID: in one transaction, the rows are inserted and `built`
    /// is renamed into its place in the tasks, so that all are written or
    /// none is. Refused as [`CaptureError::Taken`] when another task of the
    /// benchmark has the name.
    pub(crate) async fn keep_task(
        &self,
        chosen: &ChosenBenchmark,
        name: &Name,
        run: &RunId,
        built: &Path,
    ) -> Result<KeptTask, CaptureError> {
        let keeping = format!("keeping the task {name} in the benchmark {}", chosen.name());
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .context(keeping.clone())?;
        let benchmark = match chosen {
            ChosenBenchmark::Existing(benchmark) => benchmark.clone(),
            ChosenBenchmark::New { name: new, stage } => {
                match insert_benchmark(&mut tx, new, *stage).await {
                    Ok(benchmark) => benchmark,
                    Err(MakeError::Taken(taken)) => {
                        return Err(CaptureError::BenchmarkTaken(taken));
                    }
                    Err(MakeError::Failed(e)) => return Err(e.context(keeping).into()),
                }
            }
        };
        let tasks_dir = self.tasks_dir();
        let id = next_task_id(&mut tx, &tasks_dir)
            .await
            .context(keeping.clone())?;
        let inserted = sqlx::query(
            "INSERT INTO tasks (id, name, benchmark, run, seq) VALUES (?1, ?2, ?3, ?4,
                 (SELECT COALESCE(MAX(seq), 0) + 1 FROM tasks WHERE benchmark = ?3))",
        )
        .bind(id.row())
        .bind(name.as_str())
        .bind(benchmark.id.row())
        .bind(run.to_string())
        .execute(&mut *tx)
        .await;
        match inserted {
            Ok(_) => {}
            Err(e) if unique_violation(&e) => {
                let benchmark = find_benchmark(&mut tx, benchmark.id)
                    .await
                    .context(keeping)?
                    .map_or(benchmark.name, |now| now.name);
                return Err(CaptureError::Taken {
                    task: name.clone(),
                    benchmark,
                });
            }
            Err(e) if foreign_key_violation(&e) => {
                return Err(CaptureError::NoSuchBenchmark(benchmark.id));
            }
            Err(e) => return Err(anyhow::Error::from(e).context(keeping).into()),
        }
        let place = self.task_dir(id);
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            built,
            rustix::fs::CWD,
            &place,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)
        .with_context(|| format!("moving the task into {}", place.display()))?;
        if let Err(e) = tx.commit().await {
            if let Err(back) = std::fs::rename(&place, built) {
                tracing::warn!(task = %name, error = %back, "taking back a task whose row was not kept failed");
            }
            return Err(anyhow::Error::from(e).context(keeping).into());
        }
        Ok(KeptTask {
            id,
            benchmark,
            dir: place,
        })
    }
}

/// The next task's ID, past the counter's last and past every directory in
/// `tasks_dir` named as one, such as of a task captured into no benchmark,
/// with the counter advanced to it.
async fn next_task_id(conn: &mut SqliteConnection, tasks_dir: &Path) -> Result<TaskId> {
    let mut next: i64 = sqlx::query_scalar("SELECT last FROM ids WHERE of = 'tasks'")
        .fetch_one(&mut *conn)
        .await?;
    loop {
        next = next
            .checked_add(1)
            .context("there is no ID left for a task")?;
        if tasks_dir.join(next.to_string()).symlink_metadata().is_err() {
            break;
        }
    }
    sqlx::query("UPDATE ids SET last = ? WHERE of = 'tasks'")
        .bind(next)
        .execute(&mut *conn)
        .await?;
    TaskId::from_row(next)
}

/// Inserts the row of the benchmark `name` of `stage` with `conn`, with the
/// next benchmark's ID, refused when a benchmark has its name.
async fn insert_benchmark(
    conn: &mut SqliteConnection,
    name: &Name,
    stage: TaskStage,
) -> Result<Benchmark, MakeError> {
    let making = || format!("making the benchmark {name}");
    let id: i64 =
        sqlx::query_scalar("UPDATE ids SET last = last + 1 WHERE of = 'benchmarks' RETURNING last")
            .fetch_one(&mut *conn)
            .await
            .with_context(making)?;
    let inserted = sqlx::query("INSERT INTO benchmarks (id, name, stage) VALUES (?, ?, ?)")
        .bind(id)
        .bind(name.as_str())
        .bind(stage.name())
        .execute(&mut *conn)
        .await;
    match inserted {
        Ok(_) => Ok(Benchmark {
            id: BenchmarkId::from_row(id)?,
            name: name.clone(),
            stage,
        }),
        Err(e) if unique_violation(&e) => Err(MakeError::Taken(name.clone())),
        Err(e) => Err(anyhow::Error::from(e).context(making()).into()),
    }
}

/// The benchmark `id`, if there is one.
async fn find_benchmark(conn: &mut SqliteConnection, id: BenchmarkId) -> Result<Option<Benchmark>> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT name, stage FROM benchmarks WHERE id = ?")
            .bind(id.row())
            .fetch_optional(&mut *conn)
            .await
            .context(READING)?;
    row.map(|(name, stage)| benchmark_of(id.row(), &name, &stage))
        .transpose()
}

const READING: &str = "reading the benchmarks";

/// The benchmark a row holds.
fn benchmark_of(id: i64, name: &str, stage: &str) -> Result<Benchmark> {
    Ok(Benchmark {
        id: BenchmarkId::from_row(id).context(READING)?,
        name: Name::parse(name).context(READING)?,
        stage: stage.parse().map_err(anyhow::Error::msg).context(READING)?,
    })
}

fn unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(e) if e.is_unique_violation())
}

fn foreign_key_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(e) if e.is_foreign_key_violation())
}

/// What `benchmarks` answers.
#[derive(Serialize)]
pub struct BenchmarksAnswer {
    pub benchmarks: Vec<BenchmarkListed>,
}

/// The benchmarks as `benchmarks` prints them: a line for each.
pub fn render_benchmarks(answer: &BenchmarksAnswer) -> String {
    if answer.benchmarks.is_empty() {
        return "benchmarks: none\n".into();
    }
    let mut text = String::new();
    for listed in &answer.benchmarks {
        let tasks = match listed.tasks {
            0 => "no tasks".to_string(),
            1 => "1 task".to_string(),
            n => format!("{n} tasks"),
        };
        let _ = writeln!(
            text,
            "{} ({}): {}, {tasks}",
            listed.benchmark.id, listed.benchmark.name, listed.benchmark.stage
        );
    }
    text
}

/// What `benchmark show BENCHMARK` answers.
#[derive(Serialize)]
pub struct BenchmarkAnswer {
    #[serde(flatten)]
    pub benchmark: Benchmark,
    pub tasks: Vec<TaskAnswer>,
    pub benchmark_runs: Vec<BenchmarkRunSummary>,
}

/// A task as `benchmark show BENCHMARK` answers it: its criteria, or why they
/// do not read.
#[derive(Serialize)]
pub struct TaskAnswer {
    pub id: TaskId,
    pub name: Name,
    pub run: RunId,
    #[serde(flatten)]
    pub criteria: CriteriaAnswer,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CriteriaAnswer {
    Criteria(String),
    /// Why they do not read.
    Unreadable(String),
}

impl BenchmarkAnswer {
    /// `shown`, its benchmark runs in the states `states`, one for each in
    /// order.
    pub fn new(shown: &BenchmarkShown, states: Vec<Shown>) -> BenchmarkAnswer {
        BenchmarkAnswer {
            benchmark: shown.benchmark.clone(),
            tasks: shown
                .tasks
                .iter()
                .map(|task| TaskAnswer {
                    id: task.id,
                    name: task.name.clone(),
                    run: task.run.clone(),
                    criteria: match &task.criteria {
                        Ok(criteria) => CriteriaAnswer::Criteria(criteria.as_str().to_string()),
                        Err(refused) => CriteriaAnswer::Unreadable(refused.why.clone()),
                    },
                })
                .collect(),
            benchmark_runs: shown
                .benchmark_runs
                .iter()
                .zip(states)
                .map(|(record, state)| BenchmarkRunSummary::new(record, state))
                .collect(),
        }
    }
}

/// A benchmark as `benchmark show BENCHMARK` prints it: its facts, then each
/// task with its run, and its criteria indented under it, then each benchmark
/// run, latest first.
pub fn render_benchmark(answer: &BenchmarkAnswer) -> String {
    let mut text = format!(
        "benchmark: {} ({})\nstage: {}\n",
        answer.benchmark.id, answer.benchmark.name, answer.benchmark.stage
    );
    if answer.tasks.is_empty() {
        text.push_str("tasks: none\n");
    }
    for task in &answer.tasks {
        let _ = writeln!(
            text,
            "task {} ({}): captured from {}",
            task.id, task.name, task.run
        );
        match &task.criteria {
            CriteriaAnswer::Criteria(criteria) => {
                for line in criteria.lines() {
                    let indent = if line.is_empty() { "" } else { "  " };
                    let _ = writeln!(text, "{indent}{line}");
                }
            }
            CriteriaAnswer::Unreadable(why) => {
                let _ = writeln!(text, "  the criteria do not read: {why}");
            }
        }
    }
    if answer.benchmark_runs.is_empty() {
        text.push_str("benchmark runs: none\n");
    }
    for benchmark_run in &answer.benchmark_runs {
        text.push_str(&answers::render_summary(benchmark_run));
    }
    text
}
