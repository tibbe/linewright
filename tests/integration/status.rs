use crate::common;

use chrono::TimeDelta;

use common::{
    assigning_linewright, at, closed_by, draining_since, implementing, in_flight,
    labeled_by_linewright, limited_until, linewright_letting_go, relabeled, sub_issue, ticket,
};
use linewright::automation::{AutomationFiles, LastScheduledRuns, Unparsed};
use linewright::domain::*;
use linewright::prompts::Readers;
use linewright::reconcile::{LiveRun, Memory, Subject, WorkKind};
use linewright::repo_files::{FileError, Unusable};
use linewright::runs::{Ending, Recorded, Started, StartedBy, State};
use std::num::NonZeroU64;

use linewright::status::{FailedPolls, Poll, Polling, Serving, Status, View, render, status};
use linewright::webhook::Received;

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

fn last_scheduled_runs(runs: &[(&str, &str)]) -> LastScheduledRuns {
    runs.iter()
        .map(|&(name, time)| (name.parse().unwrap(), at(time)))
        .collect()
}

fn automation_run(id: &str, by: StartedBy, started: &str) -> Recorded {
    Recorded::new(
        Started::new(id.parse().unwrap(), Some(by)).unwrap(),
        at(started),
        State::Ended {
            at: at(started) + TimeDelta::minutes(1),
            ending: Ending::Done,
        },
        vec![],
        None,
    )
}

fn serving() -> Serving {
    Serving {
        since: at("2026-10-01T11:00:00Z"),
        max_runs: 4,
    }
}

fn view(issues: Vec<Issue>) -> View {
    common::view(issues, vec![])
}

fn ready_issue() -> Issue {
    ticket(7, "Do the thing")
}

fn text(view: Option<&View>, now: &str) -> Vec<String> {
    text_with(None, view, now)
}

fn text_with(webhooks: Option<Received>, view: Option<&View>, now: &str) -> Vec<String> {
    text_of(webhooks, view, &[], now)
}

fn text_of(
    webhooks: Option<Received>,
    view: Option<&View>,
    recorded: &[Recorded],
    now: &str,
) -> Vec<String> {
    let answer = status(&serving(), webhooks, None, view, recorded, None, at(now));
    render(&answer, false).lines().map(str::to_string).collect()
}

fn after_linewright(view: &View, now: &str) -> Vec<String> {
    after_linewright_with(view, &[], now)
}

fn after_linewright_with(view: &View, recorded: &[Recorded], now: &str) -> Vec<String> {
    let lines = text_of(None, Some(view), recorded, now);
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
        in_flight: vec![in_flight("weekly/1", "2026-10-01T12:00:00Z")],
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

fn unusable(path: &str, error: &str) -> Unusable {
    Unusable {
        file: FileError {
            path: path.into(),
            error: error.into(),
        },
        holds: Readers::Every,
    }
}

fn with_unusable_files() -> View {
    View {
        memory: Memory {
            usage_limit: limited_until("2026-10-01T14:30:00Z").usage_limit,
            ..draining_since("2026-10-01T12:00:20Z")
        },
        unusable: vec![
            unusable(
                ".linewright/config.toml",
                "the setting `max-runs` is the operator's",
            ),
            unusable(
                ".linewright/prompts/implement.md",
                "the prompt has no `{{output}}`",
            ),
        ],
        ..view(vec![])
    }
}

#[test]
fn each_file_of_the_repositorys_linewright_cannot_use_is_shown_with_its_error_after_the_usage_limit()
 {
    let lines = text(Some(&with_unusable_files()), "2026-10-01T12:00:30Z");

    assert_eq!(
        lines[3..],
        [
            "usage limit: in force until 2026-10-01 14:30:00 UTC",
            "repository file Linewright cannot use: `.linewright/config.toml`: the setting \
             `max-runs` is the operator's",
            "repository file Linewright cannot use: `.linewright/prompts/implement.md`: \
             the prompt has no `{{output}}`",
            "drain: under way since 2026-10-01 12:00:20 UTC",
            "runs in flight: 0 of 4",
        ]
    );
}

#[test]
fn a_repository_whose_files_linewright_can_use_shows_none() {
    let lines = text(Some(&view(vec![])), "2026-10-01T12:00:30Z");

    assert!(
        !lines.iter().any(|line| line.contains("repository file")),
        "{lines:?}"
    );
}

#[test]
fn the_files_of_the_repositorys_linewright_cannot_use_are_in_the_json_and_a_client_reads_an_answer_without_them()
 {
    let answer = status(
        &serving(),
        None,
        None,
        Some(&with_unusable_files()),
        &[],
        None,
        at("2026-10-01T12:00:30Z"),
    );
    let mut json = serde_json::to_value(&answer).unwrap();

    assert_eq!(
        json["linewright"]["unusable_files"],
        serde_json::json!([
            {
                "path": ".linewright/config.toml",
                "error": "the setting `max-runs` is the operator's",
            },
            {
                "path": ".linewright/prompts/implement.md",
                "error": "the prompt has no `{{output}}`",
            },
        ])
    );
    assert_eq!(
        serde_json::from_value::<Status>(json.clone()).unwrap(),
        answer
    );
    json["linewright"]
        .as_object_mut()
        .unwrap()
        .remove("unusable_files");
    let read: Status = serde_json::from_value(json).unwrap();
    assert_eq!(read.linewright.unusable_files, []);
}

#[test]
fn a_drain_under_way_is_in_the_json_and_is_null_without_one() {
    let draining = View {
        memory: draining_since("2026-10-01T12:00:20Z"),
        ..view(vec![])
    };
    let now = at("2026-10-01T12:00:30Z");

    let shown = serde_json::to_value(status(
        &serving(),
        None,
        None,
        Some(&draining),
        &[],
        None,
        now,
    ))
    .unwrap();
    let none = serde_json::to_value(status(
        &serving(),
        None,
        None,
        Some(&view(vec![])),
        &[],
        None,
        now,
    ))
    .unwrap();

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

    let answer = status(&serving(), None, None, Some(&draining), &[], deadline, now);
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
        None,
        Some(&view),
        &[],
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
        last_refused: Some(at("2026-10-01T11:58:00Z")),
    };

    let lines = text_with(Some(webhooks), Some(&view(vec![])), "2026-10-01T12:00:30Z");

    assert_eq!(
        lines[4..],
        [
            "last webhook delivery accepted: 2026-10-01 11:59:00 UTC",
            "webhook deliveries refused: 2",
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
        [
            "last webhook delivery accepted: none yet",
            "webhook deliveries refused: 0"
        ]
    );
}

#[test]
fn each_run_in_flight_follows_the_linewright_section_with_how_long_it_has_run() {
    let view = View {
        in_flight: vec![
            in_flight("43/triage-1", "2026-10-01T12:05:00Z"),
            in_flight("42/rework-2", "2026-10-01T11:00:00Z"),
            in_flight("weekly/3", "2026-10-01T12:00:00Z"),
        ],
        ..view(vec![ready_issue()])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:05:00Z"),
        [
            "42/rework-2: #42 rework, running for 1h 5m",
            "weekly/3: automation weekly, running for 5m",
            "43/triage-1: #43 triage, running for 0s",
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
        in_flight: vec![in_flight("9/implement-1", "2026-10-01T12:00:00Z")],
        ..view(vec![ready_issue(), busy])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:00:30Z"),
        [
            "9/implement-1: #9 implement, running for 30s",
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
    let with_the_maintainer = labeled_by_linewright(Issue {
        number: Number(10),
        title: "It broke".into(),
        labels: vec!["ready-for-human".into()],
        ..ready_issue()
    });
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
        last_scheduled_runs: last_scheduled_runs(&[("weekly", "2026-09-28T06:00:02Z")]),
        ..view(vec![])
    };
    let recorded = [automation_run(
        "weekly/3",
        StartedBy::Schedule,
        "2026-09-28T06:00:02Z",
    )];

    assert_eq!(
        after_linewright_with(&view, &recorded, "2026-10-01T12:00:30Z"),
        [
            "automation weekly last run: weekly/3, started 2026-09-28 06:00:02 UTC",
            "automation weekly next run: 2026-10-05 06:00:00 UTC",
        ]
    );
}

#[test]
fn a_newer_run_by_hand_is_the_last_run_and_leaves_the_next_run_as_it_was() {
    let view = View {
        automations: files(&[("weekly", "0 6 * * 1")], &[]),
        last_scheduled_runs: last_scheduled_runs(&[("weekly", "2026-09-28T06:00:02Z")]),
        ..view(vec![])
    };
    let scheduled = automation_run("weekly/3", StartedBy::Schedule, "2026-09-28T06:00:02Z");
    let by_hand = automation_run("weekly/4", StartedBy::Hand, "2026-09-30T09:15:00Z");

    assert_eq!(
        after_linewright_with(&view, &[scheduled, by_hand], "2026-10-01T12:00:30Z"),
        [
            "automation weekly last run: weekly/4 by hand, started 2026-09-30 09:15:00 UTC",
            "automation weekly next run: 2026-10-05 06:00:00 UTC",
        ]
    );
}

#[test]
fn a_newer_scheduled_run_is_the_last_run_with_no_mark() {
    let view = View {
        automations: files(&[("weekly", "0 6 * * 1")], &[]),
        last_scheduled_runs: last_scheduled_runs(&[("weekly", "2026-09-28T06:00:02Z")]),
        ..view(vec![])
    };
    let by_hand = automation_run("weekly/3", StartedBy::Hand, "2026-09-25T09:15:00Z");
    let scheduled = automation_run("weekly/4", StartedBy::Schedule, "2026-09-28T06:00:02Z");

    assert_eq!(
        after_linewright_with(&view, &[scheduled, by_hand], "2026-10-01T12:00:30Z")[0],
        "automation weekly last run: weekly/4, started 2026-09-28 06:00:02 UTC"
    );
}

#[test]
fn only_the_automations_own_runs_count_toward_its_last_run() {
    let view = View {
        automations: files(&[("weekly", "0 6 * * 1")], &[]),
        last_scheduled_runs: last_scheduled_runs(&[("weekly", "2026-09-28T06:00:02Z")]),
        ..view(vec![])
    };
    let other = automation_run("daily/9", StartedBy::Schedule, "2026-10-01T06:00:00Z");

    assert_eq!(
        after_linewright_with(&view, &[other], "2026-10-01T12:00:30Z")[0],
        "automation weekly last run: none yet"
    );
}

#[test]
fn an_automation_seen_but_never_run_has_no_last_run_yet_and_its_next_run_as_before() {
    let view = View {
        automations: files(&[("weekly", "0 6 * * 1")], &[]),
        last_scheduled_runs: last_scheduled_runs(&[("weekly", "2026-09-28T06:00:02Z")]),
        ..view(vec![])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:00:30Z"),
        [
            "automation weekly last run: none yet",
            "automation weekly next run: 2026-10-05 06:00:00 UTC",
        ]
    );
}

#[test]
fn an_automation_whose_schedule_ticked_since_its_last_scheduled_run_is_due_now() {
    let view = View {
        automations: files(&[("hourly", "0 * * * *")], &[]),
        last_scheduled_runs: last_scheduled_runs(&[("hourly", "2026-10-01T11:30:00Z")]),
        ..view(vec![])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:10:00Z"),
        [
            "automation hourly last run: none yet",
            "automation hourly next run: due now",
        ]
    );
}

#[test]
fn an_automation_whose_schedule_never_ticks_again_has_no_next_run() {
    let view = View {
        automations: files(&[("leap", "0 0 30 2 *")], &[]),
        last_scheduled_runs: last_scheduled_runs(&[("leap", "2026-09-28T06:00:00Z")]),
        ..view(vec![])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:10:00Z"),
        [
            "automation leap last run: none yet",
            "automation leap next run: none, its schedule ticks no more",
        ]
    );
}

#[test]
fn an_automation_no_poll_has_seen_yet_has_no_next_run_until_one_does() {
    let view = View {
        automations: files(&[("hourly", "0 * * * *")], &[]),
        ..view(vec![])
    };

    assert_eq!(
        after_linewright(&view, "2026-10-01T12:10:00Z"),
        [
            "automation hourly last run: none yet",
            "automation hourly next run: not yet counted, its schedule starts when a poll first sees it",
        ]
    );
}

#[test]
fn the_items_come_before_the_automations_by_name_then_the_files_that_do_not_parse() {
    let view = View {
        automations: files(
            &[("weekly", "0 6 * * 1"), ("hourly", "0 * * * *")],
            &[(".linewright/automations/broken.md", "no front matter")],
        ),
        last_scheduled_runs: last_scheduled_runs(&[
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
            "automation hourly last run: none yet",
            "automation hourly next run: 2026-10-01 13:00:00 UTC",
            "automation weekly last run: none yet",
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
        None,
        Some(&view),
        &[],
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
        last_refused: Some(at("2026-10-01T11:58:00Z")),
    };
    let view = View {
        automations: files(&[("weekly", "0 6 * * 1")], &[("x.md", "bad")]),
        last_scheduled_runs: last_scheduled_runs(&[("weekly", "2026-09-28T06:00:00Z")]),
        in_flight: vec![
            in_flight("42/rework-2", "2026-10-01T11:00:00Z"),
            in_flight("weekly/3", "2026-10-01T12:09:00Z"),
        ],
        ..view(vec![ready_issue()])
    };
    let answer = status(
        &serving(),
        Some(webhooks),
        None,
        Some(&view),
        &[automation_run(
            "weekly/2",
            StartedBy::Schedule,
            "2026-09-28T06:00:00Z",
        )],
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
        last_scheduled_runs: last_scheduled_runs(&[("weekly", "2026-09-28T06:00:00Z")]),
        live: vec![LiveRun {
            subject: Subject::Issue(Number(42)),
            kind: WorkKind::Rework,
        }],
        in_flight: vec![
            in_flight("42/rework-2", "2026-10-01T11:00:00Z"),
            in_flight("weekly/3", "2026-10-01T12:09:00Z"),
        ],
        ..view(vec![ready_issue()])
    };
    let recorded = [
        automation_run("weekly/2", StartedBy::Schedule, "2026-09-28T06:00:00Z"),
        automation_run("weekly/3", StartedBy::Hand, "2026-09-30T09:15:00Z"),
    ];
    let answer = status(
        &serving(),
        None,
        None,
        Some(&view),
        &recorded,
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
                "failed_polls": null,
                "usage_limit_resets_at": null,
                "unusable_files": [],
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
                },
                {
                    "id": "weekly/3",
                    "automation": "weekly",
                    "started_at": "2026-10-01T12:09:00Z",
                    "duration_seconds": 60,
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
                    "next_run": "2026-10-05T06:00:00Z",
                    "last_run": "2026-09-30T09:15:00Z",
                    "last_run_id": "weekly/3",
                    "last_run_started_by": "hand",
                },
            ],
            "unparsed_automations": [],
        })
    );
}

#[test]
fn the_json_says_when_a_webhook_delivery_was_last_refused_and_reads_without_it() {
    let webhooks = Received {
        last_accepted: Some(at("2026-10-01T11:59:00Z")),
        refused: 1,
        last_refused: Some(at("2026-10-01T12:05:00Z")),
    };
    let answer = status(
        &serving(),
        Some(webhooks),
        None,
        None,
        &[],
        None,
        at("2026-10-01T12:10:00Z"),
    );

    let mut json = serde_json::to_value(&answer).unwrap();
    assert_eq!(
        json["linewright"]["webhooks"],
        serde_json::json!({
            "last_accepted": "2026-10-01T11:59:00Z",
            "refused": 1,
            "last_refused": "2026-10-01T12:05:00Z",
        })
    );
    json["linewright"]["webhooks"]
        .as_object_mut()
        .unwrap()
        .remove("last_refused");
    let read: Status = serde_json::from_value(json).unwrap();
    assert_eq!(read.linewright.webhooks.unwrap().last_refused, None);
}

#[test]
fn before_an_automations_first_run_its_json_has_no_last_run() {
    let view = View {
        automations: files(&[("weekly", "0 6 * * 1")], &[]),
        last_scheduled_runs: last_scheduled_runs(&[("weekly", "2026-09-28T06:00:00Z")]),
        ..view(vec![])
    };
    let answer = status(
        &serving(),
        None,
        None,
        Some(&view),
        &[],
        None,
        at("2026-10-01T12:10:00Z"),
    );

    assert_eq!(
        serde_json::to_value(&answer).unwrap()["automations"],
        serde_json::json!([
            {
                "name": "weekly",
                "schedule": "scheduled",
                "next_run": "2026-10-05T06:00:00Z",
            },
        ])
    );
    let json = serde_json::to_string(&answer).unwrap();
    assert_eq!(serde_json::from_str::<Status>(&json).unwrap(), answer);
}

#[test]
fn linewrights_pull_request_is_managed_with_its_own_waits_and_its_ticket_waits_for_it() {
    let pr = Pull {
        number: Number(43),
        title: "Do #7".into(),
        body: "Closes #7".into(),
        author: Login::new("factory"),
        created_at: at("2026-10-01T11:58:00Z"),
        updated_at: at("2026-10-01T11:58:00Z"),
        base_ref: "main".into(),
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
        labelings: vec![],
        assignments: vec![Assignment::Unassigned {
            assignee: Login::new("factory"),
            actor: Login::new("maintainer"),
            at: at("2026-10-01T11:59:00Z"),
        }],
        review_decision: None,
    };
    let unlabeled = Issue {
        labels: vec![],
        ..ready_issue()
    };

    assert_eq!(
        after_linewright(
            &common::view(vec![unlabeled], vec![pr]),
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

/// The ready issue, labeled `ready-for-agent` but not the required label
/// `linewright`, and so not marked for Linewright.
fn unmarked_view(issue: Issue) -> View {
    let mut view = view(vec![issue]);
    view.policy.require_label = Some("linewright".into());
    view
}

fn managed_numbers(view: &View) -> Vec<Number> {
    status(
        &serving(),
        None,
        None,
        Some(view),
        &[],
        None,
        at("2026-10-01T12:00:30Z"),
    )
    .items
    .iter()
    .map(|item| item.number)
    .collect()
}

#[test]
fn with_a_required_label_an_issue_without_it_that_linewright_has_not_worked_is_not_managed() {
    assert_eq!(managed_numbers(&unmarked_view(ready_issue())), []);
}

#[test]
fn without_a_required_label_an_issue_asking_for_a_step_is_managed() {
    assert_eq!(managed_numbers(&view(vec![ready_issue()])), [Number(7)]);
}

#[test]
fn an_issue_without_the_required_label_that_linewright_has_worked_is_managed() {
    let handed_back = Issue {
        labels: vec!["ready-for-agent".into(), label::STALLED.into()],
        events: vec![
            assigning_linewright("2026-10-01T10:00:00Z"),
            linewright_letting_go("2026-10-01T11:00:00Z"),
        ],
        ..ready_issue()
    };
    let assigned = Issue {
        assignees: vec![Login::new("factory")],
        ..ready_issue()
    };
    let opened = Issue {
        author: Login::new("factory"),
        ..ready_issue()
    };
    let closed_by_its_own = closed_by(ready_issue(), 43, "factory");

    for worked in [handed_back, assigned, opened, closed_by_its_own] {
        assert_eq!(managed_numbers(&unmarked_view(worked)), [Number(7)]);
    }
}

#[test]
fn an_issue_without_the_required_label_that_someone_elses_pull_request_closes_is_not_managed() {
    let closed = closed_by(ready_issue(), 43, "alice");

    assert_eq!(managed_numbers(&unmarked_view(closed)), []);
}

fn failed(previous: Option<FailedPolls>, time: &str, error: &str) -> Option<FailedPolls> {
    FailedPolls::after(
        previous,
        Poll::Failed {
            at: at(time),
            error: error.into(),
        },
    )
}

#[test]
fn each_failed_poll_counts_from_the_first_and_keeps_the_latest_error() {
    let once = failed(None, "2026-10-01T12:00:00Z", "GitHub is down");
    let twice = failed(once, "2026-10-01T12:01:00Z", "bad credentials");

    assert_eq!(
        twice,
        Some(FailedPolls {
            count: NonZeroU64::new(2).unwrap(),
            since: at("2026-10-01T12:00:00Z"),
            last_at: at("2026-10-01T12:01:00Z"),
            last_error: "bad credentials".into(),
        })
    );
}

#[test]
fn a_poll_that_succeeds_clears_the_failed_polls() {
    let once = failed(None, "2026-10-01T12:00:00Z", "GitHub is down");

    assert_eq!(FailedPolls::after(once, Poll::Succeeded), None);
}

/// Polling after two failed polls, the next at 12:02.
fn two_failed() -> Polling {
    let once = failed(None, "2026-10-01T12:00:00Z", "GitHub is down");
    Polling {
        next: at("2026-10-01T12:02:00Z"),
        failed: failed(once, "2026-10-01T12:01:00Z", "bad credentials"),
    }
}

#[test]
fn polls_failed_since_the_last_that_succeeded_show_after_the_next_poll() {
    let answer = status(
        &serving(),
        None,
        Some(&two_failed()),
        Some(&view(vec![])),
        &[],
        None,
        at("2026-10-01T12:01:30Z"),
    );

    let text = render(&answer, false);

    assert_eq!(
        text.lines().nth(3),
        Some(
            "polls failed: 2 since 2026-10-01 12:00:00 UTC, the last at \
             2026-10-01 12:01:00 UTC: bad credentials"
        ),
        "{text}"
    );
}

#[test]
fn polls_failed_before_the_first_that_succeeded_show_with_the_next_poll() {
    let polling = Polling {
        next: at("2026-10-01T11:01:00Z"),
        failed: failed(None, "2026-10-01T11:00:00Z", "bad credentials"),
    };

    let answer = status(
        &serving(),
        None,
        Some(&polling),
        None,
        &[],
        None,
        at("2026-10-01T11:00:05Z"),
    );

    assert_eq!(
        render(&answer, false).lines().collect::<Vec<_>>(),
        [
            "serving since: 2026-10-01 11:00:00 UTC",
            "last poll: none yet",
            "next poll: 2026-10-01 11:01:00 UTC",
            "polls failed: 1 at 2026-10-01 11:00:00 UTC: bad credentials",
            "runs in flight: 0 of 4",
        ]
    );
}

#[test]
fn polls_failed_are_in_the_json_and_a_client_reads_an_answer_without_them() {
    let answer = status(
        &serving(),
        None,
        Some(&two_failed()),
        Some(&view(vec![])),
        &[],
        None,
        at("2026-10-01T12:01:30Z"),
    );
    let mut json = serde_json::to_value(&answer).unwrap();

    assert_eq!(
        json["linewright"]["failed_polls"],
        serde_json::json!({
            "count": 2,
            "since": "2026-10-01T12:00:00Z",
            "last_at": "2026-10-01T12:01:00Z",
            "last_error": "bad credentials",
        })
    );
    json["linewright"]
        .as_object_mut()
        .unwrap()
        .remove("failed_polls");
    let read: Status = serde_json::from_value(json).unwrap();
    assert_eq!(read.linewright.failed_polls, None);
}

#[test]
fn the_open_managed_items_are_listed_but_those_in_done() {
    let merged_into = Issue {
        closed_by: vec![ClosingPull {
            number: Number(30),
            author: Login::new("factory"),
            state: PullState::Merged {
                at: at("2026-10-01T10:00:00Z"),
            },
            updated_at: at("2026-10-01T10:00:00Z"),
            base_ref: "main".into(),
        }],
        ..ticket(1, "Merged into")
    };
    let mut view = view(vec![merged_into, ticket(2, "Ready")]);
    let snapshot = std::sync::Arc::make_mut(&mut view.snapshot);
    snapshot.peoples_pulls = vec![common::open_pull(40, "alice", &[], &["factory"])];
    snapshot.merged = vec![common::merged_pull(30, "factory", &[1])];

    assert_eq!(managed_numbers(&view), [Number(2), Number(40)]);
}
