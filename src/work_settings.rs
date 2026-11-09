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
pub const SESSION_TIMEOUT: Duration = Duration::from_hours(6);

/// `setup.max-age`'s default.
pub const SETUP_MAX_AGE: Duration = Duration::from_hours(24);

/// `setup.timeout`'s default.
pub const SETUP_TIMEOUT: Duration = Duration::from_mins(60);

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
pub enum StageSetting {
    Repository(RepositorySetting),
    SessionEnv,
    SessionPassEnv,
}

/// The settings a run's sessions run with: those a stage can set, and
/// `maintenance`, which only the whole pipeline sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Setting {
    Stage(StageSetting),
    Maintenance,
}

/// The session settings of the repository's file, at its top level and in
/// its stages' tables (`docs/specs/operation.md`, The repository's file),
/// and of an automation's front matter (`docs/specs/automations.md`,
/// Session settings).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepositorySetting {
    Model,
    Effort,
    SessionTimeout,
}

impl RepositorySetting {
    pub const ALL: [RepositorySetting; 3] = [
        RepositorySetting::Model,
        RepositorySetting::Effort,
        RepositorySetting::SessionTimeout,
    ];

    /// Its key in a config file and in front matter.
    pub const fn name(self) -> &'static str {
        match self {
            RepositorySetting::Model => "model",
            RepositorySetting::Effort => "effort",
            RepositorySetting::SessionTimeout => "session-timeout",
        }
    }

    /// The setting whose key is `key`, if it is one of these.
    pub fn named(key: &str) -> Option<RepositorySetting> {
        RepositorySetting::ALL
            .into_iter()
            .find(|setting| setting.name() == key)
    }

    /// Their keys, as a list in an error: "`model`, `effort`, …".
    pub fn keys() -> String {
        format!(
            "`{}`",
            RepositorySetting::ALL
                .map(RepositorySetting::name)
                .join("`, `")
        )
    }
}

impl StageSetting {
    pub const ALL: [StageSetting; 5] = [
        StageSetting::Repository(RepositorySetting::Model),
        StageSetting::Repository(RepositorySetting::Effort),
        StageSetting::Repository(RepositorySetting::SessionTimeout),
        StageSetting::SessionEnv,
        StageSetting::SessionPassEnv,
    ];

    /// Its key in a config file.
    pub const fn name(self) -> &'static str {
        match self {
            StageSetting::Repository(setting) => setting.name(),
            StageSetting::SessionEnv => "session-env",
            StageSetting::SessionPassEnv => "session-pass-env",
        }
    }

    /// The setting whose key is `key`, if it is one of these.
    pub fn named(key: &str) -> Option<StageSetting> {
        StageSetting::ALL
            .into_iter()
            .find(|setting| setting.name() == key)
    }
}

impl Setting {
    pub const MODEL: Setting = Setting::Stage(StageSetting::Repository(RepositorySetting::Model));
    pub const EFFORT: Setting = Setting::Stage(StageSetting::Repository(RepositorySetting::Effort));
    pub const SESSION_TIMEOUT: Setting =
        Setting::Stage(StageSetting::Repository(RepositorySetting::SessionTimeout));
    pub const SESSION_ENV: Setting = Setting::Stage(StageSetting::SessionEnv);
    pub const SESSION_PASS_ENV: Setting = Setting::Stage(StageSetting::SessionPassEnv);

    /// Its key in a config file.
    pub const fn name(self) -> &'static str {
        match self {
            Setting::Stage(setting) => setting.name(),
            Setting::Maintenance => MAINTENANCE,
        }
    }
}

/// The key of the `maintenance` setting.
pub const MAINTENANCE: &str = "maintenance";

/// The key of the `setup` table, and `maintenance`'s other name: a run's
/// record may name `maintenance` so.
pub const SETUP: &str = "setup";

/// A run's record may name `maintenance` as [`SETUP`].
impl std::str::FromStr for Setting {
    type Err = String;

    fn from_str(name: &str) -> Result<Setting, String> {
        match name {
            MAINTENANCE | SETUP => Ok(Setting::Maintenance),
            name => StageSetting::named(name)
                .map(Setting::Stage)
                .ok_or_else(|| format!("{name:?} is not a session's setting")),
        }
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

/// The `maintenance` setting: the maintenance command each session's clone
/// is made ready with (`docs/specs/runs.md`, Maintenance).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Maintenance {
    /// Runs nothing. Set in a layer, it wins over a command set below it.
    Empty,
    Command(MaintenanceCommand),
}

/// A maintenance command, never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaintenanceCommand(String);

impl MaintenanceCommand {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An empty string is [`Maintenance::Empty`].
impl From<String> for Maintenance {
    fn from(text: String) -> Maintenance {
        if text.is_empty() {
            Maintenance::Empty
        } else {
            Maintenance::Command(MaintenanceCommand(text))
        }
    }
}

/// The `setup` table's `command`: the setup command the environment is
/// built with (`docs/specs/sessions.md`, Environment).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Setup {
    /// Builds no environment. Set in a layer, it wins over a command set
    /// below it.
    Empty,
    Command(SetupCommand),
}

/// A setup command, never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupCommand(String);

impl SetupCommand {
    /// `None` for an empty `text`.
    pub fn new(text: String) -> Option<SetupCommand> {
        (!text.is_empty()).then_some(SetupCommand(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An empty string is [`Setup::Empty`].
impl From<String> for Setup {
    fn from(text: String) -> Setup {
        SetupCommand::new(text).map_or(Setup::Empty, Setup::Command)
    }
}

/// The `setup` table one layer sets, `None` where it sets a key not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetupPartial {
    pub command: Option<Setup>,
    pub max_age: Option<Duration>,
    pub timeout: Option<Duration>,
}

/// What the environment is built with, when there is a setup command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupSettings {
    pub command: SetupCommand,
    /// The environment grown older than this is built again.
    pub max_age: Duration,
    /// A build still running after this long fails.
    pub timeout: Duration,
}

/// The session settings one layer sets, `None` where it sets none: those a
/// stage's table and an automation's front matter can set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Partial {
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub session_timeout: Option<Duration>,
}

/// The whole-pipeline settings one layer sets, `None` where it sets none:
/// those no stage's table or front matter can set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PipelineSettings {
    pub maintenance: Option<Maintenance>,
    pub setup: SetupPartial,
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

/// The operator's settings for sessions, each of its layers kept apart.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WorkSettings {
    pub flags_and_env: Partial,
    /// The operator's config file, but for the settings `flags_and_env`
    /// sets.
    pub file: Partial,
    /// The whole-pipeline settings the flags and environment variables set.
    pub pipeline_flags_and_env: PipelineSettings,
    /// The whole-pipeline settings the operator's config file sets, but for
    /// those `pipeline_flags_and_env` sets.
    pub pipeline_file: PipelineSettings,
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
    pipeline: PipelineSettings,
    stages: BTreeMap<WorkKind, Partial>,
}

impl RepoSettings {
    pub(crate) fn new(
        top: Partial,
        pipeline: PipelineSettings,
        stages: BTreeMap<WorkKind, Partial>,
    ) -> RepoSettings {
        RepoSettings {
            top,
            pipeline,
            stages,
        }
    }
}

/// What a run is, as far as its settings go.
#[derive(Clone, Copy, Debug)]
pub enum RunKind<'a> {
    Stage(WorkKind),
    Automation(&'a Automation),
}

/// The environment each session is given.
#[derive(Clone, Debug, Default)]
pub struct Environments {
    /// The whole pipeline's, which a build of the environment is given too.
    pub pipeline: Arc<Given>,
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

/// The setting a session's timeout came from, the one that would give the
/// next run longer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeoutSetting {
    /// `session-timeout` in the automation's front matter.
    Automation,
    /// `session-timeout` in the stage's table.
    Stage(WorkKind),
    /// The whole pipeline's `session-timeout`.
    Pipeline,
}

impl fmt::Display for TimeoutSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimeoutSetting::Automation => {
                f.write_str("`session-timeout` in the automation's front matter")
            }
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
    /// The maintenance command, which runs within its session's timeout,
    /// `session_timeout`, having none of its own.
    pub maintenance: Sourced<Maintenance>,
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
                Setting::MODEL,
                set_or_harness(self.model.as_ref().map(|model| model.layer)),
            ),
            (
                Setting::EFFORT,
                set_or_harness(self.effort.as_ref().map(|effort| effort.layer)),
            ),
            (
                Setting::SESSION_TIMEOUT,
                Source::Layer(self.session_timeout.layer),
            ),
            (Setting::Maintenance, Source::Layer(self.maintenance.layer)),
            (Setting::SESSION_ENV, Source::Layer(self.env_layers.set)),
            (
                Setting::SESSION_PASS_ENV,
                Source::Layer(self.env_layers.pass),
            ),
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
    /// A session setting takes the first value set among: the automation's
    /// front matter, the repository's stage table, the operator's stage
    /// table, the flags and environment, the repository's file, the
    /// operator's file, and the default. `maintenance` takes the first
    /// among the flags and environment, the repository's file, the
    /// operator's file, and the default.
    pub fn for_run(
        &self,
        of: RunKind,
        repo: &RepoSettings,
        environments: &Environments,
    ) -> SessionSettings {
        let none = Partial::default();
        let (front_matter, stage) = match of {
            RunKind::Automation(automation) => (&automation.session, None),
            RunKind::Stage(stage) => (&none, Some(stage)),
        };
        let table = stage.and_then(|stage| self.stages.get(&stage));
        let stage_setting = stage.map_or(TimeoutSetting::Pipeline, TimeoutSetting::Stage);
        let ladder = [
            Rung::new(front_matter, Layer::Repository, TimeoutSetting::Automation),
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
        let maintenance = [
            (&self.pipeline_flags_and_env, Layer::Operator),
            (&repo.pipeline, Layer::Repository),
            (&self.pipeline_file, Layer::Operator),
        ]
        .into_iter()
        .find_map(|(pipeline, layer)| Some(Sourced::new(pipeline.maintenance.clone()?, layer)))
        .unwrap_or(Sourced::new(Maintenance::Empty, Layer::Linewright));
        let staged_environment = stage.and_then(|stage| environments.stages.get(&stage));
        SessionSettings {
            model,
            effort,
            session_timeout,
            timeout_setting,
            maintenance,
            environment: staged_environment.unwrap_or(&environments.pipeline).clone(),
            env_layers: self.env_layers.with(table),
        }
    }

    /// The `setup` table, each key from the first layer that sets it: the
    /// flags and environment, the repository's file, the operator's file,
    /// and the default. `None` without a setup command.
    pub fn setup(&self, repo: &RepoSettings) -> Option<SetupSettings> {
        let layers = [
            &self.pipeline_flags_and_env.setup,
            &repo.pipeline.setup,
            &self.pipeline_file.setup,
        ];
        let first = |get: fn(&SetupPartial) -> Option<Duration>, default| {
            layers
                .iter()
                .find_map(|layer| get(layer))
                .unwrap_or(default)
        };
        let Setup::Command(command) = layers.iter().find_map(|layer| layer.command.clone())? else {
            return None;
        };
        Some(SetupSettings {
            command,
            max_age: first(|layer| layer.max_age, SETUP_MAX_AGE),
            timeout: first(|layer| layer.timeout, SETUP_TIMEOUT),
        })
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
