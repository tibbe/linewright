use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use serde::{Deserialize, Serialize};

use crate::briefs;
use crate::domain::Timestamp;
use crate::records;
use crate::repo_files;
use crate::store::Store;
use crate::work_settings::Effort;

pub const DIR: &str = repo_files::linewright_path!("/automations");

const DEFAULT_MAX_ISSUES: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Automation {
    pub name: AutomationName,
    pub schedule: Schedule,
    /// `None` is the `session-timeout` setting.
    pub timeout: Option<Duration>,
    /// `None` is the `model` setting.
    pub model: Option<String>,
    /// `None` is the `effort` setting.
    pub effort: Option<Effort>,
    pub max_issues: u32,
    pub instructions: String,
}

/// A cron expression's five fields, in UTC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule(Cron);

impl Schedule {
    fn parse(expression: &str) -> Result<Schedule> {
        CronParser::builder()
            .seconds(Seconds::Disallowed)
            .year(Year::Disallowed)
            .build()
            .parse(expression)
            .map(Schedule)
            .map_err(|e| anyhow!("`on.schedule` is {expression:?}, not a cron expression: {e}"))
    }

    /// When it runs, in croner's words.
    pub fn describe(&self) -> String {
        self.0.describe()
    }

    pub fn next_after(&self, time: Timestamp) -> Option<Timestamp> {
        self.0.find_next_occurrence(&time, false).ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AutomationName(String);

impl AutomationName {
    pub fn path(&self) -> String {
        format!("{DIR}/{}.md", self.0)
    }
}

impl FromStr for AutomationName {
    type Err = String;

    fn from_str(name: &str) -> Result<AutomationName, String> {
        if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) {
            return Err(format!(
                "{name:?} is not an automation's name: the name of a file in `{DIR}`, without `.md`"
            ));
        }
        Ok(AutomationName(name.to_string()))
    }
}

impl TryFrom<String> for AutomationName {
    type Error = String;

    fn try_from(name: String) -> Result<AutomationName, String> {
        name.parse()
    }
}

impl From<AutomationName> for String {
    fn from(name: AutomationName) -> String {
        name.0
    }
}

impl fmt::Display for AutomationName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LastScheduledRuns(BTreeMap<AutomationName, Timestamp>);

impl LastScheduledRuns {
    pub fn get(&self, name: &AutomationName) -> Option<Timestamp> {
        self.0.get(name).copied()
    }

    pub fn record(&mut self, name: &AutomationName, at: Timestamp) {
        self.0.insert(name.clone(), at);
    }

    /// As `recorded` has `name`'s last scheduled run, or none.
    pub fn restore(&mut self, name: &AutomationName, recorded: &LastScheduledRuns) {
        match recorded.get(name) {
            Some(at) => self.record(name, at),
            None => {
                self.0.remove(name);
            }
        }
    }

    /// Empty when the record is missing or unreadable, which is logged: every
    /// automation then counts as seen for the first time. Until the record is
    /// first saved, the one an earlier version kept stands in for it.
    pub fn load(work_dir: &Path) -> LastScheduledRuns {
        let path = last_scheduled_runs_path(work_dir);
        let path = if path.exists() {
            path
        } else {
            old_last_scheduled_runs_path(work_dir)
        };
        records::load(&path, WHAT).unwrap_or_default()
    }

    pub fn save(&self, work_dir: &Path) -> Result<()> {
        records::save(&last_scheduled_runs_path(work_dir), self, WHAT)
    }
}

const WHAT: &str = "the automations' last scheduled runs";

fn last_scheduled_runs_path(work_dir: &Path) -> PathBuf {
    work_dir.join("automation-runs.json")
}

/// Where versions before automations were renamed from workflows kept the
/// record: read until the record is first saved, never written to.
fn old_last_scheduled_runs_path(work_dir: &Path) -> PathBuf {
    work_dir.join("workflow-runs.json")
}

impl FromIterator<(AutomationName, Timestamp)> for LastScheduledRuns {
    fn from_iter<I: IntoIterator<Item = (AutomationName, Timestamp)>>(
        runs: I,
    ) -> LastScheduledRuns {
        LastScheduledRuns(runs.into_iter().collect())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutomationFiles {
    pub parsed: Vec<Automation>,
    pub unparsed: Vec<Unparsed>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unparsed {
    pub path: String,
    pub reason: String,
}

/// Every file in [`DIR`], hidden ones aside, is an automation file
/// (`docs/specs/automations.md`, A file that does not parse). Fails only
/// when the directory cannot be listed or its files read: a file that cannot
/// be read as text is one that does not parse.
pub async fn read(store: &dyn Store, branch: &str) -> Result<AutomationFiles> {
    let (named, mut files) = sort(store.list_files(branch, DIR).await?);
    let paths: Vec<String> = named.iter().map(|(path, _)| path.clone()).collect();
    let mut blobs = store.read_files(branch, &paths).await?;
    for (path, name) in named {
        let blob = blobs.remove(&path);
        let parsed = || {
            let name: AutomationName = name.parse().map_err(anyhow::Error::msg)?;
            let text = blob
                .context("the file is gone")?
                .text()
                .map_err(anyhow::Error::msg)?;
            parse(&name, &text)
        };
        match parsed() {
            Ok(automation) => files.parsed.push(automation),
            Err(e) => files.unparsed.push(Unparsed {
                path,
                reason: format!("{e:#}"),
            }),
        }
    }
    Ok(files)
}

/// Sorts `listed`, the files under [`DIR`] by their paths under it, hidden
/// ones aside, into those named as automations, each by its path in the
/// repository and its name, and those that do not parse.
fn sort(listed: Vec<String>) -> (Vec<(String, String)>, AutomationFiles) {
    let mut named = Vec::new();
    let mut files = AutomationFiles::default();
    for file in listed {
        if briefs::is_hidden(&file) {
            continue;
        }
        let path = format!("{DIR}/{file}");
        match file.strip_suffix(".md").filter(|name| !name.contains('/')) {
            Some(name) => named.push((path, name.to_string())),
            None => files.unparsed.push(Unparsed {
                path,
                reason: format!("an automation is a `.md` file directly in `{DIR}/`"),
            }),
        }
    }
    (named, files)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct FrontMatter {
    on: Trigger,
    timeout: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    max_issues: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Trigger {
    schedule: String,
}

pub fn parse(name: &AutomationName, text: &str) -> Result<Automation> {
    let (front, body) = split_front_matter(text)
        .context("the file does not open with YAML front matter between `---` lines")?;
    let front: FrontMatter =
        serde_yaml_ng::from_str(front).context("the front matter is not an automation's")?;
    let timeout = front
        .timeout
        .map(|t| {
            humantime::parse_duration(&t)
                .with_context(|| format!("`timeout` is {t:?}, not a duration such as 60m"))
        })
        .transpose()?;
    let effort = front
        .effort
        .map(|e| e.parse::<Effort>().map_err(anyhow::Error::msg))
        .transpose()?;
    Ok(Automation {
        name: name.clone(),
        schedule: Schedule::parse(&front.on.schedule)?,
        timeout,
        model: front.model,
        effort,
        max_issues: front.max_issues.unwrap_or(DEFAULT_MAX_ISSUES),
        instructions: body.to_string(),
    })
}

fn split_front_matter(text: &str) -> Option<(&str, &str)> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((&rest[..offset], &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}
