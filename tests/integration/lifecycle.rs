use crate::common;

use common::*;
use linewright::board::Column;
use linewright::domain::*;
use linewright::lifecycle::{
    Cancel, IssueFacts, IssueStage, Opener, PullClaim, PullFacts, PullRow, PullWorking, Row, State,
    TakeUp, cancels, done_by_merge, issue_state, merged_row, pull_row, pull_state, row,
};
use linewright::reconcile::{Discard, NotSpecChange, Policy, WorkKind};

/// Issue #1, opened by the maintainer, carrying `labels`, each applied at
/// 09:00 on 2026-10-01, and assigned to `assignees`.
fn issue(labels: &[&str], assignees: &[&str]) -> Issue {
    Issue {
        labels: labels.iter().map(|&label| label.into()).collect(),
        assignees: logins(assignees),
        events: labels
            .iter()
            .map(|&label| IssueEvent::Labeled {
                label: label.into(),
                by: Some(Login::new("maintainer")),
                at: at("2026-10-01T09:00:00Z"),
            })
            .collect(),
        ..ticket(1, "Add invoices")
    }
}

/// Pull request #`number`, by alice, into `main`, that closes #1, in
/// `state`.
fn closing(number: u64, state: PullState) -> ClosingPull {
    ClosingPull {
        number: Number(number),
        author: Login::new("alice"),
        state,
        updated_at: at("2026-10-01T09:00:00Z"),
        base_ref: "main".into(),
    }
}

fn merged_at(time: &str) -> PullState {
    PullState::Merged { at: at(time) }
}

const OPEN: PullState = PullState::Open { draft: false };

fn state_of(issue: &Issue, closing: &[ClosingPull], run: Option<WorkKind>) -> Option<State> {
    let merge = done_by_merge(&issue.events, closing, "main");
    issue_state(issue, closing, merge, run, &linewright())
}

/// A case: what it is, the issue, the pull requests that close it, the run in
/// flight on it and its state.
type Case = (
    &'static str,
    Issue,
    Vec<ClosingPull>,
    Option<WorkKind>,
    Option<State>,
);

#[test_log::test]
fn an_issue_s_state_is_the_first_row_of_the_state_table_that_holds() {
    let tracking = Issue {
        sub_issues: vec![sub_issue(2, true, LINEWRIGHT)],
        ..issue(&[label::DESIGNED], &[])
    };
    let cases: Vec<Case> = vec![
        (
            "closed",
            Issue {
                open: false,
                ..issue(&["ready-for-agent"], &[])
            },
            vec![],
            None,
            Some(State::Done),
        ),
        (
            "a pull request closing it merged since it was labeled",
            issue(&["ready-for-agent"], &[LINEWRIGHT]),
            vec![closing(2, merged_at("2026-10-01T10:00:00Z"))],
            None,
            Some(State::Done),
        ),
        (
            "an open pull request closes it",
            issue(&["ready-for-agent"], &[LINEWRIGHT]),
            vec![closing(2, OPEN)],
            None,
            Some(State::InReview),
        ),
        (
            "a tracking issue",
            tracking,
            vec![],
            None,
            Some(State::Tracking),
        ),
        (
            "needs-triage, assigned to Linewright",
            issue(&["needs-triage"], &[LINEWRIGHT]),
            vec![],
            None,
            Some(State::BeingTriaged),
        ),
        (
            "ready-for-agent, assigned to Linewright, not designed",
            issue(&["ready-for-agent"], &[LINEWRIGHT]),
            vec![],
            None,
            Some(State::BeingDesigned),
        ),
        (
            "ready-for-agent, assigned to Linewright, designed",
            issue(&["ready-for-agent", label::DESIGNED], &[LINEWRIGHT]),
            vec![],
            None,
            Some(State::BeingImplemented),
        ),
        (
            "needs-triage",
            issue(&["needs-triage"], &[]),
            vec![],
            None,
            Some(State::NeedsTriage),
        ),
        (
            "needs-info, assigned to Linewright",
            issue(&["needs-info"], &[LINEWRIGHT]),
            vec![],
            None,
            Some(State::NeedsInfo),
        ),
        (
            "ready-for-human",
            issue(&["ready-for-human"], &[]),
            vec![],
            None,
            Some(State::ReadyForHuman),
        ),
        (
            "ready-for-agent, assigned to someone else",
            issue(&["ready-for-agent"], &["alice"]),
            vec![],
            None,
            Some(State::ReadyForAgent),
        ),
        (
            "no status label, a triage run in flight",
            issue(&[], &[LINEWRIGHT]),
            vec![],
            Some(WorkKind::Triage),
            Some(State::BeingTriaged),
        ),
        (
            "no status label, a design run in flight",
            issue(&[], &[LINEWRIGHT]),
            vec![],
            Some(WorkKind::Design),
            Some(State::BeingDesigned),
        ),
        (
            "no status label, an implement run in flight",
            issue(&[], &[]),
            vec![],
            Some(WorkKind::Implement),
            Some(State::BeingImplemented),
        ),
        (
            "no status label, no run, a pull request closing it closed unmerged",
            issue(&[], &[LINEWRIGHT]),
            vec![closing(
                2,
                PullState::Closed {
                    at: at("2026-10-01T10:00:00Z"),
                },
            )],
            None,
            None,
        ),
    ];

    for (case, issue, closing, run, expected) in cases {
        assert_eq!(state_of(&issue, &closing, run), expected, "{case}");
    }
}

#[test_log::test]
fn an_issue_labeled_needs_triage_assigned_to_linewright_with_an_open_pull_request_closing_it_is_in_review()
 {
    let issue = issue(&["needs-triage"], &[LINEWRIGHT]);

    assert_eq!(
        state_of(&issue, &[closing(2, OPEN)], Some(WorkKind::Triage)),
        Some(State::InReview)
    );
}

#[test_log::test]
fn an_issue_relabeled_after_a_merge_is_not_done() {
    let merged = [closing(2, merged_at("2026-10-01T08:00:00Z"))];
    let relabeled = issue(&["ready-for-agent"], &[]);
    let reopened = Issue {
        labels: vec![],
        events: vec![IssueEvent::Reopened {
            at: at("2026-10-01T09:00:00Z"),
        }],
        ..issue(&[], &[])
    };
    let unlabeled_since = Issue {
        events: vec![IssueEvent::Labeled {
            label: "ready-for-agent".into(),
            by: Some(Login::new("maintainer")),
            at: at("2026-10-01T07:00:00Z"),
        }],
        ..issue(&["ready-for-agent"], &[])
    };

    assert_eq!(
        state_of(&relabeled, &merged, None),
        Some(State::ReadyForAgent)
    );
    assert_eq!(state_of(&reopened, &merged, None), None);
    assert_eq!(state_of(&unlabeled_since, &merged, None), Some(State::Done));
}

#[test_log::test]
fn a_merge_into_another_branch_does_not_make_an_issue_done() {
    let into_a_release = ClosingPull {
        base_ref: "release".into(),
        ..closing(2, merged_at("2026-10-01T10:00:00Z"))
    };

    assert_eq!(
        state_of(&issue(&["ready-for-agent"], &[]), &[into_a_release], None),
        Some(State::ReadyForAgent)
    );
}

#[test_log::test]
fn a_pull_request_is_open_merged_or_closed() {
    assert_eq!(pull_state(&PullState::Open { draft: true }), State::Open);
    assert_eq!(
        pull_state(&merged_at("2026-10-01T10:00:00Z")),
        State::Merged
    );
    assert_eq!(
        pull_state(&PullState::Closed {
            at: at("2026-10-01T10:00:00Z")
        }),
        State::Closed
    );
}

#[test_log::test]
fn each_state_has_its_words_and_its_name_in_json() {
    let expected = [
        (State::NeedsTriage, "Waiting for triage", "needs-triage"),
        (State::BeingTriaged, "Being triaged", "being-triaged"),
        (State::NeedsInfo, "Waiting for an answer", "needs-info"),
        (
            State::ReadyForHuman,
            "Waiting for a decision",
            "ready-for-human",
        ),
        (
            State::ReadyForAgent,
            "Waiting for Linewright",
            "ready-for-agent",
        ),
        (State::BeingDesigned, "Being designed", "being-designed"),
        (
            State::BeingImplemented,
            "Being implemented",
            "being-implemented",
        ),
        (State::InReview, "In review", "in-review"),
        (State::Tracking, "Tracking", "tracking"),
        (State::Done, "Done", "done"),
        (State::Open, "Open", "open"),
        (State::Merged, "Merged", "merged"),
        (State::Closed, "Closed", "closed"),
    ];

    for (state, words, json) in expected {
        assert_eq!(state.shown(), words);
        assert_eq!(serde_json::to_value(state).unwrap(), json);
    }
}

/// `docs/specs/design_system.md`, The board: a card is in its state's
/// column, and a tracking issue has no card.
#[test_log::test]
fn each_state_s_card_is_in_the_column_the_board_gives_it() {
    let expected = [
        (State::NeedsTriage, Some(Column::Triage)),
        (State::NeedsInfo, Some(Column::NeedsAPerson)),
        (State::ReadyForHuman, Some(Column::NeedsAPerson)),
        (State::ReadyForAgent, Some(Column::Ready)),
        (State::BeingTriaged, Some(Column::Working)),
        (State::BeingDesigned, Some(Column::Working)),
        (State::BeingImplemented, Some(Column::Working)),
        (State::InReview, Some(Column::InReview)),
        (State::Open, Some(Column::InReview)),
        (State::Done, Some(Column::Done)),
        (State::Merged, Some(Column::Done)),
        (State::Closed, Some(Column::Done)),
        (State::Tracking, None),
    ];

    for (state, column) in expected {
        assert_eq!(state.column(), column, "{state:?}");
    }
}

/// Linewright's pull request #`number`, that closes #1, in `state`.
fn linewrights(number: u64, state: PullState) -> ClosingPull {
    ClosingPull {
        author: linewright(),
        ..closing(number, state)
    }
}

fn closed_at(time: &str) -> PullState {
    PullState::Closed { at: at(time) }
}

/// `issue` with `assignee` assigned by `actor` at 09:30.
fn assigned(issue: Issue, assignee: &str, actor: &str) -> Issue {
    let mut issue = issue;
    if !issue.assignees.contains(&Login::new(assignee)) {
        issue.assignees.push(Login::new(assignee));
    }
    issue
        .events
        .push(IssueEvent::Assignment(Assignment::Assigned {
            assignee: Login::new(assignee),
            actor: Login::new(actor),
            at: at("2026-10-01T09:30:00Z"),
        }));
    issue
}

fn commented(issue: Issue, author: &str, time: &str) -> Issue {
    let mut issue = issue;
    issue.comments.push(Comment {
        author: Login::new(author),
        body: "A comment.".into(),
        created_at: at(time),
    });
    issue
}

/// An issue Linewright cut into ticket #2, still open: a tracking issue
/// while it carries `linewright:designed`.
fn cut(labels: &[&str], assignees: &[&str]) -> Issue {
    Issue {
        sub_issues: vec![sub_issue(2, true, LINEWRIGHT)],
        ..issue(labels, assignees)
    }
}

/// The row of the Working an item table that holds for `issue`, closed by
/// `closing`, with nothing in flight on it.
fn row_of(issue: &Issue, closing: &[ClosingPull]) -> Option<Row> {
    row_with(issue, closing, false)
}

fn row_with(issue: &Issue, closing: &[ClosingPull], run_in_flight: bool) -> Option<Row> {
    let policy = policy();
    let state = state_of(issue, closing, None);
    let discard = discard_of(issue);
    let facts = IssueFacts {
        issue,
        closing,
        run_in_flight,
        discard: discard.as_ref(),
    };
    row(&facts, state, &policy)
}

/// The discard of `issue`'s cut, while a ticket cut from it is among its
/// sub-issues.
fn discard_of(issue: &Issue) -> Option<Discard> {
    let tickets: Vec<Linked> = issue
        .tickets_cut(&linewright())
        .map(SubIssue::linked)
        .collect();
    (!tickets.is_empty()).then(|| Discard {
        issue: issue.number,
        pulls: vec![],
        tickets,
    })
}

/// `docs/specs/repository.md`, Working an item: each row of the table, other
/// than those for how a run ends and for spec changes.
#[test_log::test]
fn an_issue_is_worked_by_the_first_row_of_the_working_an_item_table_that_holds() {
    let tracking_with = |labels: &[&str], assignees: &[&str]| {
        let mut labels = labels.to_vec();
        labels.push(label::DESIGNED);
        cut(&labels, assignees)
    };
    let all_closed = Issue {
        sub_issues: vec![
            sub_issue(2, false, LINEWRIGHT),
            sub_issue(3, false, "alice"),
        ],
        ..tracking_with(&[], &[])
    };
    let asked = commented(
        issue(&["needs-info"], &[]),
        LINEWRIGHT,
        "2026-10-01T09:10:00Z",
    );
    let cases: Vec<(&str, Issue, Vec<ClosingPull>, Option<Row>)> = vec![
        (
            "a cut without linewright:designed",
            cut(&["ready-for-agent"], &[]),
            vec![],
            discard_of(&cut(&["ready-for-agent"], &[])).map(Row::DiscardCut),
        ),
        (
            "being implemented, Linewright's pull request closed unmerged since labeled",
            issue(&["ready-for-agent", label::DESIGNED], &[LINEWRIGHT]),
            vec![linewrights(5, closed_at("2026-10-01T10:00:00Z"))],
            Some(Row::HandBack { pull: Number(5) }),
        ),
        (
            "being designed, Linewright's pull request closed unmerged since labeled",
            issue(&["ready-for-agent"], &[LINEWRIGHT]),
            vec![linewrights(5, closed_at("2026-10-01T10:00:00Z"))],
            Some(Row::HandBack { pull: Number(5) }),
        ),
        (
            "needs-info, Linewright assigned by itself",
            assigned(issue(&["needs-info"], &[]), LINEWRIGHT, LINEWRIGHT),
            vec![],
            Some(Row::LetGo { comment: None }),
        ),
        (
            "ready-for-human, Linewright assigned by itself",
            assigned(issue(&["ready-for-human"], &[]), LINEWRIGHT, LINEWRIGHT),
            vec![],
            Some(Row::LetGo { comment: None }),
        ),
        (
            "no status label, Linewright assigned by itself",
            assigned(issue(&[], &[]), LINEWRIGHT, LINEWRIGHT),
            vec![],
            Some(Row::LetGo { comment: None }),
        ),
        (
            "needs-triage, a person Linewright asked assigned",
            assigned(issue(&["needs-triage"], &[]), "alice", LINEWRIGHT),
            vec![],
            Some(Row::UnassignAsked {
                people: logins(&["alice"]),
            }),
        ),
        (
            "ready-for-agent, a person Linewright asked assigned",
            assigned(issue(&["ready-for-agent"], &[]), "alice", LINEWRIGHT),
            vec![],
            Some(Row::UnassignAsked {
                people: logins(&["alice"]),
            }),
        ),
        (
            "tracking, holding a status label and Linewright's assignment",
            tracking_with(&["ready-for-agent"], &[LINEWRIGHT]),
            vec![],
            Some(Row::CleanUpTracking {
                labels: vec![StatusLabel::ReadyForAgent],
                let_go: true,
            }),
        ),
        (
            "tracking, assigned to Linewright alone",
            tracking_with(&[], &[LINEWRIGHT]),
            vec![],
            Some(Row::CleanUpTracking {
                labels: vec![],
                let_go: true,
            }),
        ),
        (
            "tracking, every sub-issue closed",
            all_closed,
            vec![],
            Some(Row::CloseTracking),
        ),
        (
            "tracking, a sub-issue open",
            tracking_with(&[], &[]),
            vec![],
            None,
        ),
        (
            "needs-info, a reply came",
            commented(asked.clone(), "reporter", "2026-10-01T09:20:00Z"),
            vec![],
            Some(Row::TakeReply),
        ),
        ("needs-info, no reply", asked, vec![], None),
        (
            "needs-triage, unassigned",
            issue(&["needs-triage"], &[]),
            vec![],
            Some(Row::Claim),
        ),
        (
            "ready-for-agent, unassigned",
            issue(&["ready-for-agent"], &[]),
            vec![],
            Some(Row::Claim),
        ),
        (
            "being triaged",
            issue(&["needs-triage"], &[LINEWRIGHT]),
            vec![],
            Some(Row::Start(IssueStage::Triage)),
        ),
        (
            "being designed",
            issue(&["ready-for-agent"], &[LINEWRIGHT]),
            vec![],
            Some(Row::Start(IssueStage::Design)),
        ),
        (
            "being implemented",
            issue(&["ready-for-agent", label::DESIGNED], &[LINEWRIGHT]),
            vec![],
            Some(Row::Start(IssueStage::Implement)),
        ),
        (
            "needs-info, carrying needs-triage from before",
            relabeled(issue(&[], &[]), "needs-triage", "needs-info"),
            vec![],
            Some(Row::RemoveStaleStatusLabels(vec![StatusLabel::NeedsTriage])),
        ),
        (
            "in review, stalled, Linewright assigned",
            issue(
                &["ready-for-agent", label::DESIGNED, label::STALLED],
                &[LINEWRIGHT],
            ),
            vec![linewrights(5, OPEN)],
            Some(Row::ClearStall),
        ),
        (
            "in review",
            issue(&["ready-for-agent", label::DESIGNED], &[LINEWRIGHT]),
            vec![linewrights(5, OPEN)],
            None,
        ),
    ];

    for (case, issue, closing, expected) in cases {
        assert_eq!(row_of(&issue, &closing), expected, "{case}");
    }
}

#[test_log::test]
fn where_two_rows_hold_the_first_is_taken() {
    let asked = commented(
        assigned(issue(&["needs-info"], &[]), LINEWRIGHT, LINEWRIGHT),
        LINEWRIGHT,
        "2026-10-01T09:40:00Z",
    );
    let replied = commented(asked, "reporter", "2026-10-01T09:50:00Z");
    let tracking_done = Issue {
        sub_issues: vec![sub_issue(2, false, LINEWRIGHT)],
        ..cut(&["ready-for-agent", label::DESIGNED], &[])
    };
    let discarded = Issue {
        sub_issues: vec![sub_issue(2, true, LINEWRIGHT)],
        ..assigned(issue(&["needs-info"], &[]), LINEWRIGHT, LINEWRIGHT)
    };
    let cases: Vec<(&str, Issue, Vec<ClosingPull>, Row)> = vec![
        (
            "a cut to discard, and Linewright assigned to a needs-info issue",
            discarded.clone(),
            vec![],
            Row::DiscardCut(discard_of(&discarded).unwrap()),
        ),
        (
            "Linewright's pull request closed unmerged, and stalled",
            issue(
                &["ready-for-agent", label::DESIGNED, label::STALLED],
                &[LINEWRIGHT],
            ),
            vec![linewrights(5, closed_at("2026-10-01T10:00:00Z"))],
            Row::HandBack { pull: Number(5) },
        ),
        (
            "a reply, and Linewright assigned to the needs-info issue",
            replied,
            vec![],
            Row::LetGo { comment: None },
        ),
        (
            "a tracking issue holding a status label, every sub-issue closed",
            tracking_done,
            vec![],
            Row::CleanUpTracking {
                labels: vec![StatusLabel::ReadyForAgent],
                let_go: false,
            },
        ),
        (
            "being triaged, carrying ready-for-agent from before, and stalled",
            {
                let mut stalled =
                    relabeled(issue(&[], &[LINEWRIGHT]), "ready-for-agent", "needs-triage");
                stalled.labels.push(label::STALLED.into());
                stalled
            },
            vec![],
            Row::Start(IssueStage::Triage),
        ),
    ];

    for (case, issue, closing, expected) in cases {
        assert_eq!(row_of(&issue, &closing), Some(expected), "{case}");
    }
}

#[test_log::test]
fn a_reply_to_a_question_opening_with_the_old_marker_line_is_taken() {
    let asked = Issue {
        comments: vec![
            Comment {
                author: linewright(),
                body: "> *Linewright, triage*\n\nWhat version are you on?".into(),
                created_at: at("2026-10-01T09:10:00Z"),
            },
            Comment {
                author: Login::new("maintainer"),
                body: "Version 3, I checked.".into(),
                created_at: at("2026-10-01T09:20:00Z"),
            },
        ],
        ..issue(&["needs-info"], &[])
    };

    assert_eq!(row_of(&asked, &[]), Some(Row::TakeReply));
}

#[test_log::test]
fn linewright_lets_go_of_an_issue_a_person_assigned_it_to_with_a_comment() {
    let by_hand = assigned(issue(&["ready-for-human"], &[]), LINEWRIGHT, "maintainer");

    assert!(matches!(
        row_of(&by_hand, &[]),
        Some(Row::LetGo { comment: Some(_) })
    ));
}

#[test_log::test]
fn a_pull_request_a_person_closed_before_the_issue_was_labeled_again_hands_nothing_back() {
    let relabeled = Issue {
        events: vec![IssueEvent::Labeled {
            label: "ready-for-agent".into(),
            by: Some(Login::new("maintainer")),
            at: at("2026-10-01T11:00:00Z"),
        }],
        ..issue(&["ready-for-agent", label::DESIGNED], &[LINEWRIGHT])
    };
    let someone_elses = issue(&["ready-for-agent", label::DESIGNED], &[LINEWRIGHT]);

    assert_eq!(
        row_of(
            &relabeled,
            &[linewrights(5, closed_at("2026-10-01T10:00:00Z"))]
        ),
        Some(Row::Start(IssueStage::Implement))
    );
    assert_eq!(
        row_of(
            &someone_elses,
            &[closing(5, closed_at("2026-10-01T10:00:00Z"))]
        ),
        Some(Row::Start(IssueStage::Implement))
    );
}

/// `docs/specs/repository.md`, Working an item: "Claiming" and "Blocked".
#[test_log::test]
fn an_issue_is_claimed_only_unassigned_marked_unblocked_and_not_unassigned_since_labeled() {
    let blocked = Issue {
        blocked_by: vec![Linked {
            number: Number(9),
            open: true,
        }],
        ..issue(&["ready-for-agent"], &[])
    };
    let unassigned_since = Issue {
        events: vec![
            IssueEvent::Labeled {
                label: "ready-for-agent".into(),
                by: Some(Login::new("maintainer")),
                at: at("2026-10-01T09:00:00Z"),
            },
            unassigning_linewright("2026-10-01T10:00:00Z"),
        ],
        ..issue(&["ready-for-agent"], &[])
    };
    let blocked_and_claimed = Issue {
        assignees: logins(&[LINEWRIGHT]),
        ..blocked.clone()
    };

    assert_eq!(row_of(&blocked, &[]), None);
    assert_eq!(row_of(&unassigned_since, &[]), None);
    assert_eq!(row_of(&issue(&["needs-triage"], &["alice"]), &[]), None);
    assert_eq!(row_of(&blocked_and_claimed, &[]), None);
    let unmarked = Policy {
        require_label: Some(REQUIRED.into()),
        ..policy()
    };
    let unmarked_issue = issue(&["needs-triage"], &[]);
    let facts = IssueFacts {
        issue: &unmarked_issue,
        closing: &[],
        run_in_flight: false,
        discard: None,
    };
    assert_eq!(row(&facts, Some(State::NeedsTriage), &unmarked), None);
}

#[test_log::test]
fn while_a_run_is_in_flight_no_row_holds() {
    let claimed = issue(
        &["ready-for-agent", label::DESIGNED, label::STALLED],
        &[LINEWRIGHT],
    );

    assert_eq!(row_with(&claimed, &[], true), None);
}

/// `docs/specs/repository.md`, Steering: closing the issue, changing or
/// removing its status label and unassigning Linewright cancel a run the
/// issue no longer asks for.
#[test_log::test]
fn a_run_is_cancelled_as_the_steering_table_says() {
    let designed = &["ready-for-agent", label::DESIGNED];
    let cases: Vec<(&str, WorkKind, Issue, Option<Cancel>)> = vec![
        (
            "the issue closed",
            WorkKind::Implement,
            Issue {
                open: false,
                ..issue(designed, &[LINEWRIGHT])
            },
            Some(Cancel::Closed),
        ),
        (
            "needs-triage relabeled ready-for-agent during triage",
            WorkKind::Triage,
            issue(&["ready-for-agent"], &[LINEWRIGHT]),
            Some(Cancel::Relabeled(StatusLabel::NeedsTriage)),
        ),
        (
            "ready-for-agent relabeled needs-info during implementation",
            WorkKind::Implement,
            issue(&["needs-info"], &[LINEWRIGHT]),
            Some(Cancel::Relabeled(StatusLabel::ReadyForAgent)),
        ),
        (
            "the status label removed during design",
            WorkKind::Design,
            issue(&[], &[LINEWRIGHT]),
            Some(Cancel::Relabeled(StatusLabel::ReadyForAgent)),
        ),
        (
            "Linewright unassigned",
            WorkKind::Implement,
            issue(designed, &[]),
            Some(Cancel::Unassigned),
        ),
        (
            "nothing changed",
            WorkKind::Triage,
            issue(&["needs-triage"], &[LINEWRIGHT]),
            None,
        ),
    ];

    for (case, stage, issue, expected) in cases {
        assert_eq!(cancels(stage, &issue, &linewright()), expected, "{case}");
    }
}

#[test_log::test]
fn a_person_changing_linewright_designed_during_a_design_or_implement_run_cancels_neither() {
    let designed = issue(&["ready-for-agent", label::DESIGNED], &[LINEWRIGHT]);
    let undesigned = issue(&["ready-for-agent"], &[LINEWRIGHT]);

    assert_eq!(cancels(WorkKind::Design, &designed, &linewright()), None);
    assert_eq!(
        cancels(WorkKind::Implement, &undesigned, &linewright()),
        None
    );
}

/// A pull request opened by `opener`, with Linewright's assignment `claim`
/// and its work `working`.
fn pull_facts(opener: Opener, claim: PullClaim, working: PullWorking) -> PullFacts {
    PullFacts {
        opener,
        claim,
        working,
        stalled: false,
    }
}

/// `docs/specs/repository.md`, Working an item and Steering: the rows of an
/// open pull request, other than those for how a run ends.
#[test_log::test]
fn a_pull_request_is_worked_by_the_first_row_that_holds() {
    use Opener::{Linewright, Person};
    use PullClaim::{Claimed, NeverAssigned, Unassigned};
    use PullWorking::{Asked, RunInFlight, Waiting};
    let stalled = |facts: PullFacts| PullFacts {
        stalled: true,
        ..facts
    };
    let cases: Vec<(&str, PullFacts, State, Option<PullRow>)> = vec![
        (
            "Linewright's, open, never assigned",
            pull_facts(Linewright, NeverAssigned, Asked),
            State::Open,
            Some(PullRow::Claim),
        ),
        (
            "Linewright's, open, unassigned from it",
            pull_facts(Linewright, Unassigned, Asked),
            State::Open,
            None,
        ),
        (
            "Linewright's, open, assigned, asking for a run or a merge",
            pull_facts(Linewright, Claimed, Asked),
            State::Open,
            Some(PullRow::Work),
        ),
        (
            "Linewright's, open, assigned, asking for a run, and stalled",
            stalled(pull_facts(Linewright, Claimed, Asked)),
            State::Open,
            Some(PullRow::Work),
        ),
        (
            "Linewright's, open, assigned, waiting, and stalled",
            stalled(pull_facts(Linewright, Claimed, Waiting)),
            State::Open,
            Some(PullRow::ClearStall),
        ),
        (
            "Linewright's, open, unassigned from it, and stalled",
            stalled(pull_facts(Linewright, Unassigned, Waiting)),
            State::Open,
            None,
        ),
        (
            "someone else's, open, Linewright assigned",
            pull_facts(Person, Claimed, Waiting),
            State::Open,
            Some(PullRow::LetGo {
                comment: format!(
                    "@{LINEWRIGHT} works only on the pull requests it opens and on spec \
                     changes, so it has let go of this one."
                ),
            }),
        ),
        (
            "someone else's, open, Linewright not assigned",
            pull_facts(Person, Unassigned, Waiting),
            State::Open,
            None,
        ),
        (
            "Linewright's, a run in flight",
            stalled(pull_facts(Linewright, NeverAssigned, RunInFlight)),
            State::Open,
            None,
        ),
    ];

    for (case, facts, state, expected) in cases {
        assert_eq!(pull_row(&facts, state, &linewright()), expected, "{case}");
    }
}

/// A merged pull request's case: its name, the spec change Linewright may
/// take up or why not, whether Linewright is assigned, and the row expected.
type MergedCase = (
    &'static str,
    Result<(), NotSpecChange>,
    bool,
    Option<TakeUp>,
);

/// `docs/specs/repository.md`, Working an item and Steering: the rows of a
/// merged pull request a person opened, a spec change Linewright may take up
/// or why not, with Linewright assigned to it or not. Only taking up a spec
/// change holds for one: any other is left as it is, whoever is assigned.
#[test_log::test]
fn a_merged_pull_request_is_worked_by_the_first_row_that_holds() {
    let before_lookback = NotSpecChange::BeforeLookback {
        merged_at: at("2026-09-01T09:00:00Z"),
        window_opened: at("2026-09-24T09:00:00Z"),
    };
    let unmarked = NotSpecChange::RequiredLabel {
        label: "from-linewright".into(),
    };
    let cases: Vec<MergedCase> = vec![
        (
            "a spec change Linewright may take up",
            Ok(()),
            false,
            Some(TakeUp { let_go: false }),
        ),
        (
            "a spec change Linewright may take up, Linewright assigned",
            Ok(()),
            true,
            Some(TakeUp { let_go: true }),
        ),
        (
            "a spec change before the lookback window",
            Err(before_lookback.clone()),
            false,
            None,
        ),
        (
            "a spec change before the lookback window, Linewright assigned",
            Err(before_lookback),
            true,
            Some(TakeUp { let_go: true }),
        ),
        (
            "a spec change not marked for Linewright",
            Err(unmarked.clone()),
            false,
            None,
        ),
        (
            "a spec change not marked for Linewright, Linewright assigned",
            Err(unmarked),
            true,
            Some(TakeUp { let_go: true }),
        ),
        (
            "a spec change taken up by its issue",
            Err(NotSpecChange::TakenUp { issue: Number(9) }),
            false,
            None,
        ),
        (
            "a spec change taken up by its issue, Linewright assigned",
            Err(NotSpecChange::TakenUp { issue: Number(9) }),
            true,
            None,
        ),
        (
            "a spec change carrying the design label, Linewright assigned",
            Err(NotSpecChange::Designed {
                label: DesignedLabel::Designed,
            }),
            true,
            None,
        ),
        (
            "no spec change, Linewright assigned",
            Err(NotSpecChange::NoSpecFile {
                spec_glob: "docs/**".into(),
            }),
            true,
            None,
        ),
        (
            "no spec change",
            Err(NotSpecChange::OtherBranch {
                base: "release".into(),
            }),
            false,
            None,
        ),
        (
            "Linewright's, Linewright assigned",
            Err(NotSpecChange::Linewrights),
            true,
            None,
        ),
    ];

    for (case, as_spec_change, assigned, expected) in cases {
        assert_eq!(merged_row(&as_spec_change, assigned), expected, "{case}");
    }
}
