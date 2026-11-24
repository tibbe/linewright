//! Each back-door state is what its stage leaves, checked by running the
//! stage from the same start.

use std::num::NonZeroU32;
use std::path::Path;

use linewright_core::clock::Clock;
use linewright_core::domain::{CheckState, Requirement};
use linewright_core::testing::captures;
use linewright_core::testing::*;
use serde_json::json;

#[test_log::test(tokio::test)]
async fn a_ticket_in_review_is_what_the_implement_stage_leaves() {
    let mut front = World::new().await;
    front.implement_a_ticket().await;
    let mut back = World::new().await;

    back.ticket_in_review().await;

    assert_same_state(&front.store, &back.store).await;
}

#[test_log::test(tokio::test)]
async fn a_ticket_in_review_closing_another_issue_is_what_the_implement_stage_leaves() {
    let mut front = World::new().await;
    implement_closing(&front, open_old_invoices(&front));
    open_ticket(&front, "Do the thing");
    front.poll().await;
    let mut back = World::new().await;
    let other = open_old_invoices(&back);

    back.ticket_in_review_closing(other).await;

    assert_same_state(&front.store, &back.store).await;
}

/// Opens an issue a pull request may close besides its ticket.
fn open_old_invoices(world: &World) -> linewright_core::domain::Number {
    world
        .store
        .open_issue("maintainer", "Old invoices", "Please.", &[])
}

#[test_log::test(tokio::test)]
async fn an_issue_triaged_for_the_maintainer_is_what_triage_leaves() {
    let mut front = World::new().await;
    triage_for_the_maintainer(&front);
    front
        .store
        .open_issue("reporter", "Resize widgets", "", &["needs-triage"]);
    front.poll().await;
    let back = World::new().await;

    back.issue_triaged_for_the_maintainer().await;

    assert_same_state(&front.store, &back.store).await;
}

#[test_log::test(tokio::test)]
async fn an_issue_asking_its_reporter_is_what_triage_leaves() {
    let mut front = World::new().await;
    triage_asking(&front, json!(["reporter"]), "Which version, @reporter?");
    front
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);
    front.poll().await;
    let back = World::new().await;

    back.issue_asking_its_reporter().await;

    assert_same_state(&front.store, &back.store).await;
}

/// A world whose spec change of the spec and of code a poll took up, and
/// the next designed.
async fn spec_change_designed() -> World {
    let mut world = World::new().await;
    tickets_script(&world);
    implement_gadgets(&world);
    merge_spec_and_code_change(&world).await;
    world.poll().await;
    world.poll().await;
    world
}

#[test_log::test(tokio::test)]
async fn a_spec_change_cut_into_tickets_is_what_design_leaves() {
    let front = spec_change_designed().await;
    let back = World::new().await;

    back.spec_change_cut_into_tickets().await;

    assert_same_state(&front.store, &back.store).await;
}

#[test_log::test(tokio::test)]
async fn a_spec_change_ticket_in_review_is_what_design_then_the_implement_stage_leave() {
    let mut front = spec_change_designed().await;
    front.poll().await;
    let back = World::new().await;

    back.spec_change_ticket_in_review().await;

    assert_same_state(&front.store, &back.store).await;
}

/// A world whose `ready-for-agent` issue a poll designed.
async fn issue_designed() -> World {
    let mut world = World::new().await;
    tickets_script(&world);
    implement_gadgets(&world);
    open_undesigned_gadgets(&world);
    world.poll().await;
    world
}

#[test_log::test(tokio::test)]
async fn an_issue_cut_into_tickets_is_what_design_leaves() {
    let front = issue_designed().await;
    let back = World::new().await;

    back.issue_cut_into_tickets().await;

    assert_same_state(&front.store, &back.store).await;
}

#[test_log::test(tokio::test)]
async fn an_issue_ticket_in_review_is_what_design_then_the_implement_stage_leave() {
    let mut front = issue_designed().await;
    front.poll().await;
    let back = World::new().await;

    back.issue_ticket_in_review().await;

    assert_same_state(&front.store, &back.store).await;
}

#[test_log::test(tokio::test)]
async fn an_issue_designed_on_the_issue_is_what_design_leaves() {
    let mut front = World::new().await;
    design_issues(&front);
    open_undesigned_gadgets(&front);
    front.poll().await;
    let back = World::new().await;
    let issue = open_undesigned_gadgets(&back);

    back.designed_on_the_issue(issue).await;

    assert_same_state(&front.store, &back.store).await;
}

/// Relabels a ticket handed back stalled, after its session timed out,
/// `ready-for-agent`.
async fn stalled_ticket_relabeled(world: &World) -> linewright_core::domain::Number {
    let (ticket, _) = world.ticket_timed_out_with_edits().await;
    relabel_ready_for_agent(world, ticket);
    ticket
}

/// Polls `world` until the ticket [`stalled_ticket_relabeled`] leaves is
/// designed: its claim, the clear of its stall and its design take a poll
/// each.
async fn poll_until_stalled_ticket_designed(world: &mut World) {
    for _ in 0..3 {
        world.poll().await;
    }
}

#[test_log::test(tokio::test)]
async fn a_stalled_ticket_designed_on_the_issue_is_what_design_leaves() {
    let mut front = World::new().await;
    stalled_ticket_relabeled(&front).await;
    design_issues(&front);
    poll_until_stalled_ticket_designed(&mut front).await;
    let back = World::new().await;
    let ticket = stalled_ticket_relabeled(&back).await;

    back.designed_on_the_issue(ticket).await;

    assert_same_state(&front.store, &back.store).await;
}

/// A world that ran the implement stage on a ticket opened by
/// [`open_designed_gadgets`], with each implement run scripted by `script`.
async fn gadgets_implemented(script: fn(&World)) -> World {
    let mut world = World::new().await;
    script(&world);
    open_designed_gadgets(&world);
    world.poll().await;
    world
}

#[test_log::test(tokio::test)]
async fn a_ticket_handed_back_with_its_branch_is_what_the_implement_stage_leaves() {
    let front = gadgets_implemented(implement_giving_up_with_earlier_work).await;
    let back = World::new().await;

    back.ticket_handed_back_with_its_branch().await;

    assert_same_state(&front.store, &back.store).await;
}

#[test_log::test(tokio::test)]
async fn a_ticket_timed_out_with_edits_is_what_the_implement_stage_leaves() {
    let front = gadgets_implemented(implement_timing_out_with_edits).await;
    let back = World::new().await;

    back.ticket_timed_out_with_edits().await;

    assert_same_state(&front.store, &back.store).await;
}

#[test_log::test(tokio::test)]
async fn a_ticket_blocked_filing_a_spec_gap_is_what_the_implement_stage_leaves() {
    let front = gadgets_implemented(implement_blocked_filing_a_spec_gap).await;
    let back = World::new().await;

    back.ticket_blocked_filing_a_spec_gap().await;

    assert_same_state(&front.store, &back.store).await;
}

#[test_log::test(tokio::test)]
async fn a_pull_request_ready_for_review_is_what_a_poll_leaves_once_cis_grace_has_passed() {
    let mut front = World::new().await;
    front.ticket_in_review().await;
    front.clock.advance(chrono::TimeDelta::minutes(6));
    front.poll().await;
    let mut back = World::new().await;
    let (_, pr) = back.ticket_in_review().await;

    back.ready_for_review(pr).await;

    assert_same_state(&front.store, &back.store).await;
}

#[test_log::test(tokio::test)]
async fn a_pull_request_held_for_a_code_owners_review_is_what_auto_merge_leaves() {
    let mut front = World::with(|config| config.auto_merge = AUTO_MERGE).await;
    let (_, front_pr) = front.ticket_in_review().await;
    front.store.block_merge(
        front_pr,
        vec![Requirement::CodeOwnerReview {
            owners: vec!["alice".into()],
        }],
    );
    front.clock.advance(chrono::TimeDelta::minutes(6));
    front.poll().await;
    front.poll().await;
    let mut back = World::with(|config| config.auto_merge = AUTO_MERGE).await;
    let (_, pr) = back.ticket_in_review().await;

    back.held_for_a_code_owners_review(pr).await;

    assert_same_state(&front.store, &back.store).await;
}

#[test_log::test(tokio::test)]
async fn a_rework_timed_out_with_edits_is_what_the_rework_stage_leaves() {
    let mut front = World::new().await;
    let (_, pr) = front.ticket_in_review().await;
    front.rework_asked(pr);
    rework_timing_out_with_edits(&front);
    front.poll().await;
    let mut back = World::new().await;

    back.rework_timed_out_with_edits().await;

    assert_same_state(&front.store, &back.store).await;
}

#[test_log::test(tokio::test)]
async fn a_pull_request_ready_for_review_on_green_checks_is_what_a_poll_leaves() {
    let mut front = World::new().await;
    let (_, front_pr) = front.ticket_in_review().await;
    front
        .store
        .set_checks(front_pr, vec![check("test", CheckState::Passed, "")])
        .await
        .unwrap();
    front.poll().await;
    let mut back = World::new().await;
    let (_, pr) = back.ticket_in_review().await;

    back.ready_for_review_on_green_checks(pr).await;

    assert_same_state(&front.store, &back.store).await;
}

#[test_log::test(tokio::test)]
async fn a_pull_request_reworked_is_what_the_rework_stage_leaves() {
    let mut front = World::new().await;
    let (_, front_pr) = front.ticket_in_review().await;
    front.rework_asked(front_pr);
    front.poll().await;
    let mut back = World::new().await;
    let (_, pr) = back.ticket_in_review().await;

    back.reworked(pr).await;

    assert_same_state(&front.store, &back.store).await;
}

#[test_log::test(tokio::test)]
async fn a_recorded_run_is_what_the_implement_stage_records() {
    let mut front = World::new().await;
    implement_gadgets(&front);
    open_designed_gadgets(&front);
    front.poll().await;
    let back = World::new().await;

    let run = captures::implemented(&back).await;

    assert_same_rows(
        &front.work_dir,
        &back.work_dir,
        "SELECT runs.kind, runs.item, runs.stage, runs.automation, runs.n, runs.started_by, \
         runs.workflow_run, runs.run_attempt, runs.started_at, runs.ended_at, runs.ending, \
         runs.answered_until, sessions.seq, sessions.dir \
         FROM runs LEFT JOIN sessions ON sessions.run = runs.id ORDER BY runs.id, sessions.seq",
    )
    .await;
    assert_same_input(&front, &back, &run).await;
}

/// The criteria the task-directory test's task is given.
const GADGETS_CRITERIA: &str = "Gadgets exist.\n";

#[test_log::test(tokio::test)]
async fn a_task_directory_is_what_a_capture_leaves() {
    let mut front = World::new().await;
    implement_gadgets(&front);
    let front_ticket = open_designed_gadgets(&front);
    front.poll().await;
    let front_run = format!("{}/implement-1", front_ticket.0);
    let captured = captures::capture(&front, &front_run, "gadgets", GADGETS_CRITERIA)
        .await
        .unwrap_or_else(|e| panic!("capturing: {e:#}"));
    let back = World::new().await;
    let (ticket, run, [main, branch]) = captures::gadgets_claimed(&back).await;
    let dir = back.dir.path().join("task");

    keep_task_dir(
        &dir,
        &back.store,
        &run,
        back.clock.now(),
        &[ticket],
        &[&main, &branch],
        GADGETS_CRITERIA,
    )
    .await;

    assert_same_tree(&captured.dir, &dir);
}

/// The files of the configuration `short`: a setting and a file of the
/// brief.
const SHORT: &[(&str, &str)] = &[
    ("config.toml", "model = \"haiku\"\n"),
    ("briefs/plugin/skills/tdd/SKILL.md", "Test first.\n"),
];

/// A work directory holding the implement benchmark `board`, 1, of the tasks
/// `lanes`, 1, and `columns`, 2, whose directory holds facts of its run
/// beside its criteria.
async fn board() -> tempfile::TempDir {
    let work_dir = tempfile::tempdir().unwrap();
    runs_in(work_dir.path()).await;
    keep_benchmark(work_dir.path(), "board", "implement").await;
    keep_task(
        work_dir.path(),
        "board",
        "lanes",
        "4/implement-1",
        "Stacks them.\n",
    )
    .await;
    let columns = keep_task(
        work_dir.path(),
        "board",
        "columns",
        "3/implement-2",
        "Names them.\n",
    )
    .await;
    write_files(
        &work_dir.path().join(format!("tasks/{columns}")),
        &[
            ("items.json", "[]\n"),
            ("repository/HEAD", "ref: refs/heads/main\n"),
        ],
    );
    work_dir
}

/// Launches a benchmark run on `board` in `work_dir` at noon, under `as-is`
/// and `short`, twice, and gives it with its benchmark runs.
async fn launch_board(
    work_dir: &Path,
) -> (
    linewright_core::benchmarks::BenchmarkRuns,
    linewright_core::benchmarks::BenchmarkRunId,
) {
    let short = linewright_core::trial::Configuration::parse(
        SHORT
            .iter()
            .map(|(path, contents)| (path.to_string(), contents.as_bytes().to_vec()))
            .collect(),
    )
    .unwrap();
    let benchmark_runs =
        linewright_core::benchmarks::BenchmarkRuns::new(runs_in(work_dir).await.database().clone());
    let launched = benchmark_runs
        .launch(
            "1".parse().unwrap(),
            vec![("short".parse().unwrap(), short)],
            NonZeroU32::new(2).unwrap(),
            "2026-11-17T12:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap_or_else(|e| panic!("launching: {e:#}"));
    (benchmark_runs, launched.id)
}

/// Keeps through the back door in `work_dir` the benchmark run
/// [`launch_board`] launches.
async fn keep_board_run(work_dir: &Path) {
    keep_benchmark_run(
        work_dir,
        &KeptBenchmarkRun {
            id: 1,
            benchmark: 1,
            launched_at: "2026-11-17T12:00:00Z",
            repetitions: NonZeroU32::new(2).unwrap(),
            tasks: &[1, 2],
            configurations: &[("short", SHORT)],
        },
    )
    .await;
}

/// Asserts that the benchmark-run tables of the work directory `front` and
/// the benchmark runs' directories in it hold what `back`'s do.
async fn assert_same_benchmark_runs(front: &Path, back: &Path) {
    for query in [
        "SELECT * FROM benchmark_runs ORDER BY id",
        "SELECT * FROM benchmark_run_tasks ORDER BY benchmark_run, seq",
        "SELECT * FROM benchmark_run_configurations ORDER BY benchmark_run, seq",
        "SELECT * FROM trials ORDER BY benchmark_run, repetition, task_seq, configuration_seq",
    ] {
        assert_same_rows(front, back, query).await;
    }
    assert_same_tree(&front.join("benchmark-runs"), &back.join("benchmark-runs"));
}

#[test_log::test(tokio::test)]
async fn a_launched_benchmark_run_is_what_a_launch_leaves() {
    let front = board().await;
    launch_board(front.path()).await;
    let back = board().await;

    keep_board_run(back.path()).await;

    assert_same_benchmark_runs(front.path(), back.path()).await;
}

#[test_log::test(tokio::test)]
async fn a_cancelled_benchmark_run_is_what_a_cancel_leaves() {
    let front = board().await;
    let (benchmark_runs, id) = launch_board(front.path()).await;
    benchmark_runs
        .cancel(id, "2026-11-17T13:00:00Z".parse().unwrap())
        .await
        .unwrap_or_else(|e| panic!("cancelling: {e:#}"));
    let back = board().await;
    keep_board_run(back.path()).await;

    benchmark_run_ended(
        back.path(),
        1,
        BenchmarkRunEnd::Cancelled("2026-11-17T13:00:00Z"),
    )
    .await;

    assert_same_benchmark_runs(front.path(), back.path()).await;
}

/// Ends each trial of the benchmark run [`launch_board`] launches in
/// `work_dir`, as a run of it under `serve` would.
async fn trials_ended(work_dir: &Path) {
    for repetition in 1..=2 {
        for task in 0..2 {
            for configuration in 0..2 {
                trial(
                    work_dir,
                    1,
                    (repetition, task, configuration),
                    "state = 'unscored', outcome = 'failed', seconds = 5, unscored = 'outcome'",
                )
                .await;
            }
        }
    }
}

#[test_log::test(tokio::test)]
async fn an_ended_benchmark_run_is_what_an_end_leaves() {
    let front = board().await;
    let (benchmark_runs, id) = launch_board(front.path()).await;
    trials_ended(front.path()).await;
    benchmark_runs
        .end(id, "2026-11-17T13:00:00Z".parse().unwrap())
        .await
        .unwrap_or_else(|e| panic!("ending: {e:#}"));
    let back = board().await;
    keep_board_run(back.path()).await;
    trials_ended(back.path()).await;

    benchmark_run_ended(
        back.path(),
        1,
        BenchmarkRunEnd::Ended("2026-11-17T13:00:00Z"),
    )
    .await;

    assert_same_benchmark_runs(front.path(), back.path()).await;
}
