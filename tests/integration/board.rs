use crate::common;

use common::{
    NOW, assigning_linewright, at, labeled_by_linewright, ticket, unassigning_linewright, view,
};
use linewright::board::{
    Board, Card, CardLine, CardStatus, Column, Elapsed, Ended, Hold, LaneHead, Running, Stop, board,
};
use linewright::domain::*;
use linewright::reconcile::WorkKind;
use linewright::runs::{Ending, Recorded, Started, State};
use linewright::status::View;

/// Where each card is: its lane's number, its column and its number, lane
/// by lane and column by column.
fn placed(board: &Board) -> Vec<(u64, Column, u64)> {
    board
        .lanes
        .iter()
        .flat_map(|lane| {
            let head = lane.head.number().0;
            Column::ALL.into_iter().flat_map(move |column| {
                lane.cards(column)
                    .iter()
                    .map(move |card| (head, column, card.number.0))
            })
        })
        .collect()
}

/// Each lane's head, lane by lane.
fn heads(board: &Board) -> Vec<LaneHead> {
    board.lanes.iter().map(|lane| lane.head.clone()).collect()
}

/// Ticket #`number`, which Linewright labeled `labels`.
fn labeled(number: u64, labels: &[&str]) -> Issue {
    labeled_by_linewright(Issue {
        labels: labels.iter().map(ToString::to_string).collect(),
        ..ticket(number, &format!("Ticket {number}"))
    })
}

#[test]
fn each_status_label_puts_its_issue_in_its_column() {
    let view = view(
        vec![
            labeled(1, &["needs-triage"]),
            labeled(2, &["needs-info"]),
            labeled(3, &["ready-for-human"]),
            labeled(4, &["ready-for-agent"]),
            labeled(5, &["ready-for-agent", label::DESIGNED]),
        ],
        vec![],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        placed(&board),
        [
            (1, Column::Triage, 1),
            (2, Column::NeedsAPerson, 2),
            (3, Column::NeedsAPerson, 3),
            (4, Column::Ready, 4),
            (5, Column::Ready, 5),
        ]
    );
    assert_eq!(board.needing_a_person, 2);
}

#[test]
fn an_issue_a_person_filed_that_linewright_labeled_in_a_hand_back_needs_a_person() {
    let handed_back = Issue {
        events: vec![IssueEvent::Labeled {
            label: "ready-for-human".into(),
            by: Some(Login::new("factory")),
            at: at("2026-10-01T10:00:00Z"),
        }],
        ..unworked(1, "ready-for-human")
    };
    let view = view(
        vec![
            handed_back,
            unworked(2, "needs-info"),
            unworked(3, "ready-for-human"),
        ],
        vec![],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(placed(&board), [(1, Column::NeedsAPerson, 1)]);
    assert_eq!(board.needing_a_person, 1);
}

/// Ticket #`number`, which a person labeled `label`, and Linewright has not
/// worked.
fn unworked(number: u64, label: &str) -> Issue {
    Issue {
        labels: vec![label.into()],
        ..ticket(number, &format!("Ticket {number}"))
    }
}

fn assigned(issue: Issue) -> Issue {
    Issue {
        assignees: vec![Login::new("factory")],
        ..issue
    }
}

/// Linewright's open pull request #`number` closing each of `closes`.
fn pull(number: u64, closes: &[u64], checks: &[CheckState]) -> Pull {
    Pull {
        number: Number(number),
        title: format!("Pull {number}"),
        body: String::new(),
        author: Login::new("factory"),
        created_at: at("2026-10-01T11:58:00Z"),
        updated_at: at("2026-10-01T11:58:00Z"),
        base_ref: "main".into(),
        head_ref: format!("{number}-pull"),
        head_sha: "abc123".into(),
        state: PullState::Open { draft: false },
        closes: closes.iter().copied().map(Number).collect(),
        pushes: vec![],
        mergeability: Mergeability::Unknown,
        rebaseability: Rebaseability::Unknown,
        checks: checks
            .iter()
            .map(|&state| Check {
                name: "test".into(),
                state,
            })
            .collect(),
        comments: vec![],
        reviews: vec![],
        threads: vec![],
        labels: vec![],
        assignees: vec![Login::new("factory")],
        labelings: vec![],
        assignments: vec![],
        review_decision: None,
    }
}

fn with_runs(mut view: View, ids: &[&str]) -> View {
    view.in_flight = ids
        .iter()
        .map(|id| common::in_flight(id, "2026-10-01T11:50:00Z"))
        .collect();
    view
}

fn with_closed(mut view: View, closed: Vec<ClosedItem>) -> View {
    std::sync::Arc::make_mut(&mut view.snapshot).closed = closed;
    view
}

/// What a ticket Linewright claimed holds of how it was worked.
fn claimed() -> WorkFacts {
    WorkFacts {
        assignees: vec![Login::new("factory")],
        events: vec![assigning_linewright("2026-09-20T10:00:00Z")],
        ..WorkFacts::default()
    }
}

/// Ticket #`number`, which Linewright claimed, closed at `closed_at`.
fn closed_issue(number: u64, closed_at: &str, merged_by: Option<u64>) -> ClosedItem {
    ClosedItem {
        number: Number(number),
        title: format!("Ticket {number}"),
        closed_at: at(closed_at),
        updated_at: at(closed_at),
        kind: ClosedKind::Issue {
            parent: None,
            facts: ManagedFacts {
                filed: None,
                sub_issues: vec![],
                labels: vec!["ready-for-agent".into(), label::DESIGNED.into()],
                work: claimed(),
            },
            merged_by: merged_by.map(Number),
            reason: None,
        },
    }
}

#[test]
fn a_claimed_ticket_and_one_with_a_triage_or_design_run_in_flight_are_in_working() {
    let view = with_runs(
        view(
            vec![
                assigned(ticket(1, "Claimed")),
                labeled(2, &["needs-triage"]),
                labeled(3, &["ready-for-agent"]),
            ],
            vec![],
        ),
        &["2/triage-1", "3/design-1"],
    );

    assert_eq!(
        placed(&board(&view, &[], at(NOW))),
        [
            (2, Column::Working, 2),
            (3, Column::Working, 3),
            (1, Column::Working, 1),
        ]
    );
}

#[test]
fn a_ticket_with_an_open_pull_request_is_in_review() {
    let view = view(
        vec![assigned(ticket(1, "Reviewed"))],
        vec![pull(10, &[1], &[CheckState::Passed])],
    );

    assert_eq!(
        placed(&board(&view, &[], at(NOW))),
        [(1, Column::InReview, 1)]
    );
}

#[test]
fn a_ticket_closed_within_the_done_window_is_in_done_and_one_closed_before_is_nowhere() {
    let view = with_closed(
        view(vec![], vec![]),
        vec![
            closed_issue(1, "2026-09-20T12:00:00Z", Some(11)),
            closed_issue(2, "2026-09-28T12:00:00Z", Some(12)),
        ],
    );

    assert_eq!(placed(&board(&view, &[], at(NOW))), [(2, Column::Done, 2)]);
}

#[test]
fn the_done_window_not_the_spec_lookback_says_how_long_a_closed_ticket_stays_in_done() {
    let mut view = with_closed(
        view(vec![], vec![]),
        vec![
            closed_issue(1, "2026-09-28T12:00:00Z", Some(11)),
            closed_issue(2, "2026-09-30T12:00:00Z", Some(12)),
        ],
    );
    view.policy.done_window = chrono::TimeDelta::days(2);
    view.policy.spec_lookback = chrono::TimeDelta::days(7);

    assert_eq!(placed(&board(&view, &[], at(NOW))), [(2, Column::Done, 2)]);
}

#[test]
fn an_item_fitting_two_columns_is_in_the_one_further_right() {
    let view = with_runs(
        view(
            vec![assigned(ticket(1, "Reworked")), labeled(2, &["needs-info"])],
            vec![pull(10, &[1], &[])],
        ),
        &["10/rework-1", "2/triage-1"],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        placed(&board),
        [(1, Column::InReview, 1), (2, Column::Working, 2)]
    );
    assert_eq!(board.needing_a_person, 0);
}

fn card(board: &Board, number: u64) -> &Card {
    board
        .lanes
        .iter()
        .flat_map(|lane| {
            Column::ALL
                .into_iter()
                .flat_map(|column| lane.cards(column))
        })
        .find(|card| card.number == Number(number))
        .expect("a card")
}

/// Linewright's pull request #`number`, closing `closes`, with
/// `mergeability` and, if `draft`, a draft.
fn reviewed(
    number: u64,
    closes: u64,
    checks: &[CheckState],
    mergeability: Mergeability,
    draft: bool,
) -> Pull {
    Pull {
        mergeability,
        state: PullState::Open { draft },
        ..pull(number, &[closes], checks)
    }
}

#[test]
fn an_in_review_card_says_its_pull_request_and_the_first_that_holds_of_what_stops_it_merging() {
    use CheckState::{Failed, Passed, Pending};
    use Mergeability::{Behind, Clean, Conflicting};
    use ReviewDecision::{Approved, ChangesRequested, ReviewRequired};
    let numbered = || (1..=10).chain(31..=34);
    let issues = numbered()
        .map(|n| assigned(ticket(n, "Reviewed")))
        .collect();
    let decided = |pull: Pull, review_decision| Pull {
        review_decision: Some(review_decision),
        ..pull
    };
    let blocked = |requirement| Mergeability::Blocked {
        requires: vec![requirement],
    };
    let code_owner = || Requirement::CodeOwnerReview {
        owners: vec!["maintainer".into()],
    };
    let pulls = vec![
        reviewed(11, 1, &[Failed, Pending], Conflicting, true),
        reviewed(12, 2, &[Failed, Pending], Conflicting, true),
        reviewed(13, 3, &[Passed, Pending], Conflicting, true),
        reviewed(14, 4, &[Passed], Conflicting, true),
        reviewed(15, 5, &[Passed], Behind, true),
        decided(
            reviewed(16, 6, &[Passed], Mergeability::Unknown, true),
            ChangesRequested,
        ),
        reviewed(17, 7, &[Passed], Clean, false),
        decided(reviewed(18, 8, &[Passed], Clean, false), ChangesRequested),
        decided(reviewed(19, 9, &[Passed], Clean, false), ReviewRequired),
        decided(reviewed(20, 10, &[Passed], Clean, false), Approved),
        reviewed(41, 31, &[Passed], blocked(Requirement::Approval), false),
        decided(
            reviewed(42, 32, &[Passed], blocked(code_owner()), false),
            ChangesRequested,
        ),
        decided(
            reviewed(43, 33, &[Passed], blocked(code_owner()), false),
            Approved,
        ),
        reviewed(44, 34, &[Failed], blocked(code_owner()), false),
    ];
    let view = with_runs(view(issues, pulls), &["11/rework-1"]);

    let board = board(&view, &[], at(NOW));

    let review = |pull_request: u64, stops: Option<Stop>| {
        Some(CardLine::Review {
            pull_request: Number(pull_request),
            author: None,
            stops,
        })
    };
    assert_eq!(
        numbered().map(|n| line(&board, n)).collect::<Vec<_>>(),
        [
            review(
                11,
                Some(Stop::Running(Running {
                    stage: WorkKind::Rework,
                    elapsed: Elapsed {
                        since: at("2026-10-01T11:50:00Z"),
                        seconds: 630,
                    },
                }))
            ),
            review(12, Some(Stop::ChecksFailed)),
            review(13, Some(Stop::ChecksRunning)),
            review(14, Some(Stop::Conflicts)),
            review(
                15,
                Some(Stop::Behind {
                    branch: "main".into()
                })
            ),
            review(16, Some(Stop::Draft)),
            review(17, None),
            review(18, Some(Stop::ChangesRequested)),
            review(19, Some(Stop::ReviewRequired)),
            review(20, Some(Stop::Approved)),
            // GitHub decides no review where the rules require a code
            // owner's review but no approvals; the blocked merge says so.
            review(41, Some(Stop::ReviewRequired)),
            review(42, Some(Stop::ChangesRequested)),
            // Approved, but not yet by a code owner.
            review(43, Some(Stop::ReviewRequired)),
            review(44, Some(Stop::ChecksFailed)),
        ]
    );
    assert!(numbered().all(|n| !card(&board, n).pull_request));
}

#[test]
fn linewrights_pull_request_closing_no_issue_on_the_board_is_a_card_of_its_own() {
    let view = view(vec![], vec![pull(10, &[1], &[CheckState::Pending])]);

    let board = board(&view, &[], at(NOW));

    assert_eq!(placed(&board), [(10, Column::InReview, 10)]);
    let card = card(&board, 10);
    assert!(card.pull_request);
    assert_eq!(card.line, Some(CardLine::MergeStop(Stop::ChecksRunning)));
}

#[test]
fn a_pull_request_card_says_only_what_stops_it_merging_and_nothing_when_nothing_does() {
    let view = view(
        vec![],
        vec![
            pull(10, &[1], &[CheckState::Failed]),
            pull(11, &[2], &[CheckState::Passed]),
        ],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        [10, 11].map(|n| line(&board, n)),
        [Some(CardLine::MergeStop(Stop::ChecksFailed)), None]
    );
}

fn merged(number: u64, closes: &[u64], author: &str, merged_at: &str) -> MergedPull {
    MergedPull {
        title: format!("Pull {number}"),
        author: Login::new(author),
        merged_at: at(merged_at),
        closes: closes.iter().copied().map(Number).collect(),
        files: vec!["src/lib.rs".into()],
        ..spec_change(number, &[])
    }
}

/// Issue #`number`, closed by a person as `reason` `number` minutes past
/// 13:00 on 2026-09-28.
fn closed_as(number: u64, reason: IssueCloseReason) -> ClosedItem {
    let mut closed = closed_issue(number, &format!("2026-09-28T13:{number:02}:00Z"), None);
    if let ClosedKind::Issue { reason: held, .. } = &mut closed.kind {
        *held = Some(reason);
    }
    closed
}

#[test]
fn a_card_in_done_says_how_its_item_ended_and_when_it_closed() {
    let mut closed_pull = closed_issue(30, "2026-09-29T12:00:00Z", None);
    closed_pull.kind = ClosedKind::Pull {
        closes: vec![Number(99)],
        author: Some(Login::new("factory")),
    };
    let view = with_merged(
        with_closed(
            view(vec![], vec![]),
            vec![
                closed_issue(1, "2026-09-28T12:00:00Z", Some(11)),
                closed_issue(2, "2026-09-28T13:00:00Z", None),
                closed_as(3, IssueCloseReason::Completed),
                closed_as(4, IssueCloseReason::NotPlanned),
                closed_as(5, IssueCloseReason::Duplicate),
                closed_pull,
            ],
        ),
        vec![
            // Linewright's, closing #1, which is on the board.
            merged(11, &[1], "factory", "2026-09-28T12:00:00Z"),
            // Linewright's, closing no issue on the board.
            merged(40, &[98], "factory", "2026-09-30T12:00:00Z"),
            // Someone else's.
            merged(41, &[], "someone", "2026-09-30T12:00:00Z"),
            // Linewright's, merged before the Done window opened.
            merged(42, &[], "factory", "2026-09-20T12:00:00Z"),
        ],
    );

    let board = board(&view, &[], at(NOW));

    let ended = |ended, closed_at| {
        Some(CardLine::Ended {
            ended,
            at: at(closed_at),
        })
    };
    let shown: Vec<(u64, bool, Option<CardLine>)> = board
        .lanes
        .iter()
        .flat_map(|lane| lane.cards(Column::Done))
        .map(|card| (card.number.0, card.pull_request, card.line.clone()))
        .collect();
    assert_eq!(
        shown,
        [
            (40, true, ended(Ended::Merged, "2026-09-30T12:00:00Z")),
            (
                30,
                true,
                ended(Ended::ClosedUnmerged, "2026-09-29T12:00:00Z")
            ),
            (
                5,
                false,
                ended(
                    Ended::Closed(Some(IssueCloseReason::Duplicate)),
                    "2026-09-28T13:05:00Z"
                )
            ),
            (
                4,
                false,
                ended(
                    Ended::Closed(Some(IssueCloseReason::NotPlanned)),
                    "2026-09-28T13:04:00Z"
                )
            ),
            (
                3,
                false,
                ended(
                    Ended::Closed(Some(IssueCloseReason::Completed)),
                    "2026-09-28T13:03:00Z"
                )
            ),
            (2, false, ended(Ended::Closed(None), "2026-09-28T13:00:00Z")),
            (
                1,
                false,
                ended(Ended::ClosedIn(Number(11)), "2026-09-28T12:00:00Z")
            ),
        ]
    );
}

/// Linewright's pull request #`number`, merged into `base` at `merged_at`,
/// as an issue it closes refers to it.
fn merged_into(number: u64, base: &str, merged_at: &str) -> ClosingPull {
    ClosingPull {
        number: Number(number),
        author: Login::new("factory"),
        state: PullState::Merged { at: at(merged_at) },
        updated_at: at(merged_at),
        base_ref: base.into(),
    }
}

fn reopened(when: &str) -> IssueEvent {
    IssueEvent::Reopened { at: at(when) }
}

#[test]
fn an_open_issue_whose_closing_pull_request_merged_into_the_default_branch_since_it_was_last_opened_is_in_done()
 {
    let ready = || labeled(1, &["ready-for-agent", label::DESIGNED]);
    let merged_into_main = Issue {
        closed_by: vec![merged_into(11, "main", "2026-09-30T10:00:00Z")],
        ..ready()
    };
    // Its status label removed, Linewright leaves it alone.
    let unlabeled = Issue {
        number: Number(2),
        labels: vec![],
        events: vec![],
        closed_by: vec![merged_into(12, "main", "2026-09-30T10:02:00Z")],
        ..ready()
    };
    let merged_elsewhere = Issue {
        number: Number(3),
        closed_by: vec![merged_into(13, "release", "2026-09-30T10:03:00Z")],
        ..ready()
    };
    let reopened_since = Issue {
        number: Number(4),
        events: [ready().events, vec![reopened("2026-09-30T11:00:00Z")]].concat(),
        closed_by: vec![merged_into(14, "main", "2026-09-30T10:04:00Z")],
        ..ready()
    };
    let merged_several = Issue {
        number: Number(5),
        events: [ready().events, vec![reopened("2026-09-30T09:00:00Z")]].concat(),
        closed_by: vec![
            merged_into(15, "main", "2026-09-30T08:00:00Z"),
            merged_into(16, "main", "2026-09-30T10:06:00Z"),
            merged_into(17, "main", "2026-09-30T10:05:00Z"),
            merged_into(18, "release", "2026-09-30T09:30:00Z"),
        ],
        ..ready()
    };
    let view = view(
        vec![
            merged_into_main,
            unlabeled,
            merged_elsewhere,
            reopened_since,
            merged_several,
        ],
        vec![],
    );

    let board = board(&view, &[], at(NOW));

    let mut placed = placed(&board);
    placed.sort_by_key(|(_, _, number)| *number);
    assert_eq!(
        placed,
        [
            (1, Column::Done, 1),
            (3, Column::Ready, 3),
            (4, Column::Ready, 4),
            (5, Column::Done, 5),
        ]
    );
    let ended = |pull_request, when| {
        Some(CardLine::Ended {
            ended: Ended::ClosedIn(Number(pull_request)),
            at: at(when),
        })
    };
    assert_eq!(card(&board, 1).line, ended(11, "2026-09-30T10:00:00Z"));
    assert_eq!(card(&board, 5).line, ended(17, "2026-09-30T10:05:00Z"));
}

/// `closed`, with `events` and pull requests `closed_by` closing it.
fn with_work(
    mut closed: ClosedItem,
    events: Vec<IssueEvent>,
    closed_by: Vec<ClosingPull>,
) -> ClosedItem {
    if let ClosedKind::Issue { facts, .. } = &mut closed.kind {
        facts.work.events = events;
        facts.work.closed_by = closed_by;
    }
    closed
}

#[test]
fn a_closed_issue_entered_done_when_its_first_closing_pull_request_since_it_was_last_opened_merged_or_else_when_it_closed()
 {
    let view = with_closed(
        view(vec![], vec![]),
        vec![
            // Merged before it closed, with a later merge closing it.
            with_work(
                closed_issue(1, "2026-09-28T12:00:00Z", Some(12)),
                vec![],
                vec![
                    merged_into(11, "main", "2026-09-26T12:00:00Z"),
                    merged_into(12, "main", "2026-09-28T11:59:00Z"),
                ],
            ),
            // Merged, reopened, then closed by a person.
            with_work(
                closed_issue(2, "2026-09-27T12:00:00Z", None),
                vec![reopened("2026-09-26T12:00:00Z")],
                vec![merged_into(21, "main", "2026-09-25T12:00:00Z")],
            ),
            // Merged into another branch, then closed by a person.
            with_work(
                closed_issue(3, "2026-09-27T13:00:00Z", None),
                vec![],
                vec![merged_into(31, "release", "2026-09-25T12:00:00Z")],
            ),
            // Merged before the Done window opened, closed within it.
            with_work(
                closed_issue(4, "2026-09-28T12:00:00Z", Some(41)),
                vec![],
                vec![merged_into(41, "main", "2026-09-20T12:00:00Z")],
            ),
        ],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        lanes(&board),
        [
            (3, Some(at("2026-09-27T13:00:00Z"))),
            (2, Some(at("2026-09-27T12:00:00Z"))),
            (1, Some(at("2026-09-26T12:00:00Z"))),
        ]
    );
    let ended = |ended, when| {
        Some(CardLine::Ended {
            ended,
            at: at(when),
        })
    };
    assert_eq!(
        card(&board, 1).line,
        ended(Ended::ClosedIn(Number(11)), "2026-09-26T12:00:00Z")
    );
    assert_eq!(
        card(&board, 2).line,
        ended(Ended::Closed(None), "2026-09-27T12:00:00Z")
    );
    assert_eq!(
        card(&board, 3).line,
        ended(Ended::Closed(None), "2026-09-27T13:00:00Z")
    );
}

/// Spec change #`number`, merged at 09:00, labeled `labels` by Linewright
/// as it merged.
fn spec_change(number: u64, labels: &[&str]) -> MergedPull {
    MergedPull {
        number: Number(number),
        title: "Add invoices to the spec".into(),
        body: String::new(),
        author: Login::new("maintainer"),
        merged_at: at("2026-10-01T09:00:00Z"),
        updated_at: at("2026-10-01T09:00:00Z"),
        base_ref: "main".into(),
        labels: labels.iter().map(ToString::to_string).collect(),
        closes: vec![],
        files: vec!["docs/specs/invoices.md".into()],
        assignees: vec![Login::new("factory")],
        events: labels
            .iter()
            .map(|label| labeling(label, "2026-10-01T09:00:00Z"))
            .collect(),
    }
}

fn with_merged(mut view: View, merged: Vec<MergedPull>) -> View {
    std::sync::Arc::make_mut(&mut view.snapshot).merged = merged;
    view
}

/// The tracking issue #`number` of spec change #`spec_change`, with its
/// tickets, each open or not.
fn tracking_issue(number: u64, spec_change: u64, tickets: &[(u64, bool)]) -> Issue {
    Issue {
        author: Login::new("factory"),
        labels: vec![],
        filed: Some(Filed::Tracking {
            spec_change: Number(spec_change),
        }),
        sub_issues: tickets
            .iter()
            .map(|&(n, open)| common::sub_issue(n, open, "factory"))
            .collect(),
        ..ticket(number, &format!("Tracking {number}"))
    }
}

fn cut_from(issue: Issue, tracking: u64, spec_change: u64) -> Issue {
    Issue {
        author: Login::new("factory"),
        parent: Some(Parent {
            number: Number(tracking),
            spec_change: Some(Number(spec_change)),
        }),
        ..issue
    }
}

#[test]
fn a_spec_change_is_its_pull_request_in_ready_then_in_working_while_being_designed() {
    let waiting = with_merged(view(vec![], vec![]), vec![spec_change(5, &[])]);
    let designing = with_runs(waiting.clone(), &["5/design-1"]);

    assert_eq!(
        placed(&board(&waiting, &[], at(NOW))),
        [(5, Column::Ready, 5)]
    );
    assert!(card(&board(&waiting, &[], at(NOW)), 5).pull_request);
    assert_eq!(
        placed(&board(&designing, &[], at(NOW))),
        [(5, Column::Working, 5)]
    );
}

#[test]
fn a_spec_change_cut_into_tickets_is_its_tracking_issues_lane() {
    let view = with_merged(
        view(
            vec![
                tracking_issue(6, 5, &[(7, true), (8, true)]),
                cut_from(ticket(7, "First"), 6, 5),
                cut_from(assigned(ticket(8, "Second")), 6, 5),
            ],
            vec![],
        ),
        vec![spec_change(5, &[label::DESIGNED])],
    );

    assert_eq!(
        placed(&board(&view, &[], at(NOW))),
        [(6, Column::Ready, 7), (6, Column::Working, 8)]
    );
}

#[test]
fn a_spec_change_designed_with_nothing_to_build_is_in_done_saying_so() {
    let view = with_merged(
        view(vec![], vec![]),
        vec![spec_change(5, &[label::DESIGNED])],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(placed(&board), [(5, Column::Done, 5)]);
    assert!(card(&board, 5).pull_request);
    assert_eq!(
        card(&board, 5).line,
        Some(CardLine::Ended {
            ended: Ended::NothingToBuild,
            at: at("2026-10-01T09:00:00Z"),
        })
    );
}

#[test]
fn lanes_are_in_number_order_with_a_tracking_issues_tickets_closed_of_total() {
    let mut closed_ticket = closed_issue(23, "2026-09-28T12:00:00Z", Some(40));
    closed_ticket.kind = ClosedKind::Issue {
        parent: Some(Parent {
            number: Number(20),
            spec_change: Some(Number(19)),
        }),
        facts: ManagedFacts {
            filed: None,
            sub_issues: vec![],
            labels: vec![label::DESIGNED.into()],
            work: claimed(),
        },
        merged_by: Some(Number(40)),
        reason: None,
    };
    let view = with_closed(
        view(
            vec![
                labeled(2, &["needs-triage"]),
                tracking_issue(20, 19, &[(21, true), (22, true), (23, false)]),
                tracking_issue(10, 9, &[(11, true), (12, false)]),
                cut_from(ticket(22, "Second"), 20, 19),
                cut_from(ticket(21, "First"), 20, 19),
                cut_from(ticket(11, "Only"), 10, 9),
            ],
            vec![],
        ),
        vec![closed_ticket],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        heads(&board),
        [
            LaneHead::Item {
                number: Number(2),
                title: "Ticket 2".into()
            },
            LaneHead::Tracking {
                number: Number(10),
                title: "Tracking 10".into(),
                tickets_closed: 1,
                tickets: 2,
                closed: None
            },
            LaneHead::Tracking {
                number: Number(20),
                title: "Tracking 20".into(),
                tickets_closed: 1,
                tickets: 3,
                closed: None
            },
        ]
    );
    assert_eq!(
        placed(&board),
        [
            (2, Column::Triage, 2),
            (10, Column::Ready, 11),
            (20, Column::Ready, 21),
            (20, Column::Ready, 22),
            (20, Column::Done, 23),
        ]
    );
}

#[test]
fn an_issue_cut_into_tickets_heads_a_lane_counting_only_its_tickets() {
    let cut = Issue {
        labels: vec![label::DESIGNED.into()],
        sub_issues: vec![
            common::sub_issue(31, true, "factory"),
            common::sub_issue(32, false, "factory"),
            common::sub_issue(33, false, "maintainer"),
        ],
        ..ticket(30, "Cut")
    };
    let ticket_of_it = Issue {
        author: Login::new("factory"),
        parent: Some(Parent {
            number: Number(30),
            spec_change: None,
        }),
        ..ticket(31, "Part")
    };
    let added = Issue {
        parent: Some(Parent {
            number: Number(30),
            spec_change: None,
        }),
        ..labeled(34, &["needs-info"])
    };
    let view = view(vec![cut, ticket_of_it, added], vec![]);

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        board.lanes[0].head,
        LaneHead::Tracking {
            number: Number(30),
            title: "Cut".into(),
            tickets_closed: 1,
            tickets: 2,
            closed: None
        }
    );
    assert_eq!(
        placed(&board),
        [(30, Column::Ready, 31), (34, Column::NeedsAPerson, 34)]
    );
}

#[test]
fn an_issue_in_review_on_someone_elses_pull_request_names_them() {
    let mut theirs = pull(10, &[1], &[CheckState::Pending]);
    theirs.author = Login::new("someone");
    let unheld = Issue {
        closed_by: vec![ClosingPull {
            number: Number(20),
            base_ref: "main".into(),
            author: Login::new("alice"),
            state: PullState::Open { draft: true },
            updated_at: at("2026-09-30T09:00:00Z"),
        }],
        ..ticket(2, "Not held")
    };
    let view = view(vec![ticket(1, "Theirs"), unheld], vec![theirs]);

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        placed(&board),
        [(1, Column::InReview, 1), (2, Column::InReview, 2)]
    );
    assert_eq!(
        [1, 2].map(|n| line(&board, n)),
        [
            Some(CardLine::Review {
                pull_request: Number(10),
                author: Some(Login::new("someone")),
                stops: Some(Stop::ChecksRunning),
            }),
            Some(CardLine::Review {
                pull_request: Number(20),
                author: Some(Login::new("alice")),
                stops: Some(Stop::Draft),
            }),
        ]
    );
}

/// Ticket #`number` of tracking issue #`tracking`, closed at `closed_at`.
fn closed_ticket(number: u64, tracking: u64, closed_at: &str) -> ClosedItem {
    let mut closed = closed_issue(number, closed_at, Some(number + 100));
    closed.kind = ClosedKind::Issue {
        parent: Some(Parent {
            number: Number(tracking),
            spec_change: Some(Number(tracking - 1)),
        }),
        facts: ManagedFacts {
            filed: None,
            sub_issues: vec![],
            labels: vec![label::DESIGNED.into()],
            work: claimed(),
        },
        merged_by: Some(Number(number + 100)),
        reason: None,
    };
    closed
}

/// The tracking issue #`number` of spec change #`spec_change`, as
/// `tracking_issue` gives it, closed at `closed_at`.
fn closed_tracking_issue(
    number: u64,
    spec_change: u64,
    tickets: &[(u64, bool)],
    closed_at: &str,
) -> ClosedItem {
    common::closed_item(&tracking_issue(number, spec_change, tickets), closed_at)
}

/// Each lane's number, and when it finished, if it did.
fn lanes(board: &Board) -> Vec<(u64, Option<Timestamp>)> {
    board
        .lanes
        .iter()
        .map(|lane| (lane.head.number().0, lane.finished()))
        .collect()
}

/// `issue`, last updated on GitHub at `updated_at`.
fn updated(issue: Issue, updated_at: &str) -> Issue {
    Issue {
        updated_at: at(updated_at),
        ..issue
    }
}

#[test]
fn unfinished_lanes_come_first_latest_changed_first_then_the_finished_lanes_latest_finished_first()
{
    let view = with_closed(
        view(
            vec![
                updated(labeled(2, &["needs-triage"]), "2026-10-01T11:00:00Z"),
                tracking_issue(10, 9, &[(11, true)]),
                updated(cut_from(ticket(11, "Ready"), 10, 9), "2026-10-01T10:00:00Z"),
                tracking_issue(20, 19, &[(21, true), (22, false)]),
                updated(
                    cut_from(labeled(21, &["needs-info"]), 20, 19),
                    "2026-10-01T09:30:00Z",
                ),
            ],
            vec![],
        ),
        vec![
            closed_tracking_issue(30, 29, &[(31, false), (32, false)], "2026-09-29T12:00:00Z"),
            closed_tracking_issue(40, 39, &[(41, false)], "2026-09-30T12:00:00Z"),
            closed_tracking_issue(50, 49, &[(51, false)], "2026-09-29T12:00:00Z"),
            closed_ticket(22, 20, "2026-09-30T12:00:00Z"),
            closed_ticket(31, 30, "2026-09-29T12:00:00Z"),
            closed_ticket(32, 30, "2026-09-27T12:00:00Z"),
            closed_ticket(41, 40, "2026-09-30T12:00:00Z"),
            closed_ticket(51, 50, "2026-09-29T12:00:00Z"),
        ],
    );

    assert_eq!(
        lanes(&board(&view, &[], at(NOW))),
        [
            (2, None),
            (10, None),
            (20, None),
            (40, Some(at("2026-09-30T12:00:00Z"))),
            (30, Some(at("2026-09-29T12:00:00Z"))),
            (50, Some(at("2026-09-29T12:00:00Z"))),
        ]
    );
}

#[test]
fn a_lane_whose_card_was_commented_on_most_recently_comes_first() {
    let commented = Issue {
        comments: vec![Comment {
            author: Login::new("alice"),
            body: "Still wanted.".into(),
            created_at: at("2026-10-01T11:00:00Z"),
        }],
        ..updated(labeled(2, &["ready-for-agent"]), "2026-10-01T11:00:00Z")
    };
    let view = view(
        vec![
            updated(labeled(1, &["ready-for-agent"]), "2026-10-01T10:00:00Z"),
            commented,
        ],
        vec![],
    );

    assert_eq!(lanes(&board(&view, &[], at(NOW))), [(2, None), (1, None)]);
}

#[test]
fn a_lane_with_a_run_in_flight_comes_before_every_unfinished_lane_without_one() {
    let view = with_runs(
        view(
            vec![
                updated(labeled(2, &["ready-for-agent"]), "2026-10-01T12:00:29Z"),
                labeled(3, &["needs-triage"]),
            ],
            vec![],
        ),
        &["3/triage-1"],
    );

    assert_eq!(lanes(&board(&view, &[], at(NOW))), [(3, None), (2, None)]);
}

#[test]
fn lanes_changed_at_the_same_time_are_in_number_order() {
    let view = view(
        vec![
            labeled(5, &["needs-info"]),
            labeled(3, &["ready-for-agent"]),
            tracking_issue(10, 9, &[(11, true)]),
            cut_from(labeled(11, &["ready-for-human"]), 10, 9),
        ],
        vec![],
    );

    assert_eq!(
        lanes(&board(&view, &[], at(NOW))),
        [(3, None), (5, None), (10, None)]
    );
}

#[test]
fn a_lane_needing_a_person_is_not_hoisted_above_one_changed_since() {
    let view = view(
        vec![
            updated(labeled(1, &["needs-info"]), "2026-10-01T10:00:00Z"),
            updated(labeled(2, &["ready-for-agent"]), "2026-10-01T11:00:00Z"),
        ],
        vec![],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(lanes(&board), [(2, None), (1, None)]);
    assert_eq!(board.needing_a_person, 1);
}

#[test]
fn a_pull_request_closing_a_card_counts_toward_its_lanes_last_change() {
    let linewrights = Pull {
        updated_at: at("2026-10-01T11:00:00Z"),
        ..pull(10, &[2], &[CheckState::Passed])
    };
    let view = view(
        vec![
            updated(labeled(1, &["ready-for-agent"]), "2026-10-01T10:00:00Z"),
            assigned(ticket(2, "Reviewed")),
            closed_unmerged(labeled(3, &["ready-for-human"]), 30, "2026-10-01T11:30:00Z"),
        ],
        vec![linewrights],
    );

    assert_eq!(
        lanes(&board(&view, &[], at(NOW))),
        [(3, None), (2, None), (1, None)]
    );
}

#[test]
fn someone_elses_open_pull_request_closing_a_card_counts_toward_its_lanes_last_change() {
    let mut theirs = common::closed_by(ticket(2, "Theirs"), 20, "alice");
    theirs.closed_by[0].updated_at = at("2026-10-01T11:30:00Z");
    let view = view(
        vec![
            updated(labeled(1, &["ready-for-agent"]), "2026-10-01T11:00:00Z"),
            theirs,
        ],
        vec![],
    );

    assert_eq!(lanes(&board(&view, &[], at(NOW))), [(2, None), (1, None)]);
}

#[test]
fn a_run_that_ended_without_writing_to_github_counts_toward_its_lanes_last_change() {
    let view = view(
        vec![
            labeled(2, &["ready-for-agent"]),
            updated(labeled(1, &["ready-for-agent"]), "2026-10-01T10:00:00Z"),
        ],
        vec![],
    );
    let recorded = [recorded(
        "2/implement-1",
        "2026-10-01T11:00:00Z",
        Ending::Cancelled,
    )];

    assert_eq!(
        lanes(&board(&view, &recorded, at(NOW))),
        [(2, None), (1, None)]
    );
}

#[test]
fn a_run_counts_toward_its_lanes_last_change_from_when_it_started() {
    let view = view(
        vec![
            labeled(2, &["ready-for-agent"]),
            updated(labeled(1, &["ready-for-agent"]), "2026-10-01T08:00:00Z"),
        ],
        vec![],
    );
    let started = Started::new("2/implement-1".parse().unwrap(), None).unwrap();
    let interrupted = Recorded::new(
        started,
        at("2026-10-01T09:00:00Z"),
        State::Interrupted,
        vec![],
        None,
    );

    assert_eq!(
        lanes(&board(&view, &[interrupted], at(NOW))),
        [(2, None), (1, None)]
    );
}

#[test]
fn an_item_closed_in_the_window_is_a_finished_lane_ordered_latest_finished_first() {
    let view = with_merged(
        with_closed(
            view(vec![], vec![]),
            vec![
                closed_tracking_issue(30, 29, &[(31, false)], "2026-09-30T12:00:00Z"),
                closed_issue(1, "2026-09-28T12:00:00Z", Some(11)),
                closed_ticket(31, 30, "2026-09-30T12:00:00Z"),
            ],
        ),
        vec![spec_change(5, &[label::DESIGNED])],
    );

    assert_eq!(
        lanes(&board(&view, &[], at(NOW))),
        [
            (5, Some(at("2026-10-01T09:00:00Z"))),
            (30, Some(at("2026-09-30T12:00:00Z"))),
            (1, Some(at("2026-09-28T12:00:00Z"))),
        ]
    );
}

#[test]
fn an_item_not_cut_into_tickets_heads_a_lane_of_its_own_with_itself_as_its_one_card() {
    let mut issue = labeled(2, &["needs-triage"]);
    issue.title = "Standalone".into();
    let mut pull = pull(30, &[99], &[CheckState::Passed]);
    pull.title = "Tidy up".into();
    let mut spec = spec_change(5, &[]);
    spec.title = "Add invoices".into();
    let view = with_merged(view(vec![issue], vec![pull]), vec![spec]);

    let board = board(&view, &[], at(NOW));

    // Latest changed first: the pull request, the spec change, the issue.
    assert_eq!(
        heads(&board),
        [
            LaneHead::Item {
                number: Number(30),
                title: "Tidy up".into()
            },
            LaneHead::Item {
                number: Number(5),
                title: "Add invoices".into()
            },
            LaneHead::Item {
                number: Number(2),
                title: "Standalone".into()
            },
        ]
    );
    assert_eq!(
        placed(&board),
        [
            (30, Column::InReview, 30),
            (5, Column::Ready, 5),
            (2, Column::Triage, 2),
        ]
    );
}

#[test]
fn a_tracking_issue_with_a_run_in_flight_on_it_is_its_lane_and_not_a_card() {
    let view = with_runs(
        view(
            vec![
                tracking_issue(10, 9, &[(11, true)]),
                cut_from(ticket(11, "Ready"), 10, 9),
            ],
            vec![],
        ),
        &["10/triage-1"],
    );

    assert_eq!(
        placed(&board(&view, &[], at(NOW))),
        [(10, Column::Ready, 11)]
    );
}

/// The line on #`number`'s card.
fn line(board: &Board, number: u64) -> Option<CardLine> {
    card(board, number).line.clone()
}

fn recorded(id: &str, ended_at: &str, ending: Ending) -> Recorded {
    let started = Started::new(id.parse().unwrap(), None).unwrap();
    let state = State::Ended {
        at: at(ended_at),
        ending,
    };
    Recorded::new(started, at("2026-10-01T09:00:00Z"), state, vec![], None)
}

/// Linewright's pull request #`pr`, closing `issue`, closed unmerged at
/// `closed_at`.
fn closed_unmerged(issue: Issue, pr: u64, closed_at: &str) -> Issue {
    let mut issue = common::closed_by(issue, pr, "factory");
    issue.closed_by[0].state = PullState::Closed { at: at(closed_at) };
    issue
}

#[test]
fn a_card_needing_a_person_says_who_was_asked_it_was_handed_back_or_a_decision_is_needed() {
    let view = view(
        vec![
            labeled(1, &["needs-info"]),
            labeled(2, &["ready-for-human"]),
            labeled(3, &["ready-for-human"]),
            labeled(4, &["ready-for-human"]),
            closed_unmerged(labeled(5, &["ready-for-human"]), 50, "2026-10-01T10:00:00Z"),
            closed_unmerged(labeled(6, &["ready-for-human"]), 60, "2026-10-01T10:00:00Z"),
            labeled(7, &["ready-for-human"]),
        ],
        vec![],
    );
    let recorded = [
        // #2: handed back by its last run.
        recorded("2/implement-1", "2026-10-01T10:00:00Z", Ending::HandedBack),
        // #3: handed back, then triaged again, which asked for a decision.
        recorded("3/implement-1", "2026-10-01T10:00:00Z", Ending::HandedBack),
        recorded("3/triage-1", "2026-10-01T11:00:00Z", Ending::Done),
        // #4: a triage that asked for a decision.
        recorded("4/triage-1", "2026-10-01T10:00:00Z", Ending::Done),
        // #6: triaged again after its pull request closed unmerged.
        recorded("6/triage-1", "2026-10-01T11:00:00Z", Ending::Done),
        // #7: the hand-back of another item.
        recorded("8/implement-1", "2026-10-01T10:00:00Z", Ending::HandedBack),
    ];

    let board = board(&view, &recorded, at(NOW));

    assert_eq!(
        [1, 2, 3, 4, 5, 6, 7].map(|n| line(&board, n)),
        [
            Some(CardLine::Asked(vec![])),
            Some(CardLine::HandedBack),
            Some(CardLine::DecisionNeeded),
            Some(CardLine::DecisionNeeded),
            Some(CardLine::HandedBack),
            Some(CardLine::DecisionNeeded),
            Some(CardLine::DecisionNeeded),
        ]
    );
}

#[test]
fn a_card_waiting_on_the_people_asked_names_its_assignees_in_order_but_linewright() {
    let with_assignees = |number, assignees: &[&str]| Issue {
        assignees: assignees.iter().map(|a| Login::new(*a)).collect(),
        ..labeled(number, &["needs-info"])
    };
    let view = view(
        vec![
            with_assignees(1, &["alice"]),
            with_assignees(2, &["bob", "alice"]),
            with_assignees(3, &["alice", "factory", "bob", "carol"]),
            with_assignees(4, &["factory"]),
        ],
        vec![],
    );

    let board = board(&view, &[], at(NOW));

    let logins = |logins: &[&str]| logins.iter().map(|l| Login::new(*l)).collect();
    assert_eq!(
        [1, 2, 3, 4].map(|n| line(&board, n)),
        [
            Some(CardLine::Asked(logins(&["alice"]))),
            Some(CardLine::Asked(logins(&["bob", "alice"]))),
            Some(CardLine::Asked(logins(&["alice", "bob", "carol"]))),
            Some(CardLine::Asked(vec![])),
        ]
    );
}

#[test]
fn a_stalled_card_needing_a_person_says_so_first() {
    let view = view(
        vec![
            assigned_to(labeled(1, &["needs-info", label::STALLED]), "alice"),
            labeled(2, &["ready-for-human", label::STALLED]),
            labeled(3, &["ready-for-human", label::STALLED]),
        ],
        vec![],
    );
    let recorded = [recorded(
        "2/implement-1",
        "2026-10-01T10:00:00Z",
        Ending::HandedBack,
    )];

    let board = board(&view, &recorded, at(NOW));

    assert_eq!(
        [1, 2, 3].map(|n| line(&board, n)),
        [
            Some(CardLine::Stalled),
            Some(CardLine::Stalled),
            Some(CardLine::Stalled),
        ]
    );
}

#[test]
fn a_stalled_pull_request_in_review_says_so_ahead_of_a_run_in_flight_and_failed_checks() {
    use CheckState::Failed;
    let stalled = Pull {
        labels: vec![label::STALLED.into()],
        ..reviewed(11, 1, &[Failed], Mergeability::Conflicting, true)
    };
    let view = with_runs(
        view(vec![assigned(ticket(1, "Reviewed"))], vec![stalled]),
        &["11/rework-1"],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        line(&board, 1),
        Some(CardLine::Review {
            pull_request: Number(11),
            author: None,
            stops: Some(Stop::Stalled),
        })
    );
}

fn blocked_by(issue: Issue, blockers: &[u64]) -> Issue {
    Issue {
        blocked_by: blockers
            .iter()
            .map(|&n| Linked {
                number: Number(n),
                open: true,
            })
            .collect(),
        ..issue
    }
}

fn assigned_to(issue: Issue, login: &str) -> Issue {
    Issue {
        assignees: vec![Login::new(login)],
        ..issue
    }
}

/// #1, blocked by #9 and #8; #2, assigned to alice;
/// and #3, ready to claim.
fn ready_view() -> View {
    view(
        vec![
            blocked_by(ticket(1, "Blocked"), &[9, 8]),
            assigned_to(ticket(2, "Theirs"), "alice"),
            ticket(3, "Ready"),
        ],
        vec![],
    )
}

fn ready_lines(view: &View) -> [Option<CardLine>; 3] {
    let board = board(view, &[], at(NOW));
    assert_eq!(
        placed(&board),
        [
            (1, Column::Ready, 1),
            (2, Column::Ready, 2),
            (3, Column::Ready, 3)
        ]
    );
    [1, 2, 3].map(|n| line(&board, n))
}

#[test]
fn a_ready_card_says_its_lowest_blocker_then_its_assignee_then_what_holds_it_back() {
    let blocked = Some(CardLine::BlockedBy(Number(8)));
    let assigned = Some(CardLine::AssignedTo(Login::new("alice")));
    let limited = View {
        memory: common::limited_until("2026-10-01T13:00:00Z"),
        ..ready_view()
    };
    let draining = View {
        memory: common::draining_since("2026-10-01T11:00:00Z"),
        ..ready_view()
    };
    let full = View {
        live: (11..=14).map(common::implementing).collect(),
        ..ready_view()
    };

    assert_eq!(
        ready_lines(&ready_view()),
        [blocked.clone(), assigned.clone(), None]
    );
    assert_eq!(
        ready_lines(&limited),
        [
            blocked.clone(),
            assigned.clone(),
            Some(CardLine::Paused(Hold::UsageLimit))
        ]
    );
    assert_eq!(
        ready_lines(&draining),
        [
            blocked.clone(),
            assigned.clone(),
            Some(CardLine::Paused(Hold::Drain))
        ]
    );
    assert_eq!(
        ready_lines(&full),
        [blocked, assigned, Some(CardLine::Queued)]
    );
}

#[test]
fn a_working_card_says_its_run_in_flight_or_else_what_holds_it_back() {
    let working = view(
        vec![
            assigned(ticket(1, "Implemented")),
            labeled(2, &["needs-triage"]),
            labeled(3, &["ready-for-agent"]),
        ],
        vec![],
    );
    let running = with_runs(working.clone(), &["1/implement-1", "2/triage-1"]);
    let limited = View {
        memory: common::limited_until("2026-10-01T13:00:00Z"),
        ..with_runs(working, &["2/triage-1"])
    };

    let running = board(&running, &[], at(NOW));
    let limited = board(&limited, &[], at(NOW));

    let run = |stage| {
        Some(CardLine::Running(Running {
            stage,
            elapsed: Elapsed {
                since: at("2026-10-01T11:50:00Z"),
                seconds: 630,
            },
        }))
    };
    assert_eq!(
        [1, 2].map(|n| line(&running, n)),
        [run(WorkKind::Implement), run(WorkKind::Triage)]
    );
    assert_eq!(
        [1, 2].map(|n| line(&limited, n)),
        [
            Some(CardLine::Paused(Hold::UsageLimit)),
            run(WorkKind::Triage)
        ]
    );
}

/// `issue`, with `events` read.
fn with_events(issue: Issue, events: Vec<IssueEvent>) -> Issue {
    Issue { events, ..issue }
}

/// Linewright labeling an item `label` at `when`.
fn labeling(label: &str, when: &str) -> IssueEvent {
    IssueEvent::Labeled {
        label: label.into(),
        by: Some(Login::new("factory")),
        at: at(when),
    }
}

/// When #`number`'s card says it entered its column.
fn entered(board: &Board, number: u64) -> Option<Timestamp> {
    card(board, number).entered
}

#[test]
fn a_card_in_triage_entered_its_column_when_needs_triage_was_applied_or_the_issue_reopened() {
    let view = view(
        vec![
            with_events(
                labeled(1, &["needs-triage"]),
                vec![labeling("needs-triage", "2026-10-01T10:00:00Z")],
            ),
            with_events(
                labeled(2, &["needs-triage"]),
                vec![
                    labeling("needs-triage", "2026-10-01T09:00:00Z"),
                    IssueEvent::Reopened {
                        at: at("2026-10-01T11:00:00Z"),
                    },
                ],
            ),
            // Created 2026-09-30T09:00:00Z, with no event read.
            labeled(3, &["needs-triage"]),
        ],
        vec![],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        [1, 2, 3].map(|n| entered(&board, n)),
        [
            Some(at("2026-10-01T10:00:00Z")),
            Some(at("2026-10-01T11:00:00Z")),
            Some(at("2026-09-30T09:00:00Z")),
        ]
    );
    assert_eq!(line(&board, 1), None, "nothing but the time");
}

#[test]
fn a_card_needing_a_person_entered_its_column_when_the_label_it_holds_was_applied() {
    let view = view(
        vec![
            with_events(
                labeled(1, &["needs-info"]),
                vec![
                    labeling("needs-triage", "2026-10-01T08:00:00Z"),
                    labeling("needs-info", "2026-10-01T10:00:00Z"),
                ],
            ),
            with_events(
                labeled(2, &["ready-for-human"]),
                vec![labeling("ready-for-human", "2026-10-01T11:00:00Z")],
            ),
            labeled(3, &["ready-for-human"]),
        ],
        vec![],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        [1, 2, 3].map(|n| entered(&board, n)),
        [
            Some(at("2026-10-01T10:00:00Z")),
            Some(at("2026-10-01T11:00:00Z")),
            Some(at("2026-09-30T09:00:00Z")),
        ]
    );
    assert_eq!(line(&board, 1), Some(CardLine::Asked(vec![])));
}

#[test]
fn a_ready_card_entered_its_column_when_ready_for_agent_was_applied_or_linewright_unassigned() {
    let view = view(
        vec![
            blocked_by(
                with_events(
                    labeled(1, &["ready-for-agent"]),
                    vec![labeling("ready-for-agent", "2026-10-01T10:00:00Z")],
                ),
                &[9],
            ),
            with_events(
                labeled(2, &["ready-for-agent"]),
                vec![
                    labeling("ready-for-agent", "2026-10-01T09:00:00Z"),
                    unassigning_linewright("2026-10-01T11:00:00Z"),
                ],
            ),
            labeled(3, &["ready-for-agent"]),
        ],
        vec![],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        [1, 2, 3].map(|n| entered(&board, n)),
        [
            Some(at("2026-10-01T10:00:00Z")),
            Some(at("2026-10-01T11:00:00Z")),
            Some(at("2026-09-30T09:00:00Z")),
        ]
    );
    assert_eq!(line(&board, 1), Some(CardLine::BlockedBy(Number(9))));
}

#[test]
fn a_ready_spec_change_entered_its_column_when_its_merge_linewright_unassigned_or_the_required_label()
 {
    let unassigned = MergedPull {
        events: vec![unassigning_linewright("2026-10-01T10:30:00Z")],
        ..spec_change(6, &[])
    };
    let relabeled = MergedPull {
        events: vec![
            unassigning_linewright("2026-10-01T10:30:00Z"),
            labeling("linewright", "2026-10-01T11:15:00Z"),
        ],
        ..spec_change(7, &["linewright"])
    };
    let mut view = with_merged(
        view(vec![], vec![]),
        vec![spec_change(5, &["linewright"]), unassigned, relabeled],
    );
    view.policy.require_label = Some("linewright".into());

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        placed(&board),
        [
            (5, Column::Ready, 5),
            (6, Column::Ready, 6),
            (7, Column::Ready, 7),
        ]
    );
    assert_eq!(
        [5, 6, 7].map(|n| entered(&board, n)),
        [
            Some(at("2026-10-01T09:00:00Z")),
            Some(at("2026-10-01T10:30:00Z")),
            Some(at("2026-10-01T11:15:00Z")),
        ]
    );
}

#[test]
fn working_in_review_and_done_cards_have_no_time_in_their_column() {
    let view = with_closed(
        with_runs(
            view(
                vec![
                    labeled(1, &["needs-triage"]),
                    assigned(ticket(2, "Reviewed")),
                ],
                vec![pull(10, &[2], &[CheckState::Passed])],
            ),
            &["1/triage-1"],
        ),
        vec![closed_issue(3, "2026-09-30T12:00:00Z", Some(13))],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        placed(&board),
        [
            (1, Column::Working, 1),
            (2, Column::InReview, 2),
            (3, Column::Done, 3),
        ]
    );
    assert_eq!([1, 2, 3].map(|n| entered(&board, n)), [None, None, None]);
}

#[test]
fn a_spec_change_linewright_was_unassigned_from_is_ready() {
    let unassigned = MergedPull {
        assignees: vec![],
        events: vec![unassigning_linewright("2026-10-01T10:30:00Z")],
        ..spec_change(6, &[])
    };
    let view = with_merged(view(vec![], vec![]), vec![unassigned]);

    assert_eq!(placed(&board(&view, &[], at(NOW))), [(6, Column::Ready, 6)]);
}

#[test]
fn with_a_required_label_an_issue_without_it_is_on_the_board_only_once_linewright_has_worked_it() {
    let unmarked = ticket(1, "Never touched");
    let handed_back = Issue {
        events: vec![
            assigning_linewright("2026-10-01T10:00:00Z"),
            unassigning_linewright("2026-10-01T10:30:00Z"),
        ],
        ..ticket(2, "Handed back")
    };
    let mut view = view(vec![unmarked, handed_back], vec![]);
    view.policy.require_label = Some("linewright".into());

    assert_eq!(placed(&board(&view, &[], at(NOW))), [(2, Column::Ready, 2)]);
}

#[test]
fn a_ready_card_linewright_was_unassigned_from_says_so_after_its_blocker_and_assignee() {
    let unassigned = |issue: Issue| {
        with_events(
            issue,
            vec![
                labeling("ready-for-agent", "2026-10-01T09:00:00Z"),
                assigning_linewright("2026-10-01T10:00:00Z"),
                unassigning_linewright("2026-10-01T11:00:00Z"),
            ],
        )
    };
    let spec_change = MergedPull {
        assignees: vec![],
        events: vec![unassigning_linewright("2026-10-01T10:30:00Z")],
        ..spec_change(4, &[])
    };
    let view = View {
        memory: common::limited_until("2026-10-01T13:00:00Z"),
        ..with_merged(
            view(
                vec![
                    unassigned(blocked_by(ticket(1, "Blocked"), &[9])),
                    unassigned(assigned_to(ticket(2, "Theirs"), "alice")),
                    unassigned(ticket(3, "Let go")),
                ],
                vec![],
            ),
            vec![spec_change],
        )
    };

    let board = board(&view, &[], at(NOW));

    let mut placed = placed(&board);
    placed.sort_by_key(|(_, _, card)| *card);
    assert_eq!(
        placed,
        [
            (1, Column::Ready, 1),
            (2, Column::Ready, 2),
            (3, Column::Ready, 3),
            (4, Column::Ready, 4),
        ]
    );
    assert_eq!(
        [1, 2, 3, 4].map(|n| line(&board, n)),
        [
            Some(CardLine::BlockedBy(Number(9))),
            Some(CardLine::AssignedTo(Login::new("alice"))),
            Some(CardLine::Unassigned),
            Some(CardLine::Unassigned),
        ]
    );
}

/// Each of `numbers` with its card's card status.
fn statuses<const N: usize>(board: &Board, numbers: [u64; N]) -> [(u64, Option<CardStatus>); N] {
    numbers.map(|n| (n, card(board, n).status()))
}

/// `docs/specs/design_system.md`, Cards: the Card status column of the table
/// of lines, on the boards the tests of each line above set up.
#[test]
fn each_card_has_the_card_status_the_cards_table_gives_its_line() {
    use CardStatus::{Failed, InFlight, PausedOrQueued, WaitingForAPerson};
    let needing = view(
        vec![
            labeled(1, &["ready-for-human", label::STALLED]),
            labeled(2, &["needs-info"]),
            labeled(3, &["ready-for-human"]),
            labeled(4, &["ready-for-human"]),
            labeled(5, &["needs-triage"]),
        ],
        vec![],
    );
    let handed_back = [recorded(
        "3/implement-1",
        "2026-10-01T10:00:00Z",
        Ending::HandedBack,
    )];
    let unassigned = with_events(
        ticket(4, "Let go"),
        vec![
            labeling("ready-for-agent", "2026-10-01T09:00:00Z"),
            assigning_linewright("2026-10-01T10:00:00Z"),
            unassigning_linewright("2026-10-01T11:00:00Z"),
        ],
    );
    let ready = || {
        let mut view = ready_view();
        std::sync::Arc::make_mut(&mut view.snapshot)
            .issues
            .push(unassigned.clone());
        view
    };
    let limited = View {
        memory: common::limited_until("2026-10-01T13:00:00Z"),
        ..ready()
    };
    let draining = View {
        memory: common::draining_since("2026-10-01T11:00:00Z"),
        ..ready()
    };
    let full = View {
        live: (11..=14).map(common::implementing).collect(),
        ..ready()
    };
    let working = view(vec![assigned(ticket(1, "Implemented"))], vec![]);
    let running = with_runs(working.clone(), &["1/implement-1"]);
    let held = View {
        memory: common::limited_until("2026-10-01T13:00:00Z"),
        ..working
    };

    assert_eq!(
        statuses(&board(&needing, &handed_back, at(NOW)), [1, 2, 3, 4, 5]),
        [
            // Stalled.
            (1, Some(Failed)),
            // Asked.
            (2, Some(WaitingForAPerson)),
            // Handed back.
            (3, Some(WaitingForAPerson)),
            // Decision needed.
            (4, Some(WaitingForAPerson)),
            // Triage: nothing but the time.
            (5, None),
        ]
    );
    // Blocked, assigned to alice, nothing but the time, unassigned.
    assert_eq!(
        statuses(&board(&ready(), &[], at(NOW)), [1, 2, 3, 4]),
        [(1, None), (2, None), (3, None), (4, None)]
    );
    for paused_or_queued in [limited, draining, full] {
        assert_eq!(
            statuses(&board(&paused_or_queued, &[], at(NOW)), [3]),
            [(3, Some(PausedOrQueued))]
        );
    }
    assert_eq!(
        statuses(&board(&running, &[], at(NOW)), [1]),
        [(1, Some(InFlight))]
    );
    assert_eq!(
        statuses(&board(&held, &[], at(NOW)), [1]),
        [(1, Some(PausedOrQueued))]
    );
}

/// `docs/specs/design_system.md`, Cards: in review, a card's status is that
/// of what stops it merging, the Card status column of its table, on an
/// item's card or on the pull request's own.
#[test]
fn a_card_in_review_has_the_card_status_of_what_stops_it_merging() {
    use CardStatus::{Failed, InFlight, WaitingForAPerson};
    use CheckState::{Failed as Fails, Passed, Pending};
    use Mergeability::{Behind, Clean, Conflicting};
    use ReviewDecision::{Approved, ChangesRequested, ReviewRequired};
    let issues = (1..=10)
        .chain([40])
        .map(|n| assigned(ticket(n, "Reviewed")))
        .collect();
    let decided = |pull: Pull, review_decision| Pull {
        review_decision: Some(review_decision),
        ..pull
    };
    let stalled = Pull {
        labels: vec![label::STALLED.into()],
        ..reviewed(41, 40, &[Passed], Clean, false)
    };
    let pulls = vec![
        reviewed(11, 1, &[Fails, Pending], Conflicting, true),
        reviewed(12, 2, &[Fails, Pending], Conflicting, true),
        reviewed(13, 3, &[Passed, Pending], Conflicting, true),
        reviewed(14, 4, &[Passed], Conflicting, true),
        reviewed(15, 5, &[Passed], Behind, true),
        decided(
            reviewed(16, 6, &[Passed], Mergeability::Unknown, true),
            ChangesRequested,
        ),
        reviewed(17, 7, &[Passed], Clean, false),
        decided(reviewed(18, 8, &[Passed], Clean, false), ChangesRequested),
        decided(reviewed(19, 9, &[Passed], Clean, false), ReviewRequired),
        decided(reviewed(20, 10, &[Passed], Clean, false), Approved),
        stalled,
        // Pull requests closing no issue on the board, cards of their own.
        pull(30, &[98], &[Fails]),
        decided(pull(31, &[97], &[Passed]), Approved),
    ];
    let view = with_runs(view(issues, pulls), &["11/rework-1"]);

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        statuses(&board, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 40, 30, 31]),
        [
            // A run in flight.
            (1, Some(InFlight)),
            // Checks failed.
            (2, Some(Failed)),
            // Checks running.
            (3, Some(InFlight)),
            // Conflicts.
            (4, Some(Failed)),
            // Behind main.
            (5, None),
            // Draft.
            (6, None),
            // Nothing stops it.
            (7, None),
            // Changes requested.
            (8, Some(WaitingForAPerson)),
            // Review required.
            (9, Some(WaitingForAPerson)),
            // Approved.
            (10, None),
            // Stalled.
            (40, Some(Failed)),
            // Checks failed, on the pull request's own card.
            (30, Some(Failed)),
            // Approved, on the pull request's own card.
            (31, None),
        ]
    );
}

#[test]
fn a_card_in_done_has_no_card_status() {
    let mut closed_pull = closed_issue(30, "2026-09-29T12:00:00Z", None);
    closed_pull.kind = ClosedKind::Pull {
        closes: vec![Number(99)],
        author: Some(Login::new("factory")),
    };
    let view = with_merged(
        with_closed(
            view(vec![], vec![]),
            vec![
                closed_issue(1, "2026-09-28T12:00:00Z", Some(11)),
                closed_issue(2, "2026-09-28T13:00:00Z", None),
                closed_as(3, IssueCloseReason::Completed),
                closed_pull,
            ],
        ),
        vec![
            merged(11, &[1], "factory", "2026-09-28T12:00:00Z"),
            merged(40, &[98], "factory", "2026-09-30T12:00:00Z"),
            spec_change(5, &[label::DESIGNED]),
        ],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        statuses(&board, [1, 2, 3, 5, 30, 40]),
        [
            (1, None),
            (2, None),
            (3, None),
            (5, None),
            (30, None),
            (40, None)
        ]
    );
}

/// A person's issue #`number`, labeled `ready-for-agent`, closed by hand
/// within the window, that Linewright never worked.
fn persons_closed_issue(number: u64) -> ClosedItem {
    let mut closed = closed_issue(number, "2026-09-30T12:00:00Z", None);
    if let ClosedKind::Issue { facts, .. } = &mut closed.kind {
        facts.labels = vec!["ready-for-agent".into()];
        facts.work = WorkFacts::default();
    }
    closed
}

/// `view` with `linewright` as its required label.
fn requiring_label(mut view: View) -> View {
    view.policy.require_label = Some("linewright".into());
    view
}

/// Where the cards are with `linewright` the required label and `closed`
/// the one item, a person's issue closed within the window, worked as
/// `work` tells.
fn placed_requiring_label(work: WorkFacts) -> Vec<(u64, Column, u64)> {
    let mut closed = persons_closed_issue(1);
    if let ClosedKind::Issue { facts, .. } = &mut closed.kind {
        facts.work = work;
    }
    let view = requiring_label(with_closed(view(vec![], vec![]), vec![closed]));
    placed(&board(&view, &[], at(NOW)))
}

#[test]
fn with_a_required_label_a_closed_issue_without_it_that_linewright_never_worked_is_not_on_the_board()
 {
    let theirs = WorkFacts {
        author: Some(Login::new("maintainer")),
        assignees: vec![Login::new("alice")],
        ..WorkFacts::default()
    };

    assert_eq!(placed_requiring_label(theirs), []);
}

#[test]
fn with_a_required_label_a_closed_issue_carrying_it_that_linewright_never_worked_is_not_on_the_board()
 {
    let mut closed = persons_closed_issue(1);
    if let ClosedKind::Issue { facts, .. } = &mut closed.kind {
        facts.labels.push("linewright".into());
    }
    let view = requiring_label(with_closed(view(vec![], vec![]), vec![closed]));

    assert_eq!(placed(&board(&view, &[], at(NOW))), []);
}

#[test]
fn with_a_required_label_a_closed_issue_without_it_that_linewright_filed_is_in_done() {
    let filed = WorkFacts {
        author: Some(Login::new("factory")),
        ..WorkFacts::default()
    };

    assert_eq!(placed_requiring_label(filed), [(1, Column::Done, 1)]);
}

#[test]
fn with_a_required_label_a_closed_issue_without_it_assigned_to_linewright_by_hand_and_never_worked_is_not_on_the_board()
 {
    let assigned = WorkFacts {
        assignees: vec![Login::new("factory")],
        events: vec![IssueEvent::Assignment(Assignment::Assigned {
            assignee: Login::new("factory"),
            actor: Login::new("maintainer"),
            at: at("2026-09-29T10:00:00Z"),
        })],
        ..WorkFacts::default()
    };

    assert_eq!(placed_requiring_label(assigned), []);
}

#[test]
fn with_a_required_label_a_closed_issue_without_it_linewright_only_commented_on_is_in_done() {
    let commented = WorkFacts {
        events: vec![IssueEvent::Commented {
            by: Login::new("factory"),
            at: at("2026-09-29T10:00:00Z"),
        }],
        ..WorkFacts::default()
    };

    assert_eq!(placed_requiring_label(commented), [(1, Column::Done, 1)]);
}

#[test]
fn with_a_required_label_a_closed_issue_without_it_linewright_claimed_and_was_unassigned_from_is_in_done()
 {
    let handed_back = WorkFacts {
        events: vec![
            assigning_linewright("2026-09-29T10:00:00Z"),
            unassigning_linewright("2026-09-29T11:00:00Z"),
        ],
        ..WorkFacts::default()
    };

    assert_eq!(placed_requiring_label(handed_back), [(1, Column::Done, 1)]);
}

#[test]
fn with_a_required_label_a_closed_issue_without_it_closed_by_a_pull_request_of_linewrights_is_in_done()
 {
    let closed_by_ours = WorkFacts {
        closed_by: vec![ClosingPull {
            number: Number(10),
            base_ref: "main".into(),
            author: Login::new("factory"),
            state: PullState::Closed {
                at: at("2026-09-30T11:00:00Z"),
            },
            updated_at: at("2026-09-30T11:00:00Z"),
        }],
        ..WorkFacts::default()
    };

    assert_eq!(
        placed_requiring_label(closed_by_ours),
        [(1, Column::Done, 1)]
    );
}

#[test]
fn with_no_required_label_a_closed_issue_linewright_never_worked_is_not_on_the_board() {
    let view = with_closed(view(vec![], vec![]), vec![persons_closed_issue(1)]);

    assert_eq!(placed(&board(&view, &[], at(NOW))), []);
}

#[test]
fn a_merged_pull_request_of_linewrights_closing_an_issue_left_off_the_board_is_a_card_of_its_own() {
    // As in a snapshot taken before Linewright read the pull requests
    // closing a closed issue.
    let view = requiring_label(with_merged(
        with_closed(view(vec![], vec![]), vec![persons_closed_issue(1)]),
        vec![merged(10, &[1], "factory", "2026-09-30T11:59:00Z")],
    ));

    assert_eq!(
        placed(&board(&view, &[], at(NOW))),
        [(10, Column::Done, 10)]
    );
}

#[test]
fn an_unfinished_lane_keeps_a_ticket_closed_before_the_done_window_in_done() {
    let view = with_closed(
        view(
            vec![
                tracking_issue(10, 9, &[(11, true), (12, false)]),
                cut_from(ticket(11, "Open"), 10, 9),
            ],
            vec![],
        ),
        vec![closed_ticket(12, 10, "2026-09-10T12:00:00Z")],
    );

    assert_eq!(
        placed(&board(&view, &[], at(NOW))),
        [(10, Column::Ready, 11), (10, Column::Done, 12)]
    );
}

#[test]
fn a_finished_lane_shows_every_ticket_until_it_finished_before_the_done_window_then_leaves_whole() {
    let view = with_closed(
        view(vec![], vec![]),
        vec![
            closed_tracking_issue(10, 9, &[(11, false), (12, false)], "2026-09-28T12:00:00Z"),
            closed_tracking_issue(20, 19, &[(21, false), (22, false)], "2026-09-20T12:00:00Z"),
            closed_ticket(11, 10, "2026-09-10T12:00:00Z"),
            closed_ticket(12, 10, "2026-09-28T12:00:00Z"),
            closed_ticket(21, 20, "2026-09-10T12:00:00Z"),
            closed_ticket(22, 20, "2026-09-20T12:00:00Z"),
        ],
    );

    assert_eq!(
        placed(&board(&view, &[], at(NOW))),
        [(10, Column::Done, 11), (10, Column::Done, 12)]
    );
}

#[test]
fn a_spec_change_designed_with_nothing_to_build_entered_done_and_finished_when_it_was_designed() {
    let merged_before = |number: u64, designed: Option<&str>| MergedPull {
        merged_at: at("2026-09-01T09:00:00Z"),
        updated_at: at(designed.unwrap_or("2026-09-01T09:00:00Z")),
        events: designed
            .map(|when| labeling(label::DESIGNED, when))
            .into_iter()
            .collect(),
        ..spec_change(number, &[label::DESIGNED])
    };
    let view = with_merged(
        view(vec![], vec![]),
        vec![
            merged_before(5, Some("2026-09-29T09:00:00Z")),
            merged_before(6, Some("2026-09-10T09:00:00Z")),
            merged_before(7, None),
        ],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(lanes(&board), [(5, Some(at("2026-09-29T09:00:00Z")))]);
    assert_eq!(
        card(&board, 5).line,
        Some(CardLine::Ended {
            ended: Ended::NothingToBuild,
            at: at("2026-09-29T09:00:00Z"),
        })
    );
}

/// Ticket #`number`, which Linewright labeled `ready-for-agent` and claimed,
/// and whose status label a person removed since.
fn taken_back(number: u64) -> Issue {
    Issue {
        labels: vec![label::DESIGNED.into()],
        assignees: vec![Login::new("factory")],
        events: vec![
            IssueEvent::Labeled {
                label: "ready-for-agent".into(),
                by: Some(Login::new("factory")),
                at: at("2026-09-30T10:00:00Z"),
            },
            assigning_linewright("2026-09-30T10:00:00Z"),
            IssueEvent::Unlabeled {
                label: "ready-for-agent".into(),
                at: at("2026-09-30T11:00:00Z"),
            },
        ],
        ..ticket(number, &format!("Ticket {number}"))
    }
}

#[test]
fn an_issue_linewright_worked_whose_status_label_a_person_removed_has_no_card() {
    let view = view(vec![taken_back(1)], vec![]);

    assert_eq!(placed(&board(&view, &[], at(NOW))), []);
}

#[test]
fn an_issue_with_no_status_label_is_in_working_while_its_run_is_in_flight() {
    let view = with_runs(view(vec![taken_back(1)], vec![]), &["1/implement-1"]);

    let board = board(&view, &[], at(NOW));

    assert_eq!(placed(&board), [(1, Column::Working, 1)]);
    assert!(matches!(line(&board, 1), Some(CardLine::Running(_))));
}

#[test]
fn an_issue_with_no_status_label_is_in_review_while_an_open_pull_request_closes_it() {
    let view = view(
        vec![taken_back(1), taken_back(2)],
        vec![pull(10, &[1], &[CheckState::Pending])],
    );
    let mut theirs = pull(20, &[2], &[]);
    theirs.author = Login::new("alice");
    theirs.assignees = vec![];
    let view = with_peoples_pulls(view, vec![theirs]);

    assert_eq!(
        placed(&board(&view, &[], at(NOW))),
        [(1, Column::InReview, 1), (2, Column::InReview, 2)]
    );
}

fn with_peoples_pulls(mut view: View, pulls: Vec<Pull>) -> View {
    let snapshot = std::sync::Arc::make_mut(&mut view.snapshot);
    for pull in &pulls {
        for &issue in &pull.closes {
            if let Some(held) = snapshot.issues.iter_mut().find(|i| i.number == issue) {
                held.closed_by.push(ClosingPull::from(pull));
            }
        }
    }
    snapshot.peoples_pulls = pulls;
    view
}

/// `alice`'s open pull request #`number` closing each of `closes`, that
/// Linewright is assigned to.
fn persons_pull(number: u64, closes: &[u64]) -> Pull {
    Pull {
        author: Login::new("alice"),
        ..pull(number, closes, &[CheckState::Pending])
    }
}

#[test]
fn a_persons_pull_request_linewright_is_assigned_to_or_worked_is_its_own_card_unless_it_closes_an_issue_with_one()
 {
    let commented = Pull {
        assignees: vec![],
        comments: vec![Comment {
            author: Login::new("factory"),
            body: "Looks good.".into(),
            created_at: at("2026-10-01T11:00:00Z"),
        }],
        ..persons_pull(21, &[])
    };
    let view = with_peoples_pulls(
        view(vec![ticket(1, "Ready")], vec![]),
        vec![persons_pull(20, &[1]), commented, persons_pull(22, &[9])],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        placed(&board),
        [
            (1, Column::InReview, 1),
            (21, Column::InReview, 21),
            (22, Column::InReview, 22),
        ]
    );
    assert_eq!(
        line(&board, 1),
        Some(CardLine::Review {
            pull_request: Number(20),
            author: Some(Login::new("alice")),
            stops: Some(Stop::ChecksRunning),
        })
    );
    assert!(card(&board, 22).pull_request);
    assert_eq!(
        line(&board, 22),
        Some(CardLine::MergeStop(Stop::ChecksRunning))
    );
}

#[test]
fn a_pull_request_linewright_worked_that_merged_or_closed_unmerged_is_in_done_whoever_opened_it() {
    let mut closed = closed_issue(31, "2026-09-30T12:00:00Z", None);
    closed.kind = ClosedKind::Pull {
        closes: vec![],
        author: Some(Login::new("alice")),
    };
    let commented = MergedPull {
        events: vec![IssueEvent::Commented {
            by: Login::new("factory"),
            at: at("2026-09-30T10:00:00Z"),
        }],
        ..merged(30, &[], "alice", "2026-09-30T11:00:00Z")
    };
    let untouched = merged(32, &[], "alice", "2026-09-30T11:00:00Z");
    let view = with_merged(
        with_closed(view(vec![], vec![]), vec![closed]),
        vec![commented, untouched],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        placed(&board),
        [(31, Column::Done, 31), (30, Column::Done, 30)]
    );
    assert_eq!(
        [30, 31].map(|n| line(&board, n)),
        [
            Some(CardLine::Ended {
                ended: Ended::Merged,
                at: at("2026-09-30T11:00:00Z"),
            }),
            Some(CardLine::Ended {
                ended: Ended::ClosedUnmerged,
                at: at("2026-09-30T12:00:00Z"),
            }),
        ]
    );
}

/// `docs/specs/design_system.md`, The board, Finished lanes: a lane with a
/// tracking issue is finished only once that issue is closed.
#[test]
fn a_lane_whose_tickets_are_all_closed_is_unfinished_while_its_tracking_issue_is_open() {
    let view = with_closed(
        view(
            vec![
                tracking_issue(10, 9, &[(11, false)]),
                updated(labeled(2, &["needs-triage"]), "2026-09-29T12:00:00Z"),
            ],
            vec![],
        ),
        vec![
            closed_ticket(11, 10, "2026-09-30T12:00:00Z"),
            closed_tracking_issue(30, 29, &[(31, false)], "2026-10-01T08:00:00Z"),
            closed_ticket(31, 30, "2026-10-01T07:00:00Z"),
        ],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        lanes(&board),
        [
            (10, None),
            (2, None),
            (30, Some(at("2026-10-01T08:00:00Z"))),
        ]
    );
    assert_eq!(
        placed(&board),
        [
            (10, Column::Done, 11),
            (2, Column::Triage, 2),
            (30, Column::Done, 31),
        ]
    );
}

/// `docs/specs/design_system.md`, The board, Finished lanes: when the last
/// of its cards entered Done or its tracking issue closed, whichever is
/// later.
#[test]
fn a_lane_finished_when_its_last_card_entered_done_or_its_tracking_issue_closed_whichever_is_later()
{
    let view = with_closed(
        view(vec![], vec![]),
        vec![
            closed_tracking_issue(10, 9, &[(11, false)], "2026-09-30T15:00:00Z"),
            closed_ticket(11, 10, "2026-09-30T12:00:00Z"),
            closed_tracking_issue(20, 19, &[(21, false)], "2026-09-29T12:00:00Z"),
            closed_ticket(21, 20, "2026-09-30T09:00:00Z"),
        ],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(
        lanes(&board),
        [
            (10, Some(at("2026-09-30T15:00:00Z"))),
            (20, Some(at("2026-09-30T09:00:00Z"))),
        ]
    );
    assert_eq!(
        board.lanes[0].head,
        LaneHead::Tracking {
            number: Number(10),
            title: "Tracking 10".into(),
            tickets_closed: 1,
            tickets: 1,
            closed: Some(at("2026-09-30T15:00:00Z")),
        }
    );
}

/// `docs/specs/design_system.md`, The board, Lanes: a lane's cards are its
/// managed tickets, and its head counts every ticket.
#[test]
fn a_ticket_that_is_not_managed_has_no_card_in_its_lane_and_its_head_still_counts_it() {
    let view = view(
        vec![
            tracking_issue(10, 9, &[(11, true), (12, true)]),
            cut_from(ticket(11, "Managed"), 10, 9),
            cut_from(taken_back(12), 10, 9),
        ],
        vec![],
    );

    let board = board(&view, &[], at(NOW));

    assert_eq!(placed(&board), [(10, Column::Ready, 11)]);
    assert_eq!(
        heads(&board),
        [LaneHead::Tracking {
            number: Number(10),
            title: "Tracking 10".into(),
            tickets_closed: 0,
            tickets: 2,
            closed: None,
        }]
    );
}
