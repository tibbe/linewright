use crate::common;

use common::*;
use linewright::board::Column;
use linewright::domain::*;
use linewright::lifecycle::{State, done_by_merge, issue_state, pull_state};
use linewright::reconcile::WorkKind;

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

#[test]
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

#[test]
fn an_issue_labeled_needs_triage_assigned_to_linewright_with_an_open_pull_request_closing_it_is_in_review()
 {
    let issue = issue(&["needs-triage"], &[LINEWRIGHT]);

    assert_eq!(
        state_of(&issue, &[closing(2, OPEN)], Some(WorkKind::Triage)),
        Some(State::InReview)
    );
}

#[test]
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

#[test]
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

#[test]
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

#[test]
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
#[test]
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
