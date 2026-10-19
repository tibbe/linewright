use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use serde::{Deserialize, Serialize};

use crate::domain::Timestamp;
use crate::records;
use crate::store::Store;

pub const DIR: &str = ".linewright/workflows";

const DEFAULT_MAX_ISSUES: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Automation {
    pub name: AutomationName,
    pub schedule: Schedule,
    /// `None` is the `session-timeout` setting.
    pub timeout: Option<Duration>,
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
                "{name:?} is not a workflow's name: the name of a file in `{DIR}`, without `.md`"
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
pub struct LastRuns(BTreeMap<AutomationName, Timestamp>);

impl LastRuns {
    pub fn get(&self, name: &AutomationName) -> Option<Timestamp> {
        self.0.get(name).copied()
    }

    pub fn record(&mut self, name: &AutomationName, at: Timestamp) {
        self.0.insert(name.clone(), at);
    }

    /// Empty when the record is missing or unreadable, which is logged: every
    /// automation then counts as seen for the first time.
    pub fn load(work_dir: &Path) -> LastRuns {
        records::load(&last_runs_path(work_dir), WHAT).unwrap_or_default()
    }

    pub fn save(&self, work_dir: &Path) -> Result<()> {
        records::save(&last_runs_path(work_dir), self, WHAT)
    }
}

const WHAT: &str = "the workflows' last runs";

fn last_runs_path(work_dir: &Path) -> PathBuf {
    work_dir.join("workflow-runs.json")
}

impl FromIterator<(AutomationName, Timestamp)> for LastRuns {
    fn from_iter<I: IntoIterator<Item = (AutomationName, Timestamp)>>(runs: I) -> LastRuns {
        LastRuns(runs.into_iter().collect())
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

/// Fails only when the directory cannot be listed: a file that cannot be
/// read is one that does not parse.
pub async fn read(store: &dyn Store, branch: &str) -> Result<AutomationFiles> {
    let mut files = AutomationFiles::default();
    for file in store.list_files(branch, DIR).await? {
        let Some(name) = file.strip_suffix(".md") else {
            continue;
        };
        let parsed = async {
            let name: AutomationName = name.parse().map_err(anyhow::Error::msg)?;
            let text = store
                .read_file(branch, &name.path())
                .await?
                .context("the file is gone")?;
            parse(&name, &text)
        };
        match parsed.await {
            Ok(automation) => files.parsed.push(automation),
            Err(e) => files.unparsed.push(Unparsed {
                path: format!("{DIR}/{file}"),
                reason: format!("{e:#}"),
            }),
        }
    }
    Ok(files)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct FrontMatter {
    on: Trigger,
    timeout: Option<String>,
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
        serde_yaml_ng::from_str(front).context("the front matter is not a workflow's")?;
    let timeout = front
        .timeout
        .map(|t| {
            humantime::parse_duration(&t)
                .with_context(|| format!("`timeout` is {t:?}, not a duration such as 60m"))
        })
        .transpose()?;
    Ok(Automation {
        name: name.clone(),
        schedule: Schedule::parse(&front.on.schedule)?,
        timeout,
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
