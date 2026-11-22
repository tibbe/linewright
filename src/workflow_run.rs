//! The attempts of workflow runs that start automations
//! (`docs/specs/automations.md`, "Workflow runs").

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
