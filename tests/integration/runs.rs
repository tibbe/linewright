use crate::common;

use std::path::Path;

use chrono::TimeDelta;
use common::*;
use linewright::domain::Number;
use linewright::reconcile::WorkKind;
use linewright::runs::{
    self, Ending, Listing, NoRuns, Query, Recorded, RunId, RunOf, Runs, Started, StartedBy, State,
};
use linewright::work_settings::{Partial, StageTable};
use serde_json::json;

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
    let (mut world, work_dir) = world_in_work_dir().await;
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs"]).await;

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

/// A world whose triage sessions run with the operator's `opus`, leaving
/// effort to the harness.
async fn triage_on_opus() -> (World, tempfile::TempDir) {
    let work_dir = tempfile::tempdir().unwrap();
    let mut world = World::with(|config| {
        config.work_dir = work_dir.path().join(REPO);
        config.work.stages.insert(
            WorkKind::Triage,
            StageTable {
                settings: Partial {
                    model: Some("opus".into()),
                    ..Partial::default()
                },
                session_env: None,
            },
        );
    })
    .await;
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;
    (world, work_dir)
}

#[tokio::test]
async fn an_items_runs_show_each_sessions_model_and_effort() {
    let (_world, work_dir) = triage_on_opus().await;

    let output = lw_in(work_dir.path(), &["runs", "1"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}", output.stdout);
    assert!(
        lines[0].starts_with("1/triage-1: done"),
        "{}",
        output.stdout
    );
    assert_eq!(
        lines[1],
        "  session 1: model opus, effort the harness's choice"
    );
}

#[tokio::test]
async fn an_items_runs_as_json_give_where_each_setting_and_prompt_file_came_from() {
    let (_world, work_dir) = triage_on_opus().await;

    let output = lw_in(work_dir.path(), &["runs", "1", "--json"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let answer: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    let session = &answer["runs"][0]["sessions"][0];
    assert_eq!(session["model"], "opus");
    assert_eq!(session["effort"], serde_json::Value::Null);
    assert_eq!(
        session["layers"]["settings"],
        json!({
            "model": "operator",
            "effort": "linewright",
            "session-timeout": "operator",
            "session-env": "linewright",
        })
    );
    assert_eq!(session["layers"]["prompts"]["triage.md"], "linewright");
    assert_eq!(session["layers"]["prompts"].get("prompts/triage.md"), None);
}

#[tokio::test]
async fn the_whole_list_of_runs_leaves_their_sessions_out() {
    let (_world, work_dir) = triage_on_opus().await;

    let output = lw_in(work_dir.path(), &["runs"]).await;
    let json = lw_in(work_dir.path(), &["runs", "--json"]).await;

    assert_eq!(output.stdout.lines().count(), 1, "{}", output.stdout);
    let answer: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    assert_eq!(answer["runs"][0].get("sessions"), None);
}

#[tokio::test]
async fn what_a_runs_sessions_ran_with_is_kept_across_a_restart() {
    let (mut world, work_dir) = triage_on_opus().await;

    let config = common::config(&work_dir.path().join(REPO));
    world.factory = linewright::factory::Factory::start(
        world.store.clone(),
        world.harness.clone(),
        world.clock.clone(),
        config,
    )
    .await
    .unwrap();
    let output = lw_in(work_dir.path(), &["runs", "1"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(
        lines[1..],
        ["  session 1: model opus, effort the harness's choice"],
        "{}",
        output.stdout
    );
}

#[tokio::test]
async fn run_ids_number_each_stage_on_an_item_on_from_before_a_restart() {
    let (mut world, work_dir) = world_in_work_dir().await;
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

    let output = lw_in(work_dir.path(), &["runs"]).await;

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
    let (mut world, work_dir) = world_in_work_dir().await;
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

    let output = lw_in(work_dir.path(), &["runs"]).await;
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
    let (world, work_dir) = world_in_work_dir().await;
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

    let output = lw_in(work_dir.path(), &["runs"]).await;

    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}", output.stdout);
    assert!(
        lines[0].starts_with("weekly/2: failed — automation weekly by hand, started "),
        "{}",
        output.stdout
    );
    assert!(
        lines[1].starts_with("weekly/1: done — automation weekly by hand, started "),
        "{}",
        output.stdout
    );
}

async fn hourly_by_schedule_then_by_hand() -> (World, tempfile::TempDir) {
    let (mut world, work_dir) = world_in_work_dir().await;
    world.hourly_by_schedule_then_by_hand().await;
    (world, work_dir)
}

#[tokio::test]
async fn an_automations_run_by_hand_is_marked_as_one_and_a_scheduled_one_is_not() {
    let (_world, work_dir) = hourly_by_schedule_then_by_hand().await;

    let output = lw_in(work_dir.path(), &["runs"]).await;

    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}", output.stdout);
    assert!(
        lines[0]
            .starts_with("hourly/2: done — automation hourly by hand, started 2026-10-01 13:05:"),
        "{}",
        output.stdout
    );
    assert!(
        lines[1].starts_with("hourly/1: done — automation hourly, started 2026-10-01 13:00:"),
        "{}",
        output.stdout
    );
}

#[tokio::test]
async fn runs_json_says_how_each_automation_run_started_and_nothing_of_work_runs() {
    let (mut world, work_dir) = hourly_by_schedule_then_by_hand().await;
    triage_ready(&world);
    let issue =
        world
            .store
            .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs", "--json"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let answer: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    let triage = format!("{}/triage-1", issue.0);
    let started_by: Vec<(&str, Option<&str>)> = answer["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| {
            let run = run.as_object().unwrap();
            (
                run["id"].as_str().unwrap(),
                run.get("started_by").map(|by| by.as_str().unwrap()),
            )
        })
        .collect();
    assert_eq!(
        started_by,
        [
            (triage.as_str(), None),
            ("hourly/2", Some("hand")),
            ("hourly/1", Some("schedule")),
        ]
    );
}

#[tokio::test]
async fn a_run_handed_back_says_so() {
    let (mut world, work_dir) = world_in_work_dir().await;
    world.harness.on(WorkKind::Implement, |_| {
        Ok(json!({"outcome": "gave-up", "reason": "It cannot be done."}).to_string())
    });
    world.store.open_issue(
        "maintainer",
        "Do it",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs", "1"]).await;

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
    let (mut world, work_dir) = world_in_work_dir().await;
    implement(&world);
    world.store.refuse_pushes(&rejected(
        "refusing to allow a Personal Access Token to create or update workflow \
         `.github/workflows/ci.yml` without `workflow` scope",
    ));
    world.store.open_issue(
        "maintainer",
        "Do it",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs", "1"]).await;

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
    let (mut world, work_dir) = world_in_work_dir().await;
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs", "7"]).await;
    let narrowed = lw_in(work_dir.path(), &["runs", "1", "--stage", "rework"]).await;

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
    let (mut world, work_dir) = world_in_work_dir().await;
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs", "--json"]).await;

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
    let text = lw_in(dir, &["runs", &item.0.to_string()]).await;
    let json = lw_in(dir, &["runs", &item.0.to_string(), "--json"]).await;

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
    let (mut world, work_dir) = world_in_work_dir().await;
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
    let (mut world, work_dir) = world_in_work_dir().await;
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
    let (mut world, work_dir) = world_in_work_dir().await;
    let (issue, closed) = world.ticket_in_review().await;
    world.store.close_pull_as(closed).unwrap();
    world.poll().await;
    world.store.unlabel_as(issue, "ready-for-human").unwrap();
    world.store.label_as(issue, "ready-for-agent").unwrap();
    design_issues(&world);
    world.poll().await;
    world.poll().await;
    let next = *world.store.pulls().last().unwrap();
    assert_ne!(next, closed);
    let dir = work_dir.path();

    let of_closed = lw_in(dir, &["runs", &closed.0.to_string()]).await;

    assert_eq!(
        listed(dir, issue).await,
        ["1/implement-2", "1/design-1", "1/implement-1"]
    );
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
    let (mut world, work_dir) = world_in_work_dir().await;
    let (_, pr) = world.ticket_in_review().await;
    world.rework_asked(pr);
    world.poll().await;
    let pr = pr.0.to_string();

    let rework = lw_in(work_dir.path(), &["runs", &pr, "--stage", "rework"]).await;
    let implement = lw_in(work_dir.path(), &["runs", &pr, "--stage", "implement"]).await;
    let later = lw_in(work_dir.path(), &["runs", &pr, "--since", "2026-10-02"]).await;

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

    let output = lw_in(work_dir.path(), &["runs", "5"]).await;

    assert_eq!(
        output.stdout,
        "5/rework-1: done — #5 rework, started 2026-10-14 12:00:00 UTC, ran 1m\n"
    );
}

#[tokio::test]
async fn a_run_recorded_before_sessions_kept_what_they_ran_with_lists_none_of_it() {
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
  "sessions": ["sessions/5/rework-1"]
}"#,
    )
    .unwrap();

    let output = lw_in(work_dir.path(), &["runs", "5"]).await;
    let json = lw_in(work_dir.path(), &["runs", "5", "--json"]).await;

    assert_eq!(
        output.stdout,
        "5/rework-1: done — #5 rework, started 2026-10-14 12:00:00 UTC, ran 1m\n\
         \x20 session 1: model and effort not recorded\n"
    );
    let answer: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    assert_eq!(answer["runs"][0]["sessions"], json!([null]));
}

fn ended(of: RunOf, n: u32, by: Option<StartedBy>, start: &str) -> Recorded {
    Recorded::new(
        Started::new(RunId { of, n }, by).unwrap(),
        at(start),
        State::Ended {
            at: at(start) + TimeDelta::minutes(1),
            ending: Ending::Done,
        },
        vec![],
        None,
    )
}

fn work(item: u64, stage: WorkKind, n: u32, start: &str) -> Recorded {
    let of = RunOf::Work {
        item: Number(item),
        stage,
    };
    ended(of, n, None, start)
}

fn weekly(n: u32, start: &str) -> Recorded {
    let of = RunOf::Automation {
        automation: "weekly".parse().unwrap(),
    };
    ended(of, n, Some(StartedBy::Schedule), start)
}

fn morning() -> Vec<Recorded> {
    vec![
        work(1, WorkKind::Triage, 1, "2026-10-14T08:00:00Z"),
        weekly(1, "2026-10-14T09:00:00Z"),
        work(1, WorkKind::Implement, 1, "2026-10-14T10:00:00Z"),
        work(2, WorkKind::Triage, 1, "2026-10-14T10:30:00Z"),
        work(1, WorkKind::Rework, 1, "2026-10-14T11:00:00Z"),
        work(1, WorkKind::Rework, 2, "2026-10-14T12:00:00Z"),
    ]
}

fn ids(listing: &Listing) -> Vec<String> {
    listing.runs.iter().map(|run| run.id.to_string()).collect()
}

#[test]
fn every_run_is_listed_newest_first() {
    let listing = runs::list(morning(), &Query::default(), at("2026-10-14T13:00:00Z")).unwrap();

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

    let listing = runs::list(morning(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(
        ids(&listing),
        ["1/rework-2", "1/rework-1", "1/implement-1", "1/triage-1"]
    );
}

fn ticket_and_pull() -> Vec<Recorded> {
    vec![
        work(1, WorkKind::Triage, 1, "2026-10-14T08:00:00Z"),
        work(1, WorkKind::Implement, 1, "2026-10-14T09:00:00Z"),
        work(3, WorkKind::Rework, 1, "2026-10-14T09:30:00Z"),
        work(2, WorkKind::Triage, 1, "2026-10-14T10:30:00Z"),
        work(3, WorkKind::FixCi, 1, "2026-10-14T11:00:00Z"),
        work(3, WorkKind::Rework, 2, "2026-10-14T12:00:00Z"),
    ]
}

#[test]
fn a_pull_requests_runs_are_its_own() {
    let query = Query {
        item: Some(Number(3)),
        ..Query::default()
    };

    let listing = runs::list(ticket_and_pull(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["3/rework-2", "3/fix-ci-1", "3/rework-1"]);
}

#[test]
fn a_tickets_runs_are_its_own_and_none_of_its_pull_requests() {
    let query = Query {
        item: Some(Number(1)),
        ..Query::default()
    };

    let listing = runs::list(ticket_and_pull(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["1/implement-1", "1/triage-1"]);
}

#[test]
fn an_item_with_no_runs_has_none_to_list() {
    let query = Query {
        item: Some(Number(3)),
        ..Query::default()
    };

    let listed = runs::list(morning(), &query, at("2026-10-14T13:00:00Z"));

    assert_eq!(listed, Err(NoRuns(Number(3))));
}

#[test]
fn stage_lists_only_that_stages_runs() {
    let query = Query {
        stage: Some(WorkKind::Triage),
        ..Query::default()
    };

    let listing = runs::list(morning(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["2/triage-1", "1/triage-1"]);
}

#[test]
fn since_lists_only_the_runs_started_then_or_after() {
    let query = Query {
        since: Some(at("2026-10-14T10:30:00Z")),
        ..Query::default()
    };

    let listing = runs::list(morning(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["1/rework-2", "1/rework-1", "2/triage-1"]);
}

#[test]
fn a_list_cut_to_its_limit_ends_saying_how_many_it_left_out_and_how_to_show_them() {
    let query = Query {
        item: Some(Number(1)),
        limit: Some(1),
        ..Query::default()
    };

    let listing = runs::list(morning(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["1/rework-2"]);
    assert_eq!(listing.left_out, 3);
    assert_eq!(
        runs::render(&listing, false),
        "1/rework-2: done — #1 rework, started 2026-10-14 12:00:00 UTC, ran 1m\n\
         left out: 3 earlier runs; show them with --limit 4\n"
    );
}

/// `docs/specs/design_system.md`, Time, Durations.
#[test]
fn a_run_shows_how_long_it_ran_to_its_two_largest_units() {
    let mut run = work(1, WorkKind::Implement, 1, "2026-10-14T09:00:00Z");
    run.state = State::Ended {
        at: at("2026-10-14T12:02:09Z"),
        ending: Ending::Done,
    };

    let listing = runs::list(vec![run], &Query::default(), at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(
        runs::render(&listing, false),
        "1/implement-1: done — #1 implement, started 2026-10-14 09:00:00 UTC, ran 3h 2m\n"
    );
}

#[test]
fn a_run_in_flight_has_run_until_now_and_one_interrupted_for_no_known_time() {
    let mut in_flight = work(1, WorkKind::Rework, 2, "2026-10-14T12:00:00Z");
    in_flight.state = State::InFlight;
    let mut interrupted = work(1, WorkKind::Rework, 1, "2026-10-14T11:00:00Z");
    interrupted.state = State::Interrupted;

    let listing = runs::list(
        vec![in_flight, interrupted],
        &Query::default(),
        at("2026-10-14T12:30:00Z"),
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
    let (mut world, work_dir) = world_in_work_dir().await;
    triage_ready(&world);
    let issue =
        world
            .store
            .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;
    triage_again(&world, issue);
    world.poll().await;
    let dir = work_dir.path();

    let limited = lw_in(
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
    let later = lw_in(dir, &["runs", "--since", "2026-10-02T00:00:00Z"]).await;
    let other_stage = lw_in(dir, &["runs", "--stage", "fix-ci"]).await;
    let no_stage = lw_in(dir, &["runs", "--stage", "review"]).await;

    let lines: Vec<&str> = limited.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{}{}", limited.stdout, limited.stderr);
    assert!(
        lines[0].starts_with("1/triage-2: done"),
        "{}",
        limited.stdout
    );
    assert_eq!(
        lines[2],
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
    let (world, work_dir) = world_in_work_dir().await;
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

    let output = lw_in(work_dir.path(), &["runs"]).await;

    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}", output.stdout);
    assert!(
        lines[0].starts_with("weekly/2: done — automation weekly by hand, started "),
        "{}",
        output.stdout
    );
    assert!(
        lines[1].starts_with("weekly/1: done — automation weekly, started 2026-09-28"),
        "{}",
        output.stdout
    );
    let json = lw_in(work_dir.path(), &["runs", "--json"]).await;
    let answer: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    assert_eq!(answer["runs"][1]["started_by"], "schedule");
}

#[tokio::test]
async fn a_spec_changes_runs_recorded_as_tickets_by_the_previous_version_are_design_runs() {
    let (mut world, work_dir) = world_in_work_dir().await;
    world.harness.on(WorkKind::Design, |_| {
        Ok(json!({"outcome": "nothing-to-build", "reason": "Nothing to build."}).to_string())
    });
    let pr = world
        .store
        .merge_change(
            "maintainer",
            "Gadgets",
            "Adds gadgets.",
            &[("docs/spec.md", "# Spec\n\nGadgets exist.\n")],
        )
        .await
        .unwrap();
    // Where, and as what, the previous version recorded a spec change's run.
    let old = work_dir
        .path()
        .join(REPO)
        .join(format!("runs/items/{}", pr.0));
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(
        old.join("tickets-1.json"),
        json!({
            "item": pr.0,
            "stage": "tickets",
            "n": 1,
            "started_at": "2026-09-28T06:00:00Z",
            "end": {"at": "2026-09-28T06:05:00Z", "ending": "handed-back"},
            "sessions": [],
        })
        .to_string(),
    )
    .unwrap();
    world.poll().await;

    let output = lw_in(
        work_dir.path(),
        &["runs", &pr.0.to_string(), "--stage", "design"],
    )
    .await;
    let tickets = lw_in(work_dir.path(), &["runs", "--stage", "tickets"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let ids = [2, 1].map(|n| format!("{}/design-{n}", pr.0));
    assert_eq!(run_ids(&output.stdout), ids, "{}", output.stdout);
    assert_eq!(tickets.code, Some(2), "{}", tickets.stderr);
}

#[tokio::test]
async fn an_operators_prompt_is_sent_and_recorded_as_the_operators() {
    let work_dir = tempfile::tempdir().unwrap();
    let operators = prompts_with("follow-ups/parse-error.md", |text| {
        format!("The operator's follow-up.\n\n{text}")
    });
    let mut world = World::with(|config| {
        config.work_dir = work_dir.path().join(REPO);
        config.prompts = linewright::briefs::OperatorPrompts::read(Some(operators.path())).unwrap();
    })
    .await;
    world
        .harness
        .on(WorkKind::Triage, |_| Ok("I think it's a bug.".into()));
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs", "1", "--json"]).await;

    let sent = world.harness.prompts(WorkKind::Triage);
    assert_eq!(sent.len(), 2);
    assert!(
        sent[1].starts_with("The operator's follow-up.\n\n"),
        "{}",
        sent[1]
    );
    let answer: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    let prompts = &answer["runs"][0]["sessions"][0]["layers"]["prompts"];
    assert_eq!(prompts["follow-ups/parse-error.md"], "operator");
    assert_eq!(prompts["triage.md"], "linewright");
}

#[tokio::test]
async fn a_run_recorded_when_skills_came_in_layers_shows_its_layers_as_kept() {
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
  "sessions": [{
    "dir": "sessions/5/rework-1",
    "model": "opus",
    "effort": null,
    "layers": {
      "settings": {"model": "operator", "effort": "linewright"},
      "briefs": {
        "prompts/rework.md": "operator",
        "plugin/skills/tdd/SKILL.md": "linewright"
      }
    }
  }]
}"#,
    )
    .unwrap();

    let output = lw_in(work_dir.path(), &["runs", "5", "--json"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let answer: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(
        answer["runs"][0]["sessions"][0]["layers"]["prompts"],
        json!({
            "prompts/rework.md": "operator",
            "plugin/skills/tdd/SKILL.md": "linewright",
        })
    );
}
