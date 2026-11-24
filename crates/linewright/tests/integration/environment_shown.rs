//! Showing and rebuilding the environment: `environment`, `environment
//! rebuild`, `status` and the server page (`docs/specs/sessions.md`, `lw
//! environment`, and `docs/specs/sessions.md`, Building).

use linewright_core::testing::*;

use crate::programs::*;

/// The lines of `text`, each account's duration, such as `0s`, as `Ns`.
fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| match line.rsplit_once(", ") {
            Some((before, after))
                if after.ends_with('s')
                    && after[..after.len() - 1].bytes().all(|b| b.is_ascii_digit()) =>
            {
                format!("{before}, Ns")
            }
            _ => line.to_string(),
        })
        .collect()
}

/// An RFC 3339 time in UTC, as the commands print it.
fn shown_time(rfc3339: &str) -> String {
    rfc3339.replace('T', " ").replace('Z', " UTC")
}

/// A commit, as builds name it.
fn short(commit: &str) -> &str {
    &commit[..12]
}

#[test_log::test(tokio::test)]
async fn without_serve_environment_prints_the_latest_build_then_the_latest_that_failed() {
    let (mut world, work_dir) = world_in_work_dir().await;
    let in_work_dir = |config: &mut linewright_core::factory::FactoryConfig| {
        config.work_dir = work_dir.path().join(REPO);
    };
    set_up_with(&world, "echo 'npm ERR! missing lockfile'; exit 3").await;
    world.poll().await;
    set_up_with(&world, "echo installed").await;
    world.restart(in_work_dir).await;
    world.poll().await;

    let shown = lw_in(work_dir.path(), &["environment"]).await;
    let json = lw_in(work_dir.path(), &["environment", "--json"]).await;

    assert_eq!(shown.code, Some(0), "{}", shown.stderr);
    let answer: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    let [(latest, latest_at), (failed, failed_at)] = [0, 1].map(|i| {
        let build = &answer["environment"]["builds"][i];
        (
            build["commit"].as_str().unwrap().to_string(),
            shown_time(build["at"].as_str().unwrap()),
        )
    });
    assert_eq!(
        lines(&shown.stdout),
        [
            format!("environment: built from `{}` {latest_at}", short(&latest)),
            String::new(),
            format!("build 2: started {latest_at}, from `{}`", short(&latest)),
            "setup: echo installed → exit 0, Ns".into(),
            "  installed".into(),
            String::new(),
            format!("build 1: started {failed_at}, from `{}`", short(&failed)),
            "setup: echo 'npm ERR! missing lockfile'; exit 3 → exit 3, Ns".into(),
            "  npm ERR! missing lockfile".into(),
        ]
    );
    let build = &answer["environment"]["builds"][1];
    assert_eq!(
        (
            &build["command"],
            &build["status"],
            &build["output"],
            &build["commit"]
        ),
        (
            &serde_json::json!("echo 'npm ERR! missing lockfile'; exit 3"),
            &serde_json::json!({"exited": 3}),
            &serde_json::json!("npm ERR! missing lockfile\n"),
            &serde_json::json!(failed),
        )
    );
}

#[test_log::test(tokio::test)]
async fn without_serve_and_with_no_setup_command_environment_says_there_is_none() {
    let (mut world, work_dir) = world_in_work_dir().await;
    world.poll().await;

    let shown = lw_in(work_dir.path(), &["environment"]).await;

    assert_eq!(
        (shown.code, shown.stdout.as_str()),
        (Some(0), "environment: there is no setup command\n")
    );
}

#[test_log::test(tokio::test)]
async fn once_the_environments_directory_is_removed_by_hand_the_next_build_takes_a_number_after_the_one_it_held()
 {
    let (mut world, work_dir) = world_in_work_dir().await;
    let in_work_dir = |config: &mut linewright_core::factory::FactoryConfig| {
        config.work_dir = work_dir.path().join(REPO);
    };
    set_up_with(&world, "echo installed").await;
    world.poll().await;
    std::fs::remove_dir_all(work_dir.path().join(REPO).join("environment")).unwrap();

    world.restart(in_work_dir).await;
    world.poll().await;
    let shown = lw_in(work_dir.path(), &["environment"]).await;

    let builds: Vec<&str> = shown
        .stdout
        .lines()
        .filter(|line| line.starts_with("build "))
        .collect();
    assert_eq!(builds.len(), 1, "{}", shown.stdout);
    assert!(builds[0].starts_with("build 2: "), "{}", shown.stdout);
}
