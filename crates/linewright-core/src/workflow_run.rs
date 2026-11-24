//! The attempts of workflow runs that start automations
//! (`docs/specs/automations.md`, "Workflow runs").

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use anyhow::{Context, Result};
use chrono::TimeDelta;
use serde::{Deserialize, Serialize};
use sqlx::{Row as _, SqliteConnection};

use crate::automation::{Automation, AutomationName, Trigger};
use crate::database::{Database, parse_time, time_text};
use crate::domain::Timestamp;
use crate::runs::State;

/// GitHub's id of a workflow run, which its re-runs keep.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
pub struct WorkflowRunId(pub u64);

impl std::fmt::Display for WorkflowRunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One attempt of a workflow run: its first run, or a re-run of it.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
pub struct AttemptId {
    pub run: WorkflowRunId,
    pub attempt: NonZeroU32,
}

impl AttemptId {
    /// The run's id and the attempt's number, as the database's columns
    /// hold them. Fails for a run id beyond what they hold.
    pub(crate) fn columns(self) -> Result<(i64, i64)> {
        let run = i64::try_from(self.run.0).with_context(|| {
            format!(
                "the workflow run {} is beyond what the database keeps",
                self.run
            )
        })?;
        Ok((run, i64::from(self.attempt.get())))
    }

    /// The attempt the database's columns `run` and `attempt` hold, as
    /// [`AttemptId::columns`] gives them.
    pub(crate) fn of_columns(run: i64, attempt: i64) -> Result<AttemptId> {
        Ok(AttemptId {
            run: WorkflowRunId(u64::try_from(run)?),
            attempt: NonZeroU32::try_from(u32::try_from(attempt)?)?,
        })
    }
}

/// A conclusion GitHub gives a completed attempt, in its wire spelling
/// (<https://docs.github.com/en/rest/actions/workflow-runs#list-workflow-runs-for-a-repository>).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Conclusion {
    Success,
    Failure,
    Cancelled,
    Skipped,
    TimedOut,
    ActionRequired,
    Neutral,
    Stale,
    StartupFailure,
}

impl Conclusion {
    /// Its spelling on GitHub's wire, such as `timed_out`.
    pub fn wire(self) -> &'static str {
        match self {
            Conclusion::Success => "success",
            Conclusion::Failure => "failure",
            Conclusion::Cancelled => "cancelled",
            Conclusion::Skipped => "skipped",
            Conclusion::TimedOut => "timed_out",
            Conclusion::ActionRequired => "action_required",
            Conclusion::Neutral => "neutral",
            Conclusion::Stale => "stale",
            Conclusion::StartupFailure => "startup_failure",
        }
    }
}

/// The conclusion of a completed attempt: one of [`Conclusion`], or one
/// GitHub has added since.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum AttemptConclusion {
    Known(Conclusion),
    Other(String),
}

impl AttemptConclusion {
    /// Its spelling on GitHub's wire, such as `failure`.
    pub fn wire(&self) -> &str {
        match self {
            AttemptConclusion::Known(conclusion) => conclusion.wire(),
            AttemptConclusion::Other(conclusion) => conclusion,
        }
    }

    fn of_wire(wire: &str) -> AttemptConclusion {
        serde_json::from_value(serde_json::Value::String(wire.to_string()))
            .unwrap_or_else(|_| AttemptConclusion::Other(wire.to_string()))
    }
}

/// An attempt of a workflow run that has completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletedAttempt {
    pub id: AttemptId,
    /// The name of the workflow run's workflow.
    pub workflow: String,
    /// When GitHub created the workflow run, its first attempt.
    pub run_created_at: Timestamp,
    pub completed_at: Timestamp,
    pub conclusion: AttemptConclusion,
    /// The workflow run's head branch: none for a run of a fork's commit.
    pub branch: Option<String>,
    /// The full name of the commit the workflow run ran on.
    pub commit: String,
    /// The attempt's page on GitHub,
    /// `https://github.com/OWNER/REPO/actions/runs/ID/attempts/N`.
    pub link: String,
}

/// A set with at least one value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NonEmptySet<T: Ord>(BTreeSet<T>);

impl<T: Ord> NonEmptySet<T> {
    pub fn contains(&self, value: &T) -> bool {
        self.0.contains(value)
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }
}

/// Fails on an empty list.
impl<T: Ord> TryFrom<Vec<T>> for NonEmptySet<T> {
    type Error = Empty;

    fn try_from(values: Vec<T>) -> Result<NonEmptySet<T>, Empty> {
        if values.is_empty() {
            return Err(Empty);
        }
        Ok(NonEmptySet(values.into_iter().collect()))
    }
}

/// A list was empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Empty;

/// An automation's `on.workflow_run`: the attempts it runs on. A filter left
/// out matches everything.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Listed")]
pub struct Filters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflows: Option<NonEmptySet<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branches: Option<NonEmptySet<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conclusions: Option<NonEmptySet<Conclusion>>,
}

/// The filters as the front matter lists them: a key given no value is
/// present.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listed {
    #[serde(default, deserialize_with = "null_as_default")]
    workflows: Option<Vec<String>>,
    #[serde(default, deserialize_with = "null_as_default")]
    branches: Option<Vec<String>>,
    #[serde(default, deserialize_with = "null_as_default")]
    conclusions: Option<Vec<Conclusion>>,
}

/// `Some` for a key that is present, its default when its value is null:
/// serde otherwise reads a key with a null value as one left out
/// (<https://github.com/serde-rs/serde/issues/984>).
pub(crate) fn null_as_default<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer).map(|value| Some(value.unwrap_or_default()))
}

impl TryFrom<Listed> for Filters {
    type Error = String;

    /// Fails on a filter that is empty or has no value, naming it.
    fn try_from(listed: Listed) -> Result<Filters, String> {
        fn filter<T: Ord>(
            name: &str,
            values: Option<Vec<T>>,
        ) -> Result<Option<NonEmptySet<T>>, String> {
            values
                .map(|values| {
                    values.try_into().map_err(|Empty| {
                        format!("the filter `{name}` is empty: leave it out to match everything")
                    })
                })
                .transpose()
        }
        Ok(Filters {
            workflows: filter("workflows", listed.workflows)?,
            branches: filter("branches", listed.branches)?,
            conclusions: filter("conclusions", listed.conclusions)?,
        })
    }
}

impl Filters {
    /// When an automation on these runs, in words, such as "When a workflow
    /// run of "CI" on "main" completes as failure.". Workflows and branches
    /// are quoted, since their names can hold any words.
    pub fn describe(&self) -> String {
        fn either(values: impl Iterator<Item = String>) -> String {
            values.collect::<Vec<_>>().join(" or ")
        }
        fn quoted(names: &NonEmptySet<String>) -> String {
            either(names.iter().map(|name| format!("\"{name}\"")))
        }
        let mut words = "When a workflow run".to_string();
        if let Some(workflows) = &self.workflows {
            words += &format!(" of {}", quoted(workflows));
        }
        if let Some(branches) = &self.branches {
            words += &format!(" on {}", quoted(branches));
        }
        words += " completes";
        if let Some(conclusions) = &self.conclusions {
            words += &format!(
                " as {}",
                either(
                    conclusions
                        .iter()
                        .map(|conclusion| conclusion.wire().to_string())
                )
            );
        }
        words + "."
    }

    /// Whether `attempt` matches every filter. A `branches` filter matches no
    /// attempt without a branch, and a `conclusions` filter none whose
    /// conclusion is not one of [`Conclusion`]'s.
    pub fn matches(&self, attempt: &CompletedAttempt) -> bool {
        let workflow = self
            .workflows
            .as_ref()
            .is_none_or(|workflows| workflows.contains(&attempt.workflow));
        let branch = self.branches.as_ref().is_none_or(|branches| {
            attempt
                .branch
                .as_ref()
                .is_some_and(|branch| branches.contains(branch))
        });
        let conclusion = self.conclusions.as_ref().is_none_or(|conclusions| {
            matches!(&attempt.conclusion, AttemptConclusion::Known(c) if conclusions.contains(c))
        });
        workflow && branch && conclusion
    }
}

/// Whether Linewright, reading `attempt` at `now`, reads it within
/// `ci_timeout` after GitHub created its workflow run
/// (`docs/specs/automations.md`, The time limit).
pub fn within_limit(attempt: &CompletedAttempt, now: Timestamp, ci_timeout: TimeDelta) -> bool {
    now <= attempt.run_created_at + ci_timeout
}

/// An automation's `on.workflow_run` as Linewright last saw it, and when it
/// first saw it so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub filters: Filters,
    pub since: Timestamp,
}

/// Each automation on `workflow_run` by its name, with its filters as
/// Linewright last saw them and since when
/// (`docs/specs/automations.md`, Earlier attempts).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TriggersSeen(BTreeMap<AutomationName, Seen>);

impl TriggersSeen {
    pub fn get(&self, name: &AutomationName) -> Option<&Seen> {
        self.0.get(name)
    }

    /// When Linewright first saw `name` with `filters`: `now` when the
    /// filters recorded for it are others, or none are.
    fn since(&self, name: &AutomationName, filters: &Filters, now: Timestamp) -> Timestamp {
        match self.0.get(name) {
            Some(seen) if seen.filters == *filters => seen.since,
            _ => now,
        }
    }

    /// Empty until first saved.
    pub async fn load(db: &Database) -> Result<TriggersSeen> {
        let rows = sqlx::query("SELECT name, filters, since FROM automation_workflow_triggers")
            .fetch_all(db.pool())
            .await
            .context("reading the automations' workflow-run triggers")?;
        rows.iter()
            .map(|row| {
                let name = row
                    .try_get::<&str, _>("name")?
                    .parse()
                    .map_err(anyhow::Error::msg)?;
                let filters = serde_json::from_str(row.try_get("filters")?)?;
                let since = parse_time(row.try_get("since")?)?;
                Ok((name, Seen { filters, since }))
            })
            .collect::<Result<_>>()
            .context("reading the automations' workflow-run triggers")
    }

    /// Keeps these, in place of those kept.
    pub async fn save(&self, db: &Database) -> Result<()> {
        const SAVING: &str = "recording the automations' workflow-run triggers";
        let mut tx = db.pool().begin().await.context(SAVING)?;
        self.replace(&mut tx).await.context(SAVING)?;
        tx.commit().await.context(SAVING)
    }

    async fn replace(&self, conn: &mut SqliteConnection) -> Result<()> {
        sqlx::query("DELETE FROM automation_workflow_triggers")
            .execute(&mut *conn)
            .await?;
        for (name, seen) in &self.0 {
            sqlx::query(
                "INSERT INTO automation_workflow_triggers (name, filters, since) VALUES (?, ?, ?)",
            )
            .bind(name.to_string())
            .bind(serde_json::to_string(&seen.filters)?)
            .bind(time_text(seen.since))
            .execute(&mut *conn)
            .await?;
        }
        Ok(())
    }
}

impl FromIterator<(AutomationName, Seen)> for TriggersSeen {
    fn from_iter<I: IntoIterator<Item = (AutomationName, Seen)>>(seen: I) -> TriggersSeen {
        TriggersSeen(seen.into_iter().collect())
    }
}

/// The completed attempts Linewright has read and keeps, as the queue of
/// the automations on `workflow_run` (`docs/specs/automations.md`, In
/// order).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attempts(BTreeMap<AttemptId, CompletedAttempt>);

impl Attempts {
    pub async fn load(db: &Database) -> Result<Attempts> {
        let rows = sqlx::query(
            "SELECT run, attempt, workflow, run_created_at, completed_at, conclusion, branch, \
                commit_sha, link FROM workflow_attempts",
        )
        .fetch_all(db.pool())
        .await
        .context(READING)?;
        rows.iter()
            .map(|row| {
                let id = AttemptId::of_columns(row.try_get("run")?, row.try_get("attempt")?)?;
                let attempt = CompletedAttempt {
                    id,
                    workflow: row.try_get("workflow")?,
                    run_created_at: parse_time(row.try_get("run_created_at")?)?,
                    completed_at: parse_time(row.try_get("completed_at")?)?,
                    conclusion: AttemptConclusion::of_wire(row.try_get("conclusion")?),
                    branch: row.try_get("branch")?,
                    commit: row.try_get("commit_sha")?,
                    link: row.try_get("link")?,
                };
                Ok((id, attempt))
            })
            .collect::<Result<_>>()
            .map(Attempts)
            .context(READING)
    }

    /// The ids of the attempts kept.
    pub fn known(&self) -> BTreeSet<AttemptId> {
        self.0.keys().copied().collect()
    }

    /// The attempts kept, by their ids.
    pub fn all(&self) -> Vec<CompletedAttempt> {
        self.0.values().cloned().collect()
    }

    /// Keeps `attempts`, leaving each whose id is kept already as it was.
    pub async fn add(&mut self, db: &Database, attempts: Vec<CompletedAttempt>) -> Result<()> {
        const ADDING: &str = "keeping the workflow runs' attempts read";
        let mut tx = db.pool().begin().await.context(ADDING)?;
        let mut added = vec![];
        for attempt in attempts {
            if self.0.contains_key(&attempt.id) {
                continue;
            }
            let (run, number) = attempt.id.columns()?;
            sqlx::query(
                "INSERT OR IGNORE INTO workflow_attempts (run, attempt, workflow, run_created_at, \
                    completed_at, conclusion, branch, commit_sha, link) \
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(run)
            .bind(number)
            .bind(&attempt.workflow)
            .bind(time_text(attempt.run_created_at))
            .bind(time_text(attempt.completed_at))
            .bind(attempt.conclusion.wire())
            .bind(&attempt.branch)
            .bind(&attempt.commit)
            .bind(&attempt.link)
            .execute(&mut *tx)
            .await
            .context(ADDING)?;
            added.push(attempt);
        }
        tx.commit().await.context(ADDING)?;
        self.0
            .extend(added.into_iter().map(|attempt| (attempt.id, attempt)));
        Ok(())
    }

    /// Drops every attempt kept but those `keep` keeps.
    pub async fn prune(
        &mut self,
        db: &Database,
        keep: impl Fn(&CompletedAttempt) -> bool,
    ) -> Result<()> {
        const PRUNING: &str = "dropping the workflow runs' attempts no longer needed";
        let pruned: Vec<AttemptId> = self
            .0
            .values()
            .filter(|attempt| !keep(attempt))
            .map(|attempt| attempt.id)
            .collect();
        if pruned.is_empty() {
            return Ok(());
        }
        let mut tx = db.pool().begin().await.context(PRUNING)?;
        for id in &pruned {
            let (run, attempt) = id.columns()?;
            sqlx::query("DELETE FROM workflow_attempts WHERE run = ? AND attempt = ?")
                .bind(run)
                .bind(attempt)
                .execute(&mut *tx)
                .await
                .context(PRUNING)?;
        }
        tx.commit().await.context(PRUNING)?;
        for id in pruned {
            self.0.remove(&id);
        }
        Ok(())
    }
}

const READING: &str = "reading the workflow runs' attempts kept";

/// A run of an automation that an attempt started, as its record has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggeredRun {
    pub automation: AutomationName,
    pub attempt: AttemptId,
    pub state: State,
}

/// What [`trigger`] decides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Triggered {
    /// The runs to start, each of an automation on the attempt that starts
    /// it, earliest completed first.
    pub runs: Vec<(Automation, CompletedAttempt)>,
    /// The triggers seen, with those of `automations` as they are now.
    pub seen: TriggersSeen,
}

/// The runs that `attempts` start of `automations`, at most `room`: for
/// each automation on `workflow_run` that `held` does not hold, its pending
/// attempt that completed earliest. An attempt is pending for an
/// automation when it matches the automation's filters, completed at or
/// after Linewright first saw them, and no run of `runs` it started has run
/// the automation. Ties go by the attempts' ids, then the automations'
/// names. An automation on `workflow_run` whose filters differ from those
/// `seen` records is seen with them at `now`; one not on `workflow_run`
/// loses its record, so that it counts as new when it comes back to one
/// (`docs/specs/automations.md`, Workflow runs).
pub fn trigger(
    automations: &[Automation],
    seen: &TriggersSeen,
    attempts: &[CompletedAttempt],
    runs: &[TriggeredRun],
    held: impl Fn(&Automation) -> bool,
    room: usize,
    now: Timestamp,
) -> Triggered {
    let mut now_seen = seen.clone();
    let mut next = vec![];
    for automation in automations {
        let Trigger::WorkflowRun(filters) = &automation.trigger else {
            now_seen.0.remove(&automation.name);
            continue;
        };
        let since = seen.since(&automation.name, filters, now);
        now_seen.0.insert(
            automation.name.clone(),
            Seen {
                filters: filters.clone(),
                since,
            },
        );
        if held(automation) {
            continue;
        }
        let earliest = attempts
            .iter()
            .filter(|attempt| {
                awaits(&automation.name, filters, since, attempt, runs)
                    && !runs.iter().any(|run| {
                        run.automation == automation.name
                            && run.attempt == attempt.id
                            && run.state == State::InFlight
                    })
            })
            .min_by_key(|attempt| (attempt.completed_at, attempt.id));
        if let Some(attempt) = earliest {
            next.push((automation, attempt));
        }
    }
    next.sort_by_key(|(automation, attempt)| (attempt.completed_at, attempt.id, &automation.name));
    Triggered {
        runs: next
            .into_iter()
            .take(room)
            .map(|(automation, attempt)| (automation.clone(), attempt.clone()))
            .collect(),
        seen: now_seen,
    }
}

/// The attempts that some automation of `automations` still awaits, as
/// [`trigger`] has them at `now`, given `seen` and `runs`: those it has
/// pending, and those whose run of it is in flight, which run it again if
/// that run shows as cancelled (`docs/specs/automations.md`, A run that
/// shows as cancelled). They are what [`Attempts::prune`] must keep.
pub fn awaited<'a>(
    automations: &'a [Automation],
    seen: &'a TriggersSeen,
    runs: &'a [TriggeredRun],
    now: Timestamp,
) -> impl Fn(&CompletedAttempt) -> bool + 'a {
    move |attempt| {
        automations
            .iter()
            .any(|automation| match &automation.trigger {
                Trigger::WorkflowRun(filters) => {
                    let since = seen.since(&automation.name, filters, now);
                    awaits(&automation.name, filters, since, attempt, runs)
                }
                Trigger::Schedule(_) => false,
            })
    }
}

/// Whether the automation `name` on `filters`, first seen `since`, awaits
/// `attempt`: it matches them, completed since, and no run of `runs` it
/// started ended with an outcome that does not show as cancelled
/// (`docs/specs/automations.md`, A run that shows as cancelled). It is
/// pending unless such a run is in flight.
fn awaits(
    name: &AutomationName,
    filters: &Filters,
    since: Timestamp,
    attempt: &CompletedAttempt,
    runs: &[TriggeredRun],
) -> bool {
    filters.matches(attempt)
        && attempt.completed_at >= since
        && !runs.iter().any(|run| {
            run.automation == *name && run.attempt == attempt.id && run.state.ended_not_cancelled()
        })
}
