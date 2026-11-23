//! Benchmark runs, kept in the database and in `benchmark-runs/` under the work
//! directory (`docs/specs/benchmarks.md`, Benchmark runs).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::{NonZeroU32, NonZeroUsize};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result};
use serde::Serialize;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row as _, SqliteConnection};

use crate::benchmarks::{Benchmark, BenchmarkId, TaskId};
use crate::cost::Usd;
use crate::database::{self, Database};
use crate::domain::Timestamp;
use crate::name::Name;
use crate::results::Verdict;
use crate::runs::{Ending, OutcomeName, Unended};
use crate::task;
use crate::tree;
use crate::trial::{self, Configuration};

/// A benchmark run's ID: a positive number, given at its launch.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, schemars::JsonSchema,
)]
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

/// The name of the configuration that holds nothing, first in every
/// benchmark run.
pub fn as_is_name() -> Name {
    Name::parse("as-is").expect("as-is is a name")
}

/// A configuration's ID: its place in its benchmark run, from 1 for `as-is`
/// (`docs/specs/benchmarks.md`, Names and IDs).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct ConfigurationId(NonZeroUsize);

impl ConfigurationId {
    /// The ID of `as-is`, first in every benchmark run.
    pub const AS_IS: ConfigurationId = ConfigurationId(NonZeroUsize::MIN);

    /// The ID of the configuration at `place` in its benchmark run's, from 0.
    pub fn of_place(place: usize) -> ConfigurationId {
        ConfigurationId(NonZeroUsize::MIN.saturating_add(place))
    }

    /// Its place in its benchmark run's configurations, from 0.
    pub fn place(self) -> usize {
        self.0.get() - 1
    }
}

impl std::str::FromStr for ConfigurationId {
    type Err = String;

    fn from_str(id: &str) -> Result<ConfigurationId, String> {
        let place: usize = super::positive(id, "a configuration's")?;
        NonZeroUsize::new(place)
            .map(ConfigurationId)
            .ok_or_else(|| format!("{id:?} is not a configuration's ID"))
    }
}

impl fmt::Display for ConfigurationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A task of a benchmark run: its ID, and its name as it is now, or as it
/// was when it was removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct RunTask {
    pub id: TaskId,
    pub name: Name,
}

/// A trial's name outside the rows of its task and its configuration:
/// "trial N of TASK under CONFIGURATION", by its repetition and the names of
/// its task and its configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrialName {
    repetition: NonZeroU32,
    task: Name,
    configuration: Name,
}

impl TrialName {
    pub fn new(repetition: NonZeroU32, task: &Name, configuration: &Name) -> TrialName {
        TrialName {
            repetition,
            task: task.clone(),
            configuration: configuration.clone(),
        }
    }

    /// The name for the start of a sentence or a heading.
    pub fn capitalized(&self) -> String {
        self.with("Trial")
    }

    /// The name, beginning with `trial`, the word for a trial.
    fn with(&self, trial: &str) -> String {
        format!(
            "{trial} {} of {} under {}",
            self.repetition, self.task, self.configuration
        )
    }
}

impl fmt::Display for TrialName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.with("trial"))
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
            TrialState::Pending | TrialState::Running => None,
            TrialState::Ended(TrialEnd::Cancelled(cancelled)) => cancelled.run(),
            TrialState::Ran { run }
            | TrialState::Scoring { run }
            | TrialState::Ended(TrialEnd::Scored { run, .. }) => Some(run),
            TrialState::Ended(TrialEnd::Unscored(no_score)) => no_score.run(),
        }
    }

    /// Its time, once it has one: its run's, or the time to a build that
    /// failed.
    pub fn time(&self) -> Option<Duration> {
        if let TrialState::Ended(TrialEnd::Unscored(NoScore::BuildFailed { time })) = self {
            Some(*time)
        } else {
            self.run().map(|run| run.time)
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
            | TrialState::Ended(TrialEnd::Unscored(_) | TrialEnd::Cancelled(_)) => None,
        }
    }

    /// Whether it ended without its run ever starting, though its output
    /// may have begun: its build failed, or the cancel came first.
    pub fn run_never_started(&self) -> bool {
        matches!(
            self,
            TrialState::Ended(
                TrialEnd::Unscored(NoScore::BuildFailed { .. })
                    | TrialEnd::Cancelled(Cancelled::BeforeRun)
            )
        )
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
    /// It has no score for a reason.
    Unscored(NoScore),
    /// Its benchmark run's cancel ended it.
    Cancelled(Cancelled),
}

/// A trial its benchmark run's cancel ended: no score and no reason.
#[derive(Clone, Debug, PartialEq)]
pub enum Cancelled {
    /// Before its run started.
    BeforeRun,
    /// The cancel stopped its run, with what that run spent until it
    /// stopped. Its outcome is never done or handed back.
    RunStopped(TrialFigures),
    /// Its run ended, to be scored, and the cancel came before its scorer
    /// made a score.
    AfterRun(TrialFigures),
}

impl Cancelled {
    /// A cancelled trial whose run ended with `run`: after its run when
    /// that run is to be scored, and stopped by the cancel otherwise.
    pub fn of_run(run: TrialFigures) -> Cancelled {
        if run.scoreable() {
            Cancelled::AfterRun(run)
        } else {
            Cancelled::RunStopped(run)
        }
    }

    /// The figures of its run, if it has them.
    pub fn run(&self) -> Option<&TrialFigures> {
        match self {
            Cancelled::BeforeRun => None,
            Cancelled::RunStopped(run) | Cancelled::AfterRun(run) => Some(run),
        }
    }
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

/// Why a trial that ended has no score (`docs/specs/benchmarks.md`, Benchmark
/// runs, A trial without a score), with the figures it has.
#[derive(Clone, Debug, PartialEq)]
pub enum NoScore {
    /// The build of its environment failed, so it has no run, and no cost:
    /// the time it took until then.
    BuildFailed { time: Duration },
    /// Its run is not done or handed back.
    Outcome(TrialFigures),
    /// Its scorer made no score.
    Scorer {
        run: TrialFigures,
        why: String,
        /// `None` when the scorer's session has no cost.
        scorer_money: Option<Usd>,
    },
}

impl NoScore {
    /// The figures of its run, if it began.
    pub fn run(&self) -> Option<&TrialFigures> {
        match self {
            NoScore::BuildFailed { .. } => None,
            NoScore::Outcome(run) | NoScore::Scorer { run, .. } => Some(run),
        }
    }

    /// The reason the trial shows (`docs/specs/benchmarks.md`, Benchmark run,
    /// A trial's reason).
    pub fn reason(&self) -> String {
        match self {
            NoScore::BuildFailed { .. } => "No score. The build of its environment failed.".into(),
            NoScore::Outcome(run) => {
                format!("No score. The run's outcome: {}", run.outcome.word())
            }
            NoScore::Scorer { why, .. } => format!("No score. {why}"),
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
    pub tasks: Vec<RunTask>,
    /// In the order of their IDs, `as-is` first.
    pub configurations: Vec<Name>,
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

    /// What its trial `key` is of; `None` when it lacks the trial's
    /// configuration or task.
    pub fn trial_of(&self, key: &TrialKey) -> Option<TrialOf<'_>> {
        Some(TrialOf {
            configuration_name: self.configurations.get(key.configuration)?,
            configuration: ConfigurationId::of_place(key.configuration),
            task: self.tasks.get(key.task)?,
        })
    }
}

/// What a trial of a benchmark run is of: a configuration and a task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrialOf<'a> {
    pub configuration: ConfigurationId,
    pub configuration_name: &'a Name,
    pub task: &'a RunTask,
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
    NoSuchBenchmark(BenchmarkId),
    #[error("the benchmark {0} has no tasks")]
    NoTasks(Name),
    #[error("two configurations are named {0}")]
    DuplicateConfiguration(Name),
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

/// Why a configuration of a benchmark run was not renamed.
#[derive(Debug, thiserror::Error)]
pub enum RenameConfigurationError {
    #[error("there is no benchmark run {0}")]
    NoSuchBenchmarkRun(BenchmarkRunId),
    #[error("benchmark run {id} has no configuration {configuration}")]
    NoSuchConfiguration {
        id: BenchmarkRunId,
        configuration: ConfigurationId,
    },
    #[error("the configuration {}, as-is, keeps its name", ConfigurationId::AS_IS)]
    AsIs,
    #[error("another configuration is named {0}")]
    Taken(Name),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// A change of a trial's state.
#[derive(Clone, Debug, PartialEq)]
pub enum TrialChange {
    /// Pending, its run starts.
    RunStarts,
    /// Running, the build of its environment fails: ended without a score,
    /// with the time it took.
    BuildFails { time: Duration },
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
    /// Not ended, its benchmark run is cancelled. Of a running trial, with
    /// the figures of the run the cancel stopped: without them, its run
    /// stopped with no money and no time.
    Cancelled { stopped: Option<TrialFigures> },
    /// Running, its benchmark run is cancelled and the build of its
    /// environment failed, so its run never started.
    CancelledBeforeRun,
}

impl TrialChange {
    /// What a step that ended once its benchmark run was cancelled records,
    /// given the change it made, if any: cancelled, with the run the cancel
    /// stopped when it has one.
    pub fn on_cancel(change: Option<TrialChange>) -> TrialChange {
        match change {
            Some(TrialChange::RunEnds(run)) => TrialChange::Cancelled { stopped: Some(run) },
            Some(TrialChange::BuildFails { .. }) => TrialChange::CancelledBeforeRun,
            Some(
                TrialChange::RunStarts
                | TrialChange::ScorerStarts
                | TrialChange::Scored(_)
                | TrialChange::ScorerMakesNone { .. }
                | TrialChange::Cancelled { .. }
                | TrialChange::CancelledBeforeRun,
            )
            | None => TrialChange::Cancelled { stopped: None },
        }
    }
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
    pub fn task_dir(&self, id: BenchmarkRunId, task: TaskId) -> PathBuf {
        kept_task(&self.dir(id), task)
    }

    /// The benchmark run `id`'s copy of the configuration `configuration`,
    /// laid out as a repository's `.linewright/`.
    pub fn configuration_dir(&self, id: BenchmarkRunId, configuration: ConfigurationId) -> PathBuf {
        kept_configuration(&self.dir(id), configuration)
    }

    /// The output of the trial of `task` under `configuration` in the
    /// repetition `repetition` of the benchmark run `id`
    /// (`docs/specs/benchmarks.md`, Benchmark runs, The output).
    pub fn output_dir(
        &self,
        id: BenchmarkRunId,
        configuration: ConfigurationId,
        task: TaskId,
        repetition: NonZeroU32,
    ) -> PathBuf {
        self.dir(id)
            .join(configuration.to_string())
            .join(task.to_string())
            .join(repetition.to_string())
    }

    /// The output of the trial `key` of `record`; `None` when `record` lacks
    /// the trial's configuration or task.
    pub fn trial_output(&self, record: &BenchmarkRunRecord, key: &TrialKey) -> Option<PathBuf> {
        let of = record.trial_of(key)?;
        Some(self.output_dir(record.id, of.configuration, of.task.id, key.repetition))
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
        benchmark: BenchmarkId,
        configurations: Vec<(Name, Configuration)>,
        repetitions: Repetitions,
        at: Timestamp,
    ) -> Result<Launched, LaunchRefusal> {
        let mut names = BTreeSet::from([as_is_name()]);
        for (name, _) in &configurations {
            if !names.insert(name.clone()) {
                return Err(LaunchRefusal::DuplicateConfiguration(name.clone()));
            }
        }
        let launching = format!("launching a benchmark run on the benchmark {benchmark}");
        let mut conn = self.db.pool().acquire().await.context(launching.clone())?;
        let tasks = benchmark_tasks(&mut conn, benchmark).await;
        drop(conn);
        let Some((found, tasks)) = tasks.context(launching.clone())? else {
            return Err(LaunchRefusal::NoSuchBenchmark(benchmark));
        };
        if tasks.is_empty() {
            return Err(LaunchRefusal::NoTasks(found.name));
        }
        let built = self
            .build(&tasks, &configurations)
            .await
            .context(launching.clone())?;
        let configuration_names: Vec<Name> = std::iter::once(as_is_name())
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
        tasks: &[TaskId],
        configurations: &[(Name, Configuration)],
    ) -> Result<Built> {
        let benchmark_runs_dir = self.benchmark_runs_dir();
        std::fs::create_dir_all(&benchmark_runs_dir)
            .with_context(|| format!("making {}", benchmark_runs_dir.display()))?;
        let dir = tempfile::Builder::new()
            .prefix(".launching-")
            .tempdir_in(&benchmark_runs_dir)?;
        let tasks_dir = self.db.work_dir().join(task::TASKS);
        let mut copied = Vec::with_capacity(tasks.len());
        for &task in tasks {
            let from = tasks_dir.join(task.to_string());
            let before = TaskDir::of(&from)?;
            tree::copy(&from, &kept_task(dir.path(), task))
                .await
                .with_context(|| format!("copying the task {task} from {}", from.display()))?;
            copied.push(before);
        }
        let as_is = kept_configuration(dir.path(), ConfigurationId::AS_IS);
        std::fs::create_dir_all(&as_is).with_context(|| format!("making {}", as_is.display()))?;
        let after_as_is = (1..).map(ConfigurationId::of_place);
        for (id, (name, configuration)) in after_as_is.zip(configurations) {
            configuration
                .keep(&kept_configuration(dir.path(), id))
                .with_context(|| format!("copying the configuration {name}"))?;
        }
        Ok(Built { dir, copied })
    }

    /// Keeps the benchmark run whose copies are `built`, in one transaction:
    /// its rows are inserted and `built` is renamed into its directory, as
    /// long as the benchmark's tasks are still those it copied.
    async fn keep(
        &self,
        benchmark: BenchmarkId,
        tasks: &[TaskId],
        configurations: &[Name],
        repetitions: Repetitions,
        at: Timestamp,
        built: Built,
    ) -> Result<Launched> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let now = benchmark_tasks(&mut tx, benchmark).await?;
        let tasks_dir = self.db.work_dir().join(task::TASKS);
        let unchanged = now.as_ref().map(|(_, now)| now.as_slice()) == Some(tasks)
            && tasks.iter().zip(&built.copied).all(|(task, copied)| {
                TaskDir::of(&tasks_dir.join(task.to_string())).ok().as_ref() == Some(copied)
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
    pub async fn of_benchmark(&self, benchmark: BenchmarkId) -> Result<Vec<BenchmarkRunRecord>> {
        self.records_of(
            "SELECT id FROM benchmark_runs WHERE benchmark = ? ORDER BY id DESC",
            Some(benchmark.row()),
        )
        .await
    }

    /// Each benchmark's latest benchmark run, with when it was launched, by the
    /// benchmark's ID: none for a benchmark with no benchmark run.
    pub async fn latest(&self) -> Result<BTreeMap<BenchmarkId, (BenchmarkRunId, Timestamp)>> {
        let rows: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT benchmark, id, launched_at FROM benchmark_runs
             WHERE id IN (SELECT MAX(id) FROM benchmark_runs GROUP BY benchmark)",
        )
        .fetch_all(self.db.pool())
        .await
        .context(READING)?;
        rows.into_iter()
            .map(|(benchmark, id, launched_at)| {
                Ok((
                    BenchmarkId::from_row(benchmark)?,
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
        bound: Option<i64>,
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

    /// Changes the name of the configuration `configuration` of the benchmark
    /// run `id` to `name`, also after its launch. Refuses `as-is` and a name
    /// another configuration of the benchmark run has. Of two changes at once,
    /// the last is kept.
    pub async fn rename_configuration(
        &self,
        id: BenchmarkRunId,
        configuration: ConfigurationId,
        name: &Name,
    ) -> Result<(), RenameConfigurationError> {
        let renaming =
            format!("renaming the configuration {configuration} of benchmark run {id} to {name}");
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .context(renaming.clone())?;
        if read_state(&mut tx, id)
            .await
            .context(renaming.clone())?
            .is_none()
        {
            return Err(RenameConfigurationError::NoSuchBenchmarkRun(id));
        }
        if configuration == ConfigurationId::AS_IS {
            return Err(RenameConfigurationError::AsIs);
        }
        let Ok(place) = i64::try_from(configuration.place()) else {
            return Err(RenameConfigurationError::NoSuchConfiguration { id, configuration });
        };
        let held: Option<i64> = sqlx::query_scalar(
            "SELECT seq FROM benchmark_run_configurations WHERE benchmark_run = ? AND seq = ?",
        )
        .bind(id.0)
        .bind(place)
        .fetch_optional(&mut *tx)
        .await
        .context(renaming.clone())?;
        if held.is_none() {
            return Err(RenameConfigurationError::NoSuchConfiguration { id, configuration });
        }
        let other: Option<i64> = sqlx::query_scalar(
            "SELECT seq FROM benchmark_run_configurations
             WHERE benchmark_run = ? AND name = ? AND seq <> ?",
        )
        .bind(id.0)
        .bind(name.as_str())
        .bind(place)
        .fetch_optional(&mut *tx)
        .await
        .context(renaming.clone())?;
        if other.is_some() {
            return Err(RenameConfigurationError::Taken(name.clone()));
        }
        sqlx::query(
            "UPDATE benchmark_run_configurations SET name = ? WHERE benchmark_run = ? AND seq = ?",
        )
        .bind(name.as_str())
        .bind(id.0)
        .bind(place)
        .execute(&mut *tx)
        .await
        .context(renaming.clone())?;
        tx.commit().await.context(renaming)?;
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
    /// end as cancelled instead, a running one with what its output kept of
    /// its cost.
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
                "SELECT trials.*, benchmark_run_tasks.task AS task_id
                 FROM trials
                 JOIN benchmark_runs ON benchmark_runs.id = trials.benchmark_run
                 JOIN benchmark_run_tasks
                     ON benchmark_run_tasks.benchmark_run = trials.benchmark_run
                     AND benchmark_run_tasks.seq = trials.task_seq
                 WHERE trials.state IN ('running', 'scoring') AND benchmark_runs.state != 'open'",
            )
            .fetch_all(&mut *tx)
            .await?;
            for row in &left {
                let benchmark_run = BenchmarkRunId::from_row(row.try_get("benchmark_run")?)?;
                let (key, state) = decode(row)?;
                let stopped = if state == TrialState::Running {
                    let output = self.output_dir(
                        benchmark_run,
                        ConfigurationId::of_place(key.configuration),
                        TaskId::from_row(row.try_get("task_id")?)?,
                        key.repetition,
                    );
                    Some(interrupted_run(trial::kept_spent(&output)))
                } else {
                    None
                };
                write_trial(
                    &mut tx,
                    benchmark_run,
                    &key,
                    &state.changed(TrialChange::Cancelled { stopped })?,
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
            (TrialState::Running, TrialChange::BuildFails { time }) => {
                TrialState::Ended(TrialEnd::Unscored(NoScore::BuildFailed { time }))
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
            (TrialState::Pending, TrialChange::Cancelled { stopped: None })
            | (TrialState::Running, TrialChange::CancelledBeforeRun) => {
                TrialState::Ended(TrialEnd::Cancelled(Cancelled::BeforeRun))
            }
            (TrialState::Running, TrialChange::Cancelled { stopped }) => {
                TrialState::Ended(TrialEnd::Cancelled(Cancelled::of_run(
                    stopped.unwrap_or_else(|| interrupted_run(None)),
                )))
            }
            (
                TrialState::Ran { run } | TrialState::Scoring { run },
                TrialChange::Cancelled { stopped: None },
            ) => TrialState::Ended(TrialEnd::Cancelled(Cancelled::AfterRun(run))),
            (state, change) => anyhow::bail!(
                "a trial {} cannot change by {change:?}",
                Columns::of(&state).state
            ),
        })
    }
}

/// The figures of a run a stop interrupted, which spent `spent`; with no
/// money and no time when what it spent is not known.
fn interrupted_run(spent: Option<trial::Spent>) -> TrialFigures {
    TrialFigures {
        outcome: OutcomeName::Unended(Unended::Interrupted),
        money: spent.and_then(|spent| spent.money),
        time: spent.map_or(Duration::ZERO, |spent| spent.time),
    }
}

/// The copies a benchmark run keeps, built in a directory of their own.
struct Built {
    dir: tempfile::TempDir,
    /// The directory of each task copied, in the order of the tasks.
    copied: Vec<TaskDir>,
}

/// Which directory a task's is, and which file its criteria: another
/// directory in its place, such as of a task removed and captured again, is
/// not the same, nor are criteria changed since, which are renamed into
/// place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TaskDir {
    dev: u64,
    ino: u64,
    criteria: Option<u64>,
}

impl TaskDir {
    fn of(dir: &Path) -> Result<TaskDir> {
        let metadata =
            std::fs::symlink_metadata(dir).with_context(|| format!("reading {}", dir.display()))?;
        let criteria = std::fs::symlink_metadata(dir.join(task::CRITERIA_FILE))
            .ok()
            .map(|criteria| criteria.ino());
        Ok(TaskDir {
            dev: metadata.dev(),
            ino: metadata.ino(),
            criteria,
        })
    }
}

/// The copy of the task `task` in the benchmark run's directory `dir`.
fn kept_task(dir: &Path, task: TaskId) -> PathBuf {
    dir.join(KEPT).join(task::TASKS).join(task.to_string())
}

/// The copy of the configuration `configuration` in the benchmark run's
/// directory `dir`.
fn kept_configuration(dir: &Path, configuration: ConfigurationId) -> PathBuf {
    dir.join(KEPT)
        .join(CONFIGURATIONS)
        .join(configuration.to_string())
}

/// The benchmark `benchmark` with its tasks, in its order, or `None` when
/// there is no such benchmark.
async fn benchmark_tasks(
    conn: &mut SqliteConnection,
    benchmark: BenchmarkId,
) -> Result<Option<(Benchmark, Vec<TaskId>)>> {
    let Some(found) = super::find_benchmark(&mut *conn, benchmark).await? else {
        return Ok(None);
    };
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM tasks WHERE benchmark = ? ORDER BY seq")
        .bind(benchmark.row())
        .fetch_all(&mut *conn)
        .await
        .context(READING)?;
    let ids = ids
        .into_iter()
        .map(TaskId::from_row)
        .collect::<Result<_>>()?;
    Ok(Some((found, ids)))
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
    benchmark: BenchmarkId,
    tasks: &[TaskId],
    configurations: &[Name],
    repetitions: Repetitions,
    at: Timestamp,
) -> Result<usize> {
    sqlx::query(
        "INSERT INTO benchmark_runs (id, benchmark, launched_at, repetitions, state)
         VALUES (?, ?, ?, ?, 'open')",
    )
    .bind(id.0)
    .bind(benchmark.row())
    .bind(database::time_text(at))
    .bind(repetitions.get())
    .execute(&mut *conn)
    .await?;
    for (i, task) in tasks.iter().enumerate() {
        sqlx::query("INSERT INTO benchmark_run_tasks (benchmark_run, seq, task) VALUES (?, ?, ?)")
            .bind(id.0)
            .bind(seq(i))
            .bind(task.row())
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

/// A benchmark run's row with its benchmark's: the benchmark's ID, name and
/// stage, then the benchmark run's launch, repetitions, state and end.
type BenchmarkRunRow = (i64, String, String, String, i64, String, Option<String>);

/// The benchmark run `id` as its rows hold it, if there is one.
async fn read_record(
    conn: &mut SqliteConnection,
    id: BenchmarkRunId,
) -> Result<Option<BenchmarkRunRecord>> {
    let row: Option<BenchmarkRunRow> = sqlx::query_as(
        "SELECT benchmarks.id, benchmarks.name, benchmarks.stage, benchmark_runs.launched_at,
                benchmark_runs.repetitions, benchmark_runs.state, benchmark_runs.ended_at
         FROM benchmark_runs JOIN benchmarks ON benchmarks.id = benchmark_runs.benchmark
         WHERE benchmark_runs.id = ?",
    )
    .bind(id.0)
    .fetch_optional(&mut *conn)
    .await
    .context(READING)?;
    let Some((benchmark, benchmark_name, stage, launched_at, repetitions, state, ended_at)) = row
    else {
        return Ok(None);
    };
    let reading = || format!("reading benchmark run {id}");
    let tasks: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT benchmark_run_tasks.task, COALESCE(tasks.name, removed_tasks.name)
         FROM benchmark_run_tasks
         LEFT JOIN tasks ON tasks.id = benchmark_run_tasks.task
         LEFT JOIN removed_tasks ON removed_tasks.id = benchmark_run_tasks.task
         WHERE benchmark_run_tasks.benchmark_run = ? ORDER BY benchmark_run_tasks.seq",
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
        let tasks: Vec<RunTask> = tasks
            .into_iter()
            .map(|(task, task_name)| {
                let task = TaskId::from_row(task)?;
                Ok(RunTask {
                    id: task,
                    name: Name::parse(
                        &task_name.with_context(|| format!("the task {task} has no name"))?,
                    )?,
                })
            })
            .collect::<Result<_>>()?;
        let configurations: Vec<Name> = configurations
            .iter()
            .map(|name| Ok(Name::parse(name)?))
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
            benchmark: super::benchmark_of(benchmark, &benchmark_name, &stage)?,
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
            (Some("cancelled"), None, run) => TrialState::Ended(TrialEnd::Cancelled(
                run.map_or(Cancelled::BeforeRun, Cancelled::of_run),
            )),
            (Some("build"), None, None) => {
                TrialState::Ended(TrialEnd::Unscored(NoScore::BuildFailed {
                    time: Duration::try_from_secs_f64(
                        seconds.context("a trial whose build failed has no time")?,
                    )?,
                }))
            }
            (Some(cause), reason, Some(run)) => {
                TrialState::Ended(TrialEnd::Unscored(match (cause, reason) {
                    ("outcome", None) => NoScore::Outcome(run),
                    ("scorer", Some(reason)) => NoScore::Scorer {
                        run,
                        why: reason,
                        scorer_money,
                    },
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
                    NoScore::BuildFailed { time } => {
                        columns.seconds = Some(time.as_secs_f64());
                        "build"
                    }
                    NoScore::Outcome(_) => "outcome",
                    NoScore::Scorer {
                        why, scorer_money, ..
                    } => {
                        columns.reason = Some(why.clone());
                        columns.scorer_money = scorer_money.map(f64::from);
                        "scorer"
                    }
                });
                ("unscored", no_score.run())
            }
            TrialState::Ended(TrialEnd::Cancelled(cancelled)) => {
                columns.unscored = Some("cancelled");
                ("unscored", cancelled.run())
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
    /// `board`, 1, of the tasks `columns`, 1, and `lanes`, 2, and a benchmark
    /// run launched on it with one repetition under `as-is` alone.
    #[track_caller]
    fn launched(work_dir: &Path) -> impl Future<Output = (BenchmarkRuns, BenchmarkRunId)> + '_ {
        let caller = Location::caller();
        async move {
            let db = Database::open(work_dir)
                .await
                .unwrap_or_else(|e| panic!("opening the database at {caller}: {e:#}"));
            sqlx::query("INSERT INTO benchmarks (id, name, stage) VALUES (1, 'board', 'design')")
                .execute(db.pool())
                .await
                .unwrap_or_else(|e| panic!("keeping the benchmark at {caller}: {e}"));
            for (id, task, run) in [(1, "columns", "3/design-1"), (2, "lanes", "4/design-1")] {
                std::fs::create_dir_all(work_dir.join(task::TASKS).join(id.to_string()))
                    .unwrap_or_else(|e| panic!("making the task at {caller}: {e}"));
                sqlx::query(
                    "INSERT INTO tasks (id, name, benchmark, run, seq) VALUES (?1, ?2, 1, ?3, ?1)",
                )
                .bind(id)
                .bind(task)
                .bind(run)
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
                "1".parse().expect("a benchmark's ID"),
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
            TrialState::Ended(TrialEnd::Cancelled(Cancelled::AfterRun(figures(
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
    async fn a_cancelled_trial_keeps_the_run_it_had_or_the_cancel_stopped() {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, id) = launched(work_dir.path()).await;

        steps(
            &benchmark_runs,
            trial(id, 0),
            vec![
                TrialChange::RunStarts,
                TrialChange::RunEnds(figures(Ending::Done)),
                TrialChange::Cancelled { stopped: None },
            ],
        )
        .await;
        steps(
            &benchmark_runs,
            trial(id, 1),
            vec![
                TrialChange::RunStarts,
                TrialChange::Cancelled {
                    stopped: Some(figures(Ending::Cancelled)),
                },
            ],
        )
        .await;

        assert_eq!(
            state_of(&benchmark_runs, trial(id, 0)).await.1,
            TrialState::Ended(TrialEnd::Cancelled(Cancelled::AfterRun(figures(
                Ending::Done
            ))))
        );
        assert_eq!(
            state_of(&benchmark_runs, trial(id, 1)).await.1,
            TrialState::Ended(TrialEnd::Cancelled(Cancelled::RunStopped(figures(
                Ending::Cancelled
            ))))
        );
    }

    /// `docs/specs/benchmarks.md`, Benchmark runs, A trial without a score:
    /// its build failed, kept with its time, read the same after a restart.
    #[test_log::test(tokio::test)]
    async fn a_trial_whose_build_fails_ends_without_a_score_kept_across_a_restart() {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, id) = launched(work_dir.path()).await;
        let time = Duration::from_millis(12_500);

        steps(
            &benchmark_runs,
            trial(id, 0),
            vec![TrialChange::RunStarts, TrialChange::BuildFails { time }],
        )
        .await;
        drop(benchmark_runs);
        let restarted = BenchmarkRuns::new(Database::open(work_dir.path()).await.unwrap());

        assert_eq!(
            state_of(&restarted, trial(id, 0)).await.1,
            TrialState::Ended(TrialEnd::Unscored(NoScore::BuildFailed { time }))
        );
    }

    /// `docs/specs/benchmarks.md`, Benchmark runs, Cancelled trials: a
    /// trial with no score and no reason at the cancel is cancelled, also when
    /// its build fails after it, and keeps the run the cancel stopped, if any.
    #[test_log::test(tokio::test)]
    async fn a_step_that_ends_once_its_benchmark_run_is_cancelled_ends_its_trial_cancelled() {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, id) = launched(work_dir.path()).await;
        let build_fails = TrialChange::BuildFails {
            time: Duration::from_secs(5),
        };
        let run_ends = TrialChange::RunEnds(figures(Ending::Cancelled));

        for (task, change) in [(0, build_fails), (1, run_ends)] {
            steps(
                &benchmark_runs,
                trial(id, task),
                vec![TrialChange::RunStarts, TrialChange::on_cancel(Some(change))],
            )
            .await;
        }

        assert_eq!(
            [
                state_of(&benchmark_runs, trial(id, 0)).await.1,
                state_of(&benchmark_runs, trial(id, 1)).await.1,
            ],
            [
                TrialState::Ended(TrialEnd::Cancelled(Cancelled::BeforeRun)),
                TrialState::Ended(TrialEnd::Cancelled(Cancelled::RunStopped(figures(
                    Ending::Cancelled
                )))),
            ]
        );
    }

    /// A kept cancelled row is `unscored = 'cancelled'`, with its run's
    /// figures when it has them, whose outcome tells a run that ended from one
    /// the cancel stopped.
    #[test_log::test(tokio::test)]
    async fn kept_cancelled_trials_read_as_cancelled_before_their_run_after_it_or_stopping_it() {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, id) = launched(work_dir.path()).await;
        let cases = [
            (
                Some(("done", 0.25, 12.5)),
                Cancelled::AfterRun(figures(Ending::Done)),
            ),
            (
                Some(("cancelled", 0.25, 12.5)),
                Cancelled::RunStopped(figures(Ending::Cancelled)),
            ),
            (None, Cancelled::BeforeRun),
        ];

        for (run, cancelled) in cases {
            sqlx::query(
                "UPDATE trials SET state = 'unscored', unscored = 'cancelled',
                     outcome = ?, money = ?, seconds = ?
                 WHERE benchmark_run = ? AND task_seq = 0",
            )
            .bind(run.map(|(outcome, ..)| outcome))
            .bind(run.map(|(_, money, _)| money))
            .bind(run.map(|(.., seconds)| seconds))
            .bind(id.0)
            .execute(benchmark_runs.db.pool())
            .await
            .unwrap();

            assert_eq!(
                state_of(&benchmark_runs, trial(id, 0)).await.1,
                TrialState::Ended(TrialEnd::Cancelled(cancelled)),
                "{run:?}"
            );
        }
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

    /// `docs/specs/benchmarks.md`, Benchmark runs, Cancelled trials: a run
    /// that a stop left in flight, of a benchmark run cancelled while `serve`
    /// was down, stopped and keeps what its output says it cost.
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
        let output = benchmark_runs.output_dir(
            id,
            ConfigurationId::AS_IS,
            "1".parse().unwrap(),
            NonZeroU32::MIN,
        );
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(
            output.join("cost.json"),
            r#"{"seconds": 12.5, "run": {"seconds": 10, "models": {}, "all": {"usd": 0.25}}}"#,
        )
        .unwrap();
        benchmark_runs.cancel(id, later()).await.unwrap();

        benchmark_runs.interrupted().await.unwrap();

        assert_eq!(
            state_of(&benchmark_runs, trial(id, 0)).await,
            (
                BenchmarkRunState::Cancelled { at: Some(later()) },
                TrialState::Ended(TrialEnd::Cancelled(Cancelled::RunStopped(TrialFigures {
                    outcome: OutcomeName::Unended(Unended::Interrupted),
                    money: Usd::new(0.25),
                    time: Duration::from_millis(12_500),
                })))
            )
        );
        assert_eq!(
            state_of(&benchmark_runs, trial(id, 1)).await.1,
            TrialState::Ended(TrialEnd::Cancelled(Cancelled::AfterRun(figures(
                Ending::Done
            ))))
        );
    }

    /// Without a cost its output kept, a stopped run has no money and no time.
    #[test_log::test(tokio::test)]
    async fn a_trial_a_stop_interrupted_of_a_cancelled_benchmark_run_with_no_kept_cost_has_none() {
        let work_dir = tempfile::tempdir().unwrap();
        let (benchmark_runs, id) = launched(work_dir.path()).await;
        steps(&benchmark_runs, trial(id, 0), vec![TrialChange::RunStarts]).await;
        benchmark_runs.cancel(id, later()).await.unwrap();

        benchmark_runs.interrupted().await.unwrap();

        assert_eq!(
            state_of(&benchmark_runs, trial(id, 0)).await.1,
            TrialState::Ended(TrialEnd::Cancelled(Cancelled::RunStopped(TrialFigures {
                outcome: OutcomeName::Unended(Unended::Interrupted),
                money: None,
                time: Duration::ZERO,
            })))
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
