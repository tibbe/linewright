//! Configuration: every setting is a flag, a `LINEWRIGHT_<FLAG>` environment
//! variable or a key in a TOML file, with precedence flag, environment, file,
//! default (`docs/specs/product.md`, "Operation"), parsed once into one typed value.
//! Credentials are not settings: they come from the environment variables
//! their tools already read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::TimeDelta;
use clap::{Args, Parser, Subcommand};

use crate::domain::{MergeMethod, UpdateMethod};
use crate::spec_glob::SpecGlob;
use crate::workflow::WorkflowName;

#[derive(Parser, Debug)]
#[command(name = "linewright", version, about = "A spec-driven software factory")]
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
    /// List the work in flight, read from GitHub.
    Status,
    /// Workflows: `workflow run NAME` runs one now, then exits.
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommand,
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

#[derive(Subcommand, Debug)]
pub enum WorkflowCommand {
    /// Run the workflow in `.linewright/workflows/NAME.md` on the default
    /// branch once, now, then exit; its schedule stays as it was.
    Run {
        #[arg(value_name = "NAME")]
        name: WorkflowName,
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
    /// The tracked Markdown files that are the spec [default: docs/**/*.md]
    #[arg(long, global = true)]
    pub spec_glob: Option<String>,
    /// How far back merged spec changes are picked up [default: 7d]
    #[arg(long, global = true)]
    pub spec_lookback: Option<String>,
    /// The ceiling on simultaneous runs [default: 4]
    #[arg(long, global = true)]
    pub max_workers: Option<String>,
    /// Where clones and session traces go [default: linewright/ under the user's data directory]
    #[arg(long, global = true)]
    pub work_dir: Option<String>,
    /// The model for every stage [default: the agent runtime's]
    #[arg(long, global = true)]
    pub model: Option<String>,
    /// A directory of briefs [default: the briefs Linewright ships]
    #[arg(long, global = true)]
    pub briefs: Option<String>,
    /// Each agent session's timeout [default: 30m]
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
    /// Merge green pull requests instead of requesting review [default: off]
    #[arg(long, global = true, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    pub auto_merge: Option<String>,
    /// `merge`, `squash` or `rebase` [default: from the repository's settings]
    #[arg(long, global = true)]
    pub merge_method: Option<String>,
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
    /// Required by the commands that talk to GitHub; `replay` runs without.
    pub repo: Option<String>,
    pub poll_interval: Duration,
    pub spec_glob: SpecGlob,
    pub spec_lookback: TimeDelta,
    pub max_workers: usize,
    pub work_dir: PathBuf,
    pub model: Option<String>,
    pub briefs: Option<PathBuf>,
    pub session_timeout: Duration,
    pub result_retries: u32,
    pub ci_grace: TimeDelta,
    pub ci_timeout: TimeDelta,
    pub ci_attempts: u32,
    pub update_branch: Option<UpdateMethod>,
    pub auto_merge: bool,
    pub merge_method: Option<MergeMethod>,
    pub log_format: LogFormat,
}

impl Settings {
    fn flags(&self) -> BTreeMap<&'static str, &String> {
        [
            ("repo", &self.repo),
            ("poll-interval", &self.poll_interval),
            ("spec-glob", &self.spec_glob),
            ("spec-lookback", &self.spec_lookback),
            ("max-workers", &self.max_workers),
            ("work-dir", &self.work_dir),
            ("model", &self.model),
            ("briefs", &self.briefs),
            ("session-timeout", &self.session_timeout),
            ("result-retries", &self.result_retries),
            ("ci-grace", &self.ci_grace),
            ("ci-timeout", &self.ci_timeout),
            ("ci-attempts", &self.ci_attempts),
            ("update-branch", &self.update_branch),
            ("auto-merge", &self.auto_merge),
            ("merge-method", &self.merge_method),
            ("log-format", &self.log_format),
        ]
        .into_iter()
        .filter_map(|(key, value)| Some((key, value.as_ref()?)))
        .collect()
    }
}

const KEYS: [&str; 17] = [
    "repo",
    "poll-interval",
    "spec-glob",
    "spec-lookback",
    "max-workers",
    "work-dir",
    "model",
    "briefs",
    "session-timeout",
    "result-retries",
    "ci-grace",
    "ci-timeout",
    "ci-attempts",
    "update-branch",
    "auto-merge",
    "merge-method",
    "log-format",
];

impl Config {
    /// Merges the layers: `settings` (flags), `env`, the config file
    /// (relative to `cwd` by default), and the defaults.
    pub fn resolve(
        settings: &Settings,
        cwd: &Path,
        env: &dyn Fn(&str) -> Option<String>,
        on_terminal: bool,
    ) -> Result<Config> {
        let file = read_file(settings.config.as_deref(), cwd)?;
        let flags = settings.flags();
        let get = |key: &str| -> Option<String> {
            let variable = format!("LINEWRIGHT_{}", key.to_uppercase().replace('-', "_"));
            flags
                .get(key)
                .map(|v| v.to_string())
                .or_else(|| env(&variable))
                .or_else(|| file.get(key).cloned())
        };
        // Each setting is parsed once, into its type, naming the setting
        // when its value is not one.
        fn parsed<T, E: Into<anyhow::Error>>(
            key: &str,
            value: Option<String>,
            default: &str,
            parse: impl Fn(&str) -> Result<T, E>,
        ) -> Result<T> {
            let value = value.unwrap_or_else(|| default.to_string());
            parse(&value)
                .map_err(Into::into)
                .with_context(|| format!("the setting `{key}` is {value:?}"))
        }
        let duration =
            |key: &str, default: &str| parsed(key, get(key), default, humantime::parse_duration);
        let delta = |key: &str, default: &str| -> Result<TimeDelta> {
            Ok(TimeDelta::from_std(duration(key, default)?)?)
        };
        let number = |key: &str, default: &str| parsed(key, get(key), default, str::parse::<u32>);
        let glob = get("spec-glob").unwrap_or_else(|| "docs/**/*.md".into());
        let spec_glob =
            SpecGlob::new(&glob).with_context(|| format!("the setting `spec-glob` is {glob:?}"))?;
        let update_branch = match get("update-branch").as_deref() {
            None => None,
            Some("rebase") => Some(UpdateMethod::Rebase),
            Some("merge") => Some(UpdateMethod::Merge),
            Some(other) => {
                bail!("the setting `update-branch` is {other:?}: it is `rebase` or `merge`")
            }
        };
        let auto_merge = match get("auto-merge").as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            Some(other) => {
                bail!("the setting `auto-merge` is {other:?}: it is `true` or `false`")
            }
        };
        let merge_method = match get("merge-method").as_deref() {
            None => None,
            Some("merge") => Some(MergeMethod::Merge),
            Some("squash") => Some(MergeMethod::Squash),
            Some("rebase") => Some(MergeMethod::Rebase),
            Some(other) => bail!(
                "the setting `merge-method` is {other:?}: it is `merge`, `squash` or `rebase`"
            ),
        };
        let log_format = match get("log-format").as_deref() {
            None if on_terminal => LogFormat::Text,
            None => LogFormat::Json,
            Some("text") => LogFormat::Text,
            Some("json") => LogFormat::Json,
            Some(other) => bail!("the setting `log-format` is {other:?}: it is `text` or `json`"),
        };
        let work_dir = match get("work-dir") {
            Some(dir) => PathBuf::from(dir),
            None => dirs::data_dir()
                .context("no data directory for the default `work-dir`; set it")?
                .join("linewright"),
        };
        let (ci_grace, ci_timeout) = (delta("ci-grace", "5m")?, delta("ci-timeout", "6h")?);
        if ci_timeout <= ci_grace {
            bail!("the setting `ci-timeout` must be longer than `ci-grace`");
        }
        Ok(Config {
            repo: get("repo"),
            poll_interval: duration("poll-interval", "60s")?,
            spec_glob,
            spec_lookback: delta("spec-lookback", "7d")?,
            max_workers: number("max-workers", "4")? as usize,
            work_dir,
            model: get("model"),
            briefs: get("briefs").map(PathBuf::from),
            session_timeout: duration("session-timeout", "30m")?,
            result_retries: number("result-retries", "1")?,
            ci_grace,
            ci_timeout,
            ci_attempts: number("ci-attempts", "2")?,
            update_branch,
            auto_merge,
            merge_method,
            log_format,
        })
    }
}

/// The config file's settings, as strings. An explicit `--config` must
/// exist; the default `linewright.toml` may not.
fn read_file(explicit: Option<&Path>, cwd: &Path) -> Result<BTreeMap<String, String>> {
    let path = explicit
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cwd.join("linewright.toml"));
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if explicit.is_none() && e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
        }
        Err(e) => return Err(anyhow!(e).context(format!("reading {}", path.display()))),
    };
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
                other => bail!(
                    "{}: the setting `{key}` is {other}, not a string, number or boolean",
                    path.display()
                ),
            };
            Ok((key, value))
        })
        .collect()
}
