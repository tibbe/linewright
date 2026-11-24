use linewright_core::reconcile::WorkKind;
use linewright_core::testing::*;
use serde_json::json;

use crate::programs::*;

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

#[test_log::test(tokio::test)]
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

fn triage_again(world: &World, issue: linewright_core::domain::Number) {
    world.store.unlabel_as(issue, "ready-for-agent").unwrap();
    world
        .store
        .label_as("maintainer", issue, "needs-triage")
        .unwrap();
}

/// A world whose triage sessions run with the operator's model, on a
/// harness that reports running `claude-opus-5-5-20261101`.
async fn triage_reporting_opus() -> (World, tempfile::TempDir) {
    let (mut world, work_dir) = world_in_work_dir().await;
    world.harness.reports_model("claude-opus-5-5-20261101");
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;
    (world, work_dir)
}

#[test_log::test(tokio::test)]
async fn a_session_records_the_model_and_effort_set_where_they_came_from_and_the_model_the_harness_reported()
 {
    let (_world, work_dir) = triage_reporting_opus().await;

    let json = lw_in(work_dir.path(), &["runs", "1", "--json"]).await;

    assert_eq!(json.code, Some(0), "{}", json.stderr);
    let answer: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    let session = &answer["runs"][0]["sessions"][0];
    assert_eq!(session["model"], MODEL);
    assert_eq!(session["reported_model"], "claude-opus-5-5-20261101");
    assert_eq!(session["effort"], "medium");
    assert_eq!(session["layers"]["settings"]["model"], "operator");
    assert_eq!(session["layers"]["settings"]["effort"], "linewright");
}

#[test_log::test(tokio::test)]
async fn runs_of_an_item_with_none_exits_4_saying_so() {
    let (mut world, work_dir) = world_in_work_dir().await;
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs", "7"]).await;

    assert_eq!(output.code, Some(4), "{}", output.stderr);
    assert_eq!(output.stdout, "");
    assert!(
        output.stderr.contains("#7 has no runs"),
        "{}",
        output.stderr
    );
}

#[test_log::test(tokio::test)]
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
    assert_eq!(answer["runs"][0]["id"], "1/triage-1");
}

#[test_log::test(tokio::test)]
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

    let runs: Vec<&str> = limited
        .stdout
        .lines()
        .filter_map(|line| line.split_once(':').map(|(run, _)| run))
        .filter(|run| run.contains('/'))
        .collect();
    assert_eq!(runs, ["1/triage-2"], "{}{}", limited.stdout, limited.stderr);
    assert_eq!((later.code, later.stdout.as_str()), (Some(0), ""));
    assert_eq!(
        (other_stage.code, other_stage.stdout.as_str()),
        (Some(0), "")
    );
    assert_eq!(no_stage.code, Some(2), "{}", no_stage.stderr);
}

#[test_log::test(tokio::test)]
async fn an_operators_part_is_sent_and_recorded_as_the_operators() {
    let work_dir = tempfile::tempdir().unwrap();
    let operators = tempfile::tempdir().unwrap();
    std::fs::create_dir(operators.path().join("parts")).unwrap();
    std::fs::write(
        operators.path().join("parts/result-no-json.md"),
        "The operator's own words.\n",
    )
    .unwrap();
    let mut world = World::with(|config| {
        config.work_dir = work_dir.path().join(REPO);
        config.briefs =
            linewright_core::briefs::OperatorBriefs::read_over(markers(), Some(operators.path()))
                .unwrap();
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
    assert_eq!(sent[1], could_not_be_used("The operator's own words."));
    let answer: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    let briefs = &answer["runs"][0]["sessions"][0]["layers"]["briefs"];
    assert_eq!(briefs["parts/result-no-json.md"], "operator");
    assert_eq!(briefs["prompts/follow-ups/result-error.md"], "linewright");
}

#[test_log::test(tokio::test)]
async fn lw_runs_where_there_is_no_database_lists_none_and_makes_none() {
    let (base, _work_dir) = work_dir_holding(&[(
        "runs/items/1/triage-1.json",
        &item_run_record(1, "triage", 1, Some("done"), &json!([])),
    )]);

    let output = lw_in(base.path(), &["runs"]).await;

    assert_eq!(
        (output.code, output.stdout.as_str()),
        (Some(0), ""),
        "{}",
        output.stderr
    );
    assert!(!base.path().join(REPO).join("linewright.db").exists());
}

#[test_log::test(tokio::test)]
async fn lw_runs_on_a_database_a_newer_release_made_exits_1_naming_that_release() {
    let (base, work_dir) = work_dir_holding(&[]);
    drop(runs_in(&work_dir.resolve()).await);
    let pool = back_door(&work_dir.resolve()).await;
    sqlx::raw_sql("PRAGMA user_version = 999; INSERT INTO releases VALUES (999, '99.0.0')")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let output = lw_in(base.path(), &["runs"]).await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert!(
        output.stderr.contains(
            "was made by Linewright 99.0.0, a newer release: to run this release, restore a backup"
        ),
        "{}",
        output.stderr
    );
}
