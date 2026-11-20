//! Benchmark runs, kept in the database and in `benchmark-runs/` under the work
//! directory (`docs/specs/benchmarks.md`, Benchmark runs).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroU32;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result};
use serde::Serialize;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row as _, SqliteConnection};

use crate::benchmarks::{Benchmark, BenchmarkName};
use crate::cost::Usd;
use crate::database::{self, Database};
use crate::domain::Timestamp;
use crate::results::Verdict;
use crate::runs::{Ending, OutcomeName};
use crate::task::{self, TaskName};
use crate::tree;
use crate::trial::Configuration;

/// A benchmark run's ID: a positive number, given at its launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct BenchmarkRunId(i64);

impl std::str::FromStr for BenchmarkRunId {
    type Err = String;

    fn from_str(id: &str) -> Result<BenchmarkRunId, String> {
        id.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| id.parse::<i64>().ok())
            .flatten()
            .and_then(BenchmarkRunId::positive)
            .ok_or_else(|| format!("{id:?} is not a benchmark run's ID: it is a positive number"))
    }
}

impl fmt::Display for BenchmarkRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl BenchmarkRunId {
    fn positive(id: i64) -> Option<BenchmarkRunId> {
        (id > 0).then_some(BenchmarkRunId(id))
    }

    fn from_row(id: i64) -> Result<BenchmarkRunId> {
        BenchmarkRunId::positive(id).with_context(|| format!("{id} is not a benchmark run's ID"))
    }
}

/// A configuration's name in a benchmark run: one or more lowercase letters,
/// digits and hyphens, starting with a letter or a digit.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigurationName(String);

impl ConfigurationName {
    /// The name of the configuration that holds nothing, first in every
    /// benchmark run.
    pub const AS_IS: &str = "as-is";

    pub fn as_is() -> ConfigurationName {
        ConfigurationName(Self::AS_IS.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for ConfigurationName {
    type Err = String;

    fn from_str(name: &str) -> Result<ConfigurationName, String> {
        task::check_name(name, "a configuration's").map(|()| ConfigurationName(name.to_string()))
    }
}

impl fmt::Display for ConfigurationName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ConfigurationName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// How many times a benchmark run runs each task under each configuration.
pub type Repetitions = NonZeroU32;

/// A benchmark run's state as it is kept: whether `serve` runs it or waits to
/// is not kept. A benchmark run that ended or was cancelled before ends were
/// kept has no end, `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BenchmarkRunState {
    Open,
    Ended { at: Option<Timestamp> },
    Cancelled { at: Option<Timestamp> },
}

impl BenchmarkRunState {
    pub fn name(self) -> &'static str {
        match self {
            BenchmarkRunState::Open => "open",
            BenchmarkRunState::Ended { .. } => "ended",
            BenchmarkRunState::Cancelled { .. } => "cancelled",
        }
    }

    /// When it ended or was cancelled, if it has and that was kept.
    pub fn ended_at(self) -> Option<Timestamp> {
        match self {
            BenchmarkRunState::Open => None,
            BenchmarkRunState::Ended { at } | BenchmarkRunState::Cancelled { at } => at,
        }
    }

    /// The state a row keeps as `name` and `ended_at`.
    fn parse(name: &str, ended_at: Option<&str>) -> Result<BenchmarkRunState> {
        let at = ended_at.map(database::parse_time).transpose()?;
        match (name, at) {
            ("open", None) => Ok(BenchmarkRunState::Open),
            ("open", Some(_)) => anyhow::bail!("an open benchmark run has an end"),
            ("ended", at) => Ok(BenchmarkRunState::Ended { at }),
            ("cancelled", at) => Ok(BenchmarkRunState::Cancelled { at }),
            (name, _) => anyhow::bail!("{name:?} is not a benchmark run's state"),
        }
    }
}

/// A trial of a benchmark run: of one task under one configuration in one
/// repetition, then its scorer. Its order is the order the trials
/// start in: by repetition, then task, then configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrialKey {
    pub repetition: NonZeroU32,
    /// The task's place in the benchmark run's tasks.
    pub task: usize,
    /// The configuration's place in the benchmark run's configurations.
    pub configuration: usize,
}

/// A trial of a given benchmark run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrialRef {
    pub benchmark_run: BenchmarkRunId,
    pub key: TrialKey,
}

/// Where a trial stands.
#[derive(Clone, Debug, PartialEq)]
pub enum TrialState {
    Pending,
    Running,
    /// Its run ended, to be scored, and awaits its scorer.
    Ran {
        run: TrialFigures,
    },
    Scoring {
        run: TrialFigures,
    },
    Ended(TrialEnd),
}

impl TrialState {
    /// The figures of its run, once its run has ended.
    pub fn run(&self) -> Option<&TrialFigures> {
        match self {
            TrialState::Pending | TrialState::Running | TrialState::Ended(TrialEnd::Cancelled) => {
                None
            }
            TrialState::Ran { run }
            | TrialState::Scoring { run }
            | TrialState::Ended(TrialEnd::Scored { run, .. }) => Some(run),
            TrialState::Ended(TrialEnd::Unscored(no_score)) => Some(no_score.run()),
        }
    }

    /// Its score, if its scorer made one.
    pub fn score(&self) -> Option<&ScoreFigures> {
        match self {
            TrialState::Ended(TrialEnd::Scored { score, .. }) => Some(score),
            TrialState::Pending
            | TrialState::Running
            | TrialState::Ran { .. }
            | TrialState::Scoring { .. }
            | TrialState::Ended(TrialEnd::Unscored(_) | TrialEnd::Cancelled) => None,
        }
    }

    /// Whether its scorer has started, and was not stopped by a cancel.
    pub fn scorer_began(&self) -> bool {
        matches!(
            self,
            TrialState::Scoring { .. }
                | TrialState::Ended(
                    TrialEnd::Scored { .. } | TrialEnd::Unscored(NoScore::Scorer { .. })
                )
        )
    }
}

/// How a trial ended.
#[derive(Clone, Debug, PartialEq)]
pub enum TrialEnd {
    Scored {
        run: TrialFigures,
        score: ScoreFigures,
    },
    /// Its run ended, and has no score.
    Unscored(NoScore),
    /// Its benchmark run was cancelled before its run ended.
    Cancelled,
}

/// What the results need of a trial.
#[derive(Clone, Debug, PartialEq)]
pub struct TrialFigures {
    pub outcome: OutcomeName,
    /// `None` when its run has no cost (`docs/specs/runs.md`, Cost).
    pub money: Option<Usd>,
    pub time: Duration,
}

impl TrialFigures {
    /// Whether the scorer scores its run: done or handed back
    /// (`docs/specs/benchmarks.md`, Benchmark runs).
    fn scoreable(&self) -> bool {
        matches!(
            self.outcome,
            OutcomeName::Ended(Ending::Done | Ending::HandedBack)
        )
    }
}

/// What the results need of a score.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreFigures {
    pub correctness: Verdict,
    pub leak: bool,
    /// `None` when the scorer's session has no cost.
    pub scorer_money: Option<Usd>,
}

/// Why a trial that ended has no score, with the figures of its run.
#[derive(Clone, Debug, PartialEq)]
pub enum NoScore {
    /// Its run is not done or handed back.
    Outcome(TrialFigures),
    /// Its scorer made no score.
    Scorer {
        run: TrialFigures,
        why: String,
        /// `None` when the scorer's session has no cost.
        scorer_money: Option<Usd>,
    },
    /// Its benchmark run was cancelled before its scorer made a score.
    Cancelled(TrialFigures),
}

impl NoScore {
    /// The figures of its run.
    pub fn run(&self) -> &TrialFigures {
        match self {
            NoScore::Outcome(run) | NoScore::Scorer { run, .. } | NoScore::Cancelled(run) => run,
        }
    }
}

/// A benchmark run, as it was launched and as far as it has gone.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchmarkRunRecord {
    pub id: BenchmarkRunId,
    pub benchmark: Benchmark,
    pub launched_at: Timestamp,
    pub repetitions: Repetitions,
    pub state: BenchmarkRunState,
    /// The benchmark's tasks at the launch, in its order.
    pub tasks: Vec<TaskName>,
    /// `as-is` first.
    pub configurations: Vec<ConfigurationName>,
    /// Every trial, in start order, each of a task and a configuration of
    /// the benchmark run's.
    pub trials: BTreeMap<TrialKey, TrialState>,
}

impl BenchmarkRunRecord {
    /// The time from its launch to its end, or to `now` while it is open;
    /// `None` when its end was not kept.
    pub fn duration(&self, now: Timestamp) -> Option<Duration> {
        let end = if self.state == BenchmarkRunState::Open {
            now
        } else {
            self.state.ended_at()?
        };
        Some((end - self.launched_at).to_std().unwrap_or_default())
    }
}

/// The directory, under the work directory, that holds the benchmark runs.
pub const BENCHMARK_RUNS: &str = "benchmark-runs";

/// The directory, in a benchmark run's, of what it kept at its launch: hidden,
/// as no configuration's name is.
const KEPT: &str = ".kept";

/// The directory, in [`KEPT`], of the configurations.
const CONFIGURATIONS: &str = "configurations";

/// Why a benchmark run was not launched.
#[derive(Debug, thiserror::Error)]
pub enum LaunchRefusal {
    #[error("there is no benchmark {0}")]
    NoSuchBenchmark(BenchmarkName),
    #[error("the benchmark {0} has no tasks")]
    NoTasks(BenchmarkName),
    #[error("two configurations are named {0}")]
    DuplicateConfiguration(ConfigurationName),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// A benchmark run just launched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Launched {
    pub id: BenchmarkRunId,
    /// How many trials it makes.
    pub trials: usize,
}

/// Why a benchmark run was not cancelled.
#[derive(Debug, thiserror::Error)]
pub enum CancelRefusal {
    #[error("there is no benchmark run {0}")]
    NoSuchBenchmarkRun(BenchmarkRunId),
    #[error("benchmark run {id} is {}", state.name())]
    NotOpen {
        id: BenchmarkRunId,
        state: BenchmarkRunState,
    },
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// A change of a trial's state.
#[derive(Clone, Debug, PartialEq)]
pub enum TrialChange {
    /// Pending, its run starts.
    RunStarts,
    /// Running, its run ends: ran when its run can be scored, and else
    /// ended without a score.
    RunEnds(TrialFigures),
    /// Ran, its scorer starts.
    ScorerStarts,
    /// Scoring, its scorer makes a score.
    Scored(ScoreFigures),
    /// Scoring, its scorer makes none.
    ScorerMakesNone {
        why: String,
        scorer_money: Option<Usd>,
    },
    /// Not ended, its benchmark run is cancelled.
    Cancelled,
}

/// The benchmark runs of a work directory, kept in its database and under
/// [`BENCHMARK_RUNS`]: every handle on one database sees the same benchmark
/// runs.
#[derive(Clone, Debug)]
pub struct BenchmarkRuns {
    db: Database,
}

const READING: &str = "reading the benchmark runs";

impl BenchmarkRuns {
    pub fn new(db: Database) -> BenchmarkRuns {
        BenchmarkRuns { db }
    }

    fn benchmark_runs_dir(&self) -> PathBuf {
        self.db.work_dir().join(BENCHMARK_RUNS)
    }

    /// The directory of the benchmark run `id`.
    pub fn dir(&self, id: BenchmarkRunId) -> PathBuf {
        self.benchmark_runs_dir().join(id.to_string())
    }

    /// The benchmark run `id`'s copy of the task `task`, as it was at the
    /// launch.
    pub fn task_dir(&self, id: BenchmarkRunId, task: &TaskName) -> PathBuf {
        kept_task(&self.dir(id), task)
    }

    /// The benchmark run `id`'s copy of the configuration `name`, laid out as a
    /// repository's `.linewright/`.
    pub fn configuration_dir(&self, id: BenchmarkRunId, name: &ConfigurationName) -> PathBuf {
        kept_configuration(&self.dir(id), name)
    }

    /// The output of the trial of `task` under `configuration` in the
    /// repetition `repetition` of the benchmark run `id`
    /// (`docs/specs/benchmarks.md`, Benchmark runs, The output).
    pub fn output_dir(
        &self,
        id: BenchmarkRunId,
        configuration: &ConfigurationName,
        task: &TaskName,
        repetition: NonZeroU32,
    ) -> PathBuf {
        self.dir(id)
            .join(configuration.as_str())
            .join(task.as_str())
            .join(repetition.to_string())
    }

    /// Launches a benchmark run at `at` on the tasks the benchmark `benchmark`
    /// holds, under `as-is` and then `configurations`, each run `repetitions`
    /// times, every trial pending, with a copy of each task and configuration
    /// in its directory. It is all or nothing.
    ///
    /// # Errors
    ///
    /// When there is no such benchmark, it holds no tasks, two configurations
    /// have one name, `as-is` among them, or a copy or a row cannot be
    /// written, such as when the benchmark's tasks change as they are copied.
    pub async fn launch(
        &self,
        benchmark: &BenchmarkName,
        configurations: Vec<(ConfigurationName, Configuration)>,
        repetitions: Repetitions,
        at: Timestamp,
    ) -> Result<Launched, LaunchRefusal> {
        let mut names = BTreeSet::from([ConfigurationName::as_is()]);
        for (name, _) in &configurations {
            if !names.insert(name.clone()) {
                return Err(LaunchRefusal::DuplicateConfiguration(name.clone()));
            }
        }
        let launching = format!("launching a benchmark run on the benchmark {benchmark}");
        let mut conn = self.db.pool().acquire().await.context(launching.clone())?;
        let tasks = benchmark_tasks(&mut conn, benchmark).await;
        drop(conn);
        let Some(tasks) = tasks.context(launching.clone())? else {
            return Err(LaunchRefusal::NoSuchBenchmark(benchmark.clone()));
        };
        if tasks.is_empty() {
            return Err(LaunchRefusal::NoTasks(benchmark.clone()));
        }
        let built = self
            .build(&tasks, &configurations)
            .await
            .context(launching.clone())?;
        let configuration_names: Vec<ConfigurationName> =
            std::iter::once(ConfigurationName::as_is())
                .chain(configurations.into_iter().map(|(name, _)| name))
                .collect();
        self.keep(
            benchmark,
            &tasks,
            &configuration_names,
            repetitions,
            at,
            built,
        )
        .await
        .map_err(|e| e.context(launching).into())
    }

    /// Builds in a new directory under [`BENCHMARK_RUNS`] the copies of `tasks`
    /// and of `as-is` and `configurations` that a benchmark run keeps, and
    /// gives the directory with the task directories it copied.
    async fn build(
        &self,
        tasks: &[TaskName],
        configurations: &[(ConfigurationName, Configuration)],
    ) -> Result<Built> {
        let benchmark_runs_dir = self.benchmark_runs_dir();
        std::fs::create_dir_all(&benchmark_runs_dir)
            .with_context(|| format!("making {}", benchmark_runs_dir.display()))?;
        let dir = tempfile::Builder::new()
            .prefix(".launching-")
            .tempdir_in(&benchmark_runs_dir)?;
        let tasks_dir = self.db.work_dir().join(task::TASKS);
        let mut copied = Vec::with_capacity(tasks.len());
        for task in tasks {
            let from = tasks_dir.join(task.as_str());
            let before = TaskDir::of(&from)?;
            tree::copy(&from, &kept_task(dir.path(), task))
                .await
                .with_context(|| format!("copying the task {task} from {}", from.display()))?;
            copied.push(before);
        }
        let as_is = kept_configuration(dir.path(), &ConfigurationName::as_is());
        std::fs::create_dir_all(&as_is).with_context(|| format!("making {}", as_is.display()))?;
        for (name, configuration) in configurations {
            configuration
                .keep(&kept_configuration(dir.path(), name))
                .with_context(|| format!("copying the configuration {name}"))?;
        }
        Ok(Built { dir, copied })
    }

    /// Keeps the benchmark run whose copies are `built`, in one transaction:
    /// its rows are inserted and `built` is renamed into its directory, as
    /// long as the benchmark's tasks are still those it copied.
    async fn keep(
        &self,
        benchmark: &BenchmarkName,
        tasks: &[TaskName],
        configurations: &[ConfigurationName],
        repetitions: Repetitions,
        at: Timestamp,
        built: Built,
    ) -> Result<Launched> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let now = benchmark_tasks(&mut tx, benchmark).await?;
        let tasks_dir = self.db.work_dir().join(task::TASKS);
        let unchanged = now.as_deref() == Some(tasks)
            && tasks.iter().zip(&built.copied).all(|(task, copied)| {
                TaskDir::of(&tasks_dir.join(task.as_str())).ok().as_ref() == Some(copied)
            });
        if !unchanged {
            anyhow::bail!("the benchmark's tasks changed as they were copied: launch again");
        }
        let id = BenchmarkRunId::from_row(next_id(&mut tx, &self.benchmark_runs_dir()).await?)?;
        let trials = insert_benchmark_run(
            &mut tx,
            id,
            benchmark,
            tasks,
            configurations,
            repetitions,
            at,
        )
        .await?;
        let place = self.dir(id);
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            built.dir.path(),
            rustix::fs::CWD,
            &place,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)
        .with_context(|| format!("moving the benchmark run into {}", place.display()))?;
        if let Err(e) = tx.commit().await {
            if let Err(back) = std::fs::rename(&place, built.dir.path()) {
                tracing::warn!(benchmark_run = %id, error = %back, "taking back a benchmark run whose rows were not kept failed");
            }
            return Err(e.into());
        }
        Ok(Launched { id, trials })
    }

    /// The benchmark run `id`, if there is one.
    pub async fn show(&self, id: BenchmarkRunId) -> Result<Option<BenchmarkRunRecord>> {
        let mut conn = self.db.pool().acquire().await.context(READING)?;
        read_record(&mut conn, id).await
    }

    /// The benchmark `benchmark`'s benchmark runs, latest first.
    pub async fn of_benchmark(&self, benchmark: &BenchmarkName) -> Result<Vec<BenchmarkRunRecord>> {
        self.records_of(
            "SELECT id FROM benchmark_runs WHERE benchmark = ? ORDER BY id DESC",
            Some(benchmark.as_str()),
        )
        .await
    }

    /// Each benchmark's latest benchmark run, with when it was launched, by the
    /// benchmark's name: none for a benchmark with no benchmark run.
    pub async fn latest(&self) -> Result<BTreeMap<BenchmarkName, (BenchmarkRunId, Timestamp)>> {
        let rows: Vec<(String, i64, String)> = sqlx::query_as(
            "SELECT benchmark, id, launched_at FROM benchmark_runs
             WHERE id IN (SELECT MAX(id) FROM benchmark_runs GROUP BY benchmark)",
        )
        .fetch_all(self.db.pool())
        .await
        .context(READING)?;
        rows.into_iter()
            .map(|(benchmark, id, launched_at)| {
                Ok((
                    parsed(&benchmark)?,
                    (
                        BenchmarkRunId::from_row(id)?,
                        database::parse_time(&launched_at)?,
                    ),
                ))
            })
            .collect::<Result<_>>()
            .context(READING)
    }

    /// The benchmark runs that are open, oldest first, each with its trials in
    /// start order.
    pub async fn open(&self) -> Result<Vec<BenchmarkRunRecord>> {
        self.records_of(
            "SELECT id FROM benchmark_runs WHERE state = 'open' ORDER BY id",
            None,
        )
        .await
    }

    /// The records of the benchmark runs whose IDs `selecting` selects, with
    /// `bound` for its one parameter if it has one, read together.
    async fn records_of(
        &self,
        selecting: &'static str,
        bound: Option<&str>,
    ) -> Result<Vec<BenchmarkRunRecord>> {
        let mut tx = self.db.pool().begin().await.context(READING)?;
        let mut query = sqlx::query_scalar(selecting);
        if let Some(bound) = bound {
            query = query.bind(bound);
        }
        let ids: Vec<i64> = query.fetch_all(&mut *tx).await.context(READING)?;
        let mut records = Vec::with_capacity(ids.len());
        for id in ids {
            let id = BenchmarkRunId::from_row(id).context(READING)?;
            if let Some(record) = read_record(&mut tx, id).await? {
                records.push(record);
            }
        }
        Ok(records)
    }

    /// Cancels the open benchmark run `id` at `at`. Its trials stand as they
    /// are.
    pub async fn cancel(&self, id: BenchmarkRunId, at: Timestamp) -> Result<(), CancelRefusal> {
        let cancelling = format!("cancelling benchmark run {id}");
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .context(cancelling.clone())?;
        match read_state(&mut tx, id).await.context(cancelling.clone())? {
            None => return Err(CancelRefusal::NoSuchBenchmarkRun(id)),
            Some(BenchmarkRunState::Open) => {}
            Some(state) => return Err(CancelRefusal::NotOpen { id, state }),
        }
        sqlx::query("UPDATE benchmark_runs SET state = 'cancelled', ended_at = ? WHERE id = ?")
            .bind(database::time_text(at))
            .bind(id.0)
            .execute(&mut *tx)
            .await
            .context(cancelling.clone())?;
        tx.commit().await.context(cancelling)?;
        Ok(())
    }

    /// The state the benchmark run `id` is kept in, if there is one.
    pub async fn state(&self, id: BenchmarkRunId) -> Result<Option<BenchmarkRunState>> {
        let mut conn = self.db.pool().acquire().await.context(READING)?;
        read_state(&mut conn, id).await.context(READING)
    }

    /// Turns each trial a stop left running back into pending, and each
    /// left scoring back into ran, so that they start again. Of a
    /// benchmark run no longer open, such as one cancelled as it stopped, they
    /// end as cancelled instead.
    pub async fn interrupted(&self) -> Result<()> {
        let interrupting = async {
            let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
            sqlx::query(
                "UPDATE trials SET state = CASE state
                     WHEN 'running' THEN 'pending'
                     WHEN 'scoring' THEN 'ran'
                 END
                 WHERE state IN ('running', 'scoring')
                     AND benchmark_run IN (SELECT id FROM benchmark_runs WHERE state = 'open')",
            )
            .execute(&mut *tx)
            .await?;
            let left = sqlx::query(
                "SELECT trials.* FROM trials
                 JOIN benchmark_runs ON benchmark_runs.id = trials.benchmark_run
                 WHERE trials.state IN ('running', 'scoring') AND benchmark_runs.state != 'open'",
            )
            .fetch_all(&mut *tx)
            .await?;
            for row in &left {
                let benchmark_run = BenchmarkRunId::from_row(row.try_get("benchmark_run")?)?;
                let (key, state) = decode(row)?;
                write_trial(
                    &mut tx,
                    benchmark_run,
                    &key,
                    &state.changed(TrialChange::Cancelled)?,
                )
                .await?;
            }
            tx.commit().await?;
            anyhow::Ok(())
        };
        interrupting
            .await
            .context("starting again the trials a stop interrupted")
    }

    /// Records that the benchmark run `id` ended at `at`, if it is open and
    /// none of its trials is left to end.
    pub async fn end(&self, id: BenchmarkRunId, at: Timestamp) -> Result<()> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        end_if_done(&mut tx, id, at).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Records `change` of the trial `trial` at `at`, and that its
    /// benchmark run ended then once it is open and none of its trials is left
    /// to end.
    ///
    /// # Errors
    ///
    /// When there is no such trial, `change` is not one of its state, or the
    /// rows cannot be written.
    pub async fn step(&self, trial: &TrialRef, change: TrialChange, at: Timestamp) -> Result<()> {
        let TrialRef { benchmark_run, key } = trial;
        let stepping = format!(
            "recording a change of the trial {}/{}/{} of benchmark run {benchmark_run}",
            key.repetition, key.task, key.configuration
        );
        let stepped = async {
            let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
            let row = sqlx::query(
                "SELECT * FROM trials
                 WHERE benchmark_run = ? AND repetition = ? AND task_seq = ? AND configuration_seq = ?",
            )
            .bind(benchmark_run.0)
            .bind(key.repetition.get())
            .bind(seq(key.task))
            .bind(seq(key.configuration))
            .fetch_optional(&mut *tx)
            .await?
            .context("there is no such trial")?;
            let (_, state) = decode(&row)?;
            write_trial(&mut tx, *benchmark_run, key, &state.changed(change)?).await?;
            end_if_done(&mut tx, *benchmark_run, at).await?;
            tx.commit().await?;
            anyhow::Ok(())
        };
        stepped.await.context(stepping)
    }
}

/// Writes `state` as the state of the trial `key` of `benchmark_run`.
async fn write_trial(
    conn: &mut SqliteConnection,
    benchmark_run: BenchmarkRunId,
    key: &TrialKey,
    state: &TrialState,
) -> Result<()> {
    let columns = Columns::of(state);
    sqlx::query(
        "UPDATE trials SET state = ?, outcome = ?, money = ?, seconds = ?,
             correctness = ?, leak = ?, scorer_money = ?, unscored = ?, reason = ?
         WHERE benchmark_run = ? AND repetition = ? AND task_seq = ? AND configuration_seq = ?",
    )
    .bind(columns.state)
    .bind(columns.outcome)
    .bind(columns.money)
    .bind(columns.seconds)
    .bind(columns.correctness)
    .bind(columns.leak)
    .bind(columns.scorer_money)
    .bind(columns.unscored)
    .bind(columns.reason)
    .bind(benchmark_run.0)
    .bind(key.repetition.get())
    .bind(seq(key.task))
    .bind(seq(key.configuration))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// The state of the benchmark run `id`, if there is one.
async fn read_state(
    conn: &mut SqliteConnection,
    id: BenchmarkRunId,
) -> Result<Option<BenchmarkRunState>> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT state, ended_at FROM benchmark_runs WHERE id = ?")
            .bind(id.0)
            .fetch_optional(&mut *conn)
            .await?;
    row.map(|(state, ended_at)| BenchmarkRunState::parse(&state, ended_at.as_deref()))
        .transpose()
}

/// Records that the benchmark run `id` ended at `at`, if it is open and none of
/// its trials is left to end.
async fn end_if_done(conn: &mut SqliteConnection, id: BenchmarkRunId, at: Timestamp) -> Result<()> {
    sqlx::query(
        "UPDATE benchmark_runs SET state = 'ended', ended_at = ?
         WHERE id = ? AND state = 'open' AND NOT EXISTS (
             SELECT 1 FROM trials
             WHERE benchmark_run = ? AND state NOT IN ('scored', 'unscored'))",
    )
    .bind(database::time_text(at))
    .bind(id.0)
    .bind(id.0)
    .execute(&mut *conn)
    .await
    .with_context(|| format!("recording that benchmark run {id} ended"))?;
    Ok(())
}

impl TrialState {
    /// The state `change` leads to from this one.
    fn changed(self, change: TrialChange) -> Result<TrialState> {
        Ok(match (self, change) {
            (TrialState::Pending, TrialChange::RunStarts) => TrialState::Running,
            (TrialState::Running, TrialChange::RunEnds(run)) if run.scoreable() => {
                TrialState::Ran { run }
            }
            (TrialState::Running, TrialChange::RunEnds(run)) => {
                TrialState::Ended(TrialEnd::Unscored(NoScore::Outcome(run)))
            }
            (TrialState::Ran { run }, TrialChange::ScorerStarts) => TrialState::Scoring { run },
            (TrialState::Scoring { run }, TrialChange::Scored(score)) => {
                TrialState::Ended(TrialEnd::Scored { run, score })
            }
            (TrialState::Scoring { run }, TrialChange::ScorerMakesNone { why, scorer_money }) => {
                TrialState::Ended(TrialEnd::Unscored(NoScore::Scorer {
                    run,
                    why,
                    scorer_money,
                }))
            }
            (TrialState::Pending | TrialState::Running, TrialChange::Cancelled) => {
                TrialState::Ended(TrialEnd::Cancelled)
            }
            (TrialState::Ran { run } | TrialState::Scoring { run }, TrialChange::Cancelled) => {
                TrialState::Ended(TrialEnd::Unscored(NoScore::Cancelled(run)))
            }
            (state, change) => anyhow::bail!(
                "a trial {} cannot change by {change:?}",
                Columns::of(&state).state
            ),
        })
    }
}

/// The copies a benchmark run keeps, built in a directory of their own.
struct Built {
    dir: tempfile::TempDir,
    /// The directory of each task copied, in the order of the tasks.
    copied: Vec<TaskDir>,
}

/// Which directory a task's is: another one in its place, such as of a
/// task removed and captured again, is not the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TaskDir {
    dev: u64,
    ino: u64,
}

impl TaskDir {
    fn of(dir: &Path) -> Result<TaskDir> {
        let metadata =
            std::fs::symlink_metadata(dir).with_context(|| format!("reading {}", dir.display()))?;
        Ok(TaskDir {
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }
}

/// The copy of the task `task` in the benchmark run's directory `dir`.
fn kept_task(dir: &Path, task: &TaskName) -> PathBuf {
    dir.join(KEPT).join(task::TASKS).join(task.as_str())
}

/// The copy of the configuration `name` in the benchmark run's directory `dir`.
fn kept_configuration(dir: &Path, name: &ConfigurationName) -> PathBuf {
    dir.join(KEPT).join(CONFIGURATIONS).join(name.as_str())
}

/// The tasks of the benchmark `benchmark`, in its order, or `None` when there
/// is no such benchmark.
async fn benchmark_tasks(
    conn: &mut SqliteConnection,
    benchmark: &BenchmarkName,
) -> Result<Option<Vec<TaskName>>> {
    let found: Option<String> = sqlx::query_scalar("SELECT name FROM benchmarks WHERE name = ?")
        .bind(benchmark.as_str())
        .fetch_optional(&mut *conn)
        .await
        .context(READING)?;
    if found.is_none() {
        return Ok(None);
    }
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM tasks WHERE benchmark = ? ORDER BY seq")
            .bind(benchmark.as_str())
            .fetch_all(&mut *conn)
            .await
            .context(READING)?;
    names
        .iter()
        .map(|name| parsed(name))
        .collect::<Result<_>>()
        .map(Some)
}

/// The ID of the next benchmark run: past every benchmark run's, and past every
/// directory in `benchmark_runs_dir` named as one, such as one a launch left
/// there as it stopped.
async fn next_id(conn: &mut SqliteConnection, benchmark_runs_dir: &Path) -> Result<i64> {
    let kept: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM benchmark_runs")
        .fetch_one(&mut *conn)
        .await?;
    let mut last = kept;
    for entry in std::fs::read_dir(benchmark_runs_dir)
        .with_context(|| format!("reading {}", benchmark_runs_dir.display()))?
    {
        let name = entry?.file_name();
        if let Some(id) = name
            .to_str()
            .and_then(|name| name.parse::<BenchmarkRunId>().ok())
        {
            last = last.max(id.0);
        }
    }
    last.checked_add(1)
        .context("there is no ID left for a benchmark run")
}

/// Inserts the rows of the benchmark run `id` on `tasks` of the benchmark
/// `benchmark` under `configurations`, launched at `at`, with every trial
/// pending, and gives how many trials it makes.
async fn insert_benchmark_run(
    conn: &mut SqliteConnection,
    id: BenchmarkRunId,
    benchmark: &BenchmarkName,
    tasks: &[TaskName],
    configurations: &[ConfigurationName],
    repetitions: Repetitions,
    at: Timestamp,
) -> Result<usize> {
    sqlx::query(
        "INSERT INTO benchmark_runs (id, benchmark, launched_at, repetitions, state)
         VALUES (?, ?, ?, ?, 'open')",
    )
    .bind(id.0)
    .bind(benchmark.as_str())
    .bind(database::time_text(at))
    .bind(repetitions.get())
    .execute(&mut *conn)
    .await?;
    for (i, task) in tasks.iter().enumerate() {
        sqlx::query("INSERT INTO benchmark_run_tasks (benchmark_run, seq, name) VALUES (?, ?, ?)")
            .bind(id.0)
            .bind(seq(i))
            .bind(task.as_str())
            .execute(&mut *conn)
            .await?;
    }
    for (i, configuration) in configurations.iter().enumerate() {
        sqlx::query(
            "INSERT INTO benchmark_run_configurations (benchmark_run, seq, name) VALUES (?, ?, ?)",
        )
        .bind(id.0)
        .bind(seq(i))
        .bind(configuration.as_str())
        .execute(&mut *conn)
        .await?;
    }
    let mut trials = 0;
    for repetition in 1..=repetitions.get() {
        for task in 0..tasks.len() {
            for configuration in 0..configurations.len() {
                sqlx::query(
                    "INSERT INTO trials (benchmark_run, repetition, task_seq, configuration_seq, state)
                     VALUES (?, ?, ?, ?, 'pending')",
                )
                .bind(id.0)
                .bind(repetition)
                .bind(seq(task))
                .bind(seq(configuration))
                .execute(&mut *conn)
                .await?;
                trials += 1;
            }
        }
    }
    Ok(trials)
}

/// A place in a list, as a row keeps it.
fn seq(place: usize) -> i64 {
    i64::try_from(place).expect("a place in a list fits a row")
}

/// A place in a list a row keeps.
fn place(seq: i64) -> Result<usize> {
    usize::try_from(seq).with_context(|| format!("{seq} is not a place in a list"))
}

/// A name a row keeps, parsed.
fn parsed<T: std::str::FromStr<Err = String>>(name: &str) -> Result<T> {
    name.parse().map_err(anyhow::Error::msg)
}

/// The benchmark run `id` as its rows hold it, if there is one.
async fn read_record(
    conn: &mut SqliteConnection,
    id: BenchmarkRunId,
) -> Result<Option<BenchmarkRunRecord>> {
    let row: Option<(String, String, String, i64, String, Option<String>)> = sqlx::query_as(
        "SELECT benchmark_runs.benchmark, benchmarks.stage, benchmark_runs.launched_at,
                benchmark_runs.repetitions, benchmark_runs.state, benchmark_runs.ended_at
         FROM benchmark_runs JOIN benchmarks ON benchmarks.name = benchmark_runs.benchmark
         WHERE benchmark_runs.id = ?",
    )
    .bind(id.0)
    .fetch_optional(&mut *conn)
    .await
    .context(READING)?;
    let Some((benchmark, stage, launched_at, repetitions, state, ended_at)) = row else {
        return Ok(None);
    };
    let reading = || format!("reading benchmark run {id}");
    let tasks: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM benchmark_run_tasks WHERE benchmark_run = ? ORDER BY seq",
    )
    .bind(id.0)
    .fetch_all(&mut *conn)
    .await
    .with_context(reading)?;
    let configurations: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM benchmark_run_configurations WHERE benchmark_run = ? ORDER BY seq",
    )
    .bind(id.0)
    .fetch_all(&mut *conn)
    .await
    .with_context(reading)?;
    let trials = sqlx::query("SELECT * FROM trials WHERE benchmark_run = ?")
        .bind(id.0)
        .fetch_all(&mut *conn)
        .await
        .with_context(reading)?;
    let record = (|| {
        let tasks: Vec<TaskName> = tasks
            .iter()
            .map(|name| parsed(name))
            .collect::<Result<_>>()?;
        let configurations: Vec<ConfigurationName> = configurations
            .iter()
            .map(|name| parsed(name))
            .collect::<Result<_>>()?;
        let trials: BTreeMap<TrialKey, TrialState> =
            trials.iter().map(decode).collect::<Result<_>>()?;
        if let Some(key) = trials
            .keys()
            .find(|key| key.task >= tasks.len() || key.configuration >= configurations.len())
        {
            anyhow::bail!("a trial is of a task or a configuration it lacks: {key:?}");
        }
        Ok::<_, anyhow::Error>(BenchmarkRunRecord {
            id,
            benchmark: Benchmark {
                name: parsed(&benchmark)?,
                stage: parsed(&stage)?,
            },
            launched_at: database::parse_time(&launched_at)?,
            repetitions: u32::try_from(repetitions)
                .ok()
                .and_then(Repetitions::new)
                .with_context(|| format!("{repetitions} is not a number of repetitions"))?,
            state: BenchmarkRunState::parse(&state, ended_at.as_deref())?,
            tasks,
            configurations,
            trials,
        })
    })()
    .with_context(reading)?;
    Ok(Some(record))
}

/// A trial's state as its row holds it.
struct Columns {
    state: &'static str,
    outcome: Option<String>,
    money: Option<f64>,
    seconds: Option<f64>,
    correctness: Option<&'static str>,
    leak: Option<bool>,
    scorer_money: Option<f64>,
    unscored: Option<&'static str>,
    reason: Option<String>,
}

/// The trial a row of `trials` holds.
fn decode(row: &SqliteRow) -> Result<(TrialKey, TrialState)> {
    let repetition: i64 = row.try_get("repetition")?;
    let key = TrialKey {
        repetition: u32::try_from(repetition)
            .ok()
            .and_then(NonZeroU32::new)
            .with_context(|| format!("{repetition} is not a repetition"))?,
        task: place(row.try_get("task_seq")?)?,
        configuration: place(row.try_get("configuration_seq")?)?,
    };
    let state: String = row.try_get("state")?;
    let outcome: Option<String> = row.try_get("outcome")?;
    let seconds: Option<f64> = row.try_get("seconds")?;
    let trial = match (outcome, seconds) {
        (Some(outcome), Some(seconds)) => Some(TrialFigures {
            outcome: serde_json::from_value(serde_json::Value::String(outcome))?,
            money: money(row.try_get("money")?)?,
            time: Duration::try_from_secs_f64(seconds)?,
        }),
        _ => None,
    };
    let with_trial = |figures: Option<TrialFigures>| {
        figures.with_context(|| format!("a trial {state} has no run"))
    };
    let scorer_money = money(row.try_get("scorer_money")?)?;
    let unscored: Option<String> = row.try_get("unscored")?;
    let reason: Option<String> = row.try_get("reason")?;
    let trial = match state.as_str() {
        "pending" => TrialState::Pending,
        "running" => TrialState::Running,
        "ran" => TrialState::Ran {
            run: with_trial(trial)?,
        },
        "scoring" => TrialState::Scoring {
            run: with_trial(trial)?,
        },
        "scored" => {
            let correctness: Option<String> = row.try_get("correctness")?;
            TrialState::Ended(TrialEnd::Scored {
                run: with_trial(trial)?,
                score: ScoreFigures {
                    correctness: [Verdict::Pass, Verdict::Fail]
                        .into_iter()
                        .find(|verdict| Some(verdict.name()) == correctness.as_deref())
                        .with_context(|| format!("{correctness:?} is not a correctness"))?,
                    leak: row
                        .try_get::<Option<bool>, _>("leak")?
                        .context("a scored trial has no leak")?,
                    scorer_money,
                },
            })
        }
        "unscored" => match (unscored.as_deref(), reason, trial) {
            (Some("cancelled"), None, None) => TrialState::Ended(TrialEnd::Cancelled),
            (Some(cause), reason, Some(run)) => {
                TrialState::Ended(TrialEnd::Unscored(match (cause, reason) {
                    ("outcome", None) => NoScore::Outcome(run),
                    ("scorer", Some(reason)) => NoScore::Scorer {
                        run,
                        why: reason,
                        scorer_money,
                    },
                    ("cancelled", None) => NoScore::Cancelled(run),
                    _ => anyhow::bail!("{cause:?} is not why a trial has no score"),
                }))
            }
            (why, ..) => anyhow::bail!("{why:?} is not why a trial has no score"),
        },
        other => anyhow::bail!("{other:?} is not a trial's state"),
    };
    Ok((key, trial))
}

fn money(dollars: Option<f64>) -> Result<Option<Usd>> {
    dollars
        .map(Usd::try_from)
        .transpose()
        .map_err(anyhow::Error::msg)
}

impl Columns {
    fn of(state: &TrialState) -> Columns {
        let mut columns = Columns {
            state: "",
            outcome: None,
            money: None,
            seconds: None,
            correctness: None,
            leak: None,
            scorer_money: None,
            unscored: None,
            reason: None,
        };
        let (name, run) = match state {
            TrialState::Pending => ("pending", None),
            TrialState::Running => ("running", None),
            TrialState::Ran { run } => ("ran", Some(run)),
            TrialState::Scoring { run } => ("scoring", Some(run)),
            TrialState::Ended(TrialEnd::Scored { run, score }) => {
                columns.correctness = Some(score.correctness.name());
                columns.leak = Some(score.leak);
                columns.scorer_money = score.scorer_money.map(f64::from);
                ("scored", Some(run))
            }
            TrialState::Ended(TrialEnd::Unscored(no_score)) => {
                columns.unscored = Some(match no_score {
                    NoScore::Outcome(_) => "outcome",
                    NoScore::Scorer {
                        why, scorer_money, ..
                    } => {
                        columns.reason = Some(why.clone());
                        columns.scorer_money = scorer_money.map(f64::from);
                        "scorer"
                    }
                    NoScore::Cancelled(_) => "cancelled",
                });
                ("unscored", Some(no_score.run()))
            }
            TrialState::Ended(TrialEnd::Cancelled) => {
                columns.unscored = Some("cancelled");
                ("unscored", None)
            }
        };
        columns.state = name;
        if let Some(run) = run {
            columns.outcome = Some(outcome_name(run.outcome));
            columns.money = run.money.map(f64::from);
            columns.seconds = Some(run.time.as_secs_f64());
        }
        columns
    }
}

/// `outcome`'s name in JSON (`docs/specs/runs.md`, Ending).
pub(super) fn outcome_name(outcome: OutcomeName) -> String {
    match serde_json::to_value(outcome) {
        Ok(serde_json::Value::String(name)) => name,
        _ => unreachable!("an outcome's name is a string"),
    }
}

#[cfg(test)]
mod tests {
    use std::panic::Location;

    use super::*;

    /// The benchmark runs of a database in `work_dir` with the design benchmark
    /// `board` of the tasks `columns` and `lanes`, and a benchmark run
    /// launched on it with one repetition under `as-is` alone.
    #[track_caller]
    fn launched(work_dir: &Path) -> impl Future<Output = (BenchmarkRuns, BenchmarkRunId)> + '_ {
        let caller = Location::caller();
        async move {
            let db = Database::open(work_dir)
                .await
                .unwrap_or_else(|e| panic!("opening the database at {caller}: {e:#}"));
            sqlx::query("INSERT INTO benchmarks (name, stage) VALUES ('board', 'design')")
                .execute(db.pool())
                .await
                .unwrap_or_else(|e| panic!("keeping the benchmark at {caller}: {e}"));
            for (seq, task, run) in [(1, "columns", "3/design-1"), (2, "lanes", "4/design-1")] {
                std::fs::create_dir_all(work_dir.join(task::TASKS).join(task))
                    .unwrap_or_else(|e| panic!("making the task at {caller}: {e}"));
                sqlx::query(
                    "INSERT INTO tasks (name, benchmark, run, seq) VALUES (?, 'board', ?, ?)",
                )
                .bind(task)
                .bind(run)
                .bind(seq)
                .execute(db.pool())
                .await
                .unwrap_or_else(|e| panic!("keeping the task at {caller}: {e}"));
            }
            let benchmark_runs = BenchmarkRuns::new(db);
            let launched = launch(&benchmark_runs)
                .await
                .unwrap_or_else(|e| panic!("launching at {caller}: {e:#}"));
            (benchmark_runs, launched.id)
        }
    }

    async fn launch(benchmark_runs: &BenchmarkRuns) -> Result<Launched, LaunchRefusal> {
        benchmark_runs
            .launch(
                &"board".parse().expect("a benchmark's name"),
                vec![],
                Repetitions::MIN,
                "2026-11-17T12:00:00Z".parse().expect("a time"),
            )
            .await
    }

    /// The time at which the tests record each change, end and cancel.
    #[track_caller]
    fn later() -> Timestamp {
        "2026-11-17T13:00:00Z".parse().expect("a time")
    }

    fn trial(benchmark_run: BenchmarkRunId, task: usize) -> TrialRef {
        TrialRef {
            benchmark_run,
            key: TrialKey {
                repetition: NonZeroU32::MIN,
                task,
                configuration: 0,
            },
        }
    }

    fn figures(ending: Ending) -> TrialFigures {
        TrialFigures {
            outcome: OutcomeName::Ended(ending),
            money: Usd::new(0.25),
            time: Duration::from_millis(12_500),
        }
    }

    /// `docs/specs/benchmarks.md`, Benchmark run, A trial's transcripts: the
    /// scorer's, once it has started.
    #[test_log::test]
    fn a_trials_scorer_has_begun_once_it_is_scoring_unless_a_cancel_stopped_it() {
        let states = [
            TrialState::Ran {
                run: figures(Ending::Done),
            },
            TrialState::Scoring {
                run: figures(Ending::Done),
            },
            TrialState::Ended(TrialEnd::Scored {
                run: figures(Ending::Done),
                score: ScoreFigures {
                    correctness: Verdict::Pass,
                    leak: false,
                    scorer_money: None,
                },
            }),
            TrialState::Ended(TrialEnd::Unscored(NoScore::Scorer {
                run: figures(Ending::Done),
                why: "it gave up".into(),
                scorer_money: None,
            })),
            TrialState::Ended(TrialEnd::Unscored(NoScore::Outcome(figures(
                Ending::Failed,
            )))),
            TrialState::Ended(TrialEnd::Unscored(NoScore::Cancelled(figures(
                Ending::Done,
            )))),
        ];

        let began = states.map(|state| state.scorer_began());

        assert_eq!(began, [false, true, true, true, false, false]);
    }

    /// Records `changes` of `trial` in turn.
    #[track_caller]
    fn steps(
        benchmark_runs: &BenchmarkRuns,
        trial: TrialRef,
        changes: Vec<TrialChange>,
    ) -> impl Future<Output = ()> + '_ {
        let caller = Location::caller();
        async move {
            for change in changes {
                benchmark_runs
                    .step(&trial, change, later())
                    .await
                    .unwrap_or_else(|e| panic!("stepping at {caller}: {e:#}"));
            }
        }
    }

    #[track_caller]
    fn state_of(
        benchmark_runs: &BenchmarkRuns,
        trial: TrialRef,
    ) -> impl Future<Output = (BenchmarkRunState, TrialState)> + '_ {
        let caller = Location::caller();
        async move {
            let record = benchmark_runs
                .show(trial.benchmark_run)
                .await
                .unwrap_or_else(|e| panic!("reading the benchmark run at {caller}: {e:#}"))
                .unwrap_or_else(|| panic!("no benchmark run at {caller}"));
            (record.state, record.trials[&trial.key].clone())
        }
    }

    #[test_log::test(tokio::test)]
    async fn a_trial_steps_through_its_run_and_its_scorer_and_its_benchmark_run_ends_with_its_last()
    {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, id) = launched(work_dir.path()).await;
        let score = ScoreFigures {
            correctness: Verdict::Pass,
            leak: true,
            scorer_money: None,
        };

        let mut seen = vec![];
        for change in [
            TrialChange::RunStarts,
            TrialChange::RunEnds(figures(Ending::HandedBack)),
            TrialChange::ScorerStarts,
            TrialChange::Scored(score.clone()),
        ] {
            steps(&benchmark_runs, trial(id, 0), vec![change]).await;
            seen.push(state_of(&benchmark_runs, trial(id, 0)).await);
        }
        steps(
            &benchmark_runs,
            trial(id, 1),
            vec![
                TrialChange::RunStarts,
                TrialChange::RunEnds(figures(Ending::Done)),
                TrialChange::ScorerStarts,
                TrialChange::ScorerMakesNone {
                    why: "it gave up".into(),
                    scorer_money: Usd::new(0.5),
                },
            ],
        )
        .await;

        let open = BenchmarkRunState::Open;
        let handed_back = figures(Ending::HandedBack);
        assert_eq!(
            seen,
            [
                (open, TrialState::Running),
                (
                    open,
                    TrialState::Ran {
                        run: handed_back.clone()
                    }
                ),
                (
                    open,
                    TrialState::Scoring {
                        run: handed_back.clone()
                    }
                ),
                (
                    open,
                    TrialState::Ended(TrialEnd::Scored {
                        run: handed_back,
                        score,
                    })
                ),
            ]
        );
        assert_eq!(
            state_of(&benchmark_runs, trial(id, 1)).await,
            (
                BenchmarkRunState::Ended { at: Some(later()) },
                TrialState::Ended(TrialEnd::Unscored(NoScore::Scorer {
                    run: figures(Ending::Done),
                    why: "it gave up".into(),
                    scorer_money: Usd::new(0.5),
                }))
            )
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_trial_whose_run_is_not_done_or_handed_back_ends_its_trial_without_a_score() {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, id) = launched(work_dir.path()).await;

        steps(
            &benchmark_runs,
            trial(id, 0),
            vec![
                TrialChange::RunStarts,
                TrialChange::RunEnds(figures(Ending::Failed)),
            ],
        )
        .await;

        assert_eq!(
            state_of(&benchmark_runs, trial(id, 0)).await.1,
            TrialState::Ended(TrialEnd::Unscored(NoScore::Outcome(figures(
                Ending::Failed
            ))))
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_cancelled_trial_keeps_the_run_it_had_and_has_none_before_its_run_ended() {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, id) = launched(work_dir.path()).await;

        steps(
            &benchmark_runs,
            trial(id, 0),
            vec![
                TrialChange::RunStarts,
                TrialChange::RunEnds(figures(Ending::Done)),
                TrialChange::Cancelled,
            ],
        )
        .await;
        steps(
            &benchmark_runs,
            trial(id, 1),
            vec![TrialChange::RunStarts, TrialChange::Cancelled],
        )
        .await;

        assert_eq!(
            state_of(&benchmark_runs, trial(id, 0)).await.1,
            TrialState::Ended(TrialEnd::Unscored(NoScore::Cancelled(figures(
                Ending::Done
            ))))
        );
        assert_eq!(
            state_of(&benchmark_runs, trial(id, 1)).await.1,
            TrialState::Ended(TrialEnd::Cancelled)
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_change_that_is_not_of_a_trials_state_is_refused() {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, id) = launched(work_dir.path()).await;

        let scored = benchmark_runs
            .step(&trial(id, 0), TrialChange::ScorerStarts, later())
            .await;
        let missing = benchmark_runs
            .step(&trial(id, 2), TrialChange::RunStarts, later())
            .await;

        assert!(
            format!("{:#}", scored.unwrap_err()).contains("a trial pending cannot change"),
            "refused for another reason"
        );
        assert!(
            format!("{:#}", missing.unwrap_err()).contains("there is no such trial"),
            "refused for another reason"
        );
        assert_eq!(
            state_of(&benchmark_runs, trial(id, 0)).await,
            (BenchmarkRunState::Open, TrialState::Pending)
        );
    }

    #[test_log::test(tokio::test)]
    async fn the_trials_a_stop_interrupted_start_again() {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, id) = launched(work_dir.path()).await;
        steps(&benchmark_runs, trial(id, 0), vec![TrialChange::RunStarts]).await;
        steps(
            &benchmark_runs,
            trial(id, 1),
            vec![
                TrialChange::RunStarts,
                TrialChange::RunEnds(figures(Ending::Done)),
                TrialChange::ScorerStarts,
            ],
        )
        .await;

        benchmark_runs.interrupted().await.unwrap();

        assert_eq!(
            state_of(&benchmark_runs, trial(id, 0)).await.1,
            TrialState::Pending
        );
        assert_eq!(
            state_of(&benchmark_runs, trial(id, 1)).await.1,
            TrialState::Ran {
                run: figures(Ending::Done)
            }
        );
    }

    #[test_log::test(tokio::test)]
    async fn the_trials_a_stop_interrupted_of_a_cancelled_benchmark_run_end_cancelled() {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, id) = launched(work_dir.path()).await;
        steps(&benchmark_runs, trial(id, 0), vec![TrialChange::RunStarts]).await;
        steps(
            &benchmark_runs,
            trial(id, 1),
            vec![
                TrialChange::RunStarts,
                TrialChange::RunEnds(figures(Ending::Done)),
                TrialChange::ScorerStarts,
            ],
        )
        .await;
        benchmark_runs.cancel(id, later()).await.unwrap();

        benchmark_runs.interrupted().await.unwrap();

        assert_eq!(
            state_of(&benchmark_runs, trial(id, 0)).await,
            (
                BenchmarkRunState::Cancelled { at: Some(later()) },
                TrialState::Ended(TrialEnd::Cancelled)
            )
        );
        assert_eq!(
            state_of(&benchmark_runs, trial(id, 1)).await.1,
            TrialState::Ended(TrialEnd::Unscored(NoScore::Cancelled(figures(
                Ending::Done
            ))))
        );
    }

    #[test_log::test(tokio::test)]
    async fn the_open_benchmark_runs_are_those_not_ended_or_cancelled_oldest_first() {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, first) = launched(work_dir.path()).await;
        let cancelled = launch(&benchmark_runs).await.unwrap().id;
        let last = launch(&benchmark_runs).await.unwrap().id;
        benchmark_runs.cancel(cancelled, later()).await.unwrap();

        let open: Vec<BenchmarkRunId> = benchmark_runs
            .open()
            .await
            .unwrap()
            .into_iter()
            .map(|record| record.id)
            .collect();

        assert_eq!(open, [first, last]);
    }

    #[test_log::test(tokio::test)]
    async fn an_open_benchmark_run_is_not_kept_with_an_end() {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, id) = launched(work_dir.path()).await;

        let ended = sqlx::query("UPDATE benchmark_runs SET ended_at = ? WHERE id = ?")
            .bind(database::time_text(later()))
            .bind(id.0)
            .execute(benchmark_runs.db.pool())
            .await;

        assert!(ended.is_err(), "an open benchmark run was kept with an end");
        assert_eq!(
            state_of(&benchmark_runs, trial(id, 0)).await.0,
            BenchmarkRunState::Open
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_benchmark_runs_duration_runs_from_its_launch_to_its_end_or_to_now_while_it_is_open()
    {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, id) = launched(work_dir.path()).await;
        let mut record = benchmark_runs.show(id).await.unwrap().unwrap();
        let now: Timestamp = "2026-11-17T12:30:00Z".parse().unwrap();

        let open = record.duration(now);
        record.state = BenchmarkRunState::Ended { at: Some(later()) };
        let ended = record.duration(now);
        record.state = BenchmarkRunState::Cancelled { at: None };
        let unrecorded = record.duration(now);

        assert_eq!(open, Some(Duration::from_mins(30)));
        assert_eq!(ended, Some(Duration::from_hours(1)));
        assert_eq!(unrecorded, None);
    }
}
