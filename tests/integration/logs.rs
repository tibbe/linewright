use crate::common;

use std::path::PathBuf;

use chrono::TimeDelta;
use common::*;
use linewright::domain::Number;
use linewright::logs::{self, LogsError, NoSuchRun, Which};
use linewright::reconcile::WorkKind;
use linewright::runs::{Ending, Recorded, RunId, Started, State};
use linewright::transcript::{self, Verbosity};
use serde_json::json;

fn run(id: &str, start: &str, sessions: usize) -> Recorded {
    Recorded::new(
        Started::new(id.parse().unwrap(), None).unwrap(),
        at(start),
        State::Ended {
            at: at(start) + TimeDelta::minutes(1),
            ending: Ending::Done,
        },
        (0..sessions)
            .map(|n| PathBuf::from(format!("sessions/{id}/{n}")).into())
            .collect(),
        None,
    )
}

fn picked(recorded: &[Recorded], which: &Which) -> Result<String, NoSuchRun> {
    logs::pick(recorded, which).map(|run| run.id.to_string())
}

fn item(n: u64) -> Which {
    Which::Item(Number(n))
}

#[test]
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

#[test]
fn with_none_in_flight_the_latest_run_that_started_a_session_is_shown() {
    let recorded = vec![
        run("1/implement-1", "2026-10-14T10:00:00Z", 1),
        run("1/rework-1", "2026-10-14T11:00:00Z", 2),
        run("1/update-1", "2026-10-14T12:00:00Z", 0),
        run("2/triage-1", "2026-10-14T13:00:00Z", 1),
    ];

    assert_eq!(picked(&recorded, &item(1)).unwrap(), "1/rework-1");
}

#[test]
fn with_no_run_that_started_a_session_the_latest_run_is_shown() {
    let recorded = vec![
        run("1/update-1", "2026-10-14T10:00:00Z", 0),
        run("1/update-2", "2026-10-14T11:00:00Z", 0),
    ];

    assert_eq!(picked(&recorded, &item(1)).unwrap(), "1/update-2");
}

#[test]
fn a_named_run_is_the_one_shown() {
    let recorded = vec![
        run("1/implement-1", "2026-10-14T10:00:00Z", 1),
        run("1/rework-1", "2026-10-14T11:00:00Z", 1),
        run("weekly/1", "2026-10-14T12:00:00Z", 1),
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
        picked(&recorded, &named("weekly/1", None)).unwrap(),
        "weekly/1"
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

#[test]
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

#[test]
fn a_tickets_runs_are_none_of_its_pull_requests() {
    assert_eq!(
        picked(&ticket_and_pull(), &item(1)).unwrap(),
        "1/implement-1"
    );
}

#[test]
fn an_item_with_no_runs_has_none_to_show() {
    let recorded = vec![run("1/implement-1", "2026-10-14T10:00:00Z", 1)];

    assert_eq!(
        picked(&recorded, &item(2)),
        Err(NoSuchRun::NoRuns(Number(2)))
    );
}

/// Implementing sessions stream the recorded transcript: its first turn
/// ends in an answer that is not a result, and its second, after the
/// follow-up, in one that is.
pub(crate) fn implement_as_recorded(world: &World) {
    world.harness.on(WorkKind::Implement, |call| {
        let turn = match call.followup {
            None => include_str!("fixtures/transcript/turn-1.jsonl"),
            Some(_) => include_str!("fixtures/transcript/turn-2.jsonl"),
        };
        let output = call.spec.record.join(linewright::transcript::OUTPUT);
        let before = std::fs::read_to_string(&output).unwrap_or_default();
        std::fs::write(&output, before + turn).unwrap();
        if call.followup.is_none() {
            return Ok("Done.".into());
        }
        commit_in(&call.spec.dir, &[("src/done.txt", "done\n")], "Do it");
        Ok(json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."}).to_string())
    });
}

/// #1 implemented in a run whose sessions stream the recorded transcript.
async fn implemented_as_recorded() -> (World, tempfile::TempDir) {
    let (mut world, work_dir) = world_in_work_dir().await;
    implement_as_recorded(&world);
    world.store.open_issue(
        "maintainer",
        "Ship gh",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.poll().await;
    (world, work_dir)
}

#[tokio::test]
async fn logs_shows_the_items_latest_run_compact() {
    let (_world, work_dir) = implemented_as_recorded().await;

    let output = lw_in(work_dir.path(), &["logs", "1"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 15, "{}", output.stdout);
    assert_eq!(lines[0], "run: 1/implement-1");
    assert_eq!(lines[1], "session 1: started 2026-10-13 10:09:08 UTC");
}

#[tokio::test]
async fn an_unknown_item_or_run_exits_4_saying_so() {
    let (_world, work_dir) = implemented_as_recorded().await;

    let no_item = lw_in(work_dir.path(), &["logs", "7"]).await;
    let no_run = lw_in(work_dir.path(), &["logs", "1", "--run", "1/rework-1"]).await;
    let other_item = lw_in(work_dir.path(), &["logs", "7", "--run", "1/implement-1"]).await;
    let neither = lw_in(work_dir.path(), &["logs"]).await;

    for (output, error) in [
        (&no_item, "#7 has no runs"),
        (&no_run, "there is no run 1/rework-1"),
        (&other_item, "1/implement-1 is not a run on #7"),
    ] {
        assert_eq!(output.code, Some(4), "{}", output.stderr);
        assert_eq!(output.stdout, "");
        assert!(output.stderr.contains(error), "{}", output.stderr);
    }
    assert_eq!(neither.code, Some(2), "{}", neither.stderr);
}

#[tokio::test]
async fn following_a_run_that_has_ended_shows_its_transcript_and_exits() {
    let (_world, work_dir) = implemented_as_recorded().await;

    for way in [
        &[][..],
        &["--verbose"],
        &["--json"],
        &["--verbose", "--json"],
    ] {
        let shown = lw_in(work_dir.path(), &[&["logs", "1"], way].concat()).await;
        let followed = lw_in(work_dir.path(), &[&["logs", "1", "--follow"], way].concat()).await;

        assert_eq!(followed.code, Some(0), "{}", followed.stderr);
        assert_eq!(followed.stdout, shown.stdout, "{way:?}");
    }
}

/// `world_in_work_dir`'s, with `setup` the setup command of every stage.
async fn world_setting_up_with(setup: &str) -> (World, tempfile::TempDir) {
    let work_dir = tempfile::tempdir().unwrap();
    let world = World::with(|config| {
        config.work_dir = work_dir.path().join(REPO);
        setting_up_with(config, setup);
    })
    .await;
    (world, work_dir)
}

/// `docs/specs/monitoring.md`, logs, Setup.
#[tokio::test]
async fn logs_shows_the_setup_command_a_session_ran() {
    let (mut world, work_dir) = world_setting_up_with("echo 'added 3 packages'").await;
    implement_as_recorded(&world);
    world.store.open_issue(
        "maintainer",
        "Ship gh",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.poll().await;

    let output = lw_in(work_dir.path(), &["logs", "1"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        output.stdout.lines().nth(2),
        Some(" 0:00  setup: echo 'added 3 packages' → exit 0, 0s"),
        "{}",
        output.stdout
    );
}

#[test]
fn a_run_recorded_before_sessions_kept_what_they_ran_with_shows_its_transcript() {
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
    ]);
    let (_base, work_dir) = work_dir_holding(&[
        ("runs/items/5/rework-1.json", &record),
        ("sessions/5/rework-1/output.jsonl", &output),
    ]);

    let transcript = logs::answer(&work_dir, &item(5)).unwrap();

    assert_eq!(
        transcript::render(&transcript, Verbosity::Compact, false),
        "run: 5/rework-1\n\
         session 1: started 2026-10-14 12:00:05 UTC\n \
         0:04  Renamed it.\n"
    );
}

#[test]
fn a_named_run_whose_record_does_not_read_fails_saying_why() {
    let (_base, work_dir) = work_dir_holding(&[("runs/items/1/implement-1.json", "not a record")]);
    let named = Which::Run {
        id: "1/implement-1".parse().unwrap(),
        item: None,
    };

    let answered = logs::answer(&work_dir, &named);

    let Err(LogsError::Other(e)) = answered else {
        panic!("not a failure to read: {answered:?}");
    };
    assert_eq!(
        format!("{e:#}"),
        "reading the record of 1/implement-1: expected ident at line 1 column 2"
    );
}

#[test]
fn a_session_yet_to_write_anything_counts_as_started() {
    let record = item_run_record(1, "triage", 1, None, &json!(["sessions/1/triage-1"]));
    let (base, work_dir) = work_dir_holding(&[("runs/items/1/triage-1.json", &record)]);
    let repo_dir = base.path().join(REPO);
    std::fs::create_dir_all(repo_dir.join("sessions/1/triage-1")).unwrap();
    let _lock = lock_of(&repo_dir.join("runs/items/1/triage-1.json"));

    let transcript = logs::answer(&work_dir, &item(1)).unwrap();

    assert_eq!(
        transcript::render(&transcript, Verbosity::Compact, false),
        "run: 1/triage-1\n"
    );
    assert_eq!(
        transcript::json_lines(&transcript, Verbosity::Compact),
        "{\"type\":\"run\",\"run\":\"1/triage-1\",\"sessions\":1}\n"
    );
}
