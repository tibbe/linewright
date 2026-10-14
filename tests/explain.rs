//! `linewright explain N`, end to end on its data model: `serve`'s view of
//! the repository and the clock in, the answer about #N and its text out
//! (`CODING_STANDARDS.md`, "Where tests go"; `docs/specs/monitoring.md`,
//! "explain").

mod views;

use linewright::domain::*;
use linewright::explain::{Answer, Explanation, explain, explain_read, render, render_answer};
use linewright::status::View;
use linewright::store::AnyItem;
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

/// The answer about an item `serve` read from GitHub, as text.
fn left_alone_text(view: &View, read: AnyItem) -> Vec<String> {
    let answer = explain_read(view, &read, at(NOW));
    render_answer(&answer, false)
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn a_closed_issue_is_left_alone_as_closed() {
    let closed = Issue {
        open: false,
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        left_alone_text(
            &view(vec![], vec![]),
            AnyItem::LeftOutIssue(Box::new(closed))
        ),
        [
            "#42: Add invoices",
            "state: closed issue",
            "last read: 2026-10-01 12:00:30 UTC",
            "left alone: it is closed",
        ]
    );
}

#[test]
fn an_open_issue_a_snapshot_does_not_read_names_the_labels_and_linewright() {
    let elsewhere = Issue {
        labels: vec!["ready-for-human".into()],
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        left_alone_text(
            &view(vec![], vec![]),
            AnyItem::LeftOutIssue(Box::new(elsewhere))
        )[1..],
        [
            "state: open issue",
            "last read: 2026-10-01 12:00:30 UTC",
            "left alone: it is labeled none of `needs-triage`, `needs-info` or `ready-for-agent`, \
             and Linewright neither opened it nor is assigned to it",
        ]
    );
}

#[test]
fn an_open_issue_with_no_pipeline_label_names_the_labels() {
    let unlabeled = Issue {
        labels: vec!["bug".into()],
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        left_alone_text(
            &view(vec![unlabeled.clone()], vec![]),
            AnyItem::HeldIssue(Box::new(unlabeled))
        )[3],
        "left alone: it is labeled none of `needs-triage`, `needs-info`, `ready-for-agent` or \
         `ready-for-human`"
    );
}

#[test]
fn an_issue_of_linewrights_own_labeled_needs_triage_is_left_alone_as_its_own() {
    let filed = Issue {
        author: Login::new("factory"),
        labels: vec!["needs-triage".into()],
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        left_alone_text(
            &view(vec![filed.clone()], vec![]),
            AnyItem::HeldIssue(Box::new(filed))
        )[3],
        "left alone: Linewright opened it and triages no issue of its own, though it is labeled \
         `needs-triage`"
    );
}

#[test]
fn a_tracking_issue_with_no_tickets_is_left_alone_as_such() {
    let tracking = Issue {
        author: Login::new("factory"),
        labels: vec![],
        filed: Some(Filed::Tracking {
            spec_change: Number(5),
        }),
        ..ticket(42, "Tickets for #5")
    };

    assert_eq!(
        left_alone_text(
            &view(vec![tracking.clone()], vec![]),
            AnyItem::HeldIssue(Box::new(tracking))
        )[3],
        "left alone: it is a tracking issue with no tickets, and none are being cut"
    );
}

#[test]
fn an_issue_the_read_places_in_the_pipeline_is_explained_as_read_then() {
    let unlabeled = Issue {
        labels: vec![],
        ..ticket(42, "Add invoices")
    };
    let labeled = ticket(42, "Add invoices");

    assert_eq!(
        left_alone_text(
            &view(vec![unlabeled], vec![]),
            AnyItem::HeldIssue(Box::new(labeled))
        ),
        [
            "#42: Add invoices",
            "state: ready to claim",
            "last read: 2026-10-01 12:00:30 UTC",
            "waiting for: nothing: Linewright acts on it at its next poll",
        ]
    );
}

/// The maintainer's pull request #`number`, open.
fn someones_pull(number: u64) -> Pull {
    Pull {
        author: Login::new("maintainer"),
        title: "Tidy up".into(),
        closes: vec![],
        ..draft(number, 1, &[])
    }
}

#[test]
fn a_closed_pull_request_is_left_alone_as_closed() {
    let closed = Pull {
        state: PullState::Closed {
            at: at("2026-10-01T11:00:00Z"),
        },
        ..draft(43, 42, &[])
    };

    assert_eq!(
        left_alone_text(&view(vec![], vec![]), AnyItem::Pull(Box::new(closed)))[1..],
        [
            "state: closed pull request",
            "last read: 2026-10-01 12:00:30 UTC",
            "left alone: it is closed",
        ]
    );
}

#[test]
fn a_pull_request_linewright_did_not_open_names_who_did() {
    assert_eq!(
        left_alone_text(
            &view(vec![], vec![]),
            AnyItem::Pull(Box::new(someones_pull(43)))
        ),
        [
            "#43: Tidy up",
            "state: open pull request",
            "last read: 2026-10-01 12:00:30 UTC",
            "left alone: maintainer opened it, and Linewright works only on its own pull requests",
        ]
    );
}

#[test]
fn linewrights_pull_request_for_no_issue_it_works_on_says_so() {
    let for_nothing = Pull {
        closes: vec![],
        ..draft(43, 42, &[])
    };
    let for_untracked = draft(43, 42, &[]);
    let untracked = Issue {
        labels: vec![],
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        left_alone_text(&view(vec![], vec![]), AnyItem::Pull(Box::new(for_nothing)))[3],
        "left alone: it is Linewright's, and closes no issue Linewright could work on"
    );
    assert_eq!(
        left_alone_text(
            &view(vec![untracked], vec![]),
            AnyItem::Pull(Box::new(for_untracked))
        )[3],
        "left alone: it is Linewright's for #42, which Linewright does not work on"
    );
}

#[test]
fn linewrights_older_pull_request_for_an_issue_names_the_one_it_works_through() {
    let claimed = Issue {
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    let older = draft(43, 42, &[]);
    let view = view(vec![claimed], vec![older.clone(), draft(44, 42, &[])]);

    assert_eq!(
        left_alone_text(&view, AnyItem::Pull(Box::new(older)))[3],
        "left alone: it is Linewright's for #42, which it works on through #44"
    );
}

/// The maintainer's pull request #5, merged into `main` at 09:00 changing
/// the spec, labeled `labels`.
fn merged(labels: &[&str]) -> MergedPull {
    MergedPull {
        number: Number(5),
        title: "Add invoices to the spec".into(),
        body: String::new(),
        author: Login::new("maintainer"),
        merged_at: at("2026-10-01T09:00:00Z"),
        base_ref: "main".into(),
        labels: labels.iter().map(|l| l.to_string()).collect(),
        files: vec!["docs/specs/invoices.md".into()],
    }
}

fn merged_text(view: &View, pull: MergedPull) -> String {
    let lines = left_alone_text(view, AnyItem::Merged(pull));
    assert_eq!(lines[1], "state: merged pull request");
    lines[3].clone()
}

#[test]
fn a_merged_pull_request_names_the_rule_that_makes_it_no_spec_change() {
    let view = view(vec![], vec![]);

    assert_eq!(
        merged_text(
            &view,
            MergedPull {
                author: Login::new("factory"),
                ..merged(&[])
            }
        ),
        "left alone: it is no spec change: Linewright opened it"
    );
    assert_eq!(
        merged_text(
            &view,
            MergedPull {
                base_ref: "release".into(),
                ..merged(&[])
            }
        ),
        "left alone: it is no spec change: it was merged into `release`, not the default branch"
    );
    assert_eq!(
        merged_text(
            &view,
            MergedPull {
                files: vec!["src/main.rs".into()],
                ..merged(&[])
            }
        ),
        "left alone: it is no spec change: it touches no file matching `docs/**/*.md`, the \
         spec-glob"
    );
    assert_eq!(
        merged_text(&view, merged(&["linewright:spec-processed"])),
        "left alone: it is no spec change: it is labeled `linewright:spec-processed`: cut into \
         tickets already"
    );
    assert_eq!(
        merged_text(&view, merged(&["ready-for-human"])),
        "left alone: it is no spec change: it is labeled `ready-for-human`: handed to the \
         maintainer"
    );
    assert_eq!(
        merged_text(
            &view,
            MergedPull {
                merged_at: at("2026-09-20T09:00:00Z"),
                ..merged(&[])
            }
        ),
        "left alone: it is no spec change: it was merged 2026-09-20 09:00:00 UTC, before the \
         spec-lookback window opened 2026-09-24 12:00:30 UTC"
    );
}

#[test]
fn a_merged_pull_request_without_the_required_label_names_it() {
    let mut view = view(vec![], vec![]);
    view.policy.require_label = Some("linewright".into());

    assert_eq!(
        merged_text(&view, merged(&[])),
        "left alone: it is no spec change: it is not labeled `linewright`, the required label"
    );
}

#[test]
fn a_spec_change_the_read_finds_is_explained_as_waiting_for_tickets() {
    let answer = explain_read(
        &view(vec![], vec![]),
        &AnyItem::Merged(merged(&[])),
        at(NOW),
    );

    assert!(
        matches!(&answer, Answer::Tracked(e) if e.waiting_for.is_empty()),
        "{answer:?}"
    );
    assert_eq!(
        render_answer(&answer, false).lines().nth(1),
        Some("state: spec change waiting for tickets")
    );
}

#[test]
fn the_json_of_an_item_left_alone_names_why() {
    let answer = explain_read(
        &view(vec![], vec![]),
        &AnyItem::Merged(MergedPull {
            files: vec!["src/main.rs".into()],
            ..merged(&[])
        }),
        at(NOW),
    );

    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        serde_json::json!({
            "number": 5,
            "title": "Add invoices to the spec",
            "item": "pull-request",
            "state": "merged",
            "last_read": "2026-10-01T12:00:30Z",
            "left_alone": {
                "reason": "no-spec-file",
                "spec_glob": "docs/**/*.md",
            },
        })
    );
    assert_eq!(
        serde_json::from_value::<Answer>(serde_json::to_value(&answer).unwrap()).unwrap(),
        answer
    );
}

#[test]
fn a_tracked_answer_reads_back_as_one() {
    let answer = Answer::Tracked(
        explain(
            &view(vec![ticket(42, "Add invoices")], vec![]),
            Number(42),
            at(NOW),
        )
        .unwrap(),
    );

    assert_eq!(
        serde_json::from_value::<Answer>(serde_json::to_value(&answer).unwrap()).unwrap(),
        answer
    );
}

#[test]
fn linewrights_pull_request_the_read_places_in_review_is_explained_as_its_issues() {
    let claimed = Issue {
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    let opened = draft(43, 42, &[CheckState::Pending]);

    assert_eq!(
        left_alone_text(
            &view(vec![claimed], vec![]),
            AnyItem::Pull(Box::new(opened))
        ),
        [
            "#43: Do #42",
            "pull request for: #42",
            "state: in review: #43 (draft, CI running)",
            "last read: 2026-10-01 12:00:30 UTC",
            "waiting for: CI on #43",
        ]
    );
}

#[test]
fn linewrights_pull_request_for_an_issue_elsewhere_in_the_pipeline_names_where() {
    let handed_back = Issue {
        labels: vec!["ready-for-human".into()],
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    let open = draft(43, 42, &[]);
    let view = view(vec![handed_back], vec![open.clone()]);

    assert_eq!(
        left_alone_text(&view, AnyItem::Pull(Box::new(open)))[3],
        "left alone: it is Linewright's for #42, which is not in review but with the maintainer"
    );
}

#[test]
fn the_json_of_an_open_issue_left_alone_names_why() {
    let unlabeled = Issue {
        labels: vec![],
        ..ticket(42, "Add invoices")
    };
    let held = explain_read(
        &view(vec![], vec![]),
        &AnyItem::HeldIssue(Box::new(unlabeled.clone())),
        at(NOW),
    );
    let left_out = explain_read(
        &view(vec![], vec![]),
        &AnyItem::LeftOutIssue(Box::new(unlabeled)),
        at(NOW),
    );

    for (answer, reason) in [(&held, "no-pipeline-label"), (&left_out, "not-in-view")] {
        assert_eq!(
            serde_json::to_value(answer).unwrap(),
            serde_json::json!({
                "number": 42,
                "title": "Add invoices",
                "item": "issue",
                "state": "open",
                "last_read": "2026-10-01T12:00:30Z",
                "left_alone": {"reason": reason},
            })
        );
        assert_eq!(
            &serde_json::from_value::<Answer>(serde_json::to_value(answer).unwrap()).unwrap(),
            answer
        );
    }
}
