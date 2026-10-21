use crate::common;

use common::{NOW, at, ticket, view};
use linewright::board::{
    Board, Card, CardLine, Column, Ended, Hold, LaneHead, Running, Stop, board,
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

fn labeled(number: u64, labels: &[&str]) -> Issue {
    Issue {
        labels: labels.iter().map(ToString::to_string).collect(),
        ..ticket(number, &format!("Ticket {number}"))
    }
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
            (2, Column::NeedsAPerson, 2),
            (3, Column::NeedsAPerson, 3),
            (1, Column::Triage, 1),
            (4, Column::Ready, 4),
            (5, Column::Ready, 5),
        ]
    );
    assert_eq!(board.needing_a_person, 2);
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

fn closed_issue(number: u64, closed_at: &str, merged_by: Option<u64>) -> ClosedItem {
    ClosedItem {
        number: Number(number),
        title: format!("Ticket {number}"),
        closed_at: at(closed_at),
        kind: ClosedKind::Issue {
            parent: None,
            filed: None,
            sub_issues: vec![],
            labels: vec!["ready-for-agent".into(), label::DESIGNED.into()],
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
            (1, Column::Working, 1),
            (2, Column::Working, 2),
            (3, Column::Working, 3),
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
fn a_ticket_closed_within_the_spec_lookback_window_is_in_done_and_one_closed_before_is_nowhere() {
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
    let issues = (1..=10).map(|n| assigned(ticket(n, "Reviewed"))).collect();
    let decided = |pull: Pull, review_decision| Pull {
        review_decision: Some(review_decision),
        ..pull
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
        (1..=10).map(|n| line(&board, n)).collect::<Vec<_>>(),
        [
            review(
                11,
                Some(Stop::Running(Running {
                    stage: WorkKind::Rework,
                    started_at: at("2026-10-01T11:50:00Z"),
                    seconds: 630,
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
            review(18, Some(Stop::Review(ChangesRequested))),
            review(19, Some(Stop::Review(ReviewRequired))),
            review(20, Some(Stop::Review(Approved))),
        ]
    );
    assert!((1..=10).all(|n| !card(&board, n).pull_request));
}

#[test]
fn linewrights_pull_request_closing_no_issue_on_the_board_is_a_card_of_its_own() {
    let view = view(vec![], vec![pull(10, &[1], &[CheckState::Pending])]);

    let board = board(&view, &[], at(NOW));

    assert_eq!(placed(&board), [(10, Column::InReview, 10)]);
    let card = card(&board, 10);
    assert!(card.pull_request);
    assert_eq!(
        card.line,
        Some(CardLine::Review {
            pull_request: Number(10),
            author: None,
            stops: Some(Stop::ChecksRunning),
        })
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
            // Linewright's, merged before the spec lookback window opened.
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

fn spec_change(number: u64, labels: &[&str]) -> MergedPull {
    MergedPull {
        number: Number(number),
        title: "Add invoices to the spec".into(),
        body: String::new(),
        author: Login::new("maintainer"),
        merged_at: at("2026-10-01T09:00:00Z"),
        base_ref: "main".into(),
        labels: labels.iter().map(ToString::to_string).collect(),
        closes: vec![],
        files: vec!["docs/specs/invoices.md".into()],
        assignees: vec![Login::new("factory")],
        events: vec![],
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
        filed: None,
        sub_issues: vec![],
        labels: vec![label::DESIGNED.into()],
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
                tickets: 2
            },
            LaneHead::Tracking {
                number: Number(20),
                title: "Tracking 20".into(),
                tickets_closed: 1,
                tickets: 3
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
        board.lanes[1].head,
        LaneHead::Tracking {
            number: Number(30),
            title: "Cut".into(),
            tickets_closed: 1,
            tickets: 2
        }
    );
    assert_eq!(
        placed(&board),
        [(34, Column::NeedsAPerson, 34), (30, Column::Ready, 31)]
    );
}

#[test]
fn an_issue_in_review_on_someone_elses_pull_request_names_them() {
    let mut theirs = pull(10, &[1], &[CheckState::Pending]);
    theirs.author = Login::new("someone");
    let unheld = Issue {
        closed_by: vec![ClosingPull {
            number: Number(20),
            author: Login::new("alice"),
            state: PullState::Open { draft: true },
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
        filed: None,
        sub_issues: vec![],
        labels: vec![label::DESIGNED.into()],
        merged_by: Some(Number(number + 100)),
        reason: None,
    };
    closed
}

/// Each lane's number, and when it finished, if it did.
fn lanes(board: &Board) -> Vec<(u64, Option<Timestamp>)> {
    board
        .lanes
        .iter()
        .map(|lane| (lane.head.number().0, lane.finished()))
        .collect()
}

#[test]
fn lanes_needing_a_person_come_first_then_the_other_unfinished_then_the_finished_latest_first() {
    let view = with_closed(
        view(
            vec![
                labeled(2, &["needs-triage"]),
                tracking_issue(10, 9, &[(11, true)]),
                cut_from(ticket(11, "Ready"), 10, 9),
                tracking_issue(20, 19, &[(21, true), (22, false)]),
                cut_from(labeled(21, &["needs-info"]), 20, 19),
                tracking_issue(30, 29, &[(31, false), (32, false)]),
                tracking_issue(40, 39, &[(41, false)]),
                tracking_issue(50, 49, &[(51, false)]),
            ],
            vec![],
        ),
        vec![
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
            (20, None),
            (2, None),
            (10, None),
            (40, Some(at("2026-09-30T12:00:00Z"))),
            (30, Some(at("2026-09-29T12:00:00Z"))),
            (50, Some(at("2026-09-29T12:00:00Z"))),
        ]
    );
}

#[test]
fn an_items_lane_comes_first_when_it_needs_a_person() {
    let view = view(
        vec![
            labeled(12, &["needs-info"]),
            tracking_issue(10, 9, &[(11, true)]),
            cut_from(ticket(11, "Ready"), 10, 9),
        ],
        vec![],
    );

    assert_eq!(lanes(&board(&view, &[], at(NOW))), [(12, None), (10, None)]);
}

#[test]
fn an_item_closed_in_the_window_is_a_finished_lane_ordered_latest_finished_first() {
    let view = with_merged(
        with_closed(
            view(vec![tracking_issue(30, 29, &[(31, false)])], vec![]),
            vec![
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
fn lanes_are_in_number_order_within_their_group() {
    let view = view(
        vec![
            labeled(2, &["needs-info"]),
            tracking_issue(5, 4, &[(6, true)]),
            cut_from(ticket(6, "Ready"), 5, 4),
            tracking_issue(20, 19, &[(21, true)]),
            cut_from(labeled(21, &["ready-for-human"]), 20, 19),
            tracking_issue(10, 9, &[(11, true)]),
            cut_from(labeled(11, &["needs-info"]), 10, 9),
        ],
        vec![],
    );

    assert_eq!(
        lanes(&board(&view, &[], at(NOW))),
        [(2, None), (10, None), (20, None), (5, None)]
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

    assert_eq!(
        heads(&board),
        [
            LaneHead::Item {
                number: Number(2),
                title: "Standalone".into()
            },
            LaneHead::Item {
                number: Number(5),
                title: "Add invoices".into()
            },
            LaneHead::Item {
                number: Number(30),
                title: "Tidy up".into()
            },
        ]
    );
    assert_eq!(
        placed(&board),
        [
            (2, Column::Triage, 2),
            (5, Column::Ready, 5),
            (30, Column::InReview, 30),
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
fn a_card_needing_a_person_says_the_reporter_was_asked_it_was_handed_back_or_a_decision_is_needed()
{
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
            Some(CardLine::ReporterAsked),
            Some(CardLine::HandedBack),
            Some(CardLine::DecisionNeeded),
            Some(CardLine::DecisionNeeded),
            Some(CardLine::HandedBack),
            Some(CardLine::DecisionNeeded),
            Some(CardLine::DecisionNeeded),
        ]
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
            started_at: at("2026-10-01T11:50:00Z"),
            seconds: 630,
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
