//! Workflows: Markdown files in `.linewright/workflows/` on the default
//! branch, each an agent's instructions under YAML front matter that says
//! when they run (`docs/specs/product.md`, "Workflows"). A file is parsed once,
//! here, into a [`Workflow`].

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use serde::Deserialize;

use crate::domain::Timestamp;

/// Where the workflows live on the default branch.
pub const DIR: &str = ".linewright/workflows";

/// The issues one run files when the workflow sets no `max-issues`.
const DEFAULT_MAX_ISSUES: u32 = 3;

#[derive(Clone, Debug)]
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
#[derive(Clone, Debug)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
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

impl fmt::Display for WorkflowName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
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
