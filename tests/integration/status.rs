use crate::views;

use linewright::automation::{AutomationFiles, LastRuns, Unparsed};
use linewright::domain::*;
use linewright::reconcile::{LiveRun, Memory, Subject, WorkKind};
use linewright::status::{Serving, Status, View, render, status};
use linewright::webhook::Received;
use views::{
    at, closed_by, draining_since, implementing, in_flight, limited_until, relabeled, sub_issue,
    ticket,
};

fn files(automations: &[(&str, &str)], unparsed: &[(&str, &str)]) -> AutomationFiles {
    AutomationFiles {
        parsed: automations
            .iter()
            .map(|&(name, cron)| {
                linewright::automation::parse(
                    &name.parse().unwrap(),
                    &format!("---\non:\n  schedule: \"{cron}\"\n---\nLook.\n"),
                )
                .unwrap()
            })
            .collect(),
        unparsed: unparsed
            .iter()
            .map(|&(path, reason)| Unparsed {
                path: path.into(),
                reason: reason.into(),
            })
            .collect(),
    }
}

fn last_runs(runs: &[(&str, &str)]) -> LastRuns {
    runs.iter()
        .map(|&(name, time)| (name.parse().unwrap(), at(time)))
        .collect()
}

fn serving() -> Serving {
    Serving {
        since: at("2026-10-01T11:00:00Z"),
        max_runs: 4,
    }
}

fn view(issues: Vec<Issue>) -> View {
    views::view(issues, vec![])
}

fn ready_issue() -> Issue {
    ticket(7, "Do the thing")
}

fn text(view: Option<&View>, now: &str) -> Vec<String> {
    text_with(None, view, now)
}

fn text_with(webhooks: Option<Received>, view: Option<&View>, now: &str) -> Vec<String> {
    let answer = status(&serving(), webhooks, view, None, at(now));
    render(&answer, false).lines().map(str::to_string).collect()
}

fn after_linewright(view: &View, now: &str) -> Vec<String> {
    let lines = text(Some(view), now);
    let blank = lines
        .iter()
        .position(String::is_empty)
        .expect("a blank line");
    lines[blank + 1..].to_vec()
}

#[test]
fn the_linewright_section_says_since_when_serve_runs_its_polls_and_the_runs_in_flight() {
    let view = View {
        live: [1, 2].map(implementing).to_vec(),
        in_flight: vec![in_flight("weekly/1", "2026-10-01T12:00:00Z", None)],
        ..view(vec![])
    };

    assert_eq!(
        text(Some(&view), "2026-10-01T12:00:30Z")[..4],
        [
            "serving since: 2026-10-01 11:00:00 UTC",
            "last poll: 2026-10-01 12:00:00 UTC",
            "next poll: 2026-10-01 12:01:00 UTC",
            "runs in flight: 3 of 4",
        ]
    );
}

#[test]
fn before_the_first_poll_there_is_no_last_poll_and_the_next_is_due_now() {
    assert_eq!(
        text(None, "2026-10-01T11:00:05Z"),
        [
            "serving since: 2026-10-01 11:00:00 UTC",
            "last poll: none yet",
            "next poll: due now",
            "runs in flight: 0 of 4",
        ]
    );
}

#[test]
fn a_usage_limit_in_force_is_shown_with_its_reset() {
    let view = View {
        memory: limited_until("2026-10-01T14:30:00Z"),
        ..view(vec![])
    };

    let lines = text(Some(&view), "2026-10-01T12:00:30Z");

    assert_eq!(
        lines[3..],
        [
            "usage limit: in force until 2026-10-01 14:30:00 UTC",
            "runs in flight: 0 of 4",
        ]
    );
}

#[test]
fn a_drain_under_way_is_shown_with_since_when_after_the_usage_limit() {
    let view = View {
        memory: Memory {
            usage_limit: limited_until("2026-10-01T14:30:00Z").usage_limit,
            ..draining_since("2026-10-01T12:00:20Z")
        },
        ..view(vec![])
    };

    let lines = text(Some(&view), "2026-10-01T12:00:30Z");

    assert_eq!(
        lines[3..],
        [
            "usage limit: in force until 2026-10-01 14:30:00 UTC",
            "drain: under way since 2026-10-01 12:00:20 UTC",
            "runs in flight: 0 of 4",
        ]
    );
}

#[test]
fn a_drain_under_way_is_in_the_json_and_is_null_without_one() {
    let draining = View {
        memory: draining_since("2026-10-01T12:00:20Z"),
        ..view(vec![])
    };
    let now = at("2026-10-01T12:00:30Z");

    let shown = serde_json::to_value(status(&serving(), None, Some(&draining), None, now)).unwrap();
    let none =
        serde_json::to_value(status(&serving(), None, Some(&view(vec![])), None, now)).unwrap();

    assert_eq!(
        shown["linewright"]["drain"],
        serde_json::json!({"since": "2026-10-01T12:00:20Z", "deadline": null})
    );
    assert_eq!(none["linewright"]["drain"], serde_json::Value::Null);
}

#[test]
fn a_drain_with_a_deadline_shows_it_in_text_and_json() {
    let draining = View {
        memory: draining_since("2026-10-01T12:00:20Z"),
        ..view(vec![])
    };
    let now = at("2026-10-01T12:00:30Z");
    let deadline = Some(at("2026-10-01T14:00:20Z"));

    let answer = status(&serving(), None, Some(&draining), deadline, now);
    let shown = serde_json::to_value(&answer).unwrap();
    let text = render(&answer, false);

    assert_eq!(
        shown["linewright"]["drain"],
        serde_json::json!({"since": "2026-10-01T12:00:20Z", "deadline": "2026-10-01T14:00:20Z"})
    );
    assert!(
        text.lines().any(|line| line
            == "drain: under way since 2026-10-01 12:00:20 UTC; deadline 2026-10-01 14:00:20 UTC"),
        "{text}"
    );
}

#[test]
fn while_draining_an_item_waits_for_the_drain() {
    let view = View {
        memory: draining_since("2026-10-01T12:00:20Z"),
        ..view(vec![ready_issue()])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:00:30Z"),
        ["#7: ready to claim — waiting for the drain, until `serve` restarts — Do the thing"]
    );
}

#[test]
fn a_usage_limit_whose_reset_has_passed_is_not_shown() {
    let view = View {
        memory: limited_until("2026-10-01T12:00:10Z"),
        ..view(vec![])
    };

    let answer = status(
        &serving(),
        None,
        Some(&view),
        None,
        at("2026-10-01T12:00:30Z"),
    );

    assert_eq!(answer.linewright.usage_limit_resets_at, None);
}

#[test]
fn with_webhooks_on_the_last_accepted_delivery_and_the_refused_ones_are_shown() {
    let webhooks = Received {
        last_accepted: Some(at("2026-10-01T11:59:00Z")),
        refused: 2,
    };

    let lines = text_with(Some(webhooks), Some(&view(vec![])), "2026-10-01T12:00:30Z");

    assert_eq!(
        lines[4..],
        [
            "last delivery accepted: 2026-10-01 11:59:00 UTC",
            "deliveries refused: 2",
        ]
    );
}

#[test]
fn with_webhooks_on_and_no_delivery_yet_none_is_shown_accepted() {
    let lines = text_with(
        Some(Received::default()),
        Some(&view(vec![])),
        "2026-10-01T12:00:30Z",
    );

    assert_eq!(
        lines[4..],
        ["last delivery accepted: none yet", "deliveries refused: 0"]
    );
}

#[test]
fn each_run_in_flight_follows_the_linewright_section_with_how_long_it_has_run_and_been_quiet() {
    let view = View {
        in_flight: vec![
            in_flight("43/triage-1", "2026-10-01T12:05:00Z", None),
            in_flight(
                "42/rework-2",
                "2026-10-01T11:00:00Z",
                Some("2026-10-01T11:40:00Z"),
            ),
            in_flight(
                "weekly/3",
                "2026-10-01T12:00:00Z",
                Some("2026-10-01T12:04:59Z"),
            ),
        ],
        ..view(vec![ready_issue()])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:05:00Z"),
        [
            "42/rework-2: #42 rework, running for 1h 5m, quiet for 25m",
            "weekly/3: automation weekly, running for 5m, quiet for 1s",
            "43/triage-1: #43 triage, running for 0s, no session yet",
            "",
            "#7: ready to claim — Do the thing",
        ]
    );
}

#[test]
fn an_item_with_a_run_in_flight_shows_as_its_run_not_among_the_other_items() {
    let busy = Issue {
        number: Number(9),
        title: "Do another thing".into(),
        assignees: vec![Login::new("factory")],
        ..ready_issue()
    };
    let view = View {
        live: vec![implementing(9)],
        in_flight: vec![in_flight(
            "9/implement-1",
            "2026-10-01T12:00:00Z",
            Some("2026-10-01T12:00:10Z"),
        )],
        ..view(vec![ready_issue(), busy])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:00:30Z"),
        [
            "9/implement-1: #9 implement, running for 30s, quiet for 20s",
            "",
            "#7: ready to claim — Do the thing",
        ]
    );
}

#[test]
fn a_ticket_someone_elses_open_pull_request_closes_is_in_review_and_waits_for_it() {
    let closed = closed_by(ready_issue(), 43, "alice");

    assert_eq!(
        after_linewright(&view(vec![closed]), "2026-10-01T12:00:30Z"),
        ["#7: in review: #43, opened by alice — waiting for #43, which closes it — Do the thing"]
    );
}

#[test]
fn a_ticket_someone_else_is_assigned_to_waits_for_them() {
    let assigned = Issue {
        assignees: vec![Login::new("alice")],
        ..ready_issue()
    };

    assert_eq!(
        after_linewright(&view(vec![assigned]), "2026-10-01T12:00:30Z"),
        ["#7: ready to claim — waiting for alice, who is assigned to it — Do the thing"]
    );
}

#[test]
fn a_blocked_ticket_linewright_is_assigned_to_by_hand_shows_as_held_by_its_blocker() {
    let held = Issue {
        assignees: vec![Login::new("factory"), Login::new("alice")],
        blocked_by: vec![Linked {
            number: Number(5),
            open: true,
        }],
        events: vec![IssueEvent::Assignment(Assignment::Assigned {
            assignee: Login::new("factory"),
            actor: Login::new("maintainer"),
            at: at("2026-10-01T11:30:00Z"),
        })],
        ..ready_issue()
    };

    assert_eq!(
        after_linewright(&view(vec![held]), "2026-10-01T12:00:30Z"),
        ["#7: claimed: being implemented — waiting for #5, which blocks it — Do the thing"]
    );
}

#[test]
fn the_managed_items_follow_the_linewright_section_each_as_its_number_and_where_it_is() {
    let blocked = Issue {
        number: Number(8),
        title: "Do the next thing".into(),
        blocked_by: vec![Linked {
            number: Number(7),
            open: true,
        }],
        ..ready_issue()
    };
    let claimed = Issue {
        number: Number(9),
        title: "Do another thing".into(),
        assignees: vec![Login::new("factory")],
        ..ready_issue()
    };
    let triage = Issue {
        number: Number(10),
        title: "It broke".into(),
        labels: vec!["needs-triage".into()],
        ..ready_issue()
    };
    let triaging = Issue {
        number: Number(11),
        title: "It broke again".into(),
        labels: vec!["needs-triage".into()],
        assignees: vec![Login::new("factory")],
        ..ready_issue()
    };
    let view = view(vec![ready_issue(), blocked, claimed, triage, triaging]);

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:00:30Z"),
        [
            "#7: ready to claim — Do the thing",
            "#8: ready to claim — waiting for #7, which blocks it — Do the next thing",
            "#9: claimed: being implemented — Do another thing",
            "#10: waiting for triage — It broke",
            "#11: claimed: being triaged — It broke again",
        ]
    );
}

/// An issue Linewright cut into tickets #8, closed, and #9, open.
fn cut_issue() -> Issue {
    Issue {
        labels: vec![label::DESIGNED.into()],
        sub_issues: vec![
            sub_issue(8, false, "factory"),
            sub_issue(9, true, "factory"),
        ],
        ..ready_issue()
    }
}

#[test]
fn an_issue_cut_into_tickets_shows_as_their_tracking_issue_with_its_closed_tickets() {
    assert_eq!(
        after_linewright(&view(vec![cut_issue()]), "2026-10-01T12:00:30Z"),
        [
            "#7: tracking issue, 1/2 tickets closed — waiting for its tickets: #9 open — Do the thing"
        ]
    );
}

#[test]
fn an_issue_not_designed_yet_shows_as_waiting_for_or_in_its_design() {
    let undesigned = Issue {
        labels: vec!["ready-for-agent".into()],
        ..ready_issue()
    };
    let designing = Issue {
        number: Number(9),
        title: "Do another thing".into(),
        assignees: vec![Login::new("factory")],
        ..undesigned.clone()
    };
    let view = view(vec![undesigned, designing]);

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:00:30Z"),
        [
            "#7: ready to claim, waiting for its design — Do the thing",
            "#9: claimed: being designed — Do another thing",
        ]
    );
}

#[test]
fn an_issue_carrying_two_status_labels_shows_by_the_newer() {
    let handed_back = Issue {
        number: Number(8),
        title: "It broke".into(),
        ..relabeled(ready_issue(), "needs-triage", "ready-for-human")
    };
    let view = view(vec![
        relabeled(ready_issue(), "ready-for-agent", "needs-triage"),
        handed_back,
    ]);

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:00:30Z"),
        [
            "#7: waiting for triage — Do the thing",
            "#8: with the maintainer — waiting for the maintainer: to label it `ready-for-agent` or `needs-triage` — It broke",
        ]
    );
}

#[test]
fn each_managed_item_says_in_one_line_what_linewright_waits_for_on_it() {
    let blocked_and_held = Issue {
        number: Number(8),
        title: "Do the next thing".into(),
        blocked_by: vec![
            Linked {
                number: Number(5),
                open: true,
            },
            Linked {
                number: Number(6),
                open: true,
            },
        ],
        ..ready_issue()
    };
    let busy = Issue {
        number: Number(9),
        title: "Do another thing".into(),
        assignees: vec![Login::new("factory")],
        ..ready_issue()
    };
    let with_the_maintainer = Issue {
        number: Number(10),
        title: "It broke".into(),
        labels: vec!["ready-for-human".into()],
        ..ready_issue()
    };
    let view = View {
        memory: limited_until("2026-10-01T14:30:00Z"),
        live: vec![implementing(9)],
        ..view(vec![
            ready_issue(),
            blocked_and_held,
            busy,
            with_the_maintainer,
        ])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:00:30Z"),
        [
            "#7: ready to claim — waiting for the usage limit, until 2026-10-01 14:30:00 UTC — Do the thing",
            "#8: ready to claim — waiting for #5, which blocks it; #6, which blocks it — Do the next thing",
            "#9: claimed: being implemented — waiting for its implement run, in flight — Do another thing",
            "#10: with the maintainer — waiting for the maintainer: to label it `ready-for-agent` or `needs-triage` — It broke",
        ]
    );
}

#[test]
fn an_automation_is_listed_with_its_last_run_and_its_next_tick_in_utc() {
    let view = View {
        automations: files(&[("weekly", "0 6 * * 1")], &[]),
        last_runs: last_runs(&[("weekly", "2026-09-28T06:00:02Z")]),
        ..view(vec![])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:00:30Z"),
        [
            "automation weekly last run: 2026-09-28 06:00:02 UTC",
            "automation weekly next run: 2026-10-05 06:00:00 UTC",
        ]
    );
}

#[test]
fn an_automation_whose_schedule_ticked_since_its_last_run_is_due_now() {
    let view = View {
        automations: files(&[("hourly", "0 * * * *")], &[]),
        last_runs: last_runs(&[("hourly", "2026-10-01T11:30:00Z")]),
        ..view(vec![])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:10:00Z"),
        [
            "automation hourly last run: 2026-10-01 11:30:00 UTC",
            "automation hourly next run: due now",
        ]
    );
}

#[test]
fn an_automation_whose_schedule_never_ticks_again_has_no_next_run() {
    let view = View {
        automations: files(&[("leap", "0 0 30 2 *")], &[]),
        last_runs: last_runs(&[("leap", "2026-09-28T06:00:00Z")]),
        ..view(vec![])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:10:00Z"),
        [
            "automation leap last run: 2026-09-28 06:00:00 UTC",
            "automation leap next run: none, its schedule ticks no more",
        ]
    );
}

#[test]
fn an_automation_with_no_recorded_run_is_shown_as_not_run_yet() {
    let view = View {
        automations: files(&[("hourly", "0 * * * *")], &[]),
        ..view(vec![])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:10:00Z"),
        ["automation hourly last run: none yet, its schedule starts when a poll first sees it"]
    );
}

#[test]
fn the_items_come_before_the_automations_by_name_then_the_files_that_do_not_parse() {
    let view = View {
        automations: files(
            &[("weekly", "0 6 * * 1"), ("hourly", "0 * * * *")],
            &[(".linewright/automations/broken.md", "no front matter")],
        ),
        last_runs: last_runs(&[
            ("weekly", "2026-09-28T06:00:00Z"),
            ("hourly", "2026-10-01T12:00:00Z"),
        ]),
        ..view(vec![ready_issue()])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:10:00Z"),
        [
            "#7: ready to claim — Do the thing",
            "",
            "automation hourly last run: 2026-10-01 12:00:00 UTC",
            "automation hourly next run: 2026-10-01 13:00:00 UTC",
            "automation weekly last run: 2026-09-28 06:00:00 UTC",
            "automation weekly next run: 2026-10-05 06:00:00 UTC",
            ".linewright/automations/broken.md: does not parse: no front matter",
        ]
    );
}

#[test]
fn on_a_terminal_the_text_is_coloured_and_otherwise_not() {
    let view = View {
        automations: files(
            &[],
            &[(".linewright/automations/broken.md", "no front matter")],
        ),
        ..view(vec![ready_issue()])
    };
    let answer = status(
        &serving(),
        None,
        Some(&view),
        None,
        at("2026-10-01T12:10:00Z"),
    );

    let coloured = render(&answer, true);
    let plain = render(&answer, false);

    assert!(coloured.contains('\x1b'), "{coloured}");
    assert!(!plain.contains('\x1b'), "{plain}");
}

#[test]
fn the_answer_reads_back_from_its_json_as_it_was() {
    let webhooks = Received {
        last_accepted: Some(at("2026-10-01T11:59:00Z")),
        refused: 2,
    };
    let view = View {
        automations: files(&[("weekly", "0 6 * * 1")], &[("x.md", "bad")]),
        last_runs: last_runs(&[("weekly", "2026-09-28T06:00:00Z")]),
        in_flight: vec![
            in_flight(
                "42/rework-2",
                "2026-10-01T11:00:00Z",
                Some("2026-10-01T11:40:00Z"),
            ),
            in_flight("weekly/3", "2026-10-01T12:09:00Z", None),
        ],
        ..view(vec![ready_issue()])
    };
    let answer = status(
        &serving(),
        Some(webhooks),
        Some(&view),
        None,
        at("2026-10-01T12:10:00Z"),
    );

    let json = serde_json::to_string(&answer).unwrap();

    assert_eq!(serde_json::from_str::<Status>(&json).unwrap(), answer);
}

#[test]
fn the_json_names_each_fact() {
    let view = View {
        automations: files(&[("weekly", "0 6 * * 1")], &[]),
        last_runs: last_runs(&[("weekly", "2026-09-28T06:00:00Z")]),
        live: vec![LiveRun {
            subject: Subject::Issue(Number(42)),
            kind: WorkKind::Rework,
        }],
        in_flight: vec![
            in_flight(
                "42/rework-2",
                "2026-10-01T11:00:00Z",
                Some("2026-10-01T11:40:00Z"),
            ),
            in_flight("weekly/3", "2026-10-01T12:09:00Z", None),
        ],
        ..view(vec![ready_issue()])
    };
    let answer = status(
        &serving(),
        None,
        Some(&view),
        None,
        at("2026-10-01T12:10:00Z"),
    );

    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        serde_json::json!({
            "linewright": {
                "serving_since": "2026-10-01T11:00:00Z",
                "polls": {
                    "last": "2026-10-01T12:00:00Z",
                    "next": "2026-10-01T12:01:00Z",
                },
                "usage_limit_resets_at": null,
                "drain": null,
                "runs_in_flight": 2,
                "max_runs": 4,
                "webhooks": null,
            },
            "runs": [
                {
                    "id": "42/rework-2",
                    "item": 42,
                    "stage": "rework",
                    "started_at": "2026-10-01T11:00:00Z",
                    "duration_seconds": 4200,
                    "quiet_seconds": 1800,
                },
                {
                    "id": "weekly/3",
                    "automation": "weekly",
                    "started_at": "2026-10-01T12:09:00Z",
                    "duration_seconds": 60,
                    "quiet_seconds": null,
                },
            ],
            "items": [
                {
                    "number": 7,
                    "title": "Do the thing",
                    "state": "ready-to-claim",
                    "waiting_for": [],
                },
            ],
            "automations": [
                {
                    "name": "weekly",
                    "schedule": "scheduled",
                    "last_run": "2026-09-28T06:00:00Z",
                    "next_run": "2026-10-05T06:00:00Z",
                },
            ],
            "unparsed_automations": [],
        })
    );
}

#[test]
fn linewrights_pull_request_is_managed_with_its_own_waits_and_its_ticket_waits_for_it() {
    let pr = Pull {
        number: Number(43),
        title: "Do #7".into(),
        body: "Closes #7".into(),
        author: Login::new("factory"),
        created_at: at("2026-10-01T11:58:00Z"),
        head_ref: "7-do".into(),
        head_sha: "abc123".into(),
        state: PullState::Open { draft: true },
        closes: vec![Number(7)],
        pushes: vec![],
        mergeability: Mergeability::Unknown,
        rebaseability: Rebaseability::Unknown,
        checks: vec![Check {
            name: "test".into(),
            state: CheckState::Failed,
        }],
        comments: vec![],
        reviews: vec![],
        threads: vec![],
        labels: vec![],
        assignees: vec![],
        assignments: vec![Assignment::Unassigned {
            assignee: Login::new("factory"),
            actor: Login::new("maintainer"),
            at: at("2026-10-01T11:59:00Z"),
        }],
    };
    let unlabeled = Issue {
        labels: vec![],
        ..ready_issue()
    };

    assert_eq!(
        after_linewright(
            &views::view(vec![unlabeled], vec![pr]),
            "2026-10-01T12:00:30Z"
        ),
        [
            "#7: in review: #43 (draft, CI failing) — waiting for #43, its pull request, until it \
             is merged or closed — Do the thing",
            "#43: in review: #43 (draft, CI failing) — waiting for the maintainer: Linewright \
             is unassigned, until they assign it again — Do #7",
        ]
    );
}
