use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::domain::{
    CheckState, Filed, Issue, Item, Login, Number, Pull, PullState, Snapshot, Timestamp, label,
};
use crate::reconcile::{
    Decisions, LiveRun, Memory, NextRun, Policy, Relabel, Wait, Workflows, decisions, next_run,
    open_pull_for, pending_spec_changes, worked_pulls,
};
use crate::runs::{InFlight, RunInFlight, RunOf};
use crate::webhook::Received;
use crate::workflow::{LastRuns, WorkflowFiles, WorkflowName};

#[derive(Clone, Debug)]
pub struct Serving {
    pub since: Timestamp,
    pub max_runs: usize,
}

#[derive(Clone, Debug)]
pub struct View {
    pub snapshot: Arc<Snapshot>,
    /// When refreshes read the items they read into the snapshot; the
    /// others were read by the last poll.
    pub read_at: HashMap<Item, Timestamp>,
    pub policy: Policy,
    pub polls: Polls,
    /// Holds the usage limit last refused for, whether or not it has reset.
    pub memory: Memory,
    /// The pipeline's runs in flight, as the reconcile step counts them: a
    /// cut that ended since the snapshot was taken counts too.
    pub live: Vec<LiveRun>,
    pub in_flight: Vec<InFlight>,
    pub workflows: WorkflowFiles,
    pub last_runs: LastRuns,
}

impl View {
    pub fn decisions(&self, now: Timestamp) -> Decisions {
        let workflows = Workflows {
            defined: self.workflows.parsed.clone(),
            last_runs: self.last_runs.clone(),
            in_flight: self.workflows_in_flight().collect(),
        };
        decisions(
            &self.snapshot,
            &self.live,
            &self.memory,
            &workflows,
            now,
            &self.policy,
        )
    }

    pub fn runs_in_flight(&self) -> usize {
        self.live.len() + self.workflows_in_flight().count()
    }

    fn workflows_in_flight(&self) -> impl Iterator<Item = WorkflowName> {
        self.in_flight.iter().filter_map(|run| match &run.id.of {
            RunOf::Workflow { workflow } => Some(workflow.clone()),
            RunOf::Work { .. } => None,
        })
    }

    fn has_run_in_flight(&self, number: Number) -> bool {
        self.in_flight
            .iter()
            .any(|run| run.id.of.item() == Some(number))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub linewright: Linewright,
    /// The runs in flight, oldest first.
    pub runs: Vec<RunInFlight>,
    /// The other items Linewright tracks: those with no run in flight.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Polls {
    pub last: Timestamp,
    pub next: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tracked {
    pub number: Number,
    pub title: String,
    #[serde(flatten)]
    pub state: ItemState,
    pub waiting_for: Vec<Wait>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum ItemState {
    WaitingForTickets,
    Tracking {
        tickets_closed: usize,
        tickets: usize,
    },
    WaitingForTriage,
    WaitingOnTheReporter,
    WithTheMaintainer,
    InReview(InReview),
    InReviewElsewhere {
        pull_request: Number,
        author: Login,
    },
    Claimed,
    ReadyToClaim,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InReview {
    pub pull_request: Number,
    pub draft: bool,
    pub ci: Ci,
}

impl InReview {
    pub(crate) fn of(pr: &Pull) -> InReview {
        let has = |state| pr.checks.iter().any(|c| c.state == state);
        InReview {
            pull_request: pr.number,
            draft: matches!(pr.state, PullState::Open { draft: true }),
            ci: if pr.checks.is_empty() {
                Ci::NoChecks
            } else if has(CheckState::Failed) {
                Ci::Failing
            } else if has(CheckState::Pending) {
                Ci::Running
            } else {
                Ci::Green
            },
        }
    }

    pub(crate) fn describe(&self) -> String {
        let stage = if self.draft {
            "draft"
        } else {
            "ready for review"
        };
        let ci = match self.ci {
            Ci::NoChecks => "no checks",
            Ci::Failing => "CI failing",
            Ci::Running => "CI running",
            Ci::Green => "CI green",
        };
        format!("in review: {} ({stage}, {ci})", self.pull_request)
    }
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "schedule", rename_all = "kebab-case")]
pub enum Runs {
    NotRunYet,
    Due {
        last_run: Timestamp,
    },
    Scheduled {
        last_run: Timestamp,
        next_run: Timestamp,
    },
    Ended {
        last_run: Timestamp,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnparsedWorkflow {
    pub path: String,
    pub reason: String,
}

/// `view` is `None` before the first poll.
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
            .and_then(|v| v.memory.usage_limit)
            .map(|limit| limit.resets_at)
            .filter(|resets_at| now < *resets_at),
        runs_in_flight: view.map_or(0, View::runs_in_flight),
        max_runs: serving.max_runs,
        webhooks,
    };
    let Some(view) = view else {
        return Status {
            linewright,
            runs: vec![],
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
    let mut runs: Vec<RunInFlight> = view.in_flight.iter().map(|run| run.at(now)).collect();
    runs.sort_by_key(|run| (run.started_at, run.id.to_string()));
    Status {
        linewright,
        runs,
        items: tracked(view, now)
            .into_iter()
            .filter(|item| !view.has_run_in_flight(item.number))
            .collect(),
        workflows,
        unparsed_workflows,
    }
}

pub(crate) fn tracked(view: &View, now: Timestamp) -> Vec<Tracked> {
    let snapshot = &view.snapshot;
    let mut waits = view.decisions(now).waits;
    let mut items = vec![];
    for pr in pending_spec_changes(snapshot, now, &view.policy) {
        items.push(Tracked {
            number: pr.number,
            title: pr.title.clone(),
            state: ItemState::WaitingForTickets,
            waiting_for: waits.remove(&pr.number).unwrap_or_default(),
        });
    }
    for issue in snapshot.issues.iter().filter(|i| i.open) {
        let waiting_for = waits.remove(&issue.number).unwrap_or_default();
        let Ok(state) = standing(issue, &view.snapshot, &view.policy, &waiting_for) else {
            continue;
        };
        items.push(Tracked {
            number: issue.number,
            title: issue.title.clone(),
            state,
            waiting_for,
        });
    }
    for (pr, _) in worked_pulls(&view.snapshot, &view.policy.linewright) {
        items.push(Tracked {
            number: pr.number,
            title: pr.title.clone(),
            state: ItemState::InReview(InReview::of(pr)),
            waiting_for: waits.remove(&pr.number).unwrap_or_default(),
        });
    }
    items
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum IssueExclusion {
    NoPipelineLabel,
    OwnIssueForTriage,
    NoTickets,
}

pub(crate) fn standing(
    issue: &Issue,
    snapshot: &Snapshot,
    policy: &Policy,
    waiting_for: &[Wait],
) -> Result<ItemState, IssueExclusion> {
    let state = issue_state(issue, snapshot, policy)?;
    if matches!(state, ItemState::Tracking { tickets: 0, .. }) && waiting_for.is_empty() {
        return Err(IssueExclusion::NoTickets);
    }
    Ok(state)
}

fn issue_state(
    issue: &Issue,
    snapshot: &Snapshot,
    policy: &Policy,
) -> Result<ItemState, IssueExclusion> {
    let open_pull = open_pull_for(snapshot, issue.number, &policy.linewright);
    let closing = snapshot.pulls_closing(issue);
    let open_closing = closing.iter().find(|p| p.is_open());
    let state = if matches!(issue.filed, Some(Filed::Tracking { .. })) {
        ItemState::Tracking {
            tickets_closed: issue.sub_issues.iter().filter(|t| !t.open).count(),
            tickets: issue.sub_issues.len(),
        }
    } else if let Some(pr) = open_pull {
        ItemState::InReview(InReview::of(pr))
    } else if let Some(pr) = open_closing {
        ItemState::InReviewElsewhere {
            pull_request: pr.number,
            author: pr.author.clone(),
        }
    } else if issue.has_label(label::NEEDS_TRIAGE) {
        if !policy.triages(issue) {
            return Err(IssueExclusion::OwnIssueForTriage);
        }
        ItemState::WaitingForTriage
    } else if issue.has_label(label::NEEDS_INFO) {
        ItemState::WaitingOnTheReporter
    } else if issue.has_label(label::READY_FOR_HUMAN) {
        ItemState::WithTheMaintainer
    } else if issue.has_label(label::READY_FOR_AGENT) {
        if issue.is_assigned(&policy.linewright) {
            ItemState::Claimed
        } else {
            ItemState::ReadyToClaim
        }
    } else {
        return Err(IssueExclusion::NoPipelineLabel);
    };
    Ok(state)
}

pub fn render(status: &Status, colour: bool) -> String {
    let paint = Paint(colour);
    let sections = [
        linewright_lines(&status.linewright, paint),
        status.runs.iter().map(|run| run.render(paint)).collect(),
        status
            .items
            .iter()
            .map(|item| {
                let waits = if item.waiting_for.is_empty() {
                    String::new()
                } else {
                    let waits: Vec<String> = item.waiting_for.iter().map(describe_wait).collect();
                    format!(" — waiting for {}", waits.join("; "))
                };
                format!(
                    "{}: {}{waits} — {}",
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

pub(crate) fn describe(state: &ItemState) -> String {
    match state {
        ItemState::WaitingForTickets => "spec change waiting for tickets".into(),
        ItemState::Tracking {
            tickets_closed,
            tickets,
        } => format!("tracking issue, {tickets_closed}/{tickets} tickets closed"),
        ItemState::WaitingForTriage => "waiting for triage".into(),
        ItemState::WaitingOnTheReporter => "waiting on the reporter".into(),
        ItemState::WithTheMaintainer => "with the maintainer".into(),
        ItemState::InReview(review) => review.describe(),
        ItemState::InReviewElsewhere {
            pull_request,
            author,
        } => format!("in review: {pull_request}, opened by {author}"),
        ItemState::Claimed => "claimed: being implemented".into(),
        ItemState::ReadyToClaim => "ready to claim".into(),
    }
}

/// What Linewright waits for, as a phrase after "waiting for".
pub(crate) fn describe_wait(wait: &Wait) -> String {
    match wait {
        Wait::Run { stage } => format!("its {} run, in flight", stage.name()),
        Wait::FreeRun {
            in_flight,
            max_runs,
        } => format!("a free run: {in_flight} of {max_runs} in flight"),
        Wait::UsageLimit { resets_at } => format!("the usage limit, until {}", utc(*resets_at)),
        Wait::Blocker { issue } => format!("{issue}, which blocks it"),
        Wait::ClosingPullRequest { pull_request } => format!("{pull_request}, which closes it"),
        Wait::PullRequest { pull_request } => {
            format!("{pull_request}, its pull request, until it is merged or closed")
        }
        Wait::Ci { pull_request } => format!("CI on {pull_request}"),
        Wait::MergeState { pull_request } => {
            format!("GitHub to work out whether {pull_request} can merge")
        }
        Wait::Tickets { open } => {
            let open: Vec<String> = open.iter().map(Number::to_string).collect();
            format!("its tickets: {} open", open.join(", "))
        }
        Wait::Reporter => format!("the reporter: labeled `{}`", label::NEEDS_INFO),
        Wait::Maintainer => format!(
            "the maintainer: to label it {}",
            label::ANSWERS
                .map(|(answer, _)| format!("`{answer}`"))
                .join(" or ")
        ),
        Wait::Unassigned => {
            "the maintainer: Linewright is unassigned, until they assign it again".into()
        }
        Wait::RequiredLabel { label } => {
            format!("the maintainer: to label it `{label}`, the required label")
        }
        Wait::Relabel { since } => {
            let since = match since {
                Relabel::Unassigned => "Linewright was unassigned".to_string(),
                Relabel::Merged { pull_request } => format!("{pull_request} merged"),
            };
            format!(
                "the maintainer: to label it `{}` again, since {since}",
                label::READY_FOR_AGENT
            )
        }
        Wait::Review { pull_request } => format!("the maintainer: to review {pull_request}"),
        Wait::Requirements {
            pull_request,
            requires,
        } => {
            let names: Vec<String> = requires.iter().map(ToString::to_string).collect();
            let required = match names.as_slice() {
                [] => "what the default branch's rules require".to_string(),
                [one] => one.clone(),
                [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
            };
            format!("{required}, before {pull_request} merges")
        }
    }
}

pub(crate) fn utc(time: Timestamp) -> String {
    time.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}
