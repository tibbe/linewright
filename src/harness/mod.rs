pub mod claude;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::automation::AutomationName;
use crate::domain::Timestamp;
use crate::reconcile::{Subject, WorkKind};
use crate::redaction::Redaction;
use crate::session_env::Given;
use crate::work_settings::Effort;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Purpose {
    Work { subject: Subject, kind: WorkKind },
    Automation(AutomationName),
}

#[derive(Clone, Debug)]
pub struct SessionSpec {
    pub purpose: Purpose,
    pub dir: PathBuf,
    /// The temporary directory for every turn: sessions running at once never
    /// share one.
    pub scratch: PathBuf,
    pub record: PathBuf,
    /// The session's attachments directory, in its record
    /// (docs/specs/runs.md, "Attachments").
    pub attachments: PathBuf,
    pub plugin: PathBuf,
    pub prompt: String,
    /// `None` is the harness's choice.
    pub model: Option<String>,
    /// `None` is the harness's choice.
    pub effort: Option<Effort>,
    /// A session starts from these alone, and what the harness sets itself.
    pub environment: Arc<Given>,
    /// When the session times out, across all its turns. A run that waits
    /// out the usage limit moves it on by the wait.
    pub deadline: tokio::time::Instant,
    /// Told what the harness reports of the session as each turn runs, so
    /// that a turn that never returns has still told it.
    pub report: Reporter,
}

/// What the harness reports a session ran with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reported {
    pub model: String,
}

/// Where the harness's reports of a session go.
#[derive(Clone)]
pub struct Reporter(Arc<dyn Fn(Reported) + Send + Sync>);

impl Reporter {
    pub fn new(report: impl Fn(Reported) + Send + Sync + 'static) -> Reporter {
        Reporter(Arc::new(report))
    }

    pub fn report(&self, reported: Reported) {
        (self.0)(reported);
    }
}

impl std::fmt::Debug for Reporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reporter")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub session: String,
    pub message: String,
    pub log_tail: String,
}

impl Turn {
    #[must_use]
    pub fn redacted(self, redaction: &Redaction) -> Turn {
        Turn {
            session: self.session,
            message: redaction.apply(&self.message),
            log_tail: redaction.apply(&self.log_tail),
        }
    }
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

impl SessionError {
    #[must_use]
    pub fn redacted(self, redaction: &Redaction) -> SessionError {
        match self {
            SessionError::Infrastructure(reason) => {
                SessionError::Infrastructure(redaction.apply(&reason))
            }
            SessionError::TimedOut { log_tail } => SessionError::TimedOut {
                log_tail: redaction.apply(&log_tail),
            },
            SessionError::Failed { reason, log_tail } => SessionError::Failed {
                reason: redaction.apply(&reason),
                log_tail: redaction.apply(&log_tail),
            },
            error @ SessionError::UsageLimit { .. } => error,
        }
    }
}

#[async_trait]
pub trait Harness: Send + Sync {
    async fn start(&self, spec: &SessionSpec) -> Result<Turn, SessionError>;
    async fn resume(
        &self,
        spec: &SessionSpec,
        session: &str,
        prompt: &str,
    ) -> Result<Turn, SessionError>;
}
