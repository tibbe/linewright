//! The setup command, run in each session's clone before the session starts
//! (`docs/specs/runs.md`, Setup).

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use linewright::domain::Number;
use linewright::reconcile::WorkKind;
use linewright::runs::Ending;
use linewright::session_env::{PassEnv, SessionEnv, SetEnv};
use linewright::work_settings::{Partial, Setup, StageTable};
use serde_json::json;

const CONFIG: &str = ".linewright/config.toml";

async fn commit_config(world: &World, config: &str) {
    world
        .store
        .commit("maintainer", "main", &[(CONFIG, config)], "Set up")
        .await
        .unwrap();
}

/// Each triage records what `file` in its clone holds, `None` when it is
/// missing, and answers that the issue is ready for an agent.
fn triage_reading(world: &World, file: &'static str) -> Arc<Mutex<Vec<Option<String>>>> {
    let read = Arc::new(Mutex::new(vec![]));
    world.harness.on(WorkKind::Triage, {
        let read = read.clone();
        move |call| {
            read.lock()
                .unwrap()
                .push(std::fs::read_to_string(call.spec.dir.join(file)).ok());
            Ok(json!({
                "outcome": "ready-for-agent",
                "category": "bug",
                "comment": "## Agent Brief\n\n**Summary:** It crashes."
            })
            .to_string())
        }
    });
    read
}

fn open_for_triage(world: &World) -> linewright::domain::Number {
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"])
}

#[tokio::test]
async fn the_repositorys_setup_runs_in_the_clones_root_before_the_session_starts() {
    let mut world = World::new().await;
    commit_config(&world, "setup = \"cat README.md > made.txt\"\n").await;
    let read = triage_reading(&world, "made.txt");
    open_for_triage(&world);

    world.poll().await;

    assert_eq!(*read.lock().unwrap(), [Some("# Widgets\n".to_string())]);
}

fn setup(command: &str) -> Setup {
    Setup::from(command.to_string())
}

/// The operator's stage table for `stage` setting `setup`.
fn stage_setup(setup: Option<Setup>) -> StageTable {
    StageTable {
        settings: Partial {
            setup,
            ..Partial::default()
        },
        ..StageTable::default()
    }
}

#[tokio::test]
async fn the_operators_stage_table_replaces_the_whole_pipelines_setup_and_an_empty_one_runs_nothing()
 {
    for (table, made) in [
        (
            Some(setup("echo triage > made.txt")),
            Some("triage\n".to_string()),
        ),
        (Some(Setup::Empty), None),
    ] {
        let mut world = World::with(|config| {
            config.work.flags_and_env.setup = Some(setup("echo pipeline > made.txt"));
            config
                .work
                .stages
                .insert(WorkKind::Triage, stage_setup(table.clone()));
        })
        .await;
        let read = triage_reading(&world, "made.txt");
        open_for_triage(&world);

        world.poll().await;

        assert_eq!(*read.lock().unwrap(), [made], "{table:?}");
    }
}

#[tokio::test]
async fn an_automations_front_matter_replaces_the_whole_pipelines_setup() {
    let world = World::with(|config| {
        config.work.flags_and_env.setup = Some(setup("echo pipeline > made.txt"));
    })
    .await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(
                ".linewright/automations/look.md",
                "---\non:\n  schedule: \"0 0 1 1 *\"\nsetup: echo automation > made.txt\n---\nLook.\n",
            )],
            "Add an automation",
        )
        .await
        .unwrap();
    let read = Arc::new(Mutex::new(vec![]));
    world.harness.on_automation({
        let read = read.clone();
        move |call| {
            read.lock()
                .unwrap()
                .push(std::fs::read_to_string(call.spec.dir.join("made.txt")).ok());
            Ok(nothing_needs_doing())
        }
    });

    world
        .factory
        .run_automation(&"look".parse().unwrap())
        .await
        .unwrap();

    assert_eq!(*read.lock().unwrap(), [Some("automation\n".to_string())]);
}

#[tokio::test]
async fn the_setup_command_runs_in_the_sessions_environment() {
    let mut world = World::with(|config| {
        let session_env = SessionEnv::new(
            PassEnv::default(),
            SetEnv::new([("GREETING".to_string(), "hello".to_string())]).unwrap(),
        );
        config.environments = config.work.environments(&session_env, [], &[]).environments;
        config.work.flags_and_env.setup =
            Some(setup("printf '%s %s' \"$GREETING\" \"$TMPDIR\" > env.txt"));
    })
    .await;
    let seen = Arc::new(Mutex::new(vec![]));
    world.harness.on(WorkKind::Triage, {
        let seen = seen.clone();
        move |call| {
            seen.lock().unwrap().push((
                std::fs::read_to_string(call.spec.dir.join("env.txt")).unwrap(),
                call.spec.scratch.clone(),
            ));
            failed_session("looked")
        }
    });
    open_for_triage(&world);

    world.poll().await;

    let seen = seen.lock().unwrap();
    let (env, scratch) = seen.first().expect("a triage session started");
    assert_eq!(*env, format!("hello {}", scratch.display()));
}

/// A directory outside every clone, where setup commands leave marks.
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

#[tokio::test]
async fn a_setup_command_that_fails_twice_hands_the_item_back_with_the_end_of_its_output() {
    let marks = marks();
    let command = format!(
        "echo ran >> '{}/runs'; echo 'npm ERR! missing lockfile'; exit 3",
        marks.path().display()
    );
    let mut world =
        World::with(|config| config.work.flags_and_env.setup = Some(setup(&command))).await;
    let read = triage_reading(&world, "README.md");
    let issue = open_for_triage(&world);

    world.poll().await;

    assert_eq!(lines_of(marks.path(), "runs"), ["ran", "ran"]);
    assert!(read.lock().unwrap().is_empty(), "no session started");
    assert!(labels(&world.issue(issue)).contains(&"ready-for-human"));
    let issue = world.issue(issue);
    let comment = last_comment(&issue);
    assert!(
        comment.contains("The run failed twice on its infrastructure"),
        "{comment}"
    );
    assert!(comment.contains("status 3"), "{comment}");
    assert!(
        comment.contains(
            "<summary>The end of the setup command's output</summary>\n\n```\nnpm ERR! missing lockfile\n```"
        ),
        "{comment}"
    );
}

#[tokio::test]
async fn a_setup_command_that_fails_once_is_retried_and_its_session_starts() {
    let marks = marks();
    let once = marks.path().join("once");
    let command = format!(
        "test -e '{once}' || {{ touch '{once}'; exit 1; }}; echo set > made.txt",
        once = once.display()
    );
    let mut world =
        World::with(|config| config.work.flags_and_env.setup = Some(setup(&command))).await;
    let read = triage_reading(&world, "made.txt");
    open_for_triage(&world);

    world.poll().await;

    assert_eq!(*read.lock().unwrap(), [Some("set\n".to_string())]);
    assert_eq!(ended_runs(&world), [Ending::Done]);
}

#[tokio::test]
async fn a_setup_command_that_times_out_hands_back_at_once_naming_the_timeout_setting() {
    let marks = marks();
    let command = format!(
        "echo ran >> '{}/runs'; echo 'fetching...'; sleep 30",
        marks.path().display()
    );
    let mut world = World::with(|config| {
        config.work.flags_and_env.setup = Some(setup(&command));
        config.work.flags_and_env.session_timeout = Some(Duration::from_secs(1));
    })
    .await;
    let issue = open_for_triage(&world);
    let started = std::time::Instant::now();

    world.poll().await;

    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(lines_of(marks.path(), "runs"), ["ran"], "not retried");
    let issue = world.issue(issue);
    let comment = last_comment(&issue);
    assert!(comment.contains("The setup command timed out"), "{comment}");
    assert!(comment.contains("`session-timeout`"), "{comment}");
    assert!(!comment.contains("failed twice"), "{comment}");
    assert!(
        comment.contains("<summary>The end of the setup command's output</summary>"),
        "{comment}"
    );
    assert!(comment.contains("fetching..."), "{comment}");
}

/// A world whose ticket's pull request the default branch has moved past,
/// in conflict with it or not, and whose update runs' setup command
/// appends to `ran` in `marks` what the ticket's file holds.
async fn moved_past_setting_up(conflicting: bool, marks: &Path) -> (World, Number) {
    let command = format!("cat src/issue-*.txt >> '{}/ran'", marks.display());
    crate::pipeline::moved_past_with(
        Repository::default(),
        |config| {
            config
                .work
                .stages
                .insert(WorkKind::Update, stage_setup(Some(setup(&command))));
        },
        conflicting,
    )
    .await
}

#[tokio::test]
async fn an_update_without_a_conflict_runs_no_setup_command() {
    let marks = marks();
    let (mut world, _) = moved_past_setting_up(false, marks.path()).await;

    world.poll().await;

    assert_eq!(
        world.harness.prompts(WorkKind::Update),
        Vec::<String>::new()
    );
    assert_eq!(lines_of(marks.path(), "ran"), Vec::<String>::new());
}

#[tokio::test]
async fn an_update_with_a_conflict_runs_the_setup_command_on_the_branch_before_updating_it() {
    let marks = marks();
    let (mut world, _) = moved_past_setting_up(true, marks.path()).await;

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

#[tokio::test]
async fn cancelling_a_run_during_its_setup_command_ends_the_command_at_once() {
    let marks = marks();
    let pid = marks.path().join("pid");
    let command = format!("echo $$ > '{}'; exec sleep 60", pid.display());
    let mut world =
        World::with(|config| config.work.flags_and_env.setup = Some(setup(&command))).await;
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

#[tokio::test]
async fn what_the_setup_command_leaves_running_is_ended_once_the_run_ends() {
    let marks = marks();
    let pid = marks.path().join("pid");
    let command = format!(
        "(while true; do echo still here; sleep 0.05; done) & echo $! > '{}'",
        pid.display()
    );
    let mut world =
        World::with(|config| config.work.flags_and_env.setup = Some(setup(&command))).await;
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

/// The files named `name` in the sessions' records under `world`'s work
/// directory.
fn records_named(world: &World, name: &str) -> Vec<PathBuf> {
    fn under(dir: &Path, name: &str, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                under(&path, name, found);
            } else if path.file_name().is_some_and(|file| file == name) {
                found.push(path);
            }
        }
    }
    let mut found = vec![];
    under(&world.dir.path().join("work/sessions"), name, &mut found);
    found
}

#[tokio::test]
async fn a_secret_in_the_setup_commands_output_is_redacted_in_its_log_and_the_hand_back() {
    let secret = "sk-setup-0123456789";
    let mut world = World::with(|config| {
        config.secrets = secrets_of(&[("API_KEY", secret)]);
        config.work.flags_and_env.setup = Some(setup(&format!("echo 'token {secret}'; exit 1")));
    })
    .await;
    let issue = open_for_triage(&world);

    world.poll().await;

    let issue = world.issue(issue);
    let comment = last_comment(&issue);
    assert!(comment.contains("token [redacted]"), "{comment}");
    assert!(!comment.contains(secret), "{comment}");
    let logs = records_named(&world, "setup.log");
    assert_eq!(logs.len(), 2, "one for each attempt");
    for log in logs {
        assert_eq!(std::fs::read_to_string(log).unwrap(), "token [redacted]\n");
    }
    let transcripts = records_named(&world, linewright::transcript::OUTPUT);
    assert_eq!(transcripts.len(), 2, "one for each attempt");
    for transcript in transcripts {
        let transcript = std::fs::read_to_string(transcript).unwrap();
        assert!(transcript.contains("token [redacted]"), "{transcript}");
        assert!(!transcript.contains(secret), "{transcript}");
    }
}

#[tokio::test]
async fn an_automations_run_whose_setup_command_fails_fails_with_the_end_of_its_output() {
    let world = World::new().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(
                ".linewright/automations/look.md",
                "---\non:\n  schedule: \"0 0 1 1 *\"\nsetup: echo 'no such tool'; exit 127\n---\nLook.\n",
            )],
            "Add an automation",
        )
        .await
        .unwrap();
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));

    let error = world
        .factory
        .run_automation(&"look".parse().unwrap())
        .await
        .unwrap_err();

    let error = format!("{error:#}");
    assert!(error.contains("status 127"), "{error}");
    assert!(
        error.contains("The end of the setup command's output:\nno such tool"),
        "{error}"
    );
    assert_eq!(world.harness.automation_prompts(), Vec::<String>::new());
}

#[tokio::test]
async fn a_bash_that_does_not_start_hands_back_after_a_retry_without_a_tail() {
    let mut world = World::with(|config| {
        config.bash = "/nowhere/bash".into();
        config.work.flags_and_env.setup = Some(setup("true"));
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
