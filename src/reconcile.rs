//! The functional core: one poll's decisions as a pure function of the
//! store's snapshot, the workflows and their last runs, the runs in flight
//! and the clock (`docs/specs/architecture.md`, "Functional core, imperative
//! shell"). The rules are the product spec's: `docs/specs/product.md` and the
//! areas it lists.

use chrono::TimeDelta;

use crate::domain::{
    CheckState, Comment, Filed, Issue, IssueEvent, Item, Login, MergeMethod, MergeMethods,
    Mergeability, MergedPull, Number, Pull, PullState, Rebaseability, Requirement, Review,
    ReviewState, Snapshot, Thread, Timestamp, UpdateMethod, UpdateRule, label,
};
use crate::marker::{Stage, marked, stage_of};
use crate::runtime::UsageLimit;
use crate::spec_glob::SpecGlob;
use crate::workflow::{LastRuns, Workflow, WorkflowName};

/// The settings the decisions depend on.
#[derive(Clone, Debug)]
pub struct Policy {
    pub machine_user: Login,
    pub spec_glob: SpecGlob,
    pub spec_lookback: TimeDelta,
    pub max_workers: usize,
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

impl Work {
    pub fn subject(&self) -> Subject {
        match self {
            Work::Triage { issue }
            | Work::Implement { issue }
            | Work::Rework { issue, .. }
            | Work::FixCi { issue, .. }
            | Work::Update { issue, .. } => Subject::Issue(*issue),
            Work::Tickets { pr } => Subject::SpecChange(*pr),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkKind {
    Triage,
    Tickets,
    Implement,
    Rework,
    FixCi,
    Update,
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

/// A merge of `pr` the store refused, for `reason`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedMerge {
    pub pr: Number,
    pub reason: String,
}

/// An update run started on `pr` at `head`, at `at`, to rebase it while
/// GitHub reported it not rebaseable. Another waits for a new head, or for
/// its ticket to be handed back or labeled `ready-for-agent` since, so that a
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
    /// Add the machine user as an assignee.
    Claim {
        issue: Number,
    },
    Relabel {
        issue: Number,
        remove: String,
        add: String,
    },
    /// Give the issue to the maintainer: `ready-for-human`, unassigned, and a
    /// comment that says why.
    HandBack {
        issue: Number,
        stage: Stage,
        reason: String,
    },
    MarkReady {
        pr: Number,
    },
    /// Forget the pull request's failed merges, so that its ticket, given
    /// back and labeled again, retries the merge afresh.
    ForgetFailedMerges {
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
    /// Keep the workflows' last runs, which changed, under the work
    /// directory.
    RecordLastRuns(LastRuns),
    /// Run the workflow, as `linewright workflow run` does.
    RunWorkflow(Box<Workflow>),
}

pub fn reconcile(
    snapshot: &Snapshot,
    live: &[LiveRun],
    memory: &Memory,
    workflows: &Workflows,
    now: Timestamp,
    policy: &Policy,
) -> Vec<Action> {
    let mut plan = Plan {
        now,
        actions: vec![],
        starts: vec![],
    };
    let is_live = |subject: Subject| live.iter().any(|run| run.subject == subject);

    for run in live {
        if let Some(reason) = cancel_reason(run, snapshot, policy) {
            plan.act(Action::Cancel {
                subject: run.subject,
                reason,
            });
        }
    }

    for pr in snapshot
        .merged
        .iter()
        .filter(|pr| is_spec_change(pr, snapshot, now, policy))
    {
        if is_live(Subject::SpecChange(pr.number)) {
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

    for issue in snapshot.issues.iter().filter(|i| i.open) {
        let subject = Subject::Issue(issue.number);
        let is_tracking = matches!(issue.filed, Some(Filed::Tracking { .. }));
        if is_tracking && !issue.sub_issues.is_empty() && issue.sub_issues.iter().all(|t| !t.open) {
            plan.act(Action::CloseTracking {
                issue: issue.number,
            });
        }
        let answered = answered_label(issue, policy);
        if let Some(waiting) = answered {
            plan.act(Action::Relabel {
                issue: issue.number,
                remove: waiting.into(),
                add: label::NEEDS_TRIAGE.into(),
            });
        }
        if answered.is_some() || wants_triage(issue, policy) {
            if !is_live(subject) {
                plan.start(Work::Triage {
                    issue: issue.number,
                });
            }
        } else if issue.has_label(label::READY_FOR_AGENT) && !issue.has_label(label::NEEDS_TRIAGE) {
            let running = is_live(subject);
            implementation(issue, snapshot, running, memory, policy, &mut plan);
        }
    }

    // Workflow runs count against the ceiling beside the pipeline's. Under
    // the usage limit there is no room: a due workflow stays due.
    let room = if memory.holds_starts(now) {
        0
    } else {
        policy
            .max_workers
            .saturating_sub(live.len() + workflows.in_flight.len())
    };
    let (mut actions, room) = plan.into_actions(room);
    actions.extend(schedule(workflows, now, room));
    actions
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
/// implementation, loses the machine user as an assignee.
fn cancel_reason(run: &LiveRun, snapshot: &Snapshot, policy: &Policy) -> Option<String> {
    match run.subject {
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
            } else if run.kind != WorkKind::Triage && !issue.is_assigned(&policy.machine_user) {
                Some(format!(
                    "{} was unassigned from {number}",
                    policy.machine_user
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
    /// implementations, each oldest first. Also returns the room left.
    fn into_actions(self, room: usize) -> (Vec<Action>, usize) {
        let mut starts = self.starts;
        starts.sort_by_key(|s| (rank(s.work.kind()), s.work.subject()));
        let room_left = room.saturating_sub(starts.len());
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
        (actions, room_left)
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

/// An issue labeled `ready-for-agent`: claim it when it is unblocked, and
/// drive it to a pull request that is green and ready for review.
fn implementation(
    issue: &Issue,
    snapshot: &Snapshot,
    live: bool,
    memory: &Memory,
    policy: &Policy,
    plan: &mut Plan,
) {
    let claimed = issue.is_assigned(&policy.machine_user);
    if live || (!claimed && (stopped_by_unassign(issue, policy) || !policy.may_start_on(issue))) {
        return;
    }
    let mut pulls = snapshot
        .pulls
        .iter()
        .filter(|p| p.author == policy.machine_user && p.closes.contains(&issue.number));
    if let Some(pr) = pulls
        .clone()
        .filter(|p| p.is_open())
        .max_by_key(|p| p.number)
    {
        if !claimed {
            plan.act(Action::Claim {
                issue: issue.number,
            });
        }
        return in_review(issue, pr, memory, policy, plan);
    }
    // GitHub closes the issue shortly after the merge; until then, or after a
    // reopen, only applying the label again asks for more work.
    let relabeled_since = |at| last_labeled(issue, label::READY_FOR_AGENT).is_some_and(|l| l > at);
    if pulls
        .clone()
        .any(|p| matches!(p.state, PullState::Merged { at } if !relabeled_since(at)))
    {
        return;
    }
    // Still claimed after a merge, the issue was reopened, and more work on
    // it is a start.
    let reopened = pulls
        .clone()
        .any(|p| matches!(p.state, PullState::Merged { .. }));
    if reopened && !policy.may_start_on(issue) {
        return;
    }
    if claimed {
        let closed = pulls
            .find(|p| matches!(p.state, PullState::Closed { at } if !linewright_commented_since(issue, at, policy)));
        if let Some(closed) = closed {
            return plan.act(Action::HandBack {
                issue: issue.number,
                stage: Stage::Review,
                reason: format!("{} was closed without merging.", closed.number),
            });
        }
    }
    if issue.blocked_by.iter().any(|b| b.open) {
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

fn last_linewright_comment(issue: &Issue, machine: &Login) -> Option<Timestamp> {
    issue
        .comments
        .iter()
        .filter(|c| stage_of(c, machine).is_some())
        .map(|c| c.created_at)
        .max()
}

fn linewright_commented_since(issue: &Issue, since: Timestamp, policy: &Policy) -> bool {
    issue
        .comments
        .iter()
        .any(|c| c.created_at > since && stage_of(c, &policy.machine_user).is_some())
}

/// An open pull request for the issue: address feedback, keep it up to date
/// and get CI green, in that order, then mark it ready for review or, with
/// auto-merge, merge it once no person holds it.
fn in_review(issue: &Issue, pr: &Pull, memory: &Memory, policy: &Policy, plan: &mut Plan) {
    let (issue_number, pr_number) = (issue.number, pr.number);
    let machine = &policy.machine_user;
    if !new_feedback(pr, machine).is_empty() {
        return plan.start(Work::Rework {
            issue: issue_number,
            pr: pr_number,
        });
    }
    let behind = matches!(
        pr.mergeability,
        Mergeability::Behind | Mergeability::Conflicting
    );
    let update = Work::Update {
        issue: issue_number,
        pr: pr_number,
    };
    if rebases_to_repair(pr, policy) && (behind || !tried_rebase(issue, pr, memory, machine)) {
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
    if pr.checks.is_empty() && now < pushed + policy.ci_grace {
        return;
    }
    // A ticket given back to the maintainer and labeled again waits a full
    // timeout again.
    let waiting_since = last_linewright_comment(issue, machine).map_or(pushed, |c| c.max(pushed));
    if !pending.is_empty() {
        if now > waiting_since + policy.ci_timeout {
            plan.act(Action::HandBack {
                issue: issue_number,
                stage: Stage::Ci,
                reason: format!(
                    "CI did not complete within {}: {} still running.",
                    humanize(policy.ci_timeout),
                    code_list(&pending)
                ),
            });
        }
        return;
    }
    if !failed.is_empty() {
        let cycle_start = [
            Some(pr.created_at),
            last_marked(&pr.comments, Stage::Rework, machine),
            last_linewright_comment(issue, machine),
        ]
        .into_iter()
        .flatten()
        .max();
        let attempts = pr
            .comments
            .iter()
            .filter(|c| stage_of(c, machine) == Some(Stage::Ci) && Some(c.created_at) > cycle_start)
            .count();
        if attempts < policy.ci_attempts as usize {
            plan.start(Work::FixCi {
                issue: issue_number,
                pr: pr_number,
                failing: failed,
            });
        } else {
            plan.act(Action::HandBack {
                issue: issue_number,
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
        Some(method) if !is_held(pr, machine) => method,
        _ => {
            if draft {
                plan.act(Action::MarkReady { pr: pr_number });
            }
            return;
        }
    };
    // Behind and conflicting were brought up to date above, and so was not
    // rebaseable, once at each head.
    match &pr.mergeability {
        Mergeability::Clean if method == MergeMethod::Rebase => match pr.rebaseability {
            Rebaseability::Rebaseable => merge(issue_number, pr, draft, method, memory, plan),
            Rebaseability::NotRebaseable => plan.act(Action::HandBack {
                issue: issue_number,
                stage: Stage::Review,
                reason: format!(
                    "GitHub reports {pr_number} not rebaseable though Linewright rebased it \
                     onto the default branch, so it cannot be merged by rebase."
                ),
            }),
            // Not computed yet: a later poll merges it.
            Rebaseability::Unknown => {}
        },
        Mergeability::Clean => merge(issue_number, pr, draft, method, memory, plan),
        // Held for review again only when what GitHub requires changes.
        Mergeability::Blocked { requires } if draft || !hold_noted(pr, requires, machine) => plan
            .act(Action::HoldForReview {
                pr: pr_number,
                draft,
                requires: requires.clone(),
            }),
        Mergeability::Unknown if draft => {
            plan.act(Action::MarkReadyForMergeState { pr: pr_number })
        }
        // A merge state not computed yet waits for the next poll.
        _ => {}
    }
}

/// Whether an update run already tried to rebase the pull request at its
/// head since the ticket was last handed back or labeled `ready-for-agent`:
/// a run cancelled or failed since is no rebase tried.
fn tried_rebase(issue: &Issue, pr: &Pull, memory: &Memory, machine: &Login) -> bool {
    let restarted = last_linewright_comment(issue, machine)
        .into_iter()
        .chain(last_labeled(issue, label::READY_FOR_AGENT))
        .max();
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

/// Merges the pull request with `method`, unless its merge has failed twice:
/// then its ticket is handed back.
fn merge(
    issue: Number,
    pr: &Pull,
    draft: bool,
    method: MergeMethod,
    memory: &Memory,
    plan: &mut Plan,
) {
    match merge_failed_twice(pr, &memory.failed_merges) {
        Some(failed) => {
            plan.act(Action::ForgetFailedMerges { pr: pr.number });
            plan.act(Action::HandBack {
                issue,
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

/// The pull request's last failed merge, if its merge has failed twice: a
/// failure is retried once.
fn merge_failed_twice<'a>(pr: &Pull, failed_merges: &'a [FailedMerge]) -> Option<&'a FailedMerge> {
    let failed: Vec<&FailedMerge> = failed_merges.iter().filter(|f| f.pr == pr.number).collect();
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
fn hold_noted(pr: &Pull, requires: &[Requirement], machine: &Login) -> bool {
    let note = marked(Stage::Review, &hold_note(requires));
    pr.comments
        .iter()
        .filter(|c| stage_of(c, machine) == Some(Stage::Review))
        .max_by_key(|c| c.created_at)
        .is_some_and(|c| c.body.trim() == note)
}

/// Whether a person who commented on or reviewed the pull request holds its
/// merge: they have no approving review newer than every comment they made
/// on it, as a PR comment, in a review thread or as another review. The
/// inline comments submitted with an approval are made no later than it, so
/// they don't hold the merge. Bots and the machine user hold nothing.
fn is_held(pr: &Pull, machine: &Login) -> bool {
    let comments = pr
        .comments
        .iter()
        .chain(pr.threads.iter().flat_map(|t| &t.comments))
        .map(|c| (&c.author, c.created_at));
    let reviews = pr.reviews.iter().map(|r| (&r.author, r.submitted_at));
    let activity: Vec<_> = comments
        .chain(reviews)
        .filter(|(author, _)| author.is_person_besides(machine))
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

/// Comments and reviews from anyone other than the machine user, posted after
/// Linewright last answered: its last push, or its last rework comment when a
/// rework needed no change. A review that only approves is not feedback.
pub fn new_feedback<'a>(pr: &'a Pull, machine: &Login) -> Feedback<'a> {
    let pushed = pr.pushes.iter().filter(|p| &p.by == machine).map(|p| p.at);
    let created = (&pr.author == machine).then_some(pr.created_at);
    let answered = pushed
        .chain(created)
        .chain(last_marked(&pr.comments, Stage::Rework, machine))
        .max();
    let new = |author: &Login, at: Timestamp| {
        author != machine && answered.is_none_or(|answered| at > answered)
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
    machine: &Login,
) -> Option<Timestamp> {
    comments
        .iter()
        .filter(|c| stage_of(c, machine) == Some(stage))
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

/// A person unassigning the machine user stops the issue's work until
/// `ready-for-agent` is applied again.
fn stopped_by_unassign(issue: &Issue, policy: &Policy) -> bool {
    let machine = &policy.machine_user;
    let unassigned = issue
        .events
        .iter()
        .filter_map(|e| match e {
            IssueEvent::Unassigned {
                assignee,
                actor,
                at,
            } if assignee == machine && actor != machine => Some(*at),
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

/// Triage runs on `needs-triage` issues not authored by the machine user:
/// tickets Linewright creates are already briefed.
fn wants_triage(issue: &Issue, policy: &Policy) -> bool {
    issue.has_label(label::NEEDS_TRIAGE)
        && issue.author != policy.machine_user
        && policy.may_start_on(issue)
}

/// An answer in the thread (`docs/specs/triage.md`): the label of
/// an issue waiting on the reporter or the maintainer, if Linewright's last
/// comment on it is a triage comment and anyone other than the machine user
/// commented after it and after the label was applied. A hand-back from after
/// triage asks no question, and a label the maintainer applied wins over their
/// comment.
fn answered_label(issue: &Issue, policy: &Policy) -> Option<&'static str> {
    let machine = &policy.machine_user;
    if issue.author == *machine
        || !policy.may_start_on(issue)
        || [label::READY_FOR_AGENT, label::NEEDS_TRIAGE]
            .into_iter()
            .any(|l| issue.has_label(l))
    {
        return None;
    }
    let waiting = [label::NEEDS_INFO, label::READY_FOR_HUMAN]
        .into_iter()
        .find(|l| issue.has_label(l))?;
    let (stage, last) = issue
        .comments
        .iter()
        .filter_map(|c| Some((stage_of(c, machine)?, c.created_at)))
        .max_by_key(|(_, at)| *at)?;
    if stage != Stage::Triage {
        return None;
    }
    let asked = last_labeled(issue, waiting).map_or(last, |labeled| labeled.max(last));
    issue
        .comments
        .iter()
        .any(|c| &c.author != machine && c.created_at > asked)
        .then_some(waiting)
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
/// touching the spec, not authored by the machine user, neither processed nor
/// handed back and, with a label required, carrying it. The label is checked
/// on every poll, so one applied after the merge counts; `cancel_reason` does
/// not look at it, so a cut under way survives its removal.
fn is_spec_change(pr: &MergedPull, snapshot: &Snapshot, now: Timestamp, policy: &Policy) -> bool {
    pr.base_ref == snapshot.default_branch
        && pr.merged_at >= now - policy.spec_lookback
        && pr.author != policy.machine_user
        && !pr.has_label(label::SPEC_PROCESSED)
        && !pr.has_label(label::READY_FOR_HUMAN)
        && policy
            .require_label
            .as_ref()
            .is_none_or(|l| pr.has_label(l))
        && pr.files.iter().any(|f| policy.spec_glob.matches(f))
}
