use crate::views;

use linewright::domain::*;
use linewright::explain::{Answer, Explanation, explain, explain_read, render, render_answer};
use linewright::runs::{Ending, Recorded, State};
use linewright::status::View;
use linewright::store::AnyItem;
use views::{
    at, closed_by, draining_since, implementing, in_flight, limited_until, relabeled, ticket, view,
};

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
        labels: vec![],
        assignees: vec![Login::new("factory")],
        assignments: vec![],
    }
}

const NOW: &str = "2026-10-01T12:00:30Z";

fn text(view: &View, number: u64) -> Vec<String> {
    let answer = explain(view, &[], Number(number), at(NOW)).expect("a tracked item");
    lines_of(&render(&answer, false))
}

fn lines_of(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

fn numbers(numbers: &[u64]) -> Vec<Number> {
    numbers.iter().copied().map(Number).collect()
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
fn a_ticket_someone_elses_open_pull_request_closes_is_in_review_names_it_as_a_child_and_waits_for_it()
 {
    let ticket = closed_by(ticket(42, "Add invoices"), 43, "alice");

    assert_eq!(
        text(&view(vec![ticket], vec![]), 42),
        [
            "#42: Add invoices",
            "child: #43, a pull request that closes it",
            "state: in review: #43, opened by alice",
            "last read: 2026-10-01 12:00:00 UTC",
            "waiting for: #43, which closes it",
        ]
    );
}

#[test]
fn the_json_names_the_pull_request_that_closes_it() {
    let ticket = closed_by(ticket(42, "Add invoices"), 43, "alice");
    let answer = explain(&view(vec![ticket], vec![]), &[], Number(42), at(NOW)).unwrap();

    assert_eq!(
        serde_json::to_value(&answer).unwrap()["waiting_for"],
        serde_json::json!([{"wait": "closing-pull-request", "pull_request": 43}])
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
fn while_draining_work_held_back_waits_for_the_drain() {
    let draining = View {
        memory: draining_since("2026-10-01T11:59:00Z"),
        ..view(vec![ticket(42, "Add invoices")], vec![])
    };

    assert_eq!(
        text(&draining, 42)[3],
        "waiting for: the drain, until `serve` restarts"
    );
}

#[test]
fn an_item_in_review_waits_for_its_pull_request() {
    let claimed = Issue {
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    let view = view(
        vec![closed_by(claimed, 43, "factory")],
        vec![draft(43, 42, &[CheckState::Pending])],
    );

    assert_eq!(
        text(&view, 42)[1..],
        [
            "child: #43, a pull request that closes it",
            "state: in review: #43 (draft, CI running)",
            "last read: 2026-10-01 12:00:00 UTC",
            "waiting for: #43, its pull request, until it is merged or closed",
        ]
    );
}

#[test]
fn an_item_with_the_maintainer_waits_for_their_label() {
    let handed_back = Issue {
        labels: vec!["ready-for-human".into()],
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        text(&view(vec![handed_back], vec![]), 42)[1..],
        [
            "state: with the maintainer",
            "last read: 2026-10-01 12:00:00 UTC",
            "waiting for: the maintainer: to label it `ready-for-agent` or `needs-triage`",
        ]
    );
}

#[test]
fn an_item_carrying_two_status_labels_is_explained_by_the_newer() {
    let triaged = relabeled(
        ticket(42, "Add invoices"),
        "needs-triage",
        "ready-for-agent",
    );

    assert_eq!(
        text(&view(vec![triaged], vec![]), 42)[1..],
        [
            "state: ready to claim",
            "last read: 2026-10-01 12:00:00 UTC",
            "waiting for: nothing: Linewright acts on it at its next poll",
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
fn linewrights_pull_request_is_explained_as_the_issue_in_review_with_it() {
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
            "parent: #42, an issue it closes",
            "state: in review: #43 (draft, CI running)",
            "last read: 2026-10-01 12:00:20 UTC",
            "waiting for: CI on #43",
        ]
    );
}

fn recorded(id: &str, started: &str, state: State) -> Recorded {
    Recorded {
        id: id.parse().unwrap(),
        started_at: at(started),
        state,
        sessions: vec![],
        answered_until: None,
    }
}

fn ended(at_: &str, ending: Ending) -> State {
    State::Ended {
        at: at(at_),
        ending,
    }
}

fn runs_of_42() -> Vec<Recorded> {
    vec![
        recorded(
            "42/triage-1",
            "2026-10-01T09:00:00Z",
            ended("2026-10-01T09:05:00Z", Ending::Done),
        ),
        recorded("42/implement-1", "2026-10-01T10:00:00Z", State::Interrupted),
        recorded("42/implement-2", "2026-10-01T11:30:00Z", State::InFlight),
        recorded(
            "7/triage-1",
            "2026-10-01T11:00:00Z",
            ended("2026-10-01T11:01:00Z", Ending::Done),
        ),
    ]
}

fn implementing_42(pulls: Vec<Pull>) -> View {
    let claimed = Issue {
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    View {
        live: vec![implementing(42)],
        in_flight: vec![in_flight(
            "42/implement-2",
            "2026-10-01T11:30:00Z",
            Some("2026-10-01T11:50:00Z"),
        )],
        ..view(vec![claimed], pulls)
    }
}

fn run_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| {
            ["run in flight: ", "past run: ", "left out: "]
                .iter()
                .any(|label| line.starts_with(label))
        })
        .collect()
}

#[test]
fn explain_shows_the_run_in_flight_with_its_quiet_time_then_the_past_runs_as_runs_lists_them() {
    let answer = explain(&implementing_42(vec![]), &runs_of_42(), Number(42), at(NOW)).unwrap();

    assert_eq!(
        run_lines(&render(&answer, false)),
        [
            "run in flight: 42/implement-2: #42 implement, running for 30m 30s, quiet for 10m 30s",
            "past run: 42/implement-1: interrupted — #42 implement, started 2026-10-01 10:00:00 UTC",
            "past run: 42/triage-1: done — #42 triage, started 2026-10-01 09:00:00 UTC, ran 5m",
        ]
    );
}

fn five_past_runs_of_42() -> Vec<Recorded> {
    let mut runs: Vec<Recorded> = (1..=5)
        .map(|n| {
            recorded(
                &format!("42/implement-{n}"),
                &format!("2026-10-01T{:02}:00:00Z", 6 + n),
                ended(&format!("2026-10-01T{:02}:01:00Z", 6 + n), Ending::Failed),
            )
        })
        .collect();
    runs.push(recorded(
        "42/implement-6",
        "2026-10-01T11:30:00Z",
        State::InFlight,
    ));
    runs
}

fn implementing_42_again(pulls: Vec<Pull>) -> View {
    View {
        in_flight: vec![in_flight(
            "42/implement-6",
            "2026-10-01T11:30:00Z",
            Some("2026-10-01T11:50:00Z"),
        )],
        ..implementing_42(pulls)
    }
}

#[test]
fn explain_shows_the_latest_three_past_runs_and_points_to_runs_for_the_rest() {
    let answer = explain(
        &implementing_42_again(vec![]),
        &five_past_runs_of_42(),
        Number(42),
        at(NOW),
    )
    .unwrap();

    assert_eq!(
        run_lines(&render(&answer, false)),
        [
            "run in flight: 42/implement-6: #42 implement, running for 30m 30s, quiet for 10m 30s",
            "past run: 42/implement-5: failed — #42 implement, started 2026-10-01 11:00:00 UTC, ran 1m",
            "past run: 42/implement-4: failed — #42 implement, started 2026-10-01 10:00:00 UTC, ran 1m",
            "past run: 42/implement-3: failed — #42 implement, started 2026-10-01 09:00:00 UTC, ran 1m",
            "left out: 2 earlier runs; show them with lw runs 42",
        ]
    );
}

#[test]
fn explain_shows_three_past_runs_and_leaves_none_out() {
    let answer = explain(
        &implementing_42_again(vec![]),
        &five_past_runs_of_42()[2..],
        Number(42),
        at(NOW),
    )
    .unwrap();

    assert_eq!(
        run_lines(&render(&answer, false)),
        [
            "run in flight: 42/implement-6: #42 implement, running for 30m 30s, quiet for 10m 30s",
            "past run: 42/implement-5: failed — #42 implement, started 2026-10-01 11:00:00 UTC, ran 1m",
            "past run: 42/implement-4: failed — #42 implement, started 2026-10-01 10:00:00 UTC, ran 1m",
            "past run: 42/implement-3: failed — #42 implement, started 2026-10-01 09:00:00 UTC, ran 1m",
        ]
    );
}

#[test]
fn linewrights_pull_request_points_to_runs_for_its_own_past_runs() {
    let view = implementing_42_again(vec![draft(43, 42, &[CheckState::Pending])]);
    let mut runs = five_past_runs_of_42();
    runs.extend((1..=5).map(|n| {
        recorded(
            &format!("43/rework-{n}"),
            &format!("2026-10-01T{:02}:30:00Z", 6 + n),
            ended(&format!("2026-10-01T{:02}:31:00Z", 6 + n), Ending::Done),
        )
    }));
    let answer = explain(&view, &runs, Number(43), at(NOW)).unwrap();

    assert_eq!(
        run_lines(&render(&answer, false)),
        [
            "past run: 43/rework-5: done — #43 rework, started 2026-10-01 11:30:00 UTC, ran 1m",
            "past run: 43/rework-4: done — #43 rework, started 2026-10-01 10:30:00 UTC, ran 1m",
            "past run: 43/rework-3: done — #43 rework, started 2026-10-01 09:30:00 UTC, ran 1m",
            "left out: 2 earlier runs; show them with lw runs 43",
        ]
    );
}

#[test]
fn an_item_left_alone_points_to_runs_for_the_past_runs_left_out() {
    let closed = Issue {
        open: false,
        ..ticket(42, "Add invoices")
    };
    let answer = explain_read(
        &view(vec![], vec![]),
        &five_past_runs_of_42()[1..5],
        &AnyItem::LeftOutIssue(Box::new(closed)),
        at(NOW),
    );

    assert_eq!(
        run_lines(&render_answer(&answer, false)),
        [
            "past run: 42/implement-5: failed — #42 implement, started 2026-10-01 11:00:00 UTC, ran 1m",
            "past run: 42/implement-4: failed — #42 implement, started 2026-10-01 10:00:00 UTC, ran 1m",
            "past run: 42/implement-3: failed — #42 implement, started 2026-10-01 09:00:00 UTC, ran 1m",
            "left out: 1 earlier run; show them with lw runs 42",
        ]
    );
}

#[test]
fn the_json_names_the_latest_three_past_runs_and_how_many_it_left_out() {
    let answer = Answer::Tracked(
        explain(
            &implementing_42_again(vec![]),
            &five_past_runs_of_42(),
            Number(42),
            at(NOW),
        )
        .unwrap(),
    );

    let json = serde_json::to_value(&answer).unwrap();

    let ids: Vec<&serde_json::Value> = json["past_runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| &run["id"])
        .collect();
    assert_eq!(ids, ["42/implement-5", "42/implement-4", "42/implement-3"]);
    assert_eq!(json["past_runs_left_out"], 2);
    assert_eq!(json["runs_of"], 42);
    assert_eq!(json["run_in_flight"]["id"], "42/implement-6");
    assert_eq!(serde_json::from_value::<Answer>(json).unwrap(), answer);
}

#[test]
fn json_that_leaves_past_runs_out_before_showing_three_does_not_read_as_an_answer() {
    let answer = Answer::Tracked(
        explain(
            &implementing_42_again(vec![]),
            &five_past_runs_of_42(),
            Number(42),
            at(NOW),
        )
        .unwrap(),
    );
    let mut json = serde_json::to_value(&answer).unwrap();
    json["past_runs"].as_array_mut().unwrap().truncate(1);

    assert!(serde_json::from_value::<Answer>(json).is_err());
}

#[test]
fn a_run_that_has_just_ended_shows_once_as_still_in_flight() {
    let mut runs = runs_of_42();
    runs[2].state = ended("2026-10-01T12:00:29Z", Ending::Done);

    let answer = explain(&implementing_42(vec![]), &runs, Number(42), at(NOW)).unwrap();

    assert_eq!(
        run_lines(&render(&answer, false))
            .iter()
            .filter(|line| line.contains("42/implement-2"))
            .count(),
        1
    );
}

#[test]
fn an_item_with_no_runs_shows_none() {
    let answer = explain(
        &view(vec![ticket(42, "Add invoices")], vec![]),
        &runs_of_42()[3..],
        Number(42),
        at(NOW),
    )
    .unwrap();

    assert_eq!(run_lines(&render(&answer, false)), Vec::<&str>::new());
}

fn runs_of_42_and_43() -> Vec<Recorded> {
    let mut runs = runs_of_42();
    runs.push(recorded(
        "43/rework-1",
        "2026-10-01T11:10:00Z",
        ended("2026-10-01T11:15:00Z", Ending::Done),
    ));
    runs
}

/// The IDs of the runs `answer` shows, as its JSON names them, the run in
/// flight first.
fn shown(answer: &Answer) -> Vec<String> {
    let json = serde_json::to_value(answer).unwrap();
    let in_flight = json["run_in_flight"]["id"].as_str().map(str::to_string);
    let past = json["past_runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["id"].as_str().unwrap().to_string());
    in_flight.into_iter().chain(past).collect()
}

fn assert_shows_the_runs_of_43(answer: &Answer) {
    assert_eq!(
        run_lines(&render_answer(answer, false)),
        ["past run: 43/rework-1: done — #43 rework, started 2026-10-01 11:10:00 UTC, ran 5m"]
    );
    assert_eq!(shown(answer), ["43/rework-1"]);
}

fn assert_shows_no_runs(answer: &Answer) {
    assert_eq!(run_lines(&render_answer(answer, false)), Vec::<&str>::new());
    assert_eq!(shown(answer), Vec::<String>::new());
}

#[test]
fn linewrights_merged_pull_request_shows_its_own_runs() {
    let merged = MergedPull {
        number: Number(43),
        title: "Do #42".into(),
        author: Login::new("factory"),
        ..merged(&[])
    };
    let answer = explain_read(
        &implementing_42(vec![]),
        &runs_of_42_and_43(),
        &AnyItem::Merged {
            pull: Box::new(merged),
            tracking_issues: vec![],
        },
        at(NOW),
    );

    assert_shows_the_runs_of_43(&answer);
}

#[test]
fn linewrights_closed_pull_request_shows_its_own_runs() {
    let closed = Pull {
        state: PullState::Closed {
            at: at("2026-10-01T11:00:00Z"),
        },
        ..draft(43, 42, &[])
    };
    let answer = explain_read(
        &implementing_42(vec![]),
        &runs_of_42_and_43(),
        &AnyItem::Pull(Box::new(closed)),
        at(NOW),
    );

    assert_shows_the_runs_of_43(&answer);
}

#[test]
fn linewrights_superseded_pull_request_shows_its_own_runs() {
    let older = draft(43, 42, &[]);
    let view = implementing_42(vec![older.clone(), draft(44, 42, &[])]);
    let answer = explain_read(
        &view,
        &runs_of_42_and_43(),
        &AnyItem::Pull(Box::new(older)),
        at(NOW),
    );

    assert_shows_the_runs_of_43(&answer);
}

#[test]
fn linewrights_pull_request_whose_issue_was_handed_back_shows_its_own_runs() {
    let handed_back = Issue {
        labels: vec!["ready-for-human".into()],
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    let open = draft(43, 42, &[]);
    let view = View {
        in_flight: implementing_42(vec![]).in_flight,
        ..view(vec![handed_back], vec![open.clone()])
    };
    let answer = explain_read(
        &view,
        &runs_of_42_and_43(),
        &AnyItem::Pull(Box::new(open)),
        at(NOW),
    );

    assert_shows_the_runs_of_43(&answer);
}

#[test]
fn linewrights_pull_request_in_review_shows_its_own_runs_as_runs_lists_them() {
    let view = implementing_42(vec![draft(43, 42, &[CheckState::Pending])]);
    let answer =
        Answer::Tracked(explain(&view, &runs_of_42_and_43(), Number(43), at(NOW)).unwrap());

    assert_shows_the_runs_of_43(&answer);
}

#[test]
fn linewrights_pull_request_shows_none_of_the_runs_of_the_issue_it_closes() {
    let view = implementing_42(vec![draft(43, 42, &[CheckState::Pending])]);
    let runs = runs_of_42();
    let answer = Answer::Tracked(explain(&view, &runs, Number(43), at(NOW)).unwrap());

    assert_shows_no_runs(&answer);
}

#[test]
fn an_issue_shows_none_of_the_runs_of_the_pull_request_that_closes_it() {
    let view = implementing_42(vec![draft(43, 42, &[CheckState::Pending])]);
    let answer = explain(&view, &runs_of_42_and_43(), Number(42), at(NOW)).unwrap();

    assert_eq!(
        run_lines(&render(&answer, false)),
        [
            "run in flight: 42/implement-2: #42 implement, running for 30m 30s, quiet for 10m 30s",
            "past run: 42/implement-1: interrupted — #42 implement, started 2026-10-01 10:00:00 UTC",
            "past run: 42/triage-1: done — #42 triage, started 2026-10-01 09:00:00 UTC, ran 5m",
        ]
    );
}

#[test]
fn a_pull_request_someone_else_opened_shows_no_runs() {
    let answer = explain_read(
        &implementing_42(vec![]),
        &runs_of_42(),
        &AnyItem::Pull(Box::new(someones_pull(43))),
        at(NOW),
    );

    assert_shows_no_runs(&answer);
}

#[test]
fn linewrights_pull_request_closing_no_issue_shows_no_runs() {
    let for_nothing = Pull {
        closes: vec![],
        ..draft(43, 42, &[])
    };
    let answer = explain_read(
        &implementing_42(vec![]),
        &runs_of_42(),
        &AnyItem::Pull(Box::new(for_nothing)),
        at(NOW),
    );

    assert_shows_no_runs(&answer);
}

#[test]
fn an_item_left_alone_shows_its_past_runs_too() {
    let closed = Issue {
        open: false,
        ..ticket(42, "Add invoices")
    };
    let answer = explain_read(
        &view(vec![], vec![]),
        &runs_of_42()[..2],
        &AnyItem::LeftOutIssue(Box::new(closed)),
        at(NOW),
    );

    assert_eq!(
        run_lines(&render_answer(&answer, false)),
        [
            "past run: 42/implement-1: interrupted — #42 implement, started 2026-10-01 10:00:00 UTC",
            "past run: 42/triage-1: done — #42 triage, started 2026-10-01 09:00:00 UTC, ran 5m",
        ]
    );
}

#[test]
fn the_json_names_the_run_in_flight_and_the_past_runs() {
    let answer = Answer::Tracked(
        explain(&implementing_42(vec![]), &runs_of_42(), Number(42), at(NOW)).unwrap(),
    );

    let json = serde_json::to_value(&answer).unwrap();

    assert_eq!(
        json["run_in_flight"],
        serde_json::json!({
            "id": "42/implement-2",
            "item": 42,
            "stage": "implement",
            "started_at": "2026-10-01T11:30:00Z",
            "duration_seconds": 1830,
            "quiet_seconds": 630,
        })
    );
    assert_eq!(
        json["past_runs"],
        serde_json::json!([
            {
                "id": "42/implement-1",
                "item": 42,
                "stage": "implement",
                "started_at": "2026-10-01T10:00:00Z",
                "duration_seconds": null,
                "outcome": "interrupted",
            },
            {
                "id": "42/triage-1",
                "item": 42,
                "stage": "triage",
                "started_at": "2026-10-01T09:00:00Z",
                "duration_seconds": 300,
                "outcome": "done",
            },
        ])
    );
    assert_eq!(serde_json::from_value::<Answer>(json).unwrap(), answer);
}

#[test]
fn an_item_linewright_does_not_track_has_no_explanation() {
    let untracked = Issue {
        labels: vec![],
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        explain(&view(vec![untracked], vec![]), &[], Number(42), at(NOW)),
        None
    );
    assert_eq!(
        explain(&view(vec![], vec![]), &[], Number(7), at(NOW)),
        None
    );
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
    let answer = explain(&view(vec![claimed], vec![]), &[], Number(42), at(NOW)).unwrap();

    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        serde_json::json!({
            "number": 42,
            "title": "Add invoices",
            "parent": null,
            "parents": [],
            "children": [],
            "state": "claimed",
            "last_read": "2026-10-01T12:00:00Z",
            "waiting_for": [{"wait": "blocker", "issue": 40}],
            "run_in_flight": null,
            "past_runs": [],
            "past_runs_left_out": 0,
            "runs_of": 42,
        })
    );
    assert_eq!(
        serde_json::from_value::<Explanation>(serde_json::to_value(&answer).unwrap()).unwrap(),
        answer
    );
}

#[test]
fn the_json_of_linewrights_pull_request_in_review_names_the_issue_it_closes_and_itself_as_runs_of()
{
    let claimed = Issue {
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    let view = view(vec![claimed], vec![draft(43, 42, &[CheckState::Pending])]);
    let answer = explain(&view, &[], Number(43), at(NOW)).unwrap();

    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        serde_json::json!({
            "number": 43,
            "title": "Do #42",
            "parent": {"number": 42, "role": "issue-it-closes"},
            "parents": [{"number": 42, "role": "issue-it-closes"}],
            "children": [],
            "pull_request_for": 42,
            "pull_request": 43,
            "draft": true,
            "ci": "running",
            "last_read": "2026-10-01T12:00:00Z",
            "waiting_for": [{"wait": "ci", "pull_request": 43}],
            "run_in_flight": null,
            "past_runs": [],
            "past_runs_left_out": 0,
            "runs_of": 43,
        })
    );
    assert_eq!(
        serde_json::from_value::<Explanation>(serde_json::to_value(&answer).unwrap()).unwrap(),
        answer
    );
}

#[test]
fn the_json_of_an_issue_in_review_names_the_pull_request_closing_it_as_its_child() {
    let claimed = Issue {
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    let view = view(
        vec![closed_by(claimed, 43, "factory")],
        vec![draft(43, 42, &[CheckState::Pending])],
    );
    let answer = explain(&view, &[], Number(42), at(NOW)).unwrap();
    let json = serde_json::to_value(&answer).unwrap();

    assert_eq!(json["parent"], serde_json::Value::Null);
    assert_eq!(json["parents"], serde_json::json!([]));
    assert_eq!(
        json["children"],
        serde_json::json!([{"number": 43, "role": "pull-request-closing-it"}])
    );
    assert_eq!(json["pull_request"], 43);
}

#[test]
fn a_ticket_someone_else_is_assigned_to_waits_for_them() {
    let assigned = Issue {
        assignees: vec![Login::new("alice")],
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        text(&view(vec![assigned], vec![]), 42)[1..],
        [
            "state: ready to claim",
            "last read: 2026-10-01 12:00:00 UTC",
            "waiting for: alice, who is assigned to it",
        ]
    );
}

#[test]
fn a_ticket_linewright_let_go_of_waits_for_its_label_or_the_required_label_again() {
    let let_go = Issue {
        labels: vec!["ready-for-agent".into(), "linewright".into()],
        events: vec![IssueEvent::Assignment(Assignment::Unassigned {
            assignee: Login::new("factory"),
            actor: Login::new("factory"),
            at: at("2026-10-01T11:00:00Z"),
        })],
        ..ticket(42, "Add invoices")
    };
    let mut view = view(vec![let_go], vec![]);
    view.policy.require_label = Some("linewright".into());

    assert_eq!(
        text(&view, 42)[3],
        "waiting for: the maintainer: to label it `ready-for-agent` or `linewright`, the \
         required label, again, since Linewright was unassigned"
    );
}

#[test]
fn an_issue_linewright_filed_labeled_needs_triage_is_tracked_for_triage() {
    let filed = Issue {
        author: Login::new("factory"),
        labels: vec!["needs-triage".into()],
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        text(&view(vec![filed], vec![]), 42)[1],
        "state: waiting for triage"
    );
}

#[test]
fn a_needs_triage_issue_linewright_was_unassigned_from_waits_for_the_label_again() {
    let stopped = Issue {
        labels: vec!["needs-triage".into()],
        events: vec![IssueEvent::Assignment(Assignment::Unassigned {
            assignee: Login::new("factory"),
            actor: Login::new("maintainer"),
            at: at("2026-10-01T11:00:00Z"),
        })],
        ..ticket(42, "Add invoices")
    };

    assert_eq!(
        text(&view(vec![stopped], vec![]), 42)[1..],
        [
            "state: waiting for triage",
            "last read: 2026-10-01 12:00:00 UTC",
            "waiting for: the maintainer: to label it `needs-triage` again, since Linewright was \
             unassigned",
        ]
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
        explain(&view(vec![tracking], vec![]), &[], Number(42), at(NOW)),
        None
    );
}

fn left_alone_text(view: &View, read: &AnyItem) -> Vec<String> {
    let answer = explain_read(view, &[], read, at(NOW));
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
            &AnyItem::LeftOutIssue(Box::new(closed))
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
            &AnyItem::LeftOutIssue(Box::new(elsewhere))
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
            &AnyItem::HeldIssue(Box::new(unlabeled))
        )[3],
        "left alone: it is labeled none of `needs-triage`, `needs-info`, `ready-for-agent` or \
         `ready-for-human`"
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
            &AnyItem::HeldIssue(Box::new(tracking))
        )[1..],
        [
            "parent: #5, the spec change it tracks",
            "state: open issue",
            "last read: 2026-10-01 12:00:30 UTC",
            "left alone: it is a tracking issue with no tickets, and none are being cut",
        ]
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
            &AnyItem::HeldIssue(Box::new(labeled))
        ),
        [
            "#42: Add invoices",
            "state: ready to claim",
            "last read: 2026-10-01 12:00:30 UTC",
            "waiting for: nothing: Linewright acts on it at its next poll",
        ]
    );
}

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
        left_alone_text(&view(vec![], vec![]), &AnyItem::Pull(Box::new(closed)))[1..],
        [
            "parent: #42, an issue it closes",
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
            &AnyItem::Pull(Box::new(someones_pull(43)))
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
fn linewrights_pull_request_closing_no_issue_says_so() {
    let for_nothing = Pull {
        closes: vec![],
        ..draft(43, 42, &[])
    };

    assert_eq!(
        left_alone_text(&view(vec![], vec![]), &AnyItem::Pull(Box::new(for_nothing)))[3],
        "left alone: it is Linewright's, and closes no issue Linewright could work on"
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
        left_alone_text(&view, &AnyItem::Pull(Box::new(older)))[4],
        "left alone: it is Linewright's for #42, which it works on through #44"
    );
}

fn merged(labels: &[&str]) -> MergedPull {
    MergedPull {
        number: Number(5),
        title: "Add invoices to the spec".into(),
        body: String::new(),
        author: Login::new("maintainer"),
        merged_at: at("2026-10-01T09:00:00Z"),
        base_ref: "main".into(),
        labels: labels.iter().map(ToString::to_string).collect(),
        closes: vec![],
        files: vec!["docs/specs/invoices.md".into()],
        assignees: vec![],
        events: vec![],
    }
}

fn merged_text(view: &View, pull: MergedPull) -> String {
    let lines = left_alone_text(
        view,
        &AnyItem::Merged {
            pull: Box::new(pull),
            tracking_issues: vec![],
        },
    );
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

fn unassigned_at(minute: &str) -> MergedPull {
    MergedPull {
        events: vec![
            IssueEvent::Labeled {
                label: "linewright".into(),
                at: at("2026-10-01T09:00:00Z"),
            },
            IssueEvent::Assignment(Assignment::Assigned {
                assignee: Login::new("factory"),
                actor: Login::new("maintainer"),
                at: at("2026-10-01T09:10:00Z"),
            }),
            IssueEvent::Assignment(Assignment::Unassigned {
                assignee: Login::new("factory"),
                actor: Login::new("factory"),
                at: at(&format!("2026-10-01T09:{minute}:00Z")),
            }),
        ],
        ..merged(&["linewright"])
    }
}

#[test]
fn a_merged_pull_request_linewright_was_unassigned_from_says_assigning_it_cuts_it_again() {
    assert_eq!(
        merged_text(&view(vec![], vec![]), unassigned_at("30")),
        "left alone: it is no spec change: Linewright was unassigned from it 2026-10-01 \
         09:30:00 UTC: assign Linewright to cut it again"
    );
}

#[test]
fn with_a_required_label_a_merged_pull_request_linewright_was_unassigned_from_names_the_label() {
    let mut view = view(vec![], vec![]);
    view.policy.require_label = Some("linewright".into());

    assert_eq!(
        merged_text(&view, unassigned_at("30")),
        "left alone: it is no spec change: Linewright was unassigned from it 2026-10-01 \
         09:30:00 UTC, and `linewright`, the required label, was not applied since: assign \
         Linewright or apply the label to cut it again"
    );
}

#[test]
fn a_ready_for_human_label_on_a_merged_pull_request_does_not_keep_it_from_being_cut() {
    let answer = explain_read(
        &view(vec![], vec![]),
        &[],
        &AnyItem::Merged {
            pull: Box::new(merged(&["ready-for-human"])),
            tracking_issues: vec![],
        },
        at(NOW),
    );

    assert_eq!(
        render_answer(&answer, false).lines().nth(1),
        Some("state: spec change waiting for tickets")
    );
}

#[test]
fn a_spec_change_the_read_finds_is_explained_as_waiting_for_tickets() {
    let answer = explain_read(
        &view(vec![], vec![]),
        &[],
        &AnyItem::Merged {
            pull: Box::new(merged(&[])),
            tracking_issues: vec![],
        },
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
        &[],
        &AnyItem::Merged {
            pull: Box::new(MergedPull {
                files: vec!["src/main.rs".into()],
                ..merged(&[])
            }),
            tracking_issues: vec![],
        },
        at(NOW),
    );

    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        serde_json::json!({
            "number": 5,
            "title": "Add invoices to the spec",
            "parent": null,
            "parents": [],
            "children": [],
            "item": "pull-request",
            "state": "merged",
            "last_read": "2026-10-01T12:00:30Z",
            "left_alone": {
                "reason": "no-spec-file",
                "spec_glob": "docs/**/*.md",
            },
            "run_in_flight": null,
            "past_runs": [],
            "past_runs_left_out": 0,
            "runs_of": 5,
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
            &[],
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
fn linewrights_pull_request_the_read_places_in_review_is_explained_as_its_issue() {
    let claimed = Issue {
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    let opened = draft(43, 42, &[CheckState::Pending]);

    assert_eq!(
        left_alone_text(
            &view(vec![claimed], vec![]),
            &AnyItem::Pull(Box::new(opened))
        ),
        [
            "#43: Do #42",
            "parent: #42, an issue it closes",
            "state: in review: #43 (draft, CI running)",
            "last read: 2026-10-01 12:00:30 UTC",
            "waiting for: CI on #43",
        ]
    );
}

#[test]
fn linewrights_pull_request_is_worked_whatever_the_labels_of_the_issue_it_is_for() {
    let handed_back = Issue {
        labels: vec!["ready-for-human".into()],
        ..ticket(42, "Add invoices")
    };
    let unlabeled = Issue {
        labels: vec![],
        ..ticket(42, "Add invoices")
    };
    let open = draft(43, 42, &[CheckState::Pending]);

    for issue in [handed_back, unlabeled] {
        assert_eq!(
            left_alone_text(
                &view(vec![issue], vec![]),
                &AnyItem::Pull(Box::new(open.clone()))
            ),
            [
                "#43: Do #42",
                "parent: #42, an issue it closes",
                "state: in review: #43 (draft, CI running)",
                "last read: 2026-10-01 12:00:30 UTC",
                "waiting for: CI on #43",
            ]
        );
    }
}

#[test]
fn linewrights_pull_request_it_is_unassigned_from_waits_for_the_maintainer() {
    let open = Pull {
        assignees: vec![],
        assignments: vec![Assignment::Unassigned {
            assignee: Login::new("factory"),
            actor: Login::new("maintainer"),
            at: at("2026-10-01T11:59:00Z"),
        }],
        ..draft(43, 42, &[CheckState::Failed])
    };
    let view = view(
        vec![closed_by(ticket(42, "Add invoices"), 43, "factory")],
        vec![open],
    );

    assert_eq!(
        text(&view, 43)[4..],
        ["waiting for: the maintainer: Linewright is unassigned, until they assign it again"]
    );
    let answer = explain(&view, &[], Number(43), at(NOW)).unwrap();
    assert_eq!(
        serde_json::to_value(&answer).unwrap()["waiting_for"],
        serde_json::json!([{"wait": "unassigned"}])
    );
    assert_eq!(
        text(&view, 42)[4..],
        ["waiting for: #43, its pull request, until it is merged or closed"]
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
        &[],
        &AnyItem::HeldIssue(Box::new(unlabeled.clone())),
        at(NOW),
    );
    let left_out = explain_read(
        &view(vec![], vec![]),
        &[],
        &AnyItem::LeftOutIssue(Box::new(unlabeled)),
        at(NOW),
    );

    for (answer, reason) in [(&held, "no-pipeline-label"), (&left_out, "not-in-view")] {
        assert_eq!(
            serde_json::to_value(answer).unwrap(),
            serde_json::json!({
                "number": 42,
                "title": "Add invoices",
                "parent": null,
                "parents": [],
                "children": [],
                "item": "issue",
                "state": "open",
                "last_read": "2026-10-01T12:00:30Z",
                "left_alone": {"reason": reason},
                "run_in_flight": null,
                "past_runs": [],
                "past_runs_left_out": 0,
                "runs_of": 42,
            })
        );
        assert_eq!(
            &serde_json::from_value::<Answer>(serde_json::to_value(answer).unwrap()).unwrap(),
            answer
        );
    }
}

fn family_lines(lines: &[String]) -> Vec<&str> {
    lines
        .iter()
        .map(String::as_str)
        .filter(|line| line.starts_with("parent: ") || line.starts_with("child: "))
        .collect()
}

#[test]
fn linewrights_pull_request_left_alone_while_open_names_the_issue_it_closes() {
    let claimed = Issue {
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    let older = draft(43, 42, &[]);
    let superseded = view(vec![claimed], vec![older.clone(), draft(44, 42, &[])]);

    assert_eq!(
        family_lines(&left_alone_text(
            &superseded,
            &AnyItem::Pull(Box::new(older))
        )),
        ["parent: #42, an issue it closes"]
    );
}

#[test]
fn linewrights_merged_pull_request_names_the_issue_it_closes() {
    let done = MergedPull {
        number: Number(43),
        title: "Do #42".into(),
        body: "Adds invoices.\n\nCloses #42".into(),
        closes: vec![Number(42)],
        author: Login::new("factory"),
        files: vec!["src/invoices.rs".into()],
        ..merged(&[])
    };

    let lines = left_alone_text(
        &view(vec![], vec![]),
        &AnyItem::Merged {
            pull: Box::new(done),
            tracking_issues: vec![],
        },
    );

    assert_eq!(
        lines[..3],
        [
            "#43: Do #42",
            "parent: #42, an issue it closes",
            "state: merged pull request",
        ]
    );
}

#[test]
fn a_pull_request_someone_else_opened_names_the_issue_it_closes() {
    let someones_merged = MergedPull {
        body: "Closes #42".into(),
        closes: vec![Number(42)],
        files: vec!["src/main.rs".into()],
        ..merged(&[])
    };
    let someones_closed = Pull {
        body: "Closes #42".into(),
        closes: vec![Number(42)],
        state: PullState::Closed {
            at: at("2026-10-01T11:00:00Z"),
        },
        ..someones_pull(43)
    };
    let someones_open = Pull {
        closes: vec![Number(42)],
        ..someones_pull(43)
    };

    for read in [
        AnyItem::Merged {
            pull: Box::new(someones_merged),
            tracking_issues: vec![],
        },
        AnyItem::Pull(Box::new(someones_closed)),
        AnyItem::Pull(Box::new(someones_open)),
    ] {
        assert_eq!(
            family_lines(&left_alone_text(&view(vec![], vec![]), &read)),
            ["parent: #42, an issue it closes"]
        );
    }
}

#[test]
fn the_json_of_a_closed_pull_request_names_the_issue_it_closes() {
    let closed = Pull {
        state: PullState::Closed {
            at: at("2026-10-01T11:00:00Z"),
        },
        ..draft(43, 42, &[])
    };
    let answer = explain_read(
        &view(vec![], vec![]),
        &[],
        &AnyItem::Pull(Box::new(closed)),
        at(NOW),
    );

    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        serde_json::json!({
            "number": 43,
            "title": "Do #42",
            "parent": {"number": 42, "role": "issue-it-closes"},
            "parents": [{"number": 42, "role": "issue-it-closes"}],
            "children": [],
            "item": "pull-request",
            "state": "closed",
            "last_read": "2026-10-01T12:00:30Z",
            "runs_of": 43,
            "run_in_flight": null,
            "past_runs": [],
            "past_runs_left_out": 0,
        })
    );
    assert_eq!(
        serde_json::from_value::<Answer>(serde_json::to_value(&answer).unwrap()).unwrap(),
        answer
    );
}

fn ticket_of_40() -> Issue {
    Issue {
        parent: Some(Parent {
            number: Number(40),
            spec_change: Some(Number(5)),
        }),
        ..ticket(42, "Add invoices")
    }
}

#[test]
fn a_ticket_names_its_tracking_issue() {
    let view = view(vec![ticket_of_40()], vec![]);

    assert_eq!(
        family_lines(&text(&view, 42)),
        ["parent: #40, its tracking issue"]
    );
}

fn tracking_40() -> Issue {
    Issue {
        author: Login::new("factory"),
        labels: vec![],
        sub_issues: vec![
            Linked {
                number: Number(41),
                open: false,
            },
            Linked {
                number: Number(42),
                open: true,
            },
        ],
        filed: Some(Filed::Tracking {
            spec_change: Number(5),
        }),
        ..ticket(40, "Spec change: Add invoices")
    }
}

#[test]
fn a_tracking_issue_names_its_spec_change_and_each_of_its_tickets() {
    let view = view(vec![tracking_40(), ticket_of_40()], vec![]);

    assert_eq!(
        family_lines(&text(&view, 40)),
        [
            "parent: #5, the spec change it tracks",
            "child: #41, one of its tickets",
            "child: #42, one of its tickets",
        ]
    );
}

#[test]
fn a_ticket_in_review_names_its_tracking_issue_and_the_pull_request_closing_it() {
    let claimed = Issue {
        assignees: vec![Login::new("factory")],
        ..ticket_of_40()
    };
    let view = view(
        vec![closed_by(claimed, 43, "factory")],
        vec![draft(43, 42, &[CheckState::Pending])],
    );

    assert_eq!(
        family_lines(&text(&view, 42)),
        [
            "parent: #40, its tracking issue",
            "child: #43, a pull request that closes it",
        ]
    );
}

#[test]
fn a_closed_ticket_names_its_tracking_issue() {
    let closed = Issue {
        open: false,
        ..ticket_of_40()
    };

    for read in [
        AnyItem::HeldIssue(Box::new(closed.clone())),
        AnyItem::LeftOutIssue(Box::new(closed)),
    ] {
        assert_eq!(
            family_lines(&left_alone_text(&view(vec![], vec![]), &read)),
            ["parent: #40, its tracking issue"]
        );
    }
}

#[test]
fn an_issue_whose_parent_is_no_tracking_issue_names_no_parent() {
    let sub_issue = Issue {
        parent: Some(Parent {
            number: Number(40),
            spec_change: None,
        }),
        ..ticket(42, "Add invoices")
    };
    let view = view(vec![sub_issue], vec![]);

    assert_eq!(family_lines(&text(&view, 42)), Vec::<&str>::new());
}

#[test]
fn the_json_of_a_tracking_issue_names_its_spec_change_and_tickets_by_role() {
    let view = view(vec![tracking_40(), ticket_of_40()], vec![]);
    let answer = explain(&view, &[], Number(40), at(NOW)).unwrap();
    let json = serde_json::to_value(&answer).unwrap();

    assert_eq!(
        json["parent"],
        serde_json::json!({"number": 5, "role": "spec-change"})
    );
    assert_eq!(
        json["children"],
        serde_json::json!([
            {"number": 41, "role": "ticket"},
            {"number": 42, "role": "ticket"},
        ])
    );
    assert_eq!(json["state"], "tracking");
}

#[test]
fn the_json_of_a_ticket_names_its_tracking_issue_by_role() {
    let view = view(vec![ticket_of_40()], vec![]);
    let answer = explain(&view, &[], Number(42), at(NOW)).unwrap();
    let json = serde_json::to_value(&answer).unwrap();

    assert_eq!(
        json["parent"],
        serde_json::json!({"number": 40, "role": "tracking-issue"})
    );
    assert_eq!(json["children"], serde_json::json!([]));
}

#[test]
fn a_spec_change_cut_into_tickets_names_its_tracking_issue() {
    let processed = AnyItem::Merged {
        pull: Box::new(merged(&["linewright:spec-processed"])),
        tracking_issues: vec![Number(40)],
    };

    let lines = left_alone_text(&view(vec![], vec![]), &processed);

    assert_eq!(
        lines[..3],
        [
            "#5: Add invoices to the spec",
            "child: #40, its tracking issue",
            "state: merged pull request",
        ]
    );
}

#[test]
fn the_json_of_a_spec_change_cut_into_tickets_names_its_tracking_issue_by_role() {
    let processed = AnyItem::Merged {
        pull: Box::new(merged(&["linewright:spec-processed"])),
        tracking_issues: vec![Number(40)],
    };
    let answer = explain_read(&view(vec![], vec![]), &[], &processed, at(NOW));
    let json = serde_json::to_value(&answer).unwrap();

    assert_eq!(json["parent"], serde_json::Value::Null);
    assert_eq!(
        json["children"],
        serde_json::json!([{"number": 40, "role": "tracking-issue"}])
    );
}

#[test]
fn a_spec_change_cut_again_names_its_newest_tracking_issue() {
    let processed = AnyItem::Merged {
        pull: Box::new(merged(&["linewright:spec-processed"])),
        tracking_issues: vec![Number(44), Number(40)],
    };

    assert_eq!(
        family_lines(&left_alone_text(&view(vec![], vec![]), &processed)),
        ["child: #44, its tracking issue"]
    );
}

#[test]
fn a_spec_change_not_cut_names_no_tracking_issue_from_a_cut_discarded_part_way() {
    let handed_back = AnyItem::Merged {
        pull: Box::new(unassigned_at("30")),
        tracking_issues: vec![Number(40)],
    };

    assert_eq!(
        family_lines(&left_alone_text(&view(vec![], vec![]), &handed_back)),
        Vec::<&str>::new()
    );
}

fn implemented_42() -> Vec<Recorded> {
    vec![recorded(
        "42/implement-1",
        "2026-10-01T10:00:00Z",
        ended("2026-10-01T10:30:00Z", Ending::Done),
    )]
}

fn read_family(view: &View, recorded: &[Recorded], read: &AnyItem) -> Vec<String> {
    let answer = explain_read(view, recorded, read, at(NOW));
    let lines = lines_of(&render_answer(&answer, false));
    family_lines(&lines)
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn merged_43(closes: &[u64]) -> MergedPull {
    MergedPull {
        number: Number(43),
        title: "Do #42".into(),
        closes: numbers(closes),
        author: Login::new("factory"),
        files: vec!["src/invoices.rs".into()],
        ..merged(&[])
    }
}

fn closed_43(closes: &[u64]) -> Pull {
    Pull {
        closes: numbers(closes),
        state: PullState::Closed {
            at: at("2026-10-01T11:00:00Z"),
        },
        ..draft(43, 42, &[])
    }
}

#[test]
fn linewrights_pull_request_names_the_issues_it_closes() {
    let edited_open = Pull {
        body: "Closes #12".into(),
        closes: numbers(&[12]),
        ..draft(43, 42, &[])
    };
    let both = [
        "parent: #42, an issue it closes",
        "parent: #12, an issue it closes",
    ];
    let only_12 = ["parent: #12, an issue it closes"];

    for (read, parents) in [
        (
            AnyItem::Merged {
                pull: Box::new(merged_43(&[42, 12])),
                tracking_issues: vec![],
            },
            &both[..],
        ),
        (
            AnyItem::Merged {
                pull: Box::new(merged_43(&[12])),
                tracking_issues: vec![],
            },
            &only_12[..],
        ),
        (AnyItem::Pull(Box::new(closed_43(&[42, 12]))), &both[..]),
        (AnyItem::Pull(Box::new(closed_43(&[12]))), &only_12[..]),
        (AnyItem::Pull(Box::new(edited_open)), &only_12[..]),
    ] {
        assert_eq!(
            read_family(&view(vec![], vec![]), &implemented_42(), &read),
            parents
        );
    }
}

fn claimed_12_and_42() -> Vec<Issue> {
    [12, 42]
        .map(|number| {
            let claimed = Issue {
                assignees: vec![Login::new("factory")],
                ..ticket(number, "Add invoices")
            };
            closed_by(claimed, 43, "factory")
        })
        .into()
}

fn open_43_closing_12_and_42() -> Pull {
    Pull {
        body: "Closes #12 and closes #42".into(),
        closes: numbers(&[12, 42]),
        ..draft(43, 42, &[CheckState::Pending])
    }
}

#[test]
fn linewrights_pull_request_in_review_names_each_issue_it_closes() {
    let in_review = view(claimed_12_and_42(), vec![open_43_closing_12_and_42()]);
    let answer = explain(&in_review, &implemented_42(), Number(43), at(NOW)).unwrap();
    let lines = lines_of(&render(&answer, false));

    assert_eq!(
        family_lines(&lines),
        [
            "parent: #12, an issue it closes",
            "parent: #42, an issue it closes",
        ]
    );
    let json = serde_json::to_value(&answer).unwrap();
    assert_eq!(
        json["parents"],
        serde_json::json!([
            {"number": 12, "role": "issue-it-closes"},
            {"number": 42, "role": "issue-it-closes"},
        ])
    );
    assert_eq!(
        json["parent"],
        serde_json::json!({"number": 12, "role": "issue-it-closes"})
    );
    assert_eq!(json["children"], serde_json::json!([]));
}

#[test]
fn linewrights_pull_request_is_a_child_of_each_issue_it_closes() {
    let view = view(claimed_12_and_42(), vec![open_43_closing_12_and_42()]);

    for issue in [12, 42] {
        let answer = explain(&view, &implemented_42(), Number(issue), at(NOW)).unwrap();
        assert_eq!(
            family_lines(&lines_of(&render(&answer, false))),
            ["child: #43, a pull request that closes it"]
        );
    }
}

#[test]
fn linewrights_pull_request_is_explained_for_the_first_issue_it_closes_whatever_its_labels() {
    let handed_back = Issue {
        labels: vec!["ready-for-human".into()],
        assignees: vec![Login::new("factory")],
        ..ticket(42, "Add invoices")
    };
    let claimed_12 = Issue {
        assignees: vec![Login::new("factory")],
        ..ticket(12, "Old invoices")
    };
    let view = view(
        vec![claimed_12, handed_back],
        vec![open_43_closing_12_and_42()],
    );

    let answer = explain(&view, &implemented_42(), Number(43), at(NOW)).unwrap();
    assert_eq!(
        lines_of(&render(&answer, false))[1..],
        [
            "parent: #12, an issue it closes",
            "parent: #42, an issue it closes",
            "state: in review: #43 (draft, CI running)",
            "last read: 2026-10-01 12:00:00 UTC",
            "waiting for: CI on #43",
        ]
    );
    assert_eq!(
        serde_json::to_value(&answer).unwrap()["pull_request_for"],
        12
    );
}

#[test]
fn a_spec_change_cut_into_tickets_names_the_issue_it_closes_and_its_tracking_issue() {
    let processed = MergedPull {
        labels: vec!["linewright:spec-processed".into()],
        files: vec!["docs/specs/invoices.md".into()],
        ..merged_43(&[12])
    };
    let read = AnyItem::Merged {
        pull: Box::new(processed),
        tracking_issues: vec![Number(40)],
    };

    assert_eq!(
        read_family(&view(vec![], vec![]), &implemented_42(), &read),
        [
            "parent: #12, an issue it closes",
            "child: #40, its tracking issue",
        ]
    );
    let answer = explain_read(&view(vec![], vec![]), &implemented_42(), &read, at(NOW));
    let json = serde_json::to_value(&answer).unwrap();
    assert_eq!(
        json["parents"],
        serde_json::json!([{"number": 12, "role": "issue-it-closes"}])
    );
    assert_eq!(
        json["children"],
        serde_json::json!([{"number": 40, "role": "tracking-issue"}])
    );
}

fn closing_40_and_41(state: PullState) -> Pull {
    Pull {
        body: "Closes #40 and closes #41".into(),
        closes: numbers(&[40, 41]),
        state,
        ..someones_pull(50)
    }
}

#[test]
fn a_pull_request_names_each_issue_it_closes_as_a_parent() {
    let closed = PullState::Closed {
        at: at("2026-10-01T11:00:00Z"),
    };
    let merged = MergedPull {
        number: Number(50),
        closes: numbers(&[40, 41]),
        ..merged(&[])
    };

    for read in [
        AnyItem::Pull(Box::new(closing_40_and_41(PullState::Open {
            draft: false,
        }))),
        AnyItem::Pull(Box::new(closing_40_and_41(closed))),
        AnyItem::Merged {
            pull: Box::new(merged),
            tracking_issues: vec![],
        },
    ] {
        assert_eq!(
            read_family(&view(vec![], vec![]), &[], &read),
            [
                "parent: #40, an issue it closes",
                "parent: #41, an issue it closes",
            ]
        );
        let answer = explain_read(&view(vec![], vec![]), &[], &read, at(NOW));
        let json = serde_json::to_value(&answer).unwrap();
        assert_eq!(
            json["parents"],
            serde_json::json!([
                {"number": 40, "role": "issue-it-closes"},
                {"number": 41, "role": "issue-it-closes"},
            ])
        );
        assert_eq!(serde_json::from_value::<Answer>(json).unwrap(), answer);
    }
}

#[test]
fn a_ticket_names_each_pull_request_that_closes_it_open_closed_or_merged() {
    let ended = |number| ClosingPull {
        number: Number(number),
        author: Login::new("alice"),
        state: PullState::Closed {
            at: at("2026-10-01T11:00:00Z"),
        },
    };
    let closed = Issue {
        open: false,
        closed_by: vec![ended(43), ended(44)],
        ..ticket(42, "Add invoices")
    };
    let mut open = closed_by(ticket(42, "Add invoices"), 45, "alice");
    open.closed_by.insert(0, ended(43));

    assert_eq!(
        family_lines(&left_alone_text(
            &view(vec![], vec![]),
            &AnyItem::LeftOutIssue(Box::new(closed))
        )),
        [
            "child: #43, a pull request that closes it",
            "child: #44, a pull request that closes it",
        ]
    );
    assert_eq!(
        family_lines(&text(&view(vec![open], vec![]), 42)),
        [
            "child: #45, a pull request that closes it",
            "child: #43, a pull request that closes it",
        ]
    );
}
