use linewright_core::reconcile::WorkKind;
use linewright_core::secrets::Secrets;
use linewright_core::testing::*;
use serde_json::json;

use crate::programs::*;

/// Implementing sessions stream the recorded transcript: its first turn
/// ends in an answer that is not a result, and its second, after the
/// follow-up, in one that is.
pub(crate) fn implement_as_recorded(world: &World) {
    before_the_recorded_transcript(world);
    world.harness.on(WorkKind::Implement, |call| {
        let turn = match call.followup {
            None => linewright_core::testing::TURN_1,
            Some(_) => linewright_core::testing::TURN_2,
        };
        let output = call.spec.record.join(linewright_core::transcript::OUTPUT);
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

#[test_log::test(tokio::test)]
async fn logs_shows_the_items_latest_run_compact() {
    let (_world, work_dir) = implemented_as_recorded().await;

    let output = lw_in(work_dir.path(), &["logs", "1"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 19, "{}", output.stdout);
    assert_eq!(lines[0], "run: 1/implement-1");
    assert_eq!(lines[1], "session 1: started 2026-10-13 10:09:06 UTC");
    assert_eq!(lines[2], " 0:00  clone → 0s");
}

#[test_log::test(tokio::test)]
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

#[test_log::test(tokio::test)]
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

/// `world_in_work_dir`'s, with `maintenance` the maintenance command of every session
/// and `secrets` Linewright's secrets.
async fn world_maintaining_with(maintenance: &str, secrets: Secrets) -> (World, tempfile::TempDir) {
    let work_dir = tempfile::tempdir().unwrap();
    let world = World::with(|config| {
        config.work_dir = work_dir.path().join(REPO);
        config.secrets = secrets;
        maintaining_with(config, maintenance);
    })
    .await;
    (world, work_dir)
}

/// `docs/specs/runs.md`, `lw logs`, Maintenance.
#[test_log::test(tokio::test)]
async fn logs_shows_the_maintenance_command_a_session_ran() {
    let (mut world, work_dir) =
        world_maintaining_with("echo 'added 3 packages'", Secrets::default()).await;
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
        output.stdout.lines().nth(3),
        Some(" 0:01  maintenance: echo 'added 3 packages' → exit 0, 0s"),
        "{}",
        output.stdout
    );
}

/// `docs/specs/operation.md`, Secrets.
#[test_log::test(tokio::test)]
async fn logs_shows_a_secret_in_the_maintenance_command_redacted() {
    let (mut world, work_dir) = world_maintaining_with(
        "echo 'token npm_s3cr3t' > /dev/null",
        secrets_of(&[("NPM_TOKEN", "npm_s3cr3t")]),
    )
    .await;
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
    assert!(
        output
            .stdout
            .contains("maintenance: echo 'token [redacted]' > /dev/null")
            && !output.stdout.contains("npm_s3cr3t"),
        "{}",
        output.stdout
    );
}
