use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::TimeDelta;
use clap::{Args, Parser, Subcommand};

use crate::automation::AutomationName;
use crate::domain::{MergeMethod, Number, Timestamp, UpdateMethod};
use crate::logs::Which;
use crate::reconcile::WorkKind;
use crate::runs::{RepoWorkDir, RunId};
use crate::session_env::SessionEnv;
use crate::spec_glob::SpecGlob;
use crate::webhook::{self, Webhooks};

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
    /// Poll the repository until stopped.
    Serve,
    /// Run one poll cycle and the runs it starts, then exit.
    Once,
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
        /// Only the runs of this stage: triage, tickets, implement, rework,
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
        /// runtime's other events.
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
    /// Workflows: `workflow run NAME` runs one now, then exits.
    #[command(name = "workflow")]
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

fn parse_since(value: &str) -> Result<Since, String> {
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
    /// Run the workflow in `.linewright/workflows/NAME.md` on the default
    /// branch once, now, then exit; its schedule stays as it was.
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
    /// The host and port `serve` listens on for webhook deliveries, with
    /// `LINEWRIGHT_WEBHOOK_SECRET` set [default: 0.0.0.0:8080]
    #[arg(long, global = true)]
    pub webhook_address: Option<String>,
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
    /// The model for every stage [default: the agent runtime's]
    #[arg(long, global = true)]
    pub model: Option<String>,
    /// A directory of briefs [default: the briefs Linewright ships]
    #[arg(long, global = true)]
    pub briefs: Option<String>,
    /// The variables of Linewright's environment that agent sessions are given
    /// besides the ones they always get, comma-separated; a name ending in `*`
    /// names every variable with that prefix [default: none]
    #[arg(long, global = true, value_name = "NAMES")]
    pub session_env: Option<String>,
    /// Each agent session's timeout [default: 60m]
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
    pub spec_glob: SpecGlob,
    pub spec_lookback: TimeDelta,
    pub max_runs: usize,
    pub work_dir: PathBuf,
    pub model: Option<String>,
    pub briefs: Option<PathBuf>,
    pub session_env: SessionEnv,
    pub session_timeout: Duration,
    pub result_retries: u32,
    pub ci_grace: TimeDelta,
    pub ci_timeout: TimeDelta,
    pub ci_attempts: u32,
    pub update_branch: Option<UpdateMethod>,
    pub auto_merge: bool,
    pub merge_method: Option<MergeMethod>,
    pub require_label: Option<String>,
    pub log_format: LogFormat,
}

impl Settings {
    fn flags(&self) -> BTreeMap<&'static str, &String> {
        [
            ("repo", &self.repo),
            ("poll-interval", &self.poll_interval),
            ("webhook-address", &self.webhook_address),
            ("spec-glob", &self.spec_glob),
            ("spec-lookback", &self.spec_lookback),
            ("max-runs", &self.max_runs),
            ("work-dir", &self.work_dir),
            ("model", &self.model),
            ("briefs", &self.briefs),
            ("session-env", &self.session_env),
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

const KEYS: [&str; 20] = [
    "repo",
    "poll-interval",
    "webhook-address",
    "spec-glob",
    "spec-lookback",
    "max-runs",
    "work-dir",
    "model",
    "briefs",
    "session-env",
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

impl Config {
    pub fn required_repo(&self) -> Result<&str> {
        self.repo.as_deref().context("set `repo` (owner/name)")
    }

    pub fn repo_work_dir(&self) -> Result<RepoWorkDir> {
        Ok(RepoWorkDir {
            base: self.work_dir.clone(),
            repo: self.required_repo()?.to_string(),
        })
    }

    pub fn resolve(
        settings: &Settings,
        cwd: &Path,
        env: &dyn Fn(&str) -> Option<String>,
        on_terminal: bool,
    ) -> Result<Config, ResolveError> {
        let file = read_file(settings.config.as_deref(), cwd)?;
        let flags = settings.flags();
        let get = |key: &str| -> Option<String> {
            let variable = format!("LINEWRIGHT_{}", key.to_uppercase().replace('-', "_"));
            flags
                .get(key)
                .copied()
                .cloned()
                .or_else(|| env(&variable))
                .or_else(|| file.get(key).cloned())
        };
        let blame = |keys: &[&str], error: anyhow::Error| {
            if keys.iter().any(|key| flags.contains_key(key)) {
                ResolveError::CommandLine(error)
            } else {
                ResolveError::Elsewhere(error)
            }
        };
        let not_one = |key: &str, value: &str, error: anyhow::Error| {
            blame(
                &[key],
                error.context(format!("the setting `{key}` is {value:?}")),
            )
        };
        let wrong = |key: &str, error: String| blame(&[key], anyhow!(error));
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
        let merge_method = match get("merge-method") {
            None => None,
            Some(name) => Some(name.parse::<MergeMethod>().map_err(|()| {
                wrong(
                    "merge-method",
                    format!(
                        "the setting `merge-method` is {name:?}: it is `merge`, `squash` or `rebase`"
                    ),
                )
            })?),
        };
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
        let webhook_address = get("webhook-address");
        let webhooks = match env(webhook::SECRET_VARIABLE).and_then(webhook::Secret::new) {
            Some(secret) => {
                let address = webhook_address.unwrap_or_else(|| "0.0.0.0:8080".into());
                Some(Webhooks {
                    address: address
                        .parse()
                        .map_err(|e| not_one("webhook-address", &address, e))?,
                    secret,
                })
            }
            None if webhook_address.is_some() => {
                // What is missing is the environment's.
                return Err(ResolveError::Elsewhere(anyhow!(
                    "the setting `webhook-address` is set, but `{}` is not: set it to the \
                         webhook's secret to turn webhooks on",
                    webhook::SECRET_VARIABLE
                )));
            }
            None => None,
        };
        let session_env = get("session-env")
            .unwrap_or_default()
            .parse::<SessionEnv>()
            .map_err(|e| wrong("session-env", e))?;
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
            spec_glob,
            spec_lookback: delta("spec-lookback", "7d")?,
            max_runs: number("max-runs", "4")? as usize,
            work_dir,
            model: get("model"),
            briefs: get("briefs").map(PathBuf::from),
            session_env,
            session_timeout: duration("session-timeout", "60m")?,
            result_retries: number("result-retries", "1")?,
            ci_grace,
            ci_timeout,
            ci_attempts: number("ci-attempts", "2")?,
            update_branch,
            auto_merge,
            merge_method,
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

fn read_file(
    explicit: Option<&Path>,
    cwd: &Path,
) -> Result<BTreeMap<String, String>, ResolveError> {
    let path = explicit
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cwd.join("linewright.toml"));
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if explicit.is_none() && e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
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

fn settings_in(path: &Path, text: &str) -> Result<BTreeMap<String, String>> {
    let table: toml::Table = text
        .parse()
        .with_context(|| format!("parsing {}", path.display()))?;
    table
        .into_iter()
        .map(|(key, value)| {
            if !KEYS.contains(&key.as_str()) {
                bail!("{} has an unknown setting `{key}`", path.display());
            }
            let value = match value {
                toml::Value::String(s) => s,
                toml::Value::Integer(i) => i.to_string(),
                toml::Value::Boolean(b) => b.to_string(),
                // In the form its flag takes, names separated by commas.
                toml::Value::Array(names) if key == "session-env" => names
                    .into_iter()
                    .map(|name| match name.as_str() {
                        Some(variable) if !variable.contains(',') => Ok(variable.to_string()),
                        Some(_) | None => bail!(
                            "{}: the setting `{key}` holds {name}, not a variable's name",
                            path.display()
                        ),
                    })
                    .collect::<Result<Vec<_>>>()?
                    .join(","),
                other @ (toml::Value::Float(_)
                | toml::Value::Datetime(_)
                | toml::Value::Array(_)
                | toml::Value::Table(_)) => bail!(
                    "{}: the setting `{key}` is {other}, not a string, number or boolean",
                    path.display()
                ),
            };
            Ok((key, value))
        })
        .collect()
}
