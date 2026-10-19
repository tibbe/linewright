use crate::common;

use std::path::Path;

use chrono::TimeDelta;
use common::*;
use linewright::domain::{Number, Timestamp};
use linewright::reconcile::WorkKind;
use linewright::runs::{self, Ending, Listing, NoRuns, Query, Recorded, RunId, RunOf, Runs, State};
use serde_json::json;

const REPO: &str = "owner/widgets";

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

fn triage_ready(world: &World) {
    world.harness.on(WorkKind::Triage, |_| {
        Ok(json!({
            "outcome": "ready-for-agent",
            "category": "bug",
            "comment": "## Agent Brief\n\n**Summary:** It crashes."
        })
        .to_string())
    });
}

#[tokio::test]
async fn a_triage_run_is_listed_under_its_run_id_with_its_outcome() {
    let (mut world, work_dir) = world().await;
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let output = lw(work_dir.path(), &["runs"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 1, "{}", output.stdout);
    assert!(
        lines[0].starts_with("1/triage-1: done — #1 triage, started 2026-10-01 12:00:"),
        "{}",
        output.stdout
    );
}

fn triage_again(world: &World, issue: linewright::domain::Number) {
    world.store.unlabel_as(issue, "ready-for-agent").unwrap();
    world.store.label_as(issue, "needs-triage").unwrap();
}

#[tokio::test]
async fn run_ids_number_each_stage_on_an_item_on_from_before_a_restart() {
    let (mut world, work_dir) = world().await;
    triage_ready(&world);
    let issue =
        world
            .store
            .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;
    triage_again(&world, issue);
    world.poll().await;

    let config = common::config(&work_dir.path().join(REPO));
    world.factory = linewright::factory::Factory::start(
        world.store.clone(),
        world.harness.clone(),
        world.clock.clone(),
        config,
    )
    .await
    .unwrap();
    triage_again(&world, issue);
    world.poll().await;

    let output = lw(work_dir.path(), &["runs"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    assert_eq!(
        run_ids(&output.stdout),
        ["1/triage-3", "1/triage-2", "1/triage-1"],
        "{}",
        output.stdout
    );
}

#[tokio::test]
async fn a_run_that_starts_no_session_is_recorded_with_none_linked() {
    let (mut world, work_dir) = world().await;
    world.ticket_in_review().await;
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

    let output = lw(work_dir.path(), &["runs"]).await;
    let recorded = Runs::new(&work_dir.path().join(REPO)).read().unwrap();

    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}", output.stdout);
    assert!(
        lines[0].starts_with("2/update-1: done — #2 update, started "),
        "{}",
        output.stdout
    );
    assert!(
        lines[1].starts_with("1/implement-1: done — "),
        "{}",
        output.stdout
    );
    let sessions = |id: &str| {
        let run = recorded
            .iter()
            .find(|run| run.id.to_string() == id)
            .unwrap();
        run.sessions.len()
    };
    assert_eq!((sessions("2/update-1"), sessions("1/implement-1")), (0, 1));
}

#[tokio::test]
async fn an_automations_runs_are_numbered_under_its_name_with_their_outcomes() {
    let (world, work_dir) = world().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(
                ".linewright/automations/weekly.md",
                "---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n",
            )],
            "Add an automation",
        )
        .await
        .unwrap();
    let weekly = "weekly".parse().unwrap();
    world
        .harness
        .on_automation(|_| Ok(json!({"summary": "Nothing to do.", "issues": []}).to_string()));
    world.factory.run_automation(&weekly).await.unwrap();
    world
        .harness
        .on_automation(|_| Ok(json!({"not": "a result"}).to_string()));
    assert!(world.factory.run_automation(&weekly).await.is_err());

    let output = lw(work_dir.path(), &["runs"]).await;

    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}", output.stdout);
    assert!(
        lines[0].starts_with("weekly/2: failed — automation weekly, started "),
        "{}",
        output.stdout
    );
    assert!(
        lines[1].starts_with("weekly/1: done — automation weekly, started "),
        "{}",
        output.stdout
    );
}

#[tokio::test]
async fn a_run_handed_back_says_so() {
    let (mut world, work_dir) = world().await;
    world.harness.on(WorkKind::Implement, |_| {
        Ok(json!({"outcome": "gave-up", "reason": "It cannot be done."}).to_string())
    });
    world
        .store
        .open_issue("maintainer", "Do it", "Please.", &["ready-for-agent"]);
    world.poll().await;

    let output = lw(work_dir.path(), &["runs", "1"]).await;

    assert!(
        output
            .stdout
            .starts_with("1/implement-1: handed back — #1 implement, "),
        "{}",
        output.stdout
    );
}

#[tokio::test]
async fn a_run_whose_push_is_refused_for_a_missing_permission_is_handed_back() {
    let (mut world, work_dir) = world().await;
    implement(&world);
    world.store.refuse_pushes(&rejected(
        "refusing to allow a Personal Access Token to create or update workflow \
         `.github/workflows/ci.yml` without `workflow` scope",
    ));
    world
        .store
        .open_issue("maintainer", "Do it", "Please.", &["ready-for-agent"]);
    world.poll().await;

    let output = lw(work_dir.path(), &["runs", "1"]).await;

    assert!(
        output
            .stdout
            .starts_with("1/implement-1: handed back — #1 implement, "),
        "{}",
        output.stdout
    );
}

#[tokio::test]
async fn runs_of_an_item_with_none_exits_4_saying_so() {
    let (mut world, work_dir) = world().await;
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let output = lw(work_dir.path(), &["runs", "7"]).await;
    let narrowed = lw(work_dir.path(), &["runs", "1", "--stage", "rework"]).await;

    assert_eq!(output.code, Some(4), "{}", output.stderr);
    assert_eq!(output.stdout, "");
    assert!(
        output.stderr.contains("#7 has no runs"),
        "{}",
        output.stderr
    );
    assert_eq!(narrowed.code, Some(0), "{}", narrowed.stderr);
    assert_eq!(narrowed.stdout, "");
}

#[tokio::test]
async fn runs_json_prints_the_answer_as_json() {
    let (mut world, work_dir) = world().await;
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let output = lw(work_dir.path(), &["runs", "--json"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let answer: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    let run = &answer["runs"][0];
    assert_eq!(run["id"], "1/triage-1");
    assert_eq!(run["item"], 1);
    assert_eq!(run["stage"], "triage");
    assert_eq!(run["outcome"], "done");
    assert!(
        run["started_at"]
            .as_str()
            .unwrap()
            .starts_with("2026-10-01T12:00:")
    );
    assert!(run["duration_seconds"].as_i64().unwrap() > 0);
    assert_eq!(answer["left_out"], 0);
}

async fn listed(dir: &Path, item: Number) -> Vec<String> {
    let text = lw(dir, &["runs", &item.0.to_string()]).await;
    let json = lw(dir, &["runs", &item.0.to_string(), "--json"]).await;

    assert_eq!(text.code, Some(0), "{}", text.stderr);
    let ids: Vec<String> = run_ids(&text.stdout)
        .into_iter()
        .map(str::to_string)
        .collect();
    let answer: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    let json_ids: Vec<String> = answer["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, json_ids);
    ids
}

#[tokio::test]
async fn a_pull_requests_runs_are_its_own_and_its_tickets_are_the_tickets() {
    let (mut world, work_dir) = world().await;
    let (issue, pr) = world.ticket_in_review().await;
    world.rework_asked(pr);
    world.poll().await;
    let dir = work_dir.path();

    assert_eq!(listed(dir, pr).await, [format!("{}/rework-1", pr.0)]);
    assert_eq!(listed(dir, issue).await, ["1/implement-1"]);
    world.store.merge_pull(pr).await.unwrap();
    assert_eq!(listed(dir, pr).await, [format!("{}/rework-1", pr.0)]);
    assert_eq!(listed(dir, issue).await, ["1/implement-1"]);
}

#[tokio::test]
async fn a_pull_requests_runs_are_counted_per_stage_on_it() {
    let (mut world, work_dir) = world().await;
    let (_, pr) = world.ticket_in_review().await;
    world.rework_asked(pr);
    world.poll().await;
    world
        .store
        .comment_as("maintainer", pr, "Rename it again.")
        .unwrap();
    world.harness.on(WorkKind::Rework, |call| {
        commit_in(
            &call.spec.dir,
            &[("src/renamed.txt", "renamed again\n")],
            "Rename again",
        );
        Ok(json!({"outcome": "done", "comment": "Renamed again."}).to_string())
    });
    world.poll().await;

    assert_eq!(
        listed(work_dir.path(), pr).await,
        [format!("{}/rework-2", pr.0), format!("{}/rework-1", pr.0)]
    );
}

#[tokio::test]
async fn a_closed_pull_request_has_no_runs_of_its_tickets() {
    let (mut world, work_dir) = world().await;
    let (issue, closed) = world.ticket_in_review().await;
    world.store.close_pull_as(closed).unwrap();
    world.poll().await;
    world.store.unlabel_as(issue, "ready-for-human").unwrap();
    world.store.label_as(issue, "ready-for-agent").unwrap();
    world.poll().await;
    let next = *world.store.pulls().last().unwrap();
    assert_ne!(next, closed);
    let dir = work_dir.path();

    let of_closed = lw(dir, &["runs", &closed.0.to_string()]).await;

    assert_eq!(listed(dir, issue).await, ["1/implement-2", "1/implement-1"]);
    assert_eq!(of_closed.code, Some(4), "{}", of_closed.stdout);
    assert!(
        of_closed
            .stderr
            .contains(&format!("#{} has no runs", closed.0)),
        "{}",
        of_closed.stderr
    );
}

#[tokio::test]
async fn a_pull_requests_runs_narrow_from_the_command_line() {
    let (mut world, work_dir) = world().await;
    let (_, pr) = world.ticket_in_review().await;
    world.rework_asked(pr);
    world.poll().await;
    let pr = pr.0.to_string();

    let rework = lw(work_dir.path(), &["runs", &pr, "--stage", "rework"]).await;
    let implement = lw(work_dir.path(), &["runs", &pr, "--stage", "implement"]).await;
    let later = lw(work_dir.path(), &["runs", &pr, "--since", "2026-10-02"]).await;

    assert_eq!(
        run_ids(&rework.stdout),
        [format!("{pr}/rework-1")],
        "{}",
        rework.stdout
    );
    assert_eq!((implement.code, implement.stdout.as_str()), (Some(0), ""));
    assert_eq!((later.code, later.stdout.as_str()), (Some(0), ""));
}

#[tokio::test]
async fn a_run_recorded_before_runs_kept_their_pull_request_is_listed() {
    let work_dir = tempfile::tempdir().unwrap();
    let dir = work_dir.path().join(REPO).join("runs/items/5");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("rework-1.json"),
        r#"{
  "item": 5,
  "stage": "rework",
  "n": 1,
  "started_at": "2026-10-14T12:00:00Z",
  "end": {"at": "2026-10-14T12:01:00Z", "ending": "done"},
  "sessions": []
}"#,
    )
    .unwrap();

    let output = lw(work_dir.path(), &["runs", "5"]).await;

    assert_eq!(
        output.stdout,
        "5/rework-1: done — #5 rework, started 2026-10-14 12:00:00 UTC, ran 1m\n"
    );
}

fn at(time: &str) -> Timestamp {
    format!("2026-10-14T{time}Z").parse().unwrap()
}

fn work(item: u64, stage: WorkKind, n: u32, start: &str) -> Recorded {
    Recorded {
        id: RunId {
            of: RunOf::Work {
                item: Number(item),
                stage,
            },
            n,
        },
        started_at: at(start),
        state: State::Ended {
            at: at(start) + TimeDelta::minutes(1),
            ending: Ending::Done,
        },
        sessions: vec![],
        answered_until: None,
    }
}

fn weekly(n: u32, start: &str) -> Recorded {
    Recorded {
        id: RunId {
            of: RunOf::Automation {
                automation: "weekly".parse().unwrap(),
            },
            n,
        },
        ..work(0, WorkKind::Triage, n, start)
    }
}

fn morning() -> Vec<Recorded> {
    vec![
        work(1, WorkKind::Triage, 1, "08:00:00"),
        weekly(1, "09:00:00"),
        work(1, WorkKind::Implement, 1, "10:00:00"),
        work(2, WorkKind::Triage, 1, "10:30:00"),
        work(1, WorkKind::Rework, 1, "11:00:00"),
        work(1, WorkKind::Rework, 2, "12:00:00"),
    ]
}

fn ids(listing: &Listing) -> Vec<String> {
    listing.runs.iter().map(|run| run.id.to_string()).collect()
}

#[test]
fn every_run_is_listed_newest_first() {
    let listing = runs::list(morning(), &Query::default(), at("13:00:00")).unwrap();

    assert_eq!(
        ids(&listing),
        [
            "1/rework-2",
            "1/rework-1",
            "2/triage-1",
            "1/implement-1",
            "weekly/1",
            "1/triage-1"
        ]
    );
    assert_eq!(listing.left_out, 0);
}

#[test]
fn an_items_runs_are_only_its_own() {
    let query = Query {
        item: Some(Number(1)),
        ..Query::default()
    };

    let listing = runs::list(morning(), &query, at("13:00:00")).unwrap();

    assert_eq!(
        ids(&listing),
        ["1/rework-2", "1/rework-1", "1/implement-1", "1/triage-1"]
    );
}

fn ticket_and_pull() -> Vec<Recorded> {
    vec![
        work(1, WorkKind::Triage, 1, "08:00:00"),
        work(1, WorkKind::Implement, 1, "09:00:00"),
        work(3, WorkKind::Rework, 1, "09:30:00"),
        work(2, WorkKind::Triage, 1, "10:30:00"),
        work(3, WorkKind::FixCi, 1, "11:00:00"),
        work(3, WorkKind::Rework, 2, "12:00:00"),
    ]
}

#[test]
fn a_pull_requests_runs_are_its_own() {
    let query = Query {
        item: Some(Number(3)),
        ..Query::default()
    };

    let listing = runs::list(ticket_and_pull(), &query, at("13:00:00")).unwrap();

    assert_eq!(ids(&listing), ["3/rework-2", "3/fix-ci-1", "3/rework-1"]);
}

#[test]
fn a_tickets_runs_are_its_own_and_none_of_its_pull_requests() {
    let query = Query {
        item: Some(Number(1)),
        ..Query::default()
    };

    let listing = runs::list(ticket_and_pull(), &query, at("13:00:00")).unwrap();

    assert_eq!(ids(&listing), ["1/implement-1", "1/triage-1"]);
}

#[test]
fn an_item_with_no_runs_has_none_to_list() {
    let query = Query {
        item: Some(Number(3)),
        ..Query::default()
    };

    let listed = runs::list(morning(), &query, at("13:00:00"));

    assert_eq!(listed, Err(NoRuns(Number(3))));
}

#[test]
fn stage_lists_only_that_stages_runs() {
    let query = Query {
        stage: Some(WorkKind::Triage),
        ..Query::default()
    };

    let listing = runs::list(morning(), &query, at("13:00:00")).unwrap();

    assert_eq!(ids(&listing), ["2/triage-1", "1/triage-1"]);
}

#[test]
fn since_lists_only_the_runs_started_then_or_after() {
    let query = Query {
        since: Some(at("10:30:00")),
        ..Query::default()
    };

    let listing = runs::list(morning(), &query, at("13:00:00")).unwrap();

    assert_eq!(ids(&listing), ["1/rework-2", "1/rework-1", "2/triage-1"]);
}

#[test]
fn a_list_cut_to_its_limit_ends_saying_how_many_it_left_out_and_how_to_show_them() {
    let query = Query {
        item: Some(Number(1)),
        limit: Some(1),
        ..Query::default()
    };

    let listing = runs::list(morning(), &query, at("13:00:00")).unwrap();

    assert_eq!(ids(&listing), ["1/rework-2"]);
    assert_eq!(listing.left_out, 3);
    assert_eq!(
        runs::render(&listing, false),
        "1/rework-2: done — #1 rework, started 2026-10-14 12:00:00 UTC, ran 1m\n\
         left out: 3 earlier runs; show them with --limit 4\n"
    );
}

#[test]
fn a_run_in_flight_has_run_until_now_and_one_interrupted_for_no_known_time() {
    let mut in_flight = work(1, WorkKind::Rework, 2, "12:00:00");
    in_flight.state = State::InFlight;
    let mut interrupted = work(1, WorkKind::Rework, 1, "11:00:00");
    interrupted.state = State::Interrupted;

    let listing = runs::list(
        vec![in_flight, interrupted],
        &Query::default(),
        at("12:30:00"),
    )
    .unwrap();

    assert_eq!(
        runs::render(&listing, false),
        "1/rework-2: in flight — #1 rework, started 2026-10-14 12:00:00 UTC, ran 30m\n\
         1/rework-1: interrupted — #1 rework, started 2026-10-14 11:00:00 UTC\n"
    );
}

#[tokio::test]
async fn the_filters_narrow_the_list_from_the_command_line() {
    let (mut world, work_dir) = world().await;
    triage_ready(&world);
    let issue =
        world
            .store
            .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;
    triage_again(&world, issue);
    world.poll().await;
    let dir = work_dir.path();

    let limited = lw(
        dir,
        &[
            "runs",
            "1",
            "--stage",
            "triage",
            "--since",
            "2026-10-01",
            "--limit",
            "1",
        ],
    )
    .await;
    let later = lw(dir, &["runs", "--since", "2026-10-02T00:00:00Z"]).await;
    let other_stage = lw(dir, &["runs", "--stage", "fix-ci"]).await;
    let no_stage = lw(dir, &["runs", "--stage", "review"]).await;

    let lines: Vec<&str> = limited.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}{}", limited.stdout, limited.stderr);
    assert!(
        lines[0].starts_with("1/triage-2: done"),
        "{}",
        limited.stdout
    );
    assert_eq!(
        lines[1],
        "left out: 1 earlier run; show them with --limit 2"
    );
    assert_eq!((later.code, later.stdout.as_str()), (Some(0), ""));
    assert_eq!(
        (other_stage.code, other_stage.stdout.as_str()),
        (Some(0), "")
    );
    assert_eq!(no_stage.code, Some(2), "{}", no_stage.stderr);
}

#[tokio::test]
async fn an_automations_runs_recorded_by_the_previous_version_still_show_and_are_counted_on_from() {
    let (world, work_dir) = world().await;
    // Where, and as what, the previous version recorded an automation's run.
    let old = work_dir.path().join(REPO).join("runs/workflows/weekly");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(
        old.join("1.json"),
        json!({
            "workflow": "weekly",
            "n": 1,
            "started_at": "2026-09-28T06:00:00Z",
            "end": {"at": "2026-09-28T06:05:00Z", "ending": "done"},
            "sessions": [],
        })
        .to_string(),
    )
    .unwrap();
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(
                ".linewright/automations/weekly.md",
                "---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n",
            )],
            "Add an automation",
        )
        .await
        .unwrap();
    world
        .harness
        .on_automation(|_| Ok(json!({"summary": "Nothing to do.", "issues": []}).to_string()));
    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();

    let output = lw(work_dir.path(), &["runs"]).await;

    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}", output.stdout);
    assert!(
        lines[0].starts_with("weekly/2: done — automation weekly, started "),
        "{}",
        output.stdout
    );
    assert!(
        lines[1].starts_with("weekly/1: done — automation weekly, started 2026-09-28"),
        "{}",
        output.stdout
    );
}
