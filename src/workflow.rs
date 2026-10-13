//! Workflows: Markdown files in `.linewright/workflows/` on the default
//! branch, each an agent's instructions under YAML front matter that says
//! when they run (`docs/specs/workflows.md`). A file is parsed once, here,
//! into a [`Workflow`]. The files are read from the store here too, and
//! the workflows' last runs kept under the work directory.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::domain::Timestamp;
use crate::store::Store;

/// Where the workflows live on the default branch.
pub const DIR: &str = ".linewright/workflows";

/// The issues one run files when the workflow sets no `max-issues`.
const DEFAULT_MAX_ISSUES: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workflow {
    pub name: WorkflowName,
    /// The trigger, `on.schedule`.
    pub schedule: Schedule,
    /// The session timeout of its runs; `None` is the `session-timeout`
    /// setting.
    pub timeout: Option<Duration>,
    /// The most issues one run files.
    pub max_issues: u32,
    /// The body: the agent's instructions.
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

    /// The first tick after `time`, if the schedule has one.
    pub fn next_after(&self, time: Timestamp) -> Option<Timestamp> {
        self.0.find_next_occurrence(&time, false).ok()
    }
}

/// A workflow's name: the name of its file in [`DIR`], without `.md`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WorkflowName(String);

impl WorkflowName {
    /// The workflow's file in the repository.
    pub fn path(&self) -> String {
        format!("{DIR}/{}.md", self.0)
    }
}

impl FromStr for WorkflowName {
    type Err = String;

    fn from_str(name: &str) -> Result<WorkflowName, String> {
        if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) {
            return Err(format!(
                "{name:?} is not a workflow's name: the name of a file in `{DIR}`, without `.md`"
            ));
        }
        Ok(WorkflowName(name.to_string()))
    }
}

impl TryFrom<String> for WorkflowName {
    type Error = String;

    fn try_from(name: String) -> Result<WorkflowName, String> {
        name.parse()
    }
}

impl From<WorkflowName> for String {
    fn from(name: WorkflowName) -> String {
        name.0
    }
}

impl fmt::Display for WorkflowName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// When each workflow last ran, by its name: bookkeeping Linewright keeps
/// under the work directory and no person reads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LastRuns(BTreeMap<WorkflowName, Timestamp>);

impl LastRuns {
    pub fn get(&self, name: &WorkflowName) -> Option<Timestamp> {
        self.0.get(name).copied()
    }

    pub fn record(&mut self, name: &WorkflowName, at: Timestamp) {
        self.0.insert(name.clone(), at);
    }

    /// The last runs recorded under `work_dir`. None are recorded when the
    /// record is missing, and when it is unreadable, which is logged: every
    /// workflow then counts as seen for the first time.
    pub fn load(work_dir: &Path) -> LastRuns {
        let path = last_runs_path(work_dir);
        let read = || -> Result<LastRuns> {
            match std::fs::read_to_string(&path) {
                Ok(text) => Ok(serde_json::from_str(&text)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LastRuns::default()),
                Err(e) => Err(e.into()),
            }
        };
        read().unwrap_or_else(|e| {
            warn!(path = %path.display(), error = format!("{e:#}"), "ignoring the workflows' last runs");
            LastRuns::default()
        })
    }

    /// Records these last runs under `work_dir`, whole: to a file beside the
    /// record, then renamed over it, so a crash leaves the old record or the
    /// new one.
    pub fn save(&self, work_dir: &Path) -> Result<()> {
        let path = last_runs_path(work_dir);
        let dir = path.parent().context("the record has a directory")?;
        std::fs::create_dir_all(dir).context("creating the work directory")?;
        let mut file =
            tempfile::NamedTempFile::new_in(dir).context("writing the workflows' last runs")?;
        serde_json::to_writer_pretty(&mut file, self)?;
        file.persist(path)
            .context("writing the workflows' last runs")?;
        Ok(())
    }
}

/// Where the workflows' last runs are kept, so they survive a restart.
fn last_runs_path(work_dir: &Path) -> PathBuf {
    work_dir.join("workflow-runs.json")
}

impl FromIterator<(WorkflowName, Timestamp)> for LastRuns {
    fn from_iter<I: IntoIterator<Item = (WorkflowName, Timestamp)>>(runs: I) -> LastRuns {
        LastRuns(runs.into_iter().collect())
    }
}

/// The files in [`DIR`] on a branch: the workflows whose files parse, and
/// the files that do not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkflowFiles {
    pub parsed: Vec<Workflow>,
    pub unparsed: Vec<Unparsed>,
}

/// A file in [`DIR`] that does not parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unparsed {
    /// Its path in the repository.
    pub path: String,
    /// Why it does not parse.
    pub reason: String,
}

/// Reads and parses the Markdown files in [`DIR`] at the head of `branch`.
/// Fails only when the directory cannot be listed: a file that cannot be
/// read is one that does not parse.
pub async fn read(store: &dyn Store, branch: &str) -> Result<WorkflowFiles> {
    let mut files = WorkflowFiles::default();
    for file in store.list_files(branch, DIR).await? {
        let Some(name) = file.strip_suffix(".md") else {
            continue;
        };
        let parsed = async {
            let name: WorkflowName = name.parse().map_err(anyhow::Error::msg)?;
            let text = store
                .read_file(branch, &name.path())
                .await?
                .context("the file is gone")?;
            parse(&name, &text)
        };
        match parsed.await {
            Ok(workflow) => files.parsed.push(workflow),
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

/// Parses the workflow `name` from its file's `text`, saying why when it
/// does not parse.
pub fn parse(name: &WorkflowName, text: &str) -> Result<Workflow> {
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
    Ok(Workflow {
        name: name.clone(),
        schedule: Schedule::parse(&front.on.schedule)?,
        timeout,
        max_issues: front.max_issues.unwrap_or(DEFAULT_MAX_ISSUES),
        instructions: body.to_string(),
    })
}

/// The front matter and the body after it.
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
