//! What a session runs with: the settings a run takes when it starts, each
//! from the first layer that sets it (`docs/specs/operation.md`,
//! Configuration and Stage settings).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::automation::Automation;
use crate::layer::{Layer, Source, Sourced};
use crate::reconcile::WorkKind;
use crate::session_env::{Given, PassEnv, SessionEnv, SetEnv};

/// `session-timeout`'s default.
pub const SESSION_TIMEOUT: Duration = Duration::from_mins(60);

/// How hard the harness thinks: the `effort` setting's levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub const ALL: [Effort; 5] = [
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::Xhigh,
        Effort::Max,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
        }
    }
}

/// The error names the setting and the levels it takes.
impl std::str::FromStr for Effort {
    type Err = String;

    fn from_str(name: &str) -> Result<Effort, String> {
        Effort::ALL
            .into_iter()
            .find(|effort| effort.name() == name)
            .ok_or_else(|| {
                let levels = Effort::ALL.map(|effort| format!("`{effort}`"));
                let (last, rest) = levels.split_last().expect("there are levels");
                format!("`effort` is {name:?}: it is {} or {last}", rest.join(", "))
            })
    }
}

impl fmt::Display for Effort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The settings a stage can set: the keys of its table
/// (`docs/specs/operation.md`, Stage settings).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Setting {
    Model,
    Effort,
    SessionTimeout,
    Setup,
    SessionEnv,
    SessionPassEnv,
}

impl Setting {
    pub const ALL: [Setting; 6] = [
        Setting::Model,
        Setting::Effort,
        Setting::SessionTimeout,
        Setting::Setup,
        Setting::SessionEnv,
        Setting::SessionPassEnv,
    ];

    /// Its key in a config file.
    pub const fn name(self) -> &'static str {
        match self {
            Setting::Model => "model",
            Setting::Effort => "effort",
            Setting::SessionTimeout => "session-timeout",
            Setting::Setup => "setup",
            Setting::SessionEnv => "session-env",
            Setting::SessionPassEnv => "session-pass-env",
        }
    }
}

impl std::str::FromStr for Setting {
    type Err = String;

    fn from_str(name: &str) -> Result<Setting, String> {
        Setting::ALL
            .into_iter()
            .find(|setting| setting.name() == name)
            .ok_or_else(|| format!("{name:?} is not a stage's setting"))
    }
}

impl Serialize for Setting {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for Setting {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Setting, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// The `setup` setting: the setup command each session's clone is prepared
/// with (`docs/specs/runs.md`, Setup).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Setup {
    /// Runs nothing. Set in a layer, it wins over a command set below it.
    Empty,
    Command(SetupCommand),
}

/// A setup command, never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupCommand(String);

impl SetupCommand {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An empty string is [`Setup::Empty`].
impl From<String> for Setup {
    fn from(text: String) -> Setup {
        if text.is_empty() {
            Setup::Empty
        } else {
            Setup::Command(SetupCommand(text))
        }
    }
}

/// The settings one layer sets, `None` where it sets none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Partial {
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub session_timeout: Option<Duration>,
    pub setup: Option<Setup>,
}

/// A stage's table in the operator's file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StageTable {
    pub settings: Partial,
    /// The values it adds to the whole pipeline's, `None` when it sets
    /// none.
    pub session_env: Option<SetEnv>,
    /// The names it adds to the whole pipeline's, `None` when it sets none.
    pub session_pass_env: Option<PassEnv>,
}

impl StageTable {
    /// What it adds to the whole pipeline's sessions' environment, `None`
    /// when it sets neither of its settings.
    fn session_environment(&self) -> Option<SessionEnv> {
        if self.session_env.is_none() && self.session_pass_env.is_none() {
            return None;
        }
        Some(SessionEnv::new(
            self.session_pass_env.clone().unwrap_or_default(),
            self.session_env.clone().unwrap_or_default(),
        ))
    }
}

/// The operator's settings for sessions, each of its layers kept apart so
/// that a run can rank the repository's between them.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WorkSettings {
    pub flags_and_env: Partial,
    /// The operator's config file, but for the settings `flags_and_env`
    /// sets, which it could never win over.
    pub file: Partial,
    /// The operator's file's `[stage.NAME]` tables.
    pub stages: BTreeMap<WorkKind, StageTable>,
    /// The layers the whole pipeline's `session-env` and
    /// `session-pass-env` came from.
    pub env_layers: EnvLayers,
}

/// The layers `session-env` and `session-pass-env` came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnvLayers {
    pub set: Layer,
    pub pass: Layer,
}

impl Default for EnvLayers {
    fn default() -> EnvLayers {
        EnvLayers {
            set: Layer::Linewright,
            pass: Layer::Linewright,
        }
    }
}

impl EnvLayers {
    /// These for a stage whose table, the operator's, is `table`.
    fn with(self, table: Option<&StageTable>) -> EnvLayers {
        let Some(table) = table else {
            return self;
        };
        let operator_if = |sets: bool, layer| if sets { Layer::Operator } else { layer };
        EnvLayers {
            set: operator_if(table.session_env.is_some(), self.set),
            pass: operator_if(table.session_pass_env.is_some(), self.pass),
        }
    }
}

/// The repository's settings for sessions.
#[derive(Clone, Debug, Default)]
pub struct RepoSettings {
    top: Partial,
    stages: BTreeMap<WorkKind, Partial>,
}

impl RepoSettings {
    pub(crate) fn new(top: Partial, stages: BTreeMap<WorkKind, Partial>) -> RepoSettings {
        RepoSettings { top, stages }
    }
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
    /// The stages whose tables add to `session-env` or `session-pass-env`.
    stages: BTreeMap<WorkKind, Arc<Given>>,
}

/// Linewright's environment, split by what each stage's sessions are given.
#[derive(Debug, Default)]
pub struct Split {
    pub environments: Environments,
    /// The names of the variables the whole pipeline's sessions are not
    /// given, sorted, but for those Linewright sets for sessions itself.
    pub withheld: Vec<String>,
    /// Of `withheld`, the names each stage's table gives its sessions.
    pub added: BTreeMap<WorkKind, Vec<String>>,
    /// The names the whole pipeline's `session-env` sets, sorted.
    pub set: Vec<String>,
    /// The names set for each stage whose table sets `session-env`.
    pub stages_set: BTreeMap<WorkKind, StageSet>,
}

/// The names `session-env` sets for a stage whose table sets it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StageSet {
    /// Those its table sets, sorted.
    pub own: Vec<String>,
    /// Those it takes from the whole pipeline's, sorted.
    pub taken: Vec<String>,
}

/// The setting a session's timeout came from. A timeout bounds what a run
/// costs, so a timed-out run is not retried: its hand-back names the setting
/// that would give the next run longer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeoutSetting {
    /// The automation's `timeout`.
    Automation,
    /// `session-timeout` in the stage's table.
    Stage(WorkKind),
    /// The whole pipeline's `session-timeout`.
    Pipeline,
}

impl fmt::Display for TimeoutSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimeoutSetting::Automation => f.write_str("the automation's `timeout`"),
            TimeoutSetting::Stage(stage) => {
                write!(f, "`session-timeout` in `[stage.{}]`", stage.name())
            }
            TimeoutSetting::Pipeline => f.write_str("`session-timeout`"),
        }
    }
}

/// What one run's sessions run with.
#[derive(Clone, Debug)]
pub struct SessionSettings {
    /// `None` is the harness's choice.
    pub model: Option<Sourced<String>>,
    /// `None` is the harness's choice.
    pub effort: Option<Sourced<Effort>>,
    pub session_timeout: Sourced<Duration>,
    pub timeout_setting: TimeoutSetting,
    /// The setup command, whose time counts against `session_timeout`.
    pub setup: Sourced<Setup>,
    pub environment: Arc<Given>,
    pub env_layers: EnvLayers,
}

impl SessionSettings {
    /// The model set, `None` leaving it to the harness.
    pub fn model_set(&self) -> Option<String> {
        self.model.as_ref().map(|model| model.value.clone())
    }

    /// The effort set, `None` leaving it to the harness.
    pub fn effort_set(&self) -> Option<Effort> {
        self.effort.as_ref().map(|effort| effort.value)
    }

    /// Where each setting came from.
    pub fn sources(&self) -> BTreeMap<Setting, Source> {
        let set_or_harness = |layer: Option<Layer>| layer.map_or(Source::Harness, Source::Layer);
        BTreeMap::from([
            (
                Setting::Model,
                set_or_harness(self.model.as_ref().map(|model| model.layer)),
            ),
            (
                Setting::Effort,
                set_or_harness(self.effort.as_ref().map(|effort| effort.layer)),
            ),
            (
                Setting::SessionTimeout,
                Source::Layer(self.session_timeout.layer),
            ),
            (Setting::Setup, Source::Layer(self.setup.layer)),
            (Setting::SessionEnv, Source::Layer(self.env_layers.set)),
            (Setting::SessionPassEnv, Source::Layer(self.env_layers.pass)),
        ])
    }
}

/// One layer's settings for a run.
#[derive(Clone, Copy)]
struct Rung<'a> {
    settings: &'a Partial,
    layer: Layer,
    /// What a timeout from here is.
    timeout_setting: TimeoutSetting,
}

impl<'a> Rung<'a> {
    fn new(settings: &'a Partial, layer: Layer, timeout_setting: TimeoutSetting) -> Rung<'a> {
        Rung {
            settings,
            layer,
            timeout_setting,
        }
    }
}

/// The first value `get` finds on `ladder`, and the rung it found it on.
fn first<'a, T>(
    ladder: &[Rung<'a>],
    get: impl Fn(&'a Partial) -> Option<T>,
) -> Option<(T, Rung<'a>)> {
    ladder
        .iter()
        .find_map(|&rung| Some((get(rung.settings)?, rung)))
}

impl WorkSettings {
    /// A setting takes the first value set among: the automation's front
    /// matter, the repository's stage table, the operator's stage table,
    /// the flags and environment, the repository's file, the operator's
    /// file, and the default.
    pub fn for_run(
        &self,
        of: RunKind,
        repo: &RepoSettings,
        environments: &Environments,
    ) -> SessionSettings {
        let none = Partial::default();
        let (front_matter, stage) = match of {
            RunKind::Automation(automation) => (
                Partial {
                    model: automation.model.clone(),
                    effort: automation.effort,
                    session_timeout: automation.timeout,
                    setup: automation.setup.clone(),
                },
                None,
            ),
            RunKind::Stage(stage) => (Partial::default(), Some(stage)),
        };
        let table = stage.and_then(|stage| self.stages.get(&stage));
        let stage_setting = stage.map_or(TimeoutSetting::Pipeline, TimeoutSetting::Stage);
        let ladder = [
            Rung::new(&front_matter, Layer::Repository, TimeoutSetting::Automation),
            Rung::new(
                stage
                    .and_then(|stage| repo.stages.get(&stage))
                    .unwrap_or(&none),
                Layer::Repository,
                stage_setting,
            ),
            Rung::new(
                table.map_or(&none, |table| &table.settings),
                Layer::Operator,
                stage_setting,
            ),
            Rung::new(
                &self.flags_and_env,
                Layer::Operator,
                TimeoutSetting::Pipeline,
            ),
            Rung::new(&repo.top, Layer::Repository, TimeoutSetting::Pipeline),
            Rung::new(&self.file, Layer::Operator, TimeoutSetting::Pipeline),
        ];
        let model = first(&ladder, |partial| partial.model.clone())
            .map(|(model, rung)| Sourced::new(model, rung.layer));
        let effort = first(&ladder, |partial| partial.effort)
            .map(|(effort, rung)| Sourced::new(effort, rung.layer));
        let (session_timeout, timeout_setting) =
            match first(&ladder, |partial| partial.session_timeout) {
                Some((timeout, rung)) => (Sourced::new(timeout, rung.layer), rung.timeout_setting),
                None => (
                    Sourced::new(SESSION_TIMEOUT, Layer::Linewright),
                    TimeoutSetting::Pipeline,
                ),
            };
        let setup = first(&ladder, |partial| partial.setup.clone()).map_or(
            Sourced::new(Setup::Empty, Layer::Linewright),
            |(setup, rung)| Sourced::new(setup, rung.layer),
        );
        let staged_environment = stage.and_then(|stage| environments.stages.get(&stage));
        SessionSettings {
            model,
            effort,
            session_timeout,
            timeout_setting,
            setup,
            environment: staged_environment.unwrap_or(&environments.pipeline).clone(),
            env_layers: self.env_layers.with(table),
        }
    }

    /// Splits `environment` by what each stage's sessions are given: the
    /// whole pipeline's `session_env`, and what its table adds.
    /// `set_by_linewright` names the variables Linewright sets for sessions
    /// itself, which are not withheld.
    pub fn environments(
        &self,
        session_env: &SessionEnv,
        environment: impl IntoIterator<Item = (OsString, OsString)>,
        set_by_linewright: &[&str],
    ) -> Split {
        let environment: Vec<(OsString, OsString)> = environment.into_iter().collect();
        let pipeline = session_env.split(environment.iter().cloned(), set_by_linewright);
        let mut stages = BTreeMap::new();
        let mut added = BTreeMap::new();
        let set = session_env.set_names();
        let mut stages_set = BTreeMap::new();
        for (&stage, table) in &self.stages {
            let Some(more) = table.session_environment() else {
                continue;
            };
            if table.session_env.is_some() {
                let own = more.set_names();
                stages_set.insert(
                    stage,
                    StageSet {
                        taken: set
                            .iter()
                            .filter(|name| !own.contains(name))
                            .cloned()
                            .collect(),
                        own,
                    },
                );
            }
            let split = session_env
                .and(&more)
                .split(environment.iter().cloned(), set_by_linewright);
            added.insert(
                stage,
                pipeline
                    .withheld
                    .iter()
                    .filter(|name| !split.withheld.contains(name))
                    .cloned()
                    .collect(),
            );
            stages.insert(stage, Arc::new(split.given));
        }
        Split {
            environments: Environments {
                pipeline: Arc::new(pipeline.given),
                stages,
            },
            withheld: pipeline.withheld,
            added,
            set,
            stages_set,
        }
    }
}
