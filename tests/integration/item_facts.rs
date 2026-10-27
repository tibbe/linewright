//! The item page's facts (`docs/specs/design_system.md`, The item page),
//! through the functional core that finds them.

use crate::common;

use common::{NOW, at, sub_issue, ticket, view};
use linewright::board::{CardLine, CardStatus, Column, Stop, board};
use linewright::domain::*;
use linewright::explain::{LeftAloneIssue, LeftAloneItem, explain, explain_fresh, fresh_view};
use linewright::item_facts::{ItemFacts, Related, StateFact, item_facts};
use linewright::reconcile::Wait;
use linewright::status::View;
use linewright::store::AnyItem;

/// #`number`'s facts, as `serve` finds them for an item its view manages.
fn facts_of(view: &View, number: u64) -> ItemFacts {
    let answer = explain(view, &[], Number(number), at(NOW)).expect("a managed item");
    let answer = linewright::explain::Answer::Managed(answer);
    item_facts(&answer, view, &board(view, &[], at(NOW)))
}

/// The facts of `read`, as `serve` finds them for an item it reads from
/// GitHub.
fn facts_read(view: &View, read: &AnyItem) -> ItemFacts {
    let fresh = fresh_view(view, read, at(NOW));
    let answer = explain_fresh(&fresh, &[], read, at(NOW));
    item_facts(&answer, &fresh, &board(&fresh, &[], at(NOW)))
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
    Related {
        number: Number(number),
        title: Some(title.into()),
        pull_request,
        status,
    }
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
        facts.tracking_issue,
        Some(related(20, "Tracking 20", false, None))
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
            filed: None,
            sub_issues: vec![],
            labels: vec!["ready-for-agent".into(), label::DESIGNED.into()],
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

#[test]
fn a_tracking_issue_has_its_lanes_line_and_status_and_its_tickets_closed_of_total() {
    let facts = facts_of(&tracking_five(), 20);

    let StateFact::Lane(lane) = &facts.state else {
        panic!("no lane: {:?}", facts.state);
    };
    let (card, status) = lane.most_pressing().expect("a card status");
    assert_eq!(card.number, Number(4));
    assert_eq!(
        card.line,
        Some(CardLine::Review {
            pull_request: Number(10),
            author: None,
            stops: Some(Stop::ChecksFailed),
        })
    );
    assert_eq!(status, CardStatus::Failed);
    assert_eq!(lane.finished(), None);
    let tickets = facts.tickets.expect("tickets");
    assert_eq!(tickets.closed, 3);
    assert_eq!(
        tickets.tickets,
        [
            related(1, "Ticket 1", false, None),
            related(2, "Ticket 2", false, None),
            Related {
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
        facts.tracking_issue,
        Some(related(20, "Tracking 20", false, Some(CardStatus::Failed)))
    );
    assert!(facts.pull_request);
}

#[test]
fn a_closed_issue_out_of_the_window_is_left_alone_as_closed() {
    let closed = Issue {
        open: false,
        ..ticket(7, "Old idea")
    };

    let facts = facts_read(&view(vec![], vec![]), &AnyItem::HeldIssue(Box::new(closed)));

    assert_eq!(
        facts.state,
        StateFact::LeftAlone(LeftAloneItem::Issue(LeftAloneIssue::Closed))
    );
    assert_eq!(facts.last_read, at(NOW));
}

#[test]
fn an_item_left_out_of_the_view_lists_the_open_or_merged_pull_requests_closing_it() {
    let closing = |number, state| ClosingPull {
        number: Number(number),
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

    let numbers: Vec<Number> = facts.pull_requests.iter().map(|pull| pull.number).collect();
    assert_eq!(numbers, [Number(30), Number(32)]);
}

#[test]
fn what_linewright_waits_for_is_listed_only_while_it_waits_for_any() {
    let handed_back = Issue {
        labels: vec!["ready-for-human".into()],
        ..ticket(1, "Handed back")
    };
    let view = view(vec![handed_back, ticket(2, "Ready")], vec![]);

    assert_eq!(facts_of(&view, 1).waiting_for, [Wait::Maintainer]);
    assert_eq!(facts_of(&view, 2).waiting_for, []);
}
