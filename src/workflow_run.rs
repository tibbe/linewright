//! The attempts of workflow runs that start automations
//! (`docs/specs/automations.md`, "Workflow runs").

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use serde::Deserialize;

use crate::domain::Timestamp;

/// GitHub's id of a workflow run, which its re-runs keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
pub struct WorkflowRunId(pub u64);

impl std::fmt::Display for WorkflowRunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One attempt of a workflow run: its first run, or a re-run of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AttemptId {
    pub run: WorkflowRunId,
    pub attempt: NonZeroU32,
}

/// A conclusion GitHub gives a completed attempt, in its wire spelling
/// (<https://docs.github.com/en/rest/actions/workflow-runs#list-workflow-runs-for-a-repository>).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Listed")]
pub struct Filters {
    pub workflows: Option<NonEmptySet<String>>,
    pub branches: Option<NonEmptySet<String>>,
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
