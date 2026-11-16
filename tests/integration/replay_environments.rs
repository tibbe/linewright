//! The environments of replays: one per case head and setup command, shared
//! and aged out across replays (`docs/specs/benchmarks.md`, Replaying).

use crate::common;
use crate::environment::Marks;

use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::TimeDelta;
use common::*;
use linewright::clock::Clock as _;
use linewright::database::Database;
use linewright::domain::Timestamp;
use linewright::environment::replays::{self, BuildFailed, InUse, Unprepared};
use linewright::environment::{self, Builder, Environment, SetupRead};
use linewright::git::CommitId;
use linewright::redaction::Redaction;
use linewright::session_env::{PassEnv, SessionEnv, SetEnv};
use linewright::store::Store as _;
use linewright::work_settings::{SetupCommand, SetupSettings};

const MAX_AGE: TimeDelta = TimeDelta::days(1);

/// What builds `world`'s repository in the work directory of `database`,
/// with `GREETING` set to `greeting`.
#[track_caller]
fn builder(world: &World, database: &Database, greeting: &str) -> Builder {
    let given = SessionEnv::new(
        PassEnv::default(),
        SetEnv::new([("GREETING".to_string(), greeting.to_string())]).unwrap(),
    )
    .split([], &[])
    .given;
    Builder {
        store: world.store.clone(),
        harness: world.harness.clone(),
        bash: bash(),
        clock: world.clock.clone(),
        redaction: Redaction::default(),
        given: Arc::new(given),
        default_branch: "main".into(),
        winding_up: linewright::winding_up::WindingUp::default(),
        database: database.clone(),
    }
}

#[track_caller]
fn setup(command: &str) -> SetupSettings {
    SetupSettings {
        command: SetupCommand::new(command.into()).unwrap(),
        max_age: Duration::from_hours(24),
        timeout: Duration::from_mins(60),
    }
}

/// The head of the default branch of `world`'s repository.
#[track_caller]
fn head(world: &World) -> impl Future<Output = CommitId> + '_ {
    let caller = Location::caller();
    async move {
        let heads = world
            .store
            .heads(&["main".to_string()])
            .await
            .unwrap_or_else(|e| panic!("reading the head at {caller} failed: {e:#}"));
        heads
            .get("main")
            .unwrap_or_else(|| panic!("the repository has no `main` at {caller}"))
            .clone()
    }
}

/// The environments of replays of `world`'s repository, from its head, in
/// a work directory.
struct Replays<'a> {
    world: &'a World,
    database: Database,
    base: PathBuf,
    head: CommitId,
}

impl Replays<'_> {
    /// The environments of replays in the work directory `work_dir`.
    #[track_caller]
    fn new<'a>(world: &'a World, work_dir: &Path) -> impl Future<Output = Replays<'a>> + use<'a> {
        let caller = Location::caller();
        let head = head(world);
        let work_dir = work_dir.to_path_buf();
        async move {
            let database = Database::open(&work_dir)
                .await
                .unwrap_or_else(|e| panic!("opening the database at {caller} failed: {e:#}"));
            Replays {
                world,
                database,
                base: replays::dir(&work_dir),
                head: head.await,
            }
        }
    }

    /// Prepares the environment with `command`, with `GREETING` set to
    /// `greeting`, at `now`.
    fn prepare<'a>(
        &'a self,
        command: Option<&'a str>,
        greeting: &'a str,
        now: Timestamp,
    ) -> impl Future<Output = Result<InUse, Unprepared>> + 'a {
        replays::prepare(
            &self.base,
            &self.head,
            builder(self.world, &self.database, greeting),
            command.map(setup),
            now,
            MAX_AGE,
        )
    }

    /// Prepares the environment as [`Replays::prepare`] does, which is
    /// ready.
    #[track_caller]
    fn ready<'a>(
        &'a self,
        command: Option<&'a str>,
        greeting: &'a str,
        now: Timestamp,
    ) -> impl Future<Output = InUse> + 'a {
        let caller = Location::caller();
        async move {
            self.prepare(command, greeting, now)
                .await
                .unwrap_or_else(|e| panic!("preparing at {caller} failed: {e:#}"))
        }
    }

    /// The environments kept, by name.
    #[track_caller]
    fn kept(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.base)
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

/// What a session copying from `in_use` finds in `file`, or `None` without
/// an environment.
#[track_caller]
fn copied<'a>(in_use: &'a InUse, file: &'a str) -> impl Future<Output = Option<String>> + 'a {
    let caller = Location::caller();
    async move {
        let lease = in_use.source().lease()?;
        let dest = tempfile::tempdir()
            .unwrap_or_else(|e| panic!("making a directory at {caller} failed: {e}"));
        let copy = dest.path().join("clone");
        lease
            .copy_to(&copy)
            .await
            .unwrap_or_else(|e| panic!("copying the environment at {caller} failed: {e:#}"));
        let read = std::fs::read_to_string(copy.join(file))
            .unwrap_or_else(|e| panic!("reading `{file}` at {caller} failed: {e}"));
        Some(read)
    }
}

/// What a session copying from `in_use` finds in `node_modules/x`, or
/// `None` without an environment.
#[track_caller]
fn installed(in_use: &InUse) -> impl Future<Output = Option<String>> + '_ {
    copied(in_use, "node_modules/x")
}

/// A setup command that notes each build in `marks`, then installs
/// `node_modules/x` holding `version` and the greeting.
fn installing(marks: &Marks, version: &str) -> String {
    format!(
        "echo built >> '{}'; mkdir -p node_modules && echo \"{version} $GREETING\" > node_modules/x",
        marks.path("builds").display()
    )
}

#[test_log::test(tokio::test)]
async fn two_replays_of_the_same_head_and_setup_build_once_the_second_waiting_for_the_first() {
    let marks = Marks::new();
    let world = World::new().await;
    let work_dir = tempfile::tempdir().unwrap();
    let replays = Replays::new(&world, work_dir.path()).await;
    let gate = marks.path("go");
    let command = format!(
        "{}; while [ ! -e '{}' ]; do sleep 0.05; done",
        installing(&marks, "v1"),
        gate.display()
    );
    let now = world.clock.now();

    let first = replays.ready(Some(&command), "hi", now);
    let second = async {
        while marks.builds() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mut second = std::pin::pin!(replays.ready(Some(&command), "hi", now));
        let waited = tokio::time::timeout(Duration::from_millis(300), &mut second).await;
        assert!(waited.is_err(), "the second replay did not wait");
        std::fs::write(&gate, "").unwrap();
        second.await
    };
    let (first, second) = tokio::join!(first, second);

    assert_eq!(marks.builds(), 1);
    assert_eq!(installed(&first).await.as_deref(), Some("v1 hi\n"));
    assert_eq!(installed(&second).await.as_deref(), Some("v1 hi\n"));
    assert!(first.built_output().is_some());
    assert_eq!(second.built_output(), None);
}

#[test_log::test(tokio::test)]
async fn a_changed_setup_command_or_variable_builds_a_new_environment_beside_the_old() {
    let marks = Marks::new();
    let world = World::new().await;
    let work_dir = tempfile::tempdir().unwrap();
    let replays = Replays::new(&world, work_dir.path()).await;
    let now = world.clock.now();
    let (v1, v2) = (installing(&marks, "v1"), installing(&marks, "v2"));

    let first = replays.ready(Some(&v1), "hi", now).await;
    let command_changed = replays.ready(Some(&v2), "hi", now).await;
    let variable_changed = replays.ready(Some(&v1), "bye", now).await;
    let as_first = replays.ready(Some(&v1), "hi", now).await;

    assert_eq!(marks.builds(), 3);
    assert_eq!(replays.kept().len(), 3);
    let mut seen = vec![];
    for in_use in [first, command_changed, variable_changed, as_first] {
        seen.push(installed(&in_use).await);
    }
    assert_eq!(
        seen,
        ["v1 hi\n", "v2 hi\n", "v1 bye\n", "v1 hi\n"].map(|x| Some(x.to_string()))
    );
}

#[test_log::test]
fn an_environment_is_outlived_once_older_than_max_age_or_without_a_build() {
    let built = at("2026-11-01T12:00:00Z");

    assert!(!replays::outlived(
        Some(built),
        at("2026-11-02T12:00:00Z"),
        MAX_AGE
    ));
    assert!(replays::outlived(
        Some(built),
        at("2026-11-02T12:00:01Z"),
        MAX_AGE
    ));
    assert!(replays::outlived(None, built, MAX_AGE));
}

#[test_log::test(tokio::test)]
async fn an_environment_older_than_max_age_is_removed_at_the_next_replay_unless_one_uses_it() {
    let marks = Marks::new();
    let world = World::new().await;
    let work_dir = tempfile::tempdir().unwrap();
    let replays = Replays::new(&world, work_dir.path()).await;
    let start = world.clock.now();
    let later = start + MAX_AGE + TimeDelta::minutes(1);
    let (unused, used) = (installing(&marks, "v1"), installing(&marks, "v2"));
    replays.ready(Some(&unused), "hi", start).await;
    let in_use = replays.ready(Some(&used), "hi", start).await;
    let source = in_use.source();
    drop(in_use);

    let next = replays.ready(None, "hi", later).await;

    assert_eq!(installed(&next).await, None);
    assert_eq!(replays.kept().len(), 1);
    let used_again = replays.ready(Some(&used), "hi", later).await;
    assert_eq!(marks.builds(), 2);
    assert_eq!(installed(&used_again).await.as_deref(), Some("v2 hi\n"));
    drop((source, used_again));
    let unused_again = replays.ready(Some(&unused), "hi", later).await;
    assert_eq!(marks.builds(), 3);
    assert_eq!(installed(&unused_again).await.as_deref(), Some("v1 hi\n"));
    assert_eq!(replays.kept().len(), 1);
}

#[test_log::test(tokio::test)]
async fn a_failed_build_gives_its_output_and_a_later_replay_builds_anew() {
    let marks = Marks::new();
    let world = World::new().await;
    let work_dir = tempfile::tempdir().unwrap();
    let replays = Replays::new(&world, work_dir.path()).await;
    let now = world.clock.now();
    let fail = marks.path("fail");
    let command = format!(
        "{}; if [ -e '{}' ]; then echo 'npm ERR! missing lockfile'; exit 3; fi",
        installing(&marks, "v1"),
        fail.display()
    );
    std::fs::write(&fail, "").unwrap();

    let failed = replays.prepare(Some(&command), "hi", now).await;
    std::fs::remove_file(&fail).unwrap();
    let later = replays.ready(Some(&command), "hi", now).await;

    let Err(Unprepared::Failed(BuildFailed { why, output })) = failed else {
        panic!("the build did not fail");
    };
    assert_eq!(output.as_deref(), Some("npm ERR! missing lockfile\n"));
    assert!(
        why.starts_with("the setup command exited with status 3;"),
        "{why}"
    );
    assert_eq!(marks.builds(), 2);
    assert_eq!(installed(&later).await.as_deref(), Some("v1 hi\n"));
}

#[test_log::test(tokio::test)]
async fn an_environment_is_built_from_the_head_it_is_given() {
    let world = World::new().await;
    let work_dir = tempfile::tempdir().unwrap();
    let replays = Replays::new(&world, work_dir.path()).await;
    world
        .store
        .commit("maintainer", "main", &[("README.md", "later\n")], "Change")
        .await
        .unwrap();

    let in_use = replays
        .ready(Some("git rev-parse HEAD > head"), "hi", world.clock.now())
        .await;

    assert_eq!(
        copied(&in_use, "head").await,
        Some(format!("{}\n", replays.head))
    );
}

#[test_log::test(tokio::test)]
async fn a_replay_leaves_the_environment_of_serve_as_it_is() {
    let marks = Marks::new();
    let world = World::new().await;
    let work_dir = tempfile::tempdir().unwrap();
    let start = world.clock.now();
    let serving = environment::dir(work_dir.path());
    let database = Database::open(work_dir.path()).await.unwrap();
    let mut served = Environment::open(serving.clone(), builder(&world, &database, "hi"))
        .await
        .unwrap();
    let command = installing(&marks, "v1");
    served
        .rebuild_now(SetupRead::Given(Some(setup(&command))))
        .await
        .unwrap();
    let before = files_under(&serving);
    let replays = Replays::new(&world, work_dir.path()).await;

    let in_use = replays
        .ready(Some(&command), "hi", start + MAX_AGE * 2)
        .await;

    assert_eq!(files_under(&serving), before);
    assert_eq!(installed(&in_use).await.as_deref(), Some("v1 hi\n"));
    assert_eq!(marks.builds(), 2);
}

/// The paths of the files under `dir`, sorted.
#[track_caller]
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = vec![];
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(next) = dirs.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() && !path.is_symlink() {
                dirs.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test_log::test(tokio::test)]
async fn after_an_upgrade_from_files_a_replay_uses_the_environment_an_earlier_replay_built_until_it_is_old()
 {
    let marks = Marks::new();
    let world = World::new().await;
    let earlier = tempfile::tempdir().unwrap();
    let command = installing(&marks, "v1");
    let start = world.clock.now();
    let built = Replays::new(&world, earlier.path()).await;
    drop(built.ready(Some(&command), "hi", start).await);
    let [name] = built.kept().try_into().unwrap();
    let upgraded = tempfile::tempdir().unwrap();
    environment_as_an_earlier_release_left(
        earlier.path(),
        &format!("replays/environments/{name}"),
        upgraded.path(),
    )
    .await;

    let replays = Replays::new(&world, upgraded.path()).await;
    let reused = replays.ready(Some(&command), "hi", start).await;

    assert_eq!(marks.builds(), 1, "not built again");
    assert_eq!(installed(&reused).await.as_deref(), Some("v1 hi\n"));
    drop(reused);
    let later = start + MAX_AGE + TimeDelta::minutes(1);
    // A child another test forks holds this process's locks until it execs:
    // https://man7.org/linux/man-pages/man2/flock.2.html
    for _ in 0..50 {
        replays.ready(None, "hi", later).await;
        if replays.kept().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(replays.kept(), Vec::<String>::new());
    let pool = back_door(upgraded.path()).await;
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM environments")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "the removed environment's row is removed too");
}
