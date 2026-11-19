use super::*;

use std::panic::Location;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt as _, BufReader};

use crate::automation::LastScheduledRuns;
use crate::benchmarks::ExperimentState;
use crate::environment::kept::Builds;
use crate::pull_records::PullRecords;
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
    Migration::Sql(
        "INSERT INTO notes (text) VALUES ('half done');
        INSERT INTO missing (text) VALUES ('fails')",
    ),
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

#[test_log::test(tokio::test)]
async fn the_record_of_linewrights_pull_requests_reads_as_none_kept_until_one_is_saved() {
    let work_dir = tempfile::tempdir().unwrap();
    let db = Database::open(work_dir.path()).await.unwrap();

    let before = PullRecords::load(&db).await.unwrap();
    PullRecords::default().save(&db).await.unwrap();
    let after = PullRecords::load(&db).await.unwrap();

    assert_eq!((before, after), (None, Some(PullRecords::default())));
}

#[test_log::test(tokio::test)]
async fn a_new_database_keeps_none_of_the_records_linewright_0_22_0_kept_in_files_and_leaves_them()
{
    let work_dir = tempfile::tempdir().unwrap();
    let files = [
        (
            "pull-requests.json",
            r#"{"12": {"commented": "2026-10-01T10:00:00Z"}}"#,
        ),
        (
            "automation-runs.json",
            r#"{"hourly": "2026-10-01T11:00:00Z"}"#,
        ),
        (
            "environment/state.json",
            r#"{"id": 3, "commit": "abc123", "at": "2026-10-01T10:00:00Z", "fingerprint": "f1f1"}"#,
        ),
        (
            "runs/items/1/triage-1.json",
            r#"{"item": 1, "stage": "triage", "n": 1, "started_at": "2026-10-14T12:00:00Z",
                "end": {"at": "2026-10-14T12:01:00Z", "ending": "done"}, "sessions": []}"#,
        ),
    ];
    for (path, text) in files {
        let path = work_dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    std::fs::create_dir_all(work_dir.path().join("environment/builds/3/repo")).unwrap();

    let db = Database::open(work_dir.path()).await.unwrap();

    let served = Builds::new(crate::environment::dir(work_dir.path()));
    assert_eq!(
        (
            PullRecords::load(&db).await.unwrap(),
            LastScheduledRuns::load(&db)
                .await
                .unwrap()
                .get(&"hourly".parse().unwrap()),
            served.named(&db).await.unwrap().map(|built| built.id),
            crate::runs::Runs::new(db.clone())
                .read()
                .await
                .unwrap()
                .len(),
        ),
        (None, None, None, 0)
    );
    for (path, text) in files {
        assert_eq!(
            std::fs::read_to_string(work_dir.path().join(path)).unwrap(),
            text
        );
    }
}

/// The schema before ends of experiments were kept.
static BEFORE_ENDS: &[Migration] = MIGRATIONS.split_at(7).0;

#[test_log::test(tokio::test)]
async fn a_database_from_before_ends_were_kept_opens_with_its_experiments_ended_without_one() {
    let work_dir = tempfile::tempdir().unwrap();
    let before = Database::open_in(work_dir.path(), BEFORE_ENDS, Absent::Make)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO suites (name, stage) VALUES ('board', 'design');
         INSERT INTO experiments (id, suite, launched_at, repetitions, state)
             VALUES (1, 'board', '2026-11-17T12:00:00Z', 1, 'ended'),
                 (2, 'board', '2026-11-17T13:00:00Z', 1, 'open')",
    )
    .execute(before.pool())
    .await
    .unwrap();
    drop(before);

    let db = Database::open(work_dir.path()).await.unwrap();

    let experiments = crate::benchmarks::Experiments::new(db);
    let state = async |id: &str| {
        experiments
            .show(id.parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .state
    };
    assert_eq!(
        (state("1").await, state("2").await),
        (ExperimentState::Ended { at: None }, ExperimentState::Open)
    );
}
