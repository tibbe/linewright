use std::borrow::Cow;
use std::collections::BTreeMap;

use chrono::TimeDelta;
use serde::{Deserialize, Serialize};

use crate::automation::{Automation, AutomationName, LastRuns};
use crate::domain::{
    Assignment, BranchIssue, CheckState, ClosingPull, Comment, DesignedLabel, Filed, Issue, Item,
    Linked, Login, MergeMethod, MergeMethods, Mergeability, MergedPull, Number, Pull, PullState,
    Rebaseability, Requirement, Review, ReviewState, Snapshot, StatusLabel, Thread,
    TicketBranchName, Timestamp, UpdateMethod, UpdateRule, label, last_assigned, last_assignment,
    last_labeling, last_unassignment,
};
use crate::harness::UsageLimit;
use crate::holds::Holds;
use crate::spec_glob::SpecGlob;

#[derive(Clone, Debug)]
pub struct Policy {
    pub linewright: Login,
    pub spec_glob: SpecGlob,
    pub spec_lookback: TimeDelta,
    pub max_runs: usize,
    pub ci_grace: TimeDelta,
    pub ci_timeout: TimeDelta,
    pub ci_attempts: u32,
    pub auto_merge: Option<MergeMethod>,
    pub update_rule: UpdateRule,
    pub allowed_merge_methods: MergeMethods,
    pub require_label: Option<String>,
}

impl Policy {
    fn may_start_on(&self, issue: &Issue) -> bool {
        self.require_label
            .as_ref()
            .is_none_or(|l| issue.has_label(l))
    }

    /// A merge commit that resolved a conflict is the usual cause of GitHub
    /// reporting a pull request not rebaseable, and a rebase drops it.
    pub fn rebases_unrebaseable(&self) -> bool {
        self.allowed_merge_methods.contains(MergeMethod::Rebase)
            && self.update_rule != UpdateRule::Always(UpdateMethod::Merge)
    }
}

/// At most one run per subject is in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subject {
    Issue(Number),
    SpecChange(Number),
    Pull(Number),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Work {
    Triage {
        issue: Number,
    },
    Design {
        pr: Number,
    },
    Implement {
        issue: Number,
    },
    Rework {
        issue: Number,
        pr: Number,
    },
    FixCi {
        issue: Number,
        pr: Number,
        failing: Vec<String>,
    },
    Update {
        issue: Number,
        pr: Number,
    },
}

impl Subject {
    pub fn number(self) -> Number {
        match self {
            Subject::Issue(number) | Subject::SpecChange(number) | Subject::Pull(number) => number,
        }
    }
}

impl Work {
    pub fn subject(&self) -> Subject {
        match self {
            Work::Triage { issue } | Work::Implement { issue } => Subject::Issue(*issue),
            Work::Design { pr } => Subject::SpecChange(*pr),
            Work::Rework { pr, .. } | Work::FixCi { pr, .. } | Work::Update { pr, .. } => {
                Subject::Pull(*pr)
            }
        }
    }

    pub fn item(&self) -> Number {
        match self {
            Work::Triage { issue } | Work::Implement { issue } => *issue,
            Work::Design { pr }
            | Work::Rework { pr, .. }
            | Work::FixCi { pr, .. }
            | Work::Update { pr, .. } => *pr,
        }
    }

    pub fn starts_from(&self) -> Item {
        match self {
            Work::Triage { issue } | Work::Implement { issue } => Item::Issue(*issue),
            Work::Design { pr }
            | Work::Rework { pr, .. }
            | Work::FixCi { pr, .. }
            | Work::Update { pr, .. } => Item::Pull(*pr),
        }
    }

    pub fn kind(&self) -> WorkKind {
        match self {
            Work::Triage { .. } => WorkKind::Triage,
            Work::Design { .. } => WorkKind::Design,
            Work::Implement { .. } => WorkKind::Implement,
            Work::Rework { .. } => WorkKind::Rework,
            Work::FixCi { .. } => WorkKind::FixCi,
            Work::Update { .. } => WorkKind::Update,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "&'static str", try_from = "String")]
pub enum WorkKind {
    Triage,
    Design,
    Implement,
    Rework,
    FixCi,
    Update,
}

impl WorkKind {
    pub const ALL: [WorkKind; 6] = [
        WorkKind::Triage,
        WorkKind::Design,
        WorkKind::Implement,
        WorkKind::Rework,
        WorkKind::FixCi,
        WorkKind::Update,
    ];

    pub fn is_on_pull(self) -> bool {
        match self {
            WorkKind::Rework | WorkKind::FixCi | WorkKind::Update => true,
            WorkKind::Triage | WorkKind::Design | WorkKind::Implement => false,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            WorkKind::Triage => "triage",
            WorkKind::Design => "design",
            WorkKind::Implement => "implement",
            WorkKind::Rework => "rework",
            WorkKind::FixCi => "fix-ci",
            WorkKind::Update => "update",
        }
    }
}

impl From<WorkKind> for &'static str {
    fn from(kind: WorkKind) -> &'static str {
        kind.name()
    }
}

/// What the design stage was called before the Design area, in the run
/// records an earlier release wrote: read as the design stage, never written.
pub const OLD_DESIGN: &str = "tickets";

impl TryFrom<String> for WorkKind {
    type Error = String;

    fn try_from(name: String) -> Result<WorkKind, String> {
        match name.as_str() {
            OLD_DESIGN => Ok(WorkKind::Design),
            _ => name.parse(),
        }
    }
}

impl std::str::FromStr for WorkKind {
    type Err = String;

    fn from_str(name: &str) -> Result<WorkKind, String> {
        WorkKind::ALL
            .into_iter()
            .find(|kind| kind.name() == name)
            .ok_or_else(|| {
                let names: Vec<&str> = WorkKind::ALL.map(WorkKind::name).to_vec();
                format!("{name:?} is not a stage: one of {}", names.join(", "))
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveRun {
    pub subject: Subject,
    pub kind: WorkKind,
}

#[derive(Clone, Debug, Default)]
pub struct Memory {
    /// Oldest first.
    pub failed_merges: Vec<FailedMerge>,
    pub tried_rebases: Vec<TriedRebase>,
    pub done_runs: Vec<DoneRun>,
    pub held: Holds,
    pub usage_limit: Option<UsageLimit>,
    /// Set from when a drain of `serve` begins until it exits.
    pub drain: Option<Drain>,
    /// The cuts into tickets that ended since the snapshot was taken. The
    /// snapshot may miss what they wrote, so each counts as in flight for
    /// what starts, though there is nothing left of it to cancel.
    pub ended_cuts: Vec<Number>,
}

impl Memory {
    pub fn retain_open(&mut self, snapshot: &Snapshot) {
        let open = |pr| snapshot.pull(pr).is_some_and(Pull::is_open);
        self.failed_merges.retain(|f| open(f.pr));
        self.tried_rebases.retain(|r| open(r.pr));
        self.done_runs.retain(|r| open(r.pr));
        self.held.retain(open);
    }

    pub fn last_done(&self, pr: Number, kind: WorkKind) -> Option<Timestamp> {
        self.done_runs_of(pr, kind).map(|r| r.done_at).max()
    }

    fn done_runs_of(&self, pr: Number, kind: WorkKind) -> impl Iterator<Item = &DoneRun> {
        self.done_runs
            .iter()
            .filter(move |r| r.pr == pr && r.kind == kind)
    }

    pub fn holds_starts(&self, now: Timestamp) -> bool {
        self.usage_limit.is_some_and(|limit| now < limit.resets_at)
    }

    /// The latest reset wins. Returns whether the hold changed.
    pub fn hold_until(&mut self, resets_at: Timestamp) -> bool {
        if self
            .usage_limit
            .is_some_and(|held| held.resets_at >= resets_at)
        {
            return false;
        }
        self.usage_limit = Some(UsageLimit { resets_at });
        true
    }

    /// Returns the reset it lifted.
    pub fn lift_usage_limit(&mut self, now: Timestamp) -> Option<Timestamp> {
        if self.holds_starts(now) {
            return None;
        }
        self.usage_limit.take().map(|limit| limit.resets_at)
    }
}

/// A drain of `serve` under way: no item is claimed and no run starts
/// (`docs/specs/operation.md`, "Stopping").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drain {
    pub since: Timestamp,
}

/// The newest, when several close the issue.
pub fn open_pull_for<'a>(
    snapshot: &'a Snapshot,
    issue: Number,
    linewright: &'a Login,
) -> Option<&'a Pull> {
    snapshot
        .pulls
        .iter()
        .filter(|p| &p.author == linewright && p.is_open() && p.closes.contains(&issue))
        .max_by_key(|p| p.number)
}

pub fn ticket_of(snapshot: &Snapshot, pull: &Pull) -> Option<Number> {
    let held = pull.closes.iter().find(|&&n| snapshot.issue(n).is_some());
    held.or(pull.closes.first()).copied()
}

pub fn worked_pulls<'a>(
    snapshot: &'a Snapshot,
    linewright: &'a Login,
) -> impl Iterator<Item = (&'a Pull, Number)> + 'a {
    snapshot
        .pulls
        .iter()
        .filter(|p| p.is_open())
        .filter_map(move |p| Some((p, worked_ticket(snapshot, p, linewright).ok()?)))
}

pub fn worked_ticket(
    snapshot: &Snapshot,
    pull: &Pull,
    linewright: &Login,
) -> Result<Number, NotWorked> {
    if &pull.author != linewright {
        return Err(NotWorked::NotLinewrights);
    }
    let ticket = ticket_of(snapshot, pull).ok_or(NotWorked::ForNoIssue)?;
    match open_pull_for(snapshot, ticket, linewright).filter(|newest| newest.number != pull.number)
    {
        Some(newer) => Err(NotWorked::Superseded {
            ticket,
            by: newer.number,
        }),
        None => Ok(ticket),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotWorked {
    NotLinewrights,
    ForNoIssue,
    Superseded { ticket: Number, by: Number },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedMerge {
    pub pr: Number,
    pub reason: String,
    pub at: Timestamp,
}

/// Remembered so that a rebase that leaves the branch as it was is not
/// tried again at every poll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriedRebase {
    pub pr: Number,
    pub head: String,
    pub at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoneRun {
    pub pr: Number,
    pub kind: WorkKind,
    /// For a rework, when it last read the feedback it answered; otherwise
    /// when it ended.
    pub done_at: Timestamp,
}

#[derive(Clone, Debug, Default)]
pub struct Automations {
    pub defined: Vec<Automation>,
    pub last_runs: LastRuns,
    pub in_flight: Vec<AutomationName>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Start(Work),
    Cancel {
        subject: Subject,
        reason: String,
    },
    Claim {
        item: Item,
    },
    /// Post `comment` when there is one, then unassign Linewright from an
    /// open issue whose label does not ask for its work.
    LetGo {
        issue: Number,
        comment: Option<String>,
    },
    Relabel {
        issue: Number,
        remove: String,
        add: String,
    },
    /// Unassign the people Linewright asked a `needs-info` question, once
    /// the issue's label asks for Linewright's work again.
    UnassignAsked {
        issue: Number,
        assignees: Vec<Login>,
    },
    /// Remove a status label besides the one the issue holds.
    RemoveStatusLabel {
        issue: Number,
        label: StatusLabel,
    },
    HandBack {
        item: Item,
        reason: String,
    },
    MarkReady {
        pr: Number,
    },
    RememberRebase(TriedRebase),
    /// Only while its head is still `head`.
    Merge {
        pr: Number,
        head: String,
        draft: bool,
        method: MergeMethod,
    },
    /// Mark the draft ready for review, so that the store reports its merge
    /// state.
    MarkReadyForMergeState {
        pr: Number,
    },
    HoldForReview {
        pr: Number,
        draft: bool,
        requires: Vec<Requirement>,
    },
    CloseTracking {
        issue: Number,
    },
    DiscardTickets {
        tracking: Number,
        tickets: Vec<Number>,
    },
    DeleteBranch {
        branch: TicketBranchName,
    },
    RecordLastRuns(LastRuns),
    RunAutomation(Box<Automation>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "wait", rename_all = "kebab-case")]
pub enum Wait {
    Run {
        stage: WorkKind,
    },
    /// `in_flight` counts the runs the poll starts.
    FreeRun {
        in_flight: usize,
        max_runs: usize,
    },
    UsageLimit {
        resets_at: Timestamp,
    },
    /// Ends only with a restart of `serve`.
    Drain,
    Blocker {
        issue: Number,
    },
    /// Someone else is working on the issue.
    Assignee {
        assignee: Login,
    },
    ClosingPullRequest {
        pull_request: Number,
    },
    PullRequest {
        pull_request: Number,
    },
    Ci {
        pull_request: Number,
    },
    MergeState {
        pull_request: Number,
    },
    Tickets {
        open: Vec<Number>,
    },
    Reporter,
    Maintainer,
    /// Linewright is not assigned to the pull request.
    Unassigned,
    RequiredLabel {
        label: String,
    },
    /// For `label` applied again since an event.
    Relabel {
        label: StatusLabel,
        since: Relabel,
    },
    Review {
        pull_request: Number,
    },
    Requirements {
        pull_request: Number,
        requires: Vec<Requirement>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Relabel {
    /// Applying `required_label`, when set, counts as applying the label again.
    Unassigned {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        required_label: Option<String>,
    },
    Merged {
        pull_request: Number,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decisions {
    pub actions: Vec<Action>,
    pub waits: BTreeMap<Number, Vec<Wait>>,
}

pub fn reconcile(
    snapshot: &Snapshot,
    live: &[LiveRun],
    memory: &Memory,
    automations: &Automations,
    now: Timestamp,
    policy: &Policy,
) -> Vec<Action> {
    decisions(snapshot, live, memory, automations, now, policy).actions
}

pub fn decisions(
    snapshot: &Snapshot,
    live: &[LiveRun],
    memory: &Memory,
    automations: &Automations,
    now: Timestamp,
    policy: &Policy,
) -> Decisions {
    let in_flight = live.len() + automations.in_flight.len();
    let usage_limit = memory.usage_limit.filter(|_| memory.holds_starts(now));
    let (room, held) = match (memory.drain, usage_limit) {
        // The drain outlasts any usage limit.
        (Some(_), _) => (0, Held::Drain),
        (None, Some(UsageLimit { resets_at })) => (0, Held::UsageLimit { resets_at }),
        (None, None) => (
            policy.max_runs.saturating_sub(in_flight),
            Held::Ceiling {
                in_flight,
                max_runs: policy.max_runs,
            },
        ),
    };
    let mut plan = Plan {
        now,
        held,
        actions: vec![],
        starts: vec![],
        waits: BTreeMap::new(),
    };
    let running = |subject: Subject| {
        let ended_cut = match subject {
            Subject::SpecChange(pr) => memory.ended_cuts.contains(&pr),
            Subject::Issue(_) | Subject::Pull(_) => false,
        };
        live.iter()
            .find(|run| run.subject == subject)
            .map(|run| run.kind)
            .or(ended_cut.then_some(WorkKind::Design))
    };

    for run in live {
        if let Some(reason) = cancel_reason(run, snapshot, policy) {
            plan.act(Action::Cancel {
                subject: run.subject,
                reason,
            });
        }
    }

    // The pull request is labeled designed last, so a tracking issue
    // without the label is a decomposition that stopped part way: one
    // interrupted, cancelled or handed back.
    let mut discarded = vec![];
    for pr in snapshot
        .merged
        .iter()
        .filter(|pr| pr.designed_label().is_none())
    {
        if running(Subject::SpecChange(pr.number)).is_some() {
            continue;
        }
        let partial = snapshot.issues.iter().find(|i| {
            i.open
                && i.filed
                    == Some(Filed::Tracking {
                        spec_change: pr.number,
                    })
        });
        if let Some(partial) = partial {
            plan.act(Action::DiscardTickets {
                tracking: partial.number,
                tickets: open_numbers(&partial.sub_issues),
            });
            discarded.push(pr.number);
        }
    }

    let spec_changes = pending_spec_changes(snapshot, now, policy);
    let pending: Vec<Number> = spec_changes.iter().map(|pr| pr.number).collect();
    for pr in spec_changes {
        let work = Work::Design { pr: pr.number };
        if let Some(stage) = running(Subject::SpecChange(pr.number)) {
            plan.wait(pr.number, Wait::Run { stage });
        } else if discarded.contains(&pr.number) {
            // Cut again once the discarded set is closed.
        } else if pr.is_assigned(&policy.linewright) {
            plan.start(work);
        } else {
            plan.claim_and_start(Item::Pull(pr.number), work);
        }
    }

    for (pr, ticket) in worked_pulls(snapshot, &policy.linewright) {
        match pull_claim(pr, &policy.linewright) {
            PullClaim::Claimed => {}
            PullClaim::NeverAssigned => plan.claim(Item::Pull(pr.number)),
            PullClaim::Unassigned => {
                plan.wait(pr.number, Wait::Unassigned);
                continue;
            }
        }
        if let Some(stage) = running(Subject::Pull(pr.number)) {
            plan.wait(pr.number, Wait::Run { stage });
        } else {
            in_review(ticket, pr, memory, policy, &mut plan);
        }
    }

    for issue in snapshot.issues.iter().filter(|i| i.open) {
        let subject = Subject::Issue(issue.number);
        let tracks = match issue.filed {
            Some(Filed::Tracking { spec_change }) => Some(spec_change),
            _ => None,
        };
        let cut = tracks.and_then(|spec_change| running(Subject::SpecChange(spec_change)));
        if let Some(stage) = cut {
            plan.wait(issue.number, Wait::Run { stage });
        }
        let cutting = cut.is_some()
            || tracks.is_some_and(|sc| pending.contains(&sc) || discarded.contains(&sc));
        if tracks.is_some() && !cutting && !issue.sub_issues.is_empty() {
            let open = open_numbers(&issue.sub_issues);
            if open.is_empty() {
                plan.act(Action::CloseTracking {
                    issue: issue.number,
                });
            } else {
                plan.wait(issue.number, Wait::Tickets { open });
            }
        }
        for label in issue.stale_status_labels() {
            plan.act(Action::RemoveStatusLabel {
                issue: issue.number,
                label,
            });
        }
        let replied = has_reply_to_needs_info(issue, policy);
        let asked = if replied {
            Ok(Asked::Triage)
        } else {
            asked_work(issue.status_label())
        };
        let issue = &*let_go_of_people_asked(issue, asked, policy, &mut plan);
        if replied {
            plan.act(Action::Relabel {
                issue: issue.number,
                remove: label::NEEDS_INFO.into(),
                add: label::NEEDS_TRIAGE.into(),
            });
        }
        if let Err(unasked) = asked
            && issue.is_assigned(&policy.linewright)
        {
            plan.act(Action::LetGo {
                issue: issue.number,
                comment: answers_hand_assignment(issue, &policy.linewright)
                    .then(|| let_go_comment(unasked, &policy.linewright)),
            });
        }
        let closing = snapshot.pulls_closing(issue);
        let open_closing: Vec<&ClosingPull> = closing.iter().filter(|p| p.is_open()).collect();
        if !open_closing.is_empty() {
            if let Some(stage) = running(subject) {
                plan.wait(issue.number, Wait::Run { stage });
            }
            let mut linewrights = false;
            for pull in open_closing {
                let pull_request = pull.number;
                let wait = if pull.author == policy.linewright {
                    linewrights = true;
                    Wait::PullRequest { pull_request }
                } else {
                    Wait::ClosingPullRequest { pull_request }
                };
                plan.wait(issue.number, wait);
            }
            // Someone else's pull request may be closed with the ticket left
            // to Linewright, which then waits for its blockers; once
            // Linewright's ends, they hold nothing back.
            if !linewrights && issue.holds(StatusLabel::ReadyForAgent) {
                wait_for_blockers(issue, &mut plan);
            }
        } else {
            match asked {
                Ok(Asked::Triage) => triage(issue, replied, running(subject), policy, &mut plan),
                Ok(Asked::Implementation) => {
                    implementation(issue, &closing, running(subject), policy, &mut plan);
                }
                Err(_) => {
                    for wait in labeled_waits(issue, policy) {
                        plan.wait(issue.number, wait);
                    }
                }
            }
        }
    }

    for branch in &snapshot.ticket_branches {
        let number = branch.name.issue();
        // An issue read since the branch was, by a refresh, is as the
        // snapshot holds it; one that left the snapshot is not known closed.
        let read = match &branch.issue {
            BranchIssue::Held => None,
            BranchIssue::Read(issue) => Some(&**issue),
        };
        let closed_with_no_pull = snapshot
            .issue(number)
            .or(read)
            .is_some_and(Issue::closed_with_no_open_pull);
        // Closing the ticket cancels its run, which pushes its work as it
        // stops: the branch waits for the run to end.
        if closed_with_no_pull
            && !branch.open_pull_from_branch
            && running(Subject::Issue(number)).is_none()
        {
            plan.act(Action::DeleteBranch {
                branch: branch.name.clone(),
            });
        }
    }

    let (mut decisions, room) = plan.into_decisions(room);
    decisions.actions.extend(schedule(automations, now, room));
    decisions
}

/// The rest of a plan made before a drain began, as the drain has it: with
/// no claim and no run, and the automations' last runs as `recorded` holds
/// them for the runs it holds back, so that they run after the restart.
pub fn held_back_by_drain(rest: Vec<Action>, recorded: &LastRuns) -> Vec<Action> {
    let held_back: Vec<AutomationName> = rest
        .iter()
        .filter_map(|action| {
            if let Action::RunAutomation(automation) = action {
                Some(automation.name.clone())
            } else {
                None
            }
        })
        .collect();
    rest.into_iter()
        .filter_map(|action| match action {
            Action::Start(_)
            | Action::Claim { .. }
            | Action::RememberRebase(_)
            | Action::RunAutomation(_) => None,
            Action::RecordLastRuns(mut last_runs) => {
                for name in &held_back {
                    last_runs.restore(name, recorded);
                }
                (last_runs != *recorded).then_some(Action::RecordLastRuns(last_runs))
            }
            Action::Cancel { .. }
            | Action::LetGo { .. }
            | Action::Relabel { .. }
            | Action::UnassignAsked { .. }
            | Action::RemoveStatusLabel { .. }
            | Action::HandBack { .. }
            | Action::MarkReady { .. }
            | Action::Merge { .. }
            | Action::MarkReadyForMergeState { .. }
            | Action::HoldForReview { .. }
            | Action::CloseTracking { .. }
            | Action::DiscardTickets { .. }
            | Action::DeleteBranch { .. } => Some(action),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextRun {
    Due,
    At(Timestamp),
    Never,
}

pub fn next_run(automation: &Automation, last: Timestamp, now: Timestamp) -> NextRun {
    match automation.schedule.next_after(last) {
        Some(tick) if tick <= now => NextRun::Due,
        Some(tick) => NextRun::At(tick),
        None => NextRun::Never,
    }
}

fn schedule(automations: &Automations, now: Timestamp, room: usize) -> Vec<Action> {
    let mut last_runs = automations.last_runs.clone();
    let mut due = vec![];
    for automation in &automations.defined {
        let Some(last) = automations.last_runs.get(&automation.name) else {
            last_runs.record(&automation.name, now);
            continue;
        };
        let running = automations.in_flight.contains(&automation.name);
        if next_run(automation, last, now) == NextRun::Due && !running {
            due.push(automation);
        }
    }
    due.sort_by(|a, b| a.name.cmp(&b.name));
    let runs: Vec<Action> = due
        .into_iter()
        .take(room)
        .map(|automation| {
            last_runs.record(&automation.name, now);
            Action::RunAutomation(Box::new(automation.clone()))
        })
        .collect();
    // The record follows the runs: a stop between the two leaves the
    // automation due, to run again after the restart, rather than recorded as
    // run with no run started.
    let mut actions = runs;
    if last_runs != automations.last_runs {
        actions.push(Action::RecordLastRuns(last_runs));
    }
    actions
}

fn cancel_reason(run: &LiveRun, snapshot: &Snapshot, policy: &Policy) -> Option<String> {
    match run.subject {
        Subject::Pull(number) => {
            let Some(pr) = snapshot.pull(number) else {
                return Some(format!("{number} is no longer open"));
            };
            pull_stop(pr, &policy.linewright).map(|stop| match stop {
                PullStop::Merged => format!("{number} was merged"),
                PullStop::Closed => format!("{number} was closed"),
                PullStop::Unassigned => {
                    format!("{} was unassigned from {number}", policy.linewright)
                }
            })
        }
        Subject::SpecChange(pr) => {
            let pr = snapshot.merged_pull(pr)?;
            if let Some(label) = pr.designed_label() {
                Some(format!("{} was labeled `{}`", pr.number, label.name()))
            } else if !pr.is_assigned(&policy.linewright) {
                Some(format!(
                    "{} was unassigned from {}",
                    policy.linewright, pr.number
                ))
            } else {
                None
            }
        }
        Subject::Issue(number) => {
            let Some(issue) = snapshot.issue(number).filter(|i| i.open) else {
                return Some(format!("{number} was closed"));
            };
            let wanted = match run.kind {
                WorkKind::Triage => StatusLabel::NeedsTriage,
                WorkKind::Design
                | WorkKind::Implement
                | WorkKind::Rework
                | WorkKind::FixCi
                | WorkKind::Update => StatusLabel::ReadyForAgent,
            };
            if !issue.holds(wanted) {
                Some(format!("`{wanted}` was removed from {number}"))
            } else if !issue.is_assigned(&policy.linewright) {
                Some(format!(
                    "{} was unassigned from {number}",
                    policy.linewright
                ))
            } else {
                None
            }
        }
    }
}

struct Plan {
    now: Timestamp,
    /// What holds back the starts beyond the room for them.
    held: Held,
    actions: Vec<Action>,
    starts: Vec<Start>,
    waits: BTreeMap<Number, Vec<Wait>>,
}

#[derive(Clone, Copy)]
enum Held {
    UsageLimit { resets_at: Timestamp },
    Drain,
    Ceiling { in_flight: usize, max_runs: usize },
}

struct Start {
    claim: Option<Item>,
    rebase: Option<TriedRebase>,
    work: Work,
}

impl Plan {
    fn act(&mut self, action: Action) {
        self.actions.push(action);
    }

    /// A wait already noted for `item` is noted once.
    fn wait(&mut self, item: Number, wait: Wait) {
        let waits = self.waits.entry(item).or_default();
        if !waits.contains(&wait) {
            waits.push(wait);
        }
    }

    /// A claim on its own, outside any start, which a drain holds back too.
    fn claim(&mut self, item: Item) {
        if matches!(self.held, Held::Drain) {
            self.wait(item.number(), Wait::Drain);
        } else {
            self.act(Action::Claim { item });
        }
    }

    fn wait_all(&mut self, item: Number, waits: Vec<Wait>) {
        for wait in waits {
            self.wait(item, wait);
        }
    }

    fn start(&mut self, work: Work) {
        self.starts.push(Start {
            claim: None,
            rebase: None,
            work,
        });
    }

    fn claim_and_implement(&mut self, issue: Number) {
        self.claim_and_start(Item::Issue(issue), Work::Implement { issue });
    }

    fn claim_and_start(&mut self, item: Item, work: Work) {
        self.starts.push(Start {
            claim: Some(item),
            rebase: None,
            work,
        });
    }

    fn start_rebase(&mut self, work: Work, rebase: TriedRebase) {
        self.starts.push(Start {
            claim: None,
            rebase: Some(rebase),
            work,
        });
    }

    /// Also returns the room left.
    fn into_decisions(mut self, room: usize) -> (Decisions, usize) {
        let mut starts = std::mem::take(&mut self.starts);
        starts.sort_by_key(|s| (rank(s.work.kind()), s.work.subject()));
        let room_left = room.saturating_sub(starts.len());
        let started = starts.len().min(room);
        let wait = match self.held {
            Held::UsageLimit { resets_at } => Wait::UsageLimit { resets_at },
            Held::Drain => Wait::Drain,
            Held::Ceiling {
                in_flight,
                max_runs,
            } => Wait::FreeRun {
                in_flight: in_flight + started,
                max_runs,
            },
        };
        for start in starts.iter().skip(room) {
            self.wait(start.work.subject().number(), wait.clone());
        }
        let mut actions = self.actions;
        for start in starts.into_iter().take(room) {
            if let Some(item) = start.claim {
                actions.push(Action::Claim { item });
            }
            if let Some(rebase) = start.rebase {
                actions.push(Action::RememberRebase(rebase));
            }
            actions.push(Action::Start(start.work));
        }
        let decisions = Decisions {
            actions,
            waits: self.waits,
        };
        (decisions, room_left)
    }
}

fn rank(kind: WorkKind) -> u8 {
    match kind {
        WorkKind::Rework | WorkKind::FixCi | WorkKind::Update => 0,
        WorkKind::Triage => 1,
        WorkKind::Design => 2,
        WorkKind::Implement => 3,
    }
}

/// The work an issue's label asks of Linewright.
#[derive(Clone, Copy)]
enum Asked {
    Triage,
    Implementation,
}

/// What an issue whose label asks nothing of Linewright is labeled.
#[derive(Clone, Copy)]
enum Unasked {
    NeedsInfo,
    ReadyForHuman,
    NoStatusLabel,
}

/// What an issue labeled `status` asks of Linewright.
fn asked_work(status: Option<StatusLabel>) -> Result<Asked, Unasked> {
    match status {
        Some(StatusLabel::NeedsTriage) => Ok(Asked::Triage),
        Some(StatusLabel::ReadyForAgent) => Ok(Asked::Implementation),
        Some(StatusLabel::NeedsInfo) => Err(Unasked::NeedsInfo),
        Some(StatusLabel::ReadyForHuman) => Err(Unasked::ReadyForHuman),
        None => Err(Unasked::NoStatusLabel),
    }
}

/// Whether letting go of `issue`, whose label asks nothing of Linewright,
/// answers a person's assignment of Linewright: a person made the last one,
/// its label asked nothing of Linewright then either, and Linewright has not
/// commented since.
fn answers_hand_assignment(issue: &Issue, linewright: &Login) -> bool {
    let Some((assigned, actor)) = last_assignment(&issue.events, linewright) else {
        return false;
    };
    let unasked_then = asked_work(issue.status_label_as_of(assigned)).is_err();
    // A comment since means this one was posted, and a stop came before
    // the unassignment.
    let commented = issue
        .comments
        .iter()
        .any(|c| &c.author == linewright && c.created_at >= assigned.at);
    actor != linewright && unasked_then && !commented
}

/// The comment letting go of an issue assigned to Linewright by hand whose
/// label asks nothing of it, labeled `unasked`.
fn let_go_comment(unasked: Unasked, linewright: &Login) -> String {
    let waits = match unasked {
        Unasked::NeedsInfo => format!(
            "This issue waits for an answer to its question (`{}`)",
            label::NEEDS_INFO
        ),
        Unasked::ReadyForHuman => format!(
            "This issue waits for a person's decision (`{}`)",
            label::READY_FOR_HUMAN
        ),
        Unasked::NoStatusLabel => "This issue has no status label".into(),
    };
    format!(
        "{waits}, so @{linewright} has let go of it. Label it `{}` to have it implemented or \
         `{}` to have it assessed, and assign @{linewright} again.",
        label::READY_FOR_AGENT,
        label::NEEDS_TRIAGE,
    )
}

fn labeled_waits(issue: &Issue, policy: &Policy) -> Vec<Wait> {
    let handed_to = if issue.holds(StatusLabel::NeedsInfo) {
        Some(Wait::Reporter)
    } else if issue.holds(StatusLabel::ReadyForHuman) {
        Some(Wait::Maintainer)
    } else {
        return vec![];
    };
    let unlabeled = required_label(policy).filter(|_| !policy.may_start_on(issue));
    handed_to.into_iter().chain(unlabeled).collect()
}

fn open_numbers(linked: &[Linked]) -> Vec<Number> {
    linked.iter().filter(|l| l.open).map(|l| l.number).collect()
}

fn required_label(policy: &Policy) -> Option<Wait> {
    policy
        .require_label
        .as_ref()
        .map(|label| Wait::RequiredLabel {
            label: label.clone(),
        })
}

/// No open pull request closes `issue`. `replied` is a reply to `needs-info`
/// that labels it `needs-triage` again in this poll.
fn triage(issue: &Issue, replied: bool, live: Option<WorkKind>, policy: &Policy, plan: &mut Plan) {
    let work = Work::Triage {
        issue: issue.number,
    };
    if let Some(stage) = live {
        return plan.wait(issue.number, Wait::Run { stage });
    }
    if wait_for_blockers(issue, plan) {
        return;
    }
    if issue.is_assigned(&policy.linewright) {
        return plan.start(work);
    }
    match may_claim(issue, StatusLabel::NeedsTriage, replied, policy) {
        Ok(()) => plan.claim_and_start(Item::Issue(issue.number), work),
        Err(waits) => plan.wait_all(issue.number, waits),
    }
}

/// Whether Linewright may claim `issue`, labeled `status`, on its own, as far
/// as its assignees and labels go, or what it waits for first. `relabeled` is
/// `status` applied again in this poll.
fn may_claim(
    issue: &Issue,
    status: StatusLabel,
    relabeled: bool,
    policy: &Policy,
) -> Result<(), Vec<Wait>> {
    if !issue.assignees.is_empty() {
        let assignees = issue.assignees.iter().map(|assignee| Wait::Assignee {
            assignee: assignee.clone(),
        });
        return Err(assignees.collect());
    }
    if !relabeled && unassigned_since(issue, policy, status) {
        return Err(vec![Wait::Relabel {
            label: status,
            since: Relabel::Unassigned {
                required_label: policy.require_label.clone(),
            },
        }]);
    }
    if !policy.may_start_on(issue) {
        return Err(required_label(policy).into_iter().collect());
    }
    Ok(())
}

/// No open pull request closes `issue`; `closing` are the ended ones that do.
fn implementation(
    issue: &Issue,
    closing: &[ClosingPull],
    live: Option<WorkKind>,
    policy: &Policy,
    plan: &mut Plan,
) {
    let claimed = issue.is_assigned(&policy.linewright);
    if let Some(stage) = live {
        return plan.wait(issue.number, Wait::Run { stage });
    }
    if !claimed {
        let relabeled = false;
        if let Err(waits) = may_claim(issue, StatusLabel::ReadyForAgent, relabeled, policy) {
            return plan.wait_all(issue.number, waits);
        }
    }
    // GitHub closes the issue shortly after the merge; until then, only
    // applying the label again asks for more work. Reopened since, the
    // issue is worked as any other.
    let relabeled_since = |at| {
        issue
            .last_labeled(StatusLabel::ReadyForAgent)
            .is_some_and(|l| l > at)
    };
    let reopened_since = |at| issue.last_reopened().is_some_and(|r| r > at);
    if let Some(merged) = closing.iter().find(|p| match p.state {
        PullState::Merged { at } => !relabeled_since(at) && !reopened_since(at),
        PullState::Open { .. } | PullState::Closed { .. } => false,
    }) {
        return plan.wait(
            issue.number,
            Wait::Relabel {
                label: StatusLabel::ReadyForAgent,
                since: Relabel::Merged {
                    pull_request: merged.number,
                },
            },
        );
    }
    if claimed {
        let claimed_at = last_assigned(&issue.events, &policy.linewright);
        let refused = closing.iter().find(|p| match p.state {
            PullState::Closed { at } => {
                claimed_at.is_none_or(|since| since <= at) && !relabeled_since(at)
            }
            PullState::Open { .. } | PullState::Merged { .. } => false,
        });
        if let Some(refused) = refused {
            return plan.act(Action::HandBack {
                item: Item::Issue(issue.number),
                reason: format!("{} was closed without merging.", refused.number),
            });
        }
    }
    if wait_for_blockers(issue, plan) {
        return;
    }
    if claimed {
        plan.start(Work::Implement {
            issue: issue.number,
        });
    } else {
        plan.claim_and_implement(issue.number);
    }
}

/// Returns whether any blocker is open.
fn wait_for_blockers(issue: &Issue, plan: &mut Plan) -> bool {
    let blockers = open_numbers(&issue.blocked_by);
    for &blocker in &blockers {
        plan.wait(issue.number, Wait::Blocker { issue: blocker });
    }
    !blockers.is_empty()
}

fn in_review(ticket: Number, pr: &Pull, memory: &Memory, policy: &Policy, plan: &mut Plan) {
    let pr_number = pr.number;
    let linewright = &policy.linewright;
    let restarted = restarted_at(pr, linewright);
    if !new_feedback(
        pr,
        linewright,
        memory.last_done(pr_number, WorkKind::Rework),
    )
    .is_empty()
    {
        return plan.start(Work::Rework {
            issue: ticket,
            pr: pr_number,
        });
    }
    let behind = matches!(
        pr.mergeability,
        Mergeability::Behind | Mergeability::Conflicting
    );
    let update = Work::Update {
        issue: ticket,
        pr: pr_number,
    };
    if rebases_to_repair(pr, policy) && (behind || !tried_rebase(pr, memory, restarted)) {
        let rebase = TriedRebase {
            pr: pr_number,
            head: pr.head_sha.clone(),
            at: plan.now,
        };
        return plan.start_rebase(update, rebase);
    }
    if behind {
        return plan.start(update);
    }
    let names = |state: CheckState| -> Vec<String> {
        pr.checks
            .iter()
            .filter(|c| c.state == state)
            .map(|c| c.name.clone())
            .collect()
    };
    let (pending, failed) = (names(CheckState::Pending), names(CheckState::Failed));
    let pushed = pr
        .pushes
        .iter()
        .map(|p| p.at)
        .chain([pr.created_at])
        .max()
        .unwrap_or(pr.created_at);
    let now = plan.now;
    let ci = Wait::Ci {
        pull_request: pr_number,
    };
    if pr.checks.is_empty() && now < pushed + policy.ci_grace {
        return plan.wait(pr_number, ci);
    }
    let waiting_since = restarted.map_or(pushed, |r| r.max(pushed));
    if !pending.is_empty() {
        if now > waiting_since + policy.ci_timeout {
            plan.act(Action::HandBack {
                item: Item::Pull(pr_number),
                reason: format!(
                    "CI did not complete within {}: {} still running.",
                    humanize(policy.ci_timeout),
                    code_list(&pending)
                ),
            });
        } else {
            plan.wait(pr_number, ci);
        }
        return;
    }
    if !failed.is_empty() {
        let cycle_start = [
            Some(pr.created_at),
            memory.last_done(pr_number, WorkKind::Rework),
            restarted,
        ]
        .into_iter()
        .flatten()
        .max();
        let attempts = memory
            .done_runs_of(pr_number, WorkKind::FixCi)
            .filter(|r| Some(r.done_at) > cycle_start)
            .count();
        if attempts < policy.ci_attempts as usize {
            plan.start(Work::FixCi {
                issue: ticket,
                pr: pr_number,
                failing: failed,
            });
        } else {
            plan.act(Action::HandBack {
                item: Item::Pull(pr_number),
                reason: format!(
                    "{} still failing after {attempts} fix attempts.",
                    code_list(&failed)
                ),
            });
        }
        return;
    }
    let draft = pr.state == (PullState::Open { draft: true });
    let method = match policy.auto_merge {
        Some(method) if !is_held(pr, linewright) => method,
        _ => {
            if draft {
                plan.act(Action::MarkReady { pr: pr_number });
            } else {
                plan.wait(
                    pr_number,
                    Wait::Review {
                        pull_request: pr_number,
                    },
                );
            }
            return;
        }
    };
    let merge_state = Wait::MergeState {
        pull_request: pr_number,
    };
    // Behind and conflicting were brought up to date above, and so was not
    // rebaseable, once at each head.
    match &pr.mergeability {
        Mergeability::Clean if method == MergeMethod::Rebase => match pr.rebaseability {
            Rebaseability::Rebaseable => merge(pr, draft, method, memory, restarted, plan),
            Rebaseability::NotRebaseable => plan.act(Action::HandBack {
                item: Item::Pull(pr_number),
                reason: format!(
                    "GitHub reports {pr_number} not rebaseable though Linewright rebased it \
                     onto the default branch, so it cannot be merged by rebase."
                ),
            }),
            Rebaseability::Unknown => plan.wait(pr_number, merge_state),
        },
        Mergeability::Clean => merge(pr, draft, method, memory, restarted, plan),
        Mergeability::Blocked { requires }
            if draft || !memory.held.is_held_for(pr_number, requires) =>
        {
            plan.act(Action::HoldForReview {
                pr: pr_number,
                draft,
                requires: requires.clone(),
            });
        }
        Mergeability::Blocked { requires } => plan.wait(
            pr_number,
            Wait::Requirements {
                pull_request: pr_number,
                requires: requires.clone(),
            },
        ),
        Mergeability::Unknown if draft => {
            plan.act(Action::MarkReadyForMergeState { pr: pr_number });
        }
        Mergeability::Unknown => plan.wait(pr_number, merge_state),
        // Brought up to date above.
        Mergeability::Behind | Mergeability::Conflicting => {}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PullClaim {
    Claimed,
    /// Neither assigned nor unassigned ever, as a pull request Linewright
    /// opened before it assigned itself to those it opens.
    NeverAssigned,
    Unassigned,
}

fn pull_claim(pr: &Pull, linewright: &Login) -> PullClaim {
    let ever_assigned = pr.assignments.iter().any(|a| a.assignee() == linewright);
    if pr.is_assigned(linewright) {
        PullClaim::Claimed
    } else if ever_assigned {
        PullClaim::Unassigned
    } else {
        PullClaim::NeverAssigned
    }
}

/// Why Linewright's work on a pull request stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullStop {
    Merged,
    Closed,
    /// Linewright was unassigned from it: `docs/specs/pull_requests.md`, "The
    /// pull request".
    Unassigned,
}

pub fn pull_stop(pr: &Pull, linewright: &Login) -> Option<PullStop> {
    match pr.state {
        PullState::Merged { .. } => Some(PullStop::Merged),
        PullState::Closed { .. } => Some(PullStop::Closed),
        PullState::Open { .. } => match pull_claim(pr, linewright) {
            PullClaim::Unassigned => Some(PullStop::Unassigned),
            PullClaim::Claimed | PullClaim::NeverAssigned => None,
        },
    }
}

/// When Linewright was last assigned again after an unassignment, which
/// restarts the pull request's retry counters.
fn restarted_at(pr: &Pull, linewright: &Login) -> Option<Timestamp> {
    let mut unassigned = false;
    let mut restarted = None;
    for assignment in &pr.assignments {
        match assignment {
            Assignment::Unassigned { assignee, .. } if assignee == linewright => {
                unassigned = true;
            }
            Assignment::Assigned { assignee, at, .. } if assignee == linewright && unassigned => {
                restarted = Some(*at);
            }
            Assignment::Assigned { .. } | Assignment::Unassigned { .. } => {}
        }
    }
    restarted
}

fn tried_rebase(pr: &Pull, memory: &Memory, restarted: Option<Timestamp>) -> bool {
    memory
        .tried_rebases
        .iter()
        .any(|r| r.pr == pr.number && r.head == pr.head_sha && restarted.is_none_or(|at| at < r.at))
}

pub fn rebases_to_repair(pr: &Pull, policy: &Policy) -> bool {
    pr.rebaseability == Rebaseability::NotRebaseable && policy.rebases_unrebaseable()
}

fn merge(
    pr: &Pull,
    draft: bool,
    method: MergeMethod,
    memory: &Memory,
    restarted: Option<Timestamp>,
    plan: &mut Plan,
) {
    match merge_failed_twice(pr, &memory.failed_merges, restarted) {
        Some(failed) => {
            plan.act(Action::HandBack {
                item: Item::Pull(pr.number),
                reason: format!(
                    "Merging {} failed twice. GitHub's reason: {}",
                    pr.number, failed.reason
                ),
            });
        }
        None => plan.act(Action::Merge {
            pr: pr.number,
            head: pr.head_sha.clone(),
            draft,
            method,
        }),
    }
}

fn merge_failed_twice<'a>(
    pr: &Pull,
    failed_merges: &'a [FailedMerge],
    restarted: Option<Timestamp>,
) -> Option<&'a FailedMerge> {
    let failed: Vec<&FailedMerge> = failed_merges
        .iter()
        .filter(|f| f.pr == pr.number && restarted.is_none_or(|r| f.at > r))
        .collect();
    failed.last().copied().filter(|_| failed.len() >= 2)
}

pub fn hold_note(requires: &[Requirement]) -> String {
    let names: Vec<String> = requires.iter().map(ToString::to_string).collect();
    let required = match names.as_slice() {
        [] => "GitHub blocks the merge for a reason Linewright cannot read: the default \
               branch's rules say what it needs"
            .to_string(),
        [one] => format!("GitHub requires {one} before merging"),
        [rest @ .., last] => format!(
            "GitHub requires {} and {last} before merging",
            rest.join(", ")
        ),
    };
    format!("Ready for review: {required}. Linewright merges it once GitHub allows it.")
}

/// The inline comments submitted with an approval are made no later than it,
/// so they don't hold the merge.
fn is_held(pr: &Pull, linewright: &Login) -> bool {
    let comments = pr
        .comments
        .iter()
        .chain(pr.threads.iter().flat_map(|t| &t.comments))
        .map(|c| (&c.author, c.created_at));
    let reviews = pr.reviews.iter().map(|r| (&r.author, r.submitted_at));
    let activity: Vec<_> = comments
        .chain(reviews)
        .filter(|(author, _)| author.is_person_besides(linewright))
        .collect();
    activity.iter().any(|(person, _)| {
        let approved = pr
            .reviews
            .iter()
            .filter(|r| &r.author == *person && r.state == ReviewState::Approved)
            .map(|r| r.submitted_at)
            .max();
        approved.is_none_or(|approved| {
            activity
                .iter()
                .any(|(author, at)| author == person && *at > approved)
        })
    })
}

#[derive(Debug, Default)]
pub struct Feedback<'a> {
    pub comments: Vec<&'a Comment>,
    pub reviews: Vec<&'a Review>,
    pub threads: Vec<(&'a Thread, Vec<&'a Comment>)>,
}

impl Feedback<'_> {
    pub fn is_empty(&self) -> bool {
        self.comments.is_empty() && self.reviews.is_empty() && self.threads.is_empty()
    }
}

pub fn new_feedback<'a>(
    pr: &'a Pull,
    linewright: &Login,
    reworked: Option<Timestamp>,
) -> Feedback<'a> {
    let pushed = pr
        .pushes
        .iter()
        .filter(|p| &p.by == linewright)
        .map(|p| p.at);
    let created = (&pr.author == linewright).then_some(pr.created_at);
    let answered = pushed.chain(created).chain(reworked).max();
    let new = |author: &Login, at: Timestamp| {
        author != linewright && answered.is_none_or(|answered| at > answered)
    };
    Feedback {
        comments: pr
            .comments
            .iter()
            .filter(|c| new(&c.author, c.created_at))
            .collect(),
        reviews: pr
            .reviews
            .iter()
            .filter(|r| new(&r.author, r.submitted_at))
            .filter(|r| !(r.state == ReviewState::Approved && r.body.trim().is_empty()))
            .collect(),
        threads: pr
            .threads
            .iter()
            .filter_map(|t| {
                let comments: Vec<_> = t
                    .comments
                    .iter()
                    .filter(|c| new(&c.author, c.created_at))
                    .collect();
                (!comments.is_empty()).then_some((t, comments))
            })
            .collect(),
    }
}

fn code_list(names: &[String]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn humanize(delta: TimeDelta) -> String {
    humantime::format_duration(delta.to_std().unwrap_or_default()).to_string()
}

/// Whether Linewright was unassigned from `issue`, by anyone, after it was
/// last labeled with `status` or with the required label.
fn unassigned_since(issue: &Issue, policy: &Policy, status: StatusLabel) -> bool {
    let Some(unassigned) = last_unassignment(&issue.events, &policy.linewright) else {
        return false;
    };
    [Some(status.label()), policy.require_label.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(|l| last_labeling(&issue.events, l))
        .max()
        .is_none_or(|labeled| unassigned > labeled)
}

/// `issue` without the people Linewright assigned to it, those its
/// `needs-info` question asked, unassigning them when its label asks for
/// Linewright's work again (`asked`), by a reply or by hand, and Linewright
/// may start on it.
fn let_go_of_people_asked<'a>(
    issue: &'a Issue,
    asked: Result<Asked, Unasked>,
    policy: &Policy,
    plan: &mut Plan,
) -> Cow<'a, Issue> {
    let people_asked = issue.assigned_by(&policy.linewright);
    if asked.is_err() || !policy.may_start_on(issue) || people_asked.is_empty() {
        return Cow::Borrowed(issue);
    }
    // Unassigned before the relabeling, so that a stop between the two
    // leaves the reply to be found again.
    let mut without_asked = issue.clone();
    without_asked
        .assignees
        .retain(|a| !people_asked.contains(a));
    plan.act(Action::UnassignAsked {
        issue: issue.number,
        assignees: people_asked,
    });
    Cow::Owned(without_asked)
}

fn has_reply_to_needs_info(issue: &Issue, policy: &Policy) -> bool {
    let linewright = &policy.linewright;
    if !issue.holds(StatusLabel::NeedsInfo) || !policy.may_start_on(issue) {
        return false;
    }
    let Some(last) = issue
        .comments
        .iter()
        .filter(|c| &c.author == linewright)
        .map(|c| c.created_at)
        .max()
    else {
        return false;
    };
    let asked = issue
        .last_labeled(StatusLabel::NeedsInfo)
        .map_or(last, |labeled| labeled.max(last));
    issue
        .comments
        .iter()
        .any(|c| &c.author != linewright && c.created_at > asked)
}

pub fn pending_spec_changes<'a>(
    snapshot: &'a Snapshot,
    now: Timestamp,
    policy: &Policy,
) -> Vec<&'a MergedPull> {
    snapshot
        .merged
        .iter()
        .filter(|pr| is_spec_change(pr, snapshot, now, policy))
        .collect()
}

/// `cancel_reason` does not look at the required label, so a cut under way
/// survives its removal.
fn is_spec_change(pr: &MergedPull, snapshot: &Snapshot, now: Timestamp, policy: &Policy) -> bool {
    not_a_spec_change(pr, &snapshot.default_branch, now, policy).is_none()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum NotSpecChange {
    Linewrights,
    OtherBranch {
        base: String,
    },
    NoSpecFile {
        spec_glob: String,
    },
    Designed {
        label: DesignedLabel,
    },
    BeforeLookback {
        merged_at: Timestamp,
        window_opened: Timestamp,
    },
    /// Linewright was unassigned from it, and `required_label`, when one is
    /// set, was not applied again since.
    Unassigned {
        at: Timestamp,
        required_label: Option<String>,
    },
    RequiredLabel {
        label: String,
    },
}

/// Rules that hold for good are checked first.
pub fn not_a_spec_change(
    pr: &MergedPull,
    default_branch: &str,
    now: Timestamp,
    policy: &Policy,
) -> Option<NotSpecChange> {
    let window_opened = now - policy.spec_lookback;
    let because = if pr.author == policy.linewright {
        NotSpecChange::Linewrights
    } else if pr.base_ref != default_branch {
        NotSpecChange::OtherBranch {
            base: pr.base_ref.clone(),
        }
    } else if !pr.files.iter().any(|f| policy.spec_glob.matches(f)) {
        NotSpecChange::NoSpecFile {
            spec_glob: policy.spec_glob.pattern().to_string(),
        }
    } else if let Some(label) = pr.designed_label() {
        NotSpecChange::Designed { label }
    } else if pr.merged_at < window_opened {
        NotSpecChange::BeforeLookback {
            merged_at: pr.merged_at,
            window_opened,
        }
    } else if pr.is_assigned(&policy.linewright) {
        return None;
    } else if let Some(at) =
        pr.unassigned_since_labeled(&policy.linewright, policy.require_label.as_deref())
    {
        NotSpecChange::Unassigned {
            at,
            required_label: policy.require_label.clone(),
        }
    } else {
        let unlabeled = policy.require_label.as_ref().filter(|l| !pr.has_label(l));
        return unlabeled.map(|label| NotSpecChange::RequiredLabel {
            label: label.clone(),
        });
    };
    Some(because)
}
