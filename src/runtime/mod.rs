//! The agent runtime: headless coding-agent sessions, one per stage, each in
//! its own clone (`docs/specs/architecture.md`, "Agents are headless Claude Code
//! sessions").

pub mod claude;

use std::path::PathBuf;

use async_trait::async_trait;

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
    /// Where the session's record goes: brief, results and trace.
    pub record: PathBuf,
    pub prompt: String,
    /// When the session times out, across all its turns.
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

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The runtime could not run the session: worth one retry.
    #[error("{0}")]
    Infrastructure(String),
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
