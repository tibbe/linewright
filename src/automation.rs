use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use serde::{Deserialize, Serialize};

use crate::briefs;
use crate::config;
use crate::domain::Timestamp;
use crate::records;
use crate::repo_files;
use crate::store::Store;
use crate::work_settings::{Partial, RepositorySetting};

pub const DIR: &str = repo_files::linewright_path!("/automations");

const DEFAULT_MAX_ISSUES: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Automation {
    pub name: AutomationName,
    pub schedule: Schedule,
    /// The session settings its front matter sets, a layer above every
    /// whole-pipeline layer.
    pub session: Partial,
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
    let (names, mut files) = classify(store.list_files(branch, DIR).await?);
    let paths: Vec<String> = names.iter().map(AutomationName::path).collect();
    let mut blobs = store.read_files(branch, &paths).await?;
    for (name, path) in names.into_iter().zip(paths) {
        let parsed = blobs
            .remove(&path)
            .context("the file is gone")
            .and_then(|blob| blob.text().map_err(anyhow::Error::msg))
            .and_then(|text| parse(&name, &text));
        match parsed {
            Ok(automation) => files.parsed.push(automation),
            Err(e) => files.unparsed.push(Unparsed {
                path,
                reason: format!("{e:#}"),
            }),
        }
    }
    Ok(files)
}

/// Splits `listed`, the files under [`DIR`] by their paths under it, hidden
/// ones aside, into the automations they name and the files that do not
/// parse.
fn classify(listed: Vec<String>) -> (Vec<AutomationName>, AutomationFiles) {
    let mut names = Vec::new();
    let mut files = AutomationFiles::default();
    for file in listed {
        if briefs::is_hidden(&file) {
            continue;
        }
        let name = file
            .strip_suffix(".md")
            .filter(|name| !name.contains('/'))
            .ok_or_else(|| format!("an automation is a `.md` file directly in `{DIR}/`"))
            .and_then(str::parse);
        match name {
            Ok(name) => names.push(name),
            Err(reason) => files.unparsed.push(Unparsed {
                path: format!("{DIR}/{file}"),
                reason,
            }),
        }
    }
    (names, files)
}

/// Its keys but the session settings, which are gathered in `session` and
/// checked against [`RepositorySetting`] so that the front matter takes
/// what the repository's file takes.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct FrontMatter {
    on: Trigger,
    max_issues: Option<u32>,
    #[serde(flatten)]
    session: BTreeMap<String, serde_yaml_ng::Value>,
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
    let mut session = Partial::default();
    for (key, value) in front.session {
        let Some(setting) = RepositorySetting::named(&key) else {
            bail!(
                "the front matter has an unknown key `{key}`: it holds `on`, `max-issues`, {}",
                RepositorySetting::keys()
            );
        };
        let value = toml::Value::try_from(value)
            .map_err(|e| anyhow!("the setting `{key}` is not a string, number or boolean: {e}"))?;
        config::set_session(&mut session, setting, value)?;
    }
    Ok(Automation {
        name: name.clone(),
        schedule: Schedule::parse(&front.on.schedule)?,
        session,
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
