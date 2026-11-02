//! The environment: the repository made ready for agents once, by the setup
//! command, that every session's clone starts from
//! (`docs/specs/sessions.md`, Environment).

use crate::common;

use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::TimeDelta;
use common::*;
use linewright::explain::{explain, render};
use linewright::factory::FactoryConfig;
use linewright::reconcile::WorkKind;
use linewright::session_env::{PassEnv, SessionEnv, SetEnv};
use linewright::webhook::Pending;

const CONFIG: &str = ".linewright/config.toml";

/// Commits `files` to the default branch.
#[track_caller]
fn commit<'a>(world: &'a World, files: &'a [(&str, &str)]) -> impl Future<Output = ()> + 'a {
    let caller = Location::caller();
    async move {
        if let Err(e) = world
            .store
            .commit("maintainer", "main", files, "Change")
            .await
        {
            panic!("committing at {caller} failed: {e:#}");
        }
    }
}

/// The repository's file with `command` as its setup command.
fn setup_config(command: &str) -> String {
    format!("[setup]\ncommand = '''{command}'''\n")
}

/// Ignores `node_modules` and sets `command` as the setup command.
#[track_caller]
fn set_up_with<'a>(world: &'a World, command: &str) -> impl Future<Output = ()> + 'a {
    let config = setup_config(command);
    let caller = Location::caller();
    async move {
        let files = [(".gitignore", "node_modules/\n"), (CONFIG, config.as_str())];
        if let Err(e) = world
            .store
            .commit("maintainer", "main", &files, "Change")
            .await
        {
            panic!("setting up at {caller} failed: {e:#}");
        }
    }
}

/// A directory outside every clone, where setup commands leave marks.
struct Marks(tempfile::TempDir);

impl Marks {
    #[track_caller]
    fn new() -> Marks {
        Marks(tempfile::tempdir().unwrap())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }

    /// A setup command that notes each build, then installs `node_modules/x`
    /// holding `version`.
    fn installing(&self, version: &str) -> String {
        format!(
            "echo built >> '{}'; mkdir -p node_modules && echo {version} > node_modules/x",
            self.path("builds").display()
        )
    }

    /// How many builds [`Marks::installing`]'s commands have run.
    fn builds(&self) -> usize {
        std::fs::read_to_string(self.path("builds"))
            .unwrap_or_default()
            .lines()
            .count()
    }
}

/// What each implement session found in its clone's `node_modules/x`,
/// `None` when missing. Each answers that it failed.
fn implement_reading(world: &World) -> Arc<Mutex<Vec<Option<String>>>> {
    let seen = Arc::new(Mutex::new(vec![]));
    world.harness.on(WorkKind::Implement, {
        let seen = seen.clone();
        move |call| {
            let installed = std::fs::read_to_string(call.spec.dir.join("node_modules/x")).ok();
            seen.lock().unwrap().push(installed);
            failed_session("looked")
        }
    });
    seen
}

/// What the implement sessions found in `node_modules/x`.
#[track_caller]
fn installed(seen: &Mutex<Vec<Option<String>>>) -> Vec<Option<String>> {
    seen.lock().unwrap().clone()
}

#[test_log::test(tokio::test)]
async fn a_run_waits_for_the_first_build_then_its_clone_starts_from_the_environment() {
    let marks = Marks::new();
    let world = World::new().await;
    let gate = marks.path("go");
    set_up_with(
        &world,
        &format!(
            "while [ ! -e '{}' ]; do sleep 0.05; done; \
             mkdir -p node_modules && touch node_modules/x; echo changed > README.md",
            gate.display()
        ),
    )
    .await;
    let (sessions, mut started) = tokio::sync::mpsc::unbounded_channel();
    world.harness.on(WorkKind::Implement, move |call| {
        let read = |path: &str| std::fs::read_to_string(call.spec.dir.join(path)).ok();
        let _ = sessions.send((read("node_modules/x"), read("README.md")));
        failed_session("looked")
    });
    let ticket = open_ticket(&world, "Do the thing");
    let World {
        factory,
        clock,
        harness: _harness,
        store: _store,
        dir: _dir,
    } = world;
    let mut serving = Serving::start(factory, Duration::from_hours(1), Pending::default());

    let served = serving.served.wait_for(Option::is_some).await.unwrap();
    let view = served.as_ref().unwrap().view();
    drop(served);
    let explained = render(
        &explain(
            &view,
            &[],
            ticket,
            linewright::clock::Clock::now(clock.as_ref()),
        )
        .unwrap(),
        false,
    );
    std::fs::write(&gate, "").unwrap();
    let (installed, readme) = tokio::time::timeout(Duration::from_secs(20), started.recv())
        .await
        .expect("a session started once the build succeeded")
        .unwrap();
    serving.stop().await;

    assert!(
        explained.contains("waiting for: the environment, until a build succeeds"),
        "{explained}"
    );
    assert_eq!(installed.as_deref(), Some(""));
    assert_eq!(readme.as_deref(), Some("# Widgets\n"));
}

#[test_log::test(tokio::test)]
async fn without_a_setup_command_a_sessions_clone_is_a_plain_clone_of_its_branch() {
    let mut world = World::new().await;
    commit(&world, &[(".gitignore", "node_modules/\n")]).await;
    let seen = implement_reading(&world);
    open_ticket(&world, "Do the thing");

    world.poll().await;

    assert_eq!(installed(&seen), [None]);
}

#[test_log::test(tokio::test)]
async fn a_first_build_that_fails_leaves_runs_waiting_until_the_next_trigger_builds_one() {
    let marks = Marks::new();
    let mut world = World::new().await;
    let failing = format!("{}; exit 1", marks.installing("v1"));
    set_up_with(&world, &failing).await;
    let seen = implement_reading(&world);
    open_ticket(&world, "Do the thing");

    world.poll().await;
    world.poll().await;

    assert_eq!(installed(&seen), Vec::<Option<String>>::new());

    commit(&world, &[(CONFIG, &setup_config(&marks.installing("v2")))]).await;
    world.poll().await;
    world.poll().await;

    assert_eq!(marks.builds(), 2);
    assert_eq!(installed(&seen), [Some("v2\n".to_string())]);
}

#[test_log::test(tokio::test)]
async fn a_rebuild_that_fails_leaves_sessions_starting_from_the_last_good_environment() {
    let marks = Marks::new();
    let mut world = World::new().await;
    set_up_with(&world, &marks.installing("v1")).await;
    world.poll().await;
    let failing = format!("{}; exit 1", marks.installing("v2"));
    commit(&world, &[(CONFIG, &setup_config(&failing))]).await;
    world.poll().await;
    let seen = implement_reading(&world);
    open_ticket(&world, "Do the thing");

    world.poll().await;

    assert_eq!(marks.builds(), 2);
    assert_eq!(installed(&seen), [Some("v1\n".to_string())]);
}

/// Sets `GREETING` to `greeting` for every session, and for builds.
#[track_caller]
fn greeting(config: &mut FactoryConfig, greeting: &str) {
    let session_env = SessionEnv::new(
        PassEnv::default(),
        SetEnv::new([("GREETING".to_string(), greeting.to_string())]).unwrap(),
    );
    config.environments = config.work.environments(&session_env, [], &[]).environments;
}

#[test_log::test(tokio::test)]
async fn what_the_environment_was_built_from_changing_or_its_age_passing_max_age_rebuilds_it() {
    let marks = Marks::new();
    let mut world = World::with(|config| greeting(config, "hello")).await;
    let command = |version: &str| {
        format!(
            "{}; echo \"$GREETING\" >> node_modules/x",
            marks.installing(version)
        )
    };
    set_up_with(&world, &command("v1")).await;
    world.poll().await;

    commit(&world, &[(CONFIG, &setup_config(&command("v2")))]).await;
    world.poll().await;
    assert_eq!(marks.builds(), 2, "the setup command changed");

    world.restart(|config| greeting(config, "hello")).await;
    world.poll().await;
    assert_eq!(
        marks.builds(),
        2,
        "a restart with nothing changed builds nothing"
    );

    world.restart(|config| greeting(config, "hi")).await;
    world.poll().await;
    assert_eq!(marks.builds(), 3, "a variable changed while serve was down");

    commit(&world, &[(CONFIG, &setup_config(&command("v3")))]).await;
    world.restart(|config| greeting(config, "hi")).await;
    world.poll().await;
    assert_eq!(
        marks.builds(),
        4,
        "the command changed while serve was down"
    );

    world.clock.advance(TimeDelta::hours(25));
    world.poll().await;
    assert_eq!(marks.builds(), 5, "older than max-age");

    let seen = implement_reading(&world);
    open_ticket(&world, "Do the thing");
    world.poll().await;
    assert_eq!(installed(&seen), [Some("v3\nhi\n".to_string())]);
}

#[test_log::test(tokio::test)]
async fn a_path_ignored_in_the_environment_but_tracked_on_the_sessions_branch_keeps_the_branchs_content()
 {
    let mut world = World::new().await;
    commit(
        &world,
        &[
            (".gitignore", "generated.txt\nnode_modules/\n"),
            (
                CONFIG,
                &setup_config(
                    "echo environment > generated.txt; mkdir -p node_modules; \
                     echo environment > node_modules/x; echo environment > node_modules/y",
                ),
            ),
        ],
    )
    .await;
    let (ticket, pr) = world.ticket_in_review().await;
    let branch = format!("{}-do-the-thing", ticket.0);
    world
        .store
        .commit(
            LINEWRIGHT,
            &branch,
            &[
                ("generated.txt", "branch\n"),
                ("node_modules/x", "branch\n"),
            ],
            "Track what the environment ignores",
        )
        .await
        .unwrap();
    world.poll().await;
    world
        .store
        .comment_as("maintainer", pr, "Rename it.")
        .unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    world.harness.on(WorkKind::Rework, {
        let seen = seen.clone();
        move |call| {
            let read = |path: &str| std::fs::read_to_string(call.spec.dir.join(path)).unwrap();
            seen.lock().unwrap().push([
                read("generated.txt"),
                read("node_modules/x"),
                read("node_modules/y"),
            ]);
            failed_session("looked")
        }
    });

    world.poll().await;

    assert_eq!(
        seen.lock()
            .unwrap()
            .first()
            .expect("a rework session started"),
        &["branch\n", "branch\n", "environment\n"]
    );
}

/// Every file under `dir`, but in the environment's clones.
#[track_caller]
fn kept_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = vec![];
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name != "repo") {
                files.extend(kept_files(&path));
            }
        } else {
            files.push(path);
        }
    }
    files
}

#[test_log::test(tokio::test)]
async fn no_secret_is_kept_in_a_builds_record_its_output_or_the_state_file() {
    let secret = "sk-registry-0123456789";
    let mut world = World::with(|config| {
        config.secrets = secrets_of(&[("REGISTRY_TOKEN", secret)]);
        let session_env = SessionEnv::new(
            PassEnv::default(),
            SetEnv::new([("REGISTRY_TOKEN".to_string(), secret.to_string())]).unwrap(),
        );
        config.environments = config.work.environments(&session_env, [], &[]).environments;
    })
    .await;
    set_up_with(
        &world,
        &format!("echo \"token $REGISTRY_TOKEN\"; echo 'and {secret}'"),
    )
    .await;

    world.poll().await;

    let environment = world.dir.path().join("work/environment");
    let kept = kept_files(&environment);
    assert!(
        kept.iter().any(|file| file.ends_with("state.json")),
        "{kept:?}"
    );
    let output = kept
        .iter()
        .find(|file| file.ends_with("output.log"))
        .expect("the build's output is kept");
    assert_eq!(
        std::fs::read_to_string(output).unwrap(),
        "token [redacted]\nand [redacted]\n"
    );
    for file in kept {
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains(secret), "{}: {text}", file.display());
    }
}

#[test_log::test(tokio::test)]
async fn a_build_in_progress_is_abandoned_when_serve_exits() {
    let marks = Marks::new();
    let world = World::new().await;
    let started = marks.path("started");
    set_up_with(&world, &format!("touch '{}'; sleep 60", started.display())).await;
    let work = world.dir.path().join("work");
    let serving = Serving::start(world.factory, Duration::from_hours(1), Pending::default());
    eventually(|| started.exists()).await;

    tokio::time::timeout(Duration::from_secs(10), serving.stop())
        .await
        .expect("serve exits without waiting for the build");

    let builds = work.join("environment/builds");
    eventually(|| {
        std::fs::read_dir(&builds)
            .map(std::iter::Iterator::count)
            .unwrap_or(0)
            == 0
    })
    .await;
}

#[test_log::test(tokio::test)]
async fn without_serve_a_run_uses_the_environment_when_no_trigger_applies_and_builds_it_first_otherwise()
 {
    let marks = Marks::new();
    let mut world = World::new().await;
    set_up_with(&world, &marks.installing("v1")).await;
    world.factory.prepare_environment().await.unwrap();
    assert_eq!(marks.builds(), 1);
    world.restart(|_| {}).await;
    let seen = implement_reading(&world);
    open_ticket(&world, "Do the thing");

    world.factory.prepare_environment().await.unwrap();
    world.poll().await;

    assert_eq!(marks.builds(), 1);
    assert_eq!(installed(&seen), [Some("v1\n".to_string())]);
}

#[test_log::test(tokio::test)]
async fn without_an_environment_once_starts_no_session_and_an_automation_run_fails_naming_the_build()
 {
    let mut world = World::new().await;
    set_up_with(&world, "echo 'npm ERR! missing lockfile'; exit 3").await;
    commit(&world, &[(".linewright/automations/hourly.md", HOURLY)]).await;
    let automations = Arc::new(Mutex::new(0));
    world.harness.on_automation({
        let automations = automations.clone();
        move |_| {
            *automations.lock().unwrap() += 1;
            Ok(nothing_needs_doing())
        }
    });
    let seen = implement_reading(&world);
    open_ticket(&world, "Do the thing");

    let prepared = world.factory.prepare_environment().await.unwrap_err();
    world.poll().await;
    world.restart(|_| {}).await;
    let ran = world
        .factory
        .run_automation(&"hourly".parse().unwrap())
        .await
        .unwrap_err();

    for error in [format!("{prepared:#}"), format!("{ran:#}")] {
        assert!(
            error.contains("the build of the environment failed"),
            "{error}"
        );
        assert!(error.contains("status 3"), "{error}");
    }
    assert!(installed(&seen).is_empty(), "no session started");
    assert_eq!(*automations.lock().unwrap(), 0);
}

#[test_log::test(tokio::test)]
async fn a_config_file_linewright_cannot_use_starts_no_build() {
    let marks = Marks::new();
    let mut world = World::new().await;
    set_up_with(&world, &marks.installing("v1")).await;
    world.poll().await;
    commit(&world, &[(CONFIG, "[setup]\ncommand = 3.5\n")]).await;

    world.poll().await;

    assert_eq!(marks.builds(), 1, "an unusable file starts no build");
}
