//! What a session runs with: the settings a run takes when it starts, each
//! from the first layer that sets it (`docs/specs/operation.md`,
//! Configuration).

use std::sync::Arc;
use std::time::Duration;

use crate::automation::Automation;
use crate::layer::{Layer, Sourced};
use crate::reconcile::WorkKind;
use crate::session_env::Given;

/// `session-timeout`'s default.
pub const SESSION_TIMEOUT: Duration = Duration::from_mins(60);

/// The settings one layer sets, `None` where it sets none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Partial {
    pub model: Option<String>,
    pub session_timeout: Option<Duration>,
}

/// The operator's settings for sessions, each of its layers kept apart so
/// that a run can rank the repository's between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkSettings {
    pub flags_and_env: Partial,
    /// The operator's config file, but for the settings `flags_and_env`
    /// sets, which it could never win over.
    pub file: Partial,
    /// The layer `session-env` came from.
    pub session_env: Layer,
}

impl Default for WorkSettings {
    fn default() -> WorkSettings {
        WorkSettings {
            flags_and_env: Partial::default(),
            file: Partial::default(),
            session_env: Layer::Linewright,
        }
    }
}

/// The repository's settings for sessions.
#[derive(Clone, Debug, Default)]
pub struct RepoSettings {
    top: Partial,
}

/// What a run is, as far as its settings go.
#[derive(Clone, Copy, Debug)]
pub enum RunKind<'a> {
    Stage(WorkKind),
    Automation(&'a Automation),
}

/// The environment each session is given, worked out once at startup.
#[derive(Clone, Debug, Default)]
pub struct Environments {
    pipeline: Arc<Given>,
}

impl Environments {
    pub fn new(pipeline: Given) -> Environments {
        Environments {
            pipeline: Arc::new(pipeline),
        }
    }
}

/// What one run's sessions run with.
#[derive(Clone, Debug)]
pub struct SessionSettings {
    /// `None` is the harness's choice.
    pub model: Sourced<Option<String>>,
    pub session_timeout: Sourced<Duration>,
    pub environment: Sourced<Arc<Given>>,
}

impl WorkSettings {
    /// A setting takes the first value set among: the automation's front
    /// matter, the flags and environment, the repository's file, the
    /// operator's file, and the default.
    pub fn for_run(
        &self,
        of: RunKind,
        repo: &RepoSettings,
        environments: &Environments,
    ) -> SessionSettings {
        let front_matter = match of {
            RunKind::Automation(automation) => Partial {
                model: None,
                session_timeout: automation.timeout,
            },
            RunKind::Stage(_) => Partial::default(),
        };
        let ladder = [
            (&front_matter, Layer::Repository),
            (&self.flags_and_env, Layer::Operator),
            (&repo.top, Layer::Repository),
            (&self.file, Layer::Operator),
        ];
        let model = ladder
            .iter()
            .find_map(|(partial, layer)| {
                let model = partial.model.clone()?;
                Some(Sourced::new(Some(model), *layer))
            })
            .unwrap_or(Sourced::new(None, Layer::Linewright));
        let session_timeout = ladder
            .iter()
            .find_map(|(partial, layer)| Some(Sourced::new(partial.session_timeout?, *layer)))
            .unwrap_or(Sourced::new(SESSION_TIMEOUT, Layer::Linewright));
        SessionSettings {
            model,
            session_timeout,
            environment: Sourced::new(environments.pipeline.clone(), self.session_env),
        }
    }
}
