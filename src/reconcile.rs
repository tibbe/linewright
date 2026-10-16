//! The functional core: one poll's decisions as a pure function of the
//! store's snapshot, the workflows and their last runs, the runs in flight
//! and the clock (`docs/specs/architecture.md`, "Functional core, imperative
//! shell"). The rules are the product spec's: `docs/specs/product.md` and the
//! areas it lists.

use std::collections::BTreeMap;

use chrono::TimeDelta;
use serde::{Deserialize, Serialize};

use crate::domain::{
    BranchIssue, CheckState, Comment, Filed, Issue, IssueEvent, Item, LabelChange, Linked, Login,
    MergeMethod, MergeMethods, Mergeability, MergedPull, Number, Pull, PullState, Rebaseability,
    Requirement, Review, ReviewState, Snapshot, Thread, TicketBranchName, Timestamp, UpdateMethod,
    UpdateRule, label,
};
use crate::marker::{Stage, marked, stage_of};
use crate::runtime::UsageLimit;
use crate::spec_glob::SpecGlob;
use crate::workflow::{LastRuns, Workflow, WorkflowName};

/// The settings the decisions depend on.
#[derive(Clone, Debug)]
pub struct Policy {
    pub linewright: Login,
    pub spec_glob: SpecGlob,
    pub spec_lookback: TimeDelta,
    pub max_runs: usize,
    pub ci_grace: TimeDelta,
    pub ci_timeout: TimeDelta,
    pub ci_attempts: u32,
    /// Merge green pull requests with this method instead of requesting
    /// review.
    pub auto_merge: Option<MergeMethod>,
    pub update_rule: UpdateRule,
    /// The merge methods the repository allows.
    pub allowed_merge_methods: MergeMethods,
    /// Start work only on issues carrying this label
    /// (`docs/specs/repository.md`). Work already started carries on without
    /// it.
    pub require_label: Option<String>,
}

impl Policy {
    /// Whether Linewright triages `issue`: not one of its own, which it
    /// files already briefed.
    pub fn triages(&self, issue: &Issue) -> bool {
        issue.author != self.linewright
    }

    /// Whether Linewright may start work on `issue`: unless a label is
    /// required and the issue lacks it.
    fn may_start_on(&self, issue: &Issue) -> bool {
        self.require_label
            .as_ref()
            .is_none_or(|l| issue.has_label(l))
    }

    /// Whether a pull request GitHub reports not rebaseable is rebased onto
    /// the default branch, up to date or not: where the repository allows
    /// rebase merging and branches are not always updated by merging
    /// (`docs/specs/pull_requests.md`, "Keeping up to date"). A merge commit
    /// that resolved a conflict is the usual cause, and a rebase drops it.
    pub fn rebases_unrebaseable(&self) -> bool {
        self.allowed_merge_methods.contains(MergeMethod::Rebase)
            && self.update_rule != UpdateRule::Always(UpdateMethod::Merge)
    }
}

/// What a run works on. At most one run per subject is in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subject {
    Issue(Number),
    SpecChange(Number),
    /// Linewright's open pull request, whose runs are its own
    /// (`docs/specs/pull_requests.md`, "The pull request").
    Pull(Number),
}

/// A run to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Work {
    Triage {
        issue: Number,
    },
    Tickets {
        pr: Number,
    },
    /// A fresh run: no open pull request for the ticket.
    Implement {
        issue: Number,
    },
    /// Work on Linewright's open pull request `pr`, briefed with `issue`, the
    /// ticket it was opened for.
    Rework {
        issue: Number,
        pr: Number,
    },
    FixCi {
        issue: Number,
        pr: Number,
        failing: Vec<String>,
    },
    /// Bring the pull request's branch up to date, and nothing else.
    Update {
        issue: Number,
        pr: Number,
    },
}

impl Subject {
    /// The issue or pull request the subject is.
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
            Work::Tickets { pr } => Subject::SpecChange(*pr),
            Work::Rework { pr, .. } | Work::FixCi { pr, .. } | Work::Update { pr, .. } => {
                Subject::Pull(*pr)
            }
        }
    }

    /// The item the run is on, whose runs it counts among and which its run
    /// ID names: its pull request, or else its subject
    /// (`docs/specs/pull_requests.md`, "The pull request").
    pub fn item(&self) -> Number {
        match self {
            Work::Triage { issue } | Work::Implement { issue } => *issue,
            Work::Tickets { pr }
            | Work::Rework { pr, .. }
            | Work::FixCi { pr, .. }
            | Work::Update { pr, .. } => *pr,
        }
    }

    /// The item whose facts the run starts from: its pull request, or else
    /// its subject.
    pub fn starts_from(&self) -> Item {
        match self {
            Work::Triage { issue } | Work::Implement { issue } => Item::Issue(*issue),
            Work::Tickets { pr }
            | Work::Rework { pr, .. }
            | Work::FixCi { pr, .. }
            | Work::Update { pr, .. } => Item::Pull(*pr),
        }
    }

    /// The pull request for its issue it is given to work on, if any. A
    /// spec change's pull request is its subject instead.
    pub fn pr(&self) -> Option<Number> {
        match self {
            Work::Rework { pr, .. } | Work::FixCi { pr, .. } | Work::Update { pr, .. } => Some(*pr),
            Work::Triage { .. } | Work::Tickets { .. } | Work::Implement { .. } => None,
        }
    }

    pub fn kind(&self) -> WorkKind {
        match self {
            Work::Triage { .. } => WorkKind::Triage,
            Work::Tickets { .. } => WorkKind::Tickets,
            Work::Implement { .. } => WorkKind::Implement,
            Work::Rework { .. } => WorkKind::Rework,
            Work::FixCi { .. } => WorkKind::FixCi,
            Work::Update { .. } => WorkKind::Update,
        }
    }
}

/// The stage a run does, named in its run ID and in JSON by
/// [`WorkKind::name`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "&'static str", try_from = "String")]
pub enum WorkKind {
    Triage,
    Tickets,
    Implement,
    Rework,
    FixCi,
    Update,
}

impl WorkKind {
    pub const ALL: [WorkKind; 6] = [
        WorkKind::Triage,
        WorkKind::Tickets,
        WorkKind::Implement,
        WorkKind::Rework,
        WorkKind::FixCi,
        WorkKind::Update,
    ];

    /// The stage's name in run IDs (`docs/specs/monitoring.md`, "runs").
    pub fn name(self) -> &'static str {
        match self {
            WorkKind::Triage => "triage",
            WorkKind::Tickets => "tickets",
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

impl TryFrom<String> for WorkKind {
    type Error = String;

    fn try_from(name: String) -> Result<WorkKind, String> {
        name.parse()
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

/// A run in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveRun {
    pub subject: Subject,
    pub kind: WorkKind,
}

/// What the manager remembers between polls, and never in the store.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    /// Oldest first.
    pub failed_merges: Vec<FailedMerge>,
    pub tried_rebases: Vec<TriedRebase>,
    /// The usage limit the agent runtime refused a session for: until it
    /// resets no session starts (`docs/specs/operation.md`, "Usage limit").
    pub usage_limit: Option<UsageLimit>,
}

impl Memory {
    /// Forgets what it remembers of pull requests the snapshot does not
    /// show open.
    pub fn retain_open(&mut self, snapshot: &Snapshot) {
        let open = |pr| snapshot.pull(pr).is_some_and(|p| p.is_open());
        self.failed_merges.retain(|f| open(f.pr));
        self.tried_rebases.retain(|r| open(r.pr));
    }

    /// Whether the usage limit holds starts back at `now`.
    pub fn holds_starts(&self, now: Timestamp) -> bool {
        self.usage_limit.is_some_and(|limit| now < limit.resets_at)
    }

    /// Holds starts back until `resets_at`, unless a later reset already
    /// holds them: the latest wins. Returns whether the hold changed.
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

    /// Lifts the hold on starts if its reset has passed at `now`. Returns
    /// the reset it lifted.
    pub fn lift_usage_limit(&mut self, now: Timestamp) -> Option<Timestamp> {
        if self.holds_starts(now) {
            return None;
        }
        self.usage_limit.take().map(|limit| limit.resets_at)
    }
}

/// Which issue each of Linewright's pull requests was opened for, as the
/// runs that opened them record it under the work directory: the one fact of
/// machine state Linewright keeps rather than reads from the store
/// (`docs/specs/repository.md`, "Labels and state").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenedFor(BTreeMap<Number, Number>);

impl OpenedFor {
    /// The record of each pull request `pr` opened for the issue `issue`, as
    /// `(pr, issue)`.
    pub fn new(opened: impl IntoIterator<Item = (Number, Number)>) -> OpenedFor {
        OpenedFor(opened.into_iter().collect())
    }

    /// The issue the pull request #`pr` was opened for: `None` when no record
    /// links to it, as for one opened before runs kept their pull request.
    pub fn issue(&self, pr: Number) -> Option<Number> {
        self.0.get(&pr).copied()
    }

    /// Whether Linewright's pull request `pull` is for the issue #`issue`:
    /// the one it was opened for or, when no record links to it, any it
    /// closes.
    pub fn is_for(&self, pull: &Pull, issue: Number) -> bool {
        self.issue(pull.number).map_or_else(
            || pull.closes.contains(&issue),
            |opened_for| opened_for == issue,
        )
    }
}

/// What the decisions are taken on: the store's snapshot, with which issue
/// each of Linewright's pull requests in it was opened for.
#[derive(Clone, Copy)]
pub struct Facts<'a> {
    pub snapshot: &'a Snapshot,
    pub opened_for: &'a OpenedFor,
}

impl<'a> Facts<'a> {
    /// Linewright's pull requests for the issue #`issue`, open or ended.
    fn pulls_for(
        self,
        issue: Number,
        linewright: &'a Login,
    ) -> impl Iterator<Item = &'a Pull> + Clone {
        let opened_for = self.opened_for;
        self.snapshot
            .pulls
            .iter()
            .filter(move |p| &p.author == linewright && opened_for.is_for(p, issue))
    }

    /// The pull request Linewright works on for the issue #`issue`, which
    /// is in review while it is open: its newest open one.
    pub fn open_pull_for(self, issue: Number, linewright: &'a Login) -> Option<&'a Pull> {
        self.pulls_for(issue, linewright)
            .filter(|p| p.is_open())
            .max_by_key(|p| p.number)
    }

    /// The ticket Linewright's pull request `pull` is worked for, whose brief
    /// its runs are given: the issue it was opened for or, when no record
    /// links to it, the first it closes that the snapshot holds, else the
    /// first it closes. `None` when it is for no issue.
    pub fn ticket_of(self, pull: &Pull) -> Option<Number> {
        self.opened_for.issue(pull.number).or_else(|| {
            let held = pull
                .closes
                .iter()
                .find(|&&n| self.snapshot.issue(n).is_some());
            held.or(pull.closes.first()).copied()
        })
    }

    /// The open pull requests Linewright works on, each with its ticket.
    pub fn worked_pulls(
        self,
        linewright: &'a Login,
    ) -> impl Iterator<Item = (&'a Pull, Number)> + 'a {
        self.snapshot
            .pulls
            .iter()
            .filter(|p| p.is_open())
            .filter_map(move |p| Some((p, self.worked_ticket(p, linewright).ok()?)))
    }

    /// The ticket the open pull request `pull` is worked for, when
    /// Linewright works on it: it is Linewright's, for an issue, and the
    /// newest open one for it. `Err` with why Linewright does not.
    pub fn worked_ticket(self, pull: &Pull, linewright: &'a Login) -> Result<Number, NotWorked> {
        if &pull.author != linewright {
            return Err(NotWorked::NotLinewrights);
        }
        let ticket = self.ticket_of(pull).ok_or(NotWorked::ForNoIssue)?;
        match self.superseding(pull, ticket, linewright) {
            Some(newer) => Err(NotWorked::Superseded {
                ticket,
                by: newer.number,
            }),
            None => Ok(ticket),
        }
    }

    /// Linewright's newer open pull request for #`ticket`, which it works on
    /// instead of `pull`, if there is one.
    pub fn superseding(
        self,
        pull: &Pull,
        ticket: Number,
        linewright: &'a Login,
    ) -> Option<&'a Pull> {
        self.open_pull_for(ticket, linewright)
            .filter(|newest| newest.number != pull.number)
    }
}

/// Why Linewright does not work on an open pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotWorked {
    /// Someone else opened it.
    NotLinewrights,
    /// It is Linewright's, and closes no issue.
    ForNoIssue,
    /// It is Linewright's for `ticket`, which Linewright works on through its
    /// newer pull request `by`.
    Superseded { ticket: Number, by: Number },
}

/// A merge of `pr` the store refused at `at`, for `reason`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedMerge {
    pub pr: Number,
    pub reason: String,
    pub at: Timestamp,
}

/// An update run started on `pr` at `head`, at `at`, to rebase it while
/// GitHub reported it not rebaseable. Another waits for a new head, or for
/// `ready-for-human` to be removed from the pull request since, so that a
/// rebase that leaves the branch as it was is not tried again at every poll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriedRebase {
    pub pr: Number,
    pub head: String,
    pub at: Timestamp,
}

/// The workflows on the default branch and their runs: what the due rule
/// reads.
#[derive(Clone, Debug, Default)]
pub struct Workflows {
    /// The workflows whose files parse.
    pub defined: Vec<Workflow>,
    pub last_runs: LastRuns,
    /// The workflows with a run in flight.
    pub in_flight: Vec<WorkflowName>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Start(Work),
    Cancel {
        subject: Subject,
        reason: String,
    },
    /// Add Linewright as an assignee.
    Claim {
        issue: Number,
    },
    Relabel {
        issue: Number,
        remove: String,
        add: String,
    },
    /// Give the item to the maintainer: `ready-for-human` and a comment that
    /// says why. An issue is unassigned too; Linewright's pull request's
    /// ticket is left as it is.
    HandBack {
        item: Item,
        stage: Stage,
        reason: String,
    },
    MarkReady {
        pr: Number,
    },
    /// Remember the rebase an update run starting now tries.
    RememberRebase(TriedRebase),
    /// Merge the pull request with `method`, if its head is still
    /// `head`. A draft is marked ready for review first: GitHub merges no
    /// draft.
    Merge {
        pr: Number,
        head: String,
        draft: bool,
        method: MergeMethod,
    },
    /// Mark the draft ready for review, so that the store reports its merge
    /// state: a later poll merges it, or holds it for review.
    MarkReadyForMergeState {
        pr: Number,
    },
    /// Hand a pull request the default branch's rules block to people: mark
    /// it ready for review if a draft and comment [`hold_note`].
    HoldForReview {
        pr: Number,
        draft: bool,
        requires: Vec<Requirement>,
    },
    CloseTracking {
        issue: Number,
    },
    /// Close an interrupted decomposition's tracking issue and its open
    /// tickets, so the spec change is cut into tickets again.
    DiscardTickets {
        tracking: Number,
        tickets: Vec<Number>,
    },
    /// Delete the branch a run pushed for a ticket that is closed with no
    /// pull request open for it (`docs/specs/implementation.md`, "Working a
    /// ticket").
    DeleteBranch {
        branch: TicketBranchName,
    },
    /// Keep the workflows' last runs, which changed, under the work
    /// directory.
    RecordLastRuns(LastRuns),
    /// Run the workflow, as `lw workflow run` does.
    RunWorkflow(Box<Workflow>),
}

/// Something Linewright waits for before it acts on an item next
/// (`docs/specs/monitoring.md`, "explain").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "wait", rename_all = "kebab-case")]
pub enum Wait {
    /// Its run in flight, of `stage`.
    Run { stage: WorkKind },
    /// A free run: `in_flight` of the `max_runs` that may run at once are,
    /// counting those the poll starts.
    FreeRun { in_flight: usize, max_runs: usize },
    /// The usage limit, in force until `resets_at`.
    UsageLimit { resets_at: Timestamp },
    /// An issue that blocks it, still open.
    Blocker { issue: Number },
    /// An open pull request not opened for it that closes it, whoever opened
    /// it.
    ClosingPullRequest { pull_request: Number },
    /// Its open pull request, which Linewright works on, until it is merged
    /// or closed.
    PullRequest { pull_request: Number },
    /// CI on its pull request: checks still running, or none reported yet.
    Ci { pull_request: Number },
    /// GitHub, to work out whether its pull request can merge.
    MergeState { pull_request: Number },
    /// Its tickets still open, as its tracking issue.
    Tickets { open: Vec<Number> },
    /// The reporter, as `needs-info` says.
    Reporter,
    /// The maintainer, as `ready-for-human` says: to answer with a label.
    Maintainer,
    /// The maintainer, as `ready-for-human` on Linewright's pull request
    /// says: until they remove it.
    HandedBack,
    /// The maintainer, to apply the required label `label`.
    RequiredLabel { label: String },
    /// The maintainer, to label it `ready-for-agent` again.
    Relabel { since: Relabel },
    /// The maintainer, to review its pull request.
    Review { pull_request: Number },
    /// What the default branch's rules require before its pull request
    /// merges.
    Requirements {
        pull_request: Number,
        requires: Vec<Requirement>,
    },
}

/// Why only labeling a ticket `ready-for-agent` again asks for more work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Relabel {
    /// A person unassigned Linewright.
    Unassigned,
    /// Its pull request merged.
    Merged { pull_request: Number },
}

/// One poll's decisions: the actions to take, and for each item it does not
/// act on yet, what it waits for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decisions {
    pub actions: Vec<Action>,
    pub waits: BTreeMap<Number, Vec<Wait>>,
}

pub fn reconcile(
    facts: Facts,
    live: &[LiveRun],
    memory: &Memory,
    workflows: &Workflows,
    now: Timestamp,
    policy: &Policy,
) -> Vec<Action> {
    decisions(facts, live, memory, workflows, now, policy).actions
}

/// The decisions [`reconcile`] takes, with what each item it does not act on
/// waits for.
pub fn decisions(
    facts: Facts,
    live: &[LiveRun],
    memory: &Memory,
    workflows: &Workflows,
    now: Timestamp,
    policy: &Policy,
) -> Decisions {
    let snapshot = facts.snapshot;
    let mut plan = Plan {
        now,
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

    let spec_changes = pending_spec_changes(snapshot, now, policy);
    let pending: Vec<Number> = spec_changes.iter().map(|pr| pr.number).collect();
    for pr in spec_changes {
        if let Some(stage) = running(Subject::SpecChange(pr.number)) {
            plan.wait(pr.number, Wait::Run { stage });
            continue;
        }
        // The pull request is labeled processed last, so a tracking issue
        // without the label is a decomposition that stopped part way.
        let partial = snapshot.issues.iter().find(|i| {
            i.open
                && i.filed
                    == Some(Filed::Tracking {
                        spec_change: pr.number,
                    })
        });
        match partial {
            Some(partial) => plan.act(Action::DiscardTickets {
                tracking: partial.number,
                tickets: partial
                    .sub_issues
                    .iter()
                    .filter(|t| t.open)
                    .map(|t| t.number)
                    .collect(),
            }),
            None => plan.start(Work::Tickets { pr: pr.number }),
        }
    }

    // Linewright's pull request is worked because it is open, whatever its
    // ticket's label and assignees (`docs/specs/pull_requests.md`, "The pull
    // request").
    for (pr, ticket) in facts.worked_pulls(&policy.linewright) {
        if pr.has_label(label::READY_FOR_HUMAN) {
            plan.wait(pr.number, Wait::HandedBack);
        } else if let Some(stage) = running(Subject::Pull(pr.number)) {
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
        // While its spec change is cut into tickets, a tracking issue waits
        // for the cut; one that stopped part way is discarded above.
        let cut = tracks.and_then(|spec_change| running(Subject::SpecChange(spec_change)));
        if let Some(stage) = cut {
            plan.wait(issue.number, Wait::Run { stage });
        }
        let cutting = cut.is_some() || tracks.is_some_and(|sc| pending.contains(&sc));
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
        let replied = has_reply_to_needs_info(issue, policy);
        if replied {
            plan.act(Action::Relabel {
                issue: issue.number,
                remove: label::NEEDS_INFO.into(),
                add: label::NEEDS_TRIAGE.into(),
            });
        }
        if let Some(pr) = facts.open_pull_for(issue.number, &policy.linewright) {
            // The work is the pull request's while it is open, whatever the
            // ticket's labels.
            if let Some(stage) = running(subject) {
                plan.wait(issue.number, Wait::Run { stage });
            }
            plan.wait(
                issue.number,
                Wait::PullRequest {
                    pull_request: pr.number,
                },
            );
        } else if replied || wants_triage(issue, policy) {
            match running(subject) {
                Some(stage) => plan.wait(issue.number, Wait::Run { stage }),
                None => plan.start(Work::Triage {
                    issue: issue.number,
                }),
            }
        } else if issue.has_label(label::READY_FOR_AGENT) && !issue.has_label(label::NEEDS_TRIAGE) {
            implementation(issue, facts, running(subject), policy, &mut plan);
        } else {
            for wait in labeled_waits(issue, policy) {
                plan.wait(issue.number, wait);
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

    // Workflow runs count against the ceiling beside the pipeline's. Under
    // the usage limit there is no room: a due workflow stays due.
    let in_flight = live.len() + workflows.in_flight.len();
    let (room, held) = match memory.usage_limit.filter(|_| memory.holds_starts(now)) {
        Some(UsageLimit { resets_at }) => (0, Held::UsageLimit { resets_at }),
        None => (
            policy.max_runs.saturating_sub(in_flight),
            Held::Ceiling {
                in_flight,
                max_runs: policy.max_runs,
            },
        ),
    };
    let (mut decisions, room) = plan.into_decisions(room, held);
    decisions.actions.extend(schedule(workflows, now, room));
    decisions
}

/// When a workflow runs next, by the due rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextRun {
    /// A tick of its schedule has passed since its last run.
    Due,
    /// Its next tick, after its last run.
    At(Timestamp),
    /// Its schedule ticks no more.
    Never,
}

/// When `workflow`, last run at `last`, runs next as of `now`. A workflow is
/// due when a tick of its schedule has passed since its last run, so ticks
/// missed meanwhile collapse into one run.
pub fn next_run(workflow: &Workflow, last: Timestamp, now: Timestamp) -> NextRun {
    match workflow.schedule.next_after(last) {
        Some(tick) if tick <= now => NextRun::Due,
        Some(tick) => NextRun::At(tick),
        None => NextRun::Never,
    }
}

/// The workflows to run in the `room` the pipeline's runs left, and their
/// last runs recorded. A workflow runs when it is due and has no run in
/// flight. A workflow seen for the first time counts as run now, so adding
/// one does not start it at once.
fn schedule(workflows: &Workflows, now: Timestamp, room: usize) -> Vec<Action> {
    let mut last_runs = workflows.last_runs.clone();
    let mut due = vec![];
    for workflow in &workflows.defined {
        let Some(last) = workflows.last_runs.get(&workflow.name) else {
            last_runs.record(&workflow.name, now);
            continue;
        };
        let running = workflows.in_flight.contains(&workflow.name);
        if next_run(workflow, last, now) == NextRun::Due && !running {
            due.push(workflow);
        }
    }
    due.sort_by(|a, b| a.name.cmp(&b.name));
    let runs: Vec<Action> = due
        .into_iter()
        .take(room)
        .map(|workflow| {
            last_runs.record(&workflow.name, now);
            Action::RunWorkflow(Box::new(workflow.clone()))
        })
        .collect();
    // The record follows the runs: a stop between the two leaves the
    // workflow due, to run again after the restart, rather than recorded as
    // run with no run started.
    let mut actions = runs;
    if last_runs != workflows.last_runs {
        actions.push(Action::RecordLastRuns(last_runs));
    }
    actions
}

/// Why a run in flight must stop, if it must: its subject no longer asks for
/// the work. Runs stop when their issue loses its label, is closed or, for
/// implementation, loses Linewright as an assignee; and when their pull
/// request is merged, closed or labeled `ready-for-human`.
fn cancel_reason(run: &LiveRun, snapshot: &Snapshot, policy: &Policy) -> Option<String> {
    match run.subject {
        Subject::Pull(number) => {
            let Some(pr) = snapshot.pull(number) else {
                return Some(format!("{number} is no longer open"));
            };
            match pr.state {
                PullState::Merged { .. } => Some(format!("{number} was merged")),
                PullState::Closed { .. } => Some(format!("{number} was closed")),
                PullState::Open { .. } if pr.has_label(label::READY_FOR_HUMAN) => {
                    Some(format!("{number} was labeled `{}`", label::READY_FOR_HUMAN))
                }
                PullState::Open { .. } => None,
            }
        }
        Subject::SpecChange(pr) => {
            let pr = snapshot.merged_pull(pr)?;
            [label::SPEC_PROCESSED, label::READY_FOR_HUMAN]
                .into_iter()
                .find(|l| pr.has_label(l))
                .map(|l| format!("{} was labeled `{l}`", pr.number))
        }
        Subject::Issue(number) => {
            let Some(issue) = snapshot.issue(number).filter(|i| i.open) else {
                return Some(format!("{number} was closed"));
            };
            let wanted = match run.kind {
                WorkKind::Triage => label::NEEDS_TRIAGE,
                _ => label::READY_FOR_AGENT,
            };
            if !issue.has_label(wanted) {
                Some(format!("`{wanted}` was removed from {number}"))
            } else if run.kind != WorkKind::Triage && !issue.is_assigned(&policy.linewright) {
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

/// The actions of one poll, with the runs to start held back until the
/// ceiling on simultaneous runs is applied.
struct Plan {
    now: Timestamp,
    actions: Vec<Action>,
    starts: Vec<Start>,
    waits: BTreeMap<Number, Vec<Wait>>,
}

/// Why runs beyond the room are held back.
enum Held {
    UsageLimit {
        resets_at: Timestamp,
    },
    /// `in_flight` of `max_runs` were in flight before the poll.
    Ceiling {
        in_flight: usize,
        max_runs: usize,
    },
}

struct Start {
    /// The issue to claim before the run starts.
    claim: Option<Number>,
    /// The rebase the run tries, to remember as it starts.
    rebase: Option<TriedRebase>,
    work: Work,
}

impl Plan {
    fn act(&mut self, action: Action) {
        self.actions.push(action);
    }

    fn wait(&mut self, item: Number, wait: Wait) {
        self.waits.entry(item).or_default().push(wait);
    }

    fn wait_for(&mut self, item: Number, wait: Option<Wait>) {
        if let Some(wait) = wait {
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
        self.starts.push(Start {
            claim: Some(issue),
            rebase: None,
            work: Work::Implement { issue },
        });
    }

    fn start_rebase(&mut self, work: Work, rebase: TriedRebase) {
        self.starts.push(Start {
            claim: None,
            rebase: Some(rebase),
            work,
        });
    }

    /// Every action, then as many runs as there is room for, work closest to
    /// a person first: pull requests, then triage, then tickets, then fresh
    /// implementations, each oldest first, the rest waiting as `held` says.
    /// Also returns the room left.
    fn into_decisions(mut self, room: usize, held: Held) -> (Decisions, usize) {
        let mut starts = std::mem::take(&mut self.starts);
        starts.sort_by_key(|s| (rank(s.work.kind()), s.work.subject()));
        let room_left = room.saturating_sub(starts.len());
        let started = starts.len().min(room);
        let wait = match held {
            Held::UsageLimit { resets_at } => Wait::UsageLimit { resets_at },
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
            if let Some(issue) = start.claim {
                actions.push(Action::Claim { issue });
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
        WorkKind::Tickets => 2,
        WorkKind::Implement => 3,
    }
}

/// What an issue Linewright starts nothing on waits for, as its labels
/// say: the person a label hands it to, and the required label when it
/// lacks it, without which an answer starts nothing either. An issue of
/// Linewright's own labeled `needs-triage` waits for nothing: it is never
/// triaged.
fn labeled_waits(issue: &Issue, policy: &Policy) -> Vec<Wait> {
    let handed_to = if issue.has_label(label::NEEDS_TRIAGE) {
        if !policy.triages(issue) {
            return vec![];
        }
        None
    } else if issue.has_label(label::NEEDS_INFO) {
        Some(Wait::Reporter)
    } else if issue.has_label(label::READY_FOR_HUMAN) {
        Some(Wait::Maintainer)
    } else {
        return vec![];
    };
    let unlabeled = required_label(policy).filter(|_| !policy.may_start_on(issue));
    handed_to.into_iter().chain(unlabeled).collect()
}

/// The numbers of the linked issues still open.
fn open_numbers(linked: &[Linked]) -> Vec<Number> {
    linked.iter().filter(|l| l.open).map(|l| l.number).collect()
}

/// The wait for the maintainer to apply the required label, if one is set.
fn required_label(policy: &Policy) -> Option<Wait> {
    policy
        .require_label
        .as_ref()
        .map(|label| Wait::RequiredLabel {
            label: label.clone(),
        })
}

/// An issue labeled `ready-for-agent` with no open pull request: claim it
/// when it is unblocked, and implement it.
fn implementation(
    issue: &Issue,
    facts: Facts,
    live: Option<WorkKind>,
    policy: &Policy,
    plan: &mut Plan,
) {
    let claimed = issue.is_assigned(&policy.linewright);
    if let Some(stage) = live {
        return plan.wait(issue.number, Wait::Run { stage });
    }
    if !claimed && stopped_by_unassign(issue, policy) {
        return plan.wait(
            issue.number,
            Wait::Relabel {
                since: Relabel::Unassigned,
            },
        );
    }
    if !claimed && !policy.may_start_on(issue) {
        return plan.wait_for(issue.number, required_label(policy));
    }
    // Its open pull request, if any, is waited for above.
    let own = facts.pulls_for(issue.number, &policy.linewright);
    // GitHub closes the issue shortly after the merge; until then, or after a
    // reopen, only applying the label again asks for more work.
    let relabeled_since = |at| last_labeled(issue, label::READY_FOR_AGENT).is_some_and(|l| l > at);
    if let Some(merged) = own
        .clone()
        .find(|p| matches!(p.state, PullState::Merged { at } if !relabeled_since(at)))
    {
        return plan.wait(
            issue.number,
            Wait::Relabel {
                since: Relabel::Merged {
                    pull_request: merged.number,
                },
            },
        );
    }
    // Still claimed after a merge, the issue was reopened, and more work on
    // it is a start.
    let reopened = own
        .clone()
        .any(|p| matches!(p.state, PullState::Merged { .. }));
    if reopened && !policy.may_start_on(issue) {
        return plan.wait_for(issue.number, required_label(policy));
    }
    if claimed {
        let closed = own
            .clone()
            .find(|p| matches!(p.state, PullState::Closed { at } if !linewright_commented_since(issue, at, policy)));
        if let Some(closed) = closed {
            return plan.act(Action::HandBack {
                item: Item::Issue(issue.number),
                stage: Stage::Review,
                reason: format!("{} was closed without merging.", closed.number),
            });
        }
    }
    // A pull request not opened for the ticket that closes it, whoever
    // opened it, would do its work twice (`docs/specs/implementation.md`,
    // "Tickets"). Linewright's own for it are none of them: none is open.
    let closing = issue
        .closed_by
        .iter()
        .filter(|p| p.open && !own.clone().any(|pull| pull.number == p.number))
        .map(|p| Wait::ClosingPullRequest {
            pull_request: p.number,
        });
    let blockers = open_numbers(&issue.blocked_by)
        .into_iter()
        .map(|blocker| Wait::Blocker { issue: blocker });
    let waits: Vec<Wait> = closing.chain(blockers).collect();
    if !waits.is_empty() {
        for wait in waits {
            plan.wait(issue.number, wait);
        }
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

fn linewright_commented_since(issue: &Issue, since: Timestamp, policy: &Policy) -> bool {
    issue
        .comments
        .iter()
        .any(|c| c.created_at > since && stage_of(c, &policy.linewright).is_some())
}

/// Linewright's open pull request for the ticket #`ticket`: address
/// feedback, keep it up to date and get CI green, in that order, then mark it
/// ready for review or, with auto-merge, merge it once no person holds it.
/// What it waits for is its own, and what cannot finish hands it back.
fn in_review(ticket: Number, pr: &Pull, memory: &Memory, policy: &Policy, plan: &mut Plan) {
    let pr_number = pr.number;
    let linewright = &policy.linewright;
    let restarted = restarted_at(pr);
    if !new_feedback(pr, linewright).is_empty() {
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
    // A pull request given back to the maintainer waits a full timeout again
    // once they remove `ready-for-human`.
    let waiting_since = restarted.map_or(pushed, |r| r.max(pushed));
    if !pending.is_empty() {
        if now > waiting_since + policy.ci_timeout {
            plan.act(Action::HandBack {
                item: Item::Pull(pr_number),
                stage: Stage::Ci,
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
            last_marked(&pr.comments, Stage::Rework, linewright),
            restarted,
        ]
        .into_iter()
        .flatten()
        .max();
        let attempts = pr
            .comments
            .iter()
            .filter(|c| {
                stage_of(c, linewright) == Some(Stage::Ci) && Some(c.created_at) > cycle_start
            })
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
                stage: Stage::Ci,
                reason: format!(
                    "{} still failing after {attempts} fix attempts.",
                    code_list(&failed)
                ),
            });
        }
        return;
    }
    let draft = pr.state == (PullState::Open { draft: true });
    // A pull request a person holds is marked ready as it is without
    // auto-merge, so the people holding it can approve it.
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
                stage: Stage::Review,
                reason: format!(
                    "GitHub reports {pr_number} not rebaseable though Linewright rebased it \
                     onto the default branch, so it cannot be merged by rebase."
                ),
            }),
            // Not computed yet: a later poll merges it.
            Rebaseability::Unknown => plan.wait(pr_number, merge_state),
        },
        Mergeability::Clean => merge(pr, draft, method, memory, restarted, plan),
        // Held for review again only when what GitHub requires changes.
        Mergeability::Blocked { requires } if draft || !hold_noted(pr, requires, linewright) => {
            plan.act(Action::HoldForReview {
                pr: pr_number,
                draft,
                requires: requires.clone(),
            })
        }
        Mergeability::Blocked { requires } => plan.wait(
            pr_number,
            Wait::Requirements {
                pull_request: pr_number,
                requires: requires.clone(),
            },
        ),
        Mergeability::Unknown if draft => {
            plan.act(Action::MarkReadyForMergeState { pr: pr_number })
        }
        // A merge state not computed yet waits for the next poll.
        Mergeability::Unknown => plan.wait(pr_number, merge_state),
        // Brought up to date above.
        Mergeability::Behind | Mergeability::Conflicting => {}
    }
}

/// When the maintainer last removed `ready-for-human` from the pull request:
/// its work starts over from then, with a fresh CI timeout, fresh fix
/// attempts, no failed merges and no rebase tried
/// (`docs/specs/pull_requests.md`, "The pull request").
fn restarted_at(pr: &Pull) -> Option<Timestamp> {
    pr.label_events
        .iter()
        .filter(|e| e.label == label::READY_FOR_HUMAN && e.change == LabelChange::Removed)
        .map(|e| e.at)
        .max()
}

/// Whether an update run already tried to rebase the pull request at its
/// head since its work last `restarted`.
fn tried_rebase(pr: &Pull, memory: &Memory, restarted: Option<Timestamp>) -> bool {
    memory
        .tried_rebases
        .iter()
        .any(|r| r.pr == pr.number && r.head == pr.head_sha && restarted.is_none_or(|at| at < r.at))
}

/// Whether a run on the pull request rebases its branch onto the default
/// branch, up to date or not, instead of updating it by the update rule:
/// GitHub reports it not rebaseable, and the policy rebases such pull
/// requests.
pub fn rebases_to_repair(pr: &Pull, policy: &Policy) -> bool {
    pr.rebaseability == Rebaseability::NotRebaseable && policy.rebases_unrebaseable()
}

/// Merges the pull request with `method`, unless its merge has failed twice
/// since its work last `restarted`: then it is handed back.
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
                stage: Stage::Review,
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

/// The pull request's last failed merge, if its merge has failed twice since
/// its work last `restarted`: a failure is retried once.
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

/// The comment on a pull request held for review: what GitHub requires
/// before it merges.
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

/// Whether Linewright's latest review comment on the pull request already
/// names `requires`.
fn hold_noted(pr: &Pull, requires: &[Requirement], linewright: &Login) -> bool {
    let note = marked(Stage::Review, &hold_note(requires));
    pr.comments
        .iter()
        .filter(|c| stage_of(c, linewright) == Some(Stage::Review))
        .max_by_key(|c| c.created_at)
        .is_some_and(|c| c.body.trim() == note)
}

/// Whether a person who commented on or reviewed the pull request holds its
/// merge: they have no approving review newer than every comment they made
/// on it, as a PR comment, in a review thread or as another review. The
/// inline comments submitted with an approval are made no later than it, so
/// they don't hold the merge. Bots and Linewright hold nothing.
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

/// Feedback on a pull request that Linewright has not answered yet.
#[derive(Debug, Default)]
pub struct Feedback<'a> {
    pub comments: Vec<&'a Comment>,
    pub reviews: Vec<&'a Review>,
    /// Threads with new comments, and those comments.
    pub threads: Vec<(&'a Thread, Vec<&'a Comment>)>,
}

impl Feedback<'_> {
    pub fn is_empty(&self) -> bool {
        self.comments.is_empty() && self.reviews.is_empty() && self.threads.is_empty()
    }
}

/// Comments and reviews from anyone other than Linewright, posted after
/// Linewright last answered: its last push, or its last rework comment when a
/// rework needed no change. A rework comment from before the pull request's
/// work last restarted, such as one handing it back, answers nothing: the
/// work tries again. A review that only approves is not feedback.
pub fn new_feedback<'a>(pr: &'a Pull, linewright: &Login) -> Feedback<'a> {
    let restarted = restarted_at(pr);
    let pushed = pr
        .pushes
        .iter()
        .filter(|p| &p.by == linewright)
        .map(|p| p.at);
    let created = (&pr.author == linewright).then_some(pr.created_at);
    let reworked = last_marked(&pr.comments, Stage::Rework, linewright)
        .filter(|&at| restarted.is_none_or(|r| at > r));
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

fn last_marked(
    comments: &[crate::domain::Comment],
    stage: Stage,
    linewright: &Login,
) -> Option<Timestamp> {
    comments
        .iter()
        .filter(|c| stage_of(c, linewright) == Some(stage))
        .map(|c| c.created_at)
        .max()
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

/// A person unassigning Linewright stops the issue's work until
/// `ready-for-agent` is applied again.
fn stopped_by_unassign(issue: &Issue, policy: &Policy) -> bool {
    let linewright = &policy.linewright;
    let unassigned = issue
        .events
        .iter()
        .filter_map(|e| match e {
            IssueEvent::Unassigned {
                assignee,
                actor,
                at,
            } if assignee == linewright && actor != linewright => Some(*at),
            _ => None,
        })
        .max();
    let labeled = last_labeled(issue, label::READY_FOR_AGENT);
    match (unassigned, labeled) {
        (Some(unassigned), Some(labeled)) => unassigned > labeled,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

fn last_labeled(issue: &Issue, name: &str) -> Option<Timestamp> {
    issue
        .events
        .iter()
        .filter_map(|e| match e {
            IssueEvent::Labeled { label, at } if label == name => Some(*at),
            _ => None,
        })
        .max()
}

/// Triage runs on `needs-triage` issues not authored by Linewright:
/// tickets Linewright creates are already briefed.
fn wants_triage(issue: &Issue, policy: &Policy) -> bool {
    issue.has_label(label::NEEDS_TRIAGE) && policy.triages(issue) && policy.may_start_on(issue)
}

/// Whether a `needs-info` issue has a reply (`docs/specs/triage.md`,
/// "Answers"). Its question was asked at the later of the label and
/// Linewright's last comment; without a comment of Linewright's there is no
/// question to reply to.
fn has_reply_to_needs_info(issue: &Issue, policy: &Policy) -> bool {
    let linewright = &policy.linewright;
    if !issue.has_label(label::NEEDS_INFO)
        || !policy.triages(issue)
        || !policy.may_start_on(issue)
        || [label::READY_FOR_AGENT, label::NEEDS_TRIAGE]
            .into_iter()
            .any(|l| issue.has_label(l))
    {
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
    let asked = last_labeled(issue, label::NEEDS_INFO).map_or(last, |labeled| labeled.max(last));
    issue
        .comments
        .iter()
        .any(|c| &c.author != linewright && c.created_at > asked)
}

/// The spec changes waiting to be cut into tickets.
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

/// A spec change: merged into the default branch within the lookback window,
/// touching the spec, not authored by Linewright, neither processed nor
/// handed back and, with a label required, carrying it. The label is checked
/// on every poll, so one applied after the merge counts; `cancel_reason` does
/// not look at it, so a cut under way survives its removal.
fn is_spec_change(pr: &MergedPull, snapshot: &Snapshot, now: Timestamp, policy: &Policy) -> bool {
    not_a_spec_change(pr, &snapshot.default_branch, now, policy).is_none()
}

/// Why a merged pull request is no spec change (`docs/specs/sources_of_work.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum NotSpecChange {
    /// Linewright opened it.
    Linewrights,
    /// It was merged into `base`, not the default branch.
    OtherBranch { base: String },
    /// It touched no file matching `spec_glob`.
    NoSpecFile { spec_glob: String },
    /// It is labeled `linewright:spec-processed`.
    Processed,
    /// It is labeled `ready-for-human`.
    HandedBack,
    /// It was merged at `merged_at`, before the lookback window opened at
    /// `window_opened`.
    BeforeLookback {
        merged_at: Timestamp,
        window_opened: Timestamp,
    },
    /// It lacks the required label `label`.
    RequiredLabel { label: String },
}

/// Why `pr`, merged, is no spec change at `now` where the default branch is
/// `default_branch`: the first rule of a spec change it fails, those
/// that hold for good first. `None` for a spec change.
pub fn not_a_spec_change(
    pr: &MergedPull,
    default_branch: &str,
    now: Timestamp,
    policy: &Policy,
) -> Option<NotSpecChange> {
    let window_opened = now - policy.spec_lookback;
    let unlabeled = policy.require_label.as_ref().filter(|l| !pr.has_label(l));
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
    } else if pr.has_label(label::SPEC_PROCESSED) {
        NotSpecChange::Processed
    } else if pr.has_label(label::READY_FOR_HUMAN) {
        NotSpecChange::HandedBack
    } else if pr.merged_at < window_opened {
        NotSpecChange::BeforeLookback {
            merged_at: pr.merged_at,
            window_opened,
        }
    } else {
        // The last rule: without the required label it is none, and with it
        // or none required, a spec change.
        return unlabeled.map(|label| NotSpecChange::RequiredLabel {
            label: label.clone(),
        });
    };
    Some(because)
}
