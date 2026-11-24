//! A request by hand for an automation's run, and `serve`'s answer to it
//! (`docs/specs/automations.md`, By hand).

use serde::{Deserialize, Serialize};

use crate::automation::{Automation, AutomationFiles, AutomationName};
use crate::phrase::Phrase;
use crate::reconcile::{HandWait, Wait};
use crate::repo_files::FileError;
use crate::runs::{RunId, RunOf};
use crate::status::describe_wait;

/// What became of a request by hand that `serve` took.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "request",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum HandTaken {
    /// Its run started: `run`.
    Started { run: RunId },
    /// Its run starts once what it waits for is past.
    Waiting { waits_for: HandWait },
}

/// Why `serve` refused a request by hand.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HandRefusal {
    #[error("`serve` is draining: it takes no request to run an automation")]
    Draining,
    #[error("there is no automation `{name}`: the default branch has no {path}", path = .name.path())]
    NoSuchAutomation { name: AutomationName },
    #[error("{} does not parse: {}", .0.path, .0.error)]
    Unparsed(FileError),
}

/// Whether `serve` takes a request by hand for `name`, `draining` or not,
/// by the automation files the last poll listed, `listed`, if it could list
/// them: the automation to run, `None` when they were not listed, or why
/// `serve` refuses it.
pub fn take<'a>(
    name: &AutomationName,
    draining: bool,
    listed: Option<&'a AutomationFiles>,
) -> Result<Option<&'a Automation>, HandRefusal> {
    if draining {
        return Err(HandRefusal::Draining);
    }
    let Some(files) = listed else {
        return Ok(None);
    };
    if let Some(automation) = files.parsed.iter().find(|a| a.name == *name) {
        return Ok(Some(automation));
    }
    let path = name.path();
    Err(match files.unparsed.iter().find(|file| file.path == path) {
        Some(file) => HandRefusal::Unparsed(FileError {
            path,
            error: file.reason.clone(),
        }),
        None => HandRefusal::NoSuchAutomation { name: name.clone() },
    })
}

impl HandTaken {
    /// What `lw automation run` prints of it: the run's ID, or what the run
    /// waits for.
    pub fn render(&self) -> String {
        match self {
            HandTaken::Started { run } => format!("{run}\n"),
            HandTaken::Waiting { waits_for } => {
                format!("waiting for {}\n", describe_hand_wait(waits_for))
            }
        }
    }
}

/// What `wait` is, as a phrase after "waiting for".
pub fn describe_hand_wait(wait: &HandWait) -> Phrase {
    match wait {
        HandWait::Hold(hold) => describe_wait(&Wait::from(hold.clone())),
        HandWait::RunInFlight(run) => {
            let of = match &run.of {
                RunOf::Automation { automation } => automation.to_string(),
                RunOf::Work { item, .. } => item.to_string(),
            };
            Phrase::from(format!("a run of {of} in flight: {run}"))
        }
        HandWait::Room => Phrase::from("room under ").code("max-runs"),
        HandWait::NextPoll => Phrase::from("the next poll"),
    }
}
