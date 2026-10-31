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

/// Tickets #1, #2 and #3: #1 closed by alice's open pull request #30, which
/// Linewright is assigned to, #2 by Linewright's #20; and #41, a person's
/// pull request merged within the windows.
fn refreshed_pulls() -> Snapshot {
    Snapshot {
        issues: vec![
            closed_by(ticket(1, "One"), 30, "alice"),
            closed_by(ticket(2, "Two"), 20, LINEWRIGHT),
            ticket(3, "Three"),
        ],
        pulls: vec![open_pull(20, LINEWRIGHT, &[2], &[LINEWRIGHT])],
        peoples_pulls: vec![open_pull(30, "alice", &[1], &[LINEWRIGHT])],
        merged: vec![merged_pull(41, "alice", &[])],
        ..snapshot(vec![], vec![])
    }
}

fn peoples(closes: &[u64], pull: Option<Pull>) -> PullHeldRead {
    PullHeldRead::Peoples(PeoplesPullRead {
        closes: closes.iter().copied().map(Number).collect(),
        pull,
    })
}

#[test]
fn a_pull_request_read_is_held_as_read_and_frees_the_issues_a_persons_closes_or_closed_while_open()
{
    let renamed = Pull {
        title: "Do two, renamed".into(),
        ..open_pull(20, LINEWRIGHT, &[2], &[LINEWRIGHT])
    };
    let assigned = open_pull(32, "alice", &[3], &[LINEWRIGHT]);
    let cases = [
        (
            "a person's pull request read closed",
            30,
            peoples(&[], None),
            None,
            vec![1],
            Snapshot {
                peoples_pulls: vec![],
                ..refreshed_pulls()
            },
        ),
        (
            "a person's pull request opened since, left out",
            31,
            peoples(&[3], None),
            None,
            vec![3],
            refreshed_pulls(),
        ),
        (
            "a person's pull request opened since, assigned to Linewright",
            32,
            peoples(&[3], Some(assigned.clone())),
            None,
            vec![3],
            Snapshot {
                peoples_pulls: vec![open_pull(30, "alice", &[1], &[LINEWRIGHT]), assigned],
                ..refreshed_pulls()
            },
        ),
        (
            "Linewright's pull request read",
            20,
            PullHeldRead::Linewrights(renamed.clone()),
            None,
            vec![],
            Snapshot {
                pulls: vec![renamed],
                ..refreshed_pulls()
            },
        ),
        (
            "Linewright's pull request read merged",
            20,
            peoples(&[], None),
            Some(merged_pull(20, LINEWRIGHT, &[2])),
            vec![2],
            Snapshot {
                pulls: vec![],
                merged: vec![
                    merged_pull(20, LINEWRIGHT, &[2]),
                    merged_pull(41, "alice", &[]),
                ],
                ..refreshed_pulls()
            },
        ),
        (
            "a merged pull request read out of the windows",
            41,
            peoples(&[], None),
            None,
            vec![],
            Snapshot {
                merged: vec![],
                ..refreshed_pulls()
            },
        ),
        (
            "a pull request the store lacks",
            99,
            peoples(&[], None),
            None,
            vec![],
            refreshed_pulls(),
        ),
    ];

    for (case, number, held, merged, to_read, after) in cases {
        let mut snapshot = refreshed_pulls();

        let read = snapshot.took_pull(Number(number), PullRead { held, merged });

        let to_read: Vec<Number> = to_read.into_iter().map(Number).collect();
        assert_eq!(read, to_read, "{case}");
        assert_eq!(snapshot, after, "{case}");
    }
}

/// Tracking issue #1, with its ticket #6 `state`.
fn tracking_six(state: SubIssueState) -> Issue {
    Issue {
        sub_issues: vec![SubIssue {
            number: Number(6),
            author: linewright(),
            state,
        }],
        ..tracking(1, &[])
    }
}

/// Ticket #5, which #6 blocks, seen as `open`.
fn blocked_five(open: bool) -> Issue {
    Issue {
        blocked_by: vec![Linked {
            number: Number(6),
            open,
        }],
        ..ticket(5, "Five")
    }
}

/// Linewright's pull request #21, closing #6, closed unmerged.
fn ended_21() -> Pull {
    Pull {
        state: PullState::Closed {
            at: at(IN_DONE_WINDOW),
        },
        ..open_pull(21, LINEWRIGHT, &[6], &[])
    }
}

/// #7, closed within the Done window.
fn closed_seven() -> ClosedItem {
    closed_item(&ticket(7, "Seven"), IN_DONE_WINDOW)
}

/// Ticket #6 of tracking issue #1, blocking #5 and closed by Linewright's
/// open pull request #20 and its #21, closed unmerged; and #7, closed
/// within the Done window.
fn refreshed_issues() -> Snapshot {
    Snapshot {
        issues: vec![
            tracking_six(SubIssueState::Open),
            blocked_five(true),
            ticket(6, "Six"),
        ],
        pulls: vec![open_pull(20, LINEWRIGHT, &[6], &[]), ended_21()],
        closed: vec![closed_seven()],
        ..snapshot(vec![], vec![])
    }
}

#[test]
fn an_issue_read_is_held_as_read_and_one_that_left_has_linewrights_ended_pull_requests_closing_it_read_again()
 {
    let relabeled = Issue {
        labels: vec!["ready-for-human".into()],
        ..ticket(6, "Six")
    };
    let renamed = Pull {
        title: "Do six, renamed".into(),
        ..open_pull(20, LINEWRIGHT, &[6], &[])
    };
    let closed_six = |closed_at| {
        closed_item(
            &Issue {
                open: false,
                ..ticket(6, "Six")
            },
            closed_at,
        )
    };
    let closed_five = |closed_at| {
        closed_item(
            &Issue {
                open: false,
                ..blocked_five(true)
            },
            closed_at,
        )
    };
    let both_pulls = || vec![open_pull(20, LINEWRIGHT, &[6], &[]), ended_21()];
    let issue_taken = |pulls_read: &[u64], pulls_to_read: &[u64]| IssueTaken {
        pulls_read: pulls_read.iter().copied().map(Number).collect(),
        pulls_to_read: pulls_to_read.iter().copied().map(Number).collect(),
    };
    let holding = |issues, pulls, closed| Snapshot {
        issues,
        pulls,
        closed,
        ..snapshot(vec![], vec![])
    };
    let cases = [
        (
            "held, relabeled, with Linewright's pull request closing it",
            6,
            IssueRead::Held {
                issue: Box::new(relabeled.clone()),
                pulls: vec![renamed.clone()],
            },
            issue_taken(&[20], &[]),
            holding(
                vec![
                    tracking_six(SubIssueState::Open),
                    blocked_five(true),
                    relabeled,
                ],
                vec![renamed, ended_21()],
                vec![closed_seven()],
            ),
        ),
        (
            "held as it was",
            6,
            IssueRead::Held {
                issue: Box::new(ticket(6, "Six")),
                pulls: vec![open_pull(20, LINEWRIGHT, &[6], &[])],
            },
            issue_taken(&[20], &[]),
            refreshed_issues(),
        ),
        (
            "held closed, read open again",
            7,
            IssueRead::Held {
                issue: Box::new(ticket(7, "Seven")),
                pulls: vec![],
            },
            issue_taken(&[], &[]),
            holding(
                vec![
                    tracking_six(SubIssueState::Open),
                    blocked_five(true),
                    ticket(6, "Six"),
                    ticket(7, "Seven"),
                ],
                both_pulls(),
                vec![],
            ),
        ),
        (
            "left out, open",
            6,
            IssueRead::LeftOut { open: true },
            issue_taken(&[], &[21]),
            holding(
                vec![tracking_six(SubIssueState::Open), blocked_five(true)],
                both_pulls(),
                vec![closed_seven()],
            ),
        ),
        (
            "closed before the Done window, a ticket of a lane held",
            6,
            IssueRead::Closed(Box::new(closed_six(BEFORE_DONE_WINDOW))),
            issue_taken(&[], &[21]),
            holding(
                vec![
                    tracking_six(SubIssueState::Closed {
                        at: Some(at(BEFORE_DONE_WINDOW)),
                    }),
                    blocked_five(false),
                ],
                both_pulls(),
                vec![closed_six(BEFORE_DONE_WINDOW), closed_seven()],
            ),
        ),
        (
            "closed within the Done window",
            5,
            IssueRead::Closed(Box::new(closed_five(IN_DONE_WINDOW))),
            issue_taken(&[], &[]),
            holding(
                vec![tracking_six(SubIssueState::Open), ticket(6, "Six")],
                both_pulls(),
                vec![closed_five(IN_DONE_WINDOW), closed_seven()],
            ),
        ),
        (
            "closed before the Done window, of no lane",
            5,
            IssueRead::Closed(Box::new(closed_five(BEFORE_DONE_WINDOW))),
            issue_taken(&[], &[]),
            holding(
                vec![tracking_six(SubIssueState::Open), ticket(6, "Six")],
                both_pulls(),
                vec![closed_seven()],
            ),
        ),
        (
            "gone",
            6,
            IssueRead::Gone,
            issue_taken(&[], &[21]),
            holding(
                vec![tracking(1, &[]), ticket(5, "Five")],
                both_pulls(),
                vec![closed_seven()],
            ),
        ),
        (
            "an issue the store lacks",
            99,
            IssueRead::Gone,
            issue_taken(&[], &[]),
            refreshed_issues(),
        ),
    ];

    for (case, number, read, taken, after) in cases {
        let mut snapshot = refreshed_issues();

        let took = snapshot.took_issue(Number(number), read, windows(), &linewright());

        assert_eq!(took, taken, "{case}");
        assert_eq!(snapshot, after, "{case}");
    }
}

#[test]
fn linewrights_ended_pull_request_read_again_is_held_as_read_or_dropped() {
    let closed = |title: &str| Pull {
        title: title.into(),
        state: PullState::Closed {
            at: at(IN_DONE_WINDOW),
        },
        ..open_pull(21, LINEWRIGHT, &[6], &[])
    };
    let held = || Snapshot {
        pulls: vec![closed("Pull 21")],
        ..snapshot(vec![], vec![])
    };
    let cases = [
        (
            "read",
            Some(closed("Renamed")),
            Snapshot {
                pulls: vec![closed("Renamed")],
                ..held()
            },
        ),
        (
            "no longer held",
            None,
            Snapshot {
                pulls: vec![],
                ..held()
            },
        ),
    ];

    for (case, read, after) in cases {
        let mut snapshot = held();

        snapshot.took_ended_pull(Number(21), read);

        assert_eq!(snapshot, after, "{case}");
    }
}

#[test]
fn a_pull_request_read_frees_the_other_issues_it_closes_or_closed_while_open() {
    let snapshot = Snapshot {
        issues: vec![
            closed_by(ticket(1, "One"), 20, LINEWRIGHT),
            ticket(2, "Two"),
            ticket(3, "Three"),
            closed_by(ticket(4, "Four"), 22, LINEWRIGHT),
        ],
        pulls: vec![
            // Edited not to close #1, and to close #2 besides #3.
            open_pull(20, LINEWRIGHT, &[2, 3], &[]),
            open_pull(22, LINEWRIGHT, &[4], &[]),
        ],
        ..snapshot(vec![], vec![])
    };
    let cases = [
        (
            "a pull request closing others",
            vec![Item::Pull(Number(20))],
            vec![
                Item::Issue(Number(1)),
                Item::Issue(Number(2)),
                Item::Issue(Number(3)),
            ],
        ),
        (
            "a pull request closing an issue read with it",
            vec![Item::Pull(Number(20)), Item::Issue(Number(2))],
            vec![Item::Issue(Number(1)), Item::Issue(Number(3))],
        ),
        ("an issue", vec![Item::Issue(Number(4))], vec![]),
        (
            "a pull request it lacks",
            vec![Item::Pull(Number(99))],
            vec![],
        ),
    ];

    for (case, read, others) in cases {
        let read = BTreeSet::from_iter(read);

        assert_eq!(
            snapshot.closed_by_pulls_read(&read),
            BTreeSet::from_iter(others),
            "{case}"
        );
    }
}

#[test]
fn only_a_name_linewright_gives_a_tickets_branch_names_one() {
    let issue_of = |name: &str| TicketBranchName::parse(name).map(|branch| branch.issue());

    assert_eq!(issue_of("12-gadgets"), Some(Number(12)));
    assert_eq!(issue_of("12-blue-gadgets-2"), Some(Number(12)));
    assert_eq!(issue_of("12"), Some(Number(12)));
    for name in [
        "012-gadgets",
        "12--gadgets",
        "12-Gadgets",
        "gadgets",
        "-gadgets",
    ] {
        assert_eq!(issue_of(name), None, "{name}");
    }
}

#[test]
fn a_tickets_branch_is_named_for_it_with_the_first_number_from_2_no_open_pull_request_holds() {
    let ticket = ticket(12, "Blue gadgets");
    let on = |head: &str, state: PullState| Pull {
        head_ref: head.into(),
        state,
        ..open_pull(20, "alice", &[], &[])
    };
    let open = PullState::Open { draft: false };
    let closed = PullState::Closed {
        at: at("2026-10-01T09:00:00Z"),
    };
    let cases = [
        ("no pull request", vec![], "12-blue-gadgets"),
        (
            "the name held by an open pull request",
            vec![on("12-blue-gadgets", open)],
            "12-blue-gadgets-2",
        ),
        (
            "the name and the next held by open pull requests",
            vec![on("12-blue-gadgets", open), on("12-blue-gadgets-2", open)],
            "12-blue-gadgets-3",
        ),
        (
            "the name held by a closed pull request",
            vec![on("12-blue-gadgets", closed)],
            "12-blue-gadgets",
        ),
    ];
    for (case, pulls, branch) in cases {
        let snapshot = Snapshot {
            pulls,
            ..snapshot(vec![ticket.clone()], vec![])
        };
        assert_eq!(snapshot.ticket_branch(&ticket), branch, "{case}");
    }
}
