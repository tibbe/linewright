use std::collections::BTreeMap;

use chrono::TimeDelta;
use serde::{Deserialize, Serialize};

use crate::automation::{Automation, AutomationName, LastScheduledRuns};
use crate::domain::{
    BranchIssue, ClosingPull, DesignedLabel, HandBackKind, Issue, Item, Linked, Login, Marking,
    Merge, MergeMethod, MergeMethods, MergedPull, Number, Pull, PullState, Rebaseability,
    Requirement, Snapshot, StatusLabel, SubIssue, TicketBranchName, Timestamp, UpdateMethod,
    UpdateRule, Windows, label,
};
use crate::harness::UsageLimit;
use crate::holds::{Hold, InForce};
use crate::lifecycle::{
    Cancel, IssueFacts, IssueStage, NotClaimed, Opener, PullClaim, PullFacts, PullRow, PullWorking,
    Row, State, TakeUp, cancels, done_by_merge, issue_state, may_claim, merged_row, pull_row,
    pull_state, row,
};
use crate::phrase::Phrase;
use crate::pull_records::PullRecords;
use crate::repo_files::{FileError, Unusable};
use crate::runs::Ending;
use crate::spec_glob::SpecGlob;
use crate::work_settings::RunKind;
use crate::working_pull::{self, Row as WorkingRow};

#[derive(Clone, Debug)]
pub struct Policy {
    pub linewright: Login,
    pub spec_glob: SpecGlob,
    pub spec_lookback: TimeDelta,
    pub done_window: TimeDelta,
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
    /// Whether an item labeled `labels` is marked for Linewright
    /// (`docs/specs/repository.md`, "Marked for Linewright").
    pub(crate) fn marks(&self, labels: &[String]) -> bool {
        self.require_label
            .as_ref()
            .is_none_or(|required| labels.contains(required))
    }

    /// [`Policy::marks`], as a [`Marking`].
    pub(crate) fn marking(&self, labels: &[String]) -> Marking {
        if self.marks(labels) {
            Marking::Marked
        } else {
            Marking::Unmarked
        }
    }

    /// The windows open at `now`.
    pub fn windows(&self, now: Timestamp) -> Windows {
        Windows::at(now, self.spec_lookback, self.done_window)
    }

    /// Whether Linewright rebases a pull request GitHub reports not
    /// rebaseable: when the repository allows rebase merges and updates do
    /// not always merge.
    pub fn rebases_unrebaseable(&self) -> bool {
        self.allowed_merge_methods.contains(MergeMethod::Rebase)
            && self.update_rule != UpdateRule::Always(UpdateMethod::Merge)
    }
}

/// At most one run per subject is in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subject {
    Issue(Number),
    Pull(Number),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Work {
    Triage {
        issue: Number,
    },
    DesignIssue {
        issue: Number,
    },
    Implement {
        issue: Number,
    },
    Rework {
        issue: Number,
        pr: Number,
        /// It starts by bringing the branch up to date
        /// ([`working_pull::rework_starts_with_update`]).
        update: bool,
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
            Subject::Issue(number) | Subject::Pull(number) => number,
        }
    }
}

impl Work {
    pub fn subject(&self) -> Subject {
        match self {
            Work::Triage { issue } | Work::DesignIssue { issue } | Work::Implement { issue } => {
                Subject::Issue(*issue)
            }
            Work::Rework { pr, .. } | Work::FixCi { pr, .. } | Work::Update { pr, .. } => {
                Subject::Pull(*pr)
            }
        }
    }

    pub fn item(&self) -> Number {
        match self {
            Work::Triage { issue } | Work::DesignIssue { issue } | Work::Implement { issue } => {
                *issue
            }
            Work::Rework { pr, .. } | Work::FixCi { pr, .. } | Work::Update { pr, .. } => *pr,
        }
    }

    pub fn starts_from(&self) -> Item {
        match self {
            Work::Triage { issue } | Work::DesignIssue { issue } | Work::Implement { issue } => {
                Item::Issue(*issue)
            }
            Work::Rework { pr, .. } | Work::FixCi { pr, .. } | Work::Update { pr, .. } => {
                Item::Pull(*pr)
            }
        }
    }

    pub fn kind(&self) -> WorkKind {
        match self {
            Work::Triage { .. } => WorkKind::Triage,
            Work::DesignIssue { .. } => WorkKind::Design,
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
    pub ended_runs: Vec<EndedRun>,
    pub pulls: PullRecords,
    pub usage_limit: Option<UsageLimit>,
    /// Set from when a drain of `serve` begins until it exits.
    pub drain: Option<Drain>,
    /// Set while there is a setup command and no environment
    /// (`docs/specs/sessions.md`, Environment).
    pub awaits_environment: bool,
}

impl Memory {
    pub fn retain_open(&mut self, snapshot: &Snapshot) {
        let open = |pr| snapshot.pull(pr).is_some_and(Pull::is_open);
        self.ended_runs.retain(|r| open(r.pr));
        self.pulls.retain(open);
    }

    /// When the last rework run on `pr` that ended done last read the
    /// feedback it was given: what it answered.
    pub fn rework_answered_until(&self, pr: Number) -> Option<Timestamp> {
        self.ended_on(pr)
            .filter(|r| r.kind == WorkKind::Rework && r.ending == Ending::Done)
            .map(|r| r.answered_until.unwrap_or(r.at))
            .max()
    }

    /// The runs that ended on `pr`.
    pub(crate) fn ended_on(&self, pr: Number) -> impl Iterator<Item = &EndedRun> {
        self.ended_runs.iter().filter(move |r| r.pr == pr)
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

    /// Whether a drain has ended `serve`, with `runs_in_flight` runs still
    /// in flight (docs/specs/operation.md, "Stopping", Draining).
    pub fn drained(&self, runs_in_flight: usize) -> bool {
        self.drain.is_some() && runs_in_flight == 0
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

/// A run that ended on a pull request, as its record keeps it. An
/// interrupted run has no end, so it is none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndedRun {
    pub pr: Number,
    pub kind: WorkKind,
    pub ending: Ending,
    pub at: Timestamp,
    /// For a rework, when it last read the feedback it was given.
    pub answered_until: Option<Timestamp>,
}

#[derive(Clone, Debug, Default)]
pub struct Automations {
    pub defined: Vec<Automation>,
    pub last_scheduled_runs: LastScheduledRuns,
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
    /// Label an issue `to` in place of its other status labels.
    Relabel {
        issue: Number,
        to: StatusLabel,
    },
    /// Unassign the people Linewright asked a `needs-info` question, once
    /// the issue's label asks for Linewright's work again.
    UnassignAsked {
        issue: Number,
        assignees: Vec<Login>,
    },
    /// Remove a status label besides the one the issue holds, or one a
    /// tracking issue carries.
    RemoveStatusLabel {
        issue: Number,
        label: StatusLabel,
    },
    HandBack {
        item: Item,
        kind: HandBackKind,
        reason: String,
    },
    /// Post `comment`, then unassign Linewright from a pull request it did
    /// not open.
    UnassignFromPull {
        pr: Number,
        comment: String,
    },
    /// Take up spec change #`pr` by opening its issue, then unassign
    /// Linewright from the spec change where `let_go`
    /// (`docs/specs/sources_of_work.md`, Spec changes).
    OpenSpecChangeIssue {
        pr: Number,
        let_go: bool,
    },
    /// Remove [`label::STALLED`] from an item Linewright works again.
    ClearStall {
        item: Item,
    },
    MarkReady {
        pr: Number,
    },
    /// Only while its head is still `head`.
    Merge {
        pr: Number,
        head: String,
        method: MergeMethod,
    },
    /// Comment naming what GitHub requires to merge it.
    CommentRequirements {
        pr: Number,
        requires: Vec<Requirement>,
    },
    CloseTracking {
        issue: Number,
    },
    DiscardCut(Discard),
    DeleteBranch {
        branch: TicketBranchName,
    },
    RecordLastScheduledRuns(LastScheduledRuns),
    RunAutomation(Box<Automation>),
}

impl Action {
    /// The items it writes or starts work on.
    pub fn items(&self, snapshot: &Snapshot) -> Vec<Item> {
        match self {
            Action::Start(work) => vec![work.starts_from()],
            Action::Claim { item } => vec![*item],
            Action::LetGo { issue, .. }
            | Action::Relabel { issue, .. }
            | Action::UnassignAsked { issue, .. }
            | Action::RemoveStatusLabel { issue, .. }
            | Action::CloseTracking { issue } => vec![Item::Issue(*issue)],
            Action::DiscardCut(discard) => discard
                .tickets
                .iter()
                .map(|ticket| ticket.number)
                .chain([discard.issue])
                .map(Item::Issue)
                .chain(discard.pulls.iter().copied().map(Item::Pull))
                .collect(),
            Action::Merge { pr, .. } => snapshot
                .pull(*pr)
                .into_iter()
                .flat_map(|p| p.closes.iter().map(|issue| Item::Issue(*issue)))
                .chain([Item::Pull(*pr)])
                .collect(),
            Action::MarkReady { pr }
            | Action::UnassignFromPull { pr, .. }
            | Action::OpenSpecChangeIssue { pr, .. }
            | Action::CommentRequirements { pr, .. } => vec![Item::Pull(*pr)],
            Action::HandBack { item, .. } | Action::ClearStall { item } => vec![*item],
            Action::DeleteBranch { branch } => vec![Item::Issue(branch.issue())],
            Action::Cancel { .. }
            | Action::RecordLastScheduledRuns(_)
            | Action::RunAutomation(_) => vec![],
        }
    }
}

/// Close `pulls`, then the open `tickets` as not planned, then remove
/// `tickets` from the sub-issues of `issue`, the issue they were cut from,
/// leaving it to be designed again (docs/specs/design.md, "Discarding a
/// cut"). A person's pull request closing a ticket, and a sub-issue a person
/// added, are left as they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discard {
    pub issue: Number,
    /// Linewright's open pull requests that close a ticket of the cut, the
    /// ticket open or closed.
    pub pulls: Vec<Number>,
    /// The tickets of the cut still among the sub-issues, open or closed:
    /// removed last, so that a discard stopped part way is found again.
    pub tickets: Vec<Linked>,
}

impl Discard {
    /// The tickets it closes.
    pub fn open_tickets(&self) -> Vec<Number> {
        open_numbers(self.tickets.iter().copied())
    }

    /// What it posts on each item it closes, and on the issue its tickets are
    /// under.
    pub fn notes(&self) -> DiscardNotes {
        let issue = self.issue;
        let discarded =
            |what: &str| format!("{issue} is being designed again, so this {what} discarded.");
        let tickets = self.open_tickets();
        let mut pulls = self.pulls.clone();
        pulls.sort_unstable();
        let on_issue = (!tickets.is_empty() || !pulls.is_empty()).then(|| ParentNote {
            issue,
            note: discarded_from_issue(&tickets, &pulls),
        });
        DiscardNotes {
            on_pull: discarded("pull request for a ticket of its earlier cut is"),
            on_ticket: discarded("ticket from its earlier cut is"),
            on_issue,
        }
    }
}

/// What a discarded cut posts, by [`Discard::notes`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscardNotes {
    /// On each pull request it closes.
    pub on_pull: String,
    /// On each ticket it closes.
    pub on_ticket: String,
    /// On the issue designed again, which stays open, before its tickets
    /// leave its sub-issues: where the discard closes anything.
    pub on_issue: Option<ParentNote>,
}

/// What a discarded cut posts on `issue`, the issue its tickets are under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParentNote {
    pub issue: Number,
    pub note: String,
}

/// What an issue says of the `tickets` and `pulls` of its earlier cut that
/// were discarded, at least one of them.
fn discarded_from_issue(tickets: &[Number], pulls: &[Number]) -> String {
    let listed = |one: &str, many: &str, numbers: &[Number]| {
        let numbers: Vec<String> = numbers.iter().map(Number::to_string).collect();
        match numbers.as_slice() {
            [] => None,
            [only] => Some(format!("{one} {only}")),
            [rest @ .., last] => Some(format!("{many} {} and {last}", rest.join(", "))),
        }
    };
    let lists: Vec<String> = [
        listed("ticket", "tickets", tickets),
        listed("pull request", "pull requests", pulls),
    ]
    .into_iter()
    .flatten()
    .collect();
    let verb = if tickets.len() + pulls.len() == 1 {
        "is"
    } else {
        "are"
    };
    format!(
        "This issue is being designed again, so the {} of its earlier cut {verb} discarded.",
        lists.join(" and ")
    )
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
    /// Ends once a build of the environment succeeds.
    Environment,
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
    PeopleAsked,
    Maintainer,
    /// Linewright was unassigned from the item and is not assigned to it
    /// again.
    Unassigned {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        required_label: Option<String>,
    },
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
    /// A file of the repository's that Linewright cannot use and the run
    /// would read.
    RepositoryFile(FileError),
}

impl Wait {
    /// Whether it is for what Linewright being unassigned from the item
    /// asks: being assigned again, or its label applied again.
    pub fn is_unassigned(&self) -> bool {
        matches!(
            self,
            Wait::Unassigned { .. }
                | Wait::Relabel {
                    since: Relabel::Unassigned { .. },
                    ..
                }
        )
    }
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
    unusable: &[Unusable],
) -> Vec<Action> {
    decisions(snapshot, live, memory, automations, now, policy, unusable).actions
}

/// `unusable` are the files of the repository's Linewright cannot use,
/// which hold the runs that would read them.
pub fn decisions(
    snapshot: &Snapshot,
    live: &[LiveRun],
    memory: &Memory,
    automations: &Automations,
    now: Timestamp,
    policy: &Policy,
    unusable: &[Unusable],
) -> Decisions {
    let in_flight = live.len() + automations.in_flight.len();
    let room = policy.max_runs.saturating_sub(in_flight);
    let ceiling = Ceiling {
        in_flight,
        max_runs: policy.max_runs,
    };
    let in_force = InForce::of(memory, now, unusable);
    let held_by_runs = held_by_runs(snapshot, live, &policy.linewright);
    let mut plan = Plan {
        ceiling,
        in_force: &in_force,
        snapshot,
        held_by_runs: &held_by_runs,
        actions: vec![],
        starts: vec![],
        waits: BTreeMap::new(),
    };
    let running = |subject: Subject| {
        live.iter()
            .find(|run| run.subject == subject)
            .map(|run| run.kind)
    };

    for run in live {
        if let Some(reason) = cancel_reason(run, snapshot, policy) {
            plan.act(Action::Cancel {
                subject: run.subject,
                reason,
            });
        }
    }

    let discards = discards(snapshot, &running, policy);
    let discarded_tickets: Vec<Number> = discards.iter().flat_map(Discard::open_tickets).collect();
    let discarded_pulls: Vec<Number> = discards.iter().flat_map(|d| d.pulls.clone()).collect();

    for pr in &snapshot.merged {
        let assigned = pr.is_assigned(&policy.linewright);
        let as_spec_change = spec_change(pr, snapshot, now, policy);
        if let Some(TakeUp { let_go }) = merged_row(&as_spec_change, assigned) {
            plan.act(Action::OpenSpecChangeIssue {
                pr: pr.number,
                let_go,
            });
        }
    }

    for (pr, ticket) in worked_pulls(snapshot, &policy.linewright)
        .filter(|(pr, _)| !discarded_pulls.contains(&pr.number))
    {
        let claim = PullClaim::of(&pr.assignees, &pr.assignments, &policy.linewright);
        if claim == PullClaim::Unassigned {
            plan.wait(
                pr.number,
                Wait::Unassigned {
                    required_label: None,
                },
            );
            continue;
        }
        let run = running(Subject::Pull(pr.number));
        if let Some(stage) = run {
            plan.wait(pr.number, Wait::Run { stage });
        }
        let work = pull_work(ticket, pr, memory, policy, now);
        let working = match (run, &work) {
            (Some(_), _) => PullWorking::RunInFlight,
            (None, PullWork::Wait(_)) => PullWorking::Waiting,
            (None, PullWork::Start(_) | PullWork::Act(_)) => PullWorking::Asked,
        };
        let facts = PullFacts {
            opener: Opener::Linewright,
            claim,
            working,
            stalled: pr.is_stalled(),
        };
        let holding = pull_row(&facts, pull_state(&pr.state), &policy.linewright);
        if run.is_none() {
            work_pull(pr.number, holding, Some(work), &mut plan);
        }
    }

    let peoples_assigned_pull = PullFacts {
        opener: Opener::Person,
        claim: PullClaim::Claimed,
        working: PullWorking::Waiting,
        stalled: false,
    };
    for pr in snapshot
        .peoples_pulls
        .iter()
        .filter(|p| p.is_open() && p.author != policy.linewright)
        .filter(|p| p.is_assigned(&policy.linewright))
    {
        let holding = pull_row(&peoples_assigned_pull, State::Open, &policy.linewright);
        work_pull(pr.number, holding, None, &mut plan);
    }

    for issue in snapshot
        .issues
        .iter()
        .filter(|i| i.open && !discarded_tickets.contains(&i.number))
    {
        let discard = discards.iter().find(|d| d.issue == issue.number);
        work_issue(issue, discard, &running, policy, &mut plan);
    }

    for branch in &snapshot.ticket_branches {
        let number = branch.name.issue();
        let read = match &branch.issue {
            BranchIssue::Held => None,
            BranchIssue::Read(issue) => Some(&**issue),
        };
        let closed_with_no_pull = snapshot
            .issue(number)
            .or(read)
            .is_some_and(Issue::closed_with_no_open_pull);
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
    decisions
        .actions
        .extend(schedule(automations, now, room, &in_force));
    decisions
}

/// Works pull request `number` by `holding`, the row that holds for it, and
/// `work`, what Pull requests asks of it where Linewright opened it.
fn work_pull(number: Number, holding: Option<PullRow>, work: Option<PullWork>, plan: &mut Plan) {
    let works = match holding {
        Some(PullRow::Claim) => {
            plan.claim(Item::Pull(number));
            true
        }
        Some(PullRow::Work) => true,
        Some(PullRow::LetGo { comment }) => {
            plan.act(Action::UnassignFromPull {
                pr: number,
                comment,
            });
            false
        }
        Some(PullRow::ClearStall) => {
            plan.act(Action::ClearStall {
                item: Item::Pull(number),
            });
            false
        }
        None => false,
    };
    match work {
        Some(PullWork::Wait(wait)) => plan.wait(number, wait),
        Some(PullWork::Start(work)) if works => plan.start(work),
        Some(PullWork::Act(action)) if works => plan.act(action),
        Some(PullWork::Start(_) | PullWork::Act(_)) | None => {}
    }
}

/// The rest of a plan made before a drain began, as the drain has it: with
/// no claim, no clear of a stall that follows one, and no run, and the
/// automations' last scheduled runs as
/// `recorded` holds them for the runs it holds back, so that they run after
/// the restart.
pub fn held_back_by_drain(rest: Vec<Action>, recorded: &LastScheduledRuns) -> Vec<Action> {
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
    let claimed: Vec<Item> = rest
        .iter()
        .filter_map(|action| {
            if let Action::Claim { item } = action {
                Some(*item)
            } else {
                None
            }
        })
        .collect();
    rest.into_iter()
        .filter_map(|action| match action {
            Action::ClearStall { item } => {
                (!claimed.contains(&item)).then_some(Action::ClearStall { item })
            }
            Action::Start(_) | Action::Claim { .. } | Action::RunAutomation(_) => None,
            Action::RecordLastScheduledRuns(mut last_scheduled_runs) => {
                for name in &held_back {
                    last_scheduled_runs.restore(name, recorded);
                }
                (last_scheduled_runs != *recorded)
                    .then_some(Action::RecordLastScheduledRuns(last_scheduled_runs))
            }
            Action::Cancel { .. }
            | Action::LetGo { .. }
            | Action::Relabel { .. }
            | Action::UnassignAsked { .. }
            | Action::UnassignFromPull { .. }
            | Action::OpenSpecChangeIssue { .. }
            | Action::RemoveStatusLabel { .. }
            | Action::HandBack { .. }
            | Action::MarkReady { .. }
            | Action::Merge { .. }
            | Action::CommentRequirements { .. }
            | Action::CloseTracking { .. }
            | Action::DiscardCut(_)
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

pub fn next_run(automation: &Automation, last_scheduled_run: Timestamp, now: Timestamp) -> NextRun {
    match automation.schedule.next_after(last_scheduled_run) {
        Some(tick) if tick <= now => NextRun::Due,
        Some(tick) => NextRun::At(tick),
        None => NextRun::Never,
    }
}

/// The runs of the automations due, at most `room`, followed by the record
/// of the last scheduled runs if it changed. An automation a hold in
/// `in_force` holds stays due, to run once the hold ends.
fn schedule(
    automations: &Automations,
    now: Timestamp,
    room: usize,
    in_force: &InForce,
) -> Vec<Action> {
    let mut last_scheduled_runs = automations.last_scheduled_runs.clone();
    let mut due = vec![];
    for automation in &automations.defined {
        let Some(last_scheduled_run) = automations.last_scheduled_runs.get(&automation.name) else {
            last_scheduled_runs.record(&automation.name, now);
            continue;
        };
        let running = automations.in_flight.contains(&automation.name);
        let held = !in_force
            .holding(&RunKind::Automation(automation))
            .is_empty();
        if next_run(automation, last_scheduled_run, now) == NextRun::Due && !running && !held {
            due.push(automation);
        }
    }
    due.sort_by(|a, b| a.name.cmp(&b.name));
    let runs: Vec<Action> = due
        .into_iter()
        .take(room)
        .map(|automation| {
            last_scheduled_runs.record(&automation.name, now);
            Action::RunAutomation(Box::new(automation.clone()))
        })
        .collect();
    let mut actions = runs;
    if last_scheduled_runs != automations.last_scheduled_runs {
        actions.push(Action::RecordLastScheduledRuns(last_scheduled_runs));
    }
    actions
}

/// The items the live runs hold, each with its run's stage: those a run
/// writes as it records its result, which no other write may touch until it
/// ends. A run on an issue also holds Linewright's open pull requests that
/// close it, but not the tickets it files.
fn held_by_runs(
    snapshot: &Snapshot,
    live: &[LiveRun],
    linewright: &Login,
) -> BTreeMap<Item, WorkKind> {
    let mut held = BTreeMap::new();
    for (subject, stage) in live.iter().map(|run| (run.subject, run.kind)) {
        match subject {
            Subject::Pull(pr) => {
                held.insert(Item::Pull(pr), stage);
            }
            Subject::Issue(issue) => {
                held.insert(Item::Issue(issue), stage);
                for pull in snapshot
                    .pulls
                    .iter()
                    .filter(|p| p.is_open() && p.author == *linewright && p.closes.contains(&issue))
                {
                    held.insert(Item::Pull(pull.number), stage);
                }
            }
        }
    }
    held
}

/// The items of `action` that `held_by_runs` holds, each with its run's
/// stage: none for a cancel, which no live run holds back.
fn held_back(
    action: &Action,
    snapshot: &Snapshot,
    held_by_runs: &BTreeMap<Item, WorkKind>,
) -> Vec<(Item, WorkKind)> {
    if let Action::Cancel { .. } = action {
        return vec![];
    }
    action
        .items(snapshot)
        .into_iter()
        .filter_map(|item| held_by_runs.get(&item).map(|stage| (item, *stage)))
        .collect()
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
        Subject::Issue(number) => {
            let Some(issue) = snapshot.issue(number) else {
                return Some(format!("{number} was closed"));
            };
            cancels(run.kind, issue, &policy.linewright).map(|cancel| match cancel {
                Cancel::Closed => format!("{number} was closed"),
                Cancel::Relabeled(label) => {
                    format!("`{}` was removed from {number}", label.label())
                }
                Cancel::Unassigned => {
                    format!("{} was unassigned from {number}", policy.linewright)
                }
            })
        }
    }
}

struct Plan<'a> {
    /// What holds back the starts beyond the room for them.
    ceiling: Ceiling,
    /// The holds in force at this poll.
    in_force: &'a InForce<'a>,
    snapshot: &'a Snapshot,
    /// What [`held_by_runs`] returns: no action but a cancel touches these.
    held_by_runs: &'a BTreeMap<Item, WorkKind>,
    actions: Vec<Action>,
    starts: Vec<Start>,
    waits: BTreeMap<Number, Vec<Wait>>,
}

/// The ceiling on the runs in flight, `max-runs`.
#[derive(Clone, Copy)]
struct Ceiling {
    in_flight: usize,
    max_runs: usize,
}

struct Start {
    claim: Option<Item>,
    work: Work,
}

impl Plan<'_> {
    /// Takes `action` unless a live run holds one of its items, which then
    /// waits for the run instead: a run is its items' only writer until it
    /// ends.
    fn act(&mut self, action: Action) {
        if !self.held_back(&[&action]) {
            self.actions.push(action);
        }
    }

    /// Whether a live run holds back one of `actions`, noting the wait on
    /// each item it holds.
    fn held_back(&mut self, actions: &[&Action]) -> bool {
        let held: Vec<(Item, WorkKind)> = actions
            .iter()
            .flat_map(|action| held_back(action, self.snapshot, self.held_by_runs))
            .collect();
        for (item, stage) in &held {
            self.wait(item.number(), Wait::Run { stage: *stage });
        }
        !held.is_empty()
    }

    /// A wait already noted for `item` is noted once.
    fn wait(&mut self, item: Number, wait: Wait) {
        let waits = self.waits.entry(item).or_default();
        if !waits.contains(&wait) {
            waits.push(wait);
        }
    }

    /// A claim on its own, outside any start, which a hold over claims
    /// holds back too.
    fn claim(&mut self, item: Item) {
        if let Some(hold) = self.in_force.holds_claims() {
            self.wait(item.number(), hold.into());
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
        self.push_start(Start { claim: None, work });
    }

    fn claim_and_start(&mut self, item: Item, work: Work) {
        self.push_start(Start {
            claim: Some(item),
            work,
        });
    }

    /// A start that a live run or a hold holds is neither claimed nor
    /// started, and its item waits for the run or the holds it notes.
    fn push_start(&mut self, start: Start) {
        let claim = start.claim.map(|item| Action::Claim { item });
        let run = Action::Start(start.work.clone());
        if self.held_back(&claim.iter().chain([&run]).collect::<Vec<_>>()) {
            return;
        }
        let holds = self.in_force.holding(&RunKind::Stage(start.work.kind()));
        if holds.is_empty() {
            self.starts.push(start);
        } else {
            let waits = noted(holds).into_iter().map(Wait::from).collect();
            self.wait_all(start.work.subject().number(), waits);
        }
    }

    /// Also returns the room left.
    fn into_decisions(mut self, room: usize) -> (Decisions, usize) {
        let mut starts = std::mem::take(&mut self.starts);
        starts.sort_by_key(|s| (rank(s.work.kind()), s.work.subject()));
        let room_left = room.saturating_sub(starts.len());
        let started = starts.len().min(room);
        let wait = Wait::FreeRun {
            in_flight: self.ceiling.in_flight + started,
            max_runs: self.ceiling.max_runs,
        };
        for start in starts.iter().skip(room) {
            self.wait(start.work.subject().number(), wait.clone());
        }
        let mut actions = self.actions;
        for start in starts.into_iter().take(room) {
            if let Some(item) = start.claim {
                actions.push(Action::Claim { item });
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

/// The holds among `holds`, in rank order, that a held start notes as its
/// item's waits: the files where any hold it, and otherwise the first.
fn noted(mut holds: Vec<Hold>) -> Vec<Hold> {
    match holds.iter().position(|hold| matches!(hold, Hold::File(_))) {
        Some(first_file) => holds.split_off(first_file),
        None => holds.into_iter().take(1).collect(),
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

/// Works `issue` by the row of the Working an item table that holds for it
/// (`docs/specs/repository.md`), noting what it waits for. `discard` is the
/// discard of its cut, where it has one to discard.
fn work_issue(
    issue: &Issue,
    discard: Option<&Discard>,
    running: &impl Fn(Subject) -> Option<WorkKind>,
    policy: &Policy,
    plan: &mut Plan,
) {
    let linewright = &policy.linewright;
    let number = issue.number;
    let closing = plan.snapshot.pulls_closing(issue);
    let merge = done_by_merge(&issue.events, &closing, &plan.snapshot.default_branch);
    let run = running(Subject::Issue(number));
    let state = issue_state(issue, &closing, merge, run, linewright);
    let facts = IssueFacts {
        issue,
        closing: &closing,
        run_in_flight: run.is_some(),
        discard,
    };
    if let Some(stage) = run {
        plan.wait(number, Wait::Run { stage });
    }
    standing_waits(issue, state, merge, &closing, policy, plan);
    let Some(holding) = row(&facts, state, policy) else {
        if run.is_none() {
            idle_waits(issue, state, policy, plan);
        }
        return;
    };
    match holding {
        Row::DiscardCut(cut) => plan.act(Action::DiscardCut(cut)),
        Row::HandBack { pull } => plan.act(Action::HandBack {
            item: Item::Issue(number),
            kind: HandBackKind::Decision,
            reason: format!("{pull} was closed without merging."),
        }),
        Row::LetGo { comment } => let_go(number, comment, plan),
        Row::UnassignAsked { people } => {
            let mut once_unassigned = issue.clone();
            once_unassigned.assignees.retain(|a| !people.contains(a));
            idle_waits(&once_unassigned, state, policy, plan);
            plan.act(Action::UnassignAsked {
                issue: number,
                assignees: people,
            });
        }
        Row::CleanUpTracking {
            labels,
            let_go: lets_go,
        } => {
            remove_status_labels(number, labels, plan);
            if lets_go {
                let_go(number, None, plan);
            }
        }
        Row::CloseTracking => plan.act(Action::CloseTracking { issue: number }),
        Row::TakeReply => plan.act(Action::Relabel {
            issue: number,
            to: StatusLabel::NeedsTriage,
        }),
        Row::Claim => {
            let mut claimed = issue.clone();
            claimed.assignees.push(linewright.clone());
            let claimed_state = issue_state(&claimed, &closing, merge, None, linewright);
            let facts = IssueFacts {
                issue: &claimed,
                ..facts
            };
            if let Some(Row::Start(stage)) = row(&facts, claimed_state, policy) {
                plan.claim_and_start(Item::Issue(number), issue_work(stage, number));
            } else {
                plan.claim(Item::Issue(number));
            }
        }
        Row::Start(stage) => plan.start(issue_work(stage, number)),
        Row::RemoveStaleStatusLabels(labels) => remove_status_labels(number, labels, plan),
        Row::ClearStall => plan.act(Action::ClearStall {
            item: Item::Issue(number),
        }),
    }
}

/// The run of `stage` on `issue`.
fn issue_work(stage: IssueStage, issue: Number) -> Work {
    match stage {
        IssueStage::Triage => Work::Triage { issue },
        IssueStage::Design => Work::DesignIssue { issue },
        IssueStage::Implement => Work::Implement { issue },
    }
}

fn remove_status_labels(issue: Number, labels: Vec<StatusLabel>, plan: &mut Plan) {
    for label in labels {
        plan.act(Action::RemoveStatusLabel { issue, label });
    }
}

fn let_go(issue: Number, comment: Option<String>, plan: &mut Plan) {
    plan.act(Action::LetGo { issue, comment });
}

/// What `issue`, in `state`, waits for whatever Linewright does: the pull
/// requests that close it, its tickets, a person, or, done by `merge`, its
/// label applied again. In review with no open pull request of Linewright's,
/// an issue holding `ready-for-agent` waits for its blockers.
fn standing_waits(
    issue: &Issue,
    state: Option<State>,
    merge: Option<Merge>,
    closing: &[ClosingPull],
    policy: &Policy,
    plan: &mut Plan,
) {
    let number = issue.number;
    match state {
        Some(State::InReview) => {
            let mut linewrights = false;
            for pull in closing.iter().filter(|p| p.is_open()) {
                let pull_request = pull.number;
                let wait = if pull.author == policy.linewright {
                    linewrights = true;
                    Wait::PullRequest { pull_request }
                } else {
                    Wait::ClosingPullRequest { pull_request }
                };
                plan.wait(number, wait);
            }
            if !linewrights && issue.holds(StatusLabel::ReadyForAgent) {
                wait_for_blockers(issue, plan);
            }
        }
        Some(State::Tracking) => {
            let open = open_numbers(issue.sub_issues.iter().map(SubIssue::linked));
            if !open.is_empty() {
                plan.wait(number, Wait::Tickets { open });
            }
        }
        Some(State::NeedsInfo | State::ReadyForHuman) => {
            for wait in labeled_waits(issue, policy) {
                plan.wait(number, wait);
            }
        }
        // GitHub closes the issue shortly after the merge
        // (https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/linking-a-pull-request-to-an-issue);
        // until then, only applying a status label again asks for more work.
        Some(State::Done) => {
            if let Some(merge) = merge {
                plan.wait(
                    number,
                    Wait::Relabel {
                        label: issue.status_label().unwrap_or(StatusLabel::ReadyForAgent),
                        since: Relabel::Merged {
                            pull_request: merge.pull_request,
                        },
                    },
                );
            }
        }
        _ => {}
    }
}

/// What `issue`, in `state`, with no run in flight and no row holding for
/// it, waits for before Linewright claims it or starts its run.
fn idle_waits(issue: &Issue, state: Option<State>, policy: &Policy, plan: &mut Plan) {
    match state {
        Some(State::NeedsTriage | State::ReadyForAgent) => {
            if let Some(status) = issue.status_label()
                && let Err(not_claimed) = may_claim(issue, status, policy)
            {
                let waits = match not_claimed {
                    NotClaimed::Blocked(blockers) => blockers
                        .into_iter()
                        .map(|blocker| Wait::Blocker { issue: blocker })
                        .collect(),
                    NotClaimed::Assigned(assignees) => assignees
                        .into_iter()
                        .map(|assignee| Wait::Assignee { assignee })
                        .collect(),
                    NotClaimed::Unassigned => vec![Wait::Relabel {
                        label: status,
                        since: Relabel::Unassigned {
                            required_label: policy.require_label.clone(),
                        },
                    }],
                    NotClaimed::Unmarked => required_label(policy).into_iter().collect(),
                };
                plan.wait_all(issue.number, waits);
            }
        }
        Some(State::BeingTriaged | State::BeingDesigned | State::BeingImplemented) => {
            wait_for_blockers(issue, plan);
        }
        _ => {}
    }
}

fn labeled_waits(issue: &Issue, policy: &Policy) -> Vec<Wait> {
    let handed_to = if issue.holds(StatusLabel::NeedsInfo) {
        Some(Wait::PeopleAsked)
    } else if issue.holds(StatusLabel::ReadyForHuman) {
        Some(Wait::Maintainer)
    } else {
        return vec![];
    };
    let unlabeled = required_label(policy).filter(|_| !policy.marks(&issue.labels));
    handed_to.into_iter().chain(unlabeled).collect()
}

/// Every cut of the snapshot to discard (docs/specs/design.md, "Discarding a
/// cut"): that of each open issue with no run in flight that has tickets cut
/// from it but lacks `linewright:designed`.
fn discards(
    snapshot: &Snapshot,
    running: &impl Fn(Subject) -> Option<WorkKind>,
    policy: &Policy,
) -> Vec<Discard> {
    snapshot
        .issues
        .iter()
        .filter(|i| {
            i.open && !i.has_label(label::DESIGNED) && running(Subject::Issue(i.number)).is_none()
        })
        .map(|i| cut_to_discard(snapshot, i, policy))
        .filter(|d| !d.tickets.is_empty())
        .collect()
}

/// The discard of `issue`'s cut.
fn cut_to_discard(snapshot: &Snapshot, issue: &Issue, policy: &Policy) -> Discard {
    let tickets: Vec<Linked> = issue
        .tickets_cut(&policy.linewright)
        .map(SubIssue::linked)
        .collect();
    let pulls = snapshot
        .pulls
        .iter()
        .filter(|p| p.is_open() && p.author == policy.linewright)
        .filter(|p| tickets.iter().any(|t| p.closes.contains(&t.number)))
        .map(|p| p.number)
        .collect();
    Discard {
        issue: issue.number,
        pulls,
        tickets,
    }
}

fn open_numbers(linked: impl IntoIterator<Item = Linked>) -> Vec<Number> {
    linked
        .into_iter()
        .filter(|l| l.open)
        .map(|l| l.number)
        .collect()
}

fn required_label(policy: &Policy) -> Option<Wait> {
    policy
        .require_label
        .as_ref()
        .map(|label| Wait::RequiredLabel {
            label: label.clone(),
        })
}

/// Returns whether any blocker is open.
fn wait_for_blockers(issue: &Issue, plan: &mut Plan) -> bool {
    let blockers = issue.open_blockers();
    for &blocker in &blockers {
        plan.wait(issue.number, Wait::Blocker { issue: blocker });
    }
    !blockers.is_empty()
}

/// What Pull requests asks of a pull request Linewright works
/// (`docs/specs/pull_requests.md`), one at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PullWork {
    Start(Work),
    Act(Action),
    /// Nothing yet: it waits for this.
    Wait(Wait),
}

/// What Pull requests asks of `pr`, Linewright's pull request for `ticket`,
/// at `now`: what the first row of Working a pull request that holds does.
fn pull_work(
    ticket: Number,
    pr: &Pull,
    memory: &Memory,
    policy: &Policy,
    now: Timestamp,
) -> PullWork {
    let pr_number = pr.number;
    let hand_back = |reason: String| {
        PullWork::Act(Action::HandBack {
            item: Item::Pull(pr_number),
            kind: HandBackKind::Stall,
            reason,
        })
    };
    let ci = Wait::Ci {
        pull_request: pr_number,
    };
    let merge_state = Wait::MergeState {
        pull_request: pr_number,
    };
    let facts = working_pull::facts(pr, memory, policy, now);
    match working_pull::row(&facts) {
        WorkingRow::Rework => PullWork::Start(Work::Rework {
            issue: ticket,
            pr: pr_number,
            update: working_pull::rework_starts_with_update(&facts),
        }),
        WorkingRow::Update => PullWork::Start(Work::Update {
            issue: ticket,
            pr: pr_number,
        }),
        WorkingRow::WaitForChecks => PullWork::Wait(ci),
        WorkingRow::HandBackTimedOut { running } => hand_back(format!(
            "CI did not complete within {}: {} still running.",
            Phrase::default().span(policy.ci_timeout, None),
            code_list(&running)
        )),
        WorkingRow::FixCi { failing } => PullWork::Start(Work::FixCi {
            issue: ticket,
            pr: pr_number,
            failing,
        }),
        WorkingRow::HandBackFailing { failing, attempts } => hand_back(format!(
            "{} still failing after {attempts} fix attempts.",
            code_list(&failing)
        )),
        WorkingRow::MarkReady => PullWork::Act(Action::MarkReady { pr: pr_number }),
        WorkingRow::WaitForReview => PullWork::Wait(Wait::Review {
            pull_request: pr_number,
        }),
        WorkingRow::HandBackMergeFailed { reason } => hand_back(format!(
            "Merging {pr_number} failed twice. GitHub's reason: {reason}"
        )),
        WorkingRow::CommentRequirements { requires } => {
            PullWork::Act(Action::CommentRequirements {
                pr: pr_number,
                requires,
            })
        }
        WorkingRow::WaitForRequirements { requires } => PullWork::Wait(Wait::Requirements {
            pull_request: pr_number,
            requires,
        }),
        WorkingRow::WaitForMergeState => PullWork::Wait(merge_state),
        WorkingRow::Merge { method } => PullWork::Act(Action::Merge {
            pr: pr_number,
            head: pr.head_sha.clone(),
            method,
        }),
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
        PullState::Open { .. } => match PullClaim::of(&pr.assignees, &pr.assignments, linewright) {
            PullClaim::Unassigned => Some(PullStop::Unassigned),
            PullClaim::Claimed | PullClaim::NeverAssigned => None,
        },
    }
}

pub fn rebases_to_repair(pr: &Pull, policy: &Policy) -> bool {
    pr.rebaseability == Rebaseability::NotRebaseable && policy.rebases_unrebaseable()
}

pub fn requirements_note(requires: &[Requirement]) -> String {
    let required = if requires.is_empty() {
        "GitHub blocks the merge for a reason Linewright cannot read: the default branch's \
         rules say what it needs"
            .to_string()
    } else {
        let names = Phrase::list(requires.iter().map(Requirement::describe), "and");
        format!("GitHub requires {names} before merging")
    };
    format!("{required}. Linewright merges it once GitHub allows it.")
}

fn code_list(names: &[String]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
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
    /// Taken up by its issue, `issue`.
    TakenUp {
        issue: Number,
    },
    /// Taken up by carrying `label`.
    Designed {
        label: DesignedLabel,
    },
    BeforeLookback {
        merged_at: Timestamp,
        window_opened: Timestamp,
    },
    RequiredLabel {
        label: String,
    },
}

/// Whether merged `pr` is a spec change Linewright may take up, of those
/// `snapshot` holds at `now`, or why not (`docs/specs/sources_of_work.md`,
/// Spec changes): why it is no spec change, checked first, then why it may
/// not be taken up.
pub fn spec_change(
    pr: &MergedPull,
    snapshot: &Snapshot,
    now: Timestamp,
    policy: &Policy,
) -> Result<(), NotSpecChange> {
    let window_opened = policy.windows(now).lookback_opened();
    let because = if pr.author == policy.linewright {
        NotSpecChange::Linewrights
    } else if pr.base_ref != snapshot.default_branch {
        NotSpecChange::OtherBranch {
            base: pr.base_ref.clone(),
        }
    } else if !pr.files.iter().any(|f| policy.spec_glob.matches(f)) {
        NotSpecChange::NoSpecFile {
            spec_glob: policy.spec_glob.pattern().to_string(),
        }
    } else if let Some(issue) = snapshot.spec_change_issue(pr.number) {
        NotSpecChange::TakenUp { issue }
    } else if let Some(label) = pr.designed_label() {
        NotSpecChange::Designed { label }
    } else if pr.merged_at < window_opened {
        NotSpecChange::BeforeLookback {
            merged_at: pr.merged_at,
            window_opened,
        }
    } else if let Some(label) = policy.require_label.as_ref().filter(|l| !pr.has_label(l)) {
        NotSpecChange::RequiredLabel {
            label: label.clone(),
        }
    } else {
        return Ok(());
    };
    Err(because)
}
