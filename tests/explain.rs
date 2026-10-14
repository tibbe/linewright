//! `linewright explain N`, end to end on its data model: `serve`'s view of
//! the repository and the clock in, the answer about #N and its text out
//! (`CODING_STANDARDS.md`, "Where tests go"; `docs/specs/monitoring.md`,
//! "explain").

mod views;

use linewright::domain::*;
use linewright::explain::{Explanation, explain, render};
use linewright::status::View;
use views::{at, implementing, limited_until, ticket, view};

/// Linewright's draft pull request #`number` for #`closes`, pushed at 11:58
/// with its checks `checks`.
fn draft(number: u64, closes: u64, checks: &[CheckState]) -> Pull {
    Pull {
        number: Number(number),
        title: format!("Do #{closes}"),
        body: format!("Closes #{closes}"),
        author: Login::new("factory"),
        created_at: at("2026-10-01T11:58:00Z"),
        head_ref: format!("{closes}-do"),
        head_sha: "abc123".into(),
        state: PullState::Open { draft: true },
        closes: vec![Number(closes)],
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
    }
}

const NOW: &str = "2026-10-01T12:00:30Z";

fn text(view: &View, number: u64) -> Vec<String> {
    let answer = explain(view, Number(number), at(NOW)).expect("a tracked item");
    render(&answer, false).lines().map(str::to_string).collect()
}

#[test]
fn explain_shows_where_the_item_is_when_serve_last_read_it_and_each_wait() {
    let blocked = Issue {
        blocked_by: vec![
            Linked {
                number: Number(40),
                open: true,
            },
            Linked {
                number: Number(41),
                open: true,
            },
        ],
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        text(&view(vec![blocked], vec![]), 42),
        [
            "#42: Add invoices",
            "state: ready to claim",
            "last read: 2026-10-01 12:00:00 UTC",
            "waiting for: #40, which blocks it",
            "waiting for: #41, which blocks it",
        ]
    );
}

#[test]
fn an_item_a_refresh_read_again_was_last_read_then() {
    let mut view = view(vec![ticket(42, "Add invoices")], vec![]);
    view.read_at
        .insert(Item::Issue(Number(42)), at("2026-10-01T12:00:20Z"));

    assert_eq!(text(&view, 42)[2], "last read: 2026-10-01 12:00:20 UTC");
}

#[test]
fn work_held_back_waits_for_a_free_run_or_the_usage_limit() {
    let ready = view(vec![ticket(42, "Add invoices")], vec![]);
    let full = View {
        live: [1, 2, 3, 4].map(implementing).to_vec(),
        ..ready.clone()
    };
    let limited = View {
        memory: limited_until("2026-10-01T14:30:00Z"),
        ..ready
    };

    assert_eq!(
        text(&full, 42)[3],
        "waiting for: a free run: 4 of 4 in flight"
    );
    assert_eq!(
        text(&limited, 42)[3],
        "waiting for: the usage limit, until 2026-10-01 14:30:00 UTC"
    );
}

#[test]
fn an_item_in_review_waits_for_ci_on_its_pull_request() {
    let claimed = Issue {
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    let view = view(vec![claimed], vec![draft(43, 42, &[CheckState::Pending])]);

    assert_eq!(
        text(&view, 42)[1..],
        [
            "state: in review: #43 (draft, CI running)",
            "last read: 2026-10-01 12:00:00 UTC",
            "waiting for: CI on #43",
        ]
    );
}

#[test]
fn an_item_with_the_maintainer_waits_for_them_with_the_label() {
    let handed_back = Issue {
        labels: vec!["ready-for-human".into()],
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        text(&view(vec![handed_back], vec![]), 42)[1..],
        [
            "state: with the maintainer",
            "last read: 2026-10-01 12:00:00 UTC",
            "waiting for: the maintainer: labeled `ready-for-human`",
        ]
    );
}

#[test]
fn an_item_linewright_acts_on_next_waits_for_nothing() {
    assert_eq!(
        text(&view(vec![ticket(42, "Add invoices")], vec![]), 42)[3..],
        ["waiting for: nothing: Linewright acts on it at its next poll"]
    );
}

#[test]
fn linewrights_pull_request_is_explained_as_the_issue_it_is_for() {
    let claimed = Issue {
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    let mut view = view(vec![claimed], vec![draft(43, 42, &[CheckState::Pending])]);
    view.read_at
        .insert(Item::Pull(Number(43)), at("2026-10-01T12:00:20Z"));

    assert_eq!(
        text(&view, 43),
        [
            "#43: Do #42",
            "pull request for: #42",
            "state: in review: #43 (draft, CI running)",
            "last read: 2026-10-01 12:00:20 UTC",
            "waiting for: CI on #43",
        ]
    );
}

#[test]
fn an_item_linewright_does_not_track_has_no_explanation() {
    let untracked = Issue {
        labels: vec![],
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        explain(&view(vec![untracked], vec![]), Number(42), at(NOW)),
        None
    );
    assert_eq!(explain(&view(vec![], vec![]), Number(7), at(NOW)), None);
}

#[test]
fn the_json_names_each_fact() {
    let claimed = Issue {
        assignees: vec![Login::new("factory")],
        blocked_by: vec![Linked {
            number: Number(40),
            open: true,
        }],
        ..ticket(42, "Add invoices")
    };
    let answer = explain(&view(vec![claimed], vec![]), Number(42), at(NOW)).unwrap();

    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        serde_json::json!({
            "number": 42,
            "title": "Add invoices",
            "state": "claimed",
            "last_read": "2026-10-01T12:00:00Z",
            "waiting_for": [{"wait": "blocker", "issue": 40}],
        })
    );
    assert_eq!(
        serde_json::from_value::<Explanation>(serde_json::to_value(&answer).unwrap()).unwrap(),
        answer
    );
}

#[test]
fn the_json_of_linewrights_pull_request_names_the_issue_it_is_for() {
    let claimed = Issue {
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    let view = view(vec![claimed], vec![draft(43, 42, &[CheckState::Pending])]);
    let answer = explain(&view, Number(43), at(NOW)).unwrap();

    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        serde_json::json!({
            "number": 43,
            "title": "Do #42",
            "pull_request_for": 42,
            "pull_request": 43,
            "draft": true,
            "ci": "running",
            "last_read": "2026-10-01T12:00:00Z",
            "waiting_for": [{"wait": "ci", "pull_request": 43}],
        })
    );
    assert_eq!(
        serde_json::from_value::<Explanation>(serde_json::to_value(&answer).unwrap()).unwrap(),
        answer
    );
}

#[test]
fn an_issue_of_linewrights_own_labeled_needs_triage_is_not_tracked() {
    let filed = Issue {
        author: Login::new("factory"),
        labels: vec!["needs-triage".into()],
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        explain(&view(vec![filed], vec![]), Number(42), at(NOW)),
        None
    );
}

#[test]
fn a_tracking_issue_with_no_tickets_and_no_cut_in_flight_is_not_tracked() {
    let tracking = Issue {
        author: Login::new("factory"),
        labels: vec![],
        filed: Some(Filed::Tracking {
            spec_change: Number(5),
        }),
        ..ticket(42, "Tickets for #5")
    };

    assert_eq!(
        explain(&view(vec![tracking], vec![]), Number(42), at(NOW)),
        None
    );
}
