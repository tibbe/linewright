//! Experiments, kept in the database and in `experiments/` under the work
//! directory (`docs/specs/benchmarks.md`, Experiments).

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

use crate::benchmarks::{Suite, SuiteName};
use crate::case::{self, CaseName};
use crate::cost::Usd;
use crate::database::{self, Database};
use crate::domain::Timestamp;
use crate::replay::Configuration;
use crate::results::Verdict;
use crate::runs::{Ending, OutcomeName};
use crate::tree;

/// An experiment's ID: a positive number, given at its launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ExperimentId(i64);

impl std::str::FromStr for ExperimentId {
    type Err = String;

    fn from_str(id: &str) -> Result<ExperimentId, String> {
        id.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| id.parse::<i64>().ok())
            .flatten()
            .and_then(ExperimentId::positive)
            .ok_or_else(|| format!("{id:?} is not an experiment's ID: it is a positive number"))
    }
}

impl fmt::Display for ExperimentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ExperimentId {
    fn positive(id: i64) -> Option<ExperimentId> {
        (id > 0).then_some(ExperimentId(id))
    }

    fn from_row(id: i64) -> Result<ExperimentId> {
        ExperimentId::positive(id).with_context(|| format!("{id} is not an experiment's ID"))
    }
}

/// A configuration's name in an experiment: one or more lowercase letters,
/// digits and hyphens, starting with a letter or a digit.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigurationName(String);

impl ConfigurationName {
    /// The name of the configuration that holds nothing, first in every
    /// experiment.
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
        case::check_name(name, "a configuration's").map(|()| ConfigurationName(name.to_string()))
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

/// How many times an experiment replays each case under each configuration.
pub type Repetitions = NonZeroU32;

/// An experiment's state as it is kept: whether `serve` runs it or waits to
/// is not kept. An experiment that ended or was cancelled before ends were
/// kept has no end, `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExperimentState {
    Open,
    Ended { at: Option<Timestamp> },
    Cancelled { at: Option<Timestamp> },
}

impl ExperimentState {
    pub fn name(self) -> &'static str {
        match self {
            ExperimentState::Open => "open",
            ExperimentState::Ended { .. } => "ended",
            ExperimentState::Cancelled { .. } => "cancelled",
        }
    }

    /// When it ended or was cancelled, if it has and that was kept.
    pub fn ended_at(self) -> Option<Timestamp> {
        match self {
            ExperimentState::Open => None,
            ExperimentState::Ended { at } | ExperimentState::Cancelled { at } => at,
        }
    }

    /// The state a row keeps as `name` and `ended_at`.
    fn parse(name: &str, ended_at: Option<&str>) -> Result<ExperimentState> {
        let at = ended_at.map(database::parse_time).transpose()?;
        match (name, at) {
            ("open", None) => Ok(ExperimentState::Open),
            ("open", Some(_)) => anyhow::bail!("an open experiment has an end"),
            ("ended", at) => Ok(ExperimentState::Ended { at }),
            ("cancelled", at) => Ok(ExperimentState::Cancelled { at }),
            (name, _) => anyhow::bail!("{name:?} is not an experiment's state"),
        }
    }
}

/// A trial of an experiment: a replay of one case under one configuration
/// in one repetition, then its judge. Its order is the order the replays
/// start in: by repetition, then case, then configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrialKey {
    pub repetition: NonZeroU32,
    /// The case's place in the experiment's cases.
    pub case: usize,
    /// The configuration's place in the experiment's configurations.
    pub configuration: usize,
}

/// A trial of a given experiment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrialRef {
    pub experiment: ExperimentId,
    pub key: TrialKey,
}

/// Where a trial stands.
#[derive(Clone, Debug, PartialEq)]
pub enum TrialState {
    Pending,
    Replaying,
    /// Its replay ended with a run to judge, and awaits its judge.
    Replayed {
        replay: ReplayFigures,
    },
    Judging {
        replay: ReplayFigures,
    },
    Ended(TrialEnd),
}

/// How a trial ended.
#[derive(Clone, Debug, PartialEq)]
pub enum TrialEnd {
    Judged {
        replay: ReplayFigures,
        judgement: JudgementFigures,
    },
    /// Its replay ended, and has no judgement.
    Unjudged {
        replay: ReplayFigures,
        why: NoJudgement,
    },
    /// Its experiment was cancelled before its replay ended.
    Cancelled,
}

/// What the results need of a replay.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayFigures {
    pub outcome: OutcomeName,
    /// `None` when its run has no cost (`docs/specs/runs.md`, Cost).
    pub money: Option<Usd>,
    pub time: Duration,
}

impl ReplayFigures {
    /// Whether the judge judges its run: done or handed back
    /// (`docs/specs/benchmarks.md`, Experiments).
    fn judgeable(&self) -> bool {
        matches!(
            self.outcome,
            OutcomeName::Ended(Ending::Done | Ending::HandedBack)
        )
    }
}

/// What the results need of a judgement.
#[derive(Clone, Debug, PartialEq)]
pub struct JudgementFigures {
    pub correctness: Verdict,
    pub leak: bool,
    /// `None` when the judge's session has no cost.
    pub judge_money: Option<Usd>,
}

/// Why a replay that ended has no judgement.
#[derive(Clone, Debug, PartialEq)]
pub enum NoJudgement {
    /// Its run is not done or handed back.
    Outcome,
    /// Its judge made no judgement.
    Judge {
        reason: String,
        /// `None` when the judge's session has no cost.
        judge_money: Option<Usd>,
    },
    /// Its experiment was cancelled before its judge made a judgement.
    Cancelled,
}

/// An experiment, as it was launched and as far as it has gone.
#[derive(Clone, Debug, PartialEq)]
pub struct ExperimentRecord {
    pub id: ExperimentId,
    pub suite: Suite,
    pub launched_at: Timestamp,
    pub repetitions: Repetitions,
    pub state: ExperimentState,
    /// The suite's cases at the launch, in its order.
    pub cases: Vec<CaseName>,
    /// `as-is` first.
    pub configurations: Vec<ConfigurationName>,
    /// Every trial, in start order, each of a case and a configuration of
    /// the experiment's.
    pub trials: BTreeMap<TrialKey, TrialState>,
}

impl ExperimentRecord {
    /// The time from its launch to its end, or to `now` while it is open;
    /// `None` when its end was not kept.
    pub fn duration(&self, now: Timestamp) -> Option<Duration> {
        let end = if self.state == ExperimentState::Open {
            now
        } else {
            self.state.ended_at()?
        };
        Some((end - self.launched_at).to_std().unwrap_or_default())
    }
}

/// The directory, under the work directory, that holds the experiments.
pub const EXPERIMENTS: &str = "experiments";

/// The directory, in an experiment's, of what it kept at its launch: hidden,
/// as no configuration's name is.
const KEPT: &str = ".kept";

/// The directory, in [`KEPT`], of the configurations.
const CONFIGURATIONS: &str = "configurations";

/// Why an experiment was not launched.
#[derive(Debug, thiserror::Error)]
pub enum LaunchRefusal {
    #[error("there is no suite {0}")]
    NoSuchSuite(SuiteName),
    #[error("the suite {0} has no cases")]
    NoCases(SuiteName),
    #[error("two configurations are named {0}")]
    DuplicateConfiguration(ConfigurationName),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// An experiment just launched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Launched {
    pub id: ExperimentId,
    /// How many replays it makes.
    pub replays: usize,
}

/// Why an experiment was not cancelled.
#[derive(Debug, thiserror::Error)]
pub enum CancelRefusal {
    #[error("there is no experiment {0}")]
    NoSuchExperiment(ExperimentId),
    #[error("experiment {id} is {}", state.name())]
    NotOpen {
        id: ExperimentId,
        state: ExperimentState,
    },
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// A change of a trial's state.
#[derive(Clone, Debug, PartialEq)]
pub enum TrialChange {
    /// Pending, it starts its replay.
    ReplayStarts,
    /// Replaying, its replay ends: replayed when its run can be judged, and
    /// else ended without a judgement.
    ReplayEnds(ReplayFigures),
    /// Replayed, its judge starts.
    JudgeStarts,
    /// Judging, its judge makes a judgement.
    Judged(JudgementFigures),
    /// Judging, its judge makes none.
    JudgeMakesNone {
        reason: String,
        judge_money: Option<Usd>,
    },
    /// Not ended, its experiment is cancelled.
    Cancelled,
}

/// The experiments of a work directory, kept in its database and under
/// [`EXPERIMENTS`]: every handle on one database sees the same experiments.
#[derive(Clone, Debug)]
pub struct Experiments {
    db: Database,
}

const READING: &str = "reading the experiments";

impl Experiments {
    pub fn new(db: Database) -> Experiments {
        Experiments { db }
    }

    fn experiments_dir(&self) -> PathBuf {
        self.db.work_dir().join(EXPERIMENTS)
    }

    /// The directory of the experiment `id`.
    pub fn dir(&self, id: ExperimentId) -> PathBuf {
        self.experiments_dir().join(id.to_string())
    }

    /// The experiment `id`'s copy of the case `case`, as it was at the
    /// launch.
    pub fn case_dir(&self, id: ExperimentId, case: &CaseName) -> PathBuf {
        kept_case(&self.dir(id), case)
    }

    /// The experiment `id`'s copy of the configuration `name`, laid out as a
    /// repository's `.linewright/`.
    pub fn configuration_dir(&self, id: ExperimentId, name: &ConfigurationName) -> PathBuf {
        kept_configuration(&self.dir(id), name)
    }

    /// The output of the replay of `case` under `configuration` in the
    /// repetition `repetition` of the experiment `id`
    /// (`docs/specs/benchmarks.md`, Experiments, The output).
    pub fn output_dir(
        &self,
        id: ExperimentId,
        configuration: &ConfigurationName,
        case: &CaseName,
        repetition: NonZeroU32,
    ) -> PathBuf {
        self.dir(id)
            .join(configuration.as_str())
            .join(case.as_str())
            .join(repetition.to_string())
    }

    /// Launches an experiment at `at` on the cases the suite `suite` holds,
    /// under `as-is` and then `configurations`, each replayed `repetitions`
    /// times, every trial pending, with a copy of each case and
    /// configuration in its directory. It is all or nothing.
    ///
    /// # Errors
    ///
    /// When there is no such suite, it holds no cases, two configurations
    /// have one name, `as-is` among them, or a copy or a row cannot be
    /// written, such as when the suite's cases change as they are copied.
    pub async fn launch(
        &self,
        suite: &SuiteName,
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
        let launching = format!("launching an experiment on the suite {suite}");
        let mut conn = self.db.pool().acquire().await.context(launching.clone())?;
        let cases = suite_cases(&mut conn, suite).await;
        drop(conn);
        let Some(cases) = cases.context(launching.clone())? else {
            return Err(LaunchRefusal::NoSuchSuite(suite.clone()));
        };
        if cases.is_empty() {
            return Err(LaunchRefusal::NoCases(suite.clone()));
        }
        let built = self
            .build(&cases, &configurations)
            .await
            .context(launching.clone())?;
        let configuration_names: Vec<ConfigurationName> =
            std::iter::once(ConfigurationName::as_is())
                .chain(configurations.into_iter().map(|(name, _)| name))
                .collect();
        self.keep(suite, &cases, &configuration_names, repetitions, at, built)
            .await
            .map_err(|e| e.context(launching).into())
    }

    /// Builds in a new directory under [`EXPERIMENTS`] the copies of `cases`
    /// and of `as-is` and `configurations` that an experiment keeps, and
    /// gives the directory with the case directories it copied.
    async fn build(
        &self,
        cases: &[CaseName],
        configurations: &[(ConfigurationName, Configuration)],
    ) -> Result<Built> {
        let experiments_dir = self.experiments_dir();
        std::fs::create_dir_all(&experiments_dir)
            .with_context(|| format!("making {}", experiments_dir.display()))?;
        let dir = tempfile::Builder::new()
            .prefix(".launching-")
            .tempdir_in(&experiments_dir)?;
        let cases_dir = self.db.work_dir().join(case::CASES);
        let mut copied = Vec::with_capacity(cases.len());
        for case in cases {
            let from = cases_dir.join(case.as_str());
            let before = CaseDir::of(&from)?;
            tree::copy(&from, &kept_case(dir.path(), case))
                .await
                .with_context(|| format!("copying the case {case} from {}", from.display()))?;
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

    /// Keeps the experiment whose copies are `built`, in one transaction:
    /// its rows are inserted and `built` is renamed into its directory, as
    /// long as the suite's cases are still those it copied.
    async fn keep(
        &self,
        suite: &SuiteName,
        cases: &[CaseName],
        configurations: &[ConfigurationName],
        repetitions: Repetitions,
        at: Timestamp,
        built: Built,
    ) -> Result<Launched> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let now = suite_cases(&mut tx, suite).await?;
        let cases_dir = self.db.work_dir().join(case::CASES);
        let unchanged = now.as_deref() == Some(cases)
            && cases.iter().zip(&built.copied).all(|(case, copied)| {
                CaseDir::of(&cases_dir.join(case.as_str())).ok().as_ref() == Some(copied)
            });
        if !unchanged {
            anyhow::bail!("the suite's cases changed as they were copied: launch again");
        }
        let id = ExperimentId::from_row(next_id(&mut tx, &self.experiments_dir()).await?)?;
        let replays =
            insert_experiment(&mut tx, id, suite, cases, configurations, repetitions, at).await?;
        let place = self.dir(id);
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            built.dir.path(),
            rustix::fs::CWD,
            &place,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)
        .with_context(|| format!("moving the experiment into {}", place.display()))?;
        if let Err(e) = tx.commit().await {
            if let Err(back) = std::fs::rename(&place, built.dir.path()) {
                tracing::warn!(experiment = %id, error = %back, "taking back an experiment whose rows were not kept failed");
            }
            return Err(e.into());
        }
        Ok(Launched { id, replays })
    }

    /// The experiment `id`, if there is one.
    pub async fn show(&self, id: ExperimentId) -> Result<Option<ExperimentRecord>> {
        let mut conn = self.db.pool().acquire().await.context(READING)?;
        read_record(&mut conn, id).await
    }

    /// The suite `suite`'s experiments, latest first.
    pub async fn of_suite(&self, suite: &SuiteName) -> Result<Vec<ExperimentRecord>> {
        self.records_of(
            "SELECT id FROM experiments WHERE suite = ? ORDER BY id DESC",
            Some(suite.as_str()),
        )
        .await
    }

    /// Each suite's latest experiment, with when it was launched, by the
    /// suite's name: none for a suite with no experiment.
    pub async fn latest(&self) -> Result<BTreeMap<SuiteName, (ExperimentId, Timestamp)>> {
        let rows: Vec<(String, i64, String)> = sqlx::query_as(
            "SELECT suite, id, launched_at FROM experiments
             WHERE id IN (SELECT MAX(id) FROM experiments GROUP BY suite)",
        )
        .fetch_all(self.db.pool())
        .await
        .context(READING)?;
        rows.into_iter()
            .map(|(suite, id, launched_at)| {
                Ok((
                    parsed(&suite)?,
                    (
                        ExperimentId::from_row(id)?,
                        database::parse_time(&launched_at)?,
                    ),
                ))
            })
            .collect::<Result<_>>()
            .context(READING)
    }

    /// The experiments that are open, oldest first, each with its trials in
    /// start order.
    pub async fn open(&self) -> Result<Vec<ExperimentRecord>> {
        self.records_of(
            "SELECT id FROM experiments WHERE state = 'open' ORDER BY id",
            None,
        )
        .await
    }

    /// The records of the experiments whose IDs `selecting` selects, with
    /// `bound` for its one parameter if it has one, read together.
    async fn records_of(
        &self,
        selecting: &'static str,
        bound: Option<&str>,
    ) -> Result<Vec<ExperimentRecord>> {
        let mut tx = self.db.pool().begin().await.context(READING)?;
        let mut query = sqlx::query_scalar(selecting);
        if let Some(bound) = bound {
            query = query.bind(bound);
        }
        let ids: Vec<i64> = query.fetch_all(&mut *tx).await.context(READING)?;
        let mut records = Vec::with_capacity(ids.len());
        for id in ids {
            let id = ExperimentId::from_row(id).context(READING)?;
            if let Some(record) = read_record(&mut tx, id).await? {
                records.push(record);
            }
        }
        Ok(records)
    }

    /// Cancels the open experiment `id` at `at`. Its trials stand as they
    /// are.
    pub async fn cancel(&self, id: ExperimentId, at: Timestamp) -> Result<(), CancelRefusal> {
        let cancelling = format!("cancelling experiment {id}");
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .context(cancelling.clone())?;
        match read_state(&mut tx, id).await.context(cancelling.clone())? {
            None => return Err(CancelRefusal::NoSuchExperiment(id)),
            Some(ExperimentState::Open) => {}
            Some(state) => return Err(CancelRefusal::NotOpen { id, state }),
        }
        sqlx::query("UPDATE experiments SET state = 'cancelled', ended_at = ? WHERE id = ?")
            .bind(database::time_text(at))
            .bind(id.0)
            .execute(&mut *tx)
            .await
            .context(cancelling.clone())?;
        tx.commit().await.context(cancelling)?;
        Ok(())
    }

    /// The state the experiment `id` is kept in, if there is one.
    pub async fn state(&self, id: ExperimentId) -> Result<Option<ExperimentState>> {
        let mut conn = self.db.pool().acquire().await.context(READING)?;
        read_state(&mut conn, id).await.context(READING)
    }

    /// Turns each trial a stop left replaying back into pending, and each
    /// left judging back into replayed, so that they start again. Of an
    /// experiment no longer open, such as one cancelled as it stopped, they
    /// end as cancelled instead.
    pub async fn interrupted(&self) -> Result<()> {
        let interrupting = async {
            let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
            sqlx::query(
                "UPDATE trials SET state = CASE state
                     WHEN 'replaying' THEN 'pending'
                     WHEN 'judging' THEN 'replayed'
                 END
                 WHERE state IN ('replaying', 'judging')
                     AND experiment IN (SELECT id FROM experiments WHERE state = 'open')",
            )
            .execute(&mut *tx)
            .await?;
            let left = sqlx::query(
                "SELECT trials.* FROM trials JOIN experiments ON experiments.id = trials.experiment
                 WHERE trials.state IN ('replaying', 'judging') AND experiments.state != 'open'",
            )
            .fetch_all(&mut *tx)
            .await?;
            for row in &left {
                let experiment = ExperimentId::from_row(row.try_get("experiment")?)?;
                let (key, state) = decode(row)?;
                write_trial(
                    &mut tx,
                    experiment,
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

    /// Records that the experiment `id` ended at `at`, if it is open and
    /// none of its trials is left to end.
    pub async fn end(&self, id: ExperimentId, at: Timestamp) -> Result<()> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        end_if_done(&mut tx, id, at).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Records `change` of the trial `trial` at `at`, and that its
    /// experiment ended then once it is open and none of its trials is left
    /// to end.
    ///
    /// # Errors
    ///
    /// When there is no such trial, `change` is not one of its state, or the
    /// rows cannot be written.
    pub async fn step(&self, trial: &TrialRef, change: TrialChange, at: Timestamp) -> Result<()> {
        let TrialRef { experiment, key } = trial;
        let stepping = format!(
            "recording a change of the trial {}/{}/{} of experiment {experiment}",
            key.repetition, key.case, key.configuration
        );
        let stepped = async {
            let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
            let row = sqlx::query(
                "SELECT * FROM trials
                 WHERE experiment = ? AND repetition = ? AND case_seq = ? AND configuration_seq = ?",
            )
            .bind(experiment.0)
            .bind(key.repetition.get())
            .bind(seq(key.case))
            .bind(seq(key.configuration))
            .fetch_optional(&mut *tx)
            .await?
            .context("there is no such trial")?;
            let (_, state) = decode(&row)?;
            write_trial(&mut tx, *experiment, key, &state.changed(change)?).await?;
            end_if_done(&mut tx, *experiment, at).await?;
            tx.commit().await?;
            anyhow::Ok(())
        };
        stepped.await.context(stepping)
    }
}

/// Writes `state` as the state of the trial `key` of `experiment`.
async fn write_trial(
    conn: &mut SqliteConnection,
    experiment: ExperimentId,
    key: &TrialKey,
    state: &TrialState,
) -> Result<()> {
    let columns = Columns::of(state);
    sqlx::query(
        "UPDATE trials SET state = ?, outcome = ?, money = ?, seconds = ?,
             correctness = ?, leak = ?, judge_money = ?, unjudged = ?, reason = ?
         WHERE experiment = ? AND repetition = ? AND case_seq = ? AND configuration_seq = ?",
    )
    .bind(columns.state)
    .bind(columns.outcome)
    .bind(columns.money)
    .bind(columns.seconds)
    .bind(columns.correctness)
    .bind(columns.leak)
    .bind(columns.judge_money)
    .bind(columns.unjudged)
    .bind(columns.reason)
    .bind(experiment.0)
    .bind(key.repetition.get())
    .bind(seq(key.case))
    .bind(seq(key.configuration))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// The state of the experiment `id`, if there is one.
async fn read_state(
    conn: &mut SqliteConnection,
    id: ExperimentId,
) -> Result<Option<ExperimentState>> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT state, ended_at FROM experiments WHERE id = ?")
            .bind(id.0)
            .fetch_optional(&mut *conn)
            .await?;
    row.map(|(state, ended_at)| ExperimentState::parse(&state, ended_at.as_deref()))
        .transpose()
}

/// Records that the experiment `id` ended at `at`, if it is open and none of
/// its trials is left to end.
async fn end_if_done(conn: &mut SqliteConnection, id: ExperimentId, at: Timestamp) -> Result<()> {
    sqlx::query(
        "UPDATE experiments SET state = 'ended', ended_at = ?
         WHERE id = ? AND state = 'open' AND NOT EXISTS (
             SELECT 1 FROM trials
             WHERE experiment = ? AND state NOT IN ('judged', 'unjudged'))",
    )
    .bind(database::time_text(at))
    .bind(id.0)
    .bind(id.0)
    .execute(&mut *conn)
    .await
    .with_context(|| format!("recording that experiment {id} ended"))?;
    Ok(())
}

impl TrialState {
    /// The state `change` leads to from this one.
    fn changed(self, change: TrialChange) -> Result<TrialState> {
        Ok(match (self, change) {
            (TrialState::Pending, TrialChange::ReplayStarts) => TrialState::Replaying,
            (TrialState::Replaying, TrialChange::ReplayEnds(replay)) if replay.judgeable() => {
                TrialState::Replayed { replay }
            }
            (TrialState::Replaying, TrialChange::ReplayEnds(replay)) => {
                TrialState::Ended(TrialEnd::Unjudged {
                    replay,
                    why: NoJudgement::Outcome,
                })
            }
            (TrialState::Replayed { replay }, TrialChange::JudgeStarts) => {
                TrialState::Judging { replay }
            }
            (TrialState::Judging { replay }, TrialChange::Judged(judgement)) => {
                TrialState::Ended(TrialEnd::Judged { replay, judgement })
            }
            (
                TrialState::Judging { replay },
                TrialChange::JudgeMakesNone {
                    reason,
                    judge_money,
                },
            ) => TrialState::Ended(TrialEnd::Unjudged {
                replay,
                why: NoJudgement::Judge {
                    reason,
                    judge_money,
                },
            }),
            (TrialState::Pending | TrialState::Replaying, TrialChange::Cancelled) => {
                TrialState::Ended(TrialEnd::Cancelled)
            }
            (
                TrialState::Replayed { replay } | TrialState::Judging { replay },
                TrialChange::Cancelled,
            ) => TrialState::Ended(TrialEnd::Unjudged {
                replay,
                why: NoJudgement::Cancelled,
            }),
            (state, change) => anyhow::bail!(
                "a trial {} cannot change by {change:?}",
                Columns::of(&state).state
            ),
        })
    }
}

/// The copies an experiment keeps, built in a directory of their own.
struct Built {
    dir: tempfile::TempDir,
    /// The directory of each case copied, in the order of the cases.
    copied: Vec<CaseDir>,
}

/// Which directory a case's is: another one in its place, such as of a
/// case removed and captured again, is not the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaseDir {
    dev: u64,
    ino: u64,
}

impl CaseDir {
    fn of(dir: &Path) -> Result<CaseDir> {
        let metadata =
            std::fs::symlink_metadata(dir).with_context(|| format!("reading {}", dir.display()))?;
        Ok(CaseDir {
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }
}

/// The copy of the case `case` in the experiment's directory `dir`.
fn kept_case(dir: &Path, case: &CaseName) -> PathBuf {
    dir.join(KEPT).join(case::CASES).join(case.as_str())
}

/// The copy of the configuration `name` in the experiment's directory `dir`.
fn kept_configuration(dir: &Path, name: &ConfigurationName) -> PathBuf {
    dir.join(KEPT).join(CONFIGURATIONS).join(name.as_str())
}

/// The cases of the suite `suite`, in its order, or `None` when there is no
/// such suite.
async fn suite_cases(
    conn: &mut SqliteConnection,
    suite: &SuiteName,
) -> Result<Option<Vec<CaseName>>> {
    let found: Option<String> = sqlx::query_scalar("SELECT name FROM suites WHERE name = ?")
        .bind(suite.as_str())
        .fetch_optional(&mut *conn)
        .await
        .context(READING)?;
    if found.is_none() {
        return Ok(None);
    }
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM cases WHERE suite = ? ORDER BY seq")
            .bind(suite.as_str())
            .fetch_all(&mut *conn)
            .await
            .context(READING)?;
    names
        .iter()
        .map(|name| parsed(name))
        .collect::<Result<_>>()
        .map(Some)
}

/// The ID of the next experiment: past every experiment's, and past every
/// directory in `experiments_dir` named as one, such as one a launch left
/// there as it stopped.
async fn next_id(conn: &mut SqliteConnection, experiments_dir: &Path) -> Result<i64> {
    let kept: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM experiments")
        .fetch_one(&mut *conn)
        .await?;
    let mut last = kept;
    for entry in std::fs::read_dir(experiments_dir)
        .with_context(|| format!("reading {}", experiments_dir.display()))?
    {
        let name = entry?.file_name();
        if let Some(id) = name
            .to_str()
            .and_then(|name| name.parse::<ExperimentId>().ok())
        {
            last = last.max(id.0);
        }
    }
    last.checked_add(1)
        .context("there is no ID left for an experiment")
}

/// Inserts the rows of the experiment `id` on `cases` of the suite `suite`
/// under `configurations`, launched at `at`, with every trial pending, and
/// gives how many replays it makes.
async fn insert_experiment(
    conn: &mut SqliteConnection,
    id: ExperimentId,
    suite: &SuiteName,
    cases: &[CaseName],
    configurations: &[ConfigurationName],
    repetitions: Repetitions,
    at: Timestamp,
) -> Result<usize> {
    sqlx::query(
        "INSERT INTO experiments (id, suite, launched_at, repetitions, state)
         VALUES (?, ?, ?, ?, 'open')",
    )
    .bind(id.0)
    .bind(suite.as_str())
    .bind(database::time_text(at))
    .bind(repetitions.get())
    .execute(&mut *conn)
    .await?;
    for (i, case) in cases.iter().enumerate() {
        sqlx::query("INSERT INTO experiment_cases (experiment, seq, name) VALUES (?, ?, ?)")
            .bind(id.0)
            .bind(seq(i))
            .bind(case.as_str())
            .execute(&mut *conn)
            .await?;
    }
    for (i, configuration) in configurations.iter().enumerate() {
        sqlx::query(
            "INSERT INTO experiment_configurations (experiment, seq, name) VALUES (?, ?, ?)",
        )
        .bind(id.0)
        .bind(seq(i))
        .bind(configuration.as_str())
        .execute(&mut *conn)
        .await?;
    }
    let mut replays = 0;
    for repetition in 1..=repetitions.get() {
        for case in 0..cases.len() {
            for configuration in 0..configurations.len() {
                sqlx::query(
                    "INSERT INTO trials (experiment, repetition, case_seq, configuration_seq, state)
                     VALUES (?, ?, ?, ?, 'pending')",
                )
                .bind(id.0)
                .bind(repetition)
                .bind(seq(case))
                .bind(seq(configuration))
                .execute(&mut *conn)
                .await?;
                replays += 1;
            }
        }
    }
    Ok(replays)
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

/// The experiment `id` as its rows hold it, if there is one.
async fn read_record(
    conn: &mut SqliteConnection,
    id: ExperimentId,
) -> Result<Option<ExperimentRecord>> {
    let row: Option<(String, String, String, i64, String, Option<String>)> = sqlx::query_as(
        "SELECT experiments.suite, suites.stage, experiments.launched_at,
                experiments.repetitions, experiments.state, experiments.ended_at
         FROM experiments JOIN suites ON suites.name = experiments.suite
         WHERE experiments.id = ?",
    )
    .bind(id.0)
    .fetch_optional(&mut *conn)
    .await
    .context(READING)?;
    let Some((suite, stage, launched_at, repetitions, state, ended_at)) = row else {
        return Ok(None);
    };
    let reading = || format!("reading experiment {id}");
    let cases: Vec<String> =
        sqlx::query_scalar("SELECT name FROM experiment_cases WHERE experiment = ? ORDER BY seq")
            .bind(id.0)
            .fetch_all(&mut *conn)
            .await
            .with_context(reading)?;
    let configurations: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM experiment_configurations WHERE experiment = ? ORDER BY seq",
    )
    .bind(id.0)
    .fetch_all(&mut *conn)
    .await
    .with_context(reading)?;
    let trials = sqlx::query("SELECT * FROM trials WHERE experiment = ?")
        .bind(id.0)
        .fetch_all(&mut *conn)
        .await
        .with_context(reading)?;
    let record = (|| {
        let cases: Vec<CaseName> = cases
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
            .find(|key| key.case >= cases.len() || key.configuration >= configurations.len())
        {
            anyhow::bail!("a trial is of a case or a configuration it lacks: {key:?}");
        }
        Ok::<_, anyhow::Error>(ExperimentRecord {
            id,
            suite: Suite {
                name: parsed(&suite)?,
                stage: parsed(&stage)?,
            },
            launched_at: database::parse_time(&launched_at)?,
            repetitions: u32::try_from(repetitions)
                .ok()
                .and_then(Repetitions::new)
                .with_context(|| format!("{repetitions} is not a number of repetitions"))?,
            state: ExperimentState::parse(&state, ended_at.as_deref())?,
            cases,
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
    judge_money: Option<f64>,
    unjudged: Option<&'static str>,
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
        case: place(row.try_get("case_seq")?)?,
        configuration: place(row.try_get("configuration_seq")?)?,
    };
    let state: String = row.try_get("state")?;
    let outcome: Option<String> = row.try_get("outcome")?;
    let seconds: Option<f64> = row.try_get("seconds")?;
    let replay = match (outcome, seconds) {
        (Some(outcome), Some(seconds)) => Some(ReplayFigures {
            outcome: serde_json::from_value(serde_json::Value::String(outcome))?,
            money: money(row.try_get("money")?)?,
            time: Duration::try_from_secs_f64(seconds)?,
        }),
        _ => None,
    };
    let with_replay = |figures: Option<ReplayFigures>| {
        figures.with_context(|| format!("a trial {state} has no replay"))
    };
    let judge_money = money(row.try_get("judge_money")?)?;
    let unjudged: Option<String> = row.try_get("unjudged")?;
    let reason: Option<String> = row.try_get("reason")?;
    let trial = match state.as_str() {
        "pending" => TrialState::Pending,
        "replaying" => TrialState::Replaying,
        "replayed" => TrialState::Replayed {
            replay: with_replay(replay)?,
        },
        "judging" => TrialState::Judging {
            replay: with_replay(replay)?,
        },
        "judged" => {
            let correctness: Option<String> = row.try_get("correctness")?;
            TrialState::Ended(TrialEnd::Judged {
                replay: with_replay(replay)?,
                judgement: JudgementFigures {
                    correctness: [Verdict::Pass, Verdict::Fail]
                        .into_iter()
                        .find(|verdict| Some(verdict.name()) == correctness.as_deref())
                        .with_context(|| format!("{correctness:?} is not a correctness"))?,
                    leak: row
                        .try_get::<Option<bool>, _>("leak")?
                        .context("a judged trial has no leak")?,
                    judge_money,
                },
            })
        }
        "unjudged" => match (unjudged.as_deref(), reason, replay) {
            (Some("cancelled"), None, None) => TrialState::Ended(TrialEnd::Cancelled),
            (Some(why), reason, Some(replay)) => TrialState::Ended(TrialEnd::Unjudged {
                replay,
                why: match (why, reason) {
                    ("outcome", None) => NoJudgement::Outcome,
                    ("judge", Some(reason)) => NoJudgement::Judge {
                        reason,
                        judge_money,
                    },
                    ("cancelled", None) => NoJudgement::Cancelled,
                    _ => anyhow::bail!("{why:?} is not why a trial has no judgement"),
                },
            }),
            (why, ..) => anyhow::bail!("{why:?} is not why a trial has no judgement"),
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
            judge_money: None,
            unjudged: None,
            reason: None,
        };
        let (name, replay) = match state {
            TrialState::Pending => ("pending", None),
            TrialState::Replaying => ("replaying", None),
            TrialState::Replayed { replay } => ("replayed", Some(replay)),
            TrialState::Judging { replay } => ("judging", Some(replay)),
            TrialState::Ended(TrialEnd::Judged { replay, judgement }) => {
                columns.correctness = Some(judgement.correctness.name());
                columns.leak = Some(judgement.leak);
                columns.judge_money = judgement.judge_money.map(f64::from);
                ("judged", Some(replay))
            }
            TrialState::Ended(TrialEnd::Unjudged { replay, why }) => {
                columns.unjudged = Some(match why {
                    NoJudgement::Outcome => "outcome",
                    NoJudgement::Judge {
                        reason,
                        judge_money,
                    } => {
                        columns.reason = Some(reason.clone());
                        columns.judge_money = judge_money.map(f64::from);
                        "judge"
                    }
                    NoJudgement::Cancelled => "cancelled",
                });
                ("unjudged", Some(replay))
            }
            TrialState::Ended(TrialEnd::Cancelled) => {
                columns.unjudged = Some("cancelled");
                ("unjudged", None)
            }
        };
        columns.state = name;
        if let Some(replay) = replay {
            columns.outcome = Some(outcome_name(replay.outcome));
            columns.money = replay.money.map(f64::from);
            columns.seconds = Some(replay.time.as_secs_f64());
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

    /// The experiments of a database in `work_dir` with the design suite
    /// `board` of the cases `columns` and `lanes`, and an experiment
    /// launched on it with one repetition under `as-is` alone.
    #[track_caller]
    fn launched(work_dir: &Path) -> impl Future<Output = (Experiments, ExperimentId)> + '_ {
        let caller = Location::caller();
        async move {
            let db = Database::open(work_dir)
                .await
                .unwrap_or_else(|e| panic!("opening the database at {caller}: {e:#}"));
            sqlx::query("INSERT INTO suites (name, stage) VALUES ('board', 'design')")
                .execute(db.pool())
                .await
                .unwrap_or_else(|e| panic!("keeping the suite at {caller}: {e}"));
            for (seq, case, run) in [(1, "columns", "3/design-1"), (2, "lanes", "4/design-1")] {
                std::fs::create_dir_all(work_dir.join("cases").join(case))
                    .unwrap_or_else(|e| panic!("making the case at {caller}: {e}"));
                sqlx::query("INSERT INTO cases (name, suite, run, seq) VALUES (?, 'board', ?, ?)")
                    .bind(case)
                    .bind(run)
                    .bind(seq)
                    .execute(db.pool())
                    .await
                    .unwrap_or_else(|e| panic!("keeping the case at {caller}: {e}"));
            }
            let experiments = Experiments::new(db);
            let launched = launch(&experiments)
                .await
                .unwrap_or_else(|e| panic!("launching at {caller}: {e:#}"));
            (experiments, launched.id)
        }
    }

    async fn launch(experiments: &Experiments) -> Result<Launched, LaunchRefusal> {
        experiments
            .launch(
                &"board".parse().expect("a suite's name"),
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

    fn trial(experiment: ExperimentId, case: usize) -> TrialRef {
        TrialRef {
            experiment,
            key: TrialKey {
                repetition: NonZeroU32::MIN,
                case,
                configuration: 0,
            },
        }
    }

    fn replay(ending: Ending) -> ReplayFigures {
        ReplayFigures {
            outcome: OutcomeName::Ended(ending),
            money: Usd::new(0.25),
            time: Duration::from_millis(12_500),
        }
    }

    /// Records `changes` of `trial` in turn.
    #[track_caller]
    fn steps(
        experiments: &Experiments,
        trial: TrialRef,
        changes: Vec<TrialChange>,
    ) -> impl Future<Output = ()> + '_ {
        let caller = Location::caller();
        async move {
            for change in changes {
                experiments
                    .step(&trial, change, later())
                    .await
                    .unwrap_or_else(|e| panic!("stepping at {caller}: {e:#}"));
            }
        }
    }

    #[track_caller]
    fn state_of(
        experiments: &Experiments,
        trial: TrialRef,
    ) -> impl Future<Output = (ExperimentState, TrialState)> + '_ {
        let caller = Location::caller();
        async move {
            let record = experiments
                .show(trial.experiment)
                .await
                .unwrap_or_else(|e| panic!("reading the experiment at {caller}: {e:#}"))
                .unwrap_or_else(|| panic!("no experiment at {caller}"));
            (record.state, record.trials[&trial.key].clone())
        }
    }

    #[test_log::test(tokio::test)]
    async fn a_trial_steps_through_its_replay_and_its_judge_and_its_experiment_ends_with_its_last()
    {
        let work_dir = tempfile::tempdir().unwrap();
        let (experiments, id) = launched(work_dir.path()).await;
        let judgement = JudgementFigures {
            correctness: Verdict::Pass,
            leak: true,
            judge_money: None,
        };

        let mut seen = vec![];
        for change in [
            TrialChange::ReplayStarts,
            TrialChange::ReplayEnds(replay(Ending::HandedBack)),
            TrialChange::JudgeStarts,
            TrialChange::Judged(judgement.clone()),
        ] {
            steps(&experiments, trial(id, 0), vec![change]).await;
            seen.push(state_of(&experiments, trial(id, 0)).await);
        }
        steps(
            &experiments,
            trial(id, 1),
            vec![
                TrialChange::ReplayStarts,
                TrialChange::ReplayEnds(replay(Ending::Done)),
                TrialChange::JudgeStarts,
                TrialChange::JudgeMakesNone {
                    reason: "it gave up".into(),
                    judge_money: Usd::new(0.5),
                },
            ],
        )
        .await;

        let open = ExperimentState::Open;
        let handed_back = replay(Ending::HandedBack);
        assert_eq!(
            seen,
            [
                (open, TrialState::Replaying),
                (
                    open,
                    TrialState::Replayed {
                        replay: handed_back.clone()
                    }
                ),
                (
                    open,
                    TrialState::Judging {
                        replay: handed_back.clone()
                    }
                ),
                (
                    open,
                    TrialState::Ended(TrialEnd::Judged {
                        replay: handed_back,
                        judgement,
                    })
                ),
            ]
        );
        assert_eq!(
            state_of(&experiments, trial(id, 1)).await,
            (
                ExperimentState::Ended { at: Some(later()) },
                TrialState::Ended(TrialEnd::Unjudged {
                    replay: replay(Ending::Done),
                    why: NoJudgement::Judge {
                        reason: "it gave up".into(),
                        judge_money: Usd::new(0.5),
                    },
                })
            )
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_replay_whose_run_is_not_done_or_handed_back_ends_its_trial_without_a_judgement() {
        let work_dir = tempfile::tempdir().unwrap();
        let (experiments, id) = launched(work_dir.path()).await;

        steps(
            &experiments,
            trial(id, 0),
            vec![
                TrialChange::ReplayStarts,
                TrialChange::ReplayEnds(replay(Ending::Failed)),
            ],
        )
        .await;

        assert_eq!(
            state_of(&experiments, trial(id, 0)).await.1,
            TrialState::Ended(TrialEnd::Unjudged {
                replay: replay(Ending::Failed),
                why: NoJudgement::Outcome,
            })
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_cancelled_trial_keeps_the_replay_it_had_and_has_none_before_its_replay_ended() {
        let work_dir = tempfile::tempdir().unwrap();
        let (experiments, id) = launched(work_dir.path()).await;

        steps(
            &experiments,
            trial(id, 0),
            vec![
                TrialChange::ReplayStarts,
                TrialChange::ReplayEnds(replay(Ending::Done)),
                TrialChange::Cancelled,
            ],
        )
        .await;
        steps(
            &experiments,
            trial(id, 1),
            vec![TrialChange::ReplayStarts, TrialChange::Cancelled],
        )
        .await;

        assert_eq!(
            state_of(&experiments, trial(id, 0)).await.1,
            TrialState::Ended(TrialEnd::Unjudged {
                replay: replay(Ending::Done),
                why: NoJudgement::Cancelled,
            })
        );
        assert_eq!(
            state_of(&experiments, trial(id, 1)).await.1,
            TrialState::Ended(TrialEnd::Cancelled)
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_change_that_is_not_of_a_trials_state_is_refused() {
        let work_dir = tempfile::tempdir().unwrap();
        let (experiments, id) = launched(work_dir.path()).await;

        let judged = experiments
            .step(&trial(id, 0), TrialChange::JudgeStarts, later())
            .await;
        let missing = experiments
            .step(&trial(id, 2), TrialChange::ReplayStarts, later())
            .await;

        assert!(
            format!("{:#}", judged.unwrap_err()).contains("a trial pending cannot change"),
            "refused for another reason"
        );
        assert!(
            format!("{:#}", missing.unwrap_err()).contains("there is no such trial"),
            "refused for another reason"
        );
        assert_eq!(
            state_of(&experiments, trial(id, 0)).await,
            (ExperimentState::Open, TrialState::Pending)
        );
    }

    #[test_log::test(tokio::test)]
    async fn the_trials_a_stop_interrupted_start_again() {
        let work_dir = tempfile::tempdir().unwrap();
        let (experiments, id) = launched(work_dir.path()).await;
        steps(&experiments, trial(id, 0), vec![TrialChange::ReplayStarts]).await;
        steps(
            &experiments,
            trial(id, 1),
            vec![
                TrialChange::ReplayStarts,
                TrialChange::ReplayEnds(replay(Ending::Done)),
                TrialChange::JudgeStarts,
            ],
        )
        .await;

        experiments.interrupted().await.unwrap();

        assert_eq!(
            state_of(&experiments, trial(id, 0)).await.1,
            TrialState::Pending
        );
        assert_eq!(
            state_of(&experiments, trial(id, 1)).await.1,
            TrialState::Replayed {
                replay: replay(Ending::Done)
            }
        );
    }

    #[test_log::test(tokio::test)]
    async fn the_trials_a_stop_interrupted_of_a_cancelled_experiment_end_cancelled() {
        let work_dir = tempfile::tempdir().unwrap();
        let (experiments, id) = launched(work_dir.path()).await;
        steps(&experiments, trial(id, 0), vec![TrialChange::ReplayStarts]).await;
        steps(
            &experiments,
            trial(id, 1),
            vec![
                TrialChange::ReplayStarts,
                TrialChange::ReplayEnds(replay(Ending::Done)),
                TrialChange::JudgeStarts,
            ],
        )
        .await;
        experiments.cancel(id, later()).await.unwrap();

        experiments.interrupted().await.unwrap();

        assert_eq!(
            state_of(&experiments, trial(id, 0)).await,
            (
                ExperimentState::Cancelled { at: Some(later()) },
                TrialState::Ended(TrialEnd::Cancelled)
            )
        );
        assert_eq!(
            state_of(&experiments, trial(id, 1)).await.1,
            TrialState::Ended(TrialEnd::Unjudged {
                replay: replay(Ending::Done),
                why: NoJudgement::Cancelled
            })
        );
    }

    #[test_log::test(tokio::test)]
    async fn the_open_experiments_are_those_not_ended_or_cancelled_oldest_first() {
        let work_dir = tempfile::tempdir().unwrap();
        let (experiments, first) = launched(work_dir.path()).await;
        let cancelled = launch(&experiments).await.unwrap().id;
        let last = launch(&experiments).await.unwrap().id;
        experiments.cancel(cancelled, later()).await.unwrap();

        let open: Vec<ExperimentId> = experiments
            .open()
            .await
            .unwrap()
            .into_iter()
            .map(|record| record.id)
            .collect();

        assert_eq!(open, [first, last]);
    }

    #[test_log::test(tokio::test)]
    async fn an_open_experiment_is_not_kept_with_an_end() {
        let work_dir = tempfile::tempdir().unwrap();
        let (experiments, id) = launched(work_dir.path()).await;

        let ended = sqlx::query("UPDATE experiments SET ended_at = ? WHERE id = ?")
            .bind(database::time_text(later()))
            .bind(id.0)
            .execute(experiments.db.pool())
            .await;

        assert!(ended.is_err(), "an open experiment was kept with an end");
        assert_eq!(
            state_of(&experiments, trial(id, 0)).await.0,
            ExperimentState::Open
        );
    }

    #[test_log::test(tokio::test)]
    async fn an_experiments_duration_runs_from_its_launch_to_its_end_or_to_now_while_it_is_open() {
        let work_dir = tempfile::tempdir().unwrap();
        let (experiments, id) = launched(work_dir.path()).await;
        let mut record = experiments.show(id).await.unwrap().unwrap();
        let now: Timestamp = "2026-11-17T12:30:00Z".parse().unwrap();

        let open = record.duration(now);
        record.state = ExperimentState::Ended { at: Some(later()) };
        let ended = record.duration(now);
        record.state = ExperimentState::Cancelled { at: None };
        let unrecorded = record.duration(now);

        assert_eq!(open, Some(Duration::from_mins(30)));
        assert_eq!(ended, Some(Duration::from_hours(1)));
        assert_eq!(unrecorded, None);
    }
}
