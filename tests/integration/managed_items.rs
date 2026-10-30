use crate::common;

use common::*;
use linewright::board::{Column, Placement, board, placement};
use linewright::domain::*;
use linewright::explain::{explain_fresh, fresh_view};
use linewright::item_facts::{StateFact, item_facts};
use linewright::managed::{Held, classify, held};
use linewright::store::AnyItem;

/// Every item of a snapshot varied over the managed-items rule
/// (`docs/specs/repository.md`, Managed items), as a store reads each.
fn varied_items() -> Vec<AnyItem> {
    let labeled = |number, label: &str| {
        labeled_by_linewright(Issue {
            labels: vec![label.into()],
            ..ticket(number, &format!("Ticket {number}"))
        })
    };
    let taken_back = |number| Issue {
        labels: vec![],
        ..labeled(number, "ready-for-agent")
    };
    let tracking = |number, tickets: &[(u64, bool)]| Issue {
        author: Login::new(LINEWRIGHT),
        labels: vec![],
        filed: Some(Filed::Tracking {
            spec_change: Number(number + 70),
        }),
        sub_issues: tickets
            .iter()
            .map(|&(n, open)| sub_issue(n, open, LINEWRIGHT))
            .collect(),
        ..ticket(number, &format!("Tracking {number}"))
    };
    let cut = |number, tracked_by: u64| Issue {
        author: Login::new(LINEWRIGHT),
        parent: Some(Parent {
            number: Number(tracked_by),
            spec_change: Some(Number(tracked_by + 70)),
        }),
        ..ticket(number, &format!("Ticket {number}"))
    };
    let closing = |issue: Issue, pull: &Pull| Issue {
        closed_by: vec![ClosingPull::from(pull)],
        ..issue
    };
    let linewrights_for_1 = open_pull(50, LINEWRIGHT, &[1], &[LINEWRIGHT]);
    let peoples_for_6 = open_pull(61, "alice", &[6], &[LINEWRIGHT]);
    let commented = MergedPull {
        events: vec![IssueEvent::Commented {
            by: Login::new(LINEWRIGHT),
            at: at("2026-10-01T09:30:00Z"),
        }],
        ..merged_pull(73, "alice", &[])
    };
    let mut peoples_closed = open_pull(62, "alice", &[], &[]);
    peoples_closed.state = PullState::Closed {
        at: at("2026-10-01T10:00:00Z"),
    };
    peoples_closed.labelings = vec![Labeling {
        label: "linewright:stalled".into(),
        by: Some(Login::new(LINEWRIGHT)),
        at: at("2026-10-01T09:30:00Z"),
    }];
    vec![
        AnyItem::HeldIssue(Box::new(closing(ticket(1, "Ready"), &linewrights_for_1))),
        AnyItem::HeldIssue(Box::new(labeled(2, "needs-triage"))),
        AnyItem::LeftOutIssue(Box::new(Issue {
            labels: vec!["ready-for-human".into()],
            ..ticket(3, "A person's")
        })),
        AnyItem::HeldIssue(Box::new(taken_back(4))),
        AnyItem::HeldIssue(Box::new(taken_back(5))),
        AnyItem::HeldIssue(Box::new(closing(taken_back(6), &peoples_for_6))),
        AnyItem::HeldIssue(Box::new(labeled(7, "needs-info"))),
        AnyItem::HeldIssue(Box::new(tracking(8, &[(9, true)]))),
        AnyItem::HeldIssue(Box::new(cut(9, 8))),
        AnyItem::HeldIssue(Box::new(tracking(10, &[(11, false)]))),
        read_closed(labeled(11, "ready-for-agent"), "2026-09-30T12:00:00Z"),
        read_closed(labeled(20, "ready-for-agent"), "2026-09-30T12:00:00Z"),
        read_closed(ticket(21, "Never worked"), "2026-09-30T12:00:00Z"),
        AnyItem::Pull(Box::new(linewrights_for_1)),
        AnyItem::Pull(Box::new(open_pull(51, LINEWRIGHT, &[], &[]))),
        AnyItem::Pull(Box::new(open_pull(60, "alice", &[], &[LINEWRIGHT]))),
        AnyItem::Pull(Box::new(peoples_for_6)),
        AnyItem::Pull(Box::new(peoples_closed)),
        AnyItem::Pull(Box::new(open_pull(63, "alice", &[], &[]))),
        AnyItem::Merged {
            pull: Box::new(merged_pull(70, LINEWRIGHT, &[99])),
            tracking_issues: vec![],
        },
        AnyItem::Merged {
            pull: Box::new(merged_pull(71, "alice", &[])),
            tracking_issues: vec![],
        },
        AnyItem::Merged {
            pull: Box::new(MergedPull {
                files: vec!["docs/specs/invoices.md".into()],
                assignees: vec![Login::new(LINEWRIGHT)],
                ..merged_pull(72, "alice", &[])
            }),
            tracking_issues: vec![],
        },
        AnyItem::Merged {
            pull: Box::new(commented),
            tracking_issues: vec![],
        },
    ]
}

/// Where the board shows an item (`docs/specs/design_system.md`, The board).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shown {
    /// A card of its own in the column.
    Card(Column),
    /// On the card of the issue it closes, its own card in the column.
    OnCardOf(u64, Column),
    /// At the head of its lane.
    HeadsLane,
    /// Left alone, but at the head of its lane, unfinished until it closes,
    /// and "Tracking" on its page: an open tracking issue with no open
    /// ticket.
    HeadsUnfinishedLane,
    LeftAlone,
}

#[test]
fn an_item_is_managed_exactly_when_the_board_gives_it_a_column_or_a_lane_and_its_page_agrees() {
    let now = at("2026-10-01T12:00:30Z");
    let items = varied_items();
    let mut view = items.iter().fold(view(vec![], vec![]), |view, read| {
        fresh_view(&view, read, now)
    });
    view.in_flight = vec![in_flight("5/implement-1", "2026-10-01T11:50:00Z")];
    // A snapshot holds a person's pull request Linewright worked, closed
    // unmerged, among its closed items.
    std::sync::Arc::make_mut(&mut view.snapshot)
        .closed
        .push(ClosedItem {
            number: Number(62),
            title: "Pull 62".into(),
            closed_at: at("2026-10-01T10:00:00Z"),
            updated_at: at("2026-10-01T10:00:00Z"),
            kind: ClosedKind::Pull {
                closes: vec![],
                author: Some(Login::new("alice")),
            },
        });
    let board = board(&view, &[], now);
    let expected = [
        (1, Shown::Card(Column::InReview)),
        (2, Shown::Card(Column::Triage)),
        (3, Shown::LeftAlone),
        (4, Shown::LeftAlone),
        (5, Shown::Card(Column::Working)),
        (6, Shown::Card(Column::InReview)),
        (7, Shown::Card(Column::NeedsAPerson)),
        (8, Shown::HeadsLane),
        (9, Shown::Card(Column::Ready)),
        (10, Shown::HeadsUnfinishedLane),
        (11, Shown::Card(Column::Done)),
        (20, Shown::Card(Column::Done)),
        (21, Shown::LeftAlone),
        (50, Shown::OnCardOf(1, Column::InReview)),
        (51, Shown::Card(Column::InReview)),
        (60, Shown::Card(Column::InReview)),
        (61, Shown::OnCardOf(6, Column::InReview)),
        (62, Shown::Card(Column::Done)),
        (63, Shown::LeftAlone),
        (70, Shown::Card(Column::Done)),
        (71, Shown::LeftAlone),
        (72, Shown::Card(Column::Ready)),
        (73, Shown::Card(Column::Done)),
    ];
    assert_eq!(
        items.iter().map(|read| read.number().0).collect::<Vec<_>>(),
        expected.map(|(number, _)| number)
    );

    for (read, (number, shown)) in items.iter().zip(expected) {
        let number = Number(number);
        // The view does not hold an item it leaves out, such as a person's
        // pull request Linewright has not worked.
        let closed;
        let held = match held(&view.snapshot, number) {
            Some(held) => held,
            None => match read {
                AnyItem::HeldIssue(issue) | AnyItem::LeftOutIssue(issue) => Held::Issue(issue),
                AnyItem::ClosedIssue { issue, closing } => {
                    closed = ClosedItem::of_issue(issue, *closing);
                    Held::Closed(&closed)
                }
                AnyItem::Pull(pull) => Held::Pull(pull),
                AnyItem::Merged { pull, .. } => Held::Merged(pull),
            },
        };
        let managed = classify(&view, held, now);
        let on_the_board = match shown {
            Shown::Card(column) => board.card(number).map(|(on, _)| on) == Some(column),
            Shown::OnCardOf(issue, _) => {
                board.card(number).is_none() && board.card(Number(issue)).is_some()
            }
            Shown::HeadsLane => board.lane_headed_by(number).is_some(),
            Shown::HeadsUnfinishedLane => board
                .lane_headed_by(number)
                .is_some_and(|lane| lane.finished().is_none()),
            Shown::LeftAlone => {
                board.card(number).is_none() && board.lane_headed_by(number).is_none()
            }
        };
        let in_flight = view
            .in_flight
            .iter()
            .any(|run| run.id.of.item() == Some(number));
        let placed = managed
            .as_ref()
            .ok()
            .map(|managed| placement(&managed.state, in_flight));
        let answer = explain_fresh(&view, &[], read, now);
        let page = item_facts(&answer, &view, &board, &[], now).state;

        assert!(on_the_board, "#{number}: {shown:?} on {board:?}");
        assert_eq!(
            managed.is_ok(),
            !matches!(shown, Shown::LeftAlone | Shown::HeadsUnfinishedLane),
            "#{number}: {managed:?}"
        );
        match shown {
            Shown::Card(column) | Shown::OnCardOf(_, column) => {
                assert_eq!(placed, Some(Placement::Column(column)), "#{number}");
                assert!(
                    matches!(page, StateFact::Card { column: on, .. } if on == column),
                    "#{number}: {page:?}"
                );
            }
            Shown::HeadsLane => {
                assert_eq!(placed, Some(Placement::HeadsLane), "#{number}");
                assert!(
                    matches!(page, StateFact::Tracking { .. }),
                    "#{number}: {page:?}"
                );
            }
            Shown::HeadsUnfinishedLane => {
                assert_eq!(page, StateFact::Tracking { finished: None }, "#{number}");
            }
            Shown::LeftAlone => {
                assert!(
                    matches!(page, StateFact::LeftAlone(_)),
                    "#{number}: {page:?}"
                );
            }
        }
    }
}
