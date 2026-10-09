//! The functional core: one poll's decisions as a pure function of the
//! store's snapshot, the workflows and their last runs, the runs in flight
//! and the clock (`docs/specs/architecture.md`, "Functional core, imperative
//! shell"). The rules are `docs/specs/product.md`'s.

use chrono::TimeDelta;

use crate::domain::{
    CheckState, Comment, Filed, Issue, IssueEvent, Login, Mergeability, MergedPull, Number, Pull,
    PullState, Review, ReviewState, Snapshot, Thread, Timestamp, label,
};
use crate::marker::{Stage, stage_of};
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
    /// Merge green pull requests instead of requesting review.
    pub auto_merge: bool,
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
        reviewers: Vec<Login>,
    },
    /// Merge the pull request with the merge method, if its head is still
    /// `head`. A draft is marked ready for review first: GitHub merges no
    /// draft.
    Merge {
        pr: Number,
        head: String,
        draft: bool,
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
        let answered = has_answer(issue, policy);
        if answered {
            plan.act(Action::Relabel {
                issue: issue.number,
                remove: label::NEEDS_INFO.into(),
                add: label::NEEDS_TRIAGE.into(),
            });
        }
        if answered || wants_triage(issue, policy) {
            if !is_live(subject) {
                plan.start(Work::Triage {
                    issue: issue.number,
                });
            }
        } else if issue.has_label(label::READY_FOR_AGENT) && !issue.has_label(label::NEEDS_TRIAGE) {
            implementation(issue, snapshot, is_live(subject), policy, &mut plan);
        }
    }

    // Workflow runs count against the ceiling beside the pipeline's.
    let room = policy
        .max_workers
        .saturating_sub(live.len() + workflows.in_flight.len());
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
    work: Work,
}

impl Plan {
    fn act(&mut self, action: Action) {
        self.actions.push(action);
    }

    fn start(&mut self, work: Work) {
        self.starts.push(Start { claim: None, work });
    }

    fn claim_and_implement(&mut self, issue: Number) {
        self.starts.push(Start {
            claim: Some(issue),
            work: Work::Implement { issue },
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
    policy: &Policy,
    plan: &mut Plan,
) {
    let claimed = issue.is_assigned(&policy.machine_user);
    if live || (!claimed && stopped_by_unassign(issue, policy)) {
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
        return in_review(issue, pr, policy, plan);
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
fn in_review(issue: &Issue, pr: &Pull, policy: &Policy, plan: &mut Plan) {
    let (issue_number, pr_number) = (issue.number, pr.number);
    let machine = &policy.machine_user;
    if !new_feedback(pr, machine).is_empty() {
        return plan.start(Work::Rework {
            issue: issue_number,
            pr: pr_number,
        });
    }
    if matches!(
        pr.mergeability,
        Mergeability::Behind | Mergeability::Conflicting
    ) {
        return plan.start(Work::Update {
            issue: issue_number,
            pr: pr_number,
        });
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
    // A held pull request is marked ready as it is without auto-merge, so
    // the people holding it can approve it.
    if policy.auto_merge && !is_held(pr, machine) {
        // Behind and conflicting were brought up to date above; a
        // mergeability not computed yet waits for the next poll.
        if pr.mergeability == Mergeability::Clean {
            plan.act(Action::Merge {
                pr: pr_number,
                head: pr.head_sha.clone(),
                draft,
            });
        }
    } else if draft {
        plan.act(Action::MarkReady {
            pr: pr_number,
            reviewers: issue
                .assignees
                .iter()
                .filter(|a| a.is_person_besides(machine))
                .cloned()
                .collect(),
        });
    }
}

/// Whether a person who commented on or reviewed the pull request holds its
/// merge: their latest review does not approve it. Bots and the machine user
/// hold nothing.
fn is_held(pr: &Pull, machine: &Login) -> bool {
    let commenters = pr
        .comments
        .iter()
        .chain(pr.threads.iter().flat_map(|t| &t.comments))
        .map(|c| &c.author);
    let reviewers = pr.reviews.iter().map(|r| &r.author);
    commenters
        .chain(reviewers)
        .filter(|author| author.is_person_besides(machine))
        .any(|person| {
            pr.reviews
                .iter()
                .filter(|r| &r.author == person)
                .max_by_key(|r| r.submitted_at)
                .is_none_or(|latest| latest.state != ReviewState::Approved)
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
    issue.has_label(label::NEEDS_TRIAGE) && issue.author != policy.machine_user
}

/// `needs-info` is the one state a comment leaves: anyone other than the
/// machine user commenting after the question was asked, that is after
/// Linewright's last triage comment or the label, whichever is later.
fn has_answer(issue: &Issue, policy: &Policy) -> bool {
    if !issue.has_label(label::NEEDS_INFO) || issue.author == policy.machine_user {
        return false;
    }
    let last_triage = issue
        .comments
        .iter()
        .filter(|c| stage_of(c, &policy.machine_user) == Some(Stage::Triage))
        .map(|c| c.created_at)
        .max();
    let Some(asked) = last_triage.max(last_labeled(issue, label::NEEDS_INFO)) else {
        return false;
    };
    issue
        .comments
        .iter()
        .any(|c| c.author != policy.machine_user && c.created_at > asked)
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
/// touching the spec, not authored by the machine user, and neither processed
/// nor handed back.
fn is_spec_change(pr: &MergedPull, snapshot: &Snapshot, now: Timestamp, policy: &Policy) -> bool {
    pr.base_ref == snapshot.default_branch
        && pr.merged_at >= now - policy.spec_lookback
        && pr.author != policy.machine_user
        && !pr.has_label(label::SPEC_PROCESSED)
        && !pr.has_label(label::READY_FOR_HUMAN)
        && pr.files.iter().any(|f| policy.spec_glob.matches(f))
}
