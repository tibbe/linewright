//! A run: one piece of work, from a fresh clone through its agent session to
//! the outcome recorded in the store. Runs read the snapshot they started
//! from and write only through the store; their decisions come from the
//! reconcile step and the session's result.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::briefs::Briefs;
use crate::clock::Clock;
use crate::domain::{
    Issue, Login, MergedPull, Number, Pull, Snapshot, Timestamp, closing_references, label,
};
use crate::git::{self, Updated};
use crate::marker::{Stage, marked, workflow_marked};
use crate::prompts::{self, PullTask, Repo, Ticket};
use crate::reconcile::{
    Feedback, Policy, Subject, Work, WorkKind, new_feedback, rebases_to_repair,
};
use crate::results::{
    self, ImplementResult, PullWorkResult, SpecGap, Tickets, TriageOutcome, TriageResult,
    WorkflowIssueLabel, WorkflowResult,
};
use crate::runtime::{Purpose, Runtime, SessionError, SessionSpec, Turn};
use crate::spec_gap;
use crate::store::{Categories, Identity, NewIssue, NewPull, PushMode, RepoSettings, Store};
use crate::tracking;
use crate::workflow::Workflow;

/// What every run shares.
pub(crate) struct Shell {
    pub store: Arc<dyn Store>,
    pub runtime: Arc<dyn Runtime>,
    pub clock: Arc<dyn Clock>,
    pub policy: Policy,
    pub briefs: Briefs,
    pub identity: Identity,
    pub settings: RepoSettings,
    pub result_retries: u32,
    pub session_timeout: std::time::Duration,
    pub work_dir: PathBuf,
}

impl Shell {
    fn repo(&self) -> Repo<'_> {
        Repo {
            default_branch: &self.settings.default_branch,
            spec_glob: self.policy.spec_glob.pattern(),
        }
    }

    /// The label to put on what Linewright creates: the required label, when
    /// one is set and the repository has it. Never created here: a missing
    /// label is skipped, not an error.
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

/// How a run ends when it does not end in its outcome.
enum Failure {
    /// The runtime or the store failed before the outcome was recorded:
    /// worth one retry.
    Infrastructure(anyhow::Error),
    /// The work goes back to the maintainer.
    HandBack {
        reason: String,
        log_tail: Option<String>,
    },
    /// Recording the outcome failed part way.
    Apply(anyhow::Error),
    /// The subject stopped asking for the work.
    Cancelled,
}

/// What a run knows beyond the shell: when its snapshot was taken, and
/// whether it is still wanted.
struct Ctx {
    as_of: Timestamp,
    cancel: CancellationToken,
}

fn infra(error: anyhow::Error) -> Failure {
    Failure::Infrastructure(error)
}

fn stage_of(kind: WorkKind) -> Stage {
    match kind {
        WorkKind::Triage => Stage::Triage,
        WorkKind::Tickets => Stage::Tickets,
        WorkKind::Implement => Stage::Implementation,
        WorkKind::Rework => Stage::Rework,
        WorkKind::FixCi => Stage::Ci,
        WorkKind::Update => Stage::Update,
    }
}

pub(crate) async fn run(
    shell: Arc<Shell>,
    work: Work,
    snapshot: Arc<Snapshot>,
    as_of: Timestamp,
    cancel: CancellationToken,
) {
    drive(&shell, &work, &snapshot, &Ctx { as_of, cancel }).await
}

async fn drive(shell: &Shell, work: &Work, snapshot: &Snapshot, ctx: &Ctx) {
    let subject = work.subject();
    let run_stage = stage_of(work.kind());
    let mut retried = false;
    let (reason, log_tail) = loop {
        match attempt(shell, work, snapshot, ctx).await {
            Ok(()) => return info!(subject = ?subject, kind = ?work.kind(), "run finished"),
            Err(Failure::Infrastructure(e)) if !retried => {
                warn!(subject = ?subject, error = format!("{e:#}"), "infrastructure failure; retrying once");
                retried = true;
            }
            Err(Failure::Infrastructure(e)) => {
                break (
                    format!("The run failed twice on its infrastructure: {e:#}"),
                    None,
                );
            }
            Err(Failure::Cancelled) => {
                return info!(subject = ?subject, "run cancelled; its branch is kept");
            }
            Err(Failure::Apply(e)) => {
                break (format!("Recording the run's outcome failed: {e:#}"), None);
            }
            Err(Failure::HandBack { reason, log_tail }) => break (reason, log_tail),
        }
    };
    // Comments hold only what belongs to the repository.
    let work_dir = shell.work_dir.to_string_lossy().into_owned();
    let reason = reason.replace(&work_dir, "<work-dir>");
    let log_tail = log_tail.map(|t| t.replace(&work_dir, "<work-dir>"));
    info!(subject = ?subject, reason, "handing back");
    let handed = match subject {
        Subject::Issue(issue) => {
            hand_back(shell, issue, run_stage, &reason, log_tail.as_deref()).await
        }
        Subject::SpecChange(pr) => {
            hand_back_spec_change(shell, pr, &reason, log_tail.as_deref()).await
        }
    };
    if let Err(e) = handed {
        warn!(subject = ?subject, error = format!("{e:#}"), "handing back failed");
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
    let ticket = |n: Number| -> Result<Ticket, Failure> {
        Ok(Ticket {
            issue: issue(n)?,
            filed_gaps: spec_gap::filed_for(snapshot, n),
        })
    };
    match work {
        Work::Triage { issue: n } => triage(shell, ctx, issue(*n)?).await,
        Work::Tickets { pr } => {
            let pr = snapshot
                .merged_pull(*pr)
                .with_context(|| format!("{pr} is not in the snapshot"))
                .map_err(infra)?;
            tickets(shell, ctx, pr).await
        }
        Work::Implement { issue: n } => implement(shell, ctx, &ticket(*n)?).await,
        Work::Rework { issue: n, pr } => {
            pull_work(shell, ctx, &ticket(*n)?, pull(*pr)?, Task::Rework).await
        }
        Work::FixCi {
            issue: n,
            pr,
            failing,
        } => pull_work(shell, ctx, &ticket(*n)?, pull(*pr)?, Task::FixCi(failing)).await,
        Work::Update { issue: n, pr } => {
            pull_work(shell, ctx, &ticket(*n)?, pull(*pr)?, Task::Update).await
        }
    }
}

/// Gives the issue to the maintainer: `ready-for-human`, unassigned, and a
/// comment with the reason and the tail of the log.
pub(crate) async fn hand_back(
    shell: &Shell,
    issue: Number,
    stage: Stage,
    reason: &str,
    log_tail: Option<&str>,
) -> Result<()> {
    let store = &shell.store;
    for state in label::STATES
        .into_iter()
        .filter(|l| *l != label::READY_FOR_HUMAN)
    {
        store.remove_label(issue, state).await?;
    }
    store.add_label(issue, label::READY_FOR_HUMAN).await?;
    store.unassign(issue).await?;
    store
        .comment(issue, &marked(stage, &with_log(reason, log_tail)))
        .await
}

async fn hand_back_spec_change(
    shell: &Shell,
    pr: Number,
    reason: &str,
    log_tail: Option<&str>,
) -> Result<()> {
    shell.store.add_label(pr, label::READY_FOR_HUMAN).await?;
    let body = format!(
        "{}\n\nRemove `{}` to cut this spec change into tickets again.",
        with_log(reason, log_tail),
        label::READY_FOR_HUMAN
    );
    shell
        .store
        .comment(pr, &marked(Stage::Tickets, &body))
        .await
}

fn with_log(reason: &str, log_tail: Option<&str>) -> String {
    match log_tail.map(str::trim).filter(|t| !t.is_empty()) {
        Some(tail) => format!(
            "{reason}\n\n<details><summary>The end of the session's log</summary>\n\n```\n{tail}\n```\n\n</details>"
        ),
        None => reason.to_string(),
    }
}

/// A fresh clone for one run, with an empty scratch directory and the
/// briefs' plugin beside it, all removed when the run ends.
struct Workspace {
    _temp: TempDir,
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
        shell
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
    }
}

/// The name of what a session is for, as its clones and records are named:
/// `issue-3`, `pr-4` or `workflow-weekly`.
pub fn purpose_name(purpose: &Purpose) -> String {
    match purpose {
        Purpose::Work { subject, .. } => subject_name(*subject),
        Purpose::Workflow(name) => format!("workflow-{name}"),
    }
}

/// A new directory for a session's record: its brief, results and trace,
/// kept under the work directory after the run ends.
fn record_dir(shell: &Shell, purpose: &Purpose, now: Timestamp) -> Result<PathBuf, Failure> {
    let parent = shell.work_dir.join("sessions").join(purpose_name(purpose));
    std::fs::create_dir_all(&parent)
        .context("creating the sessions directory")
        .map_err(infra)?;
    let kind = match purpose {
        Purpose::Work { kind, .. } => kind_name(*kind),
        Purpose::Workflow(_) => "workflow",
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
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(infra(e.into())),
        }
    }
    unreachable!()
}

fn kind_name(kind: WorkKind) -> &'static str {
    match kind {
        WorkKind::Triage => "triage",
        WorkKind::Tickets => "tickets",
        WorkKind::Implement => "implement",
        WorkKind::Rework => "rework",
        WorkKind::FixCi => "fix-ci",
        WorkKind::Update => "update",
    }
}

/// Why a session's answer is not accepted.
enum Rejection {
    /// Ask the session again with this follow-up.
    Retry(String),
    Fail(Failure),
}

impl From<anyhow::Error> for Rejection {
    fn from(error: anyhow::Error) -> Rejection {
        Rejection::Fail(infra(error))
    }
}

fn parse_retry(error: String) -> Rejection {
    Rejection::Retry(prompts::parse_error(&error))
}

/// Runs a session turn and asks again, up to the result retries, while its
/// answer is rejected. Starts the session, or resumes `session` with a
/// follow-up. Returns the accepted result and the turn that gave it.
async fn converse<T, F, Fut>(
    shell: &Shell,
    ctx: &Ctx,
    spec: &SessionSpec,
    resume: Option<(&str, String)>,
    accept: F,
) -> Result<(T, Turn), Failure>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T, Rejection>>,
{
    let record = |name: &str, text: &str| {
        let _ = std::fs::write(spec.record.join(name), text);
    };
    let session_failed = |error: SessionError| match error {
        SessionError::Infrastructure(reason) => infra(anyhow::anyhow!(reason)),
        SessionError::Failed { reason, log_tail } => Failure::HandBack {
            reason: format!("The session ended unexpectedly: {reason}"),
            log_tail: Some(scrub(&log_tail, &spec.dir)),
        },
    };
    let mut turn = match &resume {
        None => {
            record("brief.md", &spec.prompt);
            cancellable(ctx, shell.runtime.start(spec)).await
        }
        Some((session, followup)) => {
            record(&followup_name(&spec.record), followup);
            cancellable(ctx, shell.runtime.resume(spec, session, followup)).await
        }
    }?
    .map_err(session_failed)?;
    let mut retries = 0;
    loop {
        match accept(turn.message.clone()).await {
            Ok(result) => {
                let json = results::json_value(&turn.message)
                    .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
                    .unwrap_or_else(|| turn.message.clone());
                record("result.json", &json);
                return Ok((result, turn));
            }
            Err(Rejection::Fail(failure)) => return Err(failure),
            Err(Rejection::Retry(followup)) if retries < shell.result_retries => {
                retries += 1;
                record(&followup_name(&spec.record), &followup);
                turn = cancellable(ctx, shell.runtime.resume(spec, &turn.session, &followup))
                    .await?
                    .map_err(session_failed)?;
            }
            Err(Rejection::Retry(followup)) => {
                return Err(Failure::HandBack {
                    reason: format!(
                        "The session's result still could not be used after {retries} follow-up(s).\n\n{followup}"
                    ),
                    log_tail: Some(tail(&scrub(&turn.message, &spec.dir), 40)),
                });
            }
        }
    }
}

/// A session turn that stops at once when the run is cancelled; what the
/// session committed stays in the clone for the caller to keep.
async fn cancellable<T>(ctx: &Ctx, turn: impl Future<Output = T>) -> Result<T, Failure> {
    tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => Err(Failure::Cancelled),
        output = turn => Ok(output),
    }
}

fn followup_name(record: &Path) -> String {
    let n = std::fs::read_dir(record)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with("followup-"))
                .count()
        })
        .unwrap_or(0);
    format!("followup-{}.md", n + 1)
}

/// The log without the clone's location: comments hold only what belongs
/// to the repository.
fn scrub(text: &str, dir: &Path) -> String {
    text.replace(&dir.to_string_lossy().into_owned(), ".")
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// A session that times out after `timeout`.
fn session_spec(
    shell: &Shell,
    purpose: Purpose,
    ws: &Workspace,
    prompt: String,
    timeout: std::time::Duration,
) -> Result<SessionSpec, Failure> {
    Ok(SessionSpec {
        record: record_dir(shell, &purpose, shell.clock.now())?,
        purpose,
        dir: ws.dir.clone(),
        scratch: ws.scratch.clone(),
        plugin: ws.plugin.clone(),
        prompt,
        deadline: tokio::time::Instant::now() + timeout,
    })
}

// Triage.

async fn triage(shell: &Shell, ctx: &Ctx, issue: &Issue) -> Result<(), Failure> {
    let purpose = Purpose::Work {
        subject: Subject::Issue(issue.number),
        kind: WorkKind::Triage,
    };
    let ws = Workspace::clone(shell, &purpose, &shell.settings.default_branch).await?;
    let categories = shell.settings.categories.names();
    let prompt = prompts::triage(&shell.repo(), issue, &categories);
    let spec = session_spec(shell, purpose, &ws, prompt, shell.session_timeout)?;
    let categories = &categories;
    let (result, _) = converse(shell, ctx, &spec, None, |message| async move {
        results::parse_triage(&message, categories).map_err(parse_retry)
    })
    .await?;
    record_triage(shell, issue, result)
        .await
        .map_err(Failure::Apply)
}

async fn record_triage(shell: &Shell, issue: &Issue, result: TriageResult) -> Result<()> {
    let store = &shell.store;
    // The maintainer answers in the thread or hands the issue back with a
    // label (`docs/specs/product.md`, "Triage"), so the hand-back names both.
    let (state, body) = match result.outcome {
        TriageOutcome::ReadyForAgent => (label::READY_FOR_AGENT, result.comment),
        TriageOutcome::NeedsInfo => (label::NEEDS_INFO, result.comment),
        TriageOutcome::ReadyForHuman => (
            label::READY_FOR_HUMAN,
            format!(
                "{}\n\nAnswer in this thread to have it triaged again with your answer, or apply \
                 `{}` to implement it as decided or `{}` to triage it again.",
                result.comment.trim_end(),
                label::READY_FOR_AGENT,
                label::NEEDS_TRIAGE
            ),
        ),
    };
    for other in label::STATES.into_iter().filter(|l| *l != state) {
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
    store
        .comment(issue.number, &marked(Stage::Triage, &body))
        .await
}

// Tickets.

async fn tickets(shell: &Shell, ctx: &Ctx, pr: &MergedPull) -> Result<(), Failure> {
    let purpose = Purpose::Work {
        subject: Subject::SpecChange(pr.number),
        kind: WorkKind::Tickets,
    };
    let ws = Workspace::clone(shell, &purpose, &shell.settings.default_branch).await?;
    let spec_files: Vec<String> = pr
        .files
        .iter()
        .filter(|f| shell.policy.spec_glob.matches(f))
        .cloned()
        .collect();
    let diff = shell
        .store
        .diff(pr.number, &spec_files)
        .await
        .map_err(infra)?;
    let prompt = prompts::tickets(&shell.repo(), pr, &diff);
    let spec = session_spec(shell, purpose, &ws, prompt, shell.session_timeout)?;
    let (tickets, _) = converse(shell, ctx, &spec, None, |message| async move {
        results::parse_tickets(&message).map_err(parse_retry)
    })
    .await?;
    record_tickets(shell, pr, tickets)
        .await
        .map_err(Failure::Apply)
}

async fn record_tickets(shell: &Shell, pr: &MergedPull, tickets: Tickets) -> Result<()> {
    let store = &shell.store;
    if tickets.tickets.is_empty() {
        store
            .comment(pr.number, &marked(Stage::Tickets, &tickets.comment))
            .await?;
        return store.add_label(pr.number, label::SPEC_PROCESSED).await;
    }
    let required: Vec<String> = shell.creation_label().await?.into_iter().collect();
    let tracking_issue = store
        .create_issue(NewIssue {
            title: tracking::title(&pr.title),
            body: tracking::body(pr.number, &[]),
            labels: required.clone(),
        })
        .await?;
    // Tickets get `ready-for-agent` only once every edge exists, so no poll
    // sees a ticket unblocked before its blockers are linked.
    let mut list: Vec<(Number, String)> = vec![];
    for ticket in &tickets.tickets {
        let number = store
            .create_issue(NewIssue {
                title: ticket.title.clone(),
                body: ticket.body.clone(),
                labels: required.clone(),
            })
            .await?;
        store.add_sub_issue(tracking_issue, number).await?;
        for &blocker in &ticket.blocked_by {
            store.add_blocked_by(number, list[blocker].0).await?;
        }
        list.push((number, ticket.title.clone()));
    }
    for (number, _) in &list {
        store.add_label(*number, label::READY_FOR_AGENT).await?;
    }
    store
        .edit_body(tracking_issue, &tracking::body(pr.number, &list))
        .await?;
    store
        .comment(tracking_issue, &marked(Stage::Tickets, &tickets.comment))
        .await?;
    let listed: String = list.iter().map(|(n, t)| format!("- {n} {t}\n")).collect();
    store
        .comment(
            pr.number,
            &marked(
                Stage::Tickets,
                &format!(
                    "{}\n\nTracked in {tracking_issue}:\n\n{listed}",
                    tickets.comment.trim()
                ),
            ),
        )
        .await?;
    store.add_label(pr.number, label::SPEC_PROCESSED).await
}

// Implementation.

/// A branch name a person could have chosen: the issue number and its
/// title, as GitHub names branches it creates for an issue.
fn branch_name(issue: &Issue) -> String {
    let mut slug = String::new();
    for c in issue.title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        format!("{}", issue.number.0)
    } else {
        format!("{}-{slug}", issue.number.0)
    }
}

async fn implement(shell: &Shell, ctx: &Ctx, ticket: &Ticket<'_>) -> Result<(), Failure> {
    let purpose = Purpose::Work {
        subject: Subject::Issue(ticket.issue.number),
        kind: WorkKind::Implement,
    };
    let ws = Workspace::clone(shell, &purpose, &shell.settings.default_branch).await?;
    let branch = branch_name(ticket.issue);
    git::create_branch(&ws.dir, &branch).await.map_err(infra)?;
    let upstream = shell.upstream();
    let prompt = prompts::implement(&shell.repo(), ticket, &branch);
    let spec = session_spec(shell, purpose, &ws, prompt, shell.session_timeout)?;
    let (dir, base) = (&ws.dir, &upstream);
    let conversation = converse(shell, ctx, &spec, None, |message| async move {
        let result: ImplementResult = results::parse(&message).map_err(parse_retry)?;
        if let Some(problem) = git::unfinished(dir).await? {
            return Err(Rejection::Retry(prompts::unfinished(&problem)));
        }
        if matches!(result, ImplementResult::Done { .. }) && git::commits_ahead(dir, base).await? == 0 {
            return Err(Rejection::Retry(prompts::unfinished(
                "you answered `done`, but the branch has no commits. Commit your work, or answer `gave-up` with the reason if the ticket needs no change.",
            )));
        }
        Ok(result)
    })
    .await;
    let (result, turn) = match conversation {
        Err(Failure::Cancelled) => {
            // The branch is kept: what the session committed is pushed.
            if git::commits_ahead(&ws.dir, &upstream)
                .await
                .map_err(infra)?
                > 0
            {
                shell
                    .store
                    .push(&ws.dir, &branch, PushMode::Replace)
                    .await
                    .map_err(infra)?;
            }
            return Err(Failure::Cancelled);
        }
        conversation => conversation?,
    };
    let has_commits = git::commits_ahead(&ws.dir, &upstream)
        .await
        .map_err(infra)?
        > 0;
    let store = &shell.store;
    let keep_branch = async || -> Result<String, Failure> {
        if !has_commits {
            return Ok(String::new());
        }
        store
            .push(&ws.dir, &branch, PushMode::Replace)
            .await
            .map_err(Failure::Apply)?;
        Ok(format!("\n\nThe work so far is on the branch `{branch}`."))
    };
    match result {
        ImplementResult::Done {
            pr_title,
            pr_body,
            spec_gaps,
        } => {
            store
                .push(&ws.dir, &branch, PushMode::Replace)
                .await
                .map_err(infra)?;
            let pr = store
                .open_pull(NewPull {
                    head: branch.clone(),
                    title: pr_title.into(),
                    body: closing(&pr_body, ticket.issue.number),
                })
                .await
                .map_err(Failure::Apply)?;
            if let Some(required) = shell.creation_label().await.map_err(Failure::Apply)? {
                store
                    .add_label(pr, &required)
                    .await
                    .map_err(Failure::Apply)?;
            }
            info!(
                issue = ticket.issue.number.0,
                pr = pr.0,
                "opened draft pull request"
            );
            file_spec_gaps(shell, &spec_gaps, ticket)
                .await
                .map_err(Failure::Apply)
        }
        ImplementResult::Blocked {
            question,
            spec_gaps,
        } => {
            file_spec_gaps(shell, &spec_gaps, ticket)
                .await
                .map_err(Failure::Apply)?;
            let kept = keep_branch().await?;
            Err(Failure::HandBack {
                reason: format!(
                    "Blocked on a question the spec cannot decide:\n\n{}{kept}",
                    question.trim()
                ),
                log_tail: None,
            })
        }
        ImplementResult::GaveUp { reason } => {
            let kept = keep_branch().await?;
            Err(Failure::HandBack {
                reason: format!(
                    "The implementer handed the ticket back:\n\n{}{kept}",
                    reason.trim()
                ),
                log_tail: Some(turn.log_tail),
            })
        }
    }
}

/// The pull request body, ending in the reference that closes the ticket.
fn closing(body: &str, ticket: Number) -> String {
    if closing_references(body).contains(&ticket) {
        body.trim().to_string()
    } else {
        format!("{}\n\nCloses {ticket}", body.trim())
    }
}

/// Files the gaps not already filed for the ticket. A session told which are
/// may still return one, so its title is compared too.
async fn file_spec_gaps(shell: &Shell, gaps: &[SpecGap], ticket: &Ticket<'_>) -> Result<()> {
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    for gap in gaps {
        if let Some(filed) = ticket
            .filed_gaps
            .iter()
            .find(|f| same(&f.title, &gap.title))
        {
            info!(
                issue = filed.number.0,
                ticket = ticket.issue.number.0,
                "spec gap already filed"
            );
            continue;
        }
        let number = shell
            .store
            .create_issue(NewIssue {
                title: gap.title.clone(),
                body: spec_gap::body(&gap.body, ticket.issue.number),
                labels: [label::READY_FOR_HUMAN.into()]
                    .into_iter()
                    .chain(shell.creation_label().await?)
                    .collect(),
            })
            .await?;
        info!(
            issue = number.0,
            ticket = ticket.issue.number.0,
            "filed a spec gap"
        );
    }
    Ok(())
}

// Work on an open pull request.

enum Task<'a> {
    Rework,
    FixCi(&'a [String]),
    Update,
}

async fn pull_work(
    shell: &Shell,
    ctx: &Ctx,
    ticket: &Ticket<'_>,
    pr: &Pull,
    task: Task<'_>,
) -> Result<(), Failure> {
    let subject = Subject::Issue(ticket.issue.number);
    let kind = match task {
        Task::Rework => WorkKind::Rework,
        Task::FixCi(_) => WorkKind::FixCi,
        Task::Update => WorkKind::Update,
    };
    let stage = stage_of(kind);
    let machine = &shell.policy.machine_user;
    let purpose = Purpose::Work { subject, kind };
    let ws = Workspace::clone(shell, &purpose, &pr.head_ref).await?;
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
    if matches!(task, Task::Update) && conflict.is_none() {
        if git::head(&ws.dir).await.map_err(infra)? == start {
            info!(pr = pr.number.0, "already up to date");
            return Ok(());
        }
        let fresh = store.pull(pr.number).await.map_err(infra)?;
        if !late(&new_feedback(&fresh, machine), ctx.as_of).is_empty() {
            info!(
                pr = pr.number.0,
                "feedback arrived; leaving the update to a rework run"
            );
            return Ok(());
        }
        return store
            .push(&ws.dir, &pr.head_ref, lease)
            .await
            .map_err(infra);
    }
    let failures = match task {
        Task::FixCi(names) => store
            .check_failures(pr.number, names)
            .await
            .map_err(infra)?,
        _ => vec![],
    };
    let feedback = new_feedback(pr, machine);
    let mut feedback_authors = feedback_people(&feedback, machine);
    let pull_task = match task {
        Task::Rework => PullTask::Rework(feedback),
        Task::FixCi(_) => PullTask::FixCi(&failures),
        Task::Update => PullTask::Update,
    };
    let prompt = prompts::pull_work(&shell.repo(), ticket, pr, &pull_task, conflict.as_ref());
    let spec = session_spec(shell, purpose, &ws, prompt, shell.session_timeout)?;
    let mut threads: Vec<String> = pr.threads.iter().map(|t| t.id.clone()).collect();
    let dir = &ws.dir;
    let accept = |message: String, pr_threads: Vec<String>| async move {
        let result: PullWorkResult = results::parse(&message).map_err(parse_retry)?;
        if let Some(problem) = git::unfinished(dir).await? {
            return Err(Rejection::Retry(prompts::unfinished(&problem)));
        }
        if let PullWorkResult::Done { replies, .. } = &result
            && let Some(unknown) = replies.iter().find(|r| !pr_threads.contains(&r.thread))
        {
            return Err(parse_retry(format!(
                "`{}` is not a thread on the pull request",
                unknown.thread
            )));
        }
        Ok(result)
    };
    let (mut result, mut turn) =
        converse(shell, ctx, &spec, None, |m| accept(m, threads.clone())).await?;
    // Feedback posted after the snapshot the brief came from would be older
    // than the push that follows, so it goes to the session before anything
    // is pushed, for as long as it keeps coming within the session's time.
    let mut pr_now;
    let mut shown_until = ctx.as_of;
    loop {
        let read_at = shell.clock.now();
        pr_now = store.pull(pr.number).await.map_err(infra)?;
        if !pr_now.is_open() || !matches!(result, PullWorkResult::Done { .. }) {
            break;
        }
        let arrived = late(&new_feedback(&pr_now, machine), shown_until);
        if arrived.is_empty() {
            break;
        }
        shown_until = read_at;
        feedback_authors.extend(feedback_people(&arrived, machine));
        threads = pr_now.threads.iter().map(|t| t.id.clone()).collect();
        let followup = prompts::late_feedback(&pr_now, &arrived);
        let session = turn.session.clone();
        (result, turn) = converse(shell, ctx, &spec, Some((&session, followup)), |m| {
            accept(m, threads.clone())
        })
        .await?;
    }
    // Merged or closed while the session worked: its work is for a pull
    // request that is done, and what happens to the ticket is the next
    // poll's to decide.
    if !pr_now.is_open() {
        info!(
            pr = pr.number.0,
            "the pull request closed while the run worked; nothing pushed"
        );
        return Ok(());
    }
    match result {
        PullWorkResult::Done {
            comment,
            pr_body,
            replies,
            spec_gaps,
        } => {
            if git::head(&ws.dir).await.map_err(infra)? != start {
                store
                    .push(&ws.dir, &pr.head_ref, lease)
                    .await
                    .map_err(infra)?;
            }
            if let Some(body) = pr_body {
                store
                    .edit_body(pr.number, &closing(&body, ticket.issue.number))
                    .await
                    .map_err(Failure::Apply)?;
            }
            record_pull_work(shell, &pr_now, stage, &comment, &replies, feedback_authors)
                .await
                .map_err(Failure::Apply)?;
            file_spec_gaps(shell, &spec_gaps, ticket)
                .await
                .map_err(Failure::Apply)
        }
        PullWorkResult::Blocked {
            question,
            spec_gaps,
        } => {
            file_spec_gaps(shell, &spec_gaps, ticket)
                .await
                .map_err(Failure::Apply)?;
            Err(Failure::HandBack {
                reason: format!(
                    "Blocked on a question the spec cannot decide:\n\n{}",
                    question.trim()
                ),
                log_tail: None,
            })
        }
        PullWorkResult::GaveUp { reason } => Err(Failure::HandBack {
            reason: format!(
                "The implementer handed the ticket back:\n\n{}",
                reason.trim()
            ),
            log_tail: Some(turn.log_tail),
        }),
    }
}

/// Feedback posted after `since`.
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

fn feedback_people(feedback: &Feedback, machine: &Login) -> Vec<Login> {
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
        .filter(|a| a.is_person_besides(machine))
        .cloned()
        .collect()
}

/// Replies in the threads and comments on `pr`, and after a rework asks the
/// people whose feedback it addressed, `feedback_authors`, to review again.
async fn record_pull_work(
    shell: &Shell,
    pr: &Pull,
    stage: Stage,
    comment: &str,
    replies: &[results::ThreadReply],
    feedback_authors: Vec<Login>,
) -> Result<()> {
    let store = &shell.store;
    for reply in replies {
        store
            .reply_in_thread(pr.number, &reply.thread, &marked(stage, &reply.body))
            .await?;
        // People resolve their own threads; the implementer resolves bots'.
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
    store.comment(pr.number, &marked(stage, comment)).await?;
    if stage == Stage::Rework {
        let mut reviewers = feedback_authors;
        reviewers.sort();
        reviewers.dedup();
        if !reviewers.is_empty() {
            store.request_review(pr.number, &reviewers).await?;
        }
    }
    Ok(())
}

// Workflows.

/// Runs `workflow` once: one session on a fresh clone of the default branch,
/// then files the issues its result returns. A workflow run has no issue to
/// hand back, so a run that fails is an error.
pub(crate) async fn workflow(
    shell: &Shell,
    workflow: &Workflow,
    cancel: CancellationToken,
) -> Result<WorkflowResult> {
    let ctx = Ctx {
        as_of: shell.clock.now(),
        cancel,
    };
    let session = async {
        let purpose = Purpose::Workflow(workflow.name.clone());
        let ws = Workspace::clone(shell, &purpose, &shell.settings.default_branch).await?;
        let prompt = prompts::workflow(&shell.briefs, &shell.repo(), workflow);
        let timeout = workflow.timeout.unwrap_or(shell.session_timeout);
        let spec = session_spec(shell, purpose, &ws, prompt, timeout)?;
        let (result, _) = converse(shell, &ctx, &spec, None, |message| async move {
            results::parse(&message).map_err(parse_retry)
        })
        .await?;
        record_workflow(shell, workflow, &result)
            .await
            .map_err(Failure::Apply)?;
        Ok(result)
    };
    session.await.map_err(|failure| match failure {
        Failure::Infrastructure(e) | Failure::Apply(e) => e,
        Failure::HandBack { reason, log_tail } => {
            match log_tail.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                Some(tail) => anyhow::anyhow!("{reason}\n\nThe end of the session's log:\n{tail}"),
                None => anyhow::anyhow!(reason),
            }
        }
        Failure::Cancelled => anyhow::anyhow!("the run was cancelled"),
    })
}

/// Files the issues `result` returns, as the machine user, up to the
/// workflow's `max-issues`; the rest are dropped.
async fn record_workflow(
    shell: &Shell,
    workflow: &Workflow,
    result: &WorkflowResult,
) -> Result<()> {
    let cap = (workflow.max_issues as usize).min(result.issues.len());
    let (filed, dropped) = result.issues.split_at(cap);
    for issue in filed {
        let state = match issue.label {
            WorkflowIssueLabel::ReadyForAgent => label::READY_FOR_AGENT,
            WorkflowIssueLabel::ReadyForHuman => label::READY_FOR_HUMAN,
        };
        let number = shell
            .store
            .create_issue(NewIssue {
                title: issue.title.clone(),
                body: workflow_marked(&workflow.name, &issue.body),
                labels: [state.into()]
                    .into_iter()
                    .chain(shell.creation_label().await?)
                    .collect(),
            })
            .await?;
        info!(
            workflow = %workflow.name,
            issue = %number,
            title = issue.title,
            "workflow issue filed"
        );
    }
    for issue in dropped {
        warn!(
            workflow = %workflow.name,
            max_issues = workflow.max_issues,
            title = issue.title,
            "workflow issue dropped: the run returned more than max-issues"
        );
    }
    Ok(())
}
