//! The agent runtime: headless coding-agent sessions, one per stage, each in
//! its own clone (`docs/specs/architecture.md`, "Agents are headless Claude Code
//! sessions").

pub mod claude;

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::domain::Timestamp;
use crate::reconcile::{Subject, WorkKind};
use crate::workflow::WorkflowName;

/// What a session is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// A stage of the pipeline, working on `subject`.
    Work { subject: Subject, kind: WorkKind },
    /// A run of the workflow with this name.
    Workflow(WorkflowName),
}

/// A session to start.
#[derive(Clone, Debug)]
pub struct SessionSpec {
    pub purpose: Purpose,
    /// The clone the session works in.
    pub dir: PathBuf,
    /// The session's own scratch directory, its temporary directory for
    /// every turn: sessions running at once never share one.
    pub scratch: PathBuf,
    /// Where the session's record goes: brief, results and trace.
    pub record: PathBuf,
    /// The plugin that gives the session the skills its brief calls.
    pub plugin: PathBuf,
    pub prompt: String,
    /// When the session times out, across all its turns. A run that waits
    /// out the usage limit moves it on by the wait.
    pub deadline: tokio::time::Instant,
}

/// The end of one turn of a session: its final message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub session: String,
    pub message: String,
    /// The end of the session's log, for a person reading why it gave up.
    pub log_tail: String,
}

/// The runtime refusing sessions for the usage limit, until `resets_at`
/// (`docs/specs/operation.md`, "Usage limit").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageLimit {
    pub resets_at: Timestamp,
}

impl UsageLimit {
    /// How long a refused session waits when its limit's reset has passed
    /// before it tries again.
    pub const SHORT_WAIT: chrono::TimeDelta = chrono::TimeDelta::minutes(1);

    /// Until when a session refused at `now` waits: the reset, or a short
    /// time when that has passed.
    pub fn wait_until(self, now: Timestamp) -> Timestamp {
        if self.resets_at > now {
            self.resets_at
        } else {
            now + UsageLimit::SHORT_WAIT
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The runtime could not run the session: worth one retry.
    #[error("{0}")]
    Infrastructure(String),
    /// The runtime refused the session's turn for the usage limit: not a
    /// failure.
    #[error("the agents' usage limit was reached; it resets at {}", .limit.resets_at)]
    UsageLimit {
        limit: UsageLimit,
        /// The session the turn got under way in, to carry on in with a
        /// resume; `None` when the turn was refused before it began, so
        /// that it is to be taken again.
        underway: Option<String>,
    },
    /// The session ended without a final message: it crashed, timed out or
    /// stopped on an error.
    #[error("{reason}")]
    Failed { reason: String, log_tail: String },
}

#[async_trait]
pub trait Runtime: Send + Sync {
    async fn start(&self, spec: &SessionSpec) -> Result<Turn, SessionError>;
    /// Continues a session that ended a turn, with a follow-up prompt.
    async fn resume(
        &self,
        spec: &SessionSpec,
        session: &str,
        prompt: &str,
    ) -> Result<Turn, SessionError>;
}
