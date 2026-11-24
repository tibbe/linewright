use linewright_core::testing::captures::*;
use linewright_core::testing::*;

use crate::fake_serve::FakeServe;
use crate::programs::*;

/// Runs `lw capture run --benchmark 1 --name name --criteria FILE` asking
/// `serve`, the file holding `criteria`, 1 being the ID of the first
/// benchmark made.
async fn lw_capture(serve: &FakeServe, run: &str, name: &str, criteria: &str) -> Output {
    lw_capture_with(serve, run, name, criteria, &[]).await
}

/// [`lw_capture`], with the arguments `more` besides.
async fn lw_capture_with(
    serve: &FakeServe,
    run: &str,
    name: &str,
    criteria: &str,
    more: &[&str],
) -> Output {
    let file = tempfile::NamedTempFile::new()
        .and_then(|file| std::fs::write(file.path(), criteria).map(|()| file))
        .unwrap_or_else(|e| panic!("writing the criteria: {e}"));
    let path = file.path().to_string_lossy();
    let mut args = vec![
        "capture",
        run,
        "--benchmark",
        "1",
        "--name",
        name,
        "--criteria",
        &path,
    ];
    args.extend(more);
    serve.lw(&args).await
}

/// `docs/specs/operation.md`, Commands, `serve` does the work; and
/// `docs/specs/access.md`, Actions, Logged.
#[test_log::test(tokio::test)]
async fn lw_capture_asks_serve_which_captures_the_run_as_the_operator_once() {
    let (world, work_dir) = world_in_work_dir().await;
    let run = implemented(&world).await;
    keep_benchmark(&work_dir.path().join(REPO), IMPLEMENTS, "implement").await;
    let serve = FakeServe::serving(world, work_dir.path()).await;
    let (log, _guard) = Log::capture();

    let output = lw_capture(&serve, &run, "gadgets", "Gadgets.\n").await;
    let again = lw_capture(&serve, &run, "gadgets", "Gadgets.\n").await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let dir = work_dir.path().join(REPO).join("tasks").join("1");
    assert_eq!(
        output.stdout,
        format!(
            "Captured {run} as the task 1 (gadgets) in the benchmark 1 ({IMPLEMENTS}), in {}\n",
            dir.display()
        )
    );
    assert!(dir.join("criteria.md").exists());
    assert_eq!(again.code, Some(1), "{}", again.stderr);
    assert!(again.stderr.contains("gadgets"), "{}", again.stderr);
    let captured: Vec<String> = log
        .lines("INFO")
        .into_iter()
        .filter(|line| line.contains("Capture a run as a task: done"))
        .collect();
    assert_eq!(captured.len(), 1, "{captured:?}");
    assert!(
        captured[0].contains(r#" identity="operator" "#),
        "{captured:?}"
    );
    serve.stop().await;
}

#[test_log::test(tokio::test)]
async fn lw_capture_json_prints_serves_answer() {
    let (world, work_dir) = world_in_work_dir().await;
    let run = implemented(&world).await;
    keep_benchmark(&work_dir.path().join(REPO), IMPLEMENTS, "implement").await;
    let serve = FakeServe::serving(world, work_dir.path()).await;

    let output = lw_capture_with(&serve, &run, "gadgets", "Gadgets.\n", &["--json"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let dir = work_dir.path().join(REPO).join("tasks").join("1");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!({
            "run": run,
            "task": 1,
            "name": "gadgets",
            "benchmark": {"id": 1, "name": IMPLEMENTS, "stage": "implement"},
            "dir": dir,
        })
    );
    serve.stop().await;
}

/// `docs/specs/benchmarks.md`, Capturing, Before the first poll.
#[test_log::test(tokio::test)]
async fn lw_capture_before_serves_first_poll_exits_1_saying_why() {
    let work_dir = tempfile::tempdir().unwrap();
    let serve = FakeServe::over(work_dir.path()).await;
    keep_benchmark(&work_dir.path().join(REPO), IMPLEMENTS, "implement").await;

    let output = lw_capture(&serve, "7/implement-1", "seven", "Gadgets.\n").await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert_eq!(
        output.stderr,
        "lw: Linewright can't capture a run before its first poll.\n"
    );
    serve.stop().await;
}

#[test_log::test(tokio::test)]
async fn lw_capture_refuses_no_name() {
    let work_dir = tempfile::tempdir().unwrap();
    let serve = FakeServe::over(work_dir.path()).await;

    let output = lw_capture(&serve, "7/implement-1", " \t\u{200b} ", "Gadgets.\n").await;

    assert_eq!(output.code, Some(2), "{}", output.stderr);
    assert!(
        output
            .stderr
            .contains("it holds nothing once it is clean, which is no name"),
        "{}",
        output.stderr
    );
}

#[test_log::test(tokio::test)]
async fn lw_capture_of_a_run_id_that_names_no_run_exits_4_saying_why() {
    let (world, work_dir) = world_in_work_dir().await;
    let serve = FakeServe::serving(world, work_dir.path()).await;

    let output = lw_capture(&serve, "7/implement-1", "seven", "Gadgets.\n").await;

    assert_eq!(output.code, Some(4), "{}", output.stderr);
    assert!(output.stderr.contains("7/implement-1"), "{}", output.stderr);
    serve.stop().await;
}

#[test_log::test(tokio::test)]
async fn lw_capture_of_an_automations_run_exits_1_saying_why() {
    let (world, work_dir) = world_in_work_dir().await;
    hourly_run_kept(&work_dir.path().join(REPO)).await;
    let serve = FakeServe::serving(world, work_dir.path()).await;

    let output = lw_capture(&serve, "hourly.md/1", "hourly", "Gadgets.\n").await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert!(output.stderr.contains("hourly.md/1"), "{}", output.stderr);
    serve.stop().await;
}

#[test_log::test(tokio::test)]
async fn lw_capture_without_criteria_is_a_command_line_error() {
    let (world, work_dir) = world_in_work_dir().await;
    let run = implemented(&world).await;

    let output = lw_in(
        work_dir.path(),
        &["capture", &run, "--benchmark", "1", "--name", "gadgets"],
    )
    .await;

    assert_eq!(output.code, Some(2), "{}", output.stderr);
    assert!(output.stderr.contains("--criteria"), "{}", output.stderr);
    assert!(!work_dir.path().join(REPO).join("tasks").exists());
}

#[test_log::test(tokio::test)]
async fn lw_capture_with_criteria_of_only_white_space_exits_1_before_asking_serve() {
    let (world, work_dir) = world_in_work_dir().await;
    let run = implemented(&world).await;
    keep_benchmark(&work_dir.path().join(REPO), IMPLEMENTS, "implement").await;
    let serve = FakeServe::serving(world, work_dir.path()).await;

    let output = lw_capture(&serve, &run, "gadgets", " \n\t\n").await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert!(
        output.stderr.contains("the criteria are required"),
        "{}",
        output.stderr
    );
    assert!(!work_dir.path().join(REPO).join("tasks").exists());
    serve.stop().await;
}

#[test_log::test(tokio::test)]
async fn lw_capture_without_a_benchmark_is_a_command_line_error() {
    let (world, work_dir) = world_in_work_dir().await;
    let run = implemented(&world).await;
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), "Gadgets.\n").unwrap();

    let output = lw_in(
        work_dir.path(),
        &[
            "capture",
            &run,
            "--name",
            "gadgets",
            "--criteria",
            &file.path().to_string_lossy(),
        ],
    )
    .await;

    assert_eq!(output.code, Some(2), "{}", output.stderr);
    assert!(output.stderr.contains("--benchmark"), "{}", output.stderr);
}
