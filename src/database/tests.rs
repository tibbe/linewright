use super::*;

use std::panic::Location;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt as _, BufReader};

use crate::automation::LastScheduledRuns;
use crate::domain::{Number, Timestamp};
use crate::environment::kept::{BuildId, Builds};
use crate::pull_records::{FailedMerge, PullRecords};
use tokio::process::{Child, Command};

/// The bytes of a database file's header that give its write and read
/// versions: 2 for WAL, 1 for a rollback journal.
/// <https://www.sqlite.org/fileformat2.html#the_database_header>
#[track_caller]
fn header_versions(db: &Path) -> [u8; 2] {
    let bytes = std::fs::read(db).unwrap();
    [bytes[18], bytes[19]]
}

#[test_log::test(tokio::test)]
async fn opening_makes_the_database_in_wal_mode() {
    let work_dir = tempfile::tempdir().unwrap();

    let _db = Database::open(work_dir.path()).await.unwrap();

    assert_eq!(
        header_versions(&work_dir.path().join("linewright.db")),
        [2, 2]
    );
}

#[test_log::test(tokio::test)]
async fn opening_an_existing_database_makes_nothing_where_there_is_none() {
    let work_dir = tempfile::tempdir().unwrap();

    let db = Database::open_existing(work_dir.path()).await.unwrap();

    assert!(db.is_none());
    assert_eq!(std::fs::read_dir(work_dir.path()).unwrap().count(), 0);
}

const NOTES: Migration = Migration::Sql("CREATE TABLE notes (text TEXT NOT NULL) STRICT");

/// The schema of a release before the one that adds a table of notes.
static RELEASES_ONLY: &[Migration] = &[RELEASES_TABLE];

/// The schema of a release after that one, which adds a table of notes.
static WITH_NOTES: &[Migration] = &[RELEASES_TABLE, NOTES];

static FAILING_AFTER_NOTES: &[Migration] = &[
    RELEASES_TABLE,
    NOTES,
    Migration::Steps(|tx, _| {
        Box::pin(async move {
            sqlx::query("INSERT INTO notes (text) VALUES ('half done')")
                .execute(&mut **tx)
                .await?;
            anyhow::bail!("the step fails")
        })
    }),
];

#[test_log::test(tokio::test)]
async fn a_failing_migration_leaves_the_database_as_it_was() {
    let work_dir = tempfile::tempdir().unwrap();
    drop(
        Database::open_in(work_dir.path(), RELEASES_ONLY, Absent::Make)
            .await
            .unwrap(),
    );

    let failed = Database::open_in(work_dir.path(), FAILING_AFTER_NOTES, Absent::Make).await;

    assert!(
        matches!(failed, Err(OpenError::Failed(_))),
        "{:?}",
        failed.map(|_| ())
    );
    let db = Database::open_in(work_dir.path(), WITH_NOTES, Absent::Make)
        .await
        .unwrap();
    let notes: i64 = sqlx::query_scalar("SELECT count(*) FROM notes")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(notes, 0);
}

#[test_log::test(tokio::test)]
async fn a_database_that_fails_to_be_made_is_not_left_behind() {
    let work_dir = tempfile::tempdir().unwrap();

    let failed = Database::open_in(work_dir.path(), FAILING_AFTER_NOTES, Absent::Make).await;

    assert!(failed.is_err(), "{:?}", failed.map(|_| ()));
    let db = Database::open_existing(work_dir.path()).await.unwrap();
    assert!(db.is_none());
}

#[test_log::test(tokio::test)]
async fn a_database_a_newer_release_made_is_refused_naming_that_release() {
    let work_dir = tempfile::tempdir().unwrap();
    let db = Database::open(work_dir.path()).await.unwrap();
    sqlx::raw_sql("PRAGMA user_version = 99; INSERT INTO releases VALUES (99, '9.9.9')")
        .execute(db.pool())
        .await
        .unwrap();
    drop(db);

    let refused = Database::open(work_dir.path()).await;

    let Err(e @ OpenError::Newer { version: 99, .. }) = refused else {
        panic!("{:?}", refused.map(|_| ()));
    };
    let message = e.to_string();
    assert!(message.contains("9.9.9"), "{message}");
    assert!(message.contains("restore a backup"), "{message}");
}

/// The variable that tells this test binary, started again, which work
/// directory [`another_process_holds_the_database`] opens.
const HOLD_DIR: &str = "LINEWRIGHT_TEST_HOLD_DIR";

/// The variable that tells [`another_process_holds_the_database`] whether to
/// stay in a read transaction.
const HOLD_READING: &str = "LINEWRIGHT_TEST_HOLD_READING";

/// What [`another_process_holds_the_database`] prints once it holds it.
const HOLDING: &str = "holding the database";

/// Starts this test binary again as another process that opens the work
/// directory's database with [`WITH_NOTES`], in a read transaction when
/// `reading`, and holds it until it is killed. It returns once the process
/// holds the database.
#[track_caller]
fn another_process(work_dir: &Path, reading: bool) -> impl Future<Output = Child> {
    let caller = Location::caller();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "database::tests::another_process_holds_the_database",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env(HOLD_DIR, work_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    if reading {
        command.env(HOLD_READING, "1");
    }
    async move {
        let mut child = command.spawn().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        while let Some(line) = lines.next_line().await.unwrap() {
            if line.contains(HOLDING) {
                tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
                return child;
            }
        }
        panic!(
            "the other process started at {caller} exited before it held the database: {:?}",
            child.wait().await
        );
    }
}

#[test_log::test(tokio::test)]
#[ignore = "another test starts it, as another process"]
async fn another_process_holds_the_database() {
    let work_dir = std::env::var_os(HOLD_DIR).expect("started by another test, with HOLD_DIR set");
    let db = Database::open_in(Path::new(&work_dir), WITH_NOTES, Absent::Make)
        .await
        .unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    if std::env::var_os(HOLD_READING).is_some() {
        let _: i64 = sqlx::query_scalar("SELECT count(*) FROM notes")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    }
    println!("{HOLDING}");
    tokio::io::AsyncReadExt::read_to_end(&mut tokio::io::stdin(), &mut Vec::new())
        .await
        .unwrap();
}

#[test_log::test(tokio::test)]
async fn another_process_reads_while_a_handle_writes() {
    let work_dir = tempfile::tempdir().unwrap();
    let _reader = another_process(work_dir.path(), true).await;
    let db = Database::open_in(work_dir.path(), WITH_NOTES, Absent::Make)
        .await
        .unwrap();

    let written = tokio::time::timeout(
        BUSY_TIMEOUT / 2,
        sqlx::query("INSERT INTO notes (text) VALUES ('written')").execute(db.pool()),
    )
    .await;

    assert!(matches!(written, Ok(Ok(_))), "{written:?}");
}

/// The schema of a release after the one [`another_process`] runs.
static NEWER_THAN_NOTES: &[Migration] = &[
    RELEASES_TABLE,
    NOTES,
    Migration::Sql("CREATE TABLE tags (name TEXT NOT NULL) STRICT"),
];

#[test_log::test(tokio::test)]
async fn a_migration_is_refused_while_another_process_has_the_database_open() {
    let work_dir = tempfile::tempdir().unwrap();
    let _other = another_process(work_dir.path(), false).await;

    let refused = Database::open_in(work_dir.path(), NEWER_THAN_NOTES, Absent::Make).await;

    let Err(e @ OpenError::OthersOpen) = refused else {
        panic!("{:?}", refused.map(|_| ()));
    };
    assert!(
        e.to_string()
            .contains("another Linewright process has the database open"),
        "{e}"
    );
}

#[test_log::test(tokio::test)]
async fn a_process_killed_with_sigkill_leaves_no_lock() {
    let work_dir = tempfile::tempdir().unwrap();
    let mut other = another_process(work_dir.path(), false).await;

    other.kill().await.unwrap();

    let db = Database::open_in(work_dir.path(), NEWER_THAN_NOTES, Absent::Make).await;
    assert!(db.is_ok(), "{:?}", db.map(|_| ()));
}

#[test_log::test(tokio::test)]
async fn a_handle_opens_a_current_schema_while_another_process_has_it_open() {
    let work_dir = tempfile::tempdir().unwrap();
    let _other = another_process(work_dir.path(), false).await;

    let db = Database::open_in(work_dir.path(), WITH_NOTES, Absent::Make).await;

    assert!(db.is_ok(), "{:?}", db.map(|_| ()));
}

#[test_log::test]
fn a_local_directory_is_on_no_network_filesystem() {
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(network_filesystem(dir.path()), None);
}

/// The schema of the release before the one that keeps the bookkeeping in
/// the database, which kept only the runs.
fn runs_only() -> &'static [Migration] {
    &MIGRATIONS[..2]
}

/// The work directory `dir`, its database made by the release before the
/// one that keeps the bookkeeping, then each of `files`, a path under it and
/// what it holds, written as that release left them.
#[track_caller]
fn upgraded_from<'a>(
    dir: &'a Path,
    files: &'a [(&str, &str)],
) -> impl Future<Output = Database> + 'a {
    let caller = Location::caller();
    async move {
        drop(
            Database::open_in(dir, runs_only(), Absent::Make)
                .await
                .unwrap_or_else(|e| panic!("making the earlier database at {caller}: {e:#}")),
        );
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Database::open(dir)
            .await
            .unwrap_or_else(|e| panic!("upgrading the database at {caller}: {e:#}"))
    }
}

#[test_log::test(tokio::test)]
async fn an_upgrade_keeps_the_record_of_linewrights_pull_requests_and_leaves_its_file() {
    let work_dir = tempfile::tempdir().unwrap();
    let file = r#"{
        "12": {"commented": "2026-10-01T10:00:00Z"},
        "14": {"failed_merges": [
            {"reason": "Head branch was modified.", "at": "2026-10-01T11:00:00Z"},
            {"reason": "Pull Request is not mergeable", "at": "2026-10-01T11:05:00Z"}
        ]}
    }"#;

    let db = upgraded_from(work_dir.path(), &[("pull-requests.json", file)]).await;

    let records = PullRecords::load(&db).await.unwrap().expect("kept");
    let at = |t: &str| t.parse::<Timestamp>().unwrap();
    assert_eq!(
        (
            records.last_commented(Number(12)),
            records.last_commented(Number(14)),
            records.failed_merges(Number(12)),
            records.failed_merges(Number(14)),
        ),
        (
            Some(at("2026-10-01T10:00:00Z")),
            None,
            &[][..],
            &[
                FailedMerge {
                    reason: "Head branch was modified.".into(),
                    at: at("2026-10-01T11:00:00Z"),
                },
                FailedMerge {
                    reason: "Pull Request is not mergeable".into(),
                    at: at("2026-10-01T11:05:00Z"),
                },
            ][..],
        )
    );
    assert!(work_dir.path().join("pull-requests.json").is_file());
}

#[test_log::test(tokio::test)]
async fn an_upgrade_skips_a_record_of_linewrights_pull_requests_that_does_not_read_which_reads_as_none_kept_until_one_is_saved()
 {
    let work_dir = tempfile::tempdir().unwrap();
    let db = upgraded_from(work_dir.path(), &[("pull-requests.json", "not JSON")]).await;

    let before = PullRecords::load(&db).await.unwrap();
    PullRecords::default().save(&db).await.unwrap();
    let after = PullRecords::load(&db).await.unwrap();

    assert_eq!((before, after), (None, Some(PullRecords::default())));
}

#[test_log::test(tokio::test)]
async fn an_upgrade_keeps_each_automations_last_scheduled_run() {
    let work_dir = tempfile::tempdir().unwrap();
    let file = r#"{"hourly": "2026-10-01T11:00:00Z", "daily": "2026-09-30T00:00:00Z"}"#;

    let db = upgraded_from(work_dir.path(), &[("automation-runs.json", file)]).await;

    let runs = LastScheduledRuns::load(&db).await.unwrap();
    let at = |t: &str| t.parse::<Timestamp>().unwrap();
    assert_eq!(
        [
            ("hourly", runs.get(&"hourly".parse().unwrap())),
            ("daily", runs.get(&"daily".parse().unwrap()))
        ],
        [
            ("hourly", Some(at("2026-10-01T11:00:00Z"))),
            ("daily", Some(at("2026-09-30T00:00:00Z")))
        ]
    );
}

#[test_log::test(tokio::test)]
async fn an_upgrade_keeps_the_last_scheduled_runs_a_release_kept_before_automations_were_renamed_only_where_there_are_no_later_ones()
 {
    let renamed = tempfile::tempdir().unwrap();
    let not_renamed = tempfile::tempdir().unwrap();
    let older = (
        "workflow-runs.json",
        r#"{"hourly": "2026-10-01T09:00:00Z"}"#,
    );
    let later = (
        "automation-runs.json",
        r#"{"hourly": "2026-10-01T11:00:00Z"}"#,
    );

    let both = upgraded_from(renamed.path(), &[older, later]).await;
    let only_older = upgraded_from(not_renamed.path(), &[older]).await;

    let hourly = "hourly".parse().unwrap();
    let at = |t: &str| t.parse::<Timestamp>().unwrap();
    assert_eq!(
        [
            LastScheduledRuns::load(&both).await.unwrap().get(&hourly),
            LastScheduledRuns::load(&only_older)
                .await
                .unwrap()
                .get(&hourly),
        ],
        [
            Some(at("2026-10-01T11:00:00Z")),
            Some(at("2026-10-01T09:00:00Z"))
        ]
    );
}

#[test_log::test(tokio::test)]
async fn an_upgrade_keeps_the_build_that_is_each_environment_and_whether_there_is_a_setup_command()
{
    let work_dir = tempfile::tempdir().unwrap();
    let served = crate::environment::dir(work_dir.path());
    let replayed = crate::environment::replays::dir(work_dir.path()).join("c0ffee-f1f1");
    for build in [served.join("builds/3/repo"), replayed.join("builds/1/repo")] {
        std::fs::create_dir_all(build).unwrap();
    }
    let state = |id: u64, commit: &str| {
        format!(
            r#"{{"id": {id}, "commit": "{commit}", "at": "2026-10-01T10:00:00Z", "fingerprint": "f1f1"}}"#
        )
    };
    let (served_state, replayed_state) = (state(3, "abc123"), state(1, "c0ffee"));

    let db = upgraded_from(
        work_dir.path(),
        &[
            ("environment/state.json", &served_state),
            ("environment/setup.json", r#"{"setup_command": true}"#),
            (
                "replays/environments/c0ffee-f1f1/state.json",
                &replayed_state,
            ),
        ],
    )
    .await;

    let [served, replayed] = [served, replayed].map(Builds::new);
    let mut kept = vec![];
    for builds in [&served, &replayed] {
        let built = builds.named(&db).await.unwrap().expect("named");
        kept.push((
            built.id,
            built.commit,
            built.at.to_rfc3339(),
            built.fingerprint.as_str().to_string(),
            builds.setup_given(&db).await.unwrap(),
        ));
    }
    let at = "2026-10-01T10:00:00+00:00".to_string();
    assert_eq!(
        kept,
        [
            (
                "3".parse::<BuildId>().unwrap(),
                "abc123".into(),
                at.clone(),
                "f1f1".into(),
                Some(true)
            ),
            (
                "1".parse::<BuildId>().unwrap(),
                "c0ffee".into(),
                at,
                "f1f1".into(),
                None
            ),
        ]
    );
}
