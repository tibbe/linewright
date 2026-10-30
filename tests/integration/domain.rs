use crate::common;

use std::collections::BTreeSet;

use chrono::TimeDelta;
use common::*;
use linewright::domain::*;

fn linewright() -> Login {
    Login::new(LINEWRIGHT)
}

/// A comment `by` wrote at 10:00 on 2026-10-01.
fn comment(by: &str) -> Comment {
    Comment {
        author: Login::new(by),
        body: "Which version?".into(),
        created_at: at("2026-10-01T10:00:00Z"),
    }
}

/// `label` applied `by` someone at 09:30 on 2026-10-01.
fn labeling(label: &str, by: &str) -> IssueEvent {
    IssueEvent::Labeled {
        label: label.into(),
        by: Some(Login::new(by)),
        at: at("2026-10-01T09:30:00Z"),
    }
}

/// Linewright assigned to an item by the maintainer at `when`, by hand.
fn assigning_linewright_by_hand(when: &str) -> Assignment {
    Assignment::Assigned {
        assignee: linewright(),
        actor: Login::new("maintainer"),
        at: at(when),
    }
}

/// Issue #1, which the maintainer opened, unlabeled.
fn persons_issue() -> Issue {
    Issue {
        labels: vec![],
        ..ticket(1, "Odd crash")
    }
}

#[test]
fn an_issue_is_worked_once_linewright_opened_it_recorded_work_on_it_tracks_the_tickets_it_cut_or_opened_a_pull_request_closing_it()
 {
    let cases = [
        (
            "opened by Linewright",
            Issue {
                author: linewright(),
                ..persons_issue()
            },
            vec![],
            true,
        ),
        ("left alone", persons_issue(), vec![], false),
        (
            "only commented on by Linewright",
            Issue {
                comments: vec![comment(LINEWRIGHT)],
                ..persons_issue()
            },
            vec![],
            true,
        ),
        (
            "commented on by a person",
            Issue {
                comments: vec![comment("alice")],
                ..persons_issue()
            },
            vec![],
            false,
        ),
        (
            "labeled and claimed by Linewright, both since removed by a person",
            Issue {
                events: vec![
                    labeling("ready-for-human", LINEWRIGHT),
                    assigning_linewright("2026-10-01T09:40:00Z"),
                    IssueEvent::Unlabeled {
                        label: "ready-for-human".into(),
                        at: at("2026-10-01T09:50:00Z"),
                    },
                    unassigning_linewright("2026-10-01T09:55:00Z"),
                ],
                ..persons_issue()
            },
            vec![],
            true,
        ),
        (
            "labeled by a person",
            Issue {
                labels: vec!["ready-for-agent".into()],
                events: vec![labeling("ready-for-agent", "maintainer")],
                ..persons_issue()
            },
            vec![],
            false,
        ),
        (
            "assigned to Linewright by hand",
            Issue {
                assignees: vec![linewright()],
                events: vec![IssueEvent::Assignment(assigning_linewright_by_hand(
                    "2026-10-01T09:40:00Z",
                ))],
                ..persons_issue()
            },
            vec![],
            false,
        ),
        (
            "closed by Linewright's pull request",
            persons_issue(),
            closed_by(persons_issue(), 20, LINEWRIGHT).closed_by,
            true,
        ),
        (
            "closed by a person's pull request",
            persons_issue(),
            closed_by(persons_issue(), 20, "alice").closed_by,
            false,
        ),
        (
            "designed, tracking a ticket Linewright cut from it",
            Issue {
                labels: vec![label::DESIGNED.into()],
                sub_issues: vec![sub_issue(2, true, LINEWRIGHT)],
                ..persons_issue()
            },
            vec![],
            true,
        ),
        (
            "designed, with a sub-issue a person opened",
            Issue {
                labels: vec![label::DESIGNED.into()],
                sub_issues: vec![sub_issue(2, true, "alice")],
                ..persons_issue()
            },
            vec![],
            false,
        ),
    ];

    for (case, issue, closing, worked) in cases {
        assert_eq!(issue.worked_by(&linewright(), &closing), worked, "{case}");
    }
}

#[test]
fn a_closed_issue_is_worked_as_an_open_one_is_and_a_closed_pull_request_held_is_worked() {
    let closed = |issue: Issue| closed_item(&issue, "2026-10-01T11:00:00Z");
    let cases = [
        (
            "an issue Linewright only commented on",
            closed(Issue {
                comments: vec![comment(LINEWRIGHT)],
                ..persons_issue()
            }),
            true,
        ),
        (
            "an issue only assigned to Linewright by hand",
            closed(Issue {
                assignees: vec![linewright()],
                events: vec![IssueEvent::Assignment(assigning_linewright_by_hand(
                    "2026-10-01T09:40:00Z",
                ))],
                ..persons_issue()
            }),
            false,
        ),
        (
            "an issue Linewright claimed",
            closed(Issue {
                events: vec![assigning_linewright("2026-10-01T09:40:00Z")],
                ..persons_issue()
            }),
            true,
        ),
        ("an issue left alone", closed(persons_issue()), false),
        (
            "a pull request closed unmerged",
            ClosedItem {
                number: Number(20),
                title: "Tidy up".into(),
                closed_at: at("2026-10-01T11:00:00Z"),
                updated_at: at("2026-10-01T11:00:00Z"),
                kind: ClosedKind::Pull {
                    closes: vec![],
                    author: Some(Login::new("alice")),
                },
            },
            true,
        ),
    ];

    for (case, item, worked) in cases {
        assert_eq!(item.worked_by(&linewright()), worked, "{case}");
    }
}

#[test]
fn a_pull_request_is_held_as_linewrights_when_it_opened_it_and_as_a_persons_while_open_and_assigned_to_or_worked_by_linewright()
 {
    let persons = |assignees: &[&str]| open_pull(60, "alice", &[], assignees);
    let closed = |pull: Pull| Pull {
        state: PullState::Closed {
            at: at("2026-10-01T11:00:00Z"),
        },
        ..pull
    };
    let cases = [
        (
            "Linewright's, open",
            open_pull(50, LINEWRIGHT, &[], &[]),
            Some(PullHeld::Linewrights),
            true,
        ),
        (
            "Linewright's, closed unmerged",
            closed(open_pull(50, LINEWRIGHT, &[1], &[])),
            Some(PullHeld::Linewrights),
            true,
        ),
        (
            "a person's Linewright is assigned to",
            Pull {
                assignments: vec![assigning_linewright_by_hand("2026-10-01T09:40:00Z")],
                ..persons(&[LINEWRIGHT])
            },
            Some(PullHeld::Peoples),
            false,
        ),
        (
            "a person's Linewright was assigned to, then unassigned from",
            Pull {
                assignments: vec![
                    assigning_linewright_by_hand("2026-10-01T09:40:00Z"),
                    Assignment::Unassigned {
                        assignee: linewright(),
                        actor: Login::new("maintainer"),
                        at: at("2026-10-01T09:50:00Z"),
                    },
                ],
                ..persons(&[])
            },
            None,
            false,
        ),
        (
            "a person's Linewright commented on",
            Pull {
                comments: vec![comment(LINEWRIGHT)],
                ..persons(&[])
            },
            Some(PullHeld::Peoples),
            true,
        ),
        (
            "a person's Linewright labeled",
            Pull {
                labelings: vec![Labeling {
                    label: label::STALLED.into(),
                    by: Some(linewright()),
                    at: at("2026-10-01T09:30:00Z"),
                }],
                ..persons(&[])
            },
            Some(PullHeld::Peoples),
            true,
        ),
        (
            "a person's only a person commented on",
            Pull {
                comments: vec![comment("maintainer")],
                ..persons(&[])
            },
            None,
            false,
        ),
        (
            "a person's Linewright worked, closed unmerged",
            closed(Pull {
                comments: vec![comment(LINEWRIGHT)],
                ..persons(&[])
            }),
            None,
            true,
        ),
    ];

    for (case, pull, held, worked) in cases {
        assert_eq!(pull.held(&linewright()), held, "{case}");
        assert_eq!(pull.worked_by(&linewright()), worked, "{case}");
    }
}

/// Windows at noon on 2026-10-01, the Done window opened at noon on
/// 2026-09-24.
fn windows() -> Windows {
    Windows::at(
        at("2026-10-01T12:00:00Z"),
        TimeDelta::days(7),
        TimeDelta::days(7),
    )
}

const IN_DONE_WINDOW: &str = "2026-09-30T12:00:00Z";
const BEFORE_DONE_WINDOW: &str = "2026-09-20T12:00:00Z";

/// Issue #`number`, which Linewright opened to track the tickets cut from
/// spec change #70, with `tickets`, each open or not.
fn tracking(number: u64, tickets: &[(u64, bool)]) -> Issue {
    Issue {
        author: linewright(),
        labels: vec![],
        filed: Some(Filed::Tracking {
            spec_change: Number(70),
        }),
        sub_issues: tickets
            .iter()
            .map(|&(n, open)| sub_issue(n, open, LINEWRIGHT))
            .collect(),
        ..ticket(number, &format!("Tracking {number}"))
    }
}

/// Issue #`number`, which Linewright opened, closed at `closed_at`.
fn closed_issue(number: u64, closed_at: &str) -> ClosedItem {
    let issue = Issue {
        author: linewright(),
        open: false,
        ..ticket(number, &format!("Ticket {number}"))
    };
    closed_item(&issue, closed_at)
}

/// Linewright's pull request #`number`, closing `closes`, closed unmerged
/// at `closed_at`.
fn closed_pull(number: u64, closes: &[u64], closed_at: &str) -> ClosedItem {
    ClosedItem {
        number: Number(number),
        title: format!("Pull {number}"),
        closed_at: at(closed_at),
        updated_at: at(closed_at),
        kind: ClosedKind::Pull {
            closes: closes.iter().copied().map(Number).collect(),
            author: Some(linewright()),
        },
    }
}

/// Issue #4, which a person opened and Linewright designed and cut into
/// ticket #5, closed, beside #6, a closed sub-issue a person opened.
fn cut() -> Issue {
    Issue {
        labels: vec![label::DESIGNED.into()],
        sub_issues: vec![
            sub_issue(5, false, LINEWRIGHT),
            sub_issue(6, false, "alice"),
        ],
        ..ticket(4, "Big thing")
    }
}

/// Tracking issue #8, closed within the Done window, with its ticket #9
/// closed.
fn closed_tracking() -> ClosedItem {
    let issue = Issue {
        open: false,
        ..tracking(8, &[(9, false)])
    };
    closed_item(&issue, IN_DONE_WINDOW)
}

fn snapshot(issues: Vec<Issue>, closed: Vec<ClosedItem>) -> Snapshot {
    Snapshot {
        default_branch: "main".into(),
        issues,
        closed,
        ..Snapshot::default()
    }
}

#[test]
fn a_snapshot_keeps_an_item_closed_within_the_done_window_or_a_ticket_of_a_lane_it_holds_and_linewrights_pull_requests_closing_one()
 {
    let snapshot = snapshot(
        vec![tracking(1, &[(2, false), (3, true)]), cut()],
        vec![closed_tracking()],
    );
    let cases = [
        (
            "an issue closed within the Done window",
            closed_issue(30, IN_DONE_WINDOW),
            true,
        ),
        (
            "an issue closed before it",
            closed_issue(31, BEFORE_DONE_WINDOW),
            false,
        ),
        (
            "a ticket of an open tracking issue, closed before it",
            closed_issue(2, BEFORE_DONE_WINDOW),
            true,
        ),
        (
            "a ticket Linewright cut from an open issue, closed before it",
            closed_issue(5, BEFORE_DONE_WINDOW),
            true,
        ),
        (
            "a sub-issue a person opened under a cut issue, closed before it",
            closed_issue(6, BEFORE_DONE_WINDOW),
            false,
        ),
        (
            "a ticket of a closed tracking issue it holds, closed before it",
            closed_issue(9, BEFORE_DONE_WINDOW),
            true,
        ),
        (
            "Linewright's pull request closing a ticket, closed before it",
            closed_pull(40, &[2], BEFORE_DONE_WINDOW),
            true,
        ),
        (
            "Linewright's pull request closing no ticket, closed before it",
            closed_pull(41, &[31], BEFORE_DONE_WINDOW),
            false,
        ),
        (
            "Linewright's pull request closing no ticket, closed within it",
            closed_pull(42, &[31], IN_DONE_WINDOW),
            true,
        ),
    ];

    for (case, item, kept) in cases {
        assert_eq!(
            snapshot.keeps_closed(&item, windows(), &linewright()),
            kept,
            "{case}"
        );
    }
}

#[test]
fn the_closed_tickets_a_snapshot_lacks_are_those_of_the_lanes_it_holds_open_or_closed() {
    let snapshot = snapshot(
        vec![tracking(1, &[(2, false), (3, true), (7, false)]), cut()],
        vec![closed_issue(7, BEFORE_DONE_WINDOW), closed_tracking()],
    );

    assert_eq!(
        snapshot.unheld_closed_tickets(&linewright()),
        BTreeSet::from([Number(2), Number(5), Number(9)])
    );
}

#[test]
fn a_snapshot_drops_the_closed_items_it_would_not_keep_with_the_tickets_of_a_tracking_issue_dropped()
 {
    let old_tracking = Issue {
        open: false,
        ..tracking(10, &[(11, false)])
    };
    let mut snapshot = snapshot(
        vec![tracking(1, &[(2, false)])],
        vec![
            closed_issue(2, BEFORE_DONE_WINDOW),
            closed_item(&old_tracking, BEFORE_DONE_WINDOW),
            closed_issue(11, BEFORE_DONE_WINDOW),
            closed_pull(12, &[11], BEFORE_DONE_WINDOW),
            closed_issue(20, IN_DONE_WINDOW),
            closed_issue(21, BEFORE_DONE_WINDOW),
        ],
    );

    snapshot.drop_unkept_closed(windows(), &linewright());

    let held: Vec<Number> = snapshot.closed.iter().map(|item| item.number).collect();
    assert_eq!(held, [Number(2), Number(20)]);
}

/// Pull request #`number`, `author`'s, `state`, closing an issue.
fn closing(number: u64, author: &str, state: PullState) -> ClosingPull {
    ClosingPull {
        number: Number(number),
        author: Login::new(author),
        base_ref: "main".into(),
        state,
        updated_at: at(BEFORE_DONE_WINDOW),
    }
}

#[test]
fn the_closing_pull_requests_a_snapshot_lacks_are_linewrights_closed_unmerged_closing_the_tickets_it_holds()
 {
    let closed = PullState::Closed {
        at: at(BEFORE_DONE_WINDOW),
    };
    let held_ticket = Issue {
        closed_by: vec![
            closing(40, LINEWRIGHT, closed),
            closing(41, "alice", closed),
            closing(
                42,
                LINEWRIGHT,
                PullState::Merged {
                    at: at(BEFORE_DONE_WINDOW),
                },
            ),
            closing(43, LINEWRIGHT, PullState::Open { draft: false }),
            closing(44, LINEWRIGHT, closed),
        ],
        author: linewright(),
        open: false,
        ..ticket(2, "Ticket 2")
    };
    let not_a_ticket = Issue {
        closed_by: vec![closing(45, LINEWRIGHT, closed)],
        author: linewright(),
        open: false,
        ..ticket(31, "Ticket 31")
    };
    let snapshot = snapshot(
        vec![tracking(1, &[(2, false)])],
        vec![
            closed_item(&held_ticket, BEFORE_DONE_WINDOW),
            closed_item(&not_a_ticket, IN_DONE_WINDOW),
            closed_pull(44, &[2], BEFORE_DONE_WINDOW),
        ],
    );

    assert_eq!(
        snapshot.unheld_closing_pulls(&linewright()),
        BTreeSet::from([Number(40)])
    );
}
