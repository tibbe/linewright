use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::attachments::{self, Dir, Refusal};
use crate::automation::Automation;
use crate::briefs::{Briefs, OperatorPrompts};
use crate::clock::Clock;
use crate::domain::{
    HandBackKind, Issue, Item, Login, MergedPull, Number, Pull, Snapshot, StatusLabel, Timestamp,
    Windows, addressed_to, closing_references, label,
};
use crate::git::{self, Updated, WorkState};
use crate::harness::{
    self, Harness, Purpose, Reporter, SessionError, SessionSpec, Turn, UsageLimit,
};
use crate::prompts::{self, DesignOf, PullTask, Repo, Ticket};
use crate::reconcile::{
    Feedback, Policy, PullStop, Subject, Work, WorkKind, new_feedback, pull_stop, rebases_to_repair,
};
use crate::repo_files::RepoLayer;
use crate::results::{
    self, AutomationIssueLabel, AutomationResult, Contract, Cut, DesignResult, ImplementResult,
    IssueWork, Posts, PullWorkResult, SpecGap, SpecGaps, Tickets, TriageOutcome, TriageResult,
};
use crate::runs::{Ending, Running, Runs, SessionRecorder};
use crate::secrets::{Held, Secrets};
use crate::setup;
use crate::spec_gap::{self, FiledGaps};
use crate::store::{
    AnyItem, AttachError, Categories, FoundSecrets, Identity, NewIssue, NewPull, PushError,
    PushMode, RepoSettings, Store,
};
use crate::tracking::{self, FiledTicket};
use crate::transcript;
use crate::work_settings::{
    Environments, RunKind, SessionSettings, Setup, SetupCommand, WorkSettings,
};

pub(crate) struct Shell {
    pub store: Arc<dyn Store>,
    pub harness: Arc<dyn Harness>,
    /// The `bash` that runs the setup command.
    pub bash: PathBuf,
    pub clock: Arc<dyn Clock>,
    pub policy: Policy,
    /// The operator's settings for sessions, which each run resolves.
    pub work: WorkSettings,
    pub environments: Environments,
    /// The operator's layer of the prompts.
    pub operator_prompts: OperatorPrompts,
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
    /// What a run of `of` works from if it starts now, with the
    /// repository's files as `repo` holds them.
    pub(crate) fn run_with(&self, repo: &RepoLayer, of: RunKind) -> RunWith {
        RunWith {
            briefs: repo.briefs.clone(),
            settings: self.work.for_run(of, &repo.settings, &self.environments),
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
        /// The tail the hand-back gives, when the failure ends the run.
        log_tail: Option<LogTail>,
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
    /// The end of a log, when what wrote it is the reason.
    pub(crate) log_tail: Option<LogTail>,
}

/// The end of a log a hand-back gives, and what it is the end of. Its text
/// is never empty.
pub(crate) struct LogTail {
    pub(crate) of: Logged,
    pub(crate) text: String,
}

/// What a log tail is the end of.
pub(crate) enum Logged {
    Session,
    Setup,
}

impl LogTail {
    /// The end of the session's log `text`, unless it holds nothing.
    fn session(text: &str) -> Option<LogTail> {
        let text = text.trim();
        (!text.is_empty()).then(|| LogTail {
            of: Logged::Session,
            text: text.to_string(),
        })
    }

    /// The end of the setup command's output `text`, unless it holds
    /// nothing.
    fn setup(text: &str) -> Option<LogTail> {
        let text = text.trim();
        (!text.is_empty()).then(|| LogTail {
            of: Logged::Setup,
            text: text.to_string(),
        })
    }

    /// What it is the end of, as a hand-back heads it.
    fn heading(&self) -> &'static str {
        match self.of {
            Logged::Session => "The end of the session's log",
            Logged::Setup => "The end of the setup command's output",
        }
    }
}

/// What a run works with, from its start to its end.
pub(crate) struct Ctx {
    pub(crate) with: RunWith,
    /// The run takes feedback since this as having arrived after.
    pub(crate) as_of: Timestamp,
    pub(crate) reworked: Option<Timestamp>,
    pub(crate) cancel: Cancellation,
    pub(crate) run: Running,
}

/// A run's cancellation, which only a run in session takes: a cancelled run
/// posts nothing more (docs/specs/runs.md, "Cancelling"), and one that has
/// begun applying its result applies all of it, unless none of it was applied
/// and it goes back to its session.
#[derive(Clone, Default)]
pub(crate) struct Cancellation(Arc<Cancel>);

#[derive(Default)]
struct Cancel {
    /// Fired once the phase is cancelled, for what the run awaits.
    token: CancellationToken,
    phase: Mutex<Phase>,
}

#[derive(Clone, Copy, Default)]
enum Phase {
    #[default]
    InSession,
    /// Applying its result; `stop` holds a cancel that came meanwhile.
    Applying {
        stop: bool,
    },
    Cancelled,
}

impl Cancellation {
    /// Cancels the run unless it is applying its result, and returns whether
    /// the run is cancelled. A cancel while applying is kept for
    /// [`Self::back_to_session`].
    pub(crate) fn cancel(&self) -> bool {
        let mut phase = self.phase();
        match *phase {
            Phase::InSession => {
                *phase = Phase::Cancelled;
                self.0.token.cancel();
                true
            }
            Phase::Applying { .. } => {
                *phase = Phase::Applying { stop: true };
                false
            }
            Phase::Cancelled => true,
        }
    }

    /// Begins applying the result, unless the run was cancelled first.
    fn begin_applying(&self) -> Result<(), Failure> {
        let mut phase = self.phase();
        match *phase {
            Phase::InSession => {
                *phase = Phase::Applying { stop: false };
                Ok(())
            }
            Phase::Applying { .. } => Ok(()),
            Phase::Cancelled => Err(Failure::Cancelled),
        }
    }

    /// Returns to the session, nothing of the result applied: the run is
    /// cancelled then if a cancel came while it was applying.
    fn back_to_session(&self) -> Result<(), Failure> {
        let mut phase = self.phase();
        match *phase {
            Phase::InSession | Phase::Applying { stop: false } => {
                *phase = Phase::InSession;
                Ok(())
            }
            Phase::Applying { stop: true } | Phase::Cancelled => {
                *phase = Phase::Cancelled;
                self.0.token.cancel();
                Err(Failure::Cancelled)
            }
        }
    }

    async fn cancelled(&self) {
        self.0.token.cancelled().await;
    }

    fn phase(&self) -> std::sync::MutexGuard<'_, Phase> {
        // A phase is set whole, so one a panic left is still sound.
        self.0
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Failure {
    /// Adds `note` to what the failure's hand-back says. A cancelled run and
    /// one that ends for the usage limit post nothing, so they take none.
    fn noted(self, note: &str) -> Failure {
        match self {
            Failure::Infrastructure {
                error,
                note: before,
                log_tail,
            } => Failure::Infrastructure {
                error,
                note: before + note,
                log_tail,
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
        log_tail: None,
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

/// Pushes a session's finished result to `branch`, as the first write of
/// applying it. A push GitHub's push protection refuses over a secret sends
/// the result back to its session, and one GitHub refuses for a missing
/// permission is handed back with how to `again`.
async fn push_result(
    shell: &Shell,
    ctx: &Ctx,
    dir: &Path,
    branch: &str,
    mode: PushMode,
    again: TryAgain,
) -> Result<(), Rejection> {
    match push(shell, dir, branch, mode).await {
        Pushed::Protected(secrets) => Err(Rejection::Retry {
            followup: prompts::pushed_secret(&ctx.with.briefs.prompts, &secrets),
            note: Some(protected_note(&secrets)),
        }),
        other @ (Pushed::Done | Pushed::Refused(_) | Pushed::Failed(_)) => {
            pushed(branch, other, again).map_err(Rejection::Fail)
        }
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
                // The retry is a new session, so a cancel can stop it again,
                // and one that came while the attempt applied its result
                // stops it now.
                if ctx.cancel.back_to_session().is_err() {
                    info!(subject = ?subject, error, "run cancelled; not retried");
                    return Ending::Cancelled;
                }
                warn!(subject = ?subject, error, "infrastructure failure; retrying once");
                retried = true;
            }
            Err(Failure::Infrastructure {
                error: e,
                note,
                log_tail,
            }) => {
                break HandBack {
                    kind: HandBackKind::Stall,
                    reason: format!("The run failed twice on its infrastructure: {e:#}{note}"),
                    log_tail,
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
    if ctx.cancel.begin_applying().is_err() {
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
        log_tail: hand_back.log_tail.map(|tail| LogTail {
            text: posted(&tail.text),
            ..tail
        }),
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
                .merged_pull(spec_change, Windows::EVER)
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

/// The writes that change what an item asks for, which a run makes last
/// (docs/specs/repository.md, Writes, "Order"), made by [`ask`].
#[derive(Default)]
pub(crate) struct Asks {
    pub designed: Option<Designed>,
    /// Label the item [`label::STALLED`].
    pub stall: bool,
    pub status: Option<StatusTo>,
    /// Unassign Linewright from the item.
    pub unassign: bool,
}

/// Whether the item is to carry [`label::DESIGNED`].
pub(crate) enum Designed {
    Add,
    Remove,
}

pub(crate) enum StatusTo {
    /// Hold this status label and no other.
    Set(StatusLabel),
    /// Hold no status label, as a tracking issue does.
    Clear,
}

/// Makes `asks`' writes on `number` in the order docs/specs/repository.md,
/// Writes, "Order", asks for: `linewright:designed`, the stall, the
/// new status label, the removal of the others, the unassignment.
pub(crate) async fn ask(store: &dyn Store, number: Number, asks: &Asks) -> Result<()> {
    match asks.designed {
        Some(Designed::Add) => store.add_label(number, label::DESIGNED).await?,
        Some(Designed::Remove) => store.remove_label(number, label::DESIGNED).await?,
        None => {}
    }
    if asks.stall {
        store.add_label(number, label::STALLED).await?;
    }
    match asks.status {
        Some(StatusTo::Set(status)) => {
            store.add_label(number, status.label()).await?;
            for other in StatusLabel::ALL.into_iter().filter(|s| *s != status) {
                store.remove_label(number, other.label()).await?;
            }
        }
        Some(StatusTo::Clear) => {
            for status in StatusLabel::ALL {
                store.remove_label(number, status.label()).await?;
            }
        }
        None => {}
    }
    if asks.unassign {
        store.unassign(number).await?;
    }
    Ok(())
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
    let n = issue.number;
    shell.store.comment(n, &with_log(hand_back)).await?;
    let asks = Asks {
        designed: (!issue.was_cut(&shell.policy.linewright)).then_some(Designed::Remove),
        stall: stalls(hand_back),
        status: Some(StatusTo::Set(StatusLabel::ReadyForHuman)),
        unassign: true,
    };
    ask(&*shell.store, n, &asks).await
}

async fn hand_back_pull(shell: &Shell, pr: Number, hand_back: &HandBack) -> Result<()> {
    hand_back_pr(shell, pr, hand_back, "try again", None).await
}

async fn hand_back_spec_change(shell: &Shell, pr: Number, hand_back: &HandBack) -> Result<()> {
    hand_back_pr(
        shell,
        pr,
        hand_back,
        "design it again",
        Some(Designed::Remove),
    )
    .await
}

/// Hands back the pull request `pr`, telling whoever reads it that
/// assigning Linewright again will `try_again`.
async fn hand_back_pr(
    shell: &Shell,
    pr: Number,
    hand_back: &HandBack,
    try_again: &str,
    designed: Option<Designed>,
) -> Result<()> {
    let body = format!(
        "{}\n\nAssign @{} to this pull request again to {try_again}.",
        with_log(hand_back),
        shell.policy.linewright
    );
    shell.store.comment(pr, &body).await?;
    let asks = Asks {
        designed,
        stall: stalls(hand_back),
        unassign: true,
        ..Asks::default()
    };
    ask(&*shell.store, pr, &asks).await
}

/// Whether `hand_back` labels its item a stall. The label goes on while
/// Linewright is still assigned: an item left so by a stop asks for the
/// work it stalled on, and the stall is cleared as it is worked again,
/// unless the item is let go of instead (docs/specs/repository.md,
/// "Stalls").
fn stalls(hand_back: &HandBack) -> bool {
    match hand_back.kind {
        HandBackKind::Stall => true,
        HandBackKind::Decision => false,
    }
}

fn with_log(hand_back: &HandBack) -> String {
    let reason = &hand_back.reason;
    match &hand_back.log_tail {
        Some(tail) => format!(
            "{reason}\n\n<details><summary>{}</summary>\n\n```\n{}\n```\n\n</details>",
            tail.heading(),
            tail.text
        ),
        None => reason.clone(),
    }
}

/// A run's clone, removed when dropped: the run is done with it then.
struct Workspace {
    /// What the setup command left running, ended before the clone is
    /// removed.
    leftovers: Option<setup::Leftovers>,
    /// `None` once dropped.
    temp: Option<TempDir>,
    dir: PathBuf,
    scratch: PathBuf,
    plugin: PathBuf,
}

impl Workspace {
    async fn clone(shell: &Shell, purpose: &Purpose, branch: &str) -> Result<Workspace, Failure> {
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
        // The clone is removed when the run ends, so the maintenance git
        // starts after a commit, a merge or a fetch would only cost time.
        let config = [
            ("user.name", shell.identity.name.as_str()),
            ("user.email", shell.identity.email.as_str()),
            ("maintenance.auto", "false"),
        ];
        shell
            .store
            .clone_repo(&dir, branch, &config)
            .await
            .map_err(infra)?;
        let scratch = temp.path().join("tmp");
        tokio::fs::create_dir(&scratch)
            .await
            .context("creating a scratch directory")
            .map_err(infra)?;
        let plugin = temp.path().join("plugin");
        Briefs::write_plugin(&plugin)
            .context("writing Linewright's plugin")
            .map_err(infra)?;
        Ok(Workspace {
            leftovers: None,
            temp: Some(temp),
            dir,
            scratch,
            plugin,
        })
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let temp = self.temp.take();
        let Some(leftovers) = self.leftovers.take() else {
            return;
        };
        // Ending them waits out a grace, which the run's task does not.
        let end = move || {
            leftovers.end();
            drop(temp);
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(end)),
            Err(_) => end(),
        }
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

/// Why a result goes back to its session.
enum Rejection {
    /// Sent back with `followup`, counted against `result-retries`; `note`
    /// is what the hand-back adds once they run out.
    Retry {
        followup: String,
        note: Option<String>,
    },
    /// Sent back with `followup` as a new question, the count reset: the
    /// answer to new feedback is a new result.
    Resume(String),
    Fail(Failure),
}

impl Rejection {
    /// Sent back with `followup`, the hand-back adding nothing.
    fn retry(followup: String) -> Rejection {
        Rejection::Retry {
            followup,
            note: None,
        }
    }
}

/// Why a result's first writes refused it.
enum Refused {
    /// Before they wrote to the session's work, which is as it left it.
    Untouched(Rejection),
    /// As they wrote the work, whose failure says what became of it.
    Writing(Rejection),
}

impl From<anyhow::Error> for Refused {
    fn from(error: anyhow::Error) -> Refused {
        Refused::Untouched(error.into())
    }
}

/// How a conversation ended before its result's first writes were done.
enum Unbegun {
    /// In session, or sent back to it: the stage's work is as the session
    /// left it.
    InSession(Failure),
    /// In the result's first writes, whose failure says what became of the
    /// work.
    Begun(Failure),
}

impl Unbegun {
    /// The failure, noted with what `keep` keeps of the work the session
    /// left.
    async fn keeping(self, keep: impl AsyncFnOnce() -> String) -> Failure {
        match self {
            Unbegun::InSession(failure) => failure.noted(&keep().await),
            Unbegun::Begun(failure) => failure,
        }
    }
}

impl From<Unbegun> for Failure {
    fn from(unbegun: Unbegun) -> Failure {
        match unbegun {
            Unbegun::InSession(failure) | Unbegun::Begun(failure) => failure,
        }
    }
}

impl From<anyhow::Error> for Rejection {
    fn from(error: anyhow::Error) -> Rejection {
        Rejection::Fail(infra(error))
    }
}

/// Sends a result that does not parse back to the session with its `error`.
fn parse_retry(ctx: &Ctx) -> impl Fn(String) -> Rejection + '_ {
    |error| Rejection::retry(prompts::result_error(&ctx.with.briefs.prompts, &error))
}

/// Runs a session until `accept` takes its result, in session, and `begin`
/// has made the result's first writes, once applying it has begun. A result
/// either rejects goes back to the session.
async fn converse<T, U, F, Fut, B, BFut>(
    shell: &Shell,
    ctx: &Ctx,
    spec: &mut SessionSpec,
    first: Step,
    accept: F,
    begin: B,
) -> Result<(U, Turn), Unbegun>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T, Rejection>>,
    B: Fn(T) -> BFut,
    BFut: Future<Output = Result<U, Refused>>,
{
    converse_counting(shell, ctx, spec, first, &mut 0, accept, begin).await
}

/// As [`converse`], counting the follow-ups the result is sent back with in
/// `retries`, which holds those it was sent back with before.
async fn converse_counting<T, U, F, Fut, B, BFut>(
    shell: &Shell,
    ctx: &Ctx,
    spec: &mut SessionSpec,
    first: Step,
    retries: &mut u32,
    accept: F,
    begin: B,
) -> Result<(U, Turn), Unbegun>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T, Rejection>>,
    B: Fn(T) -> BFut,
    BFut: Future<Output = Result<U, Refused>>,
{
    let in_session = Unbegun::InSession;
    let mut turn = take_turn(shell, ctx, spec, first)
        .await
        .map_err(in_session)?;
    loop {
        let refused = match accept(turn.message.clone()).await {
            Ok(result) => {
                let json = results::json_value(&turn.message)
                    .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
                    .unwrap_or_else(|| turn.message.clone());
                record(spec, "result.json", &json);
                // The session may end as the run is cancelled, too late to
                // stop it.
                ctx.cancel.begin_applying().map_err(in_session)?;
                match begin(result).await {
                    Ok(written) => return Ok((written, turn)),
                    Err(Refused::Writing(Rejection::Fail(failure))) => {
                        return Err(Unbegun::Begun(failure));
                    }
                    Err(refused) => {
                        // A cancel that came while applying cancels the run
                        // now, rather than send the result back or end it
                        // otherwise.
                        ctx.cancel.back_to_session().map_err(in_session)?;
                        refused
                    }
                }
            }
            Err(rejection) => Refused::Untouched(rejection),
        };
        // What the first writes refused as they wrote the work, they would
        // refuse again of the work as the session left it.
        let (rejection, unbegun): (_, fn(Failure) -> Unbegun) = match refused {
            Refused::Untouched(rejection) => (rejection, Unbegun::InSession),
            Refused::Writing(rejection) => (rejection, Unbegun::Begun),
        };
        let step = match rejection {
            Rejection::Fail(failure) => return Err(unbegun(failure)),
            Rejection::Retry { followup, note } => send_back(shell, spec, &turn, followup, retries)
                .map_err(|failure| unbegun(failure.noted(note.as_deref().unwrap_or_default())))?,
            Rejection::Resume(followup) => {
                *retries = 0;
                Step::Resume {
                    session: turn.session.clone(),
                    prompt: followup,
                }
            }
        };
        turn = take_turn(shell, ctx, spec, step)
            .await
            .map_err(in_session)?;
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
        log_tail: LogTail::session(&tail(&scrub(&turn.message, &spec.dir), 40)),
    }))
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
            Err(SessionError::TimedOut { log_tail }) => {
                return Err(timed_out(
                    &ctx.with.settings,
                    LogTail::session(&scrub(&log_tail, &spec.dir)),
                ));
            }
            Err(SessionError::Failed { reason, log_tail }) => {
                return Err(Failure::HandBack(HandBack {
                    kind: HandBackKind::Stall,
                    reason: format!("The session ended unexpectedly: {reason}"),
                    log_tail: LogTail::session(&scrub(&log_tail, &spec.dir)),
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

/// The hand-back of a session that timed out, with `log_tail` as its log
/// (docs/specs/runs.md, "Limits", Session timeout): it is not retried.
fn timed_out(settings: &SessionSettings, log_tail: Option<LogTail>) -> Failure {
    Failure::HandBack(HandBack {
        kind: HandBackKind::Stall,
        reason: format!(
            "The session ended unexpectedly: {}. The work may need cutting smaller, or {} \
             may need to be longer.",
            harness::TIMED_OUT,
            settings.timeout_setting
        ),
        log_tail,
    })
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

/// A session opened in its clone, whose prompt is still to be filled.
struct Opened<'a> {
    settings: &'a SessionSettings,
    purpose: Purpose,
    record: PathBuf,
    attachments: Dir,
    recorder: SessionRecorder,
    dir: PathBuf,
    scratch: PathBuf,
    plugin: PathBuf,
    /// When the session times out, its setup command included.
    deadline: tokio::time::Instant,
}

/// Opens the session of `purpose` in `ws`: makes its record and its
/// attachments directory, registers it with the run, sets its deadline and
/// runs its setup command (docs/specs/runs.md, "Setup"), so that the
/// command's output is under the session whether or not the agent starts,
/// and its time counts against the session's timeout.
async fn open_session<'a>(
    shell: &Shell,
    ctx: &'a Ctx,
    purpose: Purpose,
    ws: &mut Workspace,
) -> Result<Opened<'a>, Failure> {
    let settings = &ctx.with.settings;
    let record = record_dir(shell, &purpose, shell.clock.now())?;
    let attachments = Dir::create(&record)
        .context("creating the attachments directory")
        .map_err(infra)?;
    let recorder = ctx.run.session(&record, settings, &ctx.with.briefs);
    let deadline = tokio::time::Instant::now() + settings.session_timeout.value;
    if let Setup::Command(command) = &settings.setup.value {
        set_up(shell, ctx, command, &record, deadline, ws).await?;
    }
    Ok(Opened {
        settings,
        purpose,
        record,
        attachments,
        recorder,
        dir: ws.dir.clone(),
        scratch: ws.scratch.clone(),
        plugin: ws.plugin.clone(),
        deadline,
    })
}

/// Runs the setup `command` in `ws`, which keeps what it leaves running,
/// until the session's `deadline`. It fails as the session would end.
async fn set_up(
    shell: &Shell,
    ctx: &Ctx,
    command: &SetupCommand,
    record: &Path,
    deadline: tokio::time::Instant,
    ws: &mut Workspace,
) -> Result<(), Failure> {
    let settings = &ctx.with.settings;
    let request = setup::Request {
        command,
        dir: &ws.dir,
        environment: shell.harness.environment(
            &settings.environment,
            &ws.scratch,
            settings.effort_set(),
        ),
        deadline,
        started_at: shell.clock.now(),
        record,
    };
    let (ran, leftovers) = setup::run(
        &shell.bash,
        request,
        shell.secrets.redaction(),
        ctx.cancel.cancelled(),
    )
    .await
    .context("running the setup command")
    .map_err(infra)?;
    ws.leftovers = Some(leftovers);
    set_up_failure(&ran, &ws.dir, settings).map_or(Ok(()), Err)
}

/// Why the session ends after the setup command ran in `dir` as `ran`, if
/// it does (docs/specs/runs.md, "Setup", Failing): a command still running
/// at the session's deadline is the session timing out.
fn set_up_failure(ran: &setup::Ran, dir: &Path, settings: &SessionSettings) -> Option<Failure> {
    let log_tail = || LogTail::setup(&scrub(&tail(&ran.output, 40), dir));
    let failed = match ran.status {
        setup::Status::Exited(0) => return None,
        setup::Status::Exited(code) => format!("exited with status {code}"),
        setup::Status::Signalled(signal) => format!("was ended by signal {signal}"),
        setup::Status::TimedOut => return Some(timed_out(settings, log_tail())),
        setup::Status::Cancelled => return Some(Failure::Cancelled),
    };
    Some(Failure::Infrastructure {
        error: anyhow::anyhow!("the setup command {failed}"),
        note: String::new(),
        log_tail: log_tail(),
    })
}

impl Opened<'_> {
    /// The session, sent the `prompt` made for its attachments directory,
    /// and that directory.
    fn spec(self, prompt: impl FnOnce(&Path) -> String) -> (SessionSpec, Dir) {
        let Opened {
            settings,
            purpose,
            record,
            attachments,
            recorder,
            dir,
            scratch,
            plugin,
            deadline,
        } = self;
        let spec = SessionSpec {
            record,
            attachments: attachments.path().to_path_buf(),
            prompt: prompt(attachments.path()),
            purpose,
            dir,
            scratch,
            plugin,
            model: settings.model_set(),
            effort: settings.effort_set(),
            environment: settings.environment.clone(),
            deadline,
            report: Reporter::new(move |reported| recorder.reported(reported)),
        };
        (spec, attachments)
    }
}

/// Uploads each attachment `result`'s posted Markdown refers to in `dir`
/// and puts its URL in place of its path, as the first write of applying
/// it (docs/specs/runs.md, "Attachments"). A reference or a file the
/// session can fix sends the result back to it.
async fn post_attachments(
    shell: &Shell,
    ctx: &Ctx,
    dir: &Dir,
    result: &mut impl Posts,
) -> Result<(), Refused> {
    let prompts = &ctx.with.briefs.prompts;
    let sent_back = |followup| Err(Refused::Untouched(Rejection::retry(followup)));
    let resolved = {
        let markdown = result.markdown();
        let texts: Vec<&str> = markdown.iter().map(|text| text.as_str()).collect();
        attachments::resolve(dir, &texts, &shell.secrets)
    };
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(Refusal::Secret { variable, path }) => {
            return sent_back(prompts::attachment_secret(prompts, &variable, &path));
        }
        Err(refusal @ (Refusal::NotAFile { .. } | Refusal::Outside { .. })) => {
            return sent_back(prompts::result_error(prompts, &refusal.to_string()));
        }
    };
    let mut urls = BTreeMap::new();
    for file in resolved.files() {
        let attached = shell
            .store
            .attach(file.path(), file.name(), file.content_type())
            .await;
        let failure = match attached {
            Ok(url) => {
                urls.insert(file.path().to_path_buf(), url);
                continue;
            }
            Err(AttachError::RefusedFile { reason }) => {
                return sent_back(prompts::attachment_refused(prompts, file.path(), &reason));
            }
            Err(AttachError::RefusedToken { reason }) => Failure::HandBack(HandBack {
                kind: HandBackKind::Stall,
                reason: format!(
                    "GitHub refused Linewright's token the upload of the attachment `{}`. \
                     GitHub's reason: {reason}\n\nLinewright's token needs write access to \
                     the repository, in a classic or fine-grained personal access token, to \
                     attach files to what it posts.",
                    file.name()
                ),
                log_tail: None,
            }),
            Err(AttachError::Failed(e)) => infra(e),
        };
        return Err(Refused::Untouched(Rejection::Fail(failure)));
    }
    resolved.rewrite(&mut result.markdown(), &urls);
    Ok(())
}

/// Drops from `result`'s spec gaps those already filed and still open,
/// which are not posted again (docs/specs/runs.md, "Spec gaps").
fn drop_filed_gaps(result: &mut impl SpecGaps, filed: &FiledGaps<'_>) {
    let Some(gaps) = result.spec_gaps_mut() else {
        return;
    };
    gaps.retain(|gap| match spec_gap::filed_as(filed, &gap.title) {
        Some(issue) => {
            info!(
                issue = issue.number.0,
                item = filed.item.0,
                "spec gap already filed"
            );
            false
        }
        None => true,
    });
}

async fn triage(shell: &Shell, ctx: &Ctx, issue: &Issue) -> Result<(), Failure> {
    let purpose = Purpose::Work {
        subject: Subject::Issue(issue.number),
        kind: WorkKind::Triage,
    };
    let mut ws = Workspace::clone(shell, &purpose, &shell.settings.default_branch).await?;
    let categories = shell.settings.categories.names();
    let opened = open_session(shell, ctx, purpose, &mut ws).await?;
    let (mut spec, dir) = opened.spec(|attachments| {
        prompts::triage(
            &ctx.with.briefs.prompts,
            &shell.repo(),
            issue,
            &categories,
            attachments,
        )
    });
    let (categories, dir) = (&categories, &dir);
    let (result, _) =
        converse(
            shell,
            ctx,
            &mut spec,
            Step::Start,
            |message| async move {
                results::parse_triage(&message, categories).map_err(parse_retry(ctx))
            },
            |mut result: TriageResult| async move {
                post_attachments(shell, ctx, dir, &mut result).await?;
                Ok(result)
            },
        )
        .await?;
    record_triage(shell, issue, result).await.map_err(apply)
}

async fn record_triage(shell: &Shell, issue: &Issue, result: TriageResult) -> Result<()> {
    let store = &shell.store;
    let (state, body) = match &result.outcome {
        TriageOutcome::ReadyForAgent => (StatusLabel::ReadyForAgent, result.comment),
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
            (
                StatusLabel::NeedsInfo,
                addressed_to(&people, &result.comment),
            )
        }
        TriageOutcome::ReadyForHuman => (
            StatusLabel::ReadyForHuman,
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
    let asks = Asks {
        status: Some(StatusTo::Set(state)),
        unassign: !result.outcome.asks_for_linewright(),
        ..Asks::default()
    };
    ask(&**store, issue.number, &asks).await
}

/// Runs a design session on `subject`, sent `prompt` made for its
/// attachments directory, and begins applying its result, the gaps in
/// `filed_gaps` dropped from it.
async fn design_session<C>(
    shell: &Shell,
    ctx: &Ctx,
    subject: Subject,
    filed_gaps: &FiledGaps<'_>,
    prompt: impl FnOnce(&Path) -> String,
) -> Result<(DesignResult<C>, Turn), Failure>
where
    DesignResult<C>: Contract + Posts + SpecGaps,
{
    let purpose = Purpose::Work {
        subject,
        kind: WorkKind::Design,
    };
    let mut ws = Workspace::clone(shell, &purpose, &shell.settings.default_branch).await?;
    let opened = open_session(shell, ctx, purpose, &mut ws).await?;
    let (mut spec, dir) = opened.spec(prompt);
    let dir = &dir;
    let begin = |mut result: DesignResult<C>| async move {
        drop_filed_gaps(&mut result, filed_gaps);
        post_attachments(shell, ctx, dir, &mut result).await?;
        Ok(result)
    };
    converse(
        shell,
        ctx,
        &mut spec,
        Step::Start,
        |message| async move {
            results::parse::<DesignResult<C>>(&message).map_err(parse_retry(ctx))
        },
        begin,
    )
    .await
    .map_err(Failure::from)
}

/// A design blocked on `question` files its spec gaps and hands its item
/// back.
async fn design_blocked(
    shell: &Shell,
    question: &str,
    spec_gaps: &[SpecGap],
    item: Number,
) -> Result<(), Failure> {
    file_spec_gaps(shell, spec_gaps, item)
        .await
        .map_err(apply)?;
    Err(Failure::HandBack(HandBack {
        kind: HandBackKind::Decision,
        reason: blocked_on(question),
        log_tail: None,
    }))
}

fn design_gave_up(of: DesignOf, reason: &str, turn: &Turn) -> Failure {
    Failure::HandBack(HandBack {
        kind: HandBackKind::Decision,
        reason: format!("The designer handed the {} back:\n\n{reason}", of.noun()),
        log_tail: LogTail::session(&turn.log_tail),
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
    let prompt = |attachments: &Path| {
        prompts::design_spec_change(
            &ctx.with.briefs.prompts,
            &shell.repo(),
            pr,
            &diff,
            &filed_issues,
            attachments,
        )
    };
    let subject = Subject::SpecChange(pr.number);
    let (result, turn) = design_session::<Cut>(shell, ctx, subject, &filed_gaps, prompt).await?;
    match result {
        DesignResult::Designed {
            design,
            cut: Cut { tickets, comment },
            spec_gaps,
        } => {
            file_tickets(shell, pr, &design, &tickets, &comment)
                .await
                .map_err(apply)?;
            file_spec_gaps(shell, &spec_gaps, filed_gaps.item)
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
        } => design_blocked(shell, &question, &spec_gaps, filed_gaps.item).await,
        DesignResult::GaveUp { reason } => {
            Err(design_gave_up(DesignOf::SpecChange, &reason, &turn))
        }
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
    let prompt = |attachments: &Path| {
        prompts::design_issue(
            &ctx.with.briefs.prompts,
            &shell.repo(),
            issue,
            &filed_issues,
            attachments,
        )
    };
    let subject = Subject::Issue(issue.number);
    let (result, turn) =
        design_session::<IssueWork>(shell, ctx, subject, &filed_gaps, prompt).await?;
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
            file_spec_gaps(shell, &spec_gaps, filed_gaps.item)
                .await
                .map_err(apply)?;
            let asks = match cut {
                IssueWork::AsItIs => Asks {
                    designed: Some(Designed::Add),
                    ..Asks::default()
                },
                // A tracking issue carries no status label, and is worked
                // through its tickets.
                IssueWork::Cut(_) => Asks {
                    designed: Some(Designed::Add),
                    status: Some(StatusTo::Clear),
                    unassign: true,
                    ..Asks::default()
                },
            };
            ask(&**store, n, &asks).await.map_err(apply)
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
        } => design_blocked(shell, &question, &spec_gaps, filed_gaps.item).await,
        DesignResult::GaveUp { reason } => Err(design_gave_up(DesignOf::Issue, &reason, &turn)),
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
    let asks = Asks {
        designed: Some(Designed::Add),
        unassign: true,
        ..Asks::default()
    };
    ask(&*shell.store, pr.number, &asks).await
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
    let asks = Asks {
        designed: Some(Designed::Add),
        status: Some(StatusTo::Set(StatusLabel::ReadyForAgent)),
        ..Asks::default()
    };
    for ticket in tickets {
        ask(&**store, ticket.number, &asks).await?;
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
/// `past` add: docs/specs/runs.md, "A result Linewright can't accept".
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
    let mut ws = Workspace::clone(shell, &purpose, &shell.settings.default_branch).await?;
    // Where the run starts: docs/specs/implementation.md, "Working a
    // ticket". The clone, not the snapshot, says whether the branch exists: a
    // snapshot taken before an earlier run pushed it does not hold it.
    let upstream = shell.upstream();
    let start = git::check_out_branch(&ws.dir, &branch, &upstream)
        .await
        .map_err(infra)?;
    let opened = open_session(shell, ctx, purpose, &mut ws).await?;
    let (mut spec, attachments) = opened.spec(|attachments| {
        prompts::implement(
            &ctx.with.briefs.prompts,
            &shell.repo(),
            ticket,
            &branch,
            &start,
            attachments,
        )
    });
    let (dir, base) = (&ws.dir, &upstream);
    let accept = |message: String| async move {
        let result: ImplementResult = results::parse(&message).map_err(parse_retry(ctx))?;
        if !matches!(result, ImplementResult::Done { .. }) {
            return Ok(result);
        }
        if let Some(problem) = git::loose_ends(dir).await? {
            return Err(Rejection::retry(prompts::loose_ends(
                &ctx.with.briefs.prompts,
                &problem,
            )));
        }
        if git::commits_ahead(dir, base).await? == 0 {
            return Err(Rejection::retry(prompts::loose_ends(
                &ctx.with.briefs.prompts,
                "you answered `done`, but the branch has no commits. Commit your work, or answer `gave-up` with the reason if the ticket needs no change.",
            )));
        }
        let unfinished = git::unfinished_commits(dir, base, "HEAD").await?;
        if let Some(unfinished) = unfinished {
            return Err(Rejection::retry(prompts::fold_unfinished(
                &ctx.with.briefs.prompts,
                &unfinished,
            )));
        }
        if let Some(held) = held_secret(shell, dir, &[base]).await? {
            return Err(Rejection::retry(prompts::held_secret(
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
    // A finished result is pushed as it begins applying, after its
    // attachments, so that one GitHub refuses over a secret goes back to its
    // session.
    let (filed_gaps, attachments) = (&ticket.filed_gaps(), &attachments);
    let begin = |mut result: ImplementResult| async {
        drop_filed_gaps(&mut result, filed_gaps);
        post_attachments(shell, ctx, attachments, &mut result).await?;
        if !matches!(result, ImplementResult::Done { .. }) {
            return Ok(result);
        }
        let pushing = push_result(
            shell,
            ctx,
            &ws.dir,
            &branch,
            PushMode::Replace,
            TryAgain::Relabel,
        );
        pushing.await.map_err(Refused::Writing)?;
        Ok(result)
    };
    let (result, turn) = match converse(shell, ctx, &mut spec, Step::Start, accept, begin).await {
        Ok(conversation) => conversation,
        Err(unbegun) => return Err(unbegun.keeping(keep).await),
    };
    let store = &shell.store;
    match result {
        ImplementResult::Done {
            pr_title,
            pr_body,
            spec_gaps,
        } => {
            // Opening the pull request puts the ticket in review, which
            // changes what it asks for, so the gaps go first
            // (docs/specs/repository.md, Writes, "Order").
            file_spec_gaps(shell, &spec_gaps, ticket.number())
                .await
                .map_err(apply)?;
            let pr = store
                .open_pull(NewPull {
                    head: branch.clone(),
                    title: pr_title,
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
            Ok(())
        }
        ImplementResult::Blocked {
            question,
            spec_gaps,
        } => {
            let kept = keep().await;
            file_spec_gaps(shell, &spec_gaps, ticket.number())
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
                log_tail: LogTail::session(&turn.log_tail),
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

/// Files `gaps` for `item`, those already filed dropped from them by
/// [`drop_filed_gaps`].
async fn file_spec_gaps(shell: &Shell, gaps: &[SpecGap], item: Number) -> Result<()> {
    for gap in gaps {
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
    ctx.cancel.begin_applying()?;
    let outcome = push(shell, dir, &pr.head_ref, lease).await;
    pushed(&pr.head_ref, outcome, TryAgain::Reassign)
}

/// Brings `pr`'s branch, checked out in `dir`, up to date with the default
/// branch, and returns the conflict it stopped on, if any.
async fn update_branch(
    shell: &Shell,
    pr: &Pull,
    dir: &Path,
) -> Result<Option<git::Conflict>, Failure> {
    let default_branch = &shell.settings.default_branch;
    let policy = &shell.policy;
    let updated = if rebases_to_repair(pr, policy) {
        git::rebase(dir, default_branch).await
    } else {
        git::update(dir, default_branch, policy.update_rule).await
    };
    Ok(match updated.map_err(infra)? {
        Updated::Conflict(conflict) => Some(conflict),
        Updated::UpToDate | Updated::Updated => None,
    })
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
    let mut ws = Workspace::clone(shell, &purpose, &pr.head_ref).await?;
    let start = git::head(&ws.dir).await.map_err(infra)?;
    // The setup command runs on the branch as it was cloned, before the
    // update (docs/specs/runs.md, "Setup"). An update run starts a session
    // only on a conflict, so it updates first, and on a conflict aborts the
    // update to run the command, then updates again.
    let opened = match task {
        Task::Update => match update_branch(shell, pr, &ws.dir).await? {
            None => return push_update(shell, ctx, pr, &ws.dir, &start).await,
            Some(conflict) => {
                git::abort_update(&ws.dir, &conflict).await.map_err(infra)?;
                open_session(shell, ctx, purpose, &mut ws).await?
            }
        },
        Task::Rework | Task::FixCi(_) => open_session(shell, ctx, purpose, &mut ws).await?,
    };
    let conflict = update_branch(shell, pr, &ws.dir).await?;
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
    let briefed_authors = feedback_people(&feedback, linewright);
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
    let (mut spec, attachments) = opened.spec(|attachments| {
        prompts::pull_work(
            &ctx.with.briefs.prompts,
            &shell.repo(),
            ticket,
            pr,
            &pull_task,
            left_unfinished.as_ref(),
            attachments,
        )
    });
    let before = git::work_state(dir).await.map_err(infra)?;
    let (upstream, base) = (&upstream, start.as_str());
    let check_result = |message: String, pr_threads: Vec<String>| async move {
        let result: PullWorkResult = results::parse(&message).map_err(parse_retry(ctx))?;
        let PullWorkResult::Done { replies, .. } = &result else {
            return Ok(result);
        };
        if let Some(problem) = git::loose_ends(dir).await? {
            return Err(Rejection::retry(prompts::loose_ends(
                &ctx.with.briefs.prompts,
                &problem,
            )));
        }
        if let Some(unfinished) = git::unfinished_commits(dir, upstream, "HEAD").await? {
            return Err(Rejection::retry(prompts::fold_unfinished(
                &ctx.with.briefs.prompts,
                &unfinished,
            )));
        }
        if let Some(held) = held_secret(shell, dir, &[base, upstream]).await? {
            return Err(Rejection::retry(prompts::held_secret(
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
    let shown = Mutex::new(Shown {
        until: ctx.as_of,
        threads: pr.threads.iter().map(|t| t.id.clone()).collect(),
        feedback_authors: briefed_authors,
    });
    let (check_result, shown) = (&check_result, &shown);
    let accept = |message: String| async move {
        let threads = lock(shown).threads.clone();
        let result = check_result(message, threads).await?;
        let read_at = shell.clock.now();
        let pr_now = store.pull(pr.number).await?;
        if stopped(&pr_now, linewright).is_some() {
            return Err(Rejection::Fail(Failure::Cancelled));
        }
        if !matches!(result, PullWorkResult::Done { .. }) {
            return Ok((result, pr_now, read_at));
        }
        // Feedback posted after the snapshot the brief came from would be
        // older than the push that follows, so it goes to the session before
        // anything is pushed, for as long as it keeps coming within the
        // session's time.
        let mut shown = lock(shown);
        let arrived = late(
            &new_feedback(&pr_now, linewright, ctx.reworked),
            shown.until,
        );
        if arrived.is_empty() {
            return Ok((result, pr_now, read_at));
        }
        shown.until = read_at;
        shown
            .feedback_authors
            .extend(feedback_people(&arrived, linewright));
        shown.threads = pr_now.threads.iter().map(|t| t.id.clone()).collect();
        Err(Rejection::Resume(prompts::late_feedback(
            &ctx.with.briefs.prompts,
            &pr_now,
            &arrived,
        )))
    };
    // A finished result is pushed as it begins applying, after its
    // attachments, as for an implement run.
    let (start, lease) = (&start, &lease);
    let (filed_gaps, attachments) = (&ticket.filed_gaps(), &attachments);
    let begin = |(mut result, pr_now, read_at): (PullWorkResult, Pull, Timestamp)| async move {
        drop_filed_gaps(&mut result, filed_gaps);
        post_attachments(shell, ctx, attachments, &mut result).await?;
        if !matches!(result, PullWorkResult::Done { .. }) || git::head(dir).await? == *start {
            return Ok((result, pr_now, read_at));
        }
        let pushing = push_result(
            shell,
            ctx,
            dir,
            &pr.head_ref,
            lease.clone(),
            TryAgain::Reassign,
        );
        pushing.await.map_err(Refused::Writing)?;
        Ok((result, pr_now, read_at))
    };
    let ((result, pr_now, read_at), turn) =
        match converse(shell, ctx, &mut spec, Step::Start, accept, begin).await {
            Ok(conversation) => conversation,
            Err(unbegun) => return Err(unbegun.keeping(keep).await),
        };
    let feedback_authors = lock(shown).feedback_authors.clone();
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
            file_spec_gaps(shell, &spec_gaps, ticket.number())
                .await
                .map_err(apply)
        }
        PullWorkResult::Blocked {
            question,
            spec_gaps,
        } => {
            let kept = keep().await;
            file_spec_gaps(shell, &spec_gaps, ticket.number())
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
                log_tail: LogTail::session(&turn.log_tail),
            }))
        }
    }
}

/// What a pull work session was shown of its pull request.
struct Shown {
    /// The feedback up to then.
    until: Timestamp,
    /// The threads it may reply in.
    threads: Vec<String>,
    /// The people whose feedback it was shown.
    feedback_authors: Vec<Login>,
}

fn lock(shown: &Mutex<Shown>) -> std::sync::MutexGuard<'_, Shown> {
    // Each field is set whole, so what a panic left is still sound.
    shown
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        let mut ws = Workspace::clone(shell, &purpose, &shell.settings.default_branch).await?;
        let opened = open_session(shell, &ctx, purpose, &mut ws).await?;
        let (mut spec, dir) = opened.spec(|attachments| {
            prompts::automation(
                &ctx.with.briefs.prompts,
                &shell.repo(),
                automation,
                attachments,
            )
        });
        let (ctx, dir) = (&ctx, &dir);
        let begin = |mut result: AutomationResult| async move {
            drop_past_max_issues(automation, &mut result);
            post_attachments(shell, ctx, dir, &mut result).await?;
            Ok(result)
        };
        let (result, _) = converse(
            shell,
            ctx,
            &mut spec,
            Step::Start,
            |message| async move { results::parse(&message).map_err(parse_retry(ctx)) },
            begin,
        )
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
        Err(Failure::Infrastructure {
            error: e,
            log_tail: Some(log_tail),
            ..
        }) => Err(with_tail(&e, &log_tail)),
        Err(Failure::Infrastructure { error: e, .. } | Failure::Apply { error: e, .. }) => Err(e),
        Err(Failure::HandBack(HandBack {
            reason,
            log_tail: Some(log_tail),
            ..
        })) => Err(with_tail(&anyhow::anyhow!(reason), &log_tail)),
        Err(Failure::HandBack(HandBack { reason, .. })) => Err(anyhow::anyhow!(reason)),
        Err(Failure::Cancelled) => Err(anyhow::anyhow!("the run was cancelled")),
    }
}

/// An automation run's `error`, with `log_tail` under it.
fn with_tail(error: &anyhow::Error, log_tail: &LogTail) -> anyhow::Error {
    anyhow::anyhow!("{error:#}\n\n{}:\n{}", log_tail.heading(), log_tail.text)
}

/// Drops the issues past `max-issues` from `result`, which are not filed.
fn drop_past_max_issues(automation: &Automation, result: &mut AutomationResult) {
    let cap = (automation.max_issues as usize).min(result.issues.len());
    for issue in result.issues.drain(cap..) {
        warn!(
            automation = %automation.name,
            max_issues = automation.max_issues,
            title = issue.title,
            "automation issue dropped: the run returned more than max-issues"
        );
    }
}

async fn record_automation(
    shell: &Shell,
    automation: &Automation,
    result: &AutomationResult,
) -> Result<()> {
    for issue in &result.issues {
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancel_before_applying_cancels_and_applying_then_fails() {
        let cancellation = Cancellation::default();

        assert!(cancellation.cancel());

        assert!(matches!(
            cancellation.begin_applying(),
            Err(Failure::Cancelled)
        ));
        assert!(cancellation.0.token.is_cancelled());
    }

    #[test]
    fn a_cancel_while_applying_does_not_stop_the_run() {
        let cancellation = Cancellation::default();

        assert!(cancellation.begin_applying().is_ok());

        assert!(!cancellation.cancel());
        assert!(!cancellation.0.token.is_cancelled());
        assert!(cancellation.begin_applying().is_ok());
    }

    #[test]
    fn back_in_session_with_no_cancel_meanwhile_the_run_can_be_cancelled_again() {
        let cancellation = Cancellation::default();
        cancellation.begin_applying().ok();

        assert!(cancellation.back_to_session().is_ok());

        assert!(cancellation.cancel());
        assert!(cancellation.0.token.is_cancelled());
    }

    #[test]
    fn back_in_session_after_a_cancel_while_applying_the_run_is_cancelled() {
        let cancellation = Cancellation::default();
        cancellation.begin_applying().ok();
        cancellation.cancel();

        assert!(matches!(
            cancellation.back_to_session(),
            Err(Failure::Cancelled)
        ));

        assert!(cancellation.0.token.is_cancelled());
        assert!(cancellation.cancel());
    }

    #[test]
    fn applying_again_after_being_cancelled_fails() {
        let cancellation = Cancellation::default();
        cancellation.begin_applying().ok();
        cancellation.cancel();
        cancellation.back_to_session().ok();

        assert!(matches!(
            cancellation.begin_applying(),
            Err(Failure::Cancelled)
        ));
    }
}
