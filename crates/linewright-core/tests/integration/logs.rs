use linewright_core::domain::Number;
use linewright_core::logs::{self, LogsError, NoSuchRun, Which};
use linewright_core::runs::{Recorded, RunId, State};
use linewright_core::testing::logs::*;
use linewright_core::testing::*;
use linewright_core::transcript::{self, Verbosity};
use serde_json::json;

fn picked(recorded: &[Recorded], which: &Which) -> Result<String, NoSuchRun> {
    logs::pick(recorded, which).map(|run| run.id.to_string())
}

fn item(n: u64) -> Which {
    Which::Item(Number(n))
}

#[test_log::test]
fn an_items_run_in_flight_is_the_one_shown() {
    let mut in_flight = run("1/rework-1", "2026-10-14T11:00:00Z", 0);
    in_flight.state = State::InFlight;
    let recorded = vec![
        run("1/implement-1", "2026-10-14T10:00:00Z", 1),
        in_flight,
        run("2/triage-1", "2026-10-14T12:00:00Z", 1),
    ];

    assert_eq!(picked(&recorded, &item(1)).unwrap(), "1/rework-1");
}

#[test_log::test]
fn with_none_in_flight_the_latest_run_that_started_a_session_is_shown() {
    let recorded = vec![
        run("1/implement-1", "2026-10-14T10:00:00Z", 1),
        run("1/rework-1", "2026-10-14T11:00:00Z", 2),
        run("1/update-1", "2026-10-14T12:00:00Z", 0),
        run("2/triage-1", "2026-10-14T13:00:00Z", 1),
    ];

    assert_eq!(picked(&recorded, &item(1)).unwrap(), "1/rework-1");
}

#[test_log::test]
fn with_no_run_that_started_a_session_the_latest_run_is_shown() {
    let recorded = vec![
        run("1/update-1", "2026-10-14T10:00:00Z", 0),
        run("1/update-2", "2026-10-14T11:00:00Z", 0),
    ];

    assert_eq!(picked(&recorded, &item(1)).unwrap(), "1/update-2");
}

#[test_log::test]
fn a_named_run_is_the_one_shown() {
    let recorded = vec![
        run("1/implement-1", "2026-10-14T10:00:00Z", 1),
        run("1/rework-1", "2026-10-14T11:00:00Z", 1),
        run("weekly.md/1", "2026-10-14T12:00:00Z", 1),
    ];
    let named = |id: &str, of: Option<u64>| Which::Run {
        id: id.parse::<RunId>().unwrap(),
        item: of.map(Number),
    };

    assert_eq!(
        picked(&recorded, &named("1/implement-1", Some(1))).unwrap(),
        "1/implement-1"
    );
    assert_eq!(
        picked(&recorded, &named("weekly.md/1", None)).unwrap(),
        "weekly.md/1"
    );
    assert_eq!(
        picked(&recorded, &named("1/rework-2", None)),
        Err(NoSuchRun::NoRun("1/rework-2".parse().unwrap()))
    );
    assert_eq!(
        picked(&recorded, &named("1/rework-1", Some(2))),
        Err(NoSuchRun::NotOf {
            run: "1/rework-1".parse().unwrap(),
            item: Number(2),
        })
    );
}

fn ticket_and_pull() -> Vec<Recorded> {
    let mut in_flight = run("3/rework-2", "2026-10-14T11:30:00Z", 1);
    in_flight.state = State::InFlight;
    vec![
        run("1/triage-1", "2026-10-14T09:00:00Z", 1),
        run("1/implement-1", "2026-10-14T10:00:00Z", 1),
        run("3/rework-1", "2026-10-14T11:00:00Z", 1),
        in_flight,
    ]
}

#[test_log::test]
fn a_pull_requests_runs_are_its_own() {
    let named = |id: &str| Which::Run {
        id: id.parse::<RunId>().unwrap(),
        item: Some(Number(3)),
    };

    assert_eq!(picked(&ticket_and_pull(), &item(3)).unwrap(), "3/rework-2");
    assert_eq!(
        picked(&ticket_and_pull(), &named("3/rework-1")).unwrap(),
        "3/rework-1"
    );
    assert_eq!(
        picked(&ticket_and_pull(), &named("1/implement-1")),
        Err(NoSuchRun::NotOf {
            run: "1/implement-1".parse().unwrap(),
            item: Number(3),
        })
    );
}

#[test_log::test]
fn a_tickets_runs_are_none_of_its_pull_requests() {
    assert_eq!(
        picked(&ticket_and_pull(), &item(1)).unwrap(),
        "1/implement-1"
    );
}

#[test_log::test]
fn an_item_with_no_runs_has_none_to_show() {
    let recorded = vec![run("1/implement-1", "2026-10-14T10:00:00Z", 1)];

    assert_eq!(
        picked(&recorded, &item(2)),
        Err(NoSuchRun::NoRuns(Number(2)))
    );
}

#[test_log::test(tokio::test)]
async fn a_run_recorded_before_sessions_kept_what_they_ran_with_shows_its_transcript() {
    let record = item_run_record(
        5,
        "rework",
        1,
        Some("done"),
        &json!(["sessions/5/rework-1"]),
    );
    let output = output_of(&[
        json!({"type": "linewright-turn", "started_at": "2026-10-14T12:00:05Z", "started_by": "brief"}),
        json!({
            "type": "assistant",
            "timestamp": "2026-10-14T12:00:09Z",
            "message": {"content": [{"type": "text", "text": "Renamed it."}]},
        }),
        json!({"type": "result", "subtype": "success", "is_error": false, "result": "Renamed it."}),
    ]);
    let (_base, work_dir) =
        work_dir_keeping(&[&record], &[("sessions/5/rework-1/output.jsonl", &output)]).await;

    let transcript = logs::answer(Some(&runs_in(&work_dir.resolve()).await), &item(5))
        .await
        .unwrap();

    assert_eq!(
        transcript::render(&transcript, Verbosity::Compact, false),
        "run: 5/rework-1\n\
         session 1: started 2026-10-14 12:00:05 UTC\n \
         0:04  Renamed it.\n \
         0:04  worked for 4s\n"
    );
}

#[test_log::test(tokio::test)]
async fn a_named_run_whose_record_does_not_read_fails_saying_why() {
    let (_base, work_dir) = work_dir_holding(&[]);
    let runs = runs_in(&work_dir.resolve()).await;
    let unreadable_session = json!([{"dir": "sessions/1/implement-1", "model": 5}]);
    let record = item_run_record(1, "implement", 1, Some("done"), &unreadable_session);
    keep_record(&work_dir.resolve(), &serde_json::from_str(&record).unwrap()).await;
    let named = Which::Run {
        id: "1/implement-1".parse().unwrap(),
        item: None,
    };

    let answered = logs::answer(Some(&runs), &named).await;

    let Err(LogsError::Other(e)) = answered else {
        panic!("not a failure to read: {answered:?}");
    };
    assert_eq!(
        format!("{e:#}"),
        format!(
            "reading the record of 1/implement-1: reading what the session in {} ran with: invalid type: integer `5`, expected a string at line 1 column 10",
            work_dir.resolve().join("sessions/1/implement-1").display()
        )
    );
}

#[test_log::test(tokio::test)]
async fn a_session_yet_to_write_anything_counts_as_started() {
    let record = item_run_record(1, "triage", 1, None, &json!(["sessions/1/triage-1"]));
    let (base, work_dir) = work_dir_keeping(&[&record], &[]).await;
    let repo_dir = base.path().join(REPO);
    std::fs::create_dir_all(repo_dir.join("sessions/1/triage-1")).unwrap();
    let _lock = lock_of(&repo_dir.join("runs/items/1/triage-1.json"));

    let transcript = logs::answer(Some(&runs_in(&work_dir.resolve()).await), &item(1))
        .await
        .unwrap();

    assert_eq!(
        transcript::render(&transcript, Verbosity::Compact, false),
        "run: 1/triage-1\n"
    );
    assert_eq!(
        transcript::json_lines(&transcript, Verbosity::Compact),
        "{\"type\":\"run\",\"run\":\"1/triage-1\",\"sessions\":1}\n"
    );
}

/// `docs/specs/runs.md`, Transcripts, Running.
#[test_log::test(tokio::test)]
async fn every_session_but_a_run_in_flights_last_shows_a_call_that_never_returned_as_interrupted() {
    let made = |at: &str| {
        output_of(&[
            json!({"type": "linewright-turn", "started_at": at, "started_by": "brief"}),
            json!({
                "type": "assistant",
                "timestamp": at,
                "message": {"content": [
                    {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo test"}},
                ]},
            }),
        ])
    };
    let shown = async |ending: Option<&str>| {
        let sessions = json!(["sessions/5/rework-1", "sessions/5/rework-1-2"]);
        let record = item_run_record(5, "rework", 1, ending, &sessions);
        let (base, work_dir) = work_dir_keeping(
            &[&record],
            &[
                (
                    "sessions/5/rework-1/output.jsonl",
                    &made("2026-10-14T12:00:05Z"),
                ),
                (
                    "sessions/5/rework-1-2/output.jsonl",
                    &made("2026-10-14T12:00:30Z"),
                ),
            ],
        )
        .await;
        let _lock = ending
            .is_none()
            .then(|| lock_of(&base.path().join(REPO).join("runs/items/5/rework-1.json")));
        let transcript = logs::answer(Some(&runs_in(&work_dir.resolve()).await), &item(5))
            .await
            .unwrap();
        transcript::render(&transcript, Verbosity::Compact, false)
    };

    assert_eq!(
        shown(None).await,
        "run: 5/rework-1\n\
         session 1: started 2026-10-14 12:00:05 UTC\n \
         0:00  Bash(cargo test) → interrupted\n \
         0:00  worked for, interrupted\n\
         session 2: started 2026-10-14 12:00:30 UTC\n \
         0:00  Bash(cargo test) → running\n"
    );
    assert_eq!(
        shown(Some("done")).await,
        "run: 5/rework-1\n\
         session 1: started 2026-10-14 12:00:05 UTC\n \
         0:00  Bash(cargo test) → interrupted\n \
         0:00  worked for, interrupted\n\
         session 2: started 2026-10-14 12:00:30 UTC\n \
         0:00  Bash(cargo test) → interrupted\n \
         0:00  worked for, interrupted\n"
    );
}
