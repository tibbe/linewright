use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::automation::{AutomationFiles, AutomationName, LastRuns};
use crate::domain::{
    CheckState, Issue, Item, Login, Number, Pull, PullState, Requirement, Snapshot, StatusLabel,
    Timestamp, label,
};
use crate::phrase::{Phrase, Tone};
use crate::reconcile::{
    Automations, Decisions, Drain, LiveRun, Memory, NextRun, Policy, Relabel, Wait, decisions,
    next_run, open_pull_for, pending_spec_changes, worked_pulls,
};
use crate::runs::{InFlight, RunId, RunInFlight, RunOf};
use crate::webhook::Received;

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
    pub automations: AutomationFiles,
    pub last_runs: LastRuns,
}

impl View {
    pub fn decisions(&self, now: Timestamp) -> Decisions {
        let automations = Automations {
            defined: self.automations.parsed.clone(),
            last_runs: self.last_runs.clone(),
            in_flight: self.automations_in_flight().collect(),
        };
        decisions(
            &self.snapshot,
            &self.live,
            &self.memory,
            &automations,
            now,
            &self.policy,
        )
    }

    pub fn runs_in_flight(&self) -> usize {
        self.live.len() + self.automations_in_flight().count()
    }

    fn automations_in_flight(&self) -> impl Iterator<Item = AutomationName> {
        self.in_flight.iter().filter_map(|run| match &run.id.of {
            RunOf::Automation { automation } => Some(automation.clone()),
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
    /// The other managed items: those with no run in flight.
    pub items: Vec<Managed>,
    pub automations: Vec<AutomationRuns>,
    pub unparsed_automations: Vec<UnparsedAutomation>,
}

impl Status {
    pub fn run_ids(&self) -> Vec<RunId> {
        self.runs.iter().map(|run| run.id.clone()).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Linewright {
    pub serving_since: Timestamp,
    /// `None` before the first poll, which is due at once.
    pub polls: Option<Polls>,
    /// Set only while a usage limit is in force.
    pub usage_limit_resets_at: Option<Timestamp>,
    /// Set only while a drain is under way.
    pub drain: Option<DrainShown>,
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
pub struct Managed {
    pub number: Number,
    pub title: String,
    #[serde(flatten)]
    pub state: ItemState,
    pub waiting_for: Vec<Wait>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum ItemState {
    WaitingForDesign,
    Tracking {
        tickets_closed: usize,
        tickets: usize,
    },
    WaitingForTriage,
    BeingTriaged,
    WaitingOnTheReporter,
    WithTheMaintainer,
    InReview(InReview),
    InReviewElsewhere {
        pull_request: Number,
        author: Login,
    },
    /// Claimed, and not designed yet.
    BeingDesigned,
    /// Not claimed, and not designed yet.
    ReadyToDesign,
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

    pub(crate) fn describe(&self) -> Phrase {
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
        Phrase::from("in review: ")
            .item(self.pull_request)
            .text(format!(" ({stage}, {ci})"))
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

impl Ci {
    pub(crate) fn tone(self) -> Option<Tone> {
        match self {
            Ci::NoChecks => None,
            Ci::Failing => Some(Tone::Failure),
            Ci::Running => Some(Tone::InProgress),
            Ci::Green => Some(Tone::Success),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRuns {
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
pub struct UnparsedAutomation {
    pub path: String,
    pub reason: String,
}

/// A drain under way, as `status` shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrainShown {
    pub since: Timestamp,
    /// When the drain becomes a stop, if it has a deadline.
    pub deadline: Option<Timestamp>,
}

/// `view` is `None` before the first poll; `drain_deadline` is the deadline
/// of the drain under way, if any.
pub fn status(
    serving: &Serving,
    webhooks: Option<Received>,
    view: Option<&View>,
    drain_deadline: Option<Timestamp>,
    now: Timestamp,
) -> Status {
    let linewright = Linewright {
        serving_since: serving.since,
        polls: view.map(|v| v.polls),
        usage_limit_resets_at: view
            .and_then(|v| v.memory.usage_limit)
            .map(|limit| limit.resets_at)
            .filter(|resets_at| now < *resets_at),
        drain: view
            .and_then(|v| v.memory.drain)
            .map(|Drain { since }| DrainShown {
                since,
                deadline: drain_deadline,
            }),
        runs_in_flight: view.map_or(0, View::runs_in_flight),
        max_runs: serving.max_runs,
        webhooks,
    };
    let Some(view) = view else {
        return Status {
            linewright,
            runs: vec![],
            items: vec![],
            automations: vec![],
            unparsed_automations: vec![],
        };
    };
    let mut automations: Vec<AutomationRuns> = view
        .automations
        .parsed
        .iter()
        .map(|automation| AutomationRuns {
            name: automation.name.to_string(),
            runs: match view.last_runs.get(&automation.name) {
                None => Runs::NotRunYet,
                Some(last_run) => match next_run(automation, last_run, now) {
                    NextRun::Due => Runs::Due { last_run },
                    NextRun::At(next_run) => Runs::Scheduled { last_run, next_run },
                    NextRun::Never => Runs::Ended { last_run },
                },
            },
        })
        .collect();
    automations.sort_by(|a, b| a.name.cmp(&b.name));
    let mut unparsed_automations: Vec<UnparsedAutomation> = view
        .automations
        .unparsed
        .iter()
        .map(|file| UnparsedAutomation {
            path: file.path.clone(),
            reason: file.reason.clone(),
        })
        .collect();
    unparsed_automations.sort_by(|a, b| a.path.cmp(&b.path));
    let mut runs: Vec<RunInFlight> = view.in_flight.iter().map(|run| run.at(now)).collect();
    runs.sort_by_key(|run| (run.started_at, run.id.to_string()));
    Status {
        linewright,
        runs,
        items: managed(view, now)
            .into_iter()
            .filter(|item| !view.has_run_in_flight(item.number))
            .collect(),
        automations,
        unparsed_automations,
    }
}

pub(crate) fn managed(view: &View, now: Timestamp) -> Vec<Managed> {
    let snapshot = &view.snapshot;
    let mut waits = view.decisions(now).waits;
    let mut items = vec![];
    for pr in pending_spec_changes(snapshot, now, &view.policy) {
        items.push(Managed {
            number: pr.number,
            title: pr.title.clone(),
            state: ItemState::WaitingForDesign,
            waiting_for: waits.remove(&pr.number).unwrap_or_default(),
        });
    }
    for issue in snapshot.issues.iter().filter(|i| i.open) {
        let waiting_for = waits.remove(&issue.number).unwrap_or_default();
        let Ok(state) = standing(issue, &view.snapshot, &view.policy, &waiting_for) else {
            continue;
        };
        items.push(Managed {
            number: issue.number,
            title: issue.title.clone(),
            state,
            waiting_for,
        });
    }
    for (pr, _) in worked_pulls(&view.snapshot, &view.policy.linewright) {
        items.push(Managed {
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
    let state = if issue.tracks(&policy.linewright).is_some() {
        ItemState::Tracking {
            tickets_closed: issue.sub_issues.iter().filter(|t| !t.linked.open).count(),
            tickets: issue.sub_issues.len(),
        }
    } else if let Some(pr) = open_pull {
        ItemState::InReview(InReview::of(pr))
    } else if let Some(pr) = open_closing {
        ItemState::InReviewElsewhere {
            pull_request: pr.number,
            author: pr.author.clone(),
        }
    } else {
        match issue.status_label() {
            Some(StatusLabel::NeedsTriage) if issue.is_assigned(&policy.linewright) => {
                ItemState::BeingTriaged
            }
            Some(StatusLabel::NeedsTriage) => ItemState::WaitingForTriage,
            Some(StatusLabel::NeedsInfo) => ItemState::WaitingOnTheReporter,
            Some(StatusLabel::ReadyForHuman) => ItemState::WithTheMaintainer,
            Some(StatusLabel::ReadyForAgent) => {
                match (
                    issue.is_assigned(&policy.linewright),
                    issue.has_label(label::DESIGNED),
                ) {
                    (true, true) => ItemState::Claimed,
                    (true, false) => ItemState::BeingDesigned,
                    (false, true) => ItemState::ReadyToClaim,
                    (false, false) => ItemState::ReadyToDesign,
                }
            }
            None => return Err(IssueExclusion::NoPipelineLabel),
        }
    };
    Ok(state)
}

pub fn render(status: &Status, colour: bool) -> String {
    let paint = Paint(colour);
    let sections: [Vec<String>; 4] = [
        linewright_facts(&status.linewright)
            .iter()
            .map(|fact| fact.render(paint))
            .collect(),
        status.runs.iter().map(|run| run.render(paint)).collect(),
        status
            .items
            .iter()
            .map(|item| {
                let mut line = describe(&item.state);
                if !item.waiting_for.is_empty() {
                    let waits = item.waiting_for.iter().map(describe_wait);
                    line = line.text(" — waiting for ").then(Phrase::join(waits, "; "));
                }
                let line = line.text(" — ").text(&item.title);
                format!("{}: {line}", paint.bold(item.number))
            })
            .collect(),
        automation_facts(status)
            .iter()
            .map(|fact| fact.render(paint))
            .collect(),
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
    fn with(self, code: &str, text: impl fmt::Display) -> String {
        if self.0 {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub(crate) fn bold(self, text: impl fmt::Display) -> String {
        self.with("1", text)
    }

    pub(crate) fn yellow(self, text: impl fmt::Display) -> String {
        self.with("33", text)
    }

    pub(crate) fn red(self, text: impl fmt::Display) -> String {
        self.with("31", text)
    }
}

/// A line about Linewright itself or its automations, and how much it asks
/// for the reader's attention: a warning is a usage limit in force or a
/// drain under way.
pub(crate) enum Fact {
    Plain(Phrase),
    Warning(Phrase),
    Problem(Phrase),
}

impl Fact {
    fn render(&self, paint: Paint) -> String {
        match self {
            Fact::Plain(phrase) => phrase.to_string(),
            Fact::Warning(phrase) => paint.yellow(phrase),
            Fact::Problem(phrase) => paint.red(phrase),
        }
    }
}

pub(crate) fn linewright_facts(linewright: &Linewright) -> Vec<Fact> {
    let polls = linewright.polls;
    let mut facts = vec![
        Fact::Plain(Phrase::from("serving since: ").time(linewright.serving_since)),
        Fact::Plain(match polls {
            Some(polls) => Phrase::from("last poll: ").time(polls.last),
            None => "last poll: none yet".into(),
        }),
        Fact::Plain(match polls {
            Some(polls) => Phrase::from("next poll: ").time(polls.next),
            None => "next poll: due now".into(),
        }),
    ];
    if let Some(resets_at) = linewright.usage_limit_resets_at {
        facts.push(Fact::Warning(
            Phrase::from("usage limit: in force until ").time(resets_at),
        ));
    }
    if let Some(drain) = linewright.drain {
        let mut phrase = Phrase::from("drain: under way since ").time(drain.since);
        if let Some(deadline) = drain.deadline {
            phrase = phrase.text("; deadline ").time(deadline);
        }
        facts.push(Fact::Warning(phrase));
    }
    facts.push(Fact::Plain(
        format!(
            "runs in flight: {} of {}",
            linewright.runs_in_flight, linewright.max_runs
        )
        .into(),
    ));
    if let Some(received) = linewright.webhooks {
        facts.push(Fact::Plain(match received.last_accepted {
            Some(at) => Phrase::from("last delivery accepted: ").time(at),
            None => "last delivery accepted: none yet".into(),
        }));
        facts.push(Fact::Plain(
            format!("deliveries refused: {}", received.refused).into(),
        ));
    }
    facts
}

pub(crate) fn automation_facts(status: &Status) -> Vec<Fact> {
    let runs = status.automations.iter().flat_map(|automation| {
        let (last, next) = match automation.runs {
            Runs::NotRunYet => (
                "none yet, its schedule starts when a poll first sees it".into(),
                None,
            ),
            Runs::Due { last_run } => (Phrase::default().time(last_run), Some("due now".into())),
            Runs::Scheduled { last_run, next_run } => (
                Phrase::default().time(last_run),
                Some(Phrase::default().time(next_run)),
            ),
            Runs::Ended { last_run } => (
                Phrase::default().time(last_run),
                Some("none, its schedule ticks no more".into()),
            ),
        };
        let name = &automation.name;
        let last = Phrase::from(format!("automation {name} last run: ")).then(last);
        let next =
            next.map(|next| Phrase::from(format!("automation {name} next run: ")).then(next));
        std::iter::once(last).chain(next).map(Fact::Plain)
    });
    let unparsed = status.unparsed_automations.iter().map(|file| {
        Fact::Problem(format!("{}: does not parse: {}", file.path, file.reason).into())
    });
    runs.chain(unparsed).collect()
}

pub(crate) fn describe(state: &ItemState) -> Phrase {
    match state {
        ItemState::WaitingForDesign => "spec change waiting for its design".into(),
        ItemState::Tracking {
            tickets_closed,
            tickets,
        } => format!("tracking issue, {tickets_closed}/{tickets} tickets closed").into(),
        ItemState::WaitingForTriage => "waiting for triage".into(),
        ItemState::BeingTriaged => "claimed: being triaged".into(),
        ItemState::WaitingOnTheReporter => "waiting on the reporter".into(),
        ItemState::WithTheMaintainer => "with the maintainer".into(),
        ItemState::InReview(review) => review.describe(),
        ItemState::InReviewElsewhere {
            pull_request,
            author,
        } => Phrase::from("in review: ")
            .item(*pull_request)
            .text(format!(", opened by {author}")),
        ItemState::BeingDesigned => "claimed: being designed".into(),
        ItemState::ReadyToDesign => "ready to claim, waiting for its design".into(),
        ItemState::Claimed => "claimed: being implemented".into(),
        ItemState::ReadyToClaim => "ready to claim".into(),
    }
}

/// What Linewright waits for, as a phrase after "waiting for".
pub(crate) fn describe_wait(wait: &Wait) -> Phrase {
    match wait {
        Wait::Run { stage } => format!("its {} run, in flight", stage.name()).into(),
        Wait::FreeRun {
            in_flight,
            max_runs,
        } => format!("a free run: {in_flight} of {max_runs} in flight").into(),
        Wait::UsageLimit { resets_at } => Phrase::from("the usage limit, until ").time(*resets_at),
        Wait::Drain => Phrase::from("the drain, until ")
            .code("serve")
            .text(" restarts"),
        Wait::Blocker { issue } => Phrase::from(*issue).text(", which blocks it"),
        Wait::Assignee { assignee } => format!("{assignee}, who is assigned to it").into(),
        Wait::ClosingPullRequest { pull_request } => {
            Phrase::from(*pull_request).text(", which closes it")
        }
        Wait::PullRequest { pull_request } => {
            Phrase::from(*pull_request).text(", its pull request, until it is merged or closed")
        }
        Wait::Ci { pull_request } => Phrase::from("CI on ").item(*pull_request),
        Wait::MergeState { pull_request } => Phrase::from("GitHub to work out whether ")
            .item(*pull_request)
            .text(" can merge"),
        Wait::Tickets { open } => Phrase::from("its tickets: ")
            .then(Phrase::join(open.iter().copied().map(Phrase::from), ", "))
            .text(" open"),
        Wait::Reporter => Phrase::from("the reporter: labeled ").code(label::NEEDS_INFO),
        Wait::Maintainer => Phrase::from("the maintainer: to label it ").then(Phrase::join(
            label::ANSWERS.map(|(answer, _)| Phrase::default().code(answer)),
            " or ",
        )),
        Wait::Unassigned => {
            "the maintainer: Linewright is unassigned, until they assign it again".into()
        }
        Wait::RequiredLabel { label } => Phrase::from("the maintainer: to label it ")
            .code(label.clone())
            .text(", the required label"),
        Wait::Relabel { label, since } => {
            let mut phrase = Phrase::from("the maintainer: to label it ").code(label.to_string());
            if let Relabel::Unassigned {
                required_label: Some(required),
            } = since
            {
                phrase = phrase
                    .text(" or ")
                    .code(required.clone())
                    .text(", the required label,");
            }
            let phrase = phrase.text(" again, since ");
            match since {
                Relabel::Unassigned { .. } => phrase.text("Linewright was unassigned"),
                Relabel::Merged { pull_request } => phrase.item(*pull_request).text(" merged"),
            }
        }
        Wait::Review { pull_request } => {
            Phrase::from("the maintainer: to review ").item(*pull_request)
        }
        Wait::Requirements {
            pull_request,
            requires,
        } => {
            let required = if requires.is_empty() {
                "what the default branch's rules require".into()
            } else {
                Phrase::list(requires.iter().map(Requirement::describe), "and")
            };
            required
                .text(", before ")
                .item(*pull_request)
                .text(" merges")
        }
    }
}
