//! Suites, the cases they hold and the experiments on them, kept in the
//! database (`docs/specs/benchmarks.md`, "Suites" and "Experiments"): every
//! write to them, each in one transaction.

mod answers;
mod experiments;
pub mod results;

pub use answers::{
    ExperimentAnswer, ExperimentSummary, Shown, WaitingFor, dollars, duration, mean, percent,
    render_experiment, without,
};
pub use experiments::{
    CancelRefusal, ConfigurationName, EXPERIMENTS, ExperimentId, ExperimentRecord, ExperimentState,
    Experiments, JudgementFigures, LaunchRefusal, NoJudgement, Repetitions, ReplayFigures,
    TrialChange, TrialEnd, TrialKey, TrialRef, TrialState,
};

use std::fmt;
use std::fmt::Write as _;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Serialize;

use crate::case::{self, CaptureError, CaseName, CaseRefusal, Criteria, ReplayStage};
use crate::database::Database;
use crate::runs::RunId;

/// A suite's name: one or more lowercase letters, digits and hyphens,
/// starting with a letter or a digit.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SuiteName(String);

impl SuiteName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for SuiteName {
    type Err = String;

    fn from_str(name: &str) -> Result<SuiteName, String> {
        case::check_name(name, "a suite's").map(|()| SuiteName(name.to_string()))
    }
}

impl fmt::Display for SuiteName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for SuiteName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// A named set of cases of one stage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Suite {
    pub name: SuiteName,
    pub stage: ReplayStage,
}

/// The suite a capture puts its case into, resolved: one there is, or one
/// to make with the case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChosenSuite {
    Existing(Suite),
    New(Suite),
}

impl ChosenSuite {
    pub fn suite(&self) -> &Suite {
        match self {
            ChosenSuite::Existing(suite) | ChosenSuite::New(suite) => suite,
        }
    }
}

/// A suite, with how many cases it holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SuiteListed {
    #[serde(flatten)]
    pub suite: Suite,
    pub cases: usize,
}

/// A suite, with its cases by name and its experiments, latest first.
#[derive(Clone, Debug, PartialEq)]
pub struct SuiteShown {
    pub suite: Suite,
    pub cases: Vec<CaseEntry>,
    pub experiments: Vec<ExperimentRecord>,
}

/// A case of a suite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseEntry {
    pub name: CaseName,
    /// The run it was captured from.
    pub run: RunId,
    /// Its criteria, or why they do not read, such as once a person
    /// emptied them.
    pub criteria: Result<Criteria, CaseRefusal>,
}

/// Why a suite was not made.
#[derive(Debug, thiserror::Error)]
pub enum MakeError {
    #[error("a suite is already named {0}")]
    Taken(SuiteName),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// Why a case was not removed.
#[derive(Debug, thiserror::Error)]
pub enum RemoveError {
    #[error("there is no suite {0}")]
    NoSuchSuite(SuiteName),
    #[error("the suite {suite} has no case {case}")]
    NoSuchCase { suite: SuiteName, case: CaseName },
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// The suites of a work directory, kept in its database: every handle on
/// one database sees the same suites.
#[derive(Clone, Debug)]
pub struct Suites {
    db: Database,
}

impl Suites {
    pub fn new(db: Database) -> Suites {
        Suites { db }
    }

    /// The directory that holds the cases, under the work directory.
    pub fn cases_dir(&self) -> PathBuf {
        self.db.work_dir().join(case::CASES)
    }

    /// Makes the suite `name` of `stage`, refused when a suite has the name.
    pub async fn make(&self, name: &SuiteName, stage: ReplayStage) -> Result<Suite, MakeError> {
        let suite = Suite {
            name: name.clone(),
            stage,
        };
        insert_suite(self.db.pool(), &suite).await?;
        Ok(suite)
    }

    /// Whether a case has the name `name`: a case of a suite, or a
    /// directory in the cases, such as of a case captured into no suite.
    pub async fn has_case(&self, name: &CaseName) -> Result<bool> {
        let row: Option<String> = sqlx::query_scalar("SELECT name FROM cases WHERE name = ?")
            .bind(name.as_str())
            .fetch_optional(self.db.pool())
            .await
            .context(READING)?;
        Ok(row.is_some()
            || self
                .cases_dir()
                .join(name.as_str())
                .symlink_metadata()
                .is_ok())
    }

    /// Every suite, by name, with how many cases it holds.
    pub async fn list(&self) -> Result<Vec<SuiteListed>> {
        let rows: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT suites.name, suites.stage, COUNT(cases.name) FROM suites
             LEFT JOIN cases ON cases.suite = suites.name
             GROUP BY suites.name ORDER BY suites.name",
        )
        .fetch_all(self.db.pool())
        .await
        .context(READING)?;
        rows.into_iter()
            .map(|(name, stage, cases)| {
                Ok(SuiteListed {
                    suite: suite_of(&name, &stage)?,
                    cases: usize::try_from(cases).context(READING)?,
                })
            })
            .collect()
    }

    /// The suite `name`, if there is one.
    pub async fn find(&self, name: &SuiteName) -> Result<Option<Suite>> {
        let stage: Option<String> = sqlx::query_scalar("SELECT stage FROM suites WHERE name = ?")
            .bind(name.as_str())
            .fetch_optional(self.db.pool())
            .await
            .context(READING)?;
        stage
            .map(|stage| suite_of(name.as_str(), &stage))
            .transpose()
    }

    /// The suite `name` with its cases, by name, each with its criteria as
    /// its directory holds them now, and its experiments, latest first;
    /// `None` when there is no such suite.
    pub async fn show(&self, name: &SuiteName) -> Result<Option<SuiteShown>> {
        let Some(suite) = self.find(name).await? else {
            return Ok(None);
        };
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT name, run FROM cases WHERE suite = ? ORDER BY name")
                .bind(name.as_str())
                .fetch_all(self.db.pool())
                .await
                .context(READING)?;
        let cases_dir = self.cases_dir();
        let cases = rows
            .into_iter()
            .map(|(case, run)| {
                let case: CaseName = case.parse().map_err(anyhow::Error::msg)?;
                let criteria = case::read_criteria(&cases_dir.join(case.as_str()));
                Ok(CaseEntry {
                    name: case,
                    run: run.parse().map_err(anyhow::Error::msg)?,
                    criteria,
                })
            })
            .collect::<Result<_>>()
            .context(READING)?;
        let experiments = Experiments::new(self.db.clone()).of_suite(name).await?;
        Ok(Some(SuiteShown {
            suite,
            cases,
            experiments,
        }))
    }

    /// Removes the case `case` from the suite `suite`, and with it the
    /// case: its row and its directory go together, or neither does.
    pub async fn remove_case(&self, suite: &SuiteName, case: &CaseName) -> Result<(), RemoveError> {
        let removing = format!("removing the case {case} from the suite {suite}");
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .context(removing.clone())?;
        let found: Option<String> = sqlx::query_scalar("SELECT name FROM suites WHERE name = ?")
            .bind(suite.as_str())
            .fetch_optional(&mut *tx)
            .await
            .context(removing.clone())?;
        if found.is_none() {
            return Err(RemoveError::NoSuchSuite(suite.clone()));
        }
        let deleted = sqlx::query("DELETE FROM cases WHERE name = ? AND suite = ?")
            .bind(case.as_str())
            .bind(suite.as_str())
            .execute(&mut *tx)
            .await
            .context(removing.clone())?;
        if deleted.rows_affected() == 0 {
            return Err(RemoveError::NoSuchCase {
                suite: suite.clone(),
                case: case.clone(),
            });
        }
        let cases_dir = self.cases_dir();
        let place = cases_dir.join(case.as_str());
        std::fs::create_dir_all(&cases_dir)
            .context("creating the cases' directory")
            .context(removing.clone())?;
        let bin = tempfile::Builder::new()
            .prefix(".removing-")
            .tempdir_in(&cases_dir)
            .context(removing.clone())?;
        let binned = bin.path().join(case.as_str());
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
                tracing::warn!(case = %case, error = %back, "putting back a case whose removal failed failed");
                let _kept = bin.keep();
            }
            return Err(anyhow::Error::from(e).context(removing).into());
        }
        Ok(())
    }

    /// Keeps the case `name`, captured from `run` and built in the
    /// directory `built`, in `chosen`, made first when it is new: in one
    /// transaction, the rows are inserted and `built` is renamed into its
    /// place in the cases, so that all are written or none is. Refused as
    /// [`CaptureError::Taken`] when a case has the name, in a row or in a
    /// directory.
    pub(crate) async fn keep_case(
        &self,
        chosen: &ChosenSuite,
        name: &CaseName,
        run: &RunId,
        built: &Path,
    ) -> Result<PathBuf, CaptureError> {
        let suite = chosen.suite();
        let keeping = format!("keeping the case {name} in the suite {}", suite.name);
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .context(keeping.clone())?;
        if let ChosenSuite::New(new) = chosen {
            match insert_suite(&mut *tx, new).await {
                Ok(()) => {}
                Err(MakeError::Taken(taken)) => return Err(CaptureError::SuiteTaken(taken)),
                Err(MakeError::Failed(e)) => return Err(e.context(keeping).into()),
            }
        }
        let inserted = sqlx::query("INSERT INTO cases (name, suite, run) VALUES (?, ?, ?)")
            .bind(name.as_str())
            .bind(suite.name.as_str())
            .bind(run.to_string())
            .execute(&mut *tx)
            .await;
        match inserted {
            Ok(_) => {}
            Err(e) if unique_violation(&e) => return Err(CaptureError::Taken(name.clone())),
            Err(e) if foreign_key_violation(&e) => {
                return Err(CaptureError::NoSuchSuite(suite.name.clone()));
            }
            Err(e) => return Err(anyhow::Error::from(e).context(keeping).into()),
        }
        let place = self.cases_dir().join(name.as_str());
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

/// Inserts `suite`'s row with `executor`, refused when a suite has its name.
async fn insert_suite<'c>(
    executor: impl sqlx::SqliteExecutor<'c>,
    suite: &Suite,
) -> Result<(), MakeError> {
    let inserted = sqlx::query("INSERT INTO suites (name, stage) VALUES (?, ?)")
        .bind(suite.name.as_str())
        .bind(suite.stage.name())
        .execute(executor)
        .await;
    match inserted {
        Ok(_) => Ok(()),
        Err(e) if unique_violation(&e) => Err(MakeError::Taken(suite.name.clone())),
        Err(e) => Err(anyhow::Error::from(e)
            .context(format!("making the suite {}", suite.name))
            .into()),
    }
}

const READING: &str = "reading the suites";

/// The suite a row holds.
fn suite_of(name: &str, stage: &str) -> Result<Suite> {
    Ok(Suite {
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
pub struct SuitesAnswer {
    pub suites: Vec<SuiteListed>,
}

/// The suites as `suites` prints them: a line for each.
pub fn render_suites(answer: &SuitesAnswer) -> String {
    if answer.suites.is_empty() {
        return "suites: none\n".into();
    }
    let mut text = String::new();
    for listed in &answer.suites {
        let cases = match listed.cases {
            0 => "no cases".to_string(),
            1 => "1 case".to_string(),
            n => format!("{n} cases"),
        };
        let _ = writeln!(
            text,
            "{}: {}, {cases}",
            listed.suite.name, listed.suite.stage
        );
    }
    text
}

/// What `suite NAME` answers.
#[derive(Serialize)]
pub struct SuiteAnswer {
    #[serde(flatten)]
    pub suite: Suite,
    pub cases: Vec<CaseAnswer>,
    pub experiments: Vec<ExperimentSummary>,
}

/// A case as `suite NAME` answers it: its criteria, or why they do not
/// read.
#[derive(Serialize)]
pub struct CaseAnswer {
    pub name: CaseName,
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

impl SuiteAnswer {
    /// `shown`, its experiments in the states `states`, one for each in
    /// order.
    pub fn new(shown: &SuiteShown, states: Vec<Shown>) -> SuiteAnswer {
        SuiteAnswer {
            suite: shown.suite.clone(),
            cases: shown
                .cases
                .iter()
                .map(|case| CaseAnswer {
                    name: case.name.clone(),
                    run: case.run.clone(),
                    criteria: match &case.criteria {
                        Ok(criteria) => CriteriaAnswer::Criteria(criteria.as_str().to_string()),
                        Err(refused) => CriteriaAnswer::Unreadable(refused.why.clone()),
                    },
                })
                .collect(),
            experiments: shown
                .experiments
                .iter()
                .zip(states)
                .map(|(record, state)| ExperimentSummary::new(record, state))
                .collect(),
        }
    }
}

/// A suite as `suite NAME` prints it: its facts, then each case with its
/// run, and its criteria indented under it, then each experiment, latest
/// first.
pub fn render_suite(answer: &SuiteAnswer) -> String {
    let mut text = format!(
        "suite: {}\nstage: {}\n",
        answer.suite.name, answer.suite.stage
    );
    if answer.cases.is_empty() {
        text.push_str("cases: none\n");
    }
    for case in &answer.cases {
        let _ = writeln!(text, "case {}: captured from {}", case.name, case.run);
        match &case.criteria {
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
    if answer.experiments.is_empty() {
        text.push_str("experiments: none\n");
    }
    for experiment in &answer.experiments {
        text.push_str(&answers::render_summary(experiment));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_log::test]
    fn a_suites_name_is_lowercase_letters_digits_and_hyphens_starting_with_a_letter_or_a_digit() {
        for name in ["board", "design-2", "7"] {
            assert_eq!(
                name.parse::<SuiteName>().map(|n| n.to_string()),
                Ok(name.into())
            );
        }
        for name in [
            "", "-", "-board", "Board", "board_1", "board 1", "../board", "bōard",
        ] {
            assert!(name.parse::<SuiteName>().is_err(), "{name:?}");
        }
    }
}
