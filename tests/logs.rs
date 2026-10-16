//! `lw logs`, end to end at the operator's level: a factory doing
//! runs on the local store, its sessions streaming a recorded Claude Code
//! transcript, and the `lw` program showing it from the records with
//! no `serve` running; and which run `logs` shows, end to end on its data
//! model (`docs/specs/monitoring.md`, "logs").

mod common;

use std::path::{Path, PathBuf};

use chrono::TimeDelta;
use common::*;
use linewright::domain::{Number, Timestamp};
use linewright::logs::{self, NoSuchRun, Which};
use linewright::reconcile::WorkKind;
use linewright::runs::{Ending, Recorded, RunId, State};
use serde_json::json;

// Which run, end to end on its data model.

fn at(time: &str) -> Timestamp {
    format!("2026-10-14T{time}Z").parse().unwrap()
}

/// The run `id`, started at `start`, done a minute later, with `sessions`
/// sessions.
fn run(id: &str, start: &str, sessions: usize) -> Recorded {
    Recorded {
        id: id.parse().unwrap(),
        started_at: at(start),
        state: State::Ended {
            at: at(start) + TimeDelta::minutes(1),
            ending: Ending::Done,
        },
        sessions: (0..sessions)
            .map(|n| PathBuf::from(format!("sessions/{id}/{n}")))
            .collect(),
        pr: None,
    }
}

fn picked(recorded: Vec<Recorded>, which: Which) -> Result<String, NoSuchRun> {
    logs::pick(recorded, &which).map(|run| run.id.to_string())
}

fn item(n: u64) -> Which {
    Which::Item(Number(n))
}

#[test]
fn an_items_run_in_flight_is_the_one_shown() {
    let mut in_flight = run("1/rework-1", "11:00:00", 0);
    in_flight.state = State::InFlight;
    let recorded = vec![
        run("1/implement-1", "10:00:00", 1),
        in_flight,
        run("2/triage-1", "12:00:00", 1),
    ];

    assert_eq!(picked(recorded, item(1)).unwrap(), "1/rework-1");
}

#[test]
fn with_none_in_flight_the_latest_run_that_started_a_session_is_shown() {
    let recorded = vec![
        run("1/implement-1", "10:00:00", 1),
        run("1/rework-1", "11:00:00", 2),
        run("1/update-1", "12:00:00", 0),
        run("2/triage-1", "13:00:00", 1),
    ];

    assert_eq!(picked(recorded, item(1)).unwrap(), "1/rework-1");
}

#[test]
fn with_no_run_that_started_a_session_the_latest_run_is_shown() {
    let recorded = vec![
        run("1/update-1", "10:00:00", 0),
        run("1/update-2", "11:00:00", 0),
    ];

    assert_eq!(picked(recorded, item(1)).unwrap(), "1/update-2");
}

#[test]
fn a_named_run_is_the_one_shown() {
    let recorded = vec![
        run("1/implement-1", "10:00:00", 1),
        run("1/rework-1", "11:00:00", 1),
        run("weekly/1", "12:00:00", 1),
    ];
    let named = |id: &str, of: Option<u64>| Which::Run {
        id: id.parse::<RunId>().unwrap(),
        item: of.map(Number),
    };

    assert_eq!(
        picked(recorded.clone(), named("1/implement-1", Some(1))).unwrap(),
        "1/implement-1"
    );
    assert_eq!(
        picked(recorded.clone(), named("weekly/1", None)).unwrap(),
        "weekly/1"
    );
    assert_eq!(
        picked(recorded.clone(), named("1/rework-2", None)),
        Err(NoSuchRun::NoRun("1/rework-2".parse().unwrap()))
    );
    assert_eq!(
        picked(recorded, named("1/rework-1", Some(2))),
        Err(NoSuchRun::NotOf {
            run: "1/rework-1".parse().unwrap(),
            item: Number(2),
        })
    );
}

/// #1's runs, of which the implement run opened #3, and #3's own, the
/// latest in flight.
fn ticket_and_pull() -> Vec<Recorded> {
    let on_pull = |run: Recorded| Recorded {
        pr: Some(Number(3)),
        ..run
    };
    let mut in_flight = on_pull(run("3/rework-2", "11:30:00", 1));
    in_flight.state = State::InFlight;
    vec![
        run("1/triage-1", "09:00:00", 1),
        on_pull(run("1/implement-1", "10:00:00", 1)),
        on_pull(run("3/rework-1", "11:00:00", 1)),
        in_flight,
    ]
}

#[test]
fn a_pull_requests_runs_are_its_own() {
    let named = |id: &str| Which::Run {
        id: id.parse::<RunId>().unwrap(),
        item: Some(Number(3)),
    };

    assert_eq!(picked(ticket_and_pull(), item(3)).unwrap(), "3/rework-2");
    assert_eq!(
        picked(ticket_and_pull(), named("3/rework-1")).unwrap(),
        "3/rework-1"
    );
    assert_eq!(
        picked(ticket_and_pull(), named("1/implement-1")),
        Err(NoSuchRun::NotOf {
            run: "1/implement-1".parse().unwrap(),
            item: Number(3),
        })
    );
}

#[test]
fn a_tickets_runs_are_none_of_its_pull_requests() {
    assert_eq!(picked(ticket_and_pull(), item(1)).unwrap(), "1/implement-1");
}

#[test]
fn an_item_with_no_runs_has_none_to_show() {
    let recorded = vec![run("1/implement-1", "10:00:00", 1)];

    assert_eq!(picked(recorded, item(2)), Err(NoSuchRun::NoRuns(Number(2))));
}

// `logs` at the operator's level.

const REPO: &str = "owner/widgets";

/// A world whose factory keeps its records where `lw --work-dir`
/// finds those of `owner/widgets`, with that work directory.
async fn world() -> (World, tempfile::TempDir) {
    let work_dir = tempfile::tempdir().unwrap();
    let world = World::with(|config| config.work_dir = work_dir.path().join(REPO)).await;
    (world, work_dir)
}

struct Output {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Runs `lw --repo owner/widgets --work-dir <work_dir>` with `args`,
/// with no `serve` running.
async fn lw(work_dir: &Path, args: &[&str]) -> Output {
    let runtime_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .args(["--repo", REPO, "--work-dir"])
        .arg(work_dir)
        .args(args)
        .current_dir(cwd.path())
        .env("XDG_RUNTIME_DIR", runtime_dir.path())
        .output()
        .await
        .unwrap();
    Output {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// Implementing sessions stream the recorded transcript: its first turn
/// ends in an answer that is not a result, and its second, after the
/// follow-up, in one that is.
fn implement_as_recorded(world: &World) {
    world.runtime.on(WorkKind::Implement, |call| {
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
        json(json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."}))
    });
}

async fn implemented_as_recorded() -> (World, tempfile::TempDir) {
    let (mut world, work_dir) = world().await;
    implement_as_recorded(&world);
    world
        .store
        .open_issue("maintainer", "Ship gh", "Please.", &["ready-for-agent"]);
    world.poll().await;
    (world, work_dir)
}

#[tokio::test]
async fn logs_shows_the_items_latest_run_compact() {
    let (_world, work_dir) = implemented_as_recorded().await;

    let output = lw(work_dir.path(), &["logs", "1"]).await;

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
async fn logs_verbose_adds_the_brief_and_the_thinking() {
    let (_world, work_dir) = implemented_as_recorded().await;

    let output = lw(work_dir.path(), &["logs", "1", "--verbose"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines[2], " 0:00  brief: /linewright:implement");
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

    let output = lw(work_dir.path(), &["logs", "1", "--json"]).await;

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
async fn a_run_that_started_no_session_says_so() {
    let (mut world, work_dir) = world().await;
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

    let output = lw(work_dir.path(), &["logs", "2", "--run", "2/update-1"]).await;
    let json = lw(work_dir.path(), &["logs", "--run", "2/update-1", "--json"]).await;

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

    let no_item = lw(work_dir.path(), &["logs", "7"]).await;
    let no_run = lw(work_dir.path(), &["logs", "1", "--run", "1/rework-1"]).await;
    let other_item = lw(work_dir.path(), &["logs", "7", "--run", "1/implement-1"]).await;
    let neither = lw(work_dir.path(), &["logs"]).await;

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
        let shown = lw(work_dir.path(), &[&["logs", "1"], way].concat()).await;
        let followed = lw(work_dir.path(), &[&["logs", "1", "--follow"], way].concat()).await;

        assert_eq!(followed.code, Some(0), "{}", followed.stderr);
        assert_eq!(followed.stdout, shown.stdout, "{way:?}");
    }
}
