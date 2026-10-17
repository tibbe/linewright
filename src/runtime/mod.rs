pub mod claude;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::clock::Clock;
use crate::domain::Timestamp;
use crate::reconcile::{Subject, WorkKind};
use crate::runs::Activity;
use crate::workflow::WorkflowName;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Purpose {
    Work { subject: Subject, kind: WorkKind },
    Workflow(WorkflowName),
}

#[derive(Clone, Debug)]
pub struct SessionSpec {
    pub purpose: Purpose,
    pub dir: PathBuf,
    /// The temporary directory for every turn: sessions running at once never
    /// share one.
    pub scratch: PathBuf,
    pub record: PathBuf,
    pub plugin: PathBuf,
    pub prompt: String,
    /// When the session times out, across all its turns. A run that waits
    /// out the usage limit moves it on by the wait.
    pub deadline: tokio::time::Instant,
    pub output: OutputWatch,
}

#[derive(Clone)]
pub struct OutputWatch {
    activity: Activity,
    clock: Arc<dyn Clock>,
}

impl OutputWatch {
    pub fn new(activity: Activity, clock: Arc<dyn Clock>) -> OutputWatch {
        OutputWatch { activity, clock }
    }

    pub fn heard(&self) {
        self.activity.note(self.clock.now());
    }
}

impl std::fmt::Debug for OutputWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputWatch")
            .field("activity", &self.activity)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub session: String,
    pub message: String,
    pub log_tail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageLimit {
    pub resets_at: Timestamp,
}

impl UsageLimit {
    pub const SHORT_WAIT: chrono::TimeDelta = chrono::TimeDelta::minutes(1);

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
    /// Worth one retry.
    #[error("{0}")]
    Infrastructure(String),
    #[error("the agents' usage limit was reached; it resets at {}", .limit.resets_at)]
    UsageLimit {
        limit: UsageLimit,
        /// The session the turn got under way in, to carry on in with a
        /// resume; `None` when the turn was refused before it began, so
        /// that it is to be taken again.
        underway: Option<String>,
    },
    /// Ran past its deadline.
    #[error("it timed out")]
    TimedOut { log_tail: String },
    #[error("{reason}")]
    Failed { reason: String, log_tail: String },
}

#[async_trait]
pub trait Runtime: Send + Sync {
    async fn start(&self, spec: &SessionSpec) -> Result<Turn, SessionError>;
    async fn resume(
        &self,
        spec: &SessionSpec,
        session: &str,
        prompt: &str,
    ) -> Result<Turn, SessionError>;
}
