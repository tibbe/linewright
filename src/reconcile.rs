use std::collections::{BTreeMap, BTreeSet};

use chrono::TimeDelta;
use serde::{Deserialize, Serialize};

use crate::automation::{Automation, AutomationName, LastScheduledRuns};
use crate::domain::{
    Assignment, BranchIssue, CheckState, ClosingPull, Comment, DesignedLabel, HandBackKind, Issue,
    Item, Linked, Login, Marking, Merge, MergeMethod, MergeMethods, Mergeability, MergedPull,
    Number, Pull, PullState, Rebaseability, Requirement, Review, ReviewState, Snapshot,
    StatusLabel, SubIssue, Thread, TicketBranchName, Timestamp, UpdateMethod, UpdateRule, Windows,
    label,
};
use crate::harness::UsageLimit;
use crate::holds::Holds;
use crate::lifecycle::{
    Cancel, IssueFacts, IssueStage, NotClaimed, Opener, PullClaim, PullFacts, PullRow, PullWorking,
    Row, State, cancels, done_by_merge, issue_state, may_claim, pull_row, pull_state, row,
};
use crate::phrase::Phrase;
use crate::repo_files::{FileError, Unusable, holding};
use crate::spec_glob::SpecGlob;
use crate::work_settings::RunKind;

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
    DesignSpecChange {
        pr: Number,
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
            Work::Triage { issue } | Work::DesignIssue { issue } | Work::Implement { issue } => {
                Subject::Issue(*issue)
            }
            Work::DesignSpecChange { pr } => Subject::SpecChange(*pr),
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
            Work::DesignSpecChange { pr }
            | Work::Rework { pr, .. }
            | Work::FixCi { pr, .. }
            | Work::Update { pr, .. } => *pr,
        }
    }

    pub fn starts_from(&self) -> Item {
        match self {
            Work::Triage { issue } | Work::DesignIssue { issue } | Work::Implement { issue } => {
                Item::Issue(*issue)
            }
            Work::DesignSpecChange { pr }
            | Work::Rework { pr, .. }
            | Work::FixCi { pr, .. }
            | Work::Update { pr, .. } => Item::Pull(*pr),
        }
    }

    pub fn kind(&self) -> WorkKind {
        match self {
            Work::Triage { .. } => WorkKind::Triage,
            Work::DesignSpecChange { .. } | Work::DesignIssue { .. } => WorkKind::Design,
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
    /// Remove [`label::STALLED`] from an item Linewright works again.
    ClearStall {
        item: Item,
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
                .chain([discard.item.parent()])
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
            | Action::MarkReadyForMergeState { pr }
            | Action::HoldForReview { pr, .. } => vec![Item::Pull(*pr)],
            Action::HandBack { item, .. } | Action::ClearStall { item } => vec![*item],
            Action::DeleteBranch { branch } => vec![Item::Issue(branch.issue())],
            Action::Cancel { .. }
            | Action::RememberRebase(_)
            | Action::RecordLastScheduledRuns(_)
            | Action::RunAutomation(_) => vec![],
        }
    }
}

/// Close `pulls`, then the open `tickets` as not planned, then remove
/// `tickets` from the sub-issues of the issue they were cut from, leaving
/// `item` to be designed again (docs/specs/design.md, "Discarding a cut"). A
/// person's pull request closing a ticket, and a sub-issue a person added,
/// are left as they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discard {
    pub item: CutItem,
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
        let designed = self.item.designed();
        let discarded =
            |what: &str| format!("{designed} is being designed again, so this {what} discarded.");
        let (on_issue, closing_tracking) = match self.item {
            CutItem::SpecChange { tracking, .. } => (
                None,
                Some(ParentNote {
                    issue: tracking,
                    note: discarded("tracking issue and its tickets are"),
                }),
            ),
            CutItem::Issue(issue) => {
                let tickets = self.open_tickets();
                let mut pulls = self.pulls.clone();
                pulls.sort_unstable();
                let note = (!tickets.is_empty() || !pulls.is_empty()).then(|| ParentNote {
                    issue,
                    note: discarded_from_issue(&tickets, &pulls),
                });
                (note, None)
            }
        };
        DiscardNotes {
            on_pull: discarded("pull request for a ticket of its earlier cut is"),
            on_ticket: discarded("ticket from its earlier cut is"),
            on_issue,
            closing_tracking,
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
    /// On a spec change's tracking issue, closed once its tickets left it.
    pub closing_tracking: Option<ParentNote>,
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

/// An item cut into tickets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutItem {
    /// Its tickets are under `tracking`, an issue Linewright opened for them.
    SpecChange { pr: Number, tracking: Number },
    /// Its tickets are under the issue itself.
    Issue(Number),
}

impl CutItem {
    /// The item designed again: the spec change or the issue.
    pub fn designed(self) -> Number {
        match self {
            CutItem::SpecChange { pr, .. } => pr,
            CutItem::Issue(issue) => issue,
        }
    }

    /// The issue its tickets are under.
    pub fn parent(self) -> Number {
        match self {
            CutItem::SpecChange { tracking, .. } => tracking,
            CutItem::Issue(issue) => issue,
        }
    }
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
    PeopleAsked,
    Maintainer,
    /// Linewright was unassigned from the item and is not assigned to it
    /// again. Applying `required_label`, when set, picks a spec change up
    /// again too.
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
    let held_by_runs = held_by_runs(snapshot, live, memory, &policy.linewright);
    let mut plan = Plan {
        held,
        unusable,
        snapshot,
        held_by_runs: &held_by_runs,
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

    let discards = discards(snapshot, &running, policy);
    let discarded: Vec<Number> = discards
        .iter()
        .filter_map(|d| match d.item {
            CutItem::SpecChange { pr, .. } => Some(pr),
            CutItem::Issue(_) => None,
        })
        .collect();
    let discarded_tickets: Vec<Number> = discards.iter().flat_map(Discard::open_tickets).collect();
    let discarded_pulls: Vec<Number> = discards.iter().flat_map(|d| d.pulls.clone()).collect();
    // An issue's own cut is discarded by its row of the Working an item
    // table, below.
    for discard in discards
        .iter()
        .filter(|d| matches!(d.item, CutItem::SpecChange { .. }))
    {
        plan.act(Action::DiscardCut(discard.clone()));
    }

    let spec_changes = pending_spec_changes(snapshot, now, policy);
    let pending: Vec<Number> = spec_changes.iter().map(|pr| pr.number).collect();
    for pr in spec_changes {
        let work = Work::DesignSpecChange { pr: pr.number };
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
        let work = in_review(ticket, pr, memory, policy, now);
        let working = match (run, &work) {
            (Some(_), _) => PullWorking::RunInFlight,
            (None, PullWork::Wait(_)) => PullWorking::Waiting,
            (None, PullWork::Start(_) | PullWork::StartRebase(..) | PullWork::Act(_)) => {
                PullWorking::Asked
            }
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

    for (number, facts, state) in others_pulls(snapshot, now, policy) {
        let holding = pull_row(&facts, state, &policy.linewright);
        work_pull(number, holding, None, &mut plan);
    }

    for issue in snapshot
        .issues
        .iter()
        .filter(|i| i.open && !discarded_tickets.contains(&i.number))
    {
        // A spec change's tracking issue is cut, and its cut discarded, with
        // the spec change.
        if let Some(spec_change) = issue.spec_change() {
            if let Some(stage) = running(Subject::SpecChange(spec_change)) {
                plan.wait(issue.number, Wait::Run { stage });
                continue;
            }
            if pending.contains(&spec_change) || discarded.contains(&spec_change) {
                continue;
            }
        }
        let discard = discards
            .iter()
            .find(|d| d.item == CutItem::Issue(issue.number));
        work_issue(issue, discard, &running, policy, &mut plan);
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
    decisions.actions = clearing_stalls(
        snapshot,
        decisions.actions,
        &held_by_runs,
        &policy.linewright,
    );
    decisions
        .actions
        .extend(schedule(automations, now, room, unusable));
    decisions
}

/// The pull requests of others' Linewright is assigned to, with what the
/// Working an item table reads of each and its state: the open ones, and
/// the merged ones, which may be spec changes.
fn others_pulls(
    snapshot: &Snapshot,
    now: Timestamp,
    policy: &Policy,
) -> Vec<(Number, PullFacts, State)> {
    let linewright = &policy.linewright;
    let facts = |opener: Opener| PullFacts {
        opener,
        claim: PullClaim::Claimed,
        working: PullWorking::Waiting,
        stalled: false,
    };
    let open = snapshot
        .peoples_pulls
        .iter()
        .filter(|p| p.is_open() && &p.author != linewright && p.is_assigned(linewright))
        .map(|p| (p.number, facts(Opener::Person), State::Open));
    let merged = snapshot
        .merged
        .iter()
        .filter(|p| p.is_assigned(linewright))
        .filter_map(|p| {
            let opener = match spec_change(p, &snapshot.default_branch, now, policy) {
                Err(NotSpecChange::Linewrights) => return None,
                Err(NotSpecChange::OtherBranch { .. } | NotSpecChange::NoSpecFile { .. }) => {
                    Opener::Person
                }
                Ok(_)
                | Err(
                    NotSpecChange::Designed { .. }
                    | NotSpecChange::BeforeLookback { .. }
                    | NotSpecChange::RequiredLabel { .. },
                ) => Opener::SpecChange,
            };
            Some((p.number, facts(opener), State::Merged))
        });
    open.chain(merged).collect()
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
        Some(PullWork::StartRebase(work, rebase)) if works => plan.start_rebase(work, rebase),
        Some(PullWork::Act(action)) if works => plan.act(action),
        Some(PullWork::Start(_) | PullWork::StartRebase(..) | PullWork::Act(_)) | None => {}
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
            Action::Start(_)
            | Action::Claim { .. }
            | Action::RememberRebase(_)
            | Action::RunAutomation(_) => None,
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
            | Action::RemoveStatusLabel { .. }
            | Action::HandBack { .. }
            | Action::MarkReady { .. }
            | Action::Merge { .. }
            | Action::MarkReadyForMergeState { .. }
            | Action::HoldForReview { .. }
            | Action::CloseTracking { .. }
            | Action::DiscardCut(_)
            | Action::DeleteBranch { .. } => Some(action),
        })
        .collect()
}

/// `actions`, with the stall of each spec change Linewright works again
/// cleared: one it is assigned to, or claims in `actions`
/// (`docs/specs/repository.md`, "Stalls"), and that no live run holds. Each
/// clear follows its claim, so that a stop before the claim leaves the
/// label. An issue's and a pull request's stalls are cleared by their rows
/// of the Working an item table.
fn clearing_stalls(
    snapshot: &Snapshot,
    actions: Vec<Action>,
    held_by_runs: &BTreeMap<Item, WorkKind>,
    linewright: &Login,
) -> Vec<Action> {
    let spec_change = |item: Item| match item {
        Item::Pull(number) => snapshot.merged_pull(number),
        Item::Issue(_) => None,
    };
    let mut cleared: Vec<Action> = snapshot
        .merged
        .iter()
        .filter(|p| p.is_stalled() && p.is_assigned(linewright))
        .map(|p| Action::ClearStall {
            item: Item::Pull(p.number),
        })
        .filter(|clear| held_back(clear, snapshot, held_by_runs).is_empty())
        .collect();
    for action in actions {
        let clear = if let Action::Claim { item } = action
            && spec_change(item).is_some_and(MergedPull::is_stalled)
        {
            Some(Action::ClearStall { item })
        } else {
            None
        };
        cleared.push(action);
        cleared.extend(clear);
    }
    cleared
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

/// An automation held by a file in `unusable` stays due, to run once the
/// file is fixed.
fn schedule(
    automations: &Automations,
    now: Timestamp,
    room: usize,
    unusable: &[Unusable],
) -> Vec<Action> {
    let mut last_scheduled_runs = automations.last_scheduled_runs.clone();
    let mut due = vec![];
    for automation in &automations.defined {
        let Some(last_scheduled_run) = automations.last_scheduled_runs.get(&automation.name) else {
            last_scheduled_runs.record(&automation.name, now);
            continue;
        };
        let running = automations.in_flight.contains(&automation.name);
        let held = !holding(unusable, &RunKind::Automation(automation)).is_empty();
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
    // The record follows the runs: a stop between the two leaves the
    // automation due, to run again after the restart, rather than recorded as
    // having had a scheduled run with no run started.
    let mut actions = runs;
    if last_scheduled_runs != automations.last_scheduled_runs {
        actions.push(Action::RecordLastScheduledRuns(last_scheduled_runs));
    }
    actions
}

/// The items the live runs hold, each with its run's stage: those a run
/// writes as it records its result, which no other write may touch until it
/// ends. A cut in `memory.ended_cuts` holds what it held while live, as the
/// snapshot may miss what it wrote.
///
/// The tickets a cut files are not held: each is readied last, so a poll
/// sees it either unpublished or published to be worked.
fn held_by_runs(
    snapshot: &Snapshot,
    live: &[LiveRun],
    memory: &Memory,
    linewright: &Login,
) -> BTreeMap<Item, WorkKind> {
    let ended_cuts = memory
        .ended_cuts
        .iter()
        .map(|pr| (Subject::SpecChange(*pr), WorkKind::Design));
    let mut held = BTreeMap::new();
    for (subject, stage) in live
        .iter()
        .map(|run| (run.subject, run.kind))
        .chain(ended_cuts)
    {
        match subject {
            Subject::Pull(pr) => {
                held.insert(Item::Pull(pr), stage);
            }
            // An implement run opens its pull request, then assigns and
            // labels it.
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
            Subject::SpecChange(pr) => {
                held.insert(Item::Pull(pr), stage);
                for tracking in snapshot
                    .issues
                    .iter()
                    .filter(|i| i.open && i.spec_change() == Some(pr))
                {
                    held.insert(Item::Issue(tracking.number), stage);
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
    held: Held,
    /// The repository's files that hold the runs that would read them.
    unusable: &'a [Unusable],
    snapshot: &'a Snapshot,
    /// What [`held_by_runs`] returns: no action but a cancel touches these.
    held_by_runs: &'a BTreeMap<Item, WorkKind>,
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

impl Plan<'_> {
    /// An action on an item a live run holds is not taken, and the item
    /// waits for the run instead, so that the run is the item's only writer
    /// until it ends. The checks of `running` in [`decisions`] give the
    /// waits and keep work from being planned; this is what guarantees that
    /// nothing is written.
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
        self.push_start(Start {
            claim: None,
            rebase: None,
            work,
        });
    }

    fn claim_and_start(&mut self, item: Item, work: Work) {
        self.push_start(Start {
            claim: Some(item),
            rebase: None,
            work,
        });
    }

    fn start_rebase(&mut self, work: Work, rebase: TriedRebase) {
        self.push_start(Start {
            claim: None,
            rebase: Some(rebase),
            work,
        });
    }

    /// A start that a live run or a file in `unusable` holds is neither
    /// claimed nor started, and its item waits for the run or the file.
    fn push_start(&mut self, start: Start) {
        let claim = start.claim.map(|item| Action::Claim { item });
        let run = Action::Start(start.work.clone());
        if self.held_back(&claim.iter().chain([&run]).collect::<Vec<_>>()) {
            return;
        }
        let waits: Vec<Wait> = holding(self.unusable, &RunKind::Stage(start.work.kind()))
            .into_iter()
            .map(|file| Wait::RepositoryFile(file.clone()))
            .collect();
        if waits.is_empty() {
            self.starts.push(start);
        } else {
            self.wait_all(start.work.subject().number(), waits);
        }
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
            // What it waits for once they are unassigned, such as someone
            // else assigned.
            let mut without = issue.clone();
            without.assignees.retain(|a| !people.contains(a));
            idle_waits(&without, state, policy, plan);
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
            // The claim leaves the issue in a state whose row starts its run,
            // and both are made in this poll, so that a hold keeps an issue
            // it cannot start unclaimed.
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
/// label applied again.
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
            // Someone else's pull request may be closed with the ticket left
            // to Linewright, which then waits for its blockers; once
            // Linewright's ends, they hold nothing back.
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
        // GitHub closes the issue shortly after the merge; until then, only
        // applying a status label again asks for more work.
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
/// cut"), decided before the rest of the plan. An item is labeled designed
/// last, so one cut into tickets without the label is a cut that stopped part
/// way (interrupted, cancelled or handed back) or one whose label was removed
/// to have it designed again.
fn discards(
    snapshot: &Snapshot,
    running: &impl Fn(Subject) -> Option<WorkKind>,
    policy: &Policy,
) -> Vec<Discard> {
    let undesigned: BTreeSet<Number> = snapshot
        .merged
        .iter()
        .filter(|pr| pr.designed_label().is_none())
        .map(|pr| pr.number)
        .collect();
    // A spec change's cut, in flight or discarded with the spec change, is
    // the spec change's, its tracking issue's label or not.
    let spec_changes_cut = |spec_change: Number| {
        undesigned.contains(&spec_change) || running(Subject::SpecChange(spec_change)).is_some()
    };
    let spec_changes = undesigned
        .iter()
        .filter(|&&pr| running(Subject::SpecChange(pr)).is_none())
        .filter_map(|&pr| {
            let tracking = snapshot
                .issues
                .iter()
                .find(|i| i.open && i.spec_change() == Some(pr))?;
            let item = CutItem::SpecChange {
                pr,
                tracking: tracking.number,
            };
            Some(cut_to_discard(snapshot, item, tracking, policy))
        });
    let issues = snapshot
        .issues
        .iter()
        .filter(|i| {
            i.open
                && !i.has_label(label::DESIGNED)
                && running(Subject::Issue(i.number)).is_none()
                && !i.spec_change().is_some_and(spec_changes_cut)
        })
        .map(|i| cut_to_discard(snapshot, CutItem::Issue(i.number), i, policy))
        .filter(|d| !d.tickets.is_empty());
    spec_changes.chain(issues).collect()
}

/// The discard of `item`'s cut, whose tickets are under `parent`.
fn cut_to_discard(snapshot: &Snapshot, item: CutItem, parent: &Issue, policy: &Policy) -> Discard {
    let tickets: Vec<Linked> = parent
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
        item,
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
    /// Start the update, remembering that it rebases the head it was
    /// decided at.
    StartRebase(Work, TriedRebase),
    Act(Action),
    /// Nothing yet: it waits for this.
    Wait(Wait),
}

/// What Pull requests asks of `pr`, Linewright's pull request for `ticket`,
/// at `now`.
fn in_review(
    ticket: Number,
    pr: &Pull,
    memory: &Memory,
    policy: &Policy,
    now: Timestamp,
) -> PullWork {
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
        return PullWork::Start(Work::Rework {
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
            at: now,
        };
        return PullWork::StartRebase(update, rebase);
    }
    if behind {
        return PullWork::Start(update);
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
    let ci = Wait::Ci {
        pull_request: pr_number,
    };
    if pr.checks.is_empty() && now < pushed + policy.ci_grace {
        return PullWork::Wait(ci);
    }
    let waiting_since = restarted.map_or(pushed, |r| r.max(pushed));
    if !pending.is_empty() {
        return if now > waiting_since + policy.ci_timeout {
            PullWork::Act(Action::HandBack {
                item: Item::Pull(pr_number),
                kind: HandBackKind::Stall,
                reason: format!(
                    "CI did not complete within {}: {} still running.",
                    Phrase::default().span(policy.ci_timeout, None),
                    code_list(&pending)
                ),
            })
        } else {
            PullWork::Wait(ci)
        };
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
        return if attempts < policy.ci_attempts as usize {
            PullWork::Start(Work::FixCi {
                issue: ticket,
                pr: pr_number,
                failing: failed,
            })
        } else {
            PullWork::Act(Action::HandBack {
                item: Item::Pull(pr_number),
                kind: HandBackKind::Stall,
                reason: format!(
                    "{} still failing after {attempts} fix attempts.",
                    code_list(&failed)
                ),
            })
        };
    }
    let draft = pr.state == (PullState::Open { draft: true });
    let method = match policy.auto_merge {
        Some(method) if !is_held(pr, linewright) => method,
        _ => {
            return if draft {
                PullWork::Act(Action::MarkReady { pr: pr_number })
            } else {
                PullWork::Wait(Wait::Review {
                    pull_request: pr_number,
                })
            };
        }
    };
    let merge_state = Wait::MergeState {
        pull_request: pr_number,
    };
    // Behind and conflicting were brought up to date above, and so was not
    // rebaseable, once at each head.
    match &pr.mergeability {
        Mergeability::Clean if method == MergeMethod::Rebase => match pr.rebaseability {
            Rebaseability::Rebaseable => merge(pr, draft, method, memory, restarted),
            Rebaseability::NotRebaseable => PullWork::Act(Action::HandBack {
                item: Item::Pull(pr_number),
                kind: HandBackKind::Stall,
                reason: format!(
                    "GitHub reports {pr_number} not rebaseable though Linewright rebased it \
                     onto the default branch, so it cannot be merged by rebase."
                ),
            }),
            Rebaseability::Unknown => PullWork::Wait(merge_state),
        },
        Mergeability::Clean => merge(pr, draft, method, memory, restarted),
        Mergeability::Blocked { requires }
            if draft || !memory.held.is_held_for(pr_number, requires) =>
        {
            PullWork::Act(Action::HoldForReview {
                pr: pr_number,
                draft,
                requires: requires.clone(),
            })
        }
        Mergeability::Blocked { requires } => PullWork::Wait(Wait::Requirements {
            pull_request: pr_number,
            requires: requires.clone(),
        }),
        Mergeability::Unknown if draft => {
            PullWork::Act(Action::MarkReadyForMergeState { pr: pr_number })
        }
        Mergeability::Unknown => PullWork::Wait(merge_state),
        // Brought up to date above.
        Mergeability::Behind | Mergeability::Conflicting => {
            unreachable!("a pull request behind or conflicting is brought up to date first")
        }
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
) -> PullWork {
    match merge_failed_twice(pr, &memory.failed_merges, restarted) {
        Some(failed) => PullWork::Act(Action::HandBack {
            item: Item::Pull(pr.number),
            kind: HandBackKind::Stall,
            reason: format!(
                "Merging {} failed twice. GitHub's reason: {}",
                pr.number, failed.reason
            ),
        }),
        None => PullWork::Act(Action::Merge {
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
    let required = if requires.is_empty() {
        "GitHub blocks the merge for a reason Linewright cannot read: the default branch's \
         rules say what it needs"
            .to_string()
    } else {
        let names = Phrase::list(requires.iter().map(Requirement::describe), "and");
        format!("GitHub requires {names} before merging")
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

#[derive(Debug, Default, PartialEq, Eq)]
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
    spec_change(pr, &snapshot.default_branch, now, policy) == Ok(SpecChange::Pending)
}

/// What a merged pull request is as a spec change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecChange {
    /// Linewright cuts it.
    Pending,
    /// Linewright was unassigned from it, and `required_label`, when one is
    /// set, was not applied again since.
    Unassigned { required_label: Option<String> },
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
    RequiredLabel {
        label: String,
    },
}

/// What merged `pr` is as a spec change, or why it is none. Rules that hold
/// for good are checked first.
pub fn spec_change(
    pr: &MergedPull,
    default_branch: &str,
    now: Timestamp,
    policy: &Policy,
) -> Result<SpecChange, NotSpecChange> {
    let window_opened = policy.windows(now).lookback_opened();
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
        return Ok(SpecChange::Pending);
    } else if pr
        .unassigned_since_labeled(&policy.linewright, policy.require_label.as_deref())
        .is_some()
    {
        return Ok(SpecChange::Unassigned {
            required_label: policy.require_label.clone(),
        });
    } else if let Some(label) = policy.require_label.as_ref().filter(|l| !pr.has_label(l)) {
        NotSpecChange::RequiredLabel {
            label: label.clone(),
        }
    } else {
        return Ok(SpecChange::Pending);
    };
    Err(because)
}
