//! The action that shows the automations (`docs/specs/automations.md`,
//! Automations), and what it shows: the model that the page renders and the
//! tool returns.

use schemars::JsonSchema;
use serde::Serialize;

use crate::access::{Permit, Read};
use crate::api::Sources;
use crate::automation;
use crate::domain::Timestamp;
use crate::runs::{OutcomeName, Recorded};
use crate::status::{self, LastRun, Trigger, UnparsedAutomation, View};

/// The automations as their screen shows them, with the names `status`
/// gives the same facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct AutomationsScreen {
    /// Each automation file that does not parse, by path.
    pub unparsed_automations: Vec<UnparsedAutomation>,
    /// By name.
    pub automations: Vec<AutomationScreen>,
}

/// A row of the automations (`docs/specs/automations.md`, Automations).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct AutomationScreen {
    pub name: String,
    /// When it runs, in words.
    pub when: String,
    /// Where its trigger stands: for one on a schedule, when it runs next.
    #[serde(flatten)]
    pub trigger: Trigger,
    #[serde(flatten)]
    pub last_run: LastRunScreen,
}

/// An automation's last run, with its outcome; or, before its first,
/// when Linewright first saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum LastRunScreen {
    Run {
        #[serde(flatten)]
        run: LastRun,
        last_run_outcome: OutcomeName,
    },
    /// Before its first run: when Linewright first saw it, which is unknown
    /// until a poll has, and for an automation not on a schedule.
    NoneYet {
        #[serde(skip_serializing_if = "Option::is_none")]
        first_seen: Option<Timestamp>,
    },
}

impl AutomationsScreen {
    /// From `status`'s answer over `view` and `recorded`, as of `now`.
    pub fn of(
        status: &status::Status,
        view: &View,
        recorded: &[Recorded],
        now: Timestamp,
    ) -> AutomationsScreen {
        let automations = status
            .automations
            .iter()
            .filter_map(|answered| {
                let automation = view
                    .automations
                    .parsed
                    .iter()
                    .find(|automation| automation.name.to_string() == answered.name)?;
                let last_run = answered.last_run.as_ref().and_then(|last| {
                    let run = recorded.iter().find(|run| run.id == last.id)?;
                    Some(LastRunScreen::Run {
                        run: last.clone(),
                        last_run_outcome: run.clone().at(now).outcome.name(),
                    })
                });
                let last_run = last_run.unwrap_or_else(|| LastRunScreen::NoneYet {
                    first_seen: match automation.trigger {
                        automation::Trigger::Schedule(_) => {
                            view.last_scheduled_runs.get(&automation.name)
                        }
                        automation::Trigger::WorkflowRun(_) => None,
                    },
                });
                Some(AutomationScreen {
                    name: answered.name.clone(),
                    when: automation.trigger.describe(),
                    trigger: answered.trigger,
                    last_run,
                })
            })
            .collect();
        AutomationsScreen {
            unparsed_automations: status.unparsed_automations.clone(),
            automations,
        }
    }
}

/// The automations as their screen shows them now, `None` before the first
/// poll.
pub async fn list_automations(
    permit: &Permit<Read>,
    sources: &Sources,
) -> Option<AutomationsScreen> {
    super::overview(permit, sources).await.automations
}
