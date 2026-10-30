//! The item page's facts (`docs/specs/design_system.md`, The item page),
//! through the functional core that finds them.

use crate::common;

use common::{NOW, at, closed_item, labeled_by_linewright, read_closed, sub_issue, ticket, view};
use linewright::board::{CardLine, CardStatus, Column, Ended, LaneHead, Stop, board};
use linewright::domain::*;
use linewright::explain::{explain, explain_fresh, fresh_view};
use linewright::item_facts::{ItemFacts, Related, StateFact, item_facts};
use linewright::managed::{LeftAloneIssue, LeftAloneItem};
use linewright::reconcile::Wait;
use linewright::status::View;
use linewright::store::AnyItem;

/// #`number`'s facts, as `serve` finds them for an item its view manages.
fn facts_of(view: &View, number: u64) -> ItemFacts {
    let answer = explain(view, &[], Number(number), at(NOW)).expect("a managed item");
    let answer = linewright::explain::Answer::Managed(answer);
    item_facts(&answer, view, &board(view, &[], at(NOW)), &[], at(NOW))
}

/// The facts of `read`, as `serve` finds them for an item it reads from
/// GitHub.
fn facts_read(view: &View, read: &AnyItem) -> ItemFacts {
    let fresh = fresh_view(view, read, at(NOW));
    let answer = explain_fresh(&fresh, &[], read, at(NOW));
    item_facts(&answer, &fresh, &board(&fresh, &[], at(NOW)), &[], at(NOW))
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
            .map(|&(n, open)| sub_issue(n, open, "factory"))
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

fn related(number: u64, title: &str, pull_request: bool, status: Option<CardStatus>) -> Related {
    Related::Card {
        number: Number(number),
        title: Some(title.into()),
        pull_request,
        status,
    }
}

/// What the lane `related` is shown as the head of is the work of.
fn lane_head(related: Option<&Related>) -> &LaneHead {
    let Some(Related::LaneHead(heading)) = related else {
        panic!("no lane head: {related:?}");
    };
    &heading.head
}

/// Its column and its card's line.
fn card_state(facts: &ItemFacts) -> (Column, Option<CardLine>) {
    let StateFact::Card { column, card } = &facts.state else {
        panic!("no card: {:?}", facts.state);
    };
    (*column, card.line.clone())
}

#[test]
fn a_ticket_in_ready_blocked_by_another_is_in_ready_saying_so_under_its_tracking_issue() {
    let blocked = Issue {
        blocked_by: vec![Linked {
            number: Number(2),
            open: true,
        }],
        ..cut_from(ticket(1, "First"), 20, 5)
    };
    let view = view(
        vec![
            tracking_issue(20, 5, &[(1, true), (2, true)]),
            blocked,
            cut_from(ticket(2, "Second"), 20, 5),
        ],
        vec![],
    );

    let facts = facts_of(&view, 1);

    assert_eq!(
        card_state(&facts),
        (Column::Ready, Some(CardLine::BlockedBy(Number(2))))
    );
    assert_eq!(
        lane_head(facts.tracking_issue.as_ref()),
        &LaneHead::Tracking {
            number: Number(20),
            title: "Tracking 20".into(),
            tickets_closed: 0,
            tickets: 2,
            closed: None,
        }
    );
    assert_eq!(facts.tickets, None);
    assert_eq!(facts.spec_change, None);
    assert_eq!(facts.pull_requests, []);
    assert_eq!(facts.closes, []);
}

/// Linewright's open pull request #`number` closing #`closes`, with checks
/// in `checks`.
fn pull(number: u64, closes: u64, checks: &[CheckState]) -> Pull {
    Pull {
        number: Number(number),
        title: format!("Pull {number}"),
        body: format!("Closes #{closes}"),
        author: Login::new("factory"),
        created_at: at("2026-10-01T11:58:00Z"),
        updated_at: at("2026-10-01T11:58:00Z"),
        base_ref: "main".into(),
        head_ref: format!("{closes}-do"),
        head_sha: "abc123".into(),
        state: PullState::Open { draft: false },
        closes: vec![Number(closes)],
        pushes: vec![],
        mergeability: Mergeability::Clean,
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

fn assigned(issue: Issue) -> Issue {
    Issue {
        assignees: vec![Login::new("factory")],
        ..issue
    }
}

/// #1, in review on #10, whose checks failed.
fn in_review_with_checks_failed() -> View {
    view(
        vec![assigned(ticket(1, "Do the thing"))],
        vec![pull(10, 1, &[CheckState::Failed])],
    )
}

#[test]
fn an_issue_in_review_whose_pull_requests_checks_failed_says_so_and_lists_the_pull_request() {
    let facts = facts_of(&in_review_with_checks_failed(), 1);

    assert_eq!(
        card_state(&facts),
        (
            Column::InReview,
            Some(CardLine::Review {
                pull_request: Number(10),
                author: None,
                stops: Some(Stop::ChecksFailed),
            })
        )
    );
    assert!(!facts.pull_request);
    assert_eq!(
        facts.pull_requests,
        [related(10, "Pull 10", true, Some(CardStatus::Failed))]
    );
    assert_eq!(facts.closes, []);
}

#[test]
fn a_pull_request_shown_on_the_card_of_the_issue_it_closes_has_the_card_it_would_have_on_its_own() {
    let facts = facts_of(&in_review_with_checks_failed(), 10);

    assert_eq!(
        card_state(&facts),
        (
            Column::InReview,
            Some(CardLine::MergeStop(Stop::ChecksFailed))
        )
    );
    assert!(facts.pull_request);
    assert_eq!(
        facts.closes,
        [related(1, "Do the thing", false, Some(CardStatus::Failed))]
    );
    assert_eq!(facts.pull_requests, []);
}

/// Ticket #`number` of #20, which Linewright claimed, closed at `closed_at`.
fn closed_ticket(number: u64, closed_at: &str) -> ClosedItem {
    ClosedItem {
        number: Number(number),
        title: format!("Ticket {number}"),
        closed_at: at(closed_at),
        updated_at: at(closed_at),
        kind: ClosedKind::Issue {
            parent: Some(Parent {
                number: Number(20),
                spec_change: Some(Number(15)),
            }),
            facts: ManagedFacts {
                filed: None,
                sub_issues: vec![],
                labels: vec!["ready-for-agent".into(), label::DESIGNED.into()],
                work: WorkFacts {
                    assignees: vec![Login::new("factory")],
                    events: vec![common::assigning_linewright("2026-09-28T10:00:00Z")],
                    ..WorkFacts::default()
                },
            },
            merged_by: None,
            reason: Some(IssueCloseReason::Completed),
        },
    }
}

fn spec_change(number: u64) -> MergedPull {
    MergedPull {
        number: Number(number),
        title: "Add invoices to the spec".into(),
        body: String::new(),
        author: Login::new("maintainer"),
        merged_at: at("2026-10-01T09:00:00Z"),
        updated_at: at("2026-10-01T09:00:00Z"),
        base_ref: "main".into(),
        labels: vec![label::DESIGNED.into()],
        closes: vec![],
        files: vec!["docs/specs/invoices.md".into()],
        assignees: vec![Login::new("factory")],
        events: vec![],
    }
}

/// Tracking issue #20 of spec change #15, with tickets #1 to #5: #1 and #2
/// closed in the window, #3 closed before it, #4 in review with its checks
/// failed and #5 in Ready.
fn tracking_five() -> View {
    let mut view = view(
        vec![
            tracking_issue(
                20,
                15,
                &[(1, false), (2, false), (3, false), (4, true), (5, true)],
            ),
            assigned(cut_from(ticket(4, "Fourth"), 20, 15)),
            cut_from(ticket(5, "Fifth"), 20, 15),
        ],
        vec![pull(10, 4, &[CheckState::Failed])],
    );
    let snapshot = std::sync::Arc::make_mut(&mut view.snapshot);
    snapshot.closed = vec![
        closed_ticket(1, "2026-09-29T12:00:00Z"),
        closed_ticket(2, "2026-09-30T12:00:00Z"),
    ];
    snapshot.merged = vec![spec_change(15)];
    view
}

/// `docs/specs/design_system.md`, The item page: an unfinished tracking
/// issue's State is "Tracking" alone.
#[test]
fn a_tracking_issue_is_tracking_and_has_its_tickets_closed_of_total() {
    let facts = facts_of(&tracking_five(), 20);

    assert_eq!(facts.state, StateFact::Tracking { finished: None });
    let tickets = facts.tickets.expect("tickets");
    assert_eq!(tickets.closed, 3);
    assert_eq!(
        tickets.tickets,
        [
            related(1, "Ticket 1", false, None),
            related(2, "Ticket 2", false, None),
            Related::Card {
                number: Number(3),
                title: None,
                pull_request: false,
                status: None,
            },
            related(4, "Fourth", false, Some(CardStatus::Failed)),
            related(5, "Fifth", false, None),
        ]
    );
    assert_eq!(
        facts.spec_change,
        Some(related(15, "Add invoices to the spec", true, None))
    );
    assert_eq!(facts.tracking_issue, None);
}

#[test]
fn a_merged_spec_change_names_the_tracking_issue_cut_from_it() {
    let view = tracking_five();
    let read = AnyItem::Merged {
        pull: Box::new(spec_change(15)),
        tracking_issues: vec![Number(20)],
    };

    let facts = facts_read(&view, &read);

    assert_eq!(
        lane_head(facts.tracking_issue.as_ref()),
        &LaneHead::Tracking {
            number: Number(20),
            title: "Tracking 20".into(),
            tickets_closed: 3,
            tickets: 5,
            closed: None,
        }
    );
    assert!(facts.pull_request);
}

/// Tracking issue #20 of spec change #15, closed at `closed_at`, whose
/// tickets #1 and #2 closed in the Done window, the last at 2026-09-30
/// 12:00.
fn tracking_closed_at(closed_at: &str) -> View {
    let mut view = view(vec![], vec![]);
    let snapshot = std::sync::Arc::make_mut(&mut view.snapshot);
    snapshot.closed = vec![
        closed_item(
            &tracking_issue(20, 15, &[(1, false), (2, false)]),
            closed_at,
        ),
        closed_ticket(1, "2026-09-29T12:00:00Z"),
        closed_ticket(2, "2026-09-30T12:00:00Z"),
    ];
    snapshot.merged = vec![spec_change(15)];
    view
}

#[test]
fn a_tracking_issue_whose_lane_is_finished_is_tracking_and_when_its_last_ticket_closed() {
    let facts = facts_of(&tracking_closed_at("2026-09-30T12:00:00Z"), 20);

    assert_eq!(
        facts.state,
        StateFact::Tracking {
            finished: Some(at("2026-09-30T12:00:00Z"))
        }
    );
}

/// `docs/specs/design_system.md`, The board, Finished lanes: a lane
/// finished when its last ticket closed or its tracking issue closed,
/// whichever is later, on the board or off it.
#[test]
fn a_tracking_issue_closed_after_its_last_ticket_finished_when_it_closed() {
    let on_board = tracking_closed_at("2026-09-30T18:00:00Z");
    let (off_board, tracking) = tracking_finished_before_the_window(false);

    let on_board = facts_of(&on_board, 20);
    let off_board = facts_read(&off_board, &read_closed(tracking, "2026-09-24T12:00:00Z"));

    assert_eq!(
        [on_board.state, off_board.state],
        [
            StateFact::Tracking {
                finished: Some(at("2026-09-30T18:00:00Z"))
            },
            StateFact::Tracking {
                finished: Some(at("2026-09-24T12:00:00Z"))
            },
        ]
    );
}

/// Tracking issue #20 of spec change #15, open or not, whose tickets #1 and
/// #2 closed before the Done window opened, so that its lane left the board
/// once the issue is closed.
fn tracking_finished_before_the_window(open: bool) -> (View, Issue) {
    let closed_at = |number, at: &str| SubIssue {
        state: SubIssueState::Closed {
            at: Some(common::at(at)),
        },
        ..sub_issue(number, false, "factory")
    };
    let tracking = Issue {
        open,
        sub_issues: vec![
            closed_at(1, "2026-09-20T12:00:00Z"),
            closed_at(2, "2026-09-22T12:00:00Z"),
        ],
        ..tracking_issue(20, 15, &[])
    };
    let mut view = view(if open { vec![tracking.clone()] } else { vec![] }, vec![]);
    let snapshot = std::sync::Arc::make_mut(&mut view.snapshot);
    snapshot.closed = vec![
        closed_ticket(1, "2026-09-20T12:00:00Z"),
        closed_ticket(2, "2026-09-22T12:00:00Z"),
    ];
    snapshot.merged = vec![spec_change(15)];
    (view, tracking)
}

/// `docs/specs/repository.md`, Managed items: a tracking issue carries no
/// status label, so with none of its tickets open it is managed no more; but
/// it still heads its lane, unfinished while it is open
/// (`docs/specs/design_system.md`, The board, Finished lanes), so its State
/// is "Tracking" alone.
#[test]
fn an_open_tracking_issue_with_none_of_its_tickets_open_is_tracking_and_unfinished() {
    let (view, tracking) = tracking_finished_before_the_window(true);
    let read = AnyItem::HeldIssue(Box::new(tracking));

    let facts = facts_read(&view, &read);

    assert!(matches!(
        explain_fresh(&fresh_view(&view, &read, at(NOW)), &[], &read, at(NOW)),
        linewright::explain::Answer::LeftAlone(_)
    ));
    assert_eq!(facts.state, StateFact::Tracking { finished: None });
}

/// `docs/specs/design_system.md`, The item page, State: a tracking issue
/// says when it finished off the board too.
#[test]
fn a_closed_tracking_issue_whose_lane_left_the_board_is_tracking_and_when_its_last_ticket_closed() {
    let (view, tracking) = tracking_finished_before_the_window(false);

    let facts = facts_read(&view, &read_closed(tracking, "2026-09-22T12:00:00Z"));

    assert_eq!(
        facts.state,
        StateFact::Tracking {
            finished: Some(at("2026-09-22T12:00:00Z"))
        }
    );
}

#[test]
fn a_closed_tracking_issue_with_a_ticket_open_is_tracking() {
    let tracking = Issue {
        open: false,
        sub_issues: vec![
            SubIssue {
                state: SubIssueState::Closed {
                    at: Some(at("2026-09-20T12:00:00Z")),
                },
                ..sub_issue(1, false, "factory")
            },
            sub_issue(2, true, "factory"),
        ],
        ..tracking_issue(20, 15, &[])
    };

    let facts = facts_read(
        &view(vec![], vec![]),
        &read_closed(tracking, "2026-09-25T12:00:00Z"),
    );

    assert_eq!(facts.state, StateFact::Tracking { finished: None });
}

/// A snapshot written before sub-issues held when they closed knows of no
/// close time for them.
#[test]
fn a_closed_tracking_issue_with_a_ticket_closed_at_an_unknown_time_is_tracking() {
    let tracking = Issue {
        open: false,
        sub_issues: vec![
            SubIssue {
                state: SubIssueState::Closed {
                    at: Some(at("2026-09-20T12:00:00Z")),
                },
                ..sub_issue(1, false, "factory")
            },
            SubIssue {
                state: SubIssueState::Closed { at: None },
                ..sub_issue(2, false, "factory")
            },
        ],
        ..tracking_issue(20, 15, &[])
    };

    let facts = facts_read(
        &view(vec![], vec![]),
        &read_closed(tracking, "2026-09-25T12:00:00Z"),
    );

    assert_eq!(facts.state, StateFact::Tracking { finished: None });
}

/// `docs/specs/design_system.md`, The item page, Related items: a tracking
/// issue heading no lane has no lane head to be shown as, so it is shown by
/// its number and title, with no card status.
#[test]
fn a_tracking_issue_heading_no_lane_is_related_by_its_number_and_title_alone() {
    let mut view = view(vec![tracking_issue(20, 15, &[])], vec![]);
    std::sync::Arc::make_mut(&mut view.snapshot).merged = vec![spec_change(15)];
    let read = AnyItem::Merged {
        pull: Box::new(spec_change(15)),
        tracking_issues: vec![Number(20)],
    };

    let facts = facts_read(&view, &read);

    assert_eq!(
        facts.tracking_issue,
        Some(related(20, "Tracking 20", false, None))
    );
}

#[test]
fn a_closed_issue_linewright_has_not_worked_is_left_alone_as_closed() {
    let read = read_closed(ticket(7, "Old idea"), "2026-09-20T12:00:00Z");

    let facts = facts_read(&view(vec![], vec![]), &read);

    assert_eq!(
        facts.state,
        StateFact::LeftAlone(LeftAloneItem::Issue(LeftAloneIssue::Closed))
    );
    assert_eq!(facts.last_read, at(NOW));
}

#[test]
fn a_closed_issue_linewright_worked_is_in_done_with_the_line_it_had_on_the_board_after_it_left_it()
{
    let worked = labeled_by_linewright(ticket(7, "Old idea"));
    let done = Some(CardLine::Ended {
        ended: Ended::Closed(Some(IssueCloseReason::Completed)),
        at: at("2026-09-30T12:00:00Z"),
    });

    let off_the_board = facts_read(
        &view(vec![], vec![]),
        &read_closed(worked.clone(), "2026-09-30T12:00:00Z"),
    );
    let mut held = view(vec![], vec![]);
    std::sync::Arc::make_mut(&mut held.snapshot).closed =
        vec![closed_item(&worked, "2026-09-30T12:00:00Z")];
    let board_card = board(&held, &[], at(NOW))
        .card(Number(7))
        .map(|(column, card)| (column, card.line.clone()));
    let mut later = held.clone();
    later.policy.done_window = chrono::TimeDelta::hours(1);
    let left_the_board = facts_read(&later, &read_closed(worked, "2026-09-30T12:00:00Z"));

    assert_eq!(board_card, Some((Column::Done, done.clone())));
    assert_eq!(
        card_state(&facts_of(&held, 7)),
        (Column::Done, done.clone())
    );
    assert_eq!(card_state(&off_the_board), (Column::Done, done.clone()));
    assert!(board(&later, &[], at(NOW)).card(Number(7)).is_none());
    assert_eq!(card_state(&left_the_board), (Column::Done, done));
}

#[test]
fn an_item_left_out_of_the_view_lists_the_open_or_merged_pull_requests_closing_it() {
    let closing = |number, state| ClosingPull {
        number: Number(number),
        base_ref: "main".into(),
        author: Login::new("someone"),
        state,
        updated_at: at("2026-09-30T09:00:00Z"),
    };
    let left_out = Issue {
        labels: vec![],
        closed_by: vec![
            closing(30, PullState::Open { draft: false }),
            closing(
                31,
                PullState::Closed {
                    at: at("2026-09-30T10:00:00Z"),
                },
            ),
            closing(
                32,
                PullState::Merged {
                    at: at("2026-09-30T11:00:00Z"),
                },
            ),
        ],
        ..ticket(7, "Unlabeled")
    };

    let facts = facts_read(
        &view(vec![], vec![]),
        &AnyItem::LeftOutIssue(Box::new(left_out)),
    );

    let numbers: Vec<Number> = facts
        .pull_requests
        .iter()
        .map(|pull| match pull {
            Related::Card { number, .. } => *number,
            Related::LaneHead(heading) => panic!("a lane head: {heading:?}"),
        })
        .collect();
    assert_eq!(numbers, [Number(30), Number(32)]);
}

#[test]
fn what_linewright_waits_for_is_listed_only_while_it_waits_for_any() {
    let handed_back = labeled_by_linewright(Issue {
        labels: vec!["ready-for-human".into()],
        ..ticket(1, "Handed back")
    });
    let view = view(vec![handed_back, ticket(2, "Ready")], vec![]);

    assert_eq!(facts_of(&view, 1).waiting_for, [Wait::Maintainer]);
    assert_eq!(facts_of(&view, 2).waiting_for, []);
}

#[test]
fn a_closed_issue_lists_the_merged_pull_requests_closing_it() {
    let merged = ClosingPull {
        number: Number(30),
        base_ref: "main".into(),
        author: Login::new("factory"),
        state: PullState::Merged {
            at: at("2026-09-30T11:00:00Z"),
        },
        updated_at: at("2026-09-30T11:00:00Z"),
    };
    let closed = Issue {
        closed_by: vec![merged],
        ..ticket(7, "Done")
    };

    let facts = facts_read(
        &view(vec![], vec![]),
        &read_closed(closed, "2026-09-30T11:00:00Z"),
    );

    assert_eq!(
        facts.pull_requests,
        [Related::Card {
            number: Number(30),
            title: None,
            pull_request: true,
            status: None,
        }]
    );
}
