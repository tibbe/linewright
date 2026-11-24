use std::panic::Location;
use std::path::Path;

use linewright_core::testing::captures::*;
use linewright_core::testing::*;

use crate::programs::*;

/// Runs `lw capture run --benchmark 1 --name name --criteria FILE` in
/// `work_dir`, the file holding `criteria`, 1 being the ID of the first
/// benchmark made.
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
                "--benchmark",
                "1",
                "--name",
                name,
                "--criteria",
                &path,
            ],
        )
        .await
    }
}

#[test_log::test(tokio::test)]
async fn lw_capture_refuses_no_name() {
    let work_dir = tempfile::tempdir().unwrap();

    let output = lw_capture(
        work_dir.path(),
        "7/implement-1",
        " \t\u{200b} ",
        "Gadgets.\n",
    )
    .await;

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
    let (_world, work_dir) = world_in_work_dir().await;

    let output = lw_capture(work_dir.path(), "7/implement-1", "seven", "Gadgets.\n").await;

    assert_eq!(output.code, Some(4), "{}", output.stderr);
    assert!(output.stderr.contains("7/implement-1"), "{}", output.stderr);
}

#[test_log::test(tokio::test)]
async fn lw_capture_of_an_automations_run_exits_1_saying_why() {
    let (mut world, work_dir) = world_in_work_dir().await;
    world.hourly_by_schedule_then_by_hand().await;

    let output = lw_capture(work_dir.path(), "hourly.md/1", "hourly", "Gadgets.\n").await;

    assert_eq!(output.code, Some(1), "{}", output.stderr);
    assert!(output.stderr.contains("hourly.md/1"), "{}", output.stderr);
}

#[test_log::test(tokio::test)]
async fn lw_capture_without_criteria_is_a_command_line_error() {
    let (mut world, work_dir) = world_in_work_dir().await;
    let run = implemented(&mut world).await;

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
    assert!(!work_dir.path().join(REPO).join("tasks").exists());
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
