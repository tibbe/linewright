use crate::common;

use common::*;
use linewright::domain::*;
use linewright::party::parties;

#[track_caller]
fn assert_parties(snapshot: &Snapshot, number: u64, expected: &[&str]) {
    assert_eq!(
        parties(snapshot, &linewright(), Number(number)),
        logins(expected).into_iter().collect()
    );
}

#[test_log::test]
fn a_person_is_party_to_their_own_issue() {
    let issue = Issue {
        author: Login::new("alice"),
        ..ticket(1, "Store gadgets")
    };

    assert_parties(&snapshot_of(vec![issue]), 1, &["alice"]);
}

#[test_log::test]
fn an_assignee_is_party_to_an_item() {
    let issue = Issue {
        assignees: logins(&["bob"]),
        ..ticket(1, "Store gadgets")
    };

    assert_parties(&snapshot_of(vec![issue]), 1, &["bob", "maintainer"]);
}

/// Ticket #1 by the maintainer, and Linewright's pull request #10 closing
/// it, with `pull`'s changes.
fn ticket_in_review(pull: impl FnOnce(Pull) -> Pull) -> Snapshot {
    Snapshot {
        pulls: vec![pull(open_pull(10, LINEWRIGHT, &[1], &[]))],
        ..snapshot_of(vec![ticket(1, "Store gadgets")])
    }
}

#[test_log::test]
fn a_person_a_review_is_requested_of_by_name_is_party_to_the_pull_request_and_its_issue() {
    let snapshot = ticket_in_review(|pull| Pull {
        review_requests: logins(&["carol"]),
        ..pull
    });

    assert_parties(&snapshot, 1, &["carol", "maintainer"]);
    assert_parties(&snapshot, 10, &["carol", "maintainer"]);
}

#[test_log::test]
fn a_person_who_reviewed_a_pull_request_is_party_to_it_and_its_issue() {
    let snapshot = ticket_in_review(|pull| Pull {
        reviews: vec![Review {
            author: Login::new("dave"),
            state: ReviewState::Commented,
            body: "Looks good".into(),
            submitted_at: at("2026-10-01T11:00:00Z"),
        }],
        ..pull
    });

    assert_parties(&snapshot, 1, &["dave", "maintainer"]);
    assert_parties(&snapshot, 10, &["dave", "maintainer"]);
}

#[test_log::test]
fn a_ticket_linewright_cut_counts_as_opened_by_its_tracking_issues_opener() {
    let tracking = Issue {
        author: Login::new("erin"),
        sub_issues: vec![sub_issue(2, true, LINEWRIGHT)],
        ..ticket(1, "Gadgets")
    };
    let cut = Issue {
        author: linewright(),
        parent: Some(Parent {
            number: Number(1),
            spec_change: None,
        }),
        ..ticket(2, "Store blue gadgets")
    };

    assert_parties(&snapshot_of(vec![tracking, cut]), 2, &["erin"]);
}

/// #20, the issue Linewright opened for spec change #7, and #21, a ticket
/// cut from it.
fn spec_changes_issue_and_its_ticket() -> Vec<Issue> {
    let issue = Issue {
        author: linewright(),
        filed: Some(Filed::SpecChange {
            spec_change: Number(7),
        }),
        ..ticket(20, "Spec change: Store gadgets")
    };
    let cut = Issue {
        author: linewright(),
        parent: Some(Parent {
            number: Number(20),
            spec_change: Some(Number(7)),
        }),
        ..ticket(21, "Store blue gadgets")
    };
    vec![issue, cut]
}

#[test_log::test]
fn a_spec_changes_issue_and_its_tickets_count_as_opened_by_the_spec_changes_author() {
    let held = Snapshot {
        merged: vec![merged_pull(7, "alice", &[])],
        ..snapshot_of(spec_changes_issue_and_its_ticket())
    };
    let only_opening = Snapshot {
        openings: vec![Opening {
            number: Number(7),
            author: Login::new("alice"),
            opened_for: vec![],
        }],
        ..snapshot_of(spec_changes_issue_and_its_ticket())
    };

    for snapshot in [held, only_opening] {
        assert_parties(&snapshot, 20, &["alice"]);
        assert_parties(&snapshot, 21, &["alice"]);
    }
}

#[test_log::test]
fn a_spec_gap_counts_as_opened_by_the_opener_of_the_item_whose_work_found_it() {
    let mut issues = spec_changes_issue_and_its_ticket();
    issues.push(Issue {
        author: linewright(),
        filed: Some(Filed::SpecGap { ticket: Number(21) }),
        ..ticket(22, "Spec gap: What is blue?")
    });
    let snapshot = Snapshot {
        openings: vec![Opening {
            number: Number(7),
            author: Login::new("alice"),
            opened_for: vec![],
        }],
        ..snapshot_of(issues)
    };

    assert_parties(&snapshot, 22, &["alice"]);
}

#[test_log::test]
fn no_one_is_party_to_an_issue_an_automation_files() {
    let filed = Issue {
        author: linewright(),
        ..ticket(1, "Dependencies are out of date")
    };

    assert_parties(&snapshot_of(vec![filed]), 1, &[]);
}

#[test_log::test]
fn a_person_who_reviewed_the_merged_pull_request_of_a_done_issue_is_party_to_it() {
    let done = Issue {
        author: Login::new("alice"),
        ..ticket(1, "Store gadgets")
    };
    let snapshot = Snapshot {
        closed: vec![closed_item(&done, "2026-10-01T10:00:00Z")],
        merged: vec![MergedPull {
            reviewers: logins(&["frank"]),
            ..merged_pull(30, LINEWRIGHT, &[1])
        }],
        ..snapshot_of(vec![])
    };

    assert_parties(&snapshot, 1, &["alice", "frank"]);
}

#[test_log::test]
fn linewright_is_party_to_nothing() {
    let issue = Issue {
        author: Login::new("alice"),
        assignees: logins(&["FACTORY"]),
        ..ticket(1, "Store gadgets")
    };
    let snapshot = Snapshot {
        pulls: vec![Pull {
            review_requests: logins(&[LINEWRIGHT]),
            ..open_pull(10, LINEWRIGHT, &[1], &[LINEWRIGHT])
        }],
        ..snapshot_of(vec![issue])
    };

    assert_parties(&snapshot, 1, &["alice"]);
}

#[test_log::test]
fn a_person_is_party_once_whatever_the_case_of_their_login() {
    let issue = Issue {
        author: Login::new("Alice"),
        assignees: logins(&["alice"]),
        ..ticket(1, "Store gadgets")
    };
    let snapshot = Snapshot {
        pulls: vec![Pull {
            review_requests: logins(&["ALICE"]),
            ..open_pull(10, LINEWRIGHT, &[1], &[])
        }],
        ..snapshot_of(vec![issue])
    };

    assert_parties(&snapshot, 1, &["alice"]);
}

#[test_log::test]
fn following_back_what_items_were_opened_for_ends_at_a_cycle() {
    let opened_by_linewright_for = |number: u64, opened_for: u64| Opening {
        number: Number(number),
        author: linewright(),
        opened_for: vec![Number(opened_for)],
    };
    let snapshot = Snapshot {
        openings: vec![
            opened_by_linewright_for(1, 2),
            opened_by_linewright_for(2, 1),
        ],
        ..snapshot_of(vec![])
    };

    assert_parties(&snapshot, 1, &[]);
}
