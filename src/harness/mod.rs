pub mod claude;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::automation::AutomationName;
use crate::cost::TurnSpend;
use crate::domain::Timestamp;
use crate::reconcile::{Subject, WorkKind};
use crate::redaction::Redaction;
use crate::session_env::Given;
use crate::trial::GhConfigDir;
use crate::work_settings::Effort;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Purpose {
    Work {
        subject: Subject,
        kind: WorkKind,
    },
    Automation(AutomationName),
    /// The scorer of a trial (`docs/specs/benchmarks.md`, Scoring).
    Scorer,
}

/// How a session's `gh` reaches GitHub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GhReach {
    /// As it is configured on the host.
    Direct,
    /// Through the configuration a trial's gate serves
    /// (`docs/specs/benchmarks.md`, The repository on GitHub).
    Through(GhConfigDir),
}

impl GhReach {
    /// The configuration of `gh` it goes through, if any.
    pub fn config_dir(&self) -> Option<&Path> {
        match self {
            GhReach::Direct => None,
            GhReach::Through(config) => Some(config.path()),
        }
    }
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
    pub gh: GhReach,
    /// When the session times out, across all its turns.
    pub deadline: Deadline,
    /// Told what the harness reports of the session as each turn runs, so
    /// that a turn that never returns has still told it.
    pub report: Reporter,
}

/// What the harness reports of a session as it runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Reported {
    /// The model the session runs on.
    Model(String),
    /// A turn began.
    TurnStarted,
    /// A turn's process ended, whatever the outcome, a timeout included. A
    /// cancelled turn reports no end.
    TurnEnded(TurnSpend),
}

/// Where the reports of a session go: the harness's as each turn runs, and
/// the session's end.
#[derive(Clone)]
pub struct Reporter {
    report: Arc<dyn Fn(Reported) + Send + Sync>,
    end: Arc<dyn Fn(Duration) + Send + Sync>,
}

impl Reporter {
    /// Takes the harness's reports to `report`, and the session's end
    /// nowhere.
    pub fn new(report: impl Fn(Reported) + Send + Sync + 'static) -> Reporter {
        Reporter {
            report: Arc::new(report),
            end: Arc::new(|_| {}),
        }
    }

    /// Takes the session's end, once it is reported, to `end`.
    #[must_use]
    pub fn ending(self, end: impl Fn(Duration) + Send + Sync + 'static) -> Reporter {
        Reporter {
            end: Arc::new(end),
            ..self
        }
    }

    pub fn report(&self, reported: Reported) {
        (self.report)(reported);
    }

    /// The session ended, the session timeout having counted `time`. Reported
    /// once.
    pub fn ended(&self, time: Duration) {
        (self.end)(time);
    }
}

/// When a session times out, and the timeout it was set from: the session
/// timeout counts from the session's start, less the waits for the usage
/// limit (docs/specs/runs.md, "Limits").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline {
    at: tokio::time::Instant,
    timeout: Duration,
}

impl Deadline {
    /// The deadline of a session that starts at `start` with `timeout`.
    pub fn after(start: tokio::time::Instant, timeout: Duration) -> Deadline {
        Deadline {
            at: start + timeout,
            timeout,
        }
    }

    pub fn at(self) -> tokio::time::Instant {
        self.at
    }

    /// What the session timeout has counted by `now`.
    pub fn counted(self, now: tokio::time::Instant) -> Duration {
        match self.at.checked_duration_since(now) {
            Some(left) => self.timeout.saturating_sub(left),
            None => self.timeout.saturating_add(now - self.at),
        }
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

    /// The deadline of a session resumed at `woke_at` after waiting out the
    /// usage limit that refused it at `refused_at`, when its deadline was
    /// `deadline`: it has the time it had left (docs/specs/runs.md, "Usage
    /// limit", The refused session).
    pub fn deadline_after_wait(
        deadline: Deadline,
        refused_at: tokio::time::Instant,
        woke_at: tokio::time::Instant,
    ) -> Deadline {
        Deadline {
            at: woke_at + deadline.at.saturating_duration_since(refused_at),
            ..deadline
        }
    }
}

/// The turn a session takes next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NextTurn {
    Start,
    Resume { session: String, prompt: String },
}

/// What a session that ran past its deadline did, as its error says.
pub const TIMED_OUT: &str = "it timed out";

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Worth one retry.
    #[error("{0}")]
    Infrastructure(String),
    #[error("the agents' usage limit was reached; it resets at {}", .limit.resets_at)]
    UsageLimit {
        limit: UsageLimit,
        /// The refused session, to be resumed (docs/specs/runs.md, "Usage
        /// limit", Resuming). Claude Code keeps the refused prompt in it:
        /// <https://code.claude.com/docs/en/interactive-mode#wait-for-a-usage-limit-to-reset>.
        session: String,
    },
    /// Ran past its deadline.
    #[error("{TIMED_OUT}")]
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

/// Runs sessions' turns. A turn returns only once all it started has ended.
#[async_trait]
pub trait Harness: Send + Sync {
    /// The whole environment a session starts from, given `given`, its
    /// scratch directory, its effort and how its `gh` reaches GitHub:
    /// `given`, then what Linewright sets over it. A variable set twice
    /// takes its later value.
    fn environment(
        &self,
        given: &Given,
        scratch: &Path,
        effort: Option<Effort>,
        gh: &GhReach,
    ) -> Vec<(OsString, OsString)>;
    async fn start(&self, spec: &SessionSpec) -> Result<Turn, SessionError>;
    async fn resume(
        &self,
        spec: &SessionSpec,
        session: &str,
        prompt: &str,
    ) -> Result<Turn, SessionError>;
}
