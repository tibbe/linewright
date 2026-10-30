//! The hand-back and send-back decisions: why a run hands its item back,
//! and what the hand-back posts and labels.

use super::{Asks, Designed, StatusTo};
use crate::domain::{HandBackKind, Issue, Login, StatusLabel, label};
use crate::harness;
use crate::prompts::DesignOf;
use crate::redaction::Redaction;
use crate::secrets::Held;
use crate::store::FoundSecrets;
use crate::work_settings::TimeoutSetting;

/// How many of a log's last lines a hand-back gives.
const TAIL_LINES: usize = 40;

/// Linewright's letting go of an item it cannot finish (`GLOSSARY.md`,
/// **Hand-back**).
#[derive(Debug, PartialEq, Eq)]
pub struct HandBack {
    pub kind: HandBackKind,
    pub reason: String,
    /// The end of a log, when what wrote it is the reason.
    pub log_tail: Option<LogTail>,
}

/// The end of a log a hand-back gives, and what it is the end of. Its text
/// is never empty.
#[derive(Debug, PartialEq, Eq)]
pub struct LogTail {
    of: Logged,
    text: String,
}

/// What a log tail is the end of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Logged {
    Session,
    Setup,
}

impl LogTail {
    /// The end of the session's log `text`, unless it holds nothing.
    pub fn session(text: &str) -> Option<LogTail> {
        LogTail::new(Logged::Session, text)
    }

    /// The last lines of the setup command's `output`, unless it holds
    /// nothing.
    pub fn setup(output: &str) -> Option<LogTail> {
        LogTail::new(Logged::Setup, &tail(output))
    }

    fn new(of: Logged, text: &str) -> Option<LogTail> {
        let text = text.trim();
        (!text.is_empty()).then(|| LogTail {
            of,
            text: text.to_string(),
        })
    }

    pub fn logged(&self) -> Logged {
        self.of
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The tail with `rewrite` applied to its text, unless that leaves
    /// nothing.
    #[must_use]
    pub fn rewritten(self, rewrite: impl FnOnce(&str) -> String) -> Option<LogTail> {
        LogTail::new(self.of, &rewrite(&self.text))
    }

    /// What it is the end of, as a hand-back heads it.
    pub fn heading(&self) -> &'static str {
        match self.of {
            Logged::Session => "The end of the session's log",
            Logged::Setup => "The end of the setup command's output",
        }
    }
}

fn tail(text: &str) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// The item a hand-back lets go of.
#[derive(Clone, Copy)]
pub enum HandedBack<'a> {
    /// An issue, as it was read.
    Issue(&'a Issue),
    Pull,
    SpecChange,
}

/// What a hand-back writes on its item: the comment, then the asks.
#[derive(Debug, PartialEq, Eq)]
pub struct HandBackWrites {
    pub comment: String,
    pub asks: Asks,
}

/// What handing `item` back for `hand_back` writes on it. An issue's own
/// design goes with it, so that the next attempt designs it again, while a
/// ticket keeps the design of the item it was cut from
/// (docs/specs/repository.md, "Handing back"). A pull request's comment
/// says that assigning `linewright` again tries again.
pub fn hand_back(item: HandedBack<'_>, hand_back: &HandBack, linewright: &Login) -> HandBackWrites {
    let stall = stalls(hand_back);
    let pull = |try_again: &str, designed: Option<Designed>| HandBackWrites {
        comment: format!(
            "{}\n\nAssign @{linewright} to this pull request again to {try_again}.",
            with_log(hand_back),
        ),
        asks: Asks {
            designed,
            stall,
            unassign: true,
            ..Asks::default()
        },
    };
    match item {
        HandedBack::Issue(issue) => HandBackWrites {
            comment: with_log(hand_back),
            asks: Asks {
                designed: (!issue.was_cut(linewright)).then_some(Designed::Remove),
                stall,
                status: Some(StatusTo::Set(StatusLabel::ReadyForHuman)),
                unassign: true,
            },
        },
        HandedBack::Pull => pull("try again", None),
        HandedBack::SpecChange => pull("design it again", Some(Designed::Remove)),
    }
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

/// What becomes of a result the run sends back to its session.
#[derive(Debug, PartialEq, Eq)]
pub enum SendBack {
    /// Sent back with a follow-up.
    Retry,
    /// Past `result-retries`: handed back.
    HandBack(HandBack),
}

/// The follow-ups a session's result was sent back with, counted against
/// `result-retries` (docs/specs/runs.md, Ending, "A result Linewright can't
/// accept").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FollowUps(u32);

impl FollowUps {
    /// Sends the result back to its session with `followup`, counting the
    /// follow-up, until they reach `result_retries`; then hands it back
    /// with the end of `session_log`, the session's last message.
    pub fn send_back(
        &mut self,
        result_retries: u32,
        followup: &str,
        session_log: &str,
    ) -> SendBack {
        let FollowUps(sent) = *self;
        if sent < result_retries {
            *self = FollowUps(sent + 1);
            return SendBack::Retry;
        }
        SendBack::HandBack(HandBack {
            kind: HandBackKind::Stall,
            reason: format!(
                "The session's result still could not be used after {sent} follow-up(s).\n\n{followup}"
            ),
            log_tail: LogTail::session(&tail(session_log)),
        })
    }

    /// The session was asked a new question, whose answer is a new result
    /// with follow-ups of its own.
    pub fn resumed(&mut self) {
        *self = FollowUps::default();
    }
}

/// How to try again after GitHub refused a push.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TryAgain {
    /// Apply `ready-for-agent` again, as for a ticket.
    Relabel,
    /// Assign Linewright again, as for a pull request.
    Reassign,
}

/// What became of the work of a run that stopped before it finished.
#[derive(Debug, PartialEq, Eq)]
pub enum Kept {
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

impl Kept {
    /// What became of the work, as keeping it gave `kept`, with what Git or
    /// GitHub said redacted: hooks the session left in its clone can print
    /// a secret.
    pub fn redacted(kept: anyhow::Result<Kept>, redaction: &Redaction) -> Kept {
        match kept {
            Ok(Kept::Refused(reason)) => Kept::Refused(redaction.apply(&reason)),
            Ok(kept) => kept,
            Err(e) => Kept::Failed(redaction.apply(&format!("{e:#}"))),
        }
    }
}

/// The work of a run that stopped before it finished: what was `kept` of
/// it on `branch`, and how to `try_again` when GitHub refused it.
pub struct Stopped<'a> {
    pub branch: &'a str,
    pub kept: Kept,
    pub try_again: TryAgain,
}

impl Stopped<'_> {
    /// What a hand-back adds to say where the work is.
    pub fn note(&self) -> String {
        let Stopped {
            branch,
            kept,
            try_again,
        } = self;
        match kept {
            Kept::Nothing => String::new(),
            Kept::Pushed => format!("\n\nThe work so far is on the branch `{branch}`."),
            Kept::Held(held) => format!(
                "\n\nThe work so far was not kept: the commit `{}` adds the value of `{}`, a \
             secret, to `{}`.",
                held.commit, held.variable, held.file
            ),
            Kept::Protected(secrets) => protected_note(secrets),
            Kept::Refused(reason) => {
                format!("\n\n{}", refused_push_reason(branch, reason, *try_again))
            }
            Kept::Failed(error) => {
                format!("\n\nKeeping the work so far on the branch `{branch}` failed: {error}")
            }
        }
    }
}

/// What a hand-back adds to say the work was not kept, as GitHub's push
/// protection found `secrets` in it.
pub fn protected_note(secrets: &FoundSecrets) -> String {
    format!(
        "\n\nThe work so far was not kept, as it holds a secret: GitHub's push protection \
         found {secrets}."
    )
}

/// The hand-back of a run whose push of `branch` GitHub refused for a
/// missing permission, saying `reason`, telling how to `try_again`.
pub fn refused_push(branch: &str, reason: &str, try_again: TryAgain) -> HandBack {
    HandBack {
        kind: HandBackKind::Stall,
        reason: refused_push_reason(branch, reason, try_again),
        log_tail: None,
    }
}

fn refused_push_reason(branch: &str, reason: &str, try_again: TryAgain) -> String {
    let remedy = match try_again {
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

/// The hand-back of a run GitHub refused the upload of the attachment
/// `file` for its token, saying `reason`.
pub fn refused_attachment(file: &str, reason: &str) -> HandBack {
    HandBack {
        kind: HandBackKind::Stall,
        reason: format!(
            "GitHub refused Linewright's token the upload of the attachment `{file}`. \
             GitHub's reason: {reason}\n\nLinewright's token needs write access to \
             the repository, in a classic or fine-grained personal access token, to \
             attach files to what it posts."
        ),
        log_tail: None,
    }
}

/// The hand-back of a design blocked on `question`.
pub fn design_blocked(question: &str) -> HandBack {
    HandBack {
        kind: HandBackKind::Decision,
        reason: blocked_on(question),
        log_tail: None,
    }
}

/// The hand-back of an implement or pull request run blocked on
/// `question`, which `stopped`.
pub fn blocked(question: &str, stopped: &Stopped<'_>) -> HandBack {
    HandBack {
        kind: HandBackKind::Decision,
        reason: format!("{}{}", blocked_on(question), stopped.note()),
        log_tail: None,
    }
}

fn blocked_on(question: &str) -> String {
    format!(
        "Blocked on a question the spec cannot decide:\n\n{}",
        question.trim()
    )
}

/// The hand-back of a design of `of` that gave up for `reason`, with the end
/// of `session_log`.
pub fn design_gave_up(of: DesignOf, reason: &str, session_log: &str) -> HandBack {
    HandBack {
        kind: HandBackKind::Decision,
        reason: format!("The designer handed the {} back:\n\n{reason}", of.noun()),
        log_tail: LogTail::session(session_log),
    }
}

/// What an implementer works.
#[derive(Clone, Copy, Debug)]
pub enum Implementing {
    Ticket,
    PullRequest,
}

impl Implementing {
    pub fn noun(self) -> &'static str {
        match self {
            Implementing::Ticket => "ticket",
            Implementing::PullRequest => "pull request",
        }
    }
}

/// The hand-back of an implementer of `work` that gave up for `reason`,
/// with the end of `session_log`, which `stopped`.
pub fn gave_up(
    work: Implementing,
    reason: &str,
    session_log: &str,
    stopped: &Stopped<'_>,
) -> HandBack {
    HandBack {
        kind: HandBackKind::Decision,
        reason: format!(
            "The implementer handed the {} back:\n\n{}{}",
            work.noun(),
            reason.trim(),
            stopped.note()
        ),
        log_tail: LogTail::session(session_log),
    }
}

/// The hand-back of an issue's design that found nothing to build, for
/// `reason`.
pub fn nothing_to_build(reason: &str) -> HandBack {
    HandBack {
        kind: HandBackKind::Decision,
        reason: format!(
            "There is nothing to build: the code already does what this issue asks.\n\n{reason}"
        ),
        log_tail: None,
    }
}

/// The hand-back of a session that timed out, which `timeout_setting`
/// limits, with `log_tail` as its log (docs/specs/runs.md, "Limits",
/// Session timeout).
pub fn timed_out(timeout_setting: &TimeoutSetting, log_tail: Option<LogTail>) -> HandBack {
    HandBack {
        kind: HandBackKind::Stall,
        reason: format!(
            "The session ended unexpectedly: {}. The work may need cutting smaller, or \
             {timeout_setting} may need to be longer.",
            harness::TIMED_OUT,
        ),
        log_tail,
    }
}

/// The hand-back of a session that ended unexpectedly for `reason`, with
/// the end of `session_log`.
pub fn session_failed(reason: &str, session_log: &str) -> HandBack {
    HandBack {
        kind: HandBackKind::Stall,
        reason: format!("The session ended unexpectedly: {reason}"),
        log_tail: LogTail::session(session_log),
    }
}

/// Whether a run that failed on its infrastructure, having tried again
/// `retries` times, tries again: once (docs/specs/runs.md, Ending,
/// "Retries").
pub fn retries_infrastructure_failure(retries: u32) -> bool {
    retries < 1
}

/// The hand-back of a run whose retry failed on its infrastructure too, with
/// `error`, then `note`.
pub fn failed_twice(error: &anyhow::Error, note: &str, log_tail: Option<LogTail>) -> HandBack {
    HandBack {
        kind: HandBackKind::Stall,
        reason: format!("The run failed twice on its infrastructure: {error:#}{note}"),
        log_tail,
    }
}

/// The hand-back of a run whose recording of its result failed with
/// `error`, then `note`.
pub fn recording_failed(error: &anyhow::Error, note: &str) -> HandBack {
    HandBack {
        kind: HandBackKind::Stall,
        reason: format!("Recording the run's outcome failed: {error:#}{note}"),
        log_tail: None,
    }
}
