//! Benchmarks, the tasks they hold and the benchmark runs on them, kept in the
//! database (`docs/specs/benchmarks.md`, "Benchmarks" and "Benchmark runs"):
//! every write to them, each in one transaction.

mod answers;
pub mod results;
mod runs;

pub use answers::{
    BenchmarkRunAnswer, BenchmarkRunSummary, Shown, WaitingFor, dollars, duration, mean, percent,
    render_benchmark_run, without,
};
pub use runs::{
    BENCHMARK_RUNS, BenchmarkRunId, BenchmarkRunRecord, BenchmarkRunState, BenchmarkRuns,
    CancelRefusal, ConfigurationName, LaunchRefusal, NoScore, Repetitions, ScoreFigures,
    TrialChange, TrialEnd, TrialFigures, TrialKey, TrialRef, TrialState,
};

use std::fmt;
use std::fmt::Write as _;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Serialize;

use crate::database::Database;
use crate::runs::RunId;
use crate::task::{self, CaptureError, Criteria, TaskName, TaskRefusal, TaskStage};

/// A benchmark's name: one or more lowercase letters, digits and hyphens,
/// starting with a letter or a digit.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BenchmarkName(String);

impl BenchmarkName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for BenchmarkName {
    type Err = String;

    fn from_str(name: &str) -> Result<BenchmarkName, String> {
        task::check_name(name, "a suite's").map(|()| BenchmarkName(name.to_string()))
    }
}

impl fmt::Display for BenchmarkName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for BenchmarkName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// A named set of tasks of one stage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Benchmark {
    pub name: BenchmarkName,
    pub stage: TaskStage,
}

/// The benchmark a capture puts its task into, resolved: one there is, or one
/// to make with the task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChosenBenchmark {
    Existing(Benchmark),
    New(Benchmark),
}

impl ChosenBenchmark {
    pub fn benchmark(&self) -> &Benchmark {
        match self {
            ChosenBenchmark::Existing(benchmark) | ChosenBenchmark::New(benchmark) => benchmark,
        }
    }
}

/// A benchmark, with how many tasks it holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BenchmarkListed {
    #[serde(flatten)]
    pub benchmark: Benchmark,
    #[serde(rename = "cases")]
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
    pub name: TaskName,
    /// The run it was captured from.
    pub run: RunId,
    /// Its criteria, or why they do not read, such as once a person
    /// emptied them.
    pub criteria: Result<Criteria, TaskRefusal>,
}

/// Why a benchmark was not made.
#[derive(Debug, thiserror::Error)]
pub enum MakeError {
    #[error("a suite is already named {0}")]
    Taken(BenchmarkName),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// Why a task was not removed.
#[derive(Debug, thiserror::Error)]
pub enum RemoveError {
    #[error("there is no suite {0}")]
    NoSuchBenchmark(BenchmarkName),
    #[error("the suite {benchmark} has no case {task}")]
    NoSuchTask {
        benchmark: BenchmarkName,
        task: TaskName,
    },
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

    /// Makes the benchmark `name` of `stage`, refused when a benchmark has the
    /// name.
    pub async fn make(
        &self,
        name: &BenchmarkName,
        stage: TaskStage,
    ) -> Result<Benchmark, MakeError> {
        let benchmark = Benchmark {
            name: name.clone(),
            stage,
        };
        insert_benchmark(self.db.pool(), &benchmark).await?;
        Ok(benchmark)
    }

    /// Whether a task has the name `name`: a task of a benchmark, or a
    /// directory in the tasks, such as of a task captured into no benchmark.
    pub async fn has_task(&self, name: &TaskName) -> Result<bool> {
        let row: Option<String> = sqlx::query_scalar("SELECT name FROM tasks WHERE name = ?")
            .bind(name.as_str())
            .fetch_optional(self.db.pool())
            .await
            .context(READING)?;
        Ok(row.is_some()
            || self
                .tasks_dir()
                .join(name.as_str())
                .symlink_metadata()
                .is_ok())
    }

    /// Every benchmark, by name, with how many tasks it holds.
    pub async fn list(&self) -> Result<Vec<BenchmarkListed>> {
        let rows: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT benchmarks.name, benchmarks.stage, COUNT(tasks.name) FROM benchmarks
             LEFT JOIN tasks ON tasks.benchmark = benchmarks.name
             GROUP BY benchmarks.name ORDER BY benchmarks.name",
        )
        .fetch_all(self.db.pool())
        .await
        .context(READING)?;
        rows.into_iter()
            .map(|(name, stage, tasks)| {
                Ok(BenchmarkListed {
                    benchmark: benchmark_of(&name, &stage)?,
                    tasks: usize::try_from(tasks).context(READING)?,
                })
            })
            .collect()
    }

    /// The benchmark `name`, if there is one.
    pub async fn find(&self, name: &BenchmarkName) -> Result<Option<Benchmark>> {
        let stage: Option<String> =
            sqlx::query_scalar("SELECT stage FROM benchmarks WHERE name = ?")
                .bind(name.as_str())
                .fetch_optional(self.db.pool())
                .await
                .context(READING)?;
        stage
            .map(|stage| benchmark_of(name.as_str(), &stage))
            .transpose()
    }

    /// The benchmark `name` with its tasks, in its order, each with its
    /// criteria as its directory holds them now, and its benchmark runs, latest
    /// first; `None` when there is no such benchmark.
    pub async fn show(&self, name: &BenchmarkName) -> Result<Option<BenchmarkShown>> {
        let Some(benchmark) = self.find(name).await? else {
            return Ok(None);
        };
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT name, run FROM tasks WHERE benchmark = ? ORDER BY seq")
                .bind(name.as_str())
                .fetch_all(self.db.pool())
                .await
                .context(READING)?;
        let tasks_dir = self.tasks_dir();
        let tasks = rows
            .into_iter()
            .map(|(task, run)| {
                let task: TaskName = task.parse().map_err(anyhow::Error::msg)?;
                let criteria = task::read_criteria(&tasks_dir.join(task.as_str()));
                Ok(TaskEntry {
                    name: task,
                    run: run.parse().map_err(anyhow::Error::msg)?,
                    criteria,
                })
            })
            .collect::<Result<_>>()
            .context(READING)?;
        let benchmark_runs = BenchmarkRuns::new(self.db.clone())
            .of_benchmark(name)
            .await?;
        Ok(Some(BenchmarkShown {
            benchmark,
            tasks,
            benchmark_runs,
        }))
    }

    /// Removes the task `task` from the benchmark `benchmark`, and with it the
    /// task: its row and its directory go together, or neither does.
    pub async fn remove_task(
        &self,
        benchmark: &BenchmarkName,
        task: &TaskName,
    ) -> Result<(), RemoveError> {
        let removing = format!("removing the case {task} from the suite {benchmark}");
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .context(removing.clone())?;
        let found: Option<String> =
            sqlx::query_scalar("SELECT name FROM benchmarks WHERE name = ?")
                .bind(benchmark.as_str())
                .fetch_optional(&mut *tx)
                .await
                .context(removing.clone())?;
        if found.is_none() {
            return Err(RemoveError::NoSuchBenchmark(benchmark.clone()));
        }
        let deleted = sqlx::query("DELETE FROM tasks WHERE name = ? AND benchmark = ?")
            .bind(task.as_str())
            .bind(benchmark.as_str())
            .execute(&mut *tx)
            .await
            .context(removing.clone())?;
        if deleted.rows_affected() == 0 {
            return Err(RemoveError::NoSuchTask {
                benchmark: benchmark.clone(),
                task: task.clone(),
            });
        }
        let tasks_dir = self.tasks_dir();
        let place = tasks_dir.join(task.as_str());
        std::fs::create_dir_all(&tasks_dir)
            .context("creating the cases' directory")
            .context(removing.clone())?;
        let bin = tempfile::Builder::new()
            .prefix(".removing-")
            .tempdir_in(&tasks_dir)
            .context(removing.clone())?;
        let binned = bin.path().join(task.as_str());
        match std::fs::rename(&place, &binned) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                return Err(anyhow::Error::from(e)
                    .context(format!("moving {} out of the cases", place.display()))
                    .context(removing)
                    .into());
            }
        }
        if let Err(e) = tx.commit().await {
            if binned.exists()
                && let Err(back) = std::fs::rename(&binned, &place)
            {
                tracing::warn!(case = %task, error = %back, "putting back a case whose removal failed failed");
                let _kept = bin.keep();
            }
            return Err(anyhow::Error::from(e).context(removing).into());
        }
        Ok(())
    }

    /// Keeps the task `name`, captured from `run` and built in the
    /// directory `built`, in `chosen`, made first when it is new: in one
    /// transaction, the rows are inserted and `built` is renamed into its
    /// place in the tasks, so that all are written or none is. Refused as
    /// [`CaptureError::Taken`] when a task has the name, in a row or in a
    /// directory.
    pub(crate) async fn keep_task(
        &self,
        chosen: &ChosenBenchmark,
        name: &TaskName,
        run: &RunId,
        built: &Path,
    ) -> Result<PathBuf, CaptureError> {
        let benchmark = chosen.benchmark();
        let keeping = format!("keeping the case {name} in the suite {}", benchmark.name);
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .context(keeping.clone())?;
        if let ChosenBenchmark::New(new) = chosen {
            match insert_benchmark(&mut *tx, new).await {
                Ok(()) => {}
                Err(MakeError::Taken(taken)) => return Err(CaptureError::BenchmarkTaken(taken)),
                Err(MakeError::Failed(e)) => return Err(e.context(keeping).into()),
            }
        }
        let inserted = sqlx::query(
            "INSERT INTO tasks (name, benchmark, run, seq) VALUES (?1, ?2, ?3,
                 (SELECT COALESCE(MAX(seq), 0) + 1 FROM tasks WHERE benchmark = ?2))",
        )
        .bind(name.as_str())
        .bind(benchmark.name.as_str())
        .bind(run.to_string())
        .execute(&mut *tx)
        .await;
        match inserted {
            Ok(_) => {}
            Err(e) if unique_violation(&e) => return Err(CaptureError::Taken(name.clone())),
            Err(e) if foreign_key_violation(&e) => {
                return Err(CaptureError::NoSuchBenchmark(benchmark.name.clone()));
            }
            Err(e) => return Err(anyhow::Error::from(e).context(keeping).into()),
        }
        let place = self.tasks_dir().join(name.as_str());
        match rustix::fs::renameat_with(
            rustix::fs::CWD,
            built,
            rustix::fs::CWD,
            &place,
            rustix::fs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => {}
            Err(e) if std::io::Error::from(e).kind() == ErrorKind::AlreadyExists => {
                return Err(CaptureError::Taken(name.clone()));
            }
            Err(e) => {
                return Err(anyhow::Error::from(std::io::Error::from(e))
                    .context(format!("moving the case into {}", place.display()))
                    .into());
            }
        }
        if let Err(e) = tx.commit().await {
            if let Err(back) = std::fs::rename(&place, built) {
                tracing::warn!(case = %name, error = %back, "taking back a case whose row was not kept failed");
            }
            return Err(anyhow::Error::from(e).context(keeping).into());
        }
        Ok(place)
    }
}

/// Inserts `benchmark`'s row with `executor`, refused when a benchmark has its
/// name.
async fn insert_benchmark<'c>(
    executor: impl sqlx::SqliteExecutor<'c>,
    benchmark: &Benchmark,
) -> Result<(), MakeError> {
    let inserted = sqlx::query("INSERT INTO benchmarks (name, stage) VALUES (?, ?)")
        .bind(benchmark.name.as_str())
        .bind(benchmark.stage.name())
        .execute(executor)
        .await;
    match inserted {
        Ok(_) => Ok(()),
        Err(e) if unique_violation(&e) => Err(MakeError::Taken(benchmark.name.clone())),
        Err(e) => Err(anyhow::Error::from(e)
            .context(format!("making the suite {}", benchmark.name))
            .into()),
    }
}

const READING: &str = "reading the suites";

/// The benchmark a row holds.
fn benchmark_of(name: &str, stage: &str) -> Result<Benchmark> {
    Ok(Benchmark {
        name: name.parse().map_err(anyhow::Error::msg).context(READING)?,
        stage: stage.parse().map_err(anyhow::Error::msg).context(READING)?,
    })
}

fn unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(e) if e.is_unique_violation())
}

fn foreign_key_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(e) if e.is_foreign_key_violation())
}

/// What `suites` answers.
#[derive(Serialize)]
pub struct BenchmarksAnswer {
    #[serde(rename = "suites")]
    pub benchmarks: Vec<BenchmarkListed>,
}

/// The benchmarks as `suites` prints them: a line for each.
pub fn render_benchmarks(answer: &BenchmarksAnswer) -> String {
    if answer.benchmarks.is_empty() {
        return "suites: none\n".into();
    }
    let mut text = String::new();
    for listed in &answer.benchmarks {
        let tasks = match listed.tasks {
            0 => "no cases".to_string(),
            1 => "1 case".to_string(),
            n => format!("{n} cases"),
        };
        let _ = writeln!(
            text,
            "{}: {}, {tasks}",
            listed.benchmark.name, listed.benchmark.stage
        );
    }
    text
}

/// What `suite show NAME` answers.
#[derive(Serialize)]
pub struct BenchmarkAnswer {
    #[serde(flatten)]
    pub benchmark: Benchmark,
    #[serde(rename = "cases")]
    pub tasks: Vec<TaskAnswer>,
    #[serde(rename = "experiments")]
    pub benchmark_runs: Vec<BenchmarkRunSummary>,
}

/// A task as `suite show NAME` answers it: its criteria, or why they do not
/// read.
#[derive(Serialize)]
pub struct TaskAnswer {
    pub name: TaskName,
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

/// A benchmark as `suite show NAME` prints it: its facts, then each task with
/// its run, and its criteria indented under it, then each benchmark run, latest
/// first.
pub fn render_benchmark(answer: &BenchmarkAnswer) -> String {
    let mut text = format!(
        "suite: {}\nstage: {}\n",
        answer.benchmark.name, answer.benchmark.stage
    );
    if answer.tasks.is_empty() {
        text.push_str("cases: none\n");
    }
    for task in &answer.tasks {
        let _ = writeln!(text, "case {}: captured from {}", task.name, task.run);
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
        text.push_str("experiments: none\n");
    }
    for benchmark_run in &answer.benchmark_runs {
        text.push_str(&answers::render_summary(benchmark_run));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_log::test]
    fn a_benchmarks_name_is_lowercase_letters_digits_and_hyphens_starting_with_a_letter_or_a_digit()
    {
        for name in ["board", "design-2", "7"] {
            assert_eq!(
                name.parse::<BenchmarkName>().map(|n| n.to_string()),
                Ok(name.into())
            );
        }
        for name in [
            "", "-", "-board", "Board", "board_1", "board 1", "../board", "bōard",
        ] {
            assert!(name.parse::<BenchmarkName>().is_err(), "{name:?}");
        }
    }
}
