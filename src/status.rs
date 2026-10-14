//! `linewright status`: what `serve` is doing and what holds it back
//! (`docs/specs/monitoring.md`, "status"). `serve` answers it from its own
//! view, deriving the tracked items and the workflows' next runs the way the
//! reconcile step derives them; the command prints the answer.

use std::fmt::Write as _;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::domain::{CheckState, Filed, Number, PullState, Snapshot, Timestamp, label};
use crate::reconcile::{NextRun, Policy, next_run, pending_spec_changes};
use crate::runtime::UsageLimit;
use crate::webhook::Received;
use crate::workflow::{LastRuns, WorkflowFiles};

/// What `serve` knows of itself from its start.
#[derive(Clone, Debug)]
pub struct Serving {
    pub since: Timestamp,
    pub max_runs: usize,
}

/// `serve`'s view as of its last poll, and of what it has done since.
#[derive(Clone, Debug)]
pub struct View {
    /// The last poll's snapshot, with what refreshes read into it since.
    pub snapshot: Arc<Snapshot>,
    pub policy: Policy,
    pub polls: Polls,
    /// The usage limit last refused for, whether or not it has reset.
    pub usage_limit: Option<UsageLimit>,
    pub runs_in_flight: usize,
    /// The workflow files the last poll read.
    pub workflows: WorkflowFiles,
    pub last_runs: LastRuns,
}

/// The answer to `status`, as the local API sends it. Its fields keep their
/// names and meanings (`docs/specs/monitoring.md`, "Output").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub linewright: Linewright,
    pub items: Vec<Tracked>,
    pub workflows: Vec<WorkflowRuns>,
    pub unparsed_workflows: Vec<UnparsedWorkflow>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Linewright {
    pub serving_since: Timestamp,
    /// `None` before the first poll, which is due at once.
    pub polls: Option<Polls>,
    /// Set only while a usage limit is in force.
    pub usage_limit_resets_at: Option<Timestamp>,
    pub runs_in_flight: usize,
    pub max_runs: usize,
    /// Set only with webhooks on.
    pub webhooks: Option<Received>,
}

/// When the last poll took its snapshot, and when the next is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Polls {
    pub last: Timestamp,
    pub next: Timestamp,
}

/// An open item Linewright tracks, and where it is in the pipeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tracked {
    pub number: Number,
    pub title: String,
    #[serde(flatten)]
    pub state: ItemState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum ItemState {
    /// A merged spec change not yet cut into tickets.
    WaitingForTickets,
    Tracking {
        tickets_closed: usize,
        tickets: usize,
    },
    WaitingForTriage,
    WaitingOnTheReporter,
    WithTheMaintainer,
    InReview {
        pull_request: Number,
        draft: bool,
        ci: Ci,
    },
    BlockedBy {
        blockers: Vec<Number>,
    },
    Claimed,
    ReadyToClaim,
}

/// CI on a pull request, as its checks report it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ci {
    NoChecks,
    Failing,
    Running,
    Green,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRuns {
    pub name: String,
    #[serde(flatten)]
    pub runs: Runs,
}

/// A workflow's last run and its next, by its schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "schedule", rename_all = "kebab-case")]
pub enum Runs {
    /// No poll has seen it yet.
    NotRunYet,
    /// A tick of its schedule has passed since its last run.
    Due { last_run: Timestamp },
    Scheduled {
        last_run: Timestamp,
        next_run: Timestamp,
    },
    /// Its schedule ticks no more.
    Ended { last_run: Timestamp },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnparsedWorkflow {
    pub path: String,
    pub reason: String,
}

/// The answer at `now`: Linewright itself, with the deliveries `webhooks`
/// counts with them on, then the items it tracks, then the workflows by name
/// and the workflow files that do not parse. Before the first poll, `view` is
/// `None` and there are no items or workflows.
pub fn status(
    serving: &Serving,
    webhooks: Option<Received>,
    view: Option<&View>,
    now: Timestamp,
) -> Status {
    let linewright = Linewright {
        serving_since: serving.since,
        polls: view.map(|v| v.polls),
        usage_limit_resets_at: view
            .and_then(|v| v.usage_limit)
            .map(|limit| limit.resets_at)
            .filter(|resets_at| now < *resets_at),
        runs_in_flight: view.map_or(0, |v| v.runs_in_flight),
        max_runs: serving.max_runs,
        webhooks,
    };
    let Some(view) = view else {
        return Status {
            linewright,
            items: vec![],
            workflows: vec![],
            unparsed_workflows: vec![],
        };
    };
    let mut workflows: Vec<WorkflowRuns> = view
        .workflows
        .parsed
        .iter()
        .map(|workflow| WorkflowRuns {
            name: workflow.name.to_string(),
            runs: match view.last_runs.get(&workflow.name) {
                None => Runs::NotRunYet,
                Some(last_run) => match next_run(workflow, last_run, now) {
                    NextRun::Due => Runs::Due { last_run },
                    NextRun::At(next_run) => Runs::Scheduled { last_run, next_run },
                    NextRun::Never => Runs::Ended { last_run },
                },
            },
        })
        .collect();
    workflows.sort_by(|a, b| a.name.cmp(&b.name));
    let mut unparsed_workflows: Vec<UnparsedWorkflow> = view
        .workflows
        .unparsed
        .iter()
        .map(|file| UnparsedWorkflow {
            path: file.path.clone(),
            reason: file.reason.clone(),
        })
        .collect();
    unparsed_workflows.sort_by(|a, b| a.path.cmp(&b.path));
    Status {
        linewright,
        items: tracked(&view.snapshot, now, &view.policy),
        workflows,
        unparsed_workflows,
    }
}

fn tracked(snapshot: &Snapshot, now: Timestamp, policy: &Policy) -> Vec<Tracked> {
    let linewright = &policy.linewright;
    let mut items = vec![];
    for pr in pending_spec_changes(snapshot, now, policy) {
        items.push(Tracked {
            number: pr.number,
            title: pr.title.clone(),
            state: ItemState::WaitingForTickets,
        });
    }
    for issue in snapshot.issues.iter().filter(|i| i.open) {
        let state = if matches!(issue.filed, Some(Filed::Tracking { .. })) {
            ItemState::Tracking {
                tickets_closed: issue.sub_issues.iter().filter(|t| !t.open).count(),
                tickets: issue.sub_issues.len(),
            }
        } else if issue.has_label(label::NEEDS_TRIAGE) {
            ItemState::WaitingForTriage
        } else if issue.has_label(label::NEEDS_INFO) {
            ItemState::WaitingOnTheReporter
        } else if issue.has_label(label::READY_FOR_HUMAN) {
            ItemState::WithTheMaintainer
        } else if issue.has_label(label::READY_FOR_AGENT) {
            let pr = snapshot.pulls.iter().find(|p| {
                p.is_open() && &p.author == linewright && p.closes.contains(&issue.number)
            });
            let blockers: Vec<Number> = issue
                .blocked_by
                .iter()
                .filter(|b| b.open)
                .map(|b| b.number)
                .collect();
            match pr {
                Some(pr) => ItemState::InReview {
                    pull_request: pr.number,
                    draft: matches!(pr.state, PullState::Open { draft: true }),
                    ci: if pr.checks.is_empty() {
                        Ci::NoChecks
                    } else if pr.checks.iter().any(|c| c.state == CheckState::Failed) {
                        Ci::Failing
                    } else if pr.checks.iter().any(|c| c.state == CheckState::Pending) {
                        Ci::Running
                    } else {
                        Ci::Green
                    },
                },
                None if !blockers.is_empty() => ItemState::BlockedBy { blockers },
                None if issue.is_assigned(linewright) => ItemState::Claimed,
                None => ItemState::ReadyToClaim,
            }
        } else {
            continue;
        };
        items.push(Tracked {
            number: issue.number,
            title: issue.title.clone(),
            state,
        });
    }
    items
}

/// The answer as text: one fact per line as `label: value`, a blank line
/// between sections, coloured if `colour` (`docs/specs/monitoring.md`,
/// "Output").
pub fn render(status: &Status, colour: bool) -> String {
    let paint = Paint(colour);
    let sections = [
        linewright_lines(&status.linewright, paint),
        status
            .items
            .iter()
            .map(|item| {
                format!(
                    "{}: {} — {}",
                    paint.bold(&item.number.to_string()),
                    describe(&item.state),
                    item.title
                )
            })
            .collect(),
        workflow_lines(status, paint),
    ];
    let mut text = String::new();
    for section in sections.iter().filter(|section| !section.is_empty()) {
        if !text.is_empty() {
            text.push('\n');
        }
        for line in section {
            let _ = writeln!(text, "{line}");
        }
    }
    text
}

/// Colours text with ANSI escapes, if it is on.
#[derive(Clone, Copy)]
pub(crate) struct Paint(pub bool);

impl Paint {
    fn with(self, code: &str, text: &str) -> String {
        if self.0 {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub(crate) fn bold(self, text: &str) -> String {
        self.with("1", text)
    }

    pub(crate) fn yellow(self, text: &str) -> String {
        self.with("33", text)
    }

    pub(crate) fn red(self, text: &str) -> String {
        self.with("31", text)
    }
}

fn linewright_lines(linewright: &Linewright, paint: Paint) -> Vec<String> {
    let polls = linewright.polls;
    let mut lines = vec![
        format!("serving since: {}", utc(linewright.serving_since)),
        format!(
            "last poll: {}",
            polls.map_or("none yet".into(), |p| utc(p.last))
        ),
        format!(
            "next poll: {}",
            polls.map_or("due now".into(), |p| utc(p.next))
        ),
    ];
    if let Some(resets_at) = linewright.usage_limit_resets_at {
        lines.push(paint.yellow(&format!("usage limit: in force until {}", utc(resets_at))));
    }
    lines.push(format!(
        "runs in flight: {} of {}",
        linewright.runs_in_flight, linewright.max_runs
    ));
    if let Some(received) = linewright.webhooks {
        lines.push(format!(
            "last delivery accepted: {}",
            received.last_accepted.map_or("none yet".into(), utc)
        ));
        lines.push(format!("deliveries refused: {}", received.refused));
    }
    lines
}

fn workflow_lines(status: &Status, paint: Paint) -> Vec<String> {
    let runs = status.workflows.iter().flat_map(|workflow| {
        let (last, next) = match workflow.runs {
            Runs::NotRunYet => (
                "none yet, its schedule starts when a poll first sees it".into(),
                None,
            ),
            Runs::Due { last_run } => (utc(last_run), Some("due now".into())),
            Runs::Scheduled { last_run, next_run } => (utc(last_run), Some(utc(next_run))),
            Runs::Ended { last_run } => (
                utc(last_run),
                Some("none, its schedule ticks no more".into()),
            ),
        };
        let name = &workflow.name;
        std::iter::once(format!("workflow {name} last run: {last}"))
            .chain(next.map(|next| format!("workflow {name} next run: {next}")))
    });
    let unparsed = status
        .unparsed_workflows
        .iter()
        .map(|file| paint.red(&format!("{}: does not parse: {}", file.path, file.reason)));
    runs.chain(unparsed).collect()
}

fn describe(state: &ItemState) -> String {
    match state {
        ItemState::WaitingForTickets => "spec change waiting for tickets".into(),
        ItemState::Tracking {
            tickets_closed,
            tickets,
        } => format!("tracking issue, {tickets_closed}/{tickets} tickets closed"),
        ItemState::WaitingForTriage => "waiting for triage".into(),
        ItemState::WaitingOnTheReporter => "waiting on the reporter".into(),
        ItemState::WithTheMaintainer => "with the maintainer".into(),
        ItemState::InReview {
            pull_request,
            draft,
            ci,
        } => {
            let stage = if *draft { "draft" } else { "ready for review" };
            let ci = match ci {
                Ci::NoChecks => "no checks",
                Ci::Failing => "CI failing",
                Ci::Running => "CI running",
                Ci::Green => "CI green",
            };
            format!("in review: {pull_request} ({stage}, {ci})")
        }
        ItemState::BlockedBy { blockers } => {
            let blockers: Vec<String> = blockers.iter().map(Number::to_string).collect();
            format!("blocked by {}", blockers.join(", "))
        }
        ItemState::Claimed => "claimed: being implemented".into(),
        ItemState::ReadyToClaim => "ready to claim".into(),
    }
}

pub(crate) fn utc(time: Timestamp) -> String {
    time.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}
