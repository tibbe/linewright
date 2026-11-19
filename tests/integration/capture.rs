use crate::common;

use std::collections::{BTreeMap, BTreeSet};
use std::panic::Location;
use std::path::{Path, PathBuf};

use common::*;
use linewright::benchmarks::{BenchmarkName, Benchmarks, MakeError};
use linewright::domain::Number;
use linewright::git::{self, CommitId};
use linewright::runs::{RunId, Runs};
use linewright::store::API_VERSION;
use linewright::task::{self, BenchmarkChoice, CaptureError, Criteria, TaskName, TaskStage};
use serde_json::{Value, json};

const SECRET: &str = "s3cr3t-of-the-maintainer";

#[track_caller]
fn runs(world: &World) -> impl Future<Output = Runs> + use<> {
    runs_in(&world.dir.path().join("work"))
}

fn tasks(world: &World) -> PathBuf {
    world.dir.path().join("work").join("cases")
}

#[track_caller]
fn id(id: &str) -> RunId {
    id.parse().unwrap()
}

/// The benchmark of implement tasks the tests capture into.
const IMPLEMENTS: &str = "implements";

#[track_caller]
fn benchmarks(world: &World) -> impl Future<Output = Benchmarks> + use<> {
    let runs = runs(world);
    async move { Benchmarks::new(runs.await.database().clone()) }
}

/// Captures run `run` of `world` as the task `named`, with `criteria`, into
/// the benchmark [`IMPLEMENTS`], made if missing.
#[track_caller]
fn capture<'a>(
    world: &'a World,
    run: &str,
    named: &str,
    criteria: &str,
) -> impl Future<Output = Result<PathBuf, CaptureError>> + 'a {
    let caller = Location::caller();
    let benchmarks = benchmarks(world);
    let made = async move {
        let benchmarks = benchmarks.await;
        match benchmarks
            .make(&benchmark(IMPLEMENTS), TaskStage::Implement)
            .await
        {
            Ok(_) | Err(MakeError::Taken(_)) => {}
            Err(e) => panic!("making the suite at {caller}: {e:#}"),
        }
        benchmarks
    };
    let into = capture_into(
        world,
        run,
        named,
        criteria,
        BenchmarkChoice::Named(benchmark(IMPLEMENTS)),
    );
    Box::pin(async move {
        made.await;
        into.await
    })
}

/// Captures run `run` of `world` as the task `named`, with `criteria`, into
/// the benchmark `benchmark`.
#[track_caller]
fn capture_into<'a>(
    world: &'a World,
    run: &str,
    named: &str,
    criteria: &str,
    benchmark: BenchmarkChoice,
) -> impl Future<Output = Result<PathBuf, CaptureError>> + 'a {
    let named: TaskName = named.parse().unwrap();
    let criteria = Criteria::parse(criteria).unwrap();
    let run = id(run);
    let benchmarks = benchmarks(world);
    Box::pin(async move {
        let benchmarks = benchmarks.await;
        let capturable =
            task::capturable(&runs(world).await, &benchmarks, &run, &named, &benchmark).await?;
        task::capture(&*world.store, capturable, &criteria, &benchmarks).await
    })
}

/// The lines `git` writes with `args` in the repository at `dir`, failing
/// the test at `caller` when it fails.
async fn git_lines_at(caller: &Location<'_>, dir: &Path, args: &[&str]) -> Vec<String> {
    git::git(dir, args)
        .await
        .unwrap_or_else(|e| panic!("git {args:?} at {caller}: {e:#}"))
        .lines()
        .map(str::to_string)
        .collect()
}

/// The head of `branch` in `world`'s store.
async fn head_at(caller: &Location<'_>, world: &World, branch: &str) -> CommitId {
    let rev = format!("refs/heads/{branch}");
    let lines = git_lines_at(caller, &world.store.repository(), &["rev-parse", &rev]).await;
    CommitId::from(lines[0].as_str())
}

/// The names of the entries in `dir`, none when it does not exist.
#[track_caller]
fn entries(dir: &Path) -> BTreeSet<String> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
        Err(e) => panic!("reading {}: {e}", dir.display()),
    }
}

#[track_caller]
fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[track_caller]
fn read_json(path: &Path) -> Value {
    serde_json::from_str(&read(path)).unwrap()
}

/// Takes the properties out of every input recorded in the repository's work
/// directory `work`, as Linewright recorded inputs before it recorded the
/// properties.
#[track_caller]
fn record_as_before_the_properties(work: &Path) -> impl Future<Output = ()> + use<> {
    let caller = Location::caller();
    let pool = back_door(work);
    async move {
        let pool = pool.await;
        sqlx::query("UPDATE run_inputs SET input = json_remove(input, '$.properties')")
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("taking out the properties at {caller}: {e}"));
        pool.close().await;
    }
}

/// Leaves the store's repository holding one commit, made afresh on
/// `main`, and no other.
#[track_caller]
fn forget_every_commit(world: &World) -> impl Future<Output = ()> + '_ {
    let caller = Location::caller();
    async move {
        let repository = world.store.repository();
        let git = async |args: &[&str]| git_lines_at(caller, &repository, args).await;
        for name in git(&["for-each-ref", "--format=%(refname)"]).await {
            git(&["update-ref", "-d", &name]).await;
        }
        let empty_tree = git(&["mktree"]).await;
        let fresh = git(&[
            "-c",
            "user.name=Maintainer",
            "-c",
            "user.email=maintainer@example.com",
            "commit-tree",
            "-m",
            "Afresh",
            &empty_tree[0],
        ])
        .await;
        git(&["update-ref", "refs/heads/main", &fresh[0]]).await;
        git(&["reflog", "expire", "--expire=now", "--all"]).await;
        git(&["gc", "--quiet", "--prune=now"]).await;
    }
}

/// An implement run on a ticket whose branch holds earlier work, which
/// records the head of the default branch and of the ticket's branch: the
/// ticket, its run's ID and those heads.
#[track_caller]
fn implemented_on_a_branch(
    world: &mut World,
) -> impl Future<Output = (Number, String, BTreeMap<String, CommitId>)> + '_ {
    let caller = Location::caller();
    async move {
        let (ticket, branch) = world.ticket_handed_back_with_its_branch().await;
        relabel_ready_for_agent(world, ticket);
        world.designed_on_the_issue(ticket).await;
        let heads = BTreeMap::from([
            ("main".to_string(), head_at(caller, world, "main").await),
            (branch.clone(), head_at(caller, world, &branch).await),
        ]);
        world.poll().await;
        (ticket, format!("{}/implement-1", ticket.0), heads)
    }
}

/// An implement run on the ticket "Gadgets", and its ID.
async fn implemented(world: &mut World) -> String {
    implement_gadgets(world);
    let ticket = open_designed_gadgets(world);
    world.poll().await;
    format!("{}/implement-1", ticket.0)
}

#[test_log::test(tokio::test)]
async fn an_implement_run_is_captured_with_its_items_its_heads_and_its_criteria() {
    let mut world = World::new().await;
    let (ticket, run, heads) = implemented_on_a_branch(&mut world).await;
    let started_at = runs(&world)
        .await
        .read_run(&id(&run))
        .await
        .unwrap()
        .started_at;

    let dir = capture(&world, &run, "gadgets-1", "A good result adds gadgets.\n")
        .await
        .unwrap();

    assert_eq!(dir, tasks(&world).join("gadgets-1"));
    assert_eq!(
        entries(&dir),
        BTreeSet::from(
            [
                "case.json",
                "items.json",
                "properties.json",
                "repository.git",
                "criteria.md"
            ]
            .map(String::from)
        )
    );
    assert_eq!(
        read_json(&dir.join("case.json")),
        json!({
            "stage": "implement",
            "item": ticket.0,
            "start": started_at,
            "run": run,
            "api_version": API_VERSION,
        })
    );
    let items = read_json(&dir.join("items.json"));
    let [item] = items.as_array().unwrap().as_slice() else {
        panic!("not the ticket alone: {items:#}");
    };
    assert_eq!(item["number"], ticket.0);
    assert_eq!(
        item["resources"][format!("issues/{}", ticket.0)]["title"],
        world.issue(ticket).title
    );
    let input = runs(&world)
        .await
        .read_input(&id(&run))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        read_json(&dir.join("properties.json")),
        serde_json::to_value(input.properties.unwrap()).unwrap()
    );
    assert_eq!(
        read_json(&dir.join("properties.json"))["resources"]["account"]["login"],
        LINEWRIGHT
    );
    assert_eq!(
        read(&dir.join("criteria.md")),
        "A good result adds gadgets.\n"
    );
    let refs = git_lines_at(
        Location::caller(),
        &dir.join("repository.git"),
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    )
    .await;
    assert_eq!(
        refs,
        heads
            .iter()
            .map(|(branch, head)| format!("refs/heads/{branch} {head}"))
            .collect::<Vec<_>>()
    );
}

#[test_log::test(tokio::test)]
async fn the_criteria_are_kept_as_given_even_with_a_secret() {
    let mut world = World::new().await;
    let run = implemented(&mut world).await;
    let criteria = format!("It never shows {SECRET}.\n");

    let dir = capture(&world, &run, "gadgets", &criteria).await.unwrap();

    assert_eq!(read(&dir.join("criteria.md")), criteria);
}

#[test_log::test(tokio::test)]
async fn a_run_id_that_names_no_run_is_refused() {
    let world = World::new().await;

    let refused = capture(&world, "7/implement-1", "seven", "Gadgets.\n").await;

    assert!(
        matches!(refused, Err(CaptureError::NoSuchRun(_))),
        "{refused:?}"
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        "there is no run 7/implement-1"
    );
}

#[test_log::test(tokio::test)]
async fn an_automations_run_is_refused() {
    let mut world = World::new().await;
    world.hourly_by_schedule_then_by_hand().await;

    let refused = capture(&world, "hourly/1", "hourly", "Gadgets.\n").await;

    assert!(
        matches!(refused, Err(CaptureError::NotCapturable(_))),
        "{refused:?}"
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        "the run hourly/1 is an automation's run: only a triage, design or implement run can \
         be captured"
    );
    assert_eq!(entries(&tasks(&world)), BTreeSet::new());
}

#[test_log::test(tokio::test)]
async fn a_run_that_recorded_no_input_is_refused() {
    let mut world = World::new().await;
    world.store.fail_recording("GitHub is down");
    let run = implemented(&mut world).await;

    let refused = capture(&world, &run, "gadgets", "Gadgets.\n").await;

    assert!(
        matches!(refused, Err(CaptureError::NoInput(_))),
        "{refused:?}"
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        format!("the run {run} recorded no input, so it cannot be captured")
    );
    assert_eq!(entries(&tasks(&world)), BTreeSet::new());
}

#[test_log::test(tokio::test)]
async fn a_run_recorded_without_its_properties_is_refused_naming_them() {
    let mut world = World::new().await;
    let run = implemented(&mut world).await;
    record_as_before_the_properties(&world.dir.path().join("work")).await;

    let refused = capture(&world, &run, "gadgets", "Gadgets.\n").await;

    assert!(
        matches!(refused, Err(CaptureError::Lacks { .. })),
        "{refused:?}"
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        format!("the run {run}'s record lacks the properties, so it cannot be captured")
    );
    assert_eq!(entries(&tasks(&world)), BTreeSet::new());
}

#[test_log::test(tokio::test)]
async fn a_name_a_task_already_has_is_refused_and_the_task_kept() {
    let mut world = World::new().await;
    let run = implemented(&mut world).await;
    let dir = capture(&world, &run, "gadgets", "First.\n").await.unwrap();

    let refused = capture(&world, &run, "gadgets", "Second.\n").await;

    assert!(
        matches!(refused, Err(CaptureError::Taken(_))),
        "{refused:?}"
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        "a case is already named gadgets"
    );
    assert_eq!(read(&dir.join("criteria.md")), "First.\n");
    assert_eq!(
        entries(&tasks(&world)),
        BTreeSet::from(["gadgets".to_string()])
    );
}

#[test_log::test(tokio::test)]
async fn of_two_captures_under_one_name_one_wins_and_the_other_is_refused() {
    let mut world = World::new().await;
    let run = implemented(&mut world).await;
    let first = capture(&world, &run, "gadgets", "First.\n");
    let second = capture(&world, &run, "gadgets", "Second.\n");

    let (first, second) = tokio::join!(first, second);

    let (won, refused, criteria) = match (first, second) {
        (Ok(won), refused) => (won, refused, "First.\n"),
        (refused, Ok(won)) => (won, refused, "Second.\n"),
        (first, second) => panic!("neither won: {first:?}, {second:?}"),
    };
    assert!(
        matches!(refused, Err(CaptureError::Taken(_))),
        "{refused:?}"
    );
    assert_eq!(read(&won.join("criteria.md")), criteria);
    assert_eq!(
        entries(&tasks(&world)),
        BTreeSet::from(["gadgets".to_string()])
    );
}

#[test_log::test(tokio::test)]
async fn a_capture_whose_commit_the_store_lacks_fails_and_leaves_nothing() {
    let mut world = World::new().await;
    let (_, run, _) = implemented_on_a_branch(&mut world).await;
    forget_every_commit(&world).await;

    let failed = capture(&world, &run, "gadgets", "Lost.\n").await;

    assert!(matches!(failed, Err(CaptureError::Failed(_))), "{failed:?}");
    assert_eq!(entries(&tasks(&world)), BTreeSet::new());
}

/// The tasks of the benchmark `name` of `world`, each as its name, its run and
/// its criteria; `None` when there is no such benchmark.
#[track_caller]
fn tasks_of<'a>(
    world: &'a World,
    name: &str,
) -> impl Future<Output = Option<Vec<(String, String, String)>>> + 'a {
    let caller = Location::caller();
    let name: BenchmarkName = name.parse().unwrap();
    async move {
        let shown = benchmarks(world)
            .await
            .show(&name)
            .await
            .unwrap_or_else(|e| panic!("showing the suite at {caller}: {e:#}"))?;
        Some(
            shown
                .tasks
                .into_iter()
                .map(|task| {
                    let criteria = task
                        .criteria
                        .map_or_else(|refused| refused.why, |c| c.as_str().into());
                    (task.name.to_string(), task.run.to_string(), criteria)
                })
                .collect(),
        )
    }
}

#[track_caller]
fn benchmark(name: &str) -> BenchmarkName {
    name.parse().unwrap()
}

#[test_log::test(tokio::test)]
async fn a_capture_keeps_the_task_in_its_benchmark() {
    let mut world = World::new().await;
    let run = implemented(&mut world).await;

    capture(&world, &run, "gadgets", "Gadgets.\n")
        .await
        .unwrap();

    assert_eq!(
        tasks_of(&world, IMPLEMENTS).await,
        Some(vec![("gadgets".into(), run, "Gadgets.\n".into())])
    );
}

#[test_log::test(tokio::test)]
async fn a_capture_into_a_benchmark_of_another_stage_is_refused_and_writes_nothing() {
    let mut world = World::new().await;
    let run = implemented(&mut world).await;
    benchmarks(&world)
        .await
        .make(&benchmark("designs"), TaskStage::Design)
        .await
        .unwrap();

    let refused = capture_into(
        &world,
        &run,
        "gadgets",
        "Gadgets.\n",
        BenchmarkChoice::Named(benchmark("designs")),
    )
    .await;

    assert!(
        matches!(refused, Err(CaptureError::OtherStage { .. })),
        "{refused:?}"
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        format!("the suite designs holds cases of design, and the run {run} is of implement")
    );
    assert_eq!(entries(&tasks(&world)), BTreeSet::new());
    assert_eq!(tasks_of(&world, "designs").await, Some(vec![]));
}

#[test_log::test(tokio::test)]
async fn a_capture_into_a_benchmark_that_does_not_exist_is_refused() {
    let mut world = World::new().await;
    let run = implemented(&mut world).await;

    let refused = capture_into(
        &world,
        &run,
        "gadgets",
        "Gadgets.\n",
        BenchmarkChoice::Named(benchmark("nowhere")),
    )
    .await;

    assert!(
        matches!(refused, Err(CaptureError::NoSuchBenchmark(_))),
        "{refused:?}"
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        "there is no suite nowhere"
    );
    assert_eq!(entries(&tasks(&world)), BTreeSet::new());
}

#[test_log::test(tokio::test)]
async fn a_capture_into_a_new_benchmark_makes_it_of_the_runs_stage() {
    let mut world = World::new().await;
    let run = implemented(&mut world).await;

    capture_into(
        &world,
        &run,
        "gadgets",
        "Gadgets.\n",
        BenchmarkChoice::New(benchmark("fresh")),
    )
    .await
    .unwrap();

    let found = benchmarks(&world)
        .await
        .find(&benchmark("fresh"))
        .await
        .unwrap();
    assert_eq!(
        found.map(|benchmark| benchmark.stage),
        Some(TaskStage::Implement)
    );
    assert_eq!(
        tasks_of(&world, "fresh").await,
        Some(vec![("gadgets".into(), run, "Gadgets.\n".into())])
    );
}

#[test_log::test(tokio::test)]
async fn a_new_benchmark_a_benchmark_already_has_the_name_of_is_refused() {
    let mut world = World::new().await;
    let run = implemented(&mut world).await;
    capture(&world, &run, "first", "Gadgets.\n").await.unwrap();

    let refused = capture_into(
        &world,
        &run,
        "second",
        "Gadgets.\n",
        BenchmarkChoice::New(benchmark(IMPLEMENTS)),
    )
    .await;

    assert!(
        matches!(refused, Err(CaptureError::BenchmarkTaken(_))),
        "{refused:?}"
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        "a suite is already named implements"
    );
    assert_eq!(
        entries(&tasks(&world)),
        BTreeSet::from(["first".to_string()])
    );
}

#[test_log::test(tokio::test)]
async fn a_failed_capture_into_a_new_benchmark_leaves_no_benchmark() {
    let mut world = World::new().await;
    let (_, run, _) = implemented_on_a_branch(&mut world).await;
    forget_every_commit(&world).await;

    let failed = capture_into(
        &world,
        &run,
        "gadgets",
        "Lost.\n",
        BenchmarkChoice::New(benchmark("fresh")),
    )
    .await;

    assert!(matches!(failed, Err(CaptureError::Failed(_))), "{failed:?}");
    assert_eq!(tasks_of(&world, "fresh").await, None);
}

#[test_log::test(tokio::test)]
async fn a_run_captured_into_two_benchmarks_gives_two_tasks() {
    let mut world = World::new().await;
    let run = implemented(&mut world).await;
    capture(&world, &run, "gadgets", "Adds gadgets.\n")
        .await
        .unwrap();

    capture_into(
        &world,
        &run,
        "gadgets-fast",
        "Adds gadgets fast.\n",
        BenchmarkChoice::New(benchmark("fast")),
    )
    .await
    .unwrap();

    assert_eq!(
        tasks_of(&world, IMPLEMENTS).await,
        Some(vec![(
            "gadgets".into(),
            run.clone(),
            "Adds gadgets.\n".into()
        )])
    );
    assert_eq!(
        tasks_of(&world, "fast").await,
        Some(vec![(
            "gadgets-fast".into(),
            run,
            "Adds gadgets fast.\n".into()
        )])
    );
}

/// The capture of `run` of `world` as the task `gadgets` of the benchmark
/// [`IMPLEMENTS`], checked and not yet written.
#[track_caller]
fn gadgets_capturable<'a>(
    world: &'a World,
    run: &'a str,
) -> impl Future<Output = task::Capturable> + 'a {
    let caller = Location::caller();
    let (implements, run, gadgets) = (benchmark(IMPLEMENTS), id(run), "gadgets".parse().unwrap());
    async move {
        let benchmarks = benchmarks(world).await;
        benchmarks
            .make(&implements, TaskStage::Implement)
            .await
            .unwrap_or_else(|e| panic!("making the suite at {caller}: {e:#}"));
        task::capturable(
            &runs(world).await,
            &benchmarks,
            &run,
            &gadgets,
            &BenchmarkChoice::Named(implements),
        )
        .await
        .unwrap_or_else(|e| panic!("checking the capture at {caller}: {e}"))
    }
}

#[test_log::test(tokio::test)]
async fn a_capture_whose_directory_cannot_be_moved_into_place_keeps_no_row() {
    let mut world = World::new().await;
    let run = implemented(&mut world).await;
    let capturable = gadgets_capturable(&world, &run).await;
    let before = tasks(&world).join("gadgets");
    std::fs::create_dir_all(&before).unwrap();
    std::fs::write(before.join("criteria.md"), "Captured before suites.\n").unwrap();

    let refused = task::capture(
        &*world.store,
        capturable,
        &Criteria::parse("Gadgets.\n").unwrap(),
        &benchmarks(&world).await,
    )
    .await;

    assert!(
        matches!(refused, Err(CaptureError::Taken(_))),
        "{refused:?}"
    );
    assert_eq!(tasks_of(&world, IMPLEMENTS).await, Some(vec![]));
    assert_eq!(
        read(&before.join("criteria.md")),
        "Captured before suites.\n"
    );
    assert_eq!(
        entries(&tasks(&world)),
        BTreeSet::from(["gadgets".to_string()])
    );
}

#[test_log::test(tokio::test)]
async fn a_capture_whose_row_is_refused_writes_no_directory() {
    let mut world = World::new().await;
    let run = implemented(&mut world).await;
    let capturable = gadgets_capturable(&world, &run).await;
    keep_task_row(
        &world.dir.path().join("work"),
        IMPLEMENTS,
        "gadgets",
        "9/implement-1",
    )
    .await;

    let refused = task::capture(
        &*world.store,
        capturable,
        &Criteria::parse("Gadgets.\n").unwrap(),
        &benchmarks(&world).await,
    )
    .await;

    assert!(
        matches!(refused, Err(CaptureError::Taken(_))),
        "{refused:?}"
    );
    assert_eq!(entries(&tasks(&world)), BTreeSet::new());
}

/// Runs `lw capture run name --suite implements --criteria FILE` in
/// `work_dir`, the file holding `criteria`.
#[track_caller]
fn lw_capture<'a>(
    work_dir: &'a Path,
    run: &'a str,
    name: &'a str,
    criteria: &str,
) -> impl Future<Output = Output> + 'a {
    let caller = Location::caller();
    let file = tempfile::NamedTempFile::new()
        .and_then(|file| std::fs::write(file.path(), criteria).map(|()| file))
        .unwrap_or_else(|e| panic!("writing the criteria at {caller}: {e}"));
    async move {
        let path = file.path().to_string_lossy();
        lw_in(
            work_dir,
            &[
                "capture",
                run,
                name,
                "--suite",
                IMPLEMENTS,
                "--criteria",
                &path,
            ],
        )
        .await
    }
}

#[test_log::test(tokio::test)]
async fn lw_capture_refuses_a_name_of_other_than_lowercase_letters_digits_and_hyphens() {
    let work_dir = tempfile::tempdir().unwrap();

    let output = lw_capture(work_dir.path(), "7/implement-1", "Gadgets_1", "Gadgets.\n").await;

    assert_eq!(output.code, Some(2), "{}", output.stderr);
    assert!(
        output
            .stderr
            .contains("lowercase letters, digits and hyphens"),
        "{}",
        output.stderr
    );
}

#[test_log::test(tokio::test)]
async fn lw_capture_of_a_run_id_that_names_no_run_exits_4_saying_why() {
    let (_world, work_dir) = world_in_work_dir().await;

    let output = lw_capture(work_dir.path(), "7/implement-1", "seven", "Gadgets.\n").await;

    assert_eq!(output.code, Some(4), "{}", output.stderr);
    assert!(output.stderr.contains("7/implement-1"), "{}", output.stderr);
}

#[test_log::test(tokio::test)]
async fn lw_capture_of_a_run_recorded_without_its_properties_exits_1_naming_them() {
    let (mut world, work_dir) = world_in_work_dir().await;
    let run = implemented(&mut world).await;
    record_as_before_the_properties(&work_dir.path().join(REPO)).await;

    let output = lw_capture(work_dir.path(), &run, "gadgets", "Gadgets.\n").await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert!(
        output.stderr.contains("lacks the properties"),
        "{}",
        output.stderr
    );
}

#[test_log::test(tokio::test)]
async fn lw_capture_of_an_automations_run_exits_1_saying_why() {
    let (mut world, work_dir) = world_in_work_dir().await;
    world.hourly_by_schedule_then_by_hand().await;

    let output = lw_capture(work_dir.path(), "hourly/1", "hourly", "Gadgets.\n").await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert!(output.stderr.contains("hourly/1"), "{}", output.stderr);
}

#[test_log::test(tokio::test)]
async fn lw_capture_without_criteria_is_a_command_line_error() {
    let (mut world, work_dir) = world_in_work_dir().await;
    let run = implemented(&mut world).await;

    let output = lw_in(work_dir.path(), &["capture", &run, "gadgets"]).await;

    assert_eq!(output.code, Some(2), "{}", output.stderr);
    assert!(output.stderr.contains("--criteria"), "{}", output.stderr);
    assert!(!work_dir.path().join(REPO).join("cases").exists());
}

#[test_log::test(tokio::test)]
async fn lw_capture_with_criteria_of_only_white_space_exits_1_before_reaching_github() {
    let (mut world, work_dir) = world_in_work_dir().await;
    let run = implemented(&mut world).await;
    keep_benchmark(&work_dir.path().join(REPO), IMPLEMENTS, "implement").await;

    let output = lw_capture(work_dir.path(), &run, "gadgets", " \n\t\n").await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert!(
        output.stderr.contains("the criteria are required"),
        "{}",
        output.stderr
    );
    assert!(!work_dir.path().join(REPO).join("cases").exists());
}

#[test_log::test(tokio::test)]
async fn lw_capture_into_a_benchmark_that_does_not_exist_exits_4_saying_why() {
    let (mut world, work_dir) = world_in_work_dir().await;
    let run = implemented(&mut world).await;

    let output = lw_capture(work_dir.path(), &run, "gadgets", "Gadgets.\n").await;

    assert_eq!(output.code, Some(4), "{}", output.stderr);
    assert_eq!(output.stderr, "lw: there is no suite implements\n");
    assert!(!work_dir.path().join(REPO).join("cases").exists());
}

#[test_log::test(tokio::test)]
async fn lw_capture_into_a_benchmark_of_another_stage_exits_1_saying_why() {
    let (mut world, work_dir) = world_in_work_dir().await;
    let run = implemented(&mut world).await;
    keep_benchmark(&work_dir.path().join(REPO), IMPLEMENTS, "design").await;

    let output = lw_capture(work_dir.path(), &run, "gadgets", "Gadgets.\n").await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert_eq!(
        output.stderr,
        format!(
            "lw: the suite implements holds cases of design, and the run {run} is of implement\n"
        )
    );
    assert!(!work_dir.path().join(REPO).join("cases").exists());
}

#[test_log::test(tokio::test)]
async fn lw_capture_without_a_benchmark_is_a_command_line_error() {
    let (mut world, work_dir) = world_in_work_dir().await;
    let run = implemented(&mut world).await;
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), "Gadgets.\n").unwrap();

    let output = lw_in(
        work_dir.path(),
        &[
            "capture",
            &run,
            "gadgets",
            "--criteria",
            &file.path().to_string_lossy(),
        ],
    )
    .await;

    assert_eq!(output.code, Some(2), "{}", output.stderr);
    assert!(output.stderr.contains("--suite"), "{}", output.stderr);
}

#[test_log::test(tokio::test)]
async fn lw_capture_of_a_name_a_benchmarks_task_has_exits_1_before_reaching_github() {
    let (mut world, work_dir) = world_in_work_dir().await;
    let run = implemented(&mut world).await;
    let repo = work_dir.path().join(REPO);
    keep_benchmark(&repo, IMPLEMENTS, "implement").await;
    keep_task_row(&repo, IMPLEMENTS, "gadgets", "9/implement-1").await;

    let output = lw_capture(work_dir.path(), &run, "gadgets", "Gadgets.\n").await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert_eq!(output.stderr, "lw: a case is already named gadgets\n");
    assert!(!repo.join("cases").exists());
}
