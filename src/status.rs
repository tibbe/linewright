use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::num::NonZeroU64;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::automation::{Automation, AutomationFiles, AutomationName, LastScheduledRuns};
use crate::domain::{
    CheckState, Issue, Item, Login, MergedPull, Number, Pull, PullState, Requirement, Snapshot,
    StatusLabel, Timestamp, label,
};
use crate::phrase::Phrase;
use crate::reconcile::{
    Automations, Decisions, Drain, LiveRun, Memory, NextRun, NotSpecChange, Policy, Relabel,
    SpecChange, Wait, decisions, next_run, open_pull_for, spec_change, worked_pulls,
};
use crate::repo_files::{FileError, Unusable};
use crate::runs::{
    InFlight, Recorded, RunId, RunInFlight, RunOf, StartedBy, describe_start, last_of_automation,
};
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
    pub last_scheduled_runs: LastScheduledRuns,
    /// The files of the repository's Linewright cannot use, as the last
    /// reconcile read them.
    pub unusable: Vec<Unusable>,
}

impl View {
    pub fn decisions(&self, now: Timestamp) -> Decisions {
        let automations = Automations {
            defined: self.automations.parsed.clone(),
            last_scheduled_runs: self.last_scheduled_runs.clone(),
            in_flight: self.automations_in_flight().collect(),
        };
        decisions(
            &self.snapshot,
            &self.live,
            &self.memory,
            &automations,
            now,
            &self.policy,
            &self.unusable,
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
    /// `None` until the first poll ends; it is due at once.
    pub polls: Option<PollsShown>,
    /// Set only while polls have failed since the last that succeeded.
    /// Absent from the answers of a `serve` that does not count them.
    #[serde(default)]
    pub failed_polls: Option<FailedPolls>,
    /// Set only while a usage limit is in force.
    pub usage_limit_resets_at: Option<Timestamp>,
    /// The files of the repository's Linewright cannot use, by path.
    /// Absent from the answers of a `serve` that does not read them.
    #[serde(default)]
    pub unusable_files: Vec<FileError>,
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

/// The last poll that succeeded, `None` before the first, and the next
/// poll, as `status` shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollsShown {
    pub last: Option<Timestamp>,
    pub next: Timestamp,
}

/// Where `serve`'s polling stands once a poll has ended, kept apart from
/// its [`View`], which only a poll that succeeds makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Polling {
    pub next: Timestamp,
    /// `None` unless polls have failed since the last that succeeded.
    pub failed: Option<FailedPolls>,
}

/// The polls failed since the last that succeeded
/// (`docs/specs/monitoring.md`, status).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedPolls {
    pub count: NonZeroU64,
    /// When the first of them failed.
    pub since: Timestamp,
    /// When the latest of them failed.
    pub last_at: Timestamp,
    /// The latest's error, with Linewright's secrets redacted.
    pub last_error: String,
}

/// How a poll ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Poll {
    Succeeded,
    Failed { at: Timestamp, error: String },
}

impl FailedPolls {
    /// The polls failed since the last that succeeded, `previous` before
    /// `poll` and the returned after it.
    pub fn after(previous: Option<FailedPolls>, poll: Poll) -> Option<FailedPolls> {
        let Poll::Failed { at, error } = poll else {
            return None;
        };
        Some(match previous {
            None => FailedPolls {
                count: NonZeroU64::MIN,
                since: at,
                last_at: at,
                last_error: error,
            },
            Some(previous) => FailedPolls {
                count: previous.count.saturating_add(1),
                since: previous.since,
                last_at: at,
                last_error: error,
            },
        })
    }
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
    WaitingOnThePeopleAsked,
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
        Phrase::from("in review: ")
            .item(self.pull_request)
            .text(format!(" ({stage}, {})", self.ci.describe()))
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
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Ci::NoChecks => "no checks",
            Ci::Failing => "CI failing",
            Ci::Running => "CI running",
            Ci::Green => "CI green",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRuns {
    pub name: String,
    #[serde(flatten)]
    pub schedule: Schedule,
    /// `None` before its first run.
    #[serde(flatten)]
    pub last_run: Option<LastRun>,
}

/// Where an automation's schedule stands, counted from its last scheduled
/// run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "schedule", rename_all = "kebab-case")]
pub enum Schedule {
    /// No poll has seen the automation yet.
    NotRunYet,
    Due,
    Scheduled {
        next_run: Timestamp,
    },
    Ended,
}

/// An automation's last run: its newest, by its schedule or by hand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRun {
    #[serde(rename = "last_run")]
    pub started_at: Timestamp,
    #[serde(rename = "last_run_id")]
    pub id: RunId,
    #[serde(rename = "last_run_started_by")]
    pub started_by: StartedBy,
}

impl LastRun {
    fn of((run, started_by): (&Recorded, StartedBy)) -> LastRun {
        LastRun {
            started_at: run.started_at,
            id: run.id.clone(),
            started_by,
        }
    }
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

/// `polling` is `None` until the first poll ends; `view` is `None` before
/// the first poll that succeeded; `recorded` is every run's record;
/// `drain_deadline` is the deadline of the drain under way, if any.
pub fn status(
    serving: &Serving,
    webhooks: Option<Received>,
    polling: Option<&Polling>,
    view: Option<&View>,
    recorded: &[Recorded],
    drain_deadline: Option<Timestamp>,
    now: Timestamp,
) -> Status {
    let linewright = Linewright {
        serving_since: serving.since,
        polls: match (view, polling) {
            (Some(view), _) => Some(PollsShown {
                last: Some(view.polls.last),
                next: view.polls.next,
            }),
            (None, Some(polling)) => Some(PollsShown {
                last: None,
                next: polling.next,
            }),
            (None, None) => None,
        },
        failed_polls: polling.and_then(|polling| polling.failed.clone()),
        usage_limit_resets_at: view
            .and_then(|v| v.memory.usage_limit)
            .map(|limit| limit.resets_at)
            .filter(|resets_at| now < *resets_at),
        unusable_files: view.map_or_else(Vec::new, unusable_files),
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
            schedule: schedule_of(automation, &view.last_scheduled_runs, now),
            last_run: last_of_automation(recorded, &automation.name).map(LastRun::of),
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

/// The files of `view`'s repository Linewright cannot use, by path.
fn unusable_files(view: &View) -> Vec<FileError> {
    let mut files: Vec<FileError> = view
        .unusable
        .iter()
        .map(|unusable| unusable.file.clone())
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// Where `automation`'s schedule stands, counted from its last scheduled run.
pub(crate) fn schedule_of(
    automation: &Automation,
    last_scheduled_runs: &LastScheduledRuns,
    now: Timestamp,
) -> Schedule {
    match last_scheduled_runs.get(&automation.name) {
        None => Schedule::NotRunYet,
        Some(last_scheduled_run) => match next_run(automation, last_scheduled_run, now) {
            NextRun::Due => Schedule::Due,
            NextRun::At(next_run) => Schedule::Scheduled { next_run },
            NextRun::Never => Schedule::Ended,
        },
    }
}

pub(crate) fn managed(view: &View, now: Timestamp) -> Vec<Managed> {
    let snapshot = &view.snapshot;
    let mut waits = view.decisions(now).waits;
    let mut items = vec![];
    for pr in &snapshot.merged {
        let Ok((state, mut waiting_for)) = spec_change_standing(pr, snapshot, now, &view.policy)
        else {
            continue;
        };
        waiting_for.extend(waits.remove(&pr.number).unwrap_or_default());
        items.push(Managed {
            number: pr.number,
            title: pr.title.clone(),
            state,
            waiting_for,
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
    /// Its labels ask for a step, but it is not marked for Linewright, and
    /// Linewright has not worked it.
    NotMarked {
        label: String,
    },
}

pub(crate) fn standing(
    issue: &Issue,
    snapshot: &Snapshot,
    policy: &Policy,
    waiting_for: &[Wait],
) -> Result<ItemState, IssueExclusion> {
    let state = issue_state(issue, snapshot, policy)?;
    if let Some(label) = &policy.require_label
        && !policy.marks(&issue.labels)
        && !issue.worked_by(&policy.linewright, snapshot)
    {
        return Err(IssueExclusion::NotMarked {
            label: label.clone(),
        });
    }
    if matches!(state, ItemState::Tracking { tickets: 0, .. }) && waiting_for.is_empty() {
        return Err(IssueExclusion::NoTickets);
    }
    Ok(state)
}

/// Where merged `pr` stands as a spec change, with the waits its standing
/// implies alone, which the decisions' waits for it follow; or why it is no
/// spec change. A spec change Linewright was unassigned from has been
/// worked (`GLOSSARY.md`, **Managed**), so it stays managed, though
/// Linewright does not cut it.
pub(crate) fn spec_change_standing(
    pr: &MergedPull,
    snapshot: &Snapshot,
    now: Timestamp,
    policy: &Policy,
) -> Result<(ItemState, Vec<Wait>), NotSpecChange> {
    let waits = match spec_change(pr, &snapshot.default_branch, now, policy)? {
        SpecChange::Pending => vec![],
        SpecChange::Unassigned { required_label } => vec![Wait::Unassigned { required_label }],
    };
    Ok((ItemState::WaitingForDesign, waits))
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
            Some(StatusLabel::NeedsInfo) => ItemState::WaitingOnThePeopleAsked,
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

/// A fact about Linewright itself, of those `status` and the server page
/// show (`docs/specs/monitoring.md`, status, and
/// `docs/specs/design_system.md`, The server page), each in its own words.
pub(crate) enum LinewrightFact<'a> {
    ServingSince(Timestamp),
    /// The last poll and the next.
    Polls(Option<PollsShown>),
    FailedPolls(&'a FailedPolls),
    UsageLimit {
        resets_at: Timestamp,
    },
    UnusableFile(&'a FileError),
    Drain(DrainShown),
    RunsInFlight {
        runs: usize,
        max_runs: usize,
    },
    /// The last delivery accepted and the deliveries refused.
    Webhooks(Received),
}

impl Linewright {
    /// The facts shown of it, in the specs' order, each only while it shows.
    pub(crate) fn facts(&self) -> Vec<LinewrightFact<'_>> {
        [
            Some(LinewrightFact::ServingSince(self.serving_since)),
            Some(LinewrightFact::Polls(self.polls)),
            self.failed_polls.as_ref().map(LinewrightFact::FailedPolls),
            self.usage_limit_resets_at
                .map(|resets_at| LinewrightFact::UsageLimit { resets_at }),
        ]
        .into_iter()
        .chain(
            self.unusable_files
                .iter()
                .map(|file| Some(LinewrightFact::UnusableFile(file))),
        )
        .chain([
            self.drain.map(LinewrightFact::Drain),
            Some(LinewrightFact::RunsInFlight {
                runs: self.runs_in_flight,
                max_runs: self.max_runs,
            }),
            self.webhooks.map(LinewrightFact::Webhooks),
        ])
        .flatten()
        .collect()
    }
}

pub(crate) fn linewright_facts(linewright: &Linewright) -> Vec<Fact> {
    linewright
        .facts()
        .into_iter()
        .flat_map(|fact| match fact {
            LinewrightFact::ServingSince(since) => {
                vec![Fact::Plain(Phrase::from("serving since: ").time(since))]
            }
            LinewrightFact::Polls(polls) => vec![
                Fact::Plain(last_poll(polls)),
                Fact::Plain(match polls {
                    Some(polls) => Phrase::from("next poll: ").time(polls.next),
                    None => "next poll: due now".into(),
                }),
            ],
            LinewrightFact::FailedPolls(failed) => vec![Fact::Problem(polls_failed(failed))],
            LinewrightFact::UsageLimit { resets_at } => vec![Fact::Warning(usage_limit(resets_at))],
            LinewrightFact::UnusableFile(file) => vec![Fact::Problem(unusable_file(file))],
            LinewrightFact::Drain(drain) => vec![Fact::Warning(drain_under_way(drain))],
            LinewrightFact::RunsInFlight { runs, max_runs } => vec![Fact::Plain(
                format!("runs in flight: {runs} of {max_runs}").into(),
            )],
            LinewrightFact::Webhooks(received) => vec![
                Fact::Plain(match received.last_accepted {
                    Some(at) => Phrase::from("last webhook delivery accepted: ").time(at),
                    None => "last webhook delivery accepted: none yet".into(),
                }),
                Fact::Plain(format!("webhook deliveries refused: {}", received.refused).into()),
            ],
        })
        .collect()
}

fn last_poll(polls: Option<PollsShown>) -> Phrase {
    match polls.and_then(|polls| polls.last) {
        Some(last) => Phrase::from("last poll: ").time(last),
        None => "last poll: none yet".into(),
    }
}

fn polls_failed(failed: &FailedPolls) -> Phrase {
    let phrase = polls_failed_since(failed);
    let phrase = if failed.count == NonZeroU64::MIN {
        phrase
    } else {
        phrase.text(", the last at ").time(failed.last_at)
    };
    phrase.text(": ").text(&failed.last_error)
}

/// How many polls failed since the last that succeeded, and since when.
pub(crate) fn polls_failed_since(failed: &FailedPolls) -> Phrase {
    if failed.count == NonZeroU64::MIN {
        Phrase::from("polls failed: 1 at ").time(failed.since)
    } else {
        Phrase::from(format!("polls failed: {} since ", failed.count)).time(failed.since)
    }
}

pub(crate) fn usage_limit(resets_at: Timestamp) -> Phrase {
    Phrase::from("usage limit: in force until ").time(resets_at)
}

/// A file of the repository's Linewright cannot use, and its error.
pub(crate) fn unusable_file(file: &FileError) -> Phrase {
    Phrase::from("repository file Linewright cannot use: ").then(file_error(file))
}

/// A file's path and its error.
pub(crate) fn file_error(file: &FileError) -> Phrase {
    Phrase::default()
        .code(file.path.clone())
        .text(format!(": {}", file.error))
}

pub(crate) fn drain_under_way(drain: DrainShown) -> Phrase {
    let phrase = Phrase::from("drain: under way since ").time(drain.since);
    match drain.deadline {
        Some(deadline) => phrase.text("; deadline ").time(deadline),
        None => phrase,
    }
}

pub(crate) fn automation_facts(status: &Status) -> Vec<Fact> {
    let runs = status.automations.iter().flat_map(|automation| {
        let last = match &automation.last_run {
            None => "none yet".into(),
            Some(run) => Phrase::from(run.id.to_string()).then(describe_start(
                run.started_by == StartedBy::Hand,
                run.started_at,
            )),
        };
        let next = describe_next_run(automation.schedule);
        let name = &automation.name;
        let last = Phrase::from(format!("automation {name} last run: ")).then(last);
        let next = Phrase::from(format!("automation {name} next run: ")).then(next);
        [last, next].map(Fact::Plain)
    });
    let unparsed = status
        .unparsed_automations
        .iter()
        .map(|file| Fact::Problem(does_not_parse(file)));
    runs.chain(unparsed).collect()
}

/// What is wrong with an automation file that does not parse.
pub(crate) fn does_not_parse(file: &UnparsedAutomation) -> Phrase {
    format!("{}: does not parse: {}", file.path, file.reason).into()
}

/// When an automation runs next, from where its schedule stands.
pub(crate) fn describe_next_run(schedule: Schedule) -> Phrase {
    match schedule {
        Schedule::NotRunYet => {
            "not yet counted, its schedule starts when a poll first sees it".into()
        }
        Schedule::Due => "due now".into(),
        Schedule::Scheduled { next_run } => Phrase::default().time(next_run),
        Schedule::Ended => "none, its schedule ticks no more".into(),
    }
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
        ItemState::WaitingOnThePeopleAsked => "waiting on the people asked".into(),
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
        Wait::PeopleAsked => Phrase::from("the people asked: labeled ").code(label::NEEDS_INFO),
        Wait::Maintainer => Phrase::from("the maintainer: to label it ").then(Phrase::join(
            label::ANSWERS.map(|(answer, _)| Phrase::default().code(answer)),
            " or ",
        )),
        Wait::Unassigned { required_label } => {
            let phrase = Phrase::from(
                "the maintainer: Linewright is unassigned, until they assign it again",
            );
            match required_label {
                Some(label) => phrase
                    .text(" or label it ")
                    .code(label.clone())
                    .text(", the required label"),
                None => phrase,
            }
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
        Wait::RepositoryFile(FileError { path, error }) => {
            Phrase::from("the default branch to fix ")
                .code(path.clone())
                .text(format!(", which Linewright cannot use: {error}"))
        }
    }
}
