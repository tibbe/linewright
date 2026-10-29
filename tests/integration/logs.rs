use crate::common;

use std::path::PathBuf;

use chrono::TimeDelta;
use common::*;
use linewright::domain::Number;
use linewright::logs::{self, NoSuchRun, Which};
use linewright::reconcile::WorkKind;
use linewright::runs::{Ending, Recorded, RunId, Started, State};
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

/// Implementing sessions start a sub-agent that runs for 3h 2m 9s and makes
/// 3 tool calls, then answer with the result.
pub(crate) fn implement_with_a_sub_agent_of_3h_2m_9s(world: &World) {
    world.harness.on(WorkKind::Implement, |call| {
        let message = |kind: &str, at: &str, content: serde_json::Value| {
            json!({"type": kind, "timestamp": at, "message": {"content": content}})
        };
        let mut returned = message(
            "user",
            "2026-10-13T13:02:11Z",
            json!([{"type": "tool_result", "tool_use_id": "a1", "content": "Clean."}]),
        );
        returned["tool_use_result"] = json!({
            "status": "completed",
            "totalDurationMs": 10_929_400,
            "totalToolUseCount": 3,
        });
        let events = [
            json!({"type": "linewright-turn", "started_at": "2026-10-13T10:00:00Z", "started_by": "brief"}),
            message(
                "assistant",
                "2026-10-13T10:00:02Z",
                json!([{"type": "tool_use", "id": "a1", "name": "Agent", "input": {"description": "Review"}}]),
            ),
            returned,
            message(
                "assistant",
                "2026-10-13T13:02:15Z",
                json!([{"type": "text", "text": "The review came back clean."}]),
            ),
        ];
        let output = call.spec.record.join(linewright::transcript::OUTPUT);
        std::fs::write(
            &output,
            events.iter().map(|event| format!("{event}\n")).collect::<String>(),
        )
        .unwrap();
        commit_in(&call.spec.dir, &[("src/done.txt", "done\n")], "Do it");
        Ok(json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."}).to_string())
    });
}

async fn implemented_as_recorded() -> (World, tempfile::TempDir) {
    implemented_by(implement_as_recorded).await
}

/// #1 implemented in a run whose sessions `implement` scripts.
async fn implemented_by(implement: fn(&World)) -> (World, tempfile::TempDir) {
    let (mut world, work_dir) = world_in_work_dir().await;
    implement(&world);
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
    assert_eq!(
        lines[2],
        " 0:03  Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"im…) → exit 0, 19 lines"
    );
    assert_eq!(
        lines[9],
        " 2:02    Bash(git diff origin/main...HEAD --stat && git diff origin/main...HEAD && git log -1…) → exit 0, 7 lines"
    );
    assert!(
        lines[12].starts_with(" 3:22  follow-up: Your final message could not be used: "),
        "{}",
        output.stdout
    );
}

#[tokio::test]
async fn a_run_recorded_before_sessions_kept_what_they_ran_with_still_shows_its_transcript() {
    let (_world, work_dir) = implemented_as_recorded().await;
    let before = lw_in(work_dir.path(), &["logs", "1"]).await;
    // Its sessions as bare directories, as records kept them before.
    let path = work_dir
        .path()
        .join(REPO)
        .join("runs/items/1/implement-1.json");
    let mut record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let sessions = record["sessions"].as_array_mut().unwrap();
    for session in sessions.iter_mut() {
        *session = session["dir"].clone();
    }
    assert!(
        sessions.iter().all(serde_json::Value::is_string),
        "{record}"
    );
    std::fs::write(&path, record.to_string()).unwrap();

    let after = lw_in(work_dir.path(), &["logs", "1"]).await;

    assert_eq!(after.code, Some(0), "{}", after.stderr);
    assert_eq!(after.stdout, before.stdout);
    assert!(after.stdout.lines().count() > 2, "{}", after.stdout);
}

/// `docs/specs/design_system.md`, Transcripts; Time, Durations.
#[tokio::test]
async fn logs_shows_how_long_a_sub_agent_ran_as_a_duration_and_each_line_at_its_time() {
    let (_world, work_dir) = implemented_by(implement_with_a_sub_agent_of_3h_2m_9s).await;

    let output = lw_in(work_dir.path(), &["logs", "1"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<&str> = output.stdout.lines().skip(2).collect();
    assert_eq!(
        lines,
        [
            " 0:02  Agent(Review) → done, 3 tool uses, 3h 2m",
            "3:02:15  The review came back clean.",
        ],
        "{}",
        output.stdout
    );
}

#[tokio::test]
async fn logs_verbose_adds_the_brief_and_the_thinking() {
    let (_world, work_dir) = implemented_as_recorded().await;

    let output = lw_in(work_dir.path(), &["logs", "1", "--verbose"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines[2], " 0:00  brief: # Your task");
    assert!(
        output
            .stdout
            .contains(" 0:04  thinking: The listing has a Dockerfile.\n"),
        "{}",
        output.stdout
    );
}

#[tokio::test]
async fn logs_json_prints_one_object_per_line() {
    let (_world, work_dir) = implemented_as_recorded().await;

    let output = lw_in(work_dir.path(), &["logs", "1", "--json"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<serde_json::Value> = output
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 15, "{}", output.stdout);
    assert_eq!(
        lines[0],
        json!({"type": "run", "run": "1/implement-1", "sessions": 1})
    );
    assert_eq!(lines[14]["type"], "message");
    let first_sub_agent_line = lines.iter().find(|line| line["depth"] == 1).unwrap();
    assert_eq!(
        first_sub_agent_line["tool_use_id"],
        "toolu_01LBDfAEHMPeecSbXMbfKv6Z"
    );
    assert_eq!(
        first_sub_agent_line["parent_tool_use_id"],
        "toolu_01T5JmkDxVfWvgdZeKR3KCjP"
    );
}

#[tokio::test]
async fn an_automations_run_by_hand_says_so_in_its_header_and_a_scheduled_one_does_not() {
    let (mut world, work_dir) = world_in_work_dir().await;
    world.hourly_by_schedule_then_by_hand().await;

    let by_hand = lw_in(work_dir.path(), &["logs", "--run", "hourly/2"]).await;
    let scheduled = lw_in(work_dir.path(), &["logs", "--run", "hourly/1"]).await;
    let by_hand_json = lw_in(work_dir.path(), &["logs", "--run", "hourly/2", "--json"]).await;
    let scheduled_json = lw_in(work_dir.path(), &["logs", "--run", "hourly/1", "--json"]).await;

    assert_eq!(by_hand.code, Some(0), "{}", by_hand.stderr);
    assert_eq!(
        by_hand.stdout.lines().next(),
        Some("run: hourly/2, by hand")
    );
    assert_eq!(scheduled.stdout.lines().next(), Some("run: hourly/1"));
    let header = |output: &Output| -> serde_json::Value {
        serde_json::from_str(output.stdout.lines().next().unwrap()).unwrap()
    };
    assert_eq!(
        header(&by_hand_json),
        json!({"type": "run", "run": "hourly/2", "started_by": "hand", "sessions": 1})
    );
    assert_eq!(
        header(&scheduled_json),
        json!({"type": "run", "run": "hourly/1", "started_by": "schedule", "sessions": 1})
    );
}

#[tokio::test]
async fn a_run_that_started_no_session_says_so() {
    let (mut world, work_dir) = world_in_work_dir().await;
    let (_, pr) = world.ticket_in_review().await;
    assert_eq!(pr, Number(2));
    world
        .store
        .merge_change(
            "maintainer",
            "Other work",
            "",
            &[("src/other.txt", "other\n")],
        )
        .await
        .unwrap();
    // Brought up to date without a session: nothing conflicts.
    world.poll().await;

    let output = lw_in(work_dir.path(), &["logs", "2", "--run", "2/update-1"]).await;
    let json = lw_in(work_dir.path(), &["logs", "--run", "2/update-1", "--json"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, "run: 2/update-1 (it started no session)\n");
    assert_eq!(
        json.stdout,
        "{\"type\":\"run\",\"run\":\"2/update-1\",\"sessions\":0}\n"
    );
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
async fn a_named_run_whose_record_does_not_read_exits_1_saying_why() {
    let (_world, work_dir) = implemented_as_recorded().await;
    let [record] = &files_named(work_dir.path(), "items/1/implement-1.json")[..] else {
        panic!("one record of 1/implement-1");
    };
    std::fs::write(record, "not a record").unwrap();

    let output = lw_in(work_dir.path(), &["logs", "--run", "1/implement-1"]).await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert!(
        output
            .stderr
            .contains("reading the record of 1/implement-1: expected ident"),
        "{}",
        output.stderr
    );
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

fn json_lines(output: &Output) -> Vec<serde_json::Value> {
    output
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// `docs/specs/design_system.md`, Transcripts, Times and Setup;
/// `docs/specs/monitoring.md`, logs, Setup.
#[tokio::test]
async fn logs_shows_a_setup_command_as_its_sessions_first_entry_with_its_output_only_in_full() {
    let (mut world, work_dir) = world_setting_up_with("echo 'added 3 packages'").await;
    implement_as_recorded(&world);
    world.store.open_issue(
        "maintainer",
        "Ship gh",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.poll().await;

    let compact = lw_in(work_dir.path(), &["logs", "1"]).await;
    let verbose = lw_in(work_dir.path(), &["logs", "1", "--verbose"]).await;
    let json = lw_in(work_dir.path(), &["logs", "1", "--json"]).await;

    assert_eq!(compact.code, Some(0), "{}", compact.stderr);
    let compact: Vec<&str> = compact.stdout.lines().collect();
    // The session started with its setup command, when the clock said.
    assert!(
        compact[1].starts_with("session 1: started 2026-10-01 12:00:"),
        "{compact:#?}"
    );
    assert_eq!(
        compact[2],
        " 0:00  setup: echo 'added 3 packages' → exit 0, 0s"
    );
    assert!(compact[3].contains("  Bash(ls;"), "{compact:#?}");
    let verbose: Vec<&str> = verbose.stdout.lines().collect();
    assert_eq!(
        verbose[2..4],
        [
            " 0:00  setup: echo 'added 3 packages' → exit 0, 0s",
            "         added 3 packages",
        ]
    );
    assert!(verbose[4].contains("  brief: "), "{verbose:#?}");
    let json = json_lines(&json);
    let mut setup = json[2].clone();
    assert!(
        setup["duration_seconds"].as_f64().is_some_and(|s| s < 10.0),
        "{setup}"
    );
    setup["duration_seconds"] = json!(0);
    assert_eq!(
        setup,
        json!({
            "type": "setup",
            "session": 1,
            "at": json[1]["started_at"],
            "elapsed_seconds": 0,
            "depth": 0,
            "command": "echo 'added 3 packages'",
            "status": {"exited": 0},
            "duration_seconds": 0,
            "output": "added 3 packages\n",
        })
    );
    assert_eq!(json[3]["type"], "tool");
}

/// `docs/specs/monitoring.md`, logs: the run picked is the latest that
/// started a session, as one whose setup command failed did.
#[tokio::test]
async fn a_run_whose_setup_command_failed_shows_it_under_each_session_and_is_the_one_logs_shows() {
    let (mut world, work_dir) =
        world_setting_up_with("echo 'npm ERR! missing lockfile'; exit 3").await;
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let shown = lw_in(work_dir.path(), &["logs", "1"]).await;
    let json = lw_in(work_dir.path(), &["logs", "1", "--json"]).await;

    assert_eq!(shown.code, Some(0), "{}", shown.stderr);
    let setup = " 0:00  setup: echo 'npm ERR! missing lockfile'; exit 3 → exit 3, 0s\n         \
                 npm ERR! missing lockfile\n";
    let shown: Vec<&str> = shown.stdout.split_inclusive('\n').collect();
    assert_eq!(shown[0], "run: 1/triage-1\n");
    // Each session started with its setup command, when the clock said.
    for (session, lines) in shown[1..].chunks(3).enumerate() {
        let started = format!("session {}: started 2026-10-01 12:00:", session + 1);
        assert!(lines[0].starts_with(&started), "{shown:#?}");
        assert_eq!(lines[1..].concat(), setup);
    }
    assert_eq!(shown.len(), 7, "{shown:#?}");
    let lines = json_lines(&json);
    let types: Vec<&str> = lines
        .iter()
        .map(|line| line["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["run", "session", "setup", "session", "setup"]);
    assert_eq!(lines[2]["status"], json!({"exited": 3}));
    assert_eq!(lines[2]["output"], "npm ERR! missing lockfile\n");
}
