use crate::common;

use common::{NOW, at, ticket, view};
use linewright::board::{Board, Card, CardLine, Column, Ended, LaneHead, board};
use linewright::domain::*;
use linewright::status::{Ci, View};

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

    let board = board(&view, at(NOW));

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
        placed(&board(&view, at(NOW))),
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

    assert_eq!(placed(&board(&view, at(NOW))), [(1, Column::InReview, 1)]);
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

    assert_eq!(placed(&board(&view, at(NOW))), [(2, Column::Done, 2)]);
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

    let board = board(&view, at(NOW));

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

#[test]
fn a_tickets_card_carries_its_pull_requests_ci_state_and_a_rework_run_on_it() {
    let view = with_runs(
        view(
            vec![assigned(ticket(1, "Reworked"))],
            vec![pull(10, &[1], &[CheckState::Failed])],
        ),
        &["10/rework-1"],
    );

    let board = board(&view, at(NOW));

    let card = card(&board, 1);
    assert_eq!(
        card.pull.map(|pull| (pull.pull_request, pull.ci)),
        Some((Number(10), Ci::Failing))
    );
    let CardLine::Run(run) = &card.line else {
        panic!("no run: {:?}", card.line);
    };
    assert_eq!(run.id.to_string(), "10/rework-1");
    assert_eq!(placed(&board), [(1, Column::InReview, 1)]);
}

#[test]
fn linewrights_pull_request_closing_no_issue_on_the_board_is_a_card_of_its_own() {
    let view = view(vec![], vec![pull(10, &[1], &[CheckState::Passed])]);

    let board = board(&view, at(NOW));

    assert_eq!(placed(&board), [(10, Column::InReview, 10)]);
    assert_eq!(
        card(&board, 10).standing.to_string(),
        "in review: #10 (ready for review, CI green)"
    );
}

#[test]
fn a_card_in_done_says_how_its_item_ended() {
    let mut closed_pull = closed_issue(30, "2026-09-29T12:00:00Z", None);
    closed_pull.kind = ClosedKind::Pull {
        closes: vec![Number(99)],
    };
    let view = with_closed(
        view(vec![], vec![]),
        vec![
            closed_issue(1, "2026-09-28T12:00:00Z", Some(11)),
            closed_issue(2, "2026-09-28T12:00:00Z", None),
            closed_pull,
        ],
    );

    let board = board(&view, at(NOW));

    let ended: Vec<(u64, String, CardLine)> = [1, 2, 30]
        .map(|n| {
            let card = card(&board, n);
            (n, card.standing.to_string(), card.line.clone())
        })
        .to_vec();
    assert_eq!(
        ended,
        [
            (
                1,
                "closed issue".into(),
                CardLine::Ended {
                    ended: Ended::MergedBy(Number(11)),
                    at: at("2026-09-28T12:00:00Z"),
                }
            ),
            (
                2,
                "closed issue".into(),
                CardLine::Ended {
                    ended: Ended::Closed,
                    at: at("2026-09-28T12:00:00Z"),
                }
            ),
            (
                30,
                "closed pull request".into(),
                CardLine::Ended {
                    ended: Ended::ClosedUnmerged,
                    at: at("2026-09-29T12:00:00Z"),
                }
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

    assert_eq!(placed(&board(&waiting, at(NOW))), [(5, Column::Ready, 5)]);
    assert_eq!(
        placed(&board(&designing, at(NOW))),
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
        placed(&board(&view, at(NOW))),
        [(6, Column::Ready, 7), (6, Column::Working, 8)]
    );
}

#[test]
fn a_spec_change_designed_with_nothing_to_build_is_in_done_saying_so() {
    let view = with_merged(
        view(vec![], vec![]),
        vec![spec_change(5, &[label::DESIGNED])],
    );

    let board = board(&view, at(NOW));

    assert_eq!(placed(&board), [(5, Column::Done, 5)]);
    assert_eq!(
        card(&board, 5).line,
        CardLine::Ended {
            ended: Ended::NothingToBuild,
            at: at("2026-10-01T09:00:00Z"),
        }
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

    let board = board(&view, at(NOW));

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

    let board = board(&view, at(NOW));

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
fn an_issue_in_review_on_someone_elses_pull_request_carries_its_ci_state() {
    let mut theirs = pull(10, &[1], &[CheckState::Pending]);
    theirs.author = Login::new("someone");
    let view = view(vec![ticket(1, "Theirs")], vec![theirs]);

    let board = board(&view, at(NOW));

    assert_eq!(placed(&board), [(1, Column::InReview, 1)]);
    assert_eq!(
        card(&board, 1)
            .pull
            .map(|pull| (pull.pull_request, pull.ci)),
        Some((Number(10), Ci::Running))
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
        lanes(&board(&view, at(NOW))),
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

    assert_eq!(lanes(&board(&view, at(NOW))), [(12, None), (10, None)]);
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
        lanes(&board(&view, at(NOW))),
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
        lanes(&board(&view, at(NOW))),
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

    let board = board(&view, at(NOW));

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

    assert_eq!(placed(&board(&view, at(NOW))), [(10, Column::Ready, 11)]);
}
