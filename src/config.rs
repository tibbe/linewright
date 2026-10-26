use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::TimeDelta;
use clap::{Args, Parser, Subcommand};

use crate::automation::AutomationName;
use crate::domain::{AutoMerge, MergeMethod, Number, Repo, Timestamp, UpdateMethod};
use crate::drain::Timeout;
use crate::host::UiHosts;
use crate::layer::Layer;
use crate::logs::Which;
use crate::reconcile::WorkKind;
use crate::runs::{RepoWorkDir, RunId};
use crate::secrets;
use crate::session_env::SessionEnv;
use crate::spec_glob::SpecGlob;
use crate::variable_names::{self, VariableNames};
use crate::web;
use crate::webhook::{self, Webhooks};
use crate::work_settings::{Effort, Partial, Setting, StageTable, WorkSettings};

#[derive(Parser, Debug)]
#[command(
    name = crate::COMMAND,
    version = crate::VERSION_AND_COMMIT,
    about = "A spec-driven software factory"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
    #[command(flatten)]
    pub settings: Settings,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Poll the repository until drained or stopped.
    Serve,
    /// Run one poll cycle and the runs it starts, then exit.
    Once,
    /// Tell the running `serve` to drain.
    ///
    /// `serve` claims no item and starts no run, and exits once no run is in
    /// flight. Prints the runs it waits for and each as it ends, and returns
    /// once `serve` has exited.
    Drain {
        /// Return once `serve` has taken the request.
        #[arg(long)]
        no_wait: bool,
        /// After this long, such as 2h, the drain becomes a stop: the runs in
        /// flight are cancelled, keeping their unfinished work. Replaces the
        /// deadline of a drain already under way.
        #[arg(long, value_name = "DURATION", value_parser = Timeout::parse)]
        timeout: Option<Timeout>,
        /// Print one JSON object per line.
        #[arg(long)]
        json: bool,
    },
    /// Show what the running `serve` is doing and what holds it back.
    Status {
        /// Print `serve`'s answer as JSON, as it sent it.
        #[arg(long)]
        json: bool,
    },
    /// List the runs, newest first: all of them, or issue or pull request
    /// #N's.
    Runs {
        /// Only the runs on issue or pull request #N.
        #[arg(value_name = "N")]
        item: Option<u64>,
        /// Only the runs of this stage: triage, design, implement, rework,
        /// fix-ci or update.
        #[arg(long)]
        stage: Option<WorkKind>,
        /// Only the runs started since this time, as RFC 3339
        /// (2026-10-14T12:00:00Z) or a date (2026-10-14) in UTC, or this
        /// long ago (2h).
        #[arg(long, value_parser = parse_since)]
        since: Option<Since>,
        /// At most this many runs, the newest.
        #[arg(long)]
        limit: Option<usize>,
        /// Print the answer as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show where issue or pull request #N stands and what Linewright waits
    /// for before it acts on it next.
    Explain {
        #[arg(value_name = "N")]
        number: u64,
        /// Print `serve`'s answer as JSON, as it sent it.
        #[arg(long)]
        json: bool,
    },
    /// Show the transcript of issue or pull request #N's run in flight, or
    /// else of its latest run that started a session.
    Logs {
        #[command(flatten)]
        which: Which,
        /// Show everything the transcript holds: also the thinking, each
        /// tool's full input and result, the brief and follow-ups, and the
        /// harness's other events.
        #[arg(long)]
        verbose: bool,
        /// Print one JSON object per transcript entry and line.
        #[arg(long)]
        json: bool,
        /// Show the transcript so far, then each entry as it happens, until
        /// the run ends.
        #[arg(long)]
        follow: bool,
    },
    /// Automations: `automation run NAME` runs one now, then exits.
    Automation {
        #[command(subcommand)]
        command: AutomationCommand,
    },
    /// Write a case from a spec change or issue on GitHub: `owner/name#N`.
    Capture { target: String, dir: PathBuf },
    /// Run a case through the whole pipeline against a local store.
    Replay {
        case: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Since {
    At(Timestamp),
    Ago(Duration),
}

impl Since {
    pub fn at(self, now: Timestamp) -> Timestamp {
        match self {
            Since::At(at) => at,
            Since::Ago(ago) => TimeDelta::from_std(ago)
                .ok()
                .and_then(|ago| now.checked_sub_signed(ago))
                .unwrap_or(Timestamp::MIN_UTC),
        }
    }
}

pub(crate) fn parse_since(value: &str) -> Result<Since, String> {
    if let Ok(at) = chrono::DateTime::parse_from_rfc3339(value) {
        return Ok(Since::At(at.to_utc()));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(Since::At(date.and_time(chrono::NaiveTime::MIN).and_utc()));
    }
    humantime::parse_duration(value).map(Since::Ago).map_err(|_| {
        format!("{value:?} is not a time (2026-10-14T12:00:00Z), a date (2026-10-14) or a duration (2h)")
    })
}

/// The arguments of `logs` that say which run to show: an item, a run ID,
/// or both.
#[derive(Args, Debug)]
#[group(required = true, multiple = true)]
struct WhichArgs {
    /// The run's issue or pull request.
    #[arg(value_name = "N")]
    item: Option<u64>,
    /// Another run, by its run ID, such as 42/rework-2 or weekly/3.
    #[arg(long, value_name = "ID")]
    run: Option<RunId>,
}

impl clap::FromArgMatches for Which {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Which, clap::Error> {
        let args = WhichArgs::from_arg_matches(matches)?;
        Which::new(args.item.map(Number), args.run)
            .map_err(|e| clap::Error::raw(clap::error::ErrorKind::MissingRequiredArgument, e))
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Which::from_arg_matches(matches)?;
        Ok(())
    }
}

impl Args for Which {
    fn augment_args(command: clap::Command) -> clap::Command {
        WhichArgs::augment_args(command)
    }

    fn augment_args_for_update(command: clap::Command) -> clap::Command {
        WhichArgs::augment_args_for_update(command)
    }
}

#[derive(Subcommand, Debug)]
pub enum AutomationCommand {
    /// Run the automation in `.linewright/automations/NAME.md` on the
    /// default branch once, now, then exit; its schedule stays as it was.
    Run {
        #[arg(value_name = "NAME")]
        name: AutomationName,
    },
}

/// The settings as flags. Each is a string here and parsed once, after the
/// layers are merged.
#[derive(Args, Debug, Default)]
pub struct Settings {
    /// The TOML config file [default: linewright.toml in the working directory]
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// The repository, `owner/name` (required)
    #[arg(long, global = true)]
    pub repo: Option<String>,
    /// How often to poll [default: 60s]
    #[arg(long, global = true)]
    pub poll_interval: Option<String>,
    /// The host and port `serve` listens on for webhook deliveries
    /// [default: 0.0.0.0:8080 with `LINEWRIGHT_WEBHOOK_SECRET` set, none
    /// without it]
    #[arg(long, global = true)]
    pub webhook_address: Option<String>,
    /// The host and port `serve` serves the web UI on; empty for none
    /// [default: 127.0.0.1:8081]
    #[arg(long, global = true)]
    pub ui_address: Option<String>,
    /// The host names the web UI answers besides the local ones and IP
    /// addresses, comma-separated [default: none]
    #[arg(long, global = true, value_name = "NAMES")]
    pub ui_hosts: Option<String>,
    /// The tracked Markdown files that are the spec [default: docs/**/*.md]
    #[arg(long, global = true)]
    pub spec_glob: Option<String>,
    /// How far back merged spec changes are picked up [default: 7d]
    #[arg(long, global = true)]
    pub spec_lookback: Option<String>,
    /// The ceiling on runs in flight [default: 4]
    #[arg(long, global = true)]
    pub max_runs: Option<String>,
    /// Where clones and session transcripts go [default: linewright/ under the user's data directory]
    #[arg(long, global = true)]
    pub work_dir: Option<String>,
    /// The model for every stage; a stage's table in the config file,
    /// `[stage.NAME]`, can set its own [default: the harness's]
    #[arg(long, global = true)]
    pub model: Option<String>,
    /// How hard the agent thinks: `low`, `medium`, `high`, `xhigh` or `max`,
    /// for every stage; a stage's table in the config file, `[stage.NAME]`,
    /// can set its own [default: the harness's]
    #[arg(long, global = true)]
    pub effort: Option<String>,
    /// A directory laid out as the briefs Linewright ships, each file in it
    /// taken in place of Linewright's own [default: none]
    #[arg(long, global = true)]
    pub briefs: Option<String>,
    /// The variables of Linewright's environment that agent sessions are given
    /// besides the ones they always get, comma-separated; a name ending in `*`
    /// names every variable with that prefix; a stage's table in the config
    /// file, `[stage.NAME]`, can add names for its sessions [default: none]
    #[arg(long, global = true, value_name = "NAMES")]
    pub session_env: Option<String>,
    /// The variables of Linewright's environment whose values are secrets,
    /// redacted and refused in commits, besides the ones Linewright reads a
    /// credential from, comma-separated; a name ending in `*` names every
    /// variable with that prefix [default: none]
    #[arg(long, global = true, value_name = "NAMES")]
    pub secrets: Option<String>,
    /// Each agent session's timeout; a stage's table in the config file,
    /// `[stage.NAME]`, can set its own [default: 60m]
    #[arg(long, global = true)]
    pub session_timeout: Option<String>,
    /// How often an unparseable result goes back to its session [default: 1]
    #[arg(long, global = true)]
    pub result_retries: Option<String>,
    /// How long to wait for CI to appear [default: 5m]
    #[arg(long, global = true)]
    pub ci_grace: Option<String>,
    /// How long to wait for CI to complete [default: 6h]
    #[arg(long, global = true)]
    pub ci_timeout: Option<String>,
    /// CI fix attempts before handing back [default: 2]
    #[arg(long, global = true)]
    pub ci_attempts: Option<String>,
    /// `rebase` or `merge` [default: from the repository's settings]
    #[arg(long, global = true)]
    pub update_branch: Option<String>,
    /// Merge green pull requests instead of leaving them for review [default: off]
    #[arg(long, global = true, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    pub auto_merge: Option<String>,
    /// `merge`, `squash` or `rebase` [default: from the repository's settings]
    #[arg(long, global = true)]
    pub merge_method: Option<String>,
    /// Start work only on issues carrying this label [default: every issue]
    #[arg(long, global = true)]
    pub require_label: Option<String>,
    /// `text` or `json` [default: text on a terminal, json otherwise]
    #[arg(long, global = true)]
    pub log_format: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFormat {
    Text,
    Json,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub repo: Option<String>,
    pub poll_interval: Duration,
    pub webhooks: Option<Webhooks>,
    /// `None` when `ui-address` is empty.
    pub ui: Option<web::Ui>,
    pub spec_glob: SpecGlob,
    pub spec_lookback: TimeDelta,
    pub max_runs: usize,
    pub work_dir: PathBuf,
    /// `model`, `effort`, `session-timeout` and the stages' tables, and
    /// where `session-env` came from.
    pub work: WorkSettings,
    pub briefs: Option<PathBuf>,
    pub session_env: SessionEnv,
    pub secrets: VariableNames,
    pub result_retries: u32,
    pub ci_grace: TimeDelta,
    pub ci_timeout: TimeDelta,
    pub ci_attempts: u32,
    pub update_branch: Option<UpdateMethod>,
    pub auto_merge: Option<AutoMerge>,
    pub require_label: Option<String>,
    pub log_format: LogFormat,
}

impl Settings {
    fn flags(&self) -> BTreeMap<&'static str, &String> {
        [
            ("repo", &self.repo),
            ("poll-interval", &self.poll_interval),
            ("webhook-address", &self.webhook_address),
            ("ui-address", &self.ui_address),
            ("ui-hosts", &self.ui_hosts),
            ("spec-glob", &self.spec_glob),
            ("spec-lookback", &self.spec_lookback),
            ("max-runs", &self.max_runs),
            ("work-dir", &self.work_dir),
            ("model", &self.model),
            ("effort", &self.effort),
            ("briefs", &self.briefs),
            ("session-env", &self.session_env),
            ("secrets", &self.secrets),
            ("session-timeout", &self.session_timeout),
            ("result-retries", &self.result_retries),
            ("ci-grace", &self.ci_grace),
            ("ci-timeout", &self.ci_timeout),
            ("ci-attempts", &self.ci_attempts),
            ("update-branch", &self.update_branch),
            ("auto-merge", &self.auto_merge),
            ("merge-method", &self.merge_method),
            ("require-label", &self.require_label),
            ("log-format", &self.log_format),
        ]
        .into_iter()
        .filter_map(|(key, value)| Some((key, value.as_ref()?)))
        .collect()
    }
}

/// The settings that are lists, and what each holds, which may be an array
/// in the file.
const LISTS: [(&str, &str); 3] = [
    ("session-env", "a variable's name"),
    ("secrets", "a variable's name"),
    ("ui-hosts", "a host name"),
];

const KEYS: [&str; 24] = [
    "repo",
    "poll-interval",
    "webhook-address",
    "ui-address",
    "ui-hosts",
    "spec-glob",
    "spec-lookback",
    "max-runs",
    "work-dir",
    "model",
    "effort",
    "briefs",
    "session-env",
    "secrets",
    "session-timeout",
    "result-retries",
    "ci-grace",
    "ci-timeout",
    "ci-attempts",
    "update-branch",
    "auto-merge",
    "merge-method",
    "require-label",
    "log-format",
];

/// A feature and what turns it on or off: a setting only it uses is an
/// error while it is off unless the setting is at its default
/// (`docs/specs/operation.md`, Configuration, A feature's settings).
struct Feature {
    setting: &'static str,
    /// The settings that turn it on or off, which an error is blamed on.
    switches: &'static [&'static str],
    /// What is off and how to turn it on, after "is set, but".
    when_off: fn() -> String,
}

struct Features {
    webhooks: Feature,
    ui: Feature,
    auto_merge: Feature,
}

const FEATURES: Features = Features {
    // Turned on by the environment's secret, which no setting holds.
    webhooks: Feature {
        setting: "webhook-address",
        switches: &[],
        when_off: || {
            format!(
                "`{}` is not: set it to the webhook's secret to turn webhooks on",
                webhook::SECRET_VARIABLE
            )
        },
    },
    ui: Feature {
        setting: "ui-hosts",
        switches: &["ui-address"],
        when_off: || {
            "`ui-address` is empty: set `ui-address` to serve the web UI, or unset `ui-hosts`"
                .into()
        },
    },
    auto_merge: Feature {
        setting: "merge-method",
        switches: &["auto-merge"],
        when_off: || "`auto-merge` is off: turn `auto-merge` on, or unset `merge-method`".into(),
    },
};

/// `setting`, `feature`'s setting parsed with `None` at its default, unless
/// it is not at its default while `feature` is off.
fn refuse_while_off<T>(
    feature: &Feature,
    on: bool,
    setting: Option<T>,
    blame: impl Fn(&[&str], anyhow::Error) -> ResolveError,
) -> Result<Option<T>, ResolveError> {
    match setting {
        Some(_) if !on => Err(blame(
            feature.switches,
            anyhow!(
                "the setting `{}` is set, but {}",
                feature.setting,
                (feature.when_off)()
            ),
        )),
        setting => Ok(setting),
    }
}

/// `webhook-address`'s default with the webhook secret set.
const WEBHOOK_ADDRESS: &str = "0.0.0.0:8080";

impl Config {
    pub fn required_repo(&self) -> Result<Repo> {
        self.repo
            .as_deref()
            .context("set `repo` (owner/name)")?
            .parse()
    }

    pub fn repo_work_dir(&self) -> Result<RepoWorkDir> {
        Ok(RepoWorkDir {
            base: self.work_dir.clone(),
            repo: self.required_repo()?,
        })
    }

    pub fn resolve(
        settings: &Settings,
        cwd: &Path,
        env: &dyn Fn(&str) -> Option<String>,
        on_terminal: bool,
    ) -> Result<Config, ResolveError> {
        let FileSettings {
            path,
            top: file,
            stages,
        } = read_file(settings.config.as_deref(), cwd)?;
        let flags = settings.flags();
        // A flag, then a variable.
        let flags_and_env = |key: &str| -> Option<String> {
            let variable = format!("LINEWRIGHT_{}", key.to_uppercase().replace('-', "_"));
            flags.get(key).copied().cloned().or_else(|| env(&variable))
        };
        let get =
            |key: &str| -> Option<String> { flags_and_env(key).or_else(|| file.get(key).cloned()) };
        let blame = |keys: &[&str], error: anyhow::Error| {
            if keys.iter().any(|key| flags.contains_key(key)) {
                ResolveError::CommandLine(error)
            } else {
                ResolveError::Elsewhere(error)
            }
        };
        // A wrong value the file gave names the file.
        let in_the_file = |key: &str, error: anyhow::Error| {
            if flags_and_env(key).is_none() && file.contains_key(key) {
                error.context(path.display().to_string())
            } else {
                error
            }
        };
        let refused = |key: &str, error: anyhow::Error| blame(&[key], in_the_file(key, error));
        let not_one = |key: &str, value: &str, error: anyhow::Error| {
            refused(
                key,
                error.context(format!("the setting `{key}` is {value:?}")),
            )
        };
        let wrong = |key: &str, error: String| refused(key, anyhow!(error));
        let value = |key: &str, default: &str| get(key).unwrap_or_else(|| default.to_string());
        let duration = |key: &str, default: &str| {
            let value = value(key, default);
            humantime::parse_duration(&value).map_err(|e| not_one(key, &value, e.into()))
        };
        let delta = |key: &str, default: &str| -> Result<TimeDelta, ResolveError> {
            TimeDelta::from_std(duration(key, default)?).map_err(|e| blame(&[key], e.into()))
        };
        let number = |key: &str, default: &str| {
            let value = value(key, default);
            value
                .parse::<u32>()
                .map_err(|e| not_one(key, &value, e.into()))
        };
        let glob = value("spec-glob", "docs/**/*.md");
        let spec_glob = SpecGlob::new(&glob).map_err(|e| not_one("spec-glob", &glob, e.into()))?;
        let update_branch = match get("update-branch").as_deref() {
            None => None,
            Some("rebase") => Some(UpdateMethod::Rebase),
            Some("merge") => Some(UpdateMethod::Merge),
            Some(other) => {
                return Err(wrong(
                    "update-branch",
                    format!("the setting `update-branch` is {other:?}: it is `rebase` or `merge`"),
                ));
            }
        };
        let auto_merge = match get("auto-merge").as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            Some(other) => {
                return Err(wrong(
                    "auto-merge",
                    format!("the setting `auto-merge` is {other:?}: it is `true` or `false`"),
                ));
            }
        };
        let parse_merge_method = |name: String| {
            name.parse::<MergeMethod>().map_err(|()| {
                wrong(
                    "merge-method",
                    format!(
                        "the setting `merge-method` is {name:?}: it is `merge`, `squash` or `rebase`"
                    ),
                )
            })
        };
        // The repository's own setting by default.
        let merge_method = refuse_while_off(
            &FEATURES.auto_merge,
            auto_merge,
            get("merge-method").map(parse_merge_method).transpose()?,
            blame,
        )?;
        let auto_merge = auto_merge.then_some(AutoMerge { merge_method });
        let log_format = match get("log-format").as_deref() {
            None if on_terminal => LogFormat::Text,
            None => LogFormat::Json,
            Some("text") => LogFormat::Text,
            Some("json") => LogFormat::Json,
            Some(other) => {
                return Err(wrong(
                    "log-format",
                    format!("the setting `log-format` is {other:?}: it is `text` or `json`"),
                ));
            }
        };
        let work_dir = match get("work-dir") {
            Some(dir) => PathBuf::from(dir),
            None => dirs::data_dir()
                .ok_or_else(|| {
                    wrong(
                        "work-dir",
                        "no data directory for the default `work-dir`; set it".into(),
                    )
                })?
                .join("linewright"),
        };
        let secret = env(webhook::SECRET_VARIABLE).and_then(webhook::Secret::new);
        let parse_webhook_address = |address: &str| {
            address
                .parse()
                .map_err(|e| not_one("webhook-address", address, e))
        };
        // `WEBHOOK_ADDRESS` by default with the secret set, and none
        // without it, so any address given is not at its default.
        let webhook_address = refuse_while_off(
            &FEATURES.webhooks,
            secret.is_some(),
            get("webhook-address")
                .map(|address| parse_webhook_address(&address))
                .transpose()?,
            blame,
        )?;
        let webhooks = match secret {
            Some(secret) => Some(Webhooks {
                address: match webhook_address {
                    Some(address) => address,
                    None => parse_webhook_address(WEBHOOK_ADDRESS)?,
                },
                secret,
            }),
            None => None,
        };
        let ui_address = value("ui-address", "127.0.0.1:8081");
        // No names is its default, `""` as a flag or variable and `[]` in
        // the file.
        let ui_hosts = refuse_while_off(
            &FEATURES.ui,
            !ui_address.is_empty(),
            get("ui-hosts")
                .filter(|names| !names.is_empty())
                .map(|names| {
                    UiHosts::new(names.split(','))
                        .map_err(|e| wrong("ui-hosts", format!("the setting `ui-hosts` {e}")))
                })
                .transpose()?,
            blame,
        )?;
        let ui = match ui_address.as_str() {
            "" => None,
            address => Some(web::Ui {
                address: address
                    .parse()
                    .map_err(|e| not_one("ui-address", address, e))?,
                hosts: ui_hosts.unwrap_or_default(),
            }),
        };
        let session_env = get("session-env")
            .unwrap_or_default()
            .parse::<SessionEnv>()
            .map_err(|e| wrong("session-env", e))?;
        let secrets = VariableNames::new(
            secrets::SETTING,
            variable_names::comma_separated(&get(secrets::SETTING).unwrap_or_default()),
        )
        .map_err(|e| wrong(secrets::SETTING, e))?;
        let session_timeout = |set: Option<String>| {
            set.map(|text| session_timeout(&text).map_err(|e| refused("session-timeout", e)))
                .transpose()
        };
        let effort = |set: Option<String>| {
            set.map(|text| effort(&text).map_err(|e| refused("effort", e)))
                .transpose()
        };
        // The file's is read only when no flag or variable sets one, as it
        // could not win over them.
        let in_file = |key: &str| match flags_and_env(key) {
            Some(_) => None,
            None => file.get(key).cloned(),
        };
        let work = WorkSettings {
            flags_and_env: Partial {
                model: flags_and_env("model"),
                effort: effort(flags_and_env("effort"))?,
                session_timeout: session_timeout(flags_and_env("session-timeout"))?,
            },
            file: Partial {
                model: file.get("model").cloned(),
                effort: effort(in_file("effort"))?,
                session_timeout: session_timeout(in_file("session-timeout"))?,
            },
            stages,
            session_env: if get("session-env").is_some() {
                Layer::Operator
            } else {
                Layer::Linewright
            },
        };
        let (ci_grace, ci_timeout) = (delta("ci-grace", "5m")?, delta("ci-timeout", "6h")?);
        if ci_timeout <= ci_grace {
            return Err(blame(
                &["ci-grace", "ci-timeout"],
                anyhow!("the setting `ci-timeout` must be longer than `ci-grace`"),
            ));
        }
        Ok(Config {
            repo: get("repo"),
            poll_interval: duration("poll-interval", "60s")?,
            webhooks,
            ui,
            spec_glob,
            spec_lookback: delta("spec-lookback", "7d")?,
            max_runs: number("max-runs", "4")? as usize,
            work_dir,
            work,
            briefs: get("briefs").map(PathBuf::from),
            session_env,
            secrets,
            result_retries: number("result-retries", "1")?,
            ci_grace,
            ci_timeout,
            ci_attempts: number("ci-attempts", "2")?,
            update_branch,
            auto_merge,
            require_label: get("require-label"),
            log_format,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// A flag, or the file `--config` names, which cannot be read.
    #[error("{0:#}")]
    CommandLine(anyhow::Error),
    /// The environment or the config file's contents.
    #[error("{0:#}")]
    Elsewhere(anyhow::Error),
}

/// The operator's config file.
struct FileSettings {
    path: PathBuf,
    /// Each value in the form its flag takes.
    top: BTreeMap<String, String>,
    /// The `[stage.NAME]` tables.
    stages: BTreeMap<WorkKind, StageTable>,
}

fn read_file(explicit: Option<&Path>, cwd: &Path) -> Result<FileSettings, ResolveError> {
    let path = explicit
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cwd.join("linewright.toml"));
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if explicit.is_none() && e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileSettings {
                path,
                top: BTreeMap::new(),
                stages: BTreeMap::new(),
            });
        }
        Err(e) => {
            let error = anyhow!(e).context(format!("reading {}", path.display()));
            return Err(match explicit {
                Some(_) => ResolveError::CommandLine(error),
                None => ResolveError::Elsewhere(error),
            });
        }
    };
    settings_in(&path, &text).map_err(ResolveError::Elsewhere)
}

/// The key of the stages' tables.
const STAGE: &str = "stage";

fn settings_in(path: &Path, text: &str) -> Result<FileSettings> {
    let table: toml::Table = text
        .parse()
        .with_context(|| format!("parsing {}", path.display()))?;
    let mut settings = FileSettings {
        path: path.to_path_buf(),
        top: BTreeMap::new(),
        stages: BTreeMap::new(),
    };
    for (key, value) in table {
        if key == STAGE {
            settings.stages = stage_tables(path, value)?;
            continue;
        }
        if !KEYS.contains(&key.as_str()) {
            bail!("{} has an unknown setting `{key}`", path.display());
        }
        let value = setting_value(&key, value).with_context(|| format!("{}", path.display()))?;
        settings.top.insert(key, value);
    }
    Ok(settings)
}

/// `[stage.NAME]` tables, each holding only [`Setting`]s. An error names
/// `path` and the table.
fn stage_tables(path: &Path, stages: toml::Value) -> Result<BTreeMap<WorkKind, StageTable>> {
    let toml::Value::Table(stages) = stages else {
        bail!(
            "{}: `{STAGE}` is {stages}, not a table of stages such as `[{STAGE}.implement]`",
            path.display()
        );
    };
    stages
        .into_iter()
        .map(|(name, table)| {
            let at = format!("{}: `[{STAGE}.{name}]`", path.display());
            let stage: WorkKind = name.parse().map_err(|e| anyhow!("{at}: {e}"))?;
            let toml::Value::Table(table) = table else {
                bail!("{at} is {table}, not a table");
            };
            let mut parsed = StageTable::default();
            for (key, value) in table {
                let text = setting_value(&key, value).context(at.clone())?;
                let settings = &mut parsed.settings;
                let Ok(setting) = key.parse::<Setting>() else {
                    let keys = Setting::ALL.map(Setting::name);
                    bail!(
                        "{at} has an unknown setting `{key}`: a stage's table holds `{}`",
                        keys.join("`, `")
                    );
                };
                match setting {
                    Setting::Model => settings.model = Some(text),
                    Setting::Effort => settings.effort = Some(effort(&text).context(at.clone())?),
                    Setting::SessionTimeout => {
                        settings.session_timeout =
                            Some(session_timeout(&text).context(at.clone())?);
                    }
                    Setting::SessionEnv => {
                        parsed.session_env =
                            Some(text.parse().map_err(|e: String| anyhow!("{at}: {e}"))?);
                    }
                }
            }
            Ok((stage, parsed))
        })
        .collect()
}

fn effort(text: &str) -> Result<Effort> {
    text.parse().map_err(|e: String| anyhow!("the setting {e}"))
}

fn session_timeout(text: &str) -> Result<Duration> {
    humantime::parse_duration(text)
        .with_context(|| format!("the setting `session-timeout` is {text:?}"))
}

/// `value` in the form `key`'s flag takes.
fn setting_value(key: &str, value: toml::Value) -> Result<String> {
    Ok(match value {
        toml::Value::String(s) => s,
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        // In the form its flag takes, separated by commas.
        toml::Value::Array(entries) if let Some(holds) = list_holds(key) => entries
            .into_iter()
            .map(|entry| match entry.as_str() {
                Some(entry) if !entry.is_empty() && !entry.contains(',') => Ok(entry.to_string()),
                Some(_) | None => bail!("the setting `{key}` holds {entry}, not {holds}"),
            })
            .collect::<Result<Vec<_>>>()?
            .join(","),
        other @ (toml::Value::Float(_)
        | toml::Value::Datetime(_)
        | toml::Value::Array(_)
        | toml::Value::Table(_)) => {
            bail!("the setting `{key}` is {other}, not a string, number or boolean")
        }
    })
}

/// What the list setting `key` holds, `None` when it is not a list.
fn list_holds(key: &str) -> Option<&'static str> {
    LISTS
        .iter()
        .find_map(|&(list, holds)| (list == key).then_some(holds))
}
