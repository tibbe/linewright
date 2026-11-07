//! The maintenance command, run in each session's clone before the agent's
//! first prompt (`docs/specs/runs.md`, Maintenance).

use crate::common;

use std::panic::Location;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use linewright::domain::Number;
use linewright::reconcile::WorkKind;
use linewright::run::{AfterMaintenance, after_maintenance};
use linewright::runs::Ending;
use linewright::session_env::{PassEnv, SessionEnv, SetEnv};
use linewright::transcript::{self, MaintenanceEnded, MaintenanceRan, MaintenanceStatus};
use rstest::rstest;
use serde_json::json;

const CONFIG: &str = ".linewright/config.toml";

async fn commit_config(world: &World, config: &str) {
    world
        .store
        .commit("maintainer", "main", &[(CONFIG, config)], "Set up")
        .await
        .unwrap();
}

fn open_for_triage(world: &World) -> linewright::domain::Number {
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"])
}

#[test_log::test(tokio::test)]
async fn the_repositorys_maintenance_runs_in_the_clones_root_with_the_sessions_environment_variables_before_the_session_starts()
 {
    let mut world = World::with(|config| {
        let session_env = SessionEnv::new(
            PassEnv::default(),
            SetEnv::new([("GREETING".to_string(), "hello".to_string())]).unwrap(),
        );
        config.environments = config.work.environments(&session_env, [], &[]).environments;
    })
    .await;
    commit_config(
        &world,
        r#"maintenance = '''printf '%s %s\n' "$GREETING" "$TMPDIR" > made.txt; cat README.md >> made.txt'''"#,
    )
    .await;
    let seen = Arc::new(Mutex::new(vec![]));
    world.harness.on(WorkKind::Triage, {
        let seen = seen.clone();
        move |call| {
            seen.lock().unwrap().push((
                std::fs::read_to_string(call.spec.dir.join("made.txt")).ok(),
                call.spec.scratch.clone(),
            ));
            failed_session("looked")
        }
    });
    open_for_triage(&world);

    world.poll().await;

    let seen = seen.lock().unwrap();
    let (made, scratch) = seen.first().expect("a triage session started");
    assert_eq!(
        made.as_deref(),
        Some(format!("hello {}\n# Widgets\n", scratch.display()).as_str())
    );
}

/// A directory outside every clone, where maintenance commands leave marks.
fn marks() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// The lines of the file `name` in `marks`, none when it is missing.
fn lines_of(marks: &Path, name: &str) -> Vec<String> {
    std::fs::read_to_string(marks.join(name))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// A triage session as its agent saw it: its prompt, with its attachments
/// directory as `ATTACHMENTS`, and how the maintenance command ended as its
/// transcript says.
#[derive(Debug)]
struct Triaged {
    prompt: String,
    maintenance: Option<MaintenanceEnded>,
}

/// Triages an issue in a world maintaining with `command`, each triage
/// answering that the issue is ready for an agent, and gives the sessions and
/// the issue's labels then.
#[track_caller]
fn triaged_maintaining_with(command: &str) -> impl Future<Output = (Vec<Triaged>, Vec<String>)> {
    let caller = Location::caller();
    async move {
        let mut world = World::with(|config| {
            maintaining_with(config, command);
        })
        .await;
        let triaged = Arc::new(Mutex::new(vec![]));
        world.harness.on(WorkKind::Triage, {
            let triaged = triaged.clone();
            move |call| {
                let record = transcript::SessionRecord::read(&call.spec.record)
                    .unwrap_or_else(|e| panic!("reading the transcript at {caller}: {e:#}"));
                let maintenance = transcript::parse(&record, transcript::Written::SoFar)
                    .maintenance
                    .and_then(|maintenance| match maintenance.ran {
                        MaintenanceRan::Started { course, .. } => course.ended().cloned(),
                        MaintenanceRan::Recorded(ended) => Some(ended),
                    });
                triaged.lock().unwrap().push(Triaged {
                    prompt: call
                        .spec
                        .prompt
                        .replace(&call.spec.attachments.display().to_string(), "ATTACHMENTS"),
                    maintenance,
                });
                Ok(json!({
                    "outcome": "ready-for-agent",
                    "category": "bug",
                    "comment": "## Agent Brief\n\n**Summary:** It crashes."
                })
                .to_string())
            }
        });
        let issue = open_for_triage(&world);

        world.poll().await;

        let labels = labels(&world.issue(issue))
            .into_iter()
            .map(str::to_string)
            .collect();
        (std::mem::take(&mut *triaged.lock().unwrap()), labels)
    }
}

/// `docs/specs/runs.md`, Stages, 2a, and Out of scope: the agent is not told.
#[test_log::test(tokio::test)]
async fn a_maintenance_command_that_exits_non_zero_runs_once_and_its_session_goes_on_as_after_one_that_succeeded()
 {
    let marks = marks();
    let command = format!(
        "echo ran >> '{}/runs'; echo 'npm ERR! missing lockfile'; exit 3",
        marks.path().display()
    );
    let (succeeded, _) = triaged_maintaining_with("true").await;

    let (triaged, labels) = triaged_maintaining_with(&command).await;

    assert_eq!(lines_of(marks.path(), "runs"), ["ran"]);
    let [
        Triaged {
            prompt,
            maintenance: Some(ended),
        },
    ] = &triaged[..]
    else {
        panic!("one session, after its maintenance command ended: {triaged:?}");
    };
    assert_eq!(*prompt, succeeded[0].prompt);
    assert_eq!(ended.status, MaintenanceStatus::Exited(3));
    assert_eq!(ended.output, "npm ERR! missing lockfile\n");
    assert!(labels.iter().any(|l| l == "ready-for-agent"), "{labels:?}");
}

/// `docs/specs/runs.md`, Stages, 2a: a command that does not exit with
/// status 0 fails, and its session goes on all the same.
#[rstest]
#[case::exiting_with_status_0(MaintenanceStatus::Exited(0), AfterMaintenance::GoesOn)]
#[case::exiting_with_another_status(
    MaintenanceStatus::Exited(3),
    AfterMaintenance::GoesOnPastAFailure
)]
#[case::ended_by_a_signal(MaintenanceStatus::Signalled(9), AfterMaintenance::GoesOnPastAFailure)]
#[case::still_running_at_the_deadline(MaintenanceStatus::TimedOut, AfterMaintenance::TimesOut)]
#[case::cancelled(MaintenanceStatus::Cancelled, AfterMaintenance::Cancelled)]
fn a_session_goes_on_after_its_maintenance_command_unless_it_timed_out_or_was_cancelled(
    #[case] status: MaintenanceStatus,
    #[case] expected: AfterMaintenance,
) {
    assert_eq!(after_maintenance(status), expected);
}

#[test_log::test(tokio::test)]
async fn the_maintenance_commands_time_counts_against_its_sessions_timeout() {
    let mut world = World::with(|config| {
        maintaining_with(config, "sleep 2");
        config.work.flags_and_env.session_timeout = Some(Duration::from_secs(10));
    })
    .await;
    let left = Arc::new(Mutex::new(None));
    world.harness.on(WorkKind::Triage, {
        let left = left.clone();
        move |call| {
            *left.lock().unwrap() = Some(call.spec.deadline - tokio::time::Instant::now());
            Ok(json!({
                "outcome": "ready-for-agent",
                "category": "bug",
                "comment": "## Agent Brief\n\n**Summary:** It crashes."
            })
            .to_string())
        }
    });
    open_for_triage(&world);

    world.poll().await;

    let left = left.lock().unwrap().expect("the session started");
    assert!(left <= Duration::from_secs(8), "{left:?}");
    assert!(left > Duration::from_secs(5), "{left:?}");
}

#[test_log::test(tokio::test)]
async fn a_maintenance_command_that_outlasts_its_sessions_timeout_is_the_session_timing_out() {
    let marks = marks();
    let command = format!(
        "echo ran >> '{}/runs'; echo 'fetching...'; sleep 30",
        marks.path().display()
    );
    let mut world = World::with(|config| {
        maintaining_with(config, &command);
        config.work.flags_and_env.session_timeout = Some(Duration::from_secs(1));
    })
    .await;
    let issue = open_for_triage(&world);
    let started = std::time::Instant::now();

    world.poll().await;

    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(lines_of(marks.path(), "runs"), ["ran"], "not retried");
    let issue = world.issue(issue);
    assert!(labels(&issue).contains(&"ready-for-human"));
    let comment = last_comment(&issue);
    assert!(
        comment.contains(
            "The session ended unexpectedly: it timed out. The work may need cutting \
             smaller, or `session-timeout` may need to be longer."
        ),
        "{comment}"
    );
    assert!(!comment.contains("failed twice"), "{comment}");
    assert!(
        comment.contains("<summary>The end of the maintenance command's output</summary>"),
        "{comment}"
    );
    assert!(comment.contains("fetching..."), "{comment}");
}

/// A world whose ticket's pull request the default branch has moved past,
/// in conflict with it or not, and whose repository's maintenance command,
/// set from then on, appends to `ran` in `marks` what the ticket's file
/// holds.
async fn moved_past_maintaining(conflicting: bool, marks: &Path) -> (World, Number) {
    let moved_past =
        crate::pipeline::moved_past_with(Repository::default(), |_| {}, conflicting).await;
    commit_config(
        &moved_past.0,
        &format!(
            "maintenance = \"cat src/issue-*.txt >> '{}/ran'\"\n",
            marks.display()
        ),
    )
    .await;
    moved_past
}

#[test_log::test(tokio::test)]
async fn an_update_with_a_conflict_runs_the_maintenance_command_on_the_branch_before_updating_it() {
    let marks = marks();
    let (mut world, _) = moved_past_maintaining(true, marks.path()).await;

    world.poll().await;

    assert_eq!(lines_of(marks.path(), "ran"), ["done"], "the branch's own");
    assert_eq!(world.harness.prompts(WorkKind::Update).len(), 1);
    assert_eq!(world.harness.stopped(WorkKind::Update), [Stopped::Rebase]);
}

/// Waits until the file at `path` holds a line, and returns it.
async fn written(path: &Path) -> String {
    for _ in 0..1000 {
        if let Ok(text) = std::fs::read_to_string(path)
            && text.ends_with('\n')
        {
            return text.trim().to_string();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{} was never written", path.display());
}

/// Whether the process `pid` is running: neither gone nor a zombie.
fn running(pid: &str) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8(output.stdout).unwrap();
    !stat.trim().is_empty() && !stat.trim().starts_with('Z')
}

#[test_log::test(tokio::test)]
async fn cancelling_a_run_during_its_maintenance_command_ends_the_command_at_once() {
    let marks = marks();
    let pid = marks.path().join("pid");
    let command = format!("echo $$ > '{}'; exec sleep 60", pid.display());
    let mut world = World::with(|config| {
        maintaining_with(config, &command);
    })
    .await;
    implement(&world);
    let ticket = open_ticket(&world, "Do the thing");
    let started = std::time::Instant::now();
    world.factory.cycle().await.unwrap();
    let pid = written(&pid).await;

    world.store.unlabel_as(ticket, "ready-for-agent").unwrap();
    world.factory.cycle().await.unwrap();
    world.factory.settle().await;

    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(!running(&pid));
    assert_eq!(ended_runs(&world), [Ending::Cancelled]);
    assert_eq!(
        world.harness.prompts(WorkKind::Implement),
        Vec::<String>::new()
    );
}

#[test_log::test(tokio::test)]
async fn what_the_maintenance_command_leaves_running_is_ended_once_the_run_ends() {
    let marks = marks();
    let pid = marks.path().join("pid");
    let command = format!(
        "(while true; do echo still here; sleep 0.05; done) & echo $! > '{}'",
        pid.display()
    );
    let mut world = World::with(|config| {
        maintaining_with(config, &command);
    })
    .await;
    let during = Arc::new(Mutex::new(vec![]));
    world.harness.on(WorkKind::Triage, {
        let (during, pid) = (during.clone(), pid.clone());
        move |_| {
            std::thread::sleep(Duration::from_millis(200));
            let pid = std::fs::read_to_string(&pid).unwrap();
            during.lock().unwrap().push(running(pid.trim()));
            failed_session("looked")
        }
    });
    open_for_triage(&world);

    world.poll().await;

    assert_eq!(
        *during.lock().unwrap(),
        [true],
        "running during the session"
    );
    let pid = std::fs::read_to_string(&pid).unwrap();
    assert!(gone(pid.trim()).await);
}

/// Whether the process `pid` stops running within a few seconds.
async fn gone(pid: &str) -> bool {
    for _ in 0..500 {
        if !running(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[test_log::test(tokio::test)]
async fn an_automations_run_whose_maintenance_command_fails_goes_on_to_its_prompt() {
    let mut world = World::new().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[
                (
                    ".linewright/automations/look.md",
                    "---\non:\n  schedule: \"0 0 1 1 *\"\n---\nLook.\n",
                ),
                (CONFIG, "maintenance = \"echo 'no such tool'; exit 127\"\n"),
            ],
            "Add an automation",
        )
        .await
        .unwrap();
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));

    world
        .factory
        .run_automation(&"look".parse().unwrap())
        .await
        .unwrap();

    let prompts = world.harness.automation_prompts();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(prompts[0].contains("Look."), "{}", prompts[0]);
}

#[test_log::test(tokio::test)]
async fn a_bash_that_does_not_start_hands_back_after_a_retry_without_a_tail() {
    let mut world = World::with(|config| {
        config.bash = "/nowhere/bash".into();
        maintaining_with(config, "true");
    })
    .await;
    let issue = open_for_triage(&world);

    world.poll().await;

    let issue = world.issue(issue);
    let comment = last_comment(&issue);
    assert!(
        comment.contains("The run failed twice on its infrastructure"),
        "{comment}"
    );
    assert!(comment.contains("/nowhere/bash"), "{comment}");
    assert!(!comment.contains("<details>"), "{comment}");
}
