use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::automation::Automation;
use crate::briefs::{Briefs, OperatorBriefs};
use crate::clock::Clock;
use crate::domain::{
    HandBackKind, Issue, Item, Login, MergedPull, Number, Pull, Snapshot, StatusLabel, Timestamp,
    addressed_to, closing_references, label,
};
use crate::git::{self, Updated, WorkState};
use crate::harness::{Harness, Purpose, SessionError, SessionSpec, Turn, UsageLimit};
use crate::prompts::{self, DesignOf, PullTask, Repo, Ticket};
use crate::reconcile::{
    Feedback, Policy, PullStop, Subject, Work, WorkKind, new_feedback, pull_stop, rebases_to_repair,
};
use crate::results::{
    self, AutomationIssueLabel, AutomationResult, Cut, DesignResult, ImplementResult, IssueWork,
    PullWorkResult, SpecGap, Tickets, TriageOutcome, TriageResult,
};
use crate::runs::{Ending, Running, Runs};
use crate::secrets::{Held, Secrets};
use crate::spec_gap::{self, FiledGaps};
use crate::store::{
    AnyItem, Categories, FoundSecrets, Identity, NewIssue, NewPull, PushError, PushMode,
    RepoSettings, Store,
};
use crate::tracking::{self, FiledTicket};
use crate::transcript;
use crate::work_settings::{self, Environments, RunKind, SessionSettings, WorkSettings};

pub(crate) struct Shell {
    pub store: Arc<dyn Store>,
    pub harness: Arc<dyn Harness>,
    pub clock: Arc<dyn Clock>,
    pub policy: Policy,
    /// The operator's settings for sessions, which each run resolves.
    pub work: WorkSettings,
    pub environments: Environments,
    /// The operator's layer of the briefs.
    pub operator_briefs: OperatorBriefs,
    pub identity: Identity,
    pub settings: RepoSettings,
    pub result_retries: u32,
    pub work_dir: PathBuf,
    pub runs: Runs,
    pub waits_out_usage_limit: bool,
    pub secrets: Secrets,
    pub report_refusal: tokio::sync::mpsc::UnboundedSender<UsageLimit>,
}

/// What a run works from, resolved as it starts.
pub(crate) struct RunWith {
    pub briefs: Arc<Briefs>,
    pub settings: SessionSettings,
}

impl Shell {
    /// What a run of `of` works from if it starts now.
    pub(crate) fn run_with(&self, of: RunKind) -> RunWith {
        RunWith {
            briefs: self.operator_briefs.briefs(),
            settings: self.work.for_run(
                of,
                &work_settings::RepoSettings::default(),
                &self.environments,
            ),
        }
    }

    fn repo(&self) -> Repo<'_> {
        Repo {
            name: &self.settings.repo,
            default_branch: &self.settings.default_branch,
            spec_glob: self.policy.spec_glob.pattern(),
        }
    }

    async fn creation_label(&self) -> Result<Option<String>> {
        let Some(name) = &self.policy.require_label else {
            return Ok(None);
        };
        if self.store.label_exists(name).await? {
            return Ok(Some(name.clone()));
        }
        warn!(
            label = name,
            "the required label does not exist: not applied"
        );
        Ok(None)
    }

    fn upstream(&self) -> String {
        format!("origin/{}", self.settings.default_branch)
    }
}

enum Failure {
    Infrastructure {
        error: anyhow::Error,
        /// What the hand-back adds after the reason.
        note: String,
    },
    HandBack(HandBack),
    Apply {
        error: anyhow::Error,
        /// What the hand-back adds after the reason.
        note: String,
    },
    Cancelled,
    UsageLimit(UsageLimit),
}

/// Linewright's letting go of an item it cannot finish (`GLOSSARY.md`,
/// **Hand-back**).
pub(crate) struct HandBack {
    pub(crate) kind: HandBackKind,
    pub(crate) reason: String,
    /// The end of the session's log, when the session's end is the reason.
    pub(crate) log_tail: Option<String>,
}

/// What a run works with, from its start to its end.
pub(crate) struct Ctx {
    pub(crate) with: RunWith,
    /// The run takes feedback since this as having arrived after.
    pub(crate) as_of: Timestamp,
    pub(crate) reworked: Option<Timestamp>,
    pub(crate) cancel: CancellationToken,
    pub(crate) run: Running,
}

impl Failure {
    /// Adds `note` to what the failure's hand-back says. A cancelled run and
    /// one that ends for the usage limit post nothing, so they take none.
    fn noted(self, note: &str) -> Failure {
        match self {
            Failure::Infrastructure {
                error,
                note: before,
            } => Failure::Infrastructure {
                error,
                note: before + note,
            },
            Failure::Apply {
                error,
                note: before,
            } => Failure::Apply {
                error,
                note: before + note,
            },
            Failure::HandBack(hand_back) => Failure::HandBack(HandBack {
                reason: hand_back.reason + note,
                ..hand_back
            }),
            failure @ (Failure::Cancelled | Failure::UsageLimit(_)) => failure,
        }
    }
}

fn apply(error: anyhow::Error) -> Failure {
    Failure::Apply {
        error,
        note: String::new(),
    }
}

fn infra(error: anyhow::Error) -> Failure {
    Failure::Infrastructure {
        error,
        note: String::new(),
    }
}

/// What became of a push.
enum Pushed {
    Done,
    /// GitHub's push protection refused it over the secrets it found,
    /// redacted.
    Protected(FoundSecrets),
    /// GitHub refused it for a missing permission, saying why.
    Refused(String),
    Failed(anyhow::Error),
}

async fn push(shell: &Shell, dir: &Path, branch: &str, mode: PushMode) -> Pushed {
    match shell.store.push(dir, branch, mode).await {
        Ok(()) => Pushed::Done,
        Err(PushError::PushProtection { secrets }) => {
            Pushed::Protected(secrets.redacted(shell.secrets.redaction()))
        }
        Err(PushError::MissingPermission { reason }) => Pushed::Refused(reason),
        Err(PushError::Failed(e)) => Pushed::Failed(e),
    }
}

/// The failure of a push of `branch` that did not succeed, handed back with
/// how to `again` when GitHub refused it.
fn pushed(branch: &str, pushed: Pushed, again: TryAgain) -> Result<(), Failure> {
    match pushed {
        Pushed::Done => Ok(()),
        Pushed::Refused(reason) => Err(Failure::HandBack(HandBack {
            kind: HandBackKind::Stall,
            reason: refused_push(branch, &reason, again),
            log_tail: None,
        })),
        // Only an update that needed no session pushes without one to send
        // it back to, and its commits GitHub already took.
        Pushed::Protected(secrets) => Err(infra(anyhow::anyhow!(
            "GitHub's push protection refused the push over a secret: {secrets}"
        ))),
        Pushed::Failed(e) => Err(infra(e)),
    }
}

#[derive(Clone, Copy)]
enum TryAgain {
    Relabel,
    Reassign,
}

fn refused_push(branch: &str, reason: &str, again: TryAgain) -> String {
    let remedy = match again {
        TryAgain::Relabel => format!(
            "Grant the permission and apply `{}` again, or make the change yourself.",
            label::READY_FOR_AGENT
        ),
        TryAgain::Reassign => "Grant the permission, or make the change yourself.".to_string(),
    };
    format!(
        "GitHub refused to push the branch `{branch}` because Linewright's token lacks a \
         permission. GitHub's reason: {reason}\n\nThe work was not kept. {remedy}"
    )
}

pub(crate) async fn run(shell: Arc<Shell>, work: Work, snapshot: Arc<Snapshot>, ctx: Ctx) {
    let ending = drive(&shell, &work, &snapshot, &ctx).await;
    ctx.run.end(shell.clock.now(), ending);
}

async fn drive(shell: &Shell, work: &Work, snapshot: &Snapshot, ctx: &Ctx) -> Ending {
    let subject = work.subject();
    let mut retried = false;
    let hand_back = loop {
        match attempt(shell, work, snapshot, ctx).await {
            Ok(()) => {
                info!(subject = ?subject, kind = ?work.kind(), "run finished");
                return Ending::Done;
            }
            Err(Failure::Infrastructure { error: e, .. }) if !retried => {
                let error = shell.secrets.redaction().apply(&format!("{e:#}"));
                warn!(subject = ?subject, error, "infrastructure failure; retrying once");
                retried = true;
            }
            Err(Failure::Infrastructure { error: e, note }) => {
                break HandBack {
                    kind: HandBackKind::Stall,
                    reason: format!("The run failed twice on its infrastructure: {e:#}{note}"),
                    log_tail: None,
                };
            }
            Err(Failure::Cancelled) => {
                info!(subject = ?subject, "run cancelled; its branch is kept");
                return Ending::Cancelled;
            }
            Err(Failure::UsageLimit(limit)) => {
                warn!(
                    subject = ?subject,
                    resets_at = %limit.resets_at,
                    "run ended for the usage limit; its claim and branch are kept"
                );
                return Ending::UsageLimit;
            }
            Err(Failure::Apply { error: e, note }) => {
                break HandBack {
                    kind: HandBackKind::Stall,
                    reason: format!("Recording the run's outcome failed: {e:#}{note}"),
                    log_tail: None,
                };
            }
            Err(Failure::HandBack(hand_back)) => break hand_back,
        }
    };
    if ctx.cancel.is_cancelled() {
        info!(subject = ?subject, reason = hand_back.reason, "run cancelled; not handed back");
        return Ending::Cancelled;
    }
    if let Subject::Pull(pr) = subject {
        match still_worked(shell, pr).await {
            Ok(true) => {}
            Ok(false) => return Ending::Cancelled,
            Err(e) => warn!(
                pr = pr.0,
                error = format!("{e:#}"),
                "reading the pull request failed"
            ),
        }
    }
    let work_dir = shell.work_dir.to_string_lossy().into_owned();
    // Git's output, as Linewright pushes, can come from hooks the session
    // left in its clone.
    let posted = |text: &str| {
        shell
            .secrets
            .redaction()
            .apply(text)
            .replace(&work_dir, "<work-dir>")
    };
    let hand_back = HandBack {
        reason: posted(&hand_back.reason),
        log_tail: hand_back.log_tail.map(|t| posted(&t)),
        ..hand_back
    };
    info!(subject = ?subject, reason = hand_back.reason, "handing back");
    let handed = match subject {
        Subject::Issue(issue) => {
            self::hand_back(shell, snapshot, Item::Issue(issue), &hand_back).await
        }
        Subject::Pull(pr) => self::hand_back(shell, snapshot, Item::Pull(pr), &hand_back).await,
        Subject::SpecChange(pr) => hand_back_spec_change(shell, pr, &hand_back).await,
    };
    match handed {
        Ok(()) => Ending::HandedBack,
        Err(e) => {
            warn!(subject = ?subject, error = format!("{e:#}"), "handing back failed");
            Ending::Failed
        }
    }
}

async fn attempt(
    shell: &Shell,
    work: &Work,
    snapshot: &Snapshot,
    ctx: &Ctx,
) -> Result<(), Failure> {
    let issue = |n: Number| {
        snapshot
            .issue(n)
            .with_context(|| format!("{n} is not in the snapshot"))
            .map_err(infra)
    };
    let pull = |n: Number| {
        snapshot
            .pull(n)
            .with_context(|| format!("{n} is not in the snapshot"))
            .map_err(infra)
    };
    let (n, pr, task) = match work {
        Work::Triage { issue: n } => return triage(shell, ctx, issue(*n)?).await,
        Work::DesignSpecChange { pr } => {
            let pr = snapshot
                .merged_pull(*pr)
                .with_context(|| format!("{pr} is not in the snapshot"))
                .map_err(infra)?;
            return design_spec_change(shell, ctx, snapshot, pr).await;
        }
        Work::DesignIssue { issue: n } => {
            return design_issue(shell, ctx, snapshot, issue(*n)?).await;
        }
        Work::Implement { issue: n } => {
            let issue = issue(*n)?;
            let branch = snapshot.ticket_branch(issue);
            let ticket = ticket_of(shell, snapshot, issue).await?;
            return implement(shell, ctx, &ticket, branch).await;
        }
        Work::Rework { issue: n, pr } => (n, pr, Task::Rework),
        Work::FixCi {
            issue: n,
            pr,
            failing,
        } => (n, pr, Task::FixCi(failing)),
        Work::Update { issue: n, pr } => (n, pr, Task::Update),
    };
    let held = snapshot_or_store_issue(shell, snapshot, *n).await?;
    let ticket = ticket_of(shell, snapshot, &held).await?;
    pull_work(shell, ctx, &ticket, pull(*pr)?, task).await
}

/// The snapshot may leave out an issue a run reads, such as the ticket of a
/// pull request it works or a ticket's parent.
async fn snapshot_or_store_issue<'a>(
    shell: &Shell,
    snapshot: &'a Snapshot,
    number: Number,
) -> Result<Cow<'a, Issue>, Failure> {
    if let Some(issue) = snapshot.issue(number) {
        return Ok(Cow::Borrowed(issue));
    }
    match shell.store.any_item(number).await.map_err(infra)? {
        Some(AnyItem::HeldIssue(issue) | AnyItem::LeftOutIssue(issue)) => Ok(Cow::Owned(*issue)),
        _ => Err(infra(anyhow::anyhow!("there is no issue {number}"))),
    }
}

async fn ticket_of<'a>(
    shell: &Shell,
    snapshot: &'a Snapshot,
    issue: &'a Issue,
) -> Result<Ticket<'a>, Failure> {
    let spec_diff = match issue.cut_from() {
        None => None,
        Some(spec_change) => {
            let pr = shell
                .store
                .merged_pull(spec_change, Timestamp::MIN_UTC)
                .await
                .map_err(infra)?
                .with_context(|| {
                    format!("the spec change {spec_change} is not a merged pull request")
                })
                .map_err(infra)?;
            Some(spec_diff(shell, &pr).await?)
        }
    };
    let parent = match issue.parent {
        None => None,
        Some(parent) => Some(snapshot_or_store_issue(shell, snapshot, parent.number).await?),
    };
    let filed_gaps = spec_gap::filed_for(snapshot, issue.number);
    Ok(Ticket::new(issue, parent, filed_gaps, spec_diff))
}

/// Whether the pull request is open and Linewright still works it.
async fn still_worked(shell: &Shell, pr: Number) -> Result<bool> {
    let pull = shell.store.pull(pr).await?;
    Ok(stopped(&pull, &shell.policy.linewright).is_none())
}

async fn still_open(shell: &Shell, pr: Number) -> Result<bool> {
    Ok(shell.store.pull(pr).await?.is_open())
}

/// Why work on the pull request stopped while the run worked, if it did.
fn stopped(pr: &Pull, linewright: &Login) -> Option<PullStop> {
    let stop = pull_stop(pr, linewright)?;
    match stop {
        PullStop::Merged | PullStop::Closed => info!(
            pr = pr.number.0,
            "the pull request closed while the run worked; nothing pushed or posted"
        ),
        PullStop::Unassigned => info!(
            pr = pr.number.0,
            "Linewright was unassigned while the run worked; nothing posted"
        ),
    }
    Some(stop)
}

/// `snapshot` holds the issue handed back, if it is one, as it was read.
pub(crate) async fn hand_back(
    shell: &Shell,
    snapshot: &Snapshot,
    item: Item,
    hand_back: &HandBack,
) -> Result<()> {
    match item {
        Item::Issue(n) => {
            let issue = snapshot
                .issue(n)
                .with_context(|| format!("{n} is not in the snapshot"))?;
            hand_back_issue(shell, issue, hand_back).await
        }
        Item::Pull(pr) => hand_back_pull(shell, pr, hand_back).await,
    }
}

/// Its own design goes with it, so that the next attempt designs it again.
async fn hand_back_issue(shell: &Shell, issue: &Issue, hand_back: &HandBack) -> Result<()> {
    let store = &shell.store;
    let n = issue.number;
    if !issue.was_cut(&shell.policy.linewright) {
        store.remove_label(n, label::DESIGNED).await?;
    }
    for state in StatusLabel::ALL
        .map(StatusLabel::label)
        .into_iter()
        .filter(|l| *l != label::READY_FOR_HUMAN)
    {
        store.remove_label(n, state).await?;
    }
    store.add_label(n, label::READY_FOR_HUMAN).await?;
    store.unassign(n).await?;
    mark_stall(shell, n, hand_back.kind).await?;
    store.comment(n, &with_log(hand_back)).await
}

async fn hand_back_pull(shell: &Shell, pr: Number, hand_back: &HandBack) -> Result<()> {
    shell.store.unassign(pr).await?;
    mark_stall(shell, pr, hand_back.kind).await?;
    let body = format!(
        "{}\n\nAssign @{} to this pull request again to try again.",
        with_log(hand_back),
        shell.policy.linewright
    );
    shell.store.comment(pr, &body).await
}

async fn hand_back_spec_change(shell: &Shell, pr: Number, hand_back: &HandBack) -> Result<()> {
    shell.store.remove_label(pr, label::DESIGNED).await?;
    shell.store.unassign(pr).await?;
    mark_stall(shell, pr, hand_back.kind).await?;
    let body = format!(
        "{}\n\nAssign @{} to this pull request again to design it again.",
        with_log(hand_back),
        shell.policy.linewright
    );
    shell.store.comment(pr, &body).await
}

/// Labels the item handed back a stall, once Linewright is unassigned from
/// it, so that a stop between the two never leaves an item it is assigned to
/// labeled, for the label's clearing to take off.
async fn mark_stall(shell: &Shell, number: Number, kind: HandBackKind) -> Result<()> {
    match kind {
        HandBackKind::Stall => shell.store.add_label(number, label::STALLED).await,
        HandBackKind::Decision => Ok(()),
    }
}

fn with_log(hand_back: &HandBack) -> String {
    let reason = &hand_back.reason;
    match hand_back
        .log_tail
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(tail) => format!(
            "{reason}\n\n<details><summary>The end of the session's log</summary>\n\n```\n{tail}\n```\n\n</details>"
        ),
        None => reason.clone(),
    }
}

struct Workspace {
    _temp: TempDir,
    dir: PathBuf,
    scratch: PathBuf,
    plugin: PathBuf,
}

impl Workspace {
    async fn clone(
        shell: &Shell,
        ctx: &Ctx,
        purpose: &Purpose,
        branch: &str,
    ) -> Result<Workspace, Failure> {
        let clones = shell.work_dir.join("clones");
        tokio::fs::create_dir_all(&clones)
            .await
            .context("creating the clones directory")
            .map_err(infra)?;
        let temp = tempfile::Builder::new()
            .prefix(&format!("{}-", purpose_name(purpose)))
            .tempdir_in(&clones)
            .context("creating a clone directory")
            .map_err(infra)?;
        let dir = temp.path().join("repo");
        shell.store.clone_repo(&dir, branch).await.map_err(infra)?;
        git::set_identity(&dir, &shell.identity.name, &shell.identity.email)
            .await
            .map_err(infra)?;
        let scratch = temp.path().join("tmp");
        tokio::fs::create_dir(&scratch)
            .await
            .context("creating a scratch directory")
            .map_err(infra)?;
        let plugin = temp.path().join("plugin");
        ctx.with
            .briefs
            .write_plugin(&plugin)
            .context("writing the briefs' plugin")
            .map_err(infra)?;
        Ok(Workspace {
            _temp: temp,
            dir,
            scratch,
            plugin,
        })
    }
}

pub fn subject_name(subject: Subject) -> String {
    match subject {
        Subject::Issue(n) => format!("issue-{}", n.0),
        Subject::SpecChange(n) => format!("pr-{}", n.0),
        Subject::Pull(n) => format!("pull-{}", n.0),
    }
}

pub fn purpose_name(purpose: &Purpose) -> String {
    match purpose {
        Purpose::Work { subject, .. } => subject_name(*subject),
        Purpose::Automation(name) => format!("automation-{name}"),
    }
}

fn record_dir(shell: &Shell, purpose: &Purpose, now: Timestamp) -> Result<PathBuf, Failure> {
    let parent = shell.work_dir.join("sessions").join(purpose_name(purpose));
    std::fs::create_dir_all(&parent)
        .context("creating the sessions directory")
        .map_err(infra)?;
    let kind = match purpose {
        Purpose::Work { kind, .. } => kind.name(),
        Purpose::Automation(_) => "automation",
    };
    let base = format!("{}-{kind}", now.format("%Y%m%dT%H%M%SZ"));
    for n in 1.. {
        let dir = if n == 1 {
            parent.join(&base)
        } else {
            parent.join(format!("{base}-{n}"))
        };
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(infra(e.into())),
        }
    }
    unreachable!()
}

enum Rejection {
    Retry(String),
    Fail(Failure),
}

impl From<anyhow::Error> for Rejection {
    fn from(error: anyhow::Error) -> Rejection {
        Rejection::Fail(infra(error))
    }
}

/// Sends a result that does not parse back to the session with its `error`.
fn parse_retry(ctx: &Ctx) -> impl Fn(String) -> Rejection + '_ {
    |error| Rejection::Retry(prompts::parse_error(&ctx.with.briefs.prompts, &error))
}

async fn converse<T, F, Fut>(
    shell: &Shell,
    ctx: &Ctx,
    spec: &mut SessionSpec,
    first: Step,
    accept: F,
) -> Result<(T, Turn), Failure>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T, Rejection>>,
{
    converse_counting(shell, ctx, spec, first, &mut 0, accept).await
}

/// As [`converse`], counting the follow-ups the result is sent back with in
/// `retries`, which holds those it was sent back with before.
async fn converse_counting<T, F, Fut>(
    shell: &Shell,
    ctx: &Ctx,
    spec: &mut SessionSpec,
    first: Step,
    retries: &mut u32,
    accept: F,
) -> Result<(T, Turn), Failure>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T, Rejection>>,
{
    let mut turn = take_turn(shell, ctx, spec, first).await?;
    loop {
        match accept(turn.message.clone()).await {
            Ok(result) => {
                let json = results::json_value(&turn.message)
                    .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
                    .unwrap_or_else(|| turn.message.clone());
                record(spec, "result.json", &json);
                return Ok((result, turn));
            }
            Err(Rejection::Fail(failure)) => return Err(failure),
            Err(Rejection::Retry(followup)) => {
                let step = send_back(shell, spec, &turn, followup, retries)?;
                turn = take_turn(shell, ctx, spec, step).await?;
            }
        }
    }
}

/// The step that sends the result `turn` answered with back to its session
/// with `followup`, counted in `retries`, or the hand-back once that is past
/// `result-retries`.
fn send_back(
    shell: &Shell,
    spec: &SessionSpec,
    turn: &Turn,
    followup: String,
    retries: &mut u32,
) -> Result<Step, Failure> {
    if *retries < shell.result_retries {
        *retries += 1;
        return Ok(Step::Resume {
            session: turn.session.clone(),
            prompt: followup,
        });
    }
    Err(Failure::HandBack(HandBack {
        kind: HandBackKind::Stall,
        reason: format!(
            "The session's result still could not be used after {retries} follow-up(s).\n\n{followup}"
        ),
        log_tail: Some(tail(&scrub(&turn.message, &spec.dir), 40)),
    }))
}

/// The step that sends a result whose push GitHub's push protection refused
/// over `secrets` back to its session, as [`send_back`], or the hand-back
/// saying the work was not kept.
fn send_back_protected(
    shell: &Shell,
    ctx: &Ctx,
    spec: &SessionSpec,
    turn: &Turn,
    secrets: &FoundSecrets,
    retries: &mut u32,
) -> Result<Step, Failure> {
    let followup = prompts::pushed_secret(&ctx.with.briefs.prompts, secrets);
    send_back(shell, spec, turn, followup, retries).map_err(|f| f.noted(&protected_note(secrets)))
}

enum Step {
    Start,
    Resume { session: String, prompt: String },
}

fn record(spec: &SessionSpec, name: &str, text: &str) {
    let _ = std::fs::write(spec.record.join(name), text);
}

async fn take_turn(
    shell: &Shell,
    ctx: &Ctx,
    spec: &mut SessionSpec,
    mut step: Step,
) -> Result<Turn, Failure> {
    let subject = purpose_name(&spec.purpose);
    loop {
        let turn = match &step {
            Step::Start => {
                record(spec, transcript::BRIEF, &spec.prompt);
                cancellable(ctx, shell.harness.start(spec)).await?
            }
            Step::Resume { session, prompt } => {
                record(spec, &followup_name(&spec.record), prompt);
                cancellable(ctx, shell.harness.resume(spec, session, prompt)).await?
            }
        };
        let (limit, underway) = match turn {
            Ok(turn) => return Ok(turn),
            Err(SessionError::UsageLimit { limit, underway }) => (limit, underway),
            Err(SessionError::Infrastructure(reason)) => {
                return Err(infra(anyhow::anyhow!(reason)));
            }
            Err(e @ SessionError::TimedOut { .. }) => {
                let SessionError::TimedOut { log_tail } = &e else {
                    unreachable!("matched as a timeout")
                };
                return Err(Failure::HandBack(HandBack {
                    kind: HandBackKind::Stall,
                    reason: format!(
                        "The session ended unexpectedly: {e}. The work may need cutting \
                         smaller, or {} may need to be longer.",
                        ctx.with.settings.timeout_setting
                    ),
                    log_tail: Some(scrub(log_tail, &spec.dir)),
                }));
            }
            Err(SessionError::Failed { reason, log_tail }) => {
                return Err(Failure::HandBack(HandBack {
                    kind: HandBackKind::Stall,
                    reason: format!("The session ended unexpectedly: {reason}"),
                    log_tail: Some(scrub(&log_tail, &spec.dir)),
                }));
            }
        };
        let _ = shell.report_refusal.send(limit);
        if !shell.waits_out_usage_limit {
            return Err(Failure::UsageLimit(limit));
        }
        let resets_at = limit.resets_at;
        let left = spec
            .deadline
            .saturating_duration_since(tokio::time::Instant::now());
        let until = limit.wait_until(shell.clock.now());
        warn!(subject, %resets_at, "session refused for the usage limit");
        info!(subject, %resets_at, %until, "waiting out the usage limit");
        cancellable(ctx, shell.clock.sleep_until(until)).await?;
        spec.deadline = tokio::time::Instant::now() + left;
        match underway {
            Some(session) => {
                info!(subject, %resets_at, "usage limit reset; resuming the session");
                step = Step::Resume {
                    session,
                    prompt: prompts::usage_limit_reset(&ctx.with.briefs.prompts),
                };
            }
            None => info!(subject, %resets_at, "usage limit reset; taking the refused turn again"),
        }
    }
}

/// What the session committed stays in the clone for the caller to keep.
async fn cancellable<T>(ctx: &Ctx, turn: impl Future<Output = T>) -> Result<T, Failure> {
    tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => Err(Failure::Cancelled),
        output = turn => Ok(output),
    }
}

fn followup_name(record: &Path) -> String {
    let n = (1..=usize::MAX)
        .find(|n| !record.join(transcript::followup_file(*n)).exists())
        .expect("a session has fewer follow-ups than numbers");
    transcript::followup_file(n)
}

fn scrub(text: &str, dir: &Path) -> String {
    text.replace(&dir.to_string_lossy().into_owned(), ".")
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

fn session_spec(
    shell: &Shell,
    ctx: &Ctx,
    purpose: Purpose,
    ws: &Workspace,
    prompt: String,
) -> Result<SessionSpec, Failure> {
    let settings = &ctx.with.settings;
    let now = shell.clock.now();
    let record = record_dir(shell, &purpose, now)?;
    ctx.run.session(&record, settings, &ctx.with.briefs);
    Ok(SessionSpec {
        record,
        purpose,
        dir: ws.dir.clone(),
        scratch: ws.scratch.clone(),
        plugin: ws.plugin.clone(),
        prompt,
        model: settings.model.value.clone(),
        effort: settings.effort.value,
        environment: settings.environment.value.clone(),
        deadline: tokio::time::Instant::now() + settings.session_timeout.value,
    })
}

async fn triage(shell: &Shell, ctx: &Ctx, issue: &Issue) -> Result<(), Failure> {
    let purpose = Purpose::Work {
        subject: Subject::Issue(issue.number),
        kind: WorkKind::Triage,
    };
    let ws = Workspace::clone(shell, ctx, &purpose, &shell.settings.default_branch).await?;
    let categories = shell.settings.categories.names();
    let prompt = prompts::triage(&ctx.with.briefs.prompts, &shell.repo(), issue, &categories);
    let mut spec = session_spec(shell, ctx, purpose, &ws, prompt)?;
    let categories = &categories;
    let (result, _) = converse(shell, ctx, &mut spec, Step::Start, |message| async move {
        results::parse_triage(&message, categories).map_err(parse_retry(ctx))
    })
    .await?;
    // The session may end as the run is cancelled, too late to stop it.
    if ctx.cancel.is_cancelled() {
        return Err(Failure::Cancelled);
    }
    record_triage(shell, issue, result).await.map_err(apply)
}

async fn record_triage(shell: &Shell, issue: &Issue, result: TriageResult) -> Result<()> {
    let store = &shell.store;
    let (state, body) = match &result.outcome {
        TriageOutcome::ReadyForAgent => (label::READY_FOR_AGENT, result.comment),
        TriageOutcome::NeedsInfo { ask } => {
            let people = issue.people_asked(ask, &shell.policy.linewright);
            let refused = store.assign_people(issue.number, &people).await?;
            if !refused.is_empty() {
                warn!(
                    issue = issue.number.0,
                    ?refused,
                    "GitHub refused to assign people asked"
                );
            }
            (label::NEEDS_INFO, addressed_to(&people, &result.comment))
        }
        TriageOutcome::ReadyForHuman => (
            label::READY_FOR_HUMAN,
            format!(
                "{}\n\nAnswer with a label: {}. A comment alone starts nothing.",
                result.comment,
                label::ANSWERS
                    .map(|(answer, asks)| format!("`{answer}` {asks}"))
                    .join(" or ")
            ),
        ),
    };
    // Posted before the labeling, so that a stop between the two leaves no
    // status label without the comment it stands on.
    store.comment(issue.number, &body).await?;
    for other in StatusLabel::ALL
        .map(StatusLabel::label)
        .into_iter()
        .filter(|l| *l != state)
    {
        store.remove_label(issue.number, other).await?;
    }
    match &shell.settings.categories {
        Categories::IssueTypes(_) => store.set_issue_type(issue.number, &result.category).await?,
        Categories::Labels => {
            for other in [label::BUG, label::ENHANCEMENT]
                .into_iter()
                .filter(|l| *l != result.category)
            {
                if issue.has_label(other) {
                    store.remove_label(issue.number, other).await?;
                }
            }
            store.add_label(issue.number, &result.category).await?;
        }
    }
    store.add_label(issue.number, state).await?;
    if !result.outcome.asks_for_linewright() {
        store.unassign(issue.number).await?;
    }
    Ok(())
}

/// Runs a design session on `subject`, sent `prompt`.
async fn design_session<C: DeserializeOwned>(
    shell: &Shell,
    ctx: &Ctx,
    subject: Subject,
    prompt: String,
) -> Result<(DesignResult<C>, Turn), Failure> {
    let purpose = Purpose::Work {
        subject,
        kind: WorkKind::Design,
    };
    let ws = Workspace::clone(shell, ctx, &purpose, &shell.settings.default_branch).await?;
    let mut spec = session_spec(shell, ctx, purpose, &ws, prompt)?;
    converse(shell, ctx, &mut spec, Step::Start, |message| async move {
        results::parse::<DesignResult<C>>(&message).map_err(parse_retry(ctx))
    })
    .await
}

/// A design blocked on `question` files its spec gaps and hands its item
/// back.
async fn design_blocked(
    shell: &Shell,
    question: &str,
    spec_gaps: &[SpecGap],
    filed_gaps: &FiledGaps<'_>,
) -> Result<(), Failure> {
    file_spec_gaps(shell, spec_gaps, filed_gaps)
        .await
        .map_err(apply)?;
    Err(Failure::HandBack(HandBack {
        kind: HandBackKind::Decision,
        reason: blocked_on(question),
        log_tail: None,
    }))
}

fn design_gave_up(of: DesignOf, reason: &str, turn: Turn) -> Failure {
    Failure::HandBack(HandBack {
        kind: HandBackKind::Decision,
        reason: format!("The designer handed the {} back:\n\n{reason}", of.noun()),
        log_tail: Some(turn.log_tail),
    })
}

async fn design_spec_change(
    shell: &Shell,
    ctx: &Ctx,
    snapshot: &Snapshot,
    pr: &MergedPull,
) -> Result<(), Failure> {
    let diff = spec_diff(shell, pr).await?;
    let filed_issues = spec_gap::filed_for(snapshot, pr.number);
    let filed_gaps = FiledGaps {
        item: pr.number,
        issues: &filed_issues,
    };
    let prompt = prompts::design_spec_change(
        &ctx.with.briefs.prompts,
        &shell.repo(),
        pr,
        &diff,
        &filed_issues,
    );
    let subject = Subject::SpecChange(pr.number);
    let (result, turn) = design_session::<Cut>(shell, ctx, subject, prompt).await?;
    match result {
        DesignResult::Designed {
            design,
            cut: Cut { tickets, comment },
            spec_gaps,
        } => {
            file_tickets(shell, pr, &design, &tickets, &comment)
                .await
                .map_err(apply)?;
            file_spec_gaps(shell, &spec_gaps, &filed_gaps)
                .await
                .map_err(apply)?;
            mark_designed(shell, pr).await.map_err(apply)
        }
        DesignResult::NothingToBuild { reason } => {
            shell
                .store
                .comment(pr.number, &reason)
                .await
                .map_err(apply)?;
            mark_designed(shell, pr).await.map_err(apply)
        }
        DesignResult::Blocked {
            question,
            spec_gaps,
        } => design_blocked(shell, &question, &spec_gaps, &filed_gaps).await,
        DesignResult::GaveUp { reason } => Err(design_gave_up(DesignOf::SpecChange, &reason, turn)),
    }
}

/// An issue that fits one session has its design posted on it, where its
/// implementer reads it; one cut into tickets becomes their tracking issue,
/// and its design goes there with them (docs/specs/design.md, "Designing").
async fn design_issue(
    shell: &Shell,
    ctx: &Ctx,
    snapshot: &Snapshot,
    issue: &Issue,
) -> Result<(), Failure> {
    let filed_issues = spec_gap::filed_for(snapshot, issue.number);
    let filed_gaps = FiledGaps {
        item: issue.number,
        issues: &filed_issues,
    };
    let prompt = prompts::design_issue(
        &ctx.with.briefs.prompts,
        &shell.repo(),
        issue,
        &filed_issues,
    );
    let subject = Subject::Issue(issue.number);
    let (result, turn) = design_session::<IssueWork>(shell, ctx, subject, prompt).await?;
    match result {
        DesignResult::Designed {
            design,
            cut,
            spec_gaps,
        } => {
            let store = &shell.store;
            let n = issue.number;
            match &cut {
                IssueWork::AsItIs => store.comment(n, &design).await,
                IssueWork::Cut(Cut { tickets, comment }) => {
                    async {
                        let required: Vec<String> =
                            shell.creation_label().await?.into_iter().collect();
                        let filed = file_tickets_under(shell, n, tickets, &required).await?;
                        publish_cut(shell, n, &design, &filed, comment).await
                    }
                    .await
                }
            }
            .map_err(apply)?;
            file_spec_gaps(shell, &spec_gaps, &filed_gaps)
                .await
                .map_err(apply)?;
            // Before letting go, so that a design stopped before it is made
            // again.
            store.add_label(n, label::DESIGNED).await.map_err(apply)?;
            if let IssueWork::Cut(_) = cut {
                // A tracking issue carries no status label, and is worked
                // through its tickets.
                for status in StatusLabel::ALL {
                    store.remove_label(n, status.label()).await.map_err(apply)?;
                }
                store.unassign(n).await.map_err(apply)?;
            }
            Ok(())
        }
        DesignResult::NothingToBuild { reason } => Err(Failure::HandBack(HandBack {
            kind: HandBackKind::Decision,
            reason: format!(
                "There is nothing to build: the code already does what this issue asks.\n\n{reason}"
            ),
            log_tail: None,
        })),
        DesignResult::Blocked {
            question,
            spec_gaps,
        } => design_blocked(shell, &question, &spec_gaps, &filed_gaps).await,
        DesignResult::GaveUp { reason } => Err(design_gave_up(DesignOf::Issue, &reason, turn)),
    }
}

async fn spec_diff(shell: &Shell, pr: &MergedPull) -> Result<String, Failure> {
    let spec_files: Vec<String> = pr
        .files
        .iter()
        .filter(|f| shell.policy.spec_glob.matches(f))
        .cloned()
        .collect();
    shell
        .store
        .diff(pr.number, &spec_files)
        .await
        .map_err(infra)
}

/// The label goes on last, so that a design stopped before it is discarded
/// and made again (docs/specs/design.md, "Discarding a cut").
async fn mark_designed(shell: &Shell, pr: &MergedPull) -> Result<()> {
    // Labeled before unassigning, so that a stop between the two leaves the
    // spec change designed rather than handed back.
    shell.store.add_label(pr.number, label::DESIGNED).await?;
    shell.store.unassign(pr.number).await
}

/// Files the tickets under a tracking issue, and records the design on it
/// and the tickets on the spec change.
async fn file_tickets(
    shell: &Shell,
    pr: &MergedPull,
    design: &str,
    tickets: &Tickets,
    comment: &str,
) -> Result<()> {
    let store = &shell.store;
    let required: Vec<String> = shell.creation_label().await?.into_iter().collect();
    let tracking_issue = store
        .create_issue(NewIssue {
            title: tracking::title(&pr.title),
            body: tracking::body(pr.number, &[]),
            labels: required.clone(),
        })
        .await?;
    let list = file_tickets_under(shell, tracking_issue, tickets, &required).await?;
    store
        .edit_body(tracking_issue, &tracking::body(pr.number, &list))
        .await?;
    publish_cut(shell, tracking_issue, design, &list, comment).await?;
    store
        .comment(
            pr.number,
            &format!(
                "{comment}\n\nTracked in {tracking_issue}:\n\n{}",
                in_order(&list)
            ),
        )
        .await
}

/// Files `tickets` as sub-issues of `tracking_issue`, labeled `required`
/// and each linked to the tickets blocking it, and returns them in order,
/// not yet ready: [`publish_cut`] readies them.
async fn file_tickets_under(
    shell: &Shell,
    tracking_issue: Number,
    tickets: &Tickets,
    required: &[String],
) -> Result<Vec<FiledTicket>> {
    let store = &shell.store;
    let mut list: Vec<FiledTicket> = vec![];
    for ticket in tickets.in_order() {
        let number = store
            .create_issue(NewIssue {
                title: ticket.title.clone(),
                body: ticket.body.clone(),
                labels: required.to_vec(),
            })
            .await?;
        store.add_sub_issue(tracking_issue, number).await?;
        for &blocker in &ticket.blocked_by {
            store.add_blocked_by(number, list[blocker].number).await?;
        }
        list.push(FiledTicket {
            number,
            title: ticket.title.clone(),
        });
    }
    Ok(list)
}

/// Posts the design of work cut into `tickets` on their `tracking_issue`,
/// then readies them.
async fn publish_cut(
    shell: &Shell,
    tracking_issue: Number,
    design: &str,
    tickets: &[FiledTicket],
    comment: &str,
) -> Result<()> {
    let store = &shell.store;
    store
        .comment(tracking_issue, &with_tickets(design, tickets, comment))
        .await?;
    // Tickets get `ready-for-agent` only once every edge exists, so no poll
    // sees a ticket unblocked before its blockers are linked; once the design
    // is on the tracking issue, so that their implementers read it
    // (docs/specs/implementation.md, "What the implementer reads"); and only
    // once they are designed, so that none is taken for an issue to design.
    // A stop before then leaves the item undesigned, and its cut is discarded
    // with these tickets, ready or not (docs/specs/design.md, "Discarding a
    // cut").
    for ticket in tickets {
        store.add_label(ticket.number, label::DESIGNED).await?;
        store
            .add_label(ticket.number, label::READY_FOR_AGENT)
            .await?;
    }
    Ok(())
}

/// The design of work cut into `tickets`, with them in order and why
/// `comment` gives for the cut.
fn with_tickets(design: &str, tickets: &[FiledTicket], comment: &str) -> String {
    format!("{design}\n\n## Tickets\n\n{}\n{comment}", in_order(tickets))
}

fn in_order(tickets: &[FiledTicket]) -> String {
    tickets
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{}. {} {}\n", i + 1, t.number, t.title))
        .collect()
}

/// What became of the work of a run that stopped before it finished.
enum Kept {
    Nothing,
    Pushed,
    /// The work holds a secret, so it was not pushed.
    Held(Held),
    /// GitHub's push protection refused the push over the secrets it found,
    /// redacted.
    Protected(FoundSecrets),
    /// GitHub refused the push for a missing permission, saying why.
    Refused(String),
    /// Committing or pushing the work failed, saying why.
    Failed(String),
}

/// Keeps the work of an implement run that stopped before it finished on the
/// ticket's branch: docs/specs/runs.md, "Ending".
async fn keep_unfinished_work(shell: &Shell, dir: &Path, upstream: &str, branch: &str) -> Kept {
    reported(
        shell,
        branch,
        push_unfinished(shell, dir, branch, upstream, &[], PushMode::Replace).await,
    )
}

/// Keeps the work of a run on a pull request that stopped before it finished
/// on the pull request's branch, unless its session changed nothing since
/// the clone stood at `before`, the pull request closed or the work holds a
/// secret: docs/specs/runs.md, "Ending". The push leases the branch at `start`, so commits others pushed
/// since are not overwritten.
async fn keep_unfinished_pull_work(
    shell: &Shell,
    dir: &Path,
    pr: &Pull,
    start: &str,
    before: &WorkState,
) -> Kept {
    let kept = async {
        if git::work_state(dir).await? == *before {
            return Ok(Kept::Nothing);
        }
        if !still_open(shell, pr.number).await? {
            return Ok(Kept::Nothing);
        }
        let lease = PushMode::Lease {
            expected: start.to_string(),
        };
        let upstream = shell.upstream();
        push_unfinished(shell, dir, &pr.head_ref, start, &[&upstream], lease).await
    };
    reported(shell, &pr.head_ref, kept.await)
}

/// Commits what a stopped run's session left uncommitted, and pushes
/// `branch` with `mode` when that leaves it past `past`, unless the commits
/// the push adds hold a secret. Those are the commits past `past` and
/// past each of `taken_in`, the branches the work took in.
async fn push_unfinished(
    shell: &Shell,
    dir: &Path,
    branch: &str,
    past: &str,
    taken_in: &[&str],
    mode: PushMode,
) -> Result<Kept> {
    git::commit_unfinished(dir).await?;
    if git::commits_ahead(dir, past).await? == 0 {
        return Ok(Kept::Nothing);
    }
    let bases = [&[past], taken_in].concat();
    if let Some(held) = held_secret(shell, dir, &bases).await? {
        return Ok(Kept::Held(held));
    }
    match push(shell, dir, branch, mode).await {
        Pushed::Done => Ok(Kept::Pushed),
        Pushed::Protected(secrets) => Ok(Kept::Protected(secrets)),
        Pushed::Refused(reason) => Ok(Kept::Refused(reason)),
        Pushed::Failed(e) => Err(e),
    }
}

/// The first secret the commits on the clone's `HEAD` past each of
/// `past` add: docs/specs/runs.md, "A result the manager can't accept".
async fn held_secret(shell: &Shell, dir: &Path, past: &[&str]) -> Result<Option<Held>> {
    let additions = git::additions(dir, "HEAD", past).await?;
    Ok(shell.secrets.held(&additions))
}

/// What became of a stopped run's work on `branch`, logged when it was not
/// kept. Git's output is redacted, as hooks the session left in its clone
/// can print a secret.
fn reported(shell: &Shell, branch: &str, kept: Result<Kept>) -> Kept {
    let redacted = |text: &str| shell.secrets.redaction().apply(text);
    let kept = match kept {
        Ok(Kept::Refused(reason)) => Kept::Refused(redacted(&reason)),
        Ok(kept) => kept,
        Err(e) => Kept::Failed(redacted(&format!("{e:#}"))),
    };
    match &kept {
        Kept::Refused(reason) => warn!(
            branch,
            reason, "GitHub refused to keep the stopped run's work"
        ),
        Kept::Failed(error) => warn!(branch, error, "keeping the stopped run's work failed"),
        Kept::Protected(secrets) => info!(
            branch,
            %secrets, "GitHub's push protection refused the stopped run's work over a secret"
        ),
        Kept::Held(held) => info!(
            branch,
            variable = held.variable,
            "the stopped run's work holds a secret; it is not kept"
        ),
        Kept::Nothing | Kept::Pushed => {}
    }
    kept
}

/// What a hand-back adds to say where the work it `kept` on `branch` is, and
/// how to `try_again` when GitHub refused it.
fn kept_note(branch: &str, kept: &Kept, try_again: TryAgain) -> String {
    match kept {
        Kept::Nothing => String::new(),
        Kept::Pushed => format!("\n\nThe work so far is on the branch `{branch}`."),
        Kept::Held(held) => format!(
            "\n\nThe work so far was not kept: the commit `{}` adds the value of `{}`, a \
             secret, to `{}`.",
            held.commit, held.variable, held.file
        ),
        Kept::Protected(secrets) => protected_note(secrets),
        Kept::Refused(reason) => format!("\n\n{}", refused_push(branch, reason, try_again)),
        Kept::Failed(error) => {
            format!("\n\nKeeping the work so far on the branch `{branch}` failed: {error}")
        }
    }
}

/// What a hand-back adds to say the work was not kept, as GitHub's push
/// protection found `secrets` in it.
fn protected_note(secrets: &FoundSecrets) -> String {
    format!(
        "\n\nThe work so far was not kept, as it holds a secret: GitHub's push protection \
         found {secrets}."
    )
}

async fn implement(
    shell: &Shell,
    ctx: &Ctx,
    ticket: &Ticket<'_>,
    branch: String,
) -> Result<(), Failure> {
    let purpose = Purpose::Work {
        subject: Subject::Issue(ticket.number()),
        kind: WorkKind::Implement,
    };
    let ws = Workspace::clone(shell, ctx, &purpose, &shell.settings.default_branch).await?;
    // Where the run starts: docs/specs/implementation.md, "Working a
    // ticket". The clone, not the snapshot, says whether the branch exists: a
    // snapshot taken before an earlier run pushed it does not hold it.
    let upstream = shell.upstream();
    let start = git::check_out_branch(&ws.dir, &branch, &upstream)
        .await
        .map_err(infra)?;
    let prompt = prompts::implement(
        &ctx.with.briefs.prompts,
        &shell.repo(),
        ticket,
        &branch,
        &start,
    );
    let mut spec = session_spec(shell, ctx, purpose, &ws, prompt)?;
    let (dir, base) = (&ws.dir, &upstream);
    let accept = |message: String| async move {
        let result: ImplementResult = results::parse(&message).map_err(parse_retry(ctx))?;
        if !matches!(result, ImplementResult::Done { .. }) {
            return Ok(result);
        }
        if let Some(problem) = git::loose_ends(dir).await? {
            return Err(Rejection::Retry(prompts::loose_ends(
                &ctx.with.briefs.prompts,
                &problem,
            )));
        }
        if git::commits_ahead(dir, base).await? == 0 {
            return Err(Rejection::Retry(prompts::loose_ends(
                &ctx.with.briefs.prompts,
                "you answered `done`, but the branch has no commits. Commit your work, or answer `gave-up` with the reason if the ticket needs no change.",
            )));
        }
        let unfinished = git::unfinished_commits(dir, base, "HEAD").await?;
        if let Some(unfinished) = unfinished {
            return Err(Rejection::Retry(prompts::fold_unfinished(
                &ctx.with.briefs.prompts,
                &unfinished,
            )));
        }
        if let Some(held) = held_secret(shell, dir, &[base]).await? {
            return Err(Rejection::Retry(prompts::held_secret(
                &ctx.with.briefs.prompts,
                &held,
            )));
        }
        Ok(result)
    };
    let keep = async || {
        let kept = keep_unfinished_work(shell, &ws.dir, &upstream, &branch).await;
        kept_note(&branch, &kept, TryAgain::Relabel)
    };
    // A finished result is pushed while its session can still be resumed,
    // so that one GitHub refuses over a secret goes back to it.
    let (mut step, mut retries) = (Step::Start, 0);
    let (result, turn) = loop {
        let conversation = converse_counting(shell, ctx, &mut spec, step, &mut retries, &accept);
        let (result, turn) = match conversation.await {
            Ok(conversation) => conversation,
            Err(failure) => return Err(failure.noted(&keep().await)),
        };
        if !matches!(result, ImplementResult::Done { .. }) {
            break (result, turn);
        }
        match push(shell, &ws.dir, &branch, PushMode::Replace).await {
            Pushed::Protected(secrets) => {
                step = send_back_protected(shell, ctx, &spec, &turn, &secrets, &mut retries)?;
            }
            other @ (Pushed::Done | Pushed::Refused(_) | Pushed::Failed(_)) => {
                pushed(&branch, other, TryAgain::Relabel)?;
                break (result, turn);
            }
        }
    };
    let store = &shell.store;
    match result {
        ImplementResult::Done {
            pr_title,
            pr_body,
            spec_gaps,
        } => {
            let pr = store
                .open_pull(NewPull {
                    head: branch.clone(),
                    title: pr_title.into(),
                    body: closing(&pr_body, &[ticket.number()]),
                })
                .await
                .map_err(apply)?;
            // A pull request Linewright was never assigned to is claimed by
            // the next poll, so a failed assignment needs no hand-back.
            if let Err(e) = store.assign(pr).await {
                warn!(
                    pr = pr.0,
                    error = format!("{e:#}"),
                    "assigning the pull request failed"
                );
            }
            if let Some(required) = shell.creation_label().await.map_err(apply)? {
                store.add_label(pr, &required).await.map_err(apply)?;
            }
            info!(
                issue = ticket.number().0,
                pr = pr.0,
                "opened draft pull request"
            );
            file_spec_gaps(shell, &spec_gaps, &ticket.filed_gaps())
                .await
                .map_err(apply)
        }
        ImplementResult::Blocked {
            question,
            spec_gaps,
        } => {
            let kept = keep().await;
            file_spec_gaps(shell, &spec_gaps, &ticket.filed_gaps())
                .await
                .map_err(|e| apply(e).noted(&kept))?;
            Err(Failure::HandBack(HandBack {
                kind: HandBackKind::Decision,
                reason: format!("{}{kept}", blocked_on(&question)),
                log_tail: None,
            }))
        }
        ImplementResult::GaveUp { reason } => {
            let kept = keep().await;
            Err(Failure::HandBack(HandBack {
                kind: HandBackKind::Decision,
                reason: format!(
                    "The implementer handed the ticket back:\n\n{}{kept}",
                    reason.trim()
                ),
                log_tail: Some(turn.log_tail),
            }))
        }
    }
}

/// The reason of a hand-back blocked on `question`.
fn blocked_on(question: &str) -> String {
    format!(
        "Blocked on a question the spec cannot decide:\n\n{}",
        question.trim()
    )
}

fn closing(body: &str, closes: &[Number]) -> String {
    let referenced = closing_references(body);
    let missing: Vec<String> = closes
        .iter()
        .filter(|issue| !referenced.contains(issue))
        .map(|issue| format!("Closes {issue}"))
        .collect();
    if missing.is_empty() {
        body.trim().to_string()
    } else {
        format!("{}\n\n{}", body.trim(), missing.join("\n"))
    }
}

/// A session told which gaps are filed may still return one, so titles are
/// compared.
async fn file_spec_gaps(shell: &Shell, gaps: &[SpecGap], filed: &FiledGaps<'_>) -> Result<()> {
    let item = filed.item;
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    for gap in gaps {
        if let Some(filed) = filed.issues.iter().find(|f| same(&f.title, &gap.title)) {
            info!(
                issue = filed.number.0,
                item = item.0,
                "spec gap already filed"
            );
            continue;
        }
        let number = shell
            .store
            .create_issue(NewIssue {
                title: gap.title.clone(),
                body: spec_gap::body(&gap.body, item),
                labels: [label::READY_FOR_HUMAN.into()]
                    .into_iter()
                    .chain(shell.creation_label().await?)
                    .collect(),
            })
            .await?;
        info!(issue = number.0, item = item.0, "filed a spec gap");
    }
    Ok(())
}

enum Task<'a> {
    Rework,
    FixCi(&'a [String]),
    Update,
}

/// Pushes an update that needed no session, unless it changed nothing or
/// feedback arrived meanwhile.
async fn push_update(
    shell: &Shell,
    ctx: &Ctx,
    pr: &Pull,
    dir: &Path,
    start: &str,
) -> Result<(), Failure> {
    let store = &shell.store;
    let linewright = &shell.policy.linewright;
    if git::head(dir).await.map_err(infra)? == start {
        info!(pr = pr.number.0, "already up to date");
        return Ok(());
    }
    let fresh = store.pull(pr.number).await.map_err(infra)?;
    if stopped(&fresh, linewright).is_some() {
        return Err(Failure::Cancelled);
    }
    if !late(&new_feedback(&fresh, linewright, ctx.reworked), ctx.as_of).is_empty() {
        info!(
            pr = pr.number.0,
            "feedback arrived; leaving the update to a rework run"
        );
        return Ok(());
    }
    let lease = PushMode::Lease {
        expected: start.to_string(),
    };
    let outcome = push(shell, dir, &pr.head_ref, lease).await;
    pushed(&pr.head_ref, outcome, TryAgain::Reassign)
}

async fn pull_work(
    shell: &Shell,
    ctx: &Ctx,
    ticket: &Ticket<'_>,
    pr: &Pull,
    task: Task<'_>,
) -> Result<(), Failure> {
    // The sessions are kept with the ticket's, as a case captures them.
    let subject = Subject::Issue(ticket.number());
    let kind = match task {
        Task::Rework => WorkKind::Rework,
        Task::FixCi(_) => WorkKind::FixCi,
        Task::Update => WorkKind::Update,
    };
    let linewright = &shell.policy.linewright;
    let purpose = Purpose::Work { subject, kind };
    let ws = Workspace::clone(shell, ctx, &purpose, &pr.head_ref).await?;
    let start = git::head(&ws.dir).await.map_err(infra)?;
    let default_branch = &shell.settings.default_branch;
    let policy = &shell.policy;
    let updated = if rebases_to_repair(pr, policy) {
        git::rebase(&ws.dir, default_branch).await
    } else {
        git::update(&ws.dir, default_branch, policy.update_rule).await
    };
    let conflict = match updated.map_err(infra)? {
        Updated::Conflict(conflict) => Some(conflict),
        Updated::UpToDate | Updated::Updated => None,
    };
    let store = &shell.store;
    let lease = PushMode::Lease {
        expected: start.clone(),
    };
    let failures = match task {
        Task::FixCi(names) => store
            .check_failures(pr.number, names)
            .await
            .map_err(infra)?,
        Task::Rework | Task::Update => vec![],
    };
    let feedback = new_feedback(pr, linewright, ctx.reworked);
    let mut feedback_authors = feedback_people(&feedback, linewright);
    let pull_task = match (task, &conflict) {
        (Task::Rework, conflict) => PullTask::Rework {
            feedback,
            conflict: conflict.as_ref(),
        },
        (Task::FixCi(_), conflict) => PullTask::FixCi {
            failures: &failures,
            conflict: conflict.as_ref(),
        },
        (Task::Update, Some(conflict)) => PullTask::Update(conflict),
        (Task::Update, None) => return push_update(shell, ctx, pr, &ws.dir, &start).await,
    };
    let dir = &ws.dir;
    let upstream = shell.upstream();
    // A rebase that stopped on a conflict has not yet replayed the branch's
    // unfinished commits.
    let tip = if conflict.is_some() { &start } else { "HEAD" };
    let left_unfinished = git::unfinished_commits(dir, &upstream, tip)
        .await
        .map_err(infra)?;
    let prompt = prompts::pull_work(
        &ctx.with.briefs.prompts,
        &shell.repo(),
        ticket,
        pr,
        &pull_task,
        left_unfinished.as_ref(),
    );
    let mut spec = session_spec(shell, ctx, purpose, &ws, prompt)?;
    let mut threads: Vec<String> = pr.threads.iter().map(|t| t.id.clone()).collect();
    let before = git::work_state(dir).await.map_err(infra)?;
    let (upstream, base) = (&upstream, start.as_str());
    let accept = |message: String, pr_threads: Vec<String>| async move {
        let result: PullWorkResult = results::parse(&message).map_err(parse_retry(ctx))?;
        let PullWorkResult::Done { replies, .. } = &result else {
            return Ok(result);
        };
        if let Some(problem) = git::loose_ends(dir).await? {
            return Err(Rejection::Retry(prompts::loose_ends(
                &ctx.with.briefs.prompts,
                &problem,
            )));
        }
        if let Some(unfinished) = git::unfinished_commits(dir, upstream, "HEAD").await? {
            return Err(Rejection::Retry(prompts::fold_unfinished(
                &ctx.with.briefs.prompts,
                &unfinished,
            )));
        }
        if let Some(held) = held_secret(shell, dir, &[base, upstream]).await? {
            return Err(Rejection::Retry(prompts::held_secret(
                &ctx.with.briefs.prompts,
                &held,
            )));
        }
        if let Some(unknown) = replies.iter().find(|r| !pr_threads.contains(&r.thread)) {
            return Err(parse_retry(ctx)(format!(
                "`{}` is not a thread on the pull request",
                unknown.thread
            )));
        }
        Ok(result)
    };
    let keep = async || {
        let kept = keep_unfinished_pull_work(shell, dir, pr, &start, &before).await;
        kept_note(&pr.head_ref, &kept, TryAgain::Reassign)
    };
    let mut shown_until = ctx.as_of;
    // A finished result is pushed while its session can still be resumed,
    // as for an implement run.
    let (mut step, mut retries) = (Step::Start, 0);
    let (result, turn, pr_now, read_at) = loop {
        let ended: Result<_, Failure> = async {
            let (mut result, mut turn) =
                converse_counting(shell, ctx, &mut spec, step, &mut retries, |m| {
                    accept(m, threads.clone())
                })
                .await?;
            // Feedback posted after the snapshot the brief came from would
            // be older than the push that follows, so it goes to the session
            // before anything is pushed, for as long as it keeps coming
            // within the session's time.
            loop {
                let read_at = shell.clock.now();
                let pr_now = store.pull(pr.number).await.map_err(infra)?;
                if pull_stop(&pr_now, linewright).is_some()
                    || !matches!(result, PullWorkResult::Done { .. })
                {
                    return Ok((result, turn, pr_now, read_at));
                }
                let arrived = late(
                    &new_feedback(&pr_now, linewright, ctx.reworked),
                    shown_until,
                );
                if arrived.is_empty() {
                    return Ok((result, turn, pr_now, read_at));
                }
                shown_until = read_at;
                feedback_authors.extend(feedback_people(&arrived, linewright));
                threads = pr_now.threads.iter().map(|t| t.id.clone()).collect();
                let followup = prompts::late_feedback(&ctx.with.briefs.prompts, &pr_now, &arrived);
                let resume = Step::Resume {
                    session: turn.session.clone(),
                    prompt: followup,
                };
                // The answer to new feedback is a new result.
                retries = 0;
                (result, turn) =
                    converse_counting(shell, ctx, &mut spec, resume, &mut retries, |m| {
                        accept(m, threads.clone())
                    })
                    .await?;
            }
        }
        .await;
        let (result, turn, pr_now, read_at) = match ended {
            Ok(ended) => ended,
            Err(failure) => return Err(failure.noted(&keep().await)),
        };
        if stopped(&pr_now, linewright).is_some() {
            keep().await;
            return Err(Failure::Cancelled);
        }
        if !matches!(result, PullWorkResult::Done { .. })
            || git::head(dir).await.map_err(infra)? == start
        {
            break (result, turn, pr_now, read_at);
        }
        match push(shell, dir, &pr.head_ref, lease.clone()).await {
            Pushed::Protected(secrets) => {
                step = send_back_protected(shell, ctx, &spec, &turn, &secrets, &mut retries)?;
            }
            other @ (Pushed::Done | Pushed::Refused(_) | Pushed::Failed(_)) => {
                pushed(&pr.head_ref, other, TryAgain::Reassign)?;
                break (result, turn, pr_now, read_at);
            }
        }
    };
    match result {
        PullWorkResult::Done {
            comment,
            pr_body,
            replies,
            spec_gaps,
        } => {
            if let Some(body) = pr_body {
                store
                    .edit_body(pr.number, &closing(&body, &pr_now.closes))
                    .await
                    .map_err(apply)?;
            }
            record_pull_work(shell, &pr_now, kind, &comment, &replies, feedback_authors)
                .await
                .map_err(apply)?;
            // The session saw the feedback up to the last read; what came
            // after is for the next rework.
            if kind == WorkKind::Rework {
                ctx.run.answered(read_at);
            }
            file_spec_gaps(shell, &spec_gaps, &ticket.filed_gaps())
                .await
                .map_err(apply)
        }
        PullWorkResult::Blocked {
            question,
            spec_gaps,
        } => {
            let kept = keep().await;
            file_spec_gaps(shell, &spec_gaps, &ticket.filed_gaps())
                .await
                .map_err(|e| apply(e).noted(&kept))?;
            Err(Failure::HandBack(HandBack {
                kind: HandBackKind::Decision,
                reason: format!("{}{kept}", blocked_on(&question)),
                log_tail: None,
            }))
        }
        PullWorkResult::GaveUp { reason } => {
            let kept = keep().await;
            Err(Failure::HandBack(HandBack {
                kind: HandBackKind::Decision,
                reason: format!(
                    "The implementer handed the pull request back:\n\n{}{kept}",
                    reason.trim()
                ),
                log_tail: Some(turn.log_tail),
            }))
        }
    }
}

fn late<'a>(feedback: &Feedback<'a>, since: Timestamp) -> Feedback<'a> {
    Feedback {
        comments: feedback
            .comments
            .iter()
            .copied()
            .filter(|c| c.created_at > since)
            .collect(),
        reviews: feedback
            .reviews
            .iter()
            .copied()
            .filter(|r| r.submitted_at > since)
            .collect(),
        threads: feedback
            .threads
            .iter()
            .filter_map(|(t, comments)| {
                let comments: Vec<_> = comments
                    .iter()
                    .copied()
                    .filter(|c| c.created_at > since)
                    .collect();
                (!comments.is_empty()).then_some((*t, comments))
            })
            .collect(),
    }
}

fn feedback_people(feedback: &Feedback, linewright: &Login) -> Vec<Login> {
    feedback
        .comments
        .iter()
        .map(|c| &c.author)
        .chain(feedback.reviews.iter().map(|r| &r.author))
        .chain(
            feedback
                .threads
                .iter()
                .flat_map(|(_, cs)| cs.iter().map(|c| &c.author)),
        )
        .filter(|a| a.is_person_besides(linewright))
        .cloned()
        .collect()
}

async fn record_pull_work(
    shell: &Shell,
    pr: &Pull,
    kind: WorkKind,
    comment: &str,
    replies: &[results::ThreadReply],
    feedback_authors: Vec<Login>,
) -> Result<()> {
    let store = &shell.store;
    for reply in replies {
        store
            .reply_in_thread(pr.number, &reply.thread, &reply.body)
            .await?;
        let opened_by_bot = pr
            .threads
            .iter()
            .find(|t| t.id == reply.thread)
            .and_then(|t| t.opened_by())
            .is_some_and(Login::is_bot);
        if reply.resolve && opened_by_bot {
            store.resolve_thread(pr.number, &reply.thread).await?;
        }
    }
    store.comment(pr.number, comment).await?;
    if kind == WorkKind::Rework {
        let mut reviewers = feedback_authors;
        reviewers.sort();
        reviewers.dedup();
        if !reviewers.is_empty() {
            store.request_review(pr.number, &reviewers).await?;
        }
    }
    Ok(())
}

pub(crate) enum AutomationEnd {
    Finished(AutomationResult),
    UsageLimit(UsageLimit),
}

/// An automation run has no issue to hand back, so a run that fails is an error.
pub(crate) async fn automation(
    shell: &Shell,
    automation: &Automation,
    ctx: Ctx,
) -> Result<AutomationEnd> {
    let session = async {
        let purpose = Purpose::Automation(automation.name.clone());
        let ws = Workspace::clone(shell, &ctx, &purpose, &shell.settings.default_branch).await?;
        let prompt = prompts::automation(&ctx.with.briefs.prompts, &shell.repo(), automation);
        let mut spec = session_spec(shell, &ctx, purpose, &ws, prompt)?;
        let ctx = &ctx;
        let (result, _) = converse(shell, ctx, &mut spec, Step::Start, |message| async move {
            results::parse(&message).map_err(parse_retry(ctx))
        })
        .await?;
        record_automation(shell, automation, &result)
            .await
            .map_err(apply)?;
        Ok(result)
    };
    let ended = session.await;
    let ending = match &ended {
        Ok(_) => Ending::Done,
        Err(Failure::UsageLimit(_)) => Ending::UsageLimit,
        Err(Failure::Cancelled) => Ending::Cancelled,
        Err(Failure::Infrastructure { .. } | Failure::Apply { .. } | Failure::HandBack { .. }) => {
            Ending::Failed
        }
    };
    ctx.run.end(shell.clock.now(), ending);
    match ended {
        Ok(result) => Ok(AutomationEnd::Finished(result)),
        Err(Failure::UsageLimit(limit)) => Ok(AutomationEnd::UsageLimit(limit)),
        Err(Failure::Infrastructure { error: e, .. } | Failure::Apply { error: e, .. }) => Err(e),
        Err(Failure::HandBack(HandBack {
            reason, log_tail, ..
        })) => Err(
            match log_tail.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                Some(tail) => anyhow::anyhow!("{reason}\n\nThe end of the session's log:\n{tail}"),
                None => anyhow::anyhow!(reason),
            },
        ),
        Err(Failure::Cancelled) => Err(anyhow::anyhow!("the run was cancelled")),
    }
}

async fn record_automation(
    shell: &Shell,
    automation: &Automation,
    result: &AutomationResult,
) -> Result<()> {
    let cap = (automation.max_issues as usize).min(result.issues.len());
    let (filed, dropped) = result.issues.split_at(cap);
    for issue in filed {
        let state = match issue.label {
            AutomationIssueLabel::ReadyForAgent => label::READY_FOR_AGENT,
            AutomationIssueLabel::ReadyForHuman => label::READY_FOR_HUMAN,
        };
        let number = shell
            .store
            .create_issue(NewIssue {
                title: issue.title.clone(),
                body: format!(
                    "{}\n\nFiled by the automation `{}`.",
                    issue.body, automation.name
                ),
                labels: [state.into()]
                    .into_iter()
                    .chain(shell.creation_label().await?)
                    .collect(),
            })
            .await?;
        info!(
            automation = %automation.name,
            issue = %number,
            title = issue.title,
            "automation issue filed"
        );
    }
    for issue in dropped {
        warn!(
            automation = %automation.name,
            max_issues = automation.max_issues,
            title = issue.title,
            "automation issue dropped: the run returned more than max-issues"
        );
    }
    Ok(())
}
