use chrono::{DateTime, Utc};
use linewright::apply::hand_back::*;
use linewright::apply::{Asks, Designed, StatusTo};
use linewright::domain::{HandBackKind, Issue, Login, Number, Parent, StatusLabel};
use linewright::prompts::DesignOf;
use linewright::reconcile::WorkKind;
use linewright::secrets::Held;
use linewright::store::{FoundSecret, FoundSecrets};
use linewright::work_settings::TimeoutSetting;
use rstest::rstest;

use crate::common::linewright;

fn issue(author: &str, parent: Option<Parent>) -> Box<Issue> {
    let at: DateTime<Utc> = "2026-10-01T12:00:00Z".parse().unwrap();
    Box::new(Issue {
        number: Number(7),
        title: "Gadgets".into(),
        body: String::new(),
        author: Login::new(author),
        open: true,
        created_at: at,
        updated_at: at,
        labels: vec![],
        assignees: vec![linewright()],
        comments: vec![],
        blocked_by: vec![],
        sub_issues: vec![],
        parent,
        events: vec![],
        filed: None,
        closed_by: vec![],
    })
}

fn hand_back_of(kind: HandBackKind, reason: &str, log_tail: Option<LogTail>) -> HandBack {
    HandBack {
        kind,
        reason: reason.into(),
        log_tail,
    }
}

/// The tail of a log of `of` whose last lines are `text`, made as the
/// tests below check it is.
fn log(of: Logged, text: &str) -> LogTail {
    match of {
        Logged::Session => LogTail::session(text),
        Logged::Setup => LogTail::setup(text),
    }
    .unwrap()
}

/// The item handed back, which a case owns.
enum Item {
    Issue(Box<Issue>),
    Pull,
    SpecChange,
}

impl Item {
    fn handed_back(&self) -> HandedBack<'_> {
        match self {
            Item::Issue(issue) => HandedBack::Issue(issue),
            Item::Pull => HandedBack::Pull,
            Item::SpecChange => HandedBack::SpecChange,
        }
    }
}

#[rstest]
#[case::an_issue_loses_its_own_design(
    Item::Issue(issue("reporter", None)),
    HandBackKind::Decision,
    Asks {
        designed: Some(Designed::Remove),
        stall: false,
        status: Some(StatusTo::Set(StatusLabel::ReadyForHuman)),
        unassign: true,
    }
)]
#[case::a_stalled_issue_is_labeled_stalled(
    Item::Issue(issue("reporter", None)),
    HandBackKind::Stall,
    Asks {
        designed: Some(Designed::Remove),
        stall: true,
        status: Some(StatusTo::Set(StatusLabel::ReadyForHuman)),
        unassign: true,
    }
)]
#[case::a_ticket_cut_from_a_spec_change_keeps_its_design(
    Item::Issue(issue(
        "factory",
        Some(Parent { number: Number(3), spec_change: Some(Number(2)) }),
    )),
    HandBackKind::Decision,
    Asks {
        designed: None,
        stall: false,
        status: Some(StatusTo::Set(StatusLabel::ReadyForHuman)),
        unassign: true,
    }
)]
#[case::a_ticket_linewright_cut_from_an_issue_keeps_its_design(
    Item::Issue(issue("factory", Some(Parent { number: Number(3), spec_change: None }))),
    HandBackKind::Decision,
    Asks {
        designed: None,
        stall: false,
        status: Some(StatusTo::Set(StatusLabel::ReadyForHuman)),
        unassign: true,
    }
)]
#[case::a_sub_issue_a_person_filed_loses_its_own_design(
    Item::Issue(issue("maintainer", Some(Parent { number: Number(3), spec_change: None }))),
    HandBackKind::Decision,
    Asks {
        designed: Some(Designed::Remove),
        stall: false,
        status: Some(StatusTo::Set(StatusLabel::ReadyForHuman)),
        unassign: true,
    }
)]
#[case::a_pull_request_keeps_its_labels(
    Item::Pull,
    HandBackKind::Decision,
    Asks { designed: None, stall: false, status: None, unassign: true }
)]
#[case::a_stalled_pull_request_is_labeled_stalled(
    Item::Pull,
    HandBackKind::Stall,
    Asks { designed: None, stall: true, status: None, unassign: true }
)]
#[case::a_spec_change_loses_its_design(
    Item::SpecChange,
    HandBackKind::Decision,
    Asks { designed: Some(Designed::Remove), stall: false, status: None, unassign: true }
)]
#[case::a_stalled_spec_change_loses_its_design_and_is_labeled_stalled(
    Item::SpecChange,
    HandBackKind::Stall,
    Asks { designed: Some(Designed::Remove), stall: true, status: None, unassign: true }
)]
fn a_hand_back_asks(#[case] item: Item, #[case] kind: HandBackKind, #[case] expected: Asks) {
    let handed = hand_back_of(kind, "Too hard.", None);

    let writes = hand_back(item.handed_back(), &handed, &linewright());

    assert_eq!(writes.asks, expected);
}

#[rstest]
#[case::an_issue_is_told_why(Item::Issue(issue("reporter", None)), None, "Too hard.")]
#[case::an_issue_is_given_the_end_of_the_sessions_log(
    Item::Issue(issue("reporter", None)),
    Some(log(Logged::Session, "Gave up.")),
    "Too hard.\n\n<details><summary>The end of the session's log</summary>\n\n```\nGave up.\n```\n\n</details>"
)]
#[case::an_issue_is_given_the_end_of_the_setup_commands_output(
    Item::Issue(issue("reporter", None)),
    Some(log(Logged::Setup, "npm ERR!")),
    "Too hard.\n\n<details><summary>The end of the setup command's output</summary>\n\n```\nnpm ERR!\n```\n\n</details>"
)]
#[case::a_pull_request_is_told_to_assign_linewright_to_try_again(
    Item::Pull,
    None,
    "Too hard.\n\nAssign @factory to this pull request again to try again."
)]
#[case::a_pull_requests_log_comes_before_how_to_try_again(
    Item::Pull,
    Some(log(Logged::Session, "Gave up.")),
    "Too hard.\n\n<details><summary>The end of the session's log</summary>\n\n```\nGave up.\n```\n\n</details>\n\nAssign @factory to this pull request again to try again."
)]
#[case::a_spec_change_is_told_to_assign_linewright_to_design_it_again(
    Item::SpecChange,
    None,
    "Too hard.\n\nAssign @factory to this pull request again to design it again."
)]
fn a_hand_back_comments(
    #[case] item: Item,
    #[case] log_tail: Option<LogTail>,
    #[case] expected: &str,
) {
    let handed = hand_back_of(HandBackKind::Decision, "Too hard.", log_tail);

    let writes = hand_back(item.handed_back(), &handed, &linewright());

    assert_eq!(writes.comment, expected);
}

/// The lines `1` to `to` of a log, one number a line.
fn numbered_lines(to: u32) -> String {
    (1..=to)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[rstest]
#[case::the_first_of_one_is_sent_back(0, 1, SendBack::Retry)]
#[case::the_second_of_two_is_sent_back(1, 2, SendBack::Retry)]
#[case::past_the_retries_it_is_handed_back(
    1,
    1,
    SendBack::HandBack(hand_back_of(
        HandBackKind::Stall,
        "The session's result still could not be used after 1 follow-up(s).\n\nAnswer with JSON.",
        Some(log(Logged::Session, "I think it's a bug.")),
    ))
)]
#[case::with_no_retries_it_is_handed_back_at_once(
    0,
    0,
    SendBack::HandBack(hand_back_of(
        HandBackKind::Stall,
        "The session's result still could not be used after 0 follow-up(s).\n\nAnswer with JSON.",
        Some(log(Logged::Session, "I think it's a bug.")),
    ))
)]
fn a_result_sent_back(
    #[case] sent_before: u32,
    #[case] result_retries: u32,
    #[case] expected: SendBack,
) {
    let mut follow_ups = FollowUps::default();
    for _ in 0..sent_before {
        follow_ups.send_back(result_retries, "Answer with JSON.", "Done.\n");
    }

    let sent = follow_ups.send_back(result_retries, "Answer with JSON.", "I think it's a bug.\n");

    assert_eq!(sent, expected);
}

/// What `tail` shows: what it is the end of, and its text.
fn shown(tail: Option<&LogTail>) -> Option<(Logged, &str)> {
    tail.map(|tail| (tail.logged(), tail.text()))
}

#[rstest]
#[case::an_empty_message_gives_no_log("  \n", None)]
#[case::a_long_message_gives_its_last_forty_lines(
    &numbered_lines(41),
    Some((Logged::Session, "2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n17\n18\n19\n20\n21\n22\n23\n24\n25\n\
         26\n27\n28\n29\n30\n31\n32\n33\n34\n35\n36\n37\n38\n39\n40\n41"))
)]
fn a_result_handed_back_gives_the_end_of_the_sessions_message(
    #[case] message: &str,
    #[case] expected: Option<(Logged, &str)>,
) {
    let SendBack::HandBack(handed) =
        FollowUps::default().send_back(0, "Answer with JSON.", message)
    else {
        panic!("the result is not handed back");
    };

    assert_eq!(shown(handed.log_tail.as_ref()), expected);
}

#[rstest]
#[case::an_empty_log_gives_none("\n  \n", None)]
#[case::a_log_is_trimmed("\n  Gave up.\n\n", Some((Logged::Session, "Gave up.")))]
fn the_end_of_a_sessions_log(#[case] text: &str, #[case] expected: Option<(Logged, &str)>) {
    assert_eq!(shown(LogTail::session(text).as_ref()), expected);
}

#[rstest]
#[case::an_empty_output_gives_none("\n  \n", None)]
#[case::an_output_is_trimmed("\nnpm ERR!\n\n", Some((Logged::Setup, "npm ERR!")))]
#[case::a_long_output_gives_its_last_forty_lines(
    &numbered_lines(42),
    Some((Logged::Setup, "3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n17\n18\n19\n20\n21\n22\n23\n24\n25\n\
         26\n27\n28\n29\n30\n31\n32\n33\n34\n35\n36\n37\n38\n39\n40\n41\n42"))
)]
fn the_end_of_the_setup_commands_output(
    #[case] output: &str,
    #[case] expected: Option<(Logged, &str)>,
) {
    assert_eq!(shown(LogTail::setup(output).as_ref()), expected);
}

#[rstest]
#[case::a_sessions_log_stays_one(Logged::Session, "token [redacted]", Some((Logged::Session, "token [redacted]")))]
#[case::a_setup_commands_output_stays_one(Logged::Setup, "token [redacted]", Some((Logged::Setup, "token [redacted]")))]
#[case::its_new_text_is_trimmed(Logged::Session, "\n token [redacted]\n", Some((Logged::Session, "token [redacted]")))]
#[case::a_log_rewritten_to_nothing_gives_none(Logged::Session, " \n", None)]
fn a_rewritten_log(
    #[case] of: Logged,
    #[case] new_text: &str,
    #[case] expected: Option<(Logged, &str)>,
) {
    let rewritten = log(of, "token ghp_1").rewritten(|_| new_text.to_string());

    assert_eq!(shown(rewritten.as_ref()), expected);
}

fn found_secrets() -> FoundSecrets {
    FoundSecrets::new(vec![FoundSecret {
        kind: "GitHub Personal Access Token".into(),
        commit: "8f1d2c3".into(),
        path: "src/config.rs:3".into(),
    }])
    .unwrap()
}

fn held() -> Held {
    Held {
        variable: "GH_TOKEN".into(),
        file: "src/config.rs".into(),
        commit: "8f1d2c3".into(),
    }
}

fn stopped(kept: Kept, try_again: TryAgain) -> Stopped<'static> {
    Stopped {
        branch: "7-gadgets",
        kept,
        try_again,
    }
}

#[rstest]
#[case::nothing_kept_adds_nothing(Kept::Nothing, TryAgain::Relabel, "")]
#[case::pushed_work_is_on_its_branch(
    Kept::Pushed,
    TryAgain::Relabel,
    "\n\nThe work so far is on the branch `7-gadgets`."
)]
#[case::work_holding_a_secret_names_it_and_where(
    Kept::Held(held()),
    TryAgain::Relabel,
    "\n\nThe work so far was not kept: the commit `8f1d2c3` adds the value of `GH_TOKEN`, a \
     secret, to `src/config.rs`."
)]
#[case::work_push_protection_refused_names_what_it_found(
    Kept::Protected(found_secrets()),
    TryAgain::Relabel,
    "\n\nThe work so far was not kept, as it holds a secret: GitHub's push protection found \
     GitHub Personal Access Token in the commit `8f1d2c3`, at `src/config.rs:3`."
)]
#[case::a_refused_push_says_how_to_try_again(
    Kept::Refused("refusing to allow a workflow without `workflow` scope".into()),
    TryAgain::Relabel,
    "\n\nGitHub refused to push the branch `7-gadgets` because Linewright's token lacks a \
     permission. GitHub's reason: refusing to allow a workflow without `workflow` scope\n\n\
     The work was not kept. Grant the permission and apply `ready-for-agent` again, or make \
     the change yourself."
)]
#[case::a_failed_keep_says_why(
    Kept::Failed("git commit: exit status 1".into()),
    TryAgain::Relabel,
    "\n\nKeeping the work so far on the branch `7-gadgets` failed: git commit: exit status 1"
)]
fn the_note_on_a_stopped_runs_work(
    #[case] kept: Kept,
    #[case] try_again: TryAgain,
    #[case] expected: &str,
) {
    assert_eq!(stopped(kept, try_again).note(), expected);
}

#[rstest]
#[case::one_secret(
    found_secrets(),
    "\n\nThe work so far was not kept, as it holds a secret: GitHub's push protection found \
     GitHub Personal Access Token in the commit `8f1d2c3`, at `src/config.rs:3`."
)]
#[case::two_secrets(
    FoundSecrets::new(vec![
        FoundSecret {
            kind: "GitHub Personal Access Token".into(),
            commit: "8f1d2c3".into(),
            path: "src/config.rs:3".into(),
        },
        FoundSecret {
            kind: "Slack API Token".into(),
            commit: "9e0a1b2".into(),
            path: ".env:1".into(),
        },
    ])
    .unwrap(),
    "\n\nThe work so far was not kept, as it holds a secret: GitHub's push protection found \
     GitHub Personal Access Token in the commit `8f1d2c3`, at `src/config.rs:3`; Slack API \
     Token in the commit `9e0a1b2`, at `.env:1`."
)]
fn the_note_on_work_push_protection_refused(#[case] secrets: FoundSecrets, #[case] expected: &str) {
    assert_eq!(protected_note(&secrets), expected);
}

#[rstest]
#[case::a_ticket_says_to_relabel_it(
    TryAgain::Relabel,
    "GitHub refused to push the branch `7-gadgets` because Linewright's token lacks a \
     permission. GitHub's reason: refusing to allow a workflow without `workflow` scope\n\n\
     The work was not kept. Grant the permission and apply `ready-for-agent` again, or make \
     the change yourself."
)]
#[case::a_pull_request_says_to_grant_the_permission(
    TryAgain::Reassign,
    "GitHub refused to push the branch `7-gadgets` because Linewright's token lacks a \
     permission. GitHub's reason: refusing to allow a workflow without `workflow` scope\n\n\
     The work was not kept. Grant the permission, or make the change yourself."
)]
fn a_refused_push_stalls(#[case] try_again: TryAgain, #[case] reason: &str) {
    let refused = refused_push(
        "7-gadgets",
        "refusing to allow a workflow without `workflow` scope",
        try_again,
    );

    assert_eq!(refused, hand_back_of(HandBackKind::Stall, reason, None));
}

#[rstest]
#[case::an_image(
    "shot.png",
    "Resource not accessible by personal access token",
    "GitHub refused Linewright's token the upload of the attachment `shot.png`. GitHub's \
     reason: Resource not accessible by personal access token\n\nLinewright's token needs \
     write access to the repository, in a classic or fine-grained personal access token, to \
     attach files to what it posts."
)]
fn a_refused_attachment_stalls(#[case] file: &str, #[case] reason: &str, #[case] expected: &str) {
    assert_eq!(
        refused_attachment(file, reason),
        hand_back_of(HandBackKind::Stall, expected, None)
    );
}

#[rstest]
#[case::a_question(
    "Should gadgets expire?",
    "Blocked on a question the spec cannot decide:\n\nShould gadgets expire?"
)]
#[case::a_question_is_trimmed(
    "\n Should gadgets expire? \n",
    "Blocked on a question the spec cannot decide:\n\nShould gadgets expire?"
)]
fn a_blocked_design_is_a_decision(#[case] question: &str, #[case] reason: &str) {
    assert_eq!(
        design_blocked(question),
        hand_back_of(HandBackKind::Decision, reason, None)
    );
}

#[rstest]
#[case::nothing_kept(
    Kept::Nothing,
    "Blocked on a question the spec cannot decide:\n\nShould gadgets expire?"
)]
#[case::the_work_kept_on_its_branch(
    Kept::Pushed,
    "Blocked on a question the spec cannot decide:\n\nShould gadgets expire?\n\n\
     The work so far is on the branch `7-gadgets`."
)]
fn a_blocked_run_is_a_decision_saying_where_its_work_is(#[case] kept: Kept, #[case] reason: &str) {
    let blocked = blocked(
        " Should gadgets expire?\n",
        &stopped(kept, TryAgain::Relabel),
    );

    assert_eq!(blocked, hand_back_of(HandBackKind::Decision, reason, None));
}

#[rstest]
#[case::a_spec_change(
    DesignOf::SpecChange,
    "Read the code.",
    hand_back_of(
        HandBackKind::Decision,
        "The designer handed the spec change back:\n\nThe codebase does not build.",
        Some(log(Logged::Session, "Read the code.")),
    )
)]
#[case::an_issue_with_no_log(
    DesignOf::Issue,
    "",
    hand_back_of(
        HandBackKind::Decision,
        "The designer handed the issue back:\n\nThe codebase does not build.",
        None,
    )
)]
fn a_design_that_gave_up_is_a_decision(
    #[case] of: DesignOf,
    #[case] session_log: &str,
    #[case] expected: HandBack,
) {
    assert_eq!(
        design_gave_up(of, "The codebase does not build.", session_log),
        expected
    );
}

#[rstest]
#[case::a_ticket_with_nothing_kept(
    Implementing::Ticket,
    Kept::Nothing,
    "The implementer handed the ticket back:\n\nToo hard."
)]
#[case::a_ticket_its_work_kept(
    Implementing::Ticket,
    Kept::Pushed,
    "The implementer handed the ticket back:\n\nToo hard.\n\n\
     The work so far is on the branch `7-gadgets`."
)]
#[case::a_pull_request(
    Implementing::PullRequest,
    Kept::Nothing,
    "The implementer handed the pull request back:\n\nToo hard."
)]
fn an_implementer_that_gave_up_is_a_decision_with_the_sessions_log(
    #[case] work: Implementing,
    #[case] kept: Kept,
    #[case] reason: &str,
) {
    let gave_up = gave_up(
        work,
        "\nToo hard. \n",
        "Read the code.",
        &stopped(kept, TryAgain::Relabel),
    );

    assert_eq!(
        gave_up,
        hand_back_of(
            HandBackKind::Decision,
            reason,
            Some(log(Logged::Session, "Read the code."))
        )
    );
}

#[rstest]
#[case::with_why(
    "Gadgets exist already.",
    "There is nothing to build: the code already does what this issue asks.\n\n\
     Gadgets exist already."
)]
fn an_issue_with_nothing_to_build_is_a_decision(#[case] why: &str, #[case] reason: &str) {
    assert_eq!(
        nothing_to_build(why),
        hand_back_of(HandBackKind::Decision, reason, None)
    );
}

#[rstest]
#[case::a_stages_setting(
    TimeoutSetting::Stage(WorkKind::Implement),
    Some(log(Logged::Session, "Still reading the spec.")),
    hand_back_of(
        HandBackKind::Stall,
        "The session ended unexpectedly: it timed out. The work may need cutting smaller, or \
         `session-timeout` in `[stage.implement]` may need to be longer.",
        Some(log(Logged::Session, "Still reading the spec.")),
    )
)]
#[case::the_pipelines_setting_in_setup(
    TimeoutSetting::Pipeline,
    Some(log(Logged::Setup, "npm install")),
    hand_back_of(
        HandBackKind::Stall,
        "The session ended unexpectedly: it timed out. The work may need cutting smaller, or \
         `session-timeout` may need to be longer.",
        Some(log(Logged::Setup, "npm install")),
    )
)]
#[case::an_automations_setting_with_no_log(
    TimeoutSetting::Automation,
    None,
    hand_back_of(
        HandBackKind::Stall,
        "The session ended unexpectedly: it timed out. The work may need cutting smaller, or \
         `session-timeout` in the automation's front matter may need to be longer.",
        None,
    )
)]
fn a_timed_out_session_stalls(
    #[case] setting: TimeoutSetting,
    #[case] log_tail: Option<LogTail>,
    #[case] expected: HandBack,
) {
    assert_eq!(timed_out(&setting, log_tail), expected);
}

#[rstest]
#[case::with_its_log(
    "Out of memory.\n",
    hand_back_of(
        HandBackKind::Stall,
        "The session ended unexpectedly: it crashed",
        Some(log(Logged::Session, "Out of memory.")),
    )
)]
#[case::with_no_log(
    "",
    hand_back_of(
        HandBackKind::Stall,
        "The session ended unexpectedly: it crashed",
        None
    )
)]
fn a_session_that_ended_unexpectedly_stalls(#[case] session_log: &str, #[case] expected: HandBack) {
    assert_eq!(session_failed("it crashed", session_log), expected);
}

#[rstest]
#[case::with_the_errors_causes(
    "",
    None,
    hand_back_of(
        HandBackKind::Stall,
        "The run failed twice on its infrastructure: cloning: connection reset",
        None,
    )
)]
#[case::with_a_note_and_a_log(
    "\n\nThe work so far is on the branch `7-gadgets`.",
    Some(log(Logged::Setup, "npm ERR!")),
    hand_back_of(
        HandBackKind::Stall,
        "The run failed twice on its infrastructure: cloning: connection reset\n\n\
         The work so far is on the branch `7-gadgets`.",
        Some(log(Logged::Setup, "npm ERR!")),
    )
)]
fn a_run_that_failed_twice_on_its_infrastructure_stalls(
    #[case] note: &str,
    #[case] log_tail: Option<LogTail>,
    #[case] expected: HandBack,
) {
    let error = anyhow::anyhow!("connection reset").context("cloning");

    assert_eq!(failed_twice(&error, note, log_tail), expected);
}

#[rstest]
#[case::with_the_errors_causes("", "Recording the run's outcome failed: commenting: rate limited")]
#[case::with_a_note(
    "\n\nThe work so far is on the branch `7-gadgets`.",
    "Recording the run's outcome failed: commenting: rate limited\n\n\
     The work so far is on the branch `7-gadgets`."
)]
fn a_run_whose_recording_failed_stalls(#[case] note: &str, #[case] reason: &str) {
    let error = anyhow::anyhow!("rate limited").context("commenting");

    assert_eq!(
        recording_failed(&error, note),
        hand_back_of(HandBackKind::Stall, reason, None)
    );
}

#[rstest]
#[case::work_kept_is_as_it_was(Ok(Kept::Pushed), Kept::Pushed)]
#[case::a_refusal_has_the_secret_it_printed_redacted(
    Ok(Kept::Refused("pre-push: GH_TOKEN=ghp_s3cr3t".into())),
    Kept::Refused("pre-push: GH_TOKEN=[redacted]".into())
)]
#[case::a_failure_says_why_with_its_context_and_the_secret_redacted(
    Err(anyhow::anyhow!("pre-push: GH_TOKEN=ghp_s3cr3t").context("pushing 7-gadgets")),
    Kept::Failed("pushing 7-gadgets: pre-push: GH_TOKEN=[redacted]".into())
)]
fn what_a_stopped_runs_keep_gives(#[case] keep: anyhow::Result<Kept>, #[case] expected: Kept) {
    let redaction = crate::common::redaction_of(&[("GH_TOKEN", "ghp_s3cr3t")]);

    assert_eq!(Kept::redacted(keep, &redaction), expected);
}

#[rstest]
#[case::a_first_failure_is_retried(0, true)]
#[case::a_failure_of_the_retry_is_not(1, false)]
fn an_infrastructure_failure_is_retried(#[case] retries: u32, #[case] retried: bool) {
    assert_eq!(retries_infrastructure_failure(retries), retried);
}

/// Sends a result back once more with `follow_ups` against a
/// `result-retries` of one, and says whether it went back to the session.
fn sent_back(follow_ups: &mut FollowUps) -> bool {
    match follow_ups.send_back(1, "Answer with JSON.", "Done.") {
        SendBack::Retry => true,
        SendBack::HandBack(_) => false,
    }
}

#[test]
fn the_answer_to_a_new_question_has_follow_ups_of_its_own() {
    let mut follow_ups = FollowUps::default();
    assert!(sent_back(&mut follow_ups));

    follow_ups.resumed();

    assert!(sent_back(&mut follow_ups));
    assert!(!sent_back(&mut follow_ups));
}
