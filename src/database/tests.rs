use super::*;

use std::panic::Location;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt as _, BufReader};

use crate::automation::LastScheduledRuns;
use crate::benchmarks::{
    BenchmarkRunId, BenchmarkRunState, Cancelled, NoScore, ScoreFigures, TrialEnd, TrialFigures,
    TrialState,
};
use crate::cost::Usd;
use crate::environment::kept::Builds;
use crate::name::Name;
use crate::pull_records::PullRecords;
use crate::results::Verdict;
use crate::runs::OutcomeName;
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
                .get(&"hourly.md".parse().unwrap()),
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

/// The schema before ends of benchmark runs and the order of tasks were kept.
static BEFORE_ENDS_AND_ORDER: &[Migration] = MIGRATIONS.split_at(7).0;

#[test_log::test(tokio::test)]
async fn a_database_from_before_ends_and_order_were_kept_opens_with_ends_unknown_and_tasks_in_order()
 {
    let work_dir = tempfile::tempdir().unwrap();
    let before = Database::open_in(work_dir.path(), BEFORE_ENDS_AND_ORDER, Absent::Make)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO suites (name, stage) VALUES ('board', 'design'), ('rows', 'design');
         INSERT INTO cases (name, suite, run) VALUES ('lanes', 'board', '4/design-1'),
             ('stacks', 'rows', '5/design-1'), ('columns', 'board', '3/design-1');
         INSERT INTO experiments (id, suite, launched_at, repetitions, state)
             VALUES (1, 'board', '2026-11-17T12:00:00Z', 1, 'ended'),
                 (2, 'board', '2026-11-17T13:00:00Z', 1, 'open')",
    )
    .execute(before.pool())
    .await
    .unwrap();
    drop(before);

    let db = Database::open(work_dir.path()).await.unwrap();

    let benchmark = crate::benchmarks::Benchmarks::new(db.clone())
        .show("1".parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    let tasks: Vec<&str> = benchmark
        .tasks
        .iter()
        .map(|task| task.name.as_str())
        .collect();
    assert_eq!(tasks, ["lanes", "columns"]);
    let benchmark_runs = crate::benchmarks::BenchmarkRuns::new(db);
    let state = async |id: &str| {
        benchmark_runs
            .show(id.parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .state
    };
    assert_eq!(
        (state("1").await, state("2").await),
        (
            BenchmarkRunState::Ended { at: None },
            BenchmarkRunState::Open
        )
    );
}

/// The schema of Linewright 0.24.0, which kept benchmarks as suites, tasks as
/// cases and benchmark runs as experiments.
static BEFORE_BENCHMARK_NAMES: &[Migration] = MIGRATIONS.split_at(9).0;

/// The files of a work directory that Linewright 0.24.0 left: the task
/// `lanes`, and benchmark run 1's copy of it, the output of its first trial
/// with its score and its scorer's record, the output of a trial left as its
/// scorer ran, and an environment of its trials.
const FILES_OF_0_24_0: [(&str, &str); 10] = [
    ("cases/lanes/case.json", r#"{"stage": "design"}"#),
    ("cases/lanes/criteria.md", "Lanes for each stage.\n"),
    (
        "experiments/1/.kept/cases/lanes/case.json",
        r#"{"stage": "design"}"#,
    ),
    (
        "experiments/1/.kept/configurations/short/config.toml",
        "model = \"short\"\n",
    ),
    ("experiments/1/as-is/lanes/1/case", "4/design-1\n"),
    (
        "experiments/1/as-is/lanes/1/judgement.json",
        r#"{"session": {"record": "judge/20261117T120000Z-judge"}}"#,
    ),
    (
        "experiments/1/as-is/lanes/1/judge/20261117T120000Z-judge/transcript.jsonl",
        "{}\n",
    ),
    ("experiments/1/short/lanes/3/case", "4/design-1\n"),
    (
        "experiments/1/short/lanes/3/judge/20261117T130000Z-judge/transcript.jsonl",
        "{}\n",
    ),
    ("replays/environments/abc123-f1f1/held", ""),
];

/// Writes [`FILES_OF_0_24_0`] into `work_dir`.
#[track_caller]
fn write_files_of_0_24_0(work_dir: &Path) {
    for (path, text) in FILES_OF_0_24_0 {
        let path = work_dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

/// The paths of the files under `dir`, relative to it, in order.
#[track_caller]
fn files_under(dir: &Path) -> Vec<String> {
    #[track_caller]
    fn walk(dir: &Path, under: &Path, files: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, under, files);
            } else {
                files.push(path.strip_prefix(under).unwrap().display().to_string());
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, dir, &mut files);
    files.retain(|file| !file.starts_with("linewright.db"));
    files.sort();
    files
}

/// The files of [`FILES_OF_0_24_0`] at the paths this release keeps them,
/// with the task `lanes` as 1 and the configuration `short` as 2.
const FILES_UNDER_BENCHMARK_NAMES: [(&str, &str); 10] = [
    (
        "benchmark-runs/1/.kept/configurations/2/config.toml",
        "model = \"short\"\n",
    ),
    (
        "benchmark-runs/1/.kept/tasks/1/task.json",
        r#"{"stage": "design"}"#,
    ),
    (
        "benchmark-runs/1/1/1/1/judge/20261117T120000Z-judge/transcript.jsonl",
        "{}\n",
    ),
    (
        "benchmark-runs/1/1/1/1/score.json",
        r#"{"session": {"record": "judge/20261117T120000Z-judge"}}"#,
    ),
    ("benchmark-runs/1/1/1/1/task", "4/design-1\n"),
    (
        "benchmark-runs/1/2/1/3/scorer/20261117T130000Z-judge/transcript.jsonl",
        "{}\n",
    ),
    ("benchmark-runs/1/2/1/3/task", "4/design-1\n"),
    ("tasks/1/criteria.md", "Lanes for each stage.\n"),
    ("tasks/1/task.json", r#"{"stage": "design"}"#),
    ("trials/environments/abc123-f1f1/held", ""),
];

#[track_caller]
fn assert_files_under_benchmark_names(work_dir: &Path) {
    assert_eq!(
        files_under(work_dir),
        FILES_UNDER_BENCHMARK_NAMES.map(|(path, _)| path)
    );
    for (path, text) in FILES_UNDER_BENCHMARK_NAMES {
        assert_eq!(
            std::fs::read_to_string(work_dir.join(path)).unwrap(),
            text,
            "{path}"
        );
    }
}

#[test_log::test(tokio::test)]
async fn a_work_directory_of_0_24_0_opens_with_its_suites_cases_and_experiments_as_benchmarks_tasks_and_benchmark_runs()
 {
    let work_dir = tempfile::tempdir().unwrap();
    let before = Database::open_in(work_dir.path(), BEFORE_BENCHMARK_NAMES, Absent::Make)
        .await
        .unwrap();
    sqlx::raw_sql(
        "INSERT INTO suites (name, stage) VALUES ('board', 'design'), ('rows', 'triage');
         INSERT INTO cases (name, suite, run, seq) VALUES ('lanes', 'board', '4/design-1', 1),
             ('columns', 'board', '3/design-1', 2), ('stacks', 'rows', '5/triage-1', 1);
         INSERT INTO experiments (id, suite, launched_at, repetitions, state, ended_at)
             VALUES (1, 'board', '2026-11-17T12:00:00Z', 8, 'open', NULL),
                 (2, 'board', '2026-11-17T13:00:00Z', 1, 'ended', '2026-11-17T14:00:00Z');
         INSERT INTO experiment_cases (experiment, seq, name)
             VALUES (1, 0, 'lanes'), (2, 0, 'columns');
         INSERT INTO experiment_configurations (experiment, seq, name)
             VALUES (1, 0, 'as-is'), (1, 1, 'short'), (2, 0, 'as-is');
         INSERT INTO trials (experiment, repetition, case_seq, configuration_seq, state,
                 outcome, money, seconds, correctness, leak, judge_money, unjudged, reason)
             VALUES (1, 1, 0, 0, 'judged', 'done', 1.5, 60.0, 'pass', 0, 0.25, NULL, NULL),
                 (1, 1, 0, 1, 'judged', 'handed-back', NULL, 30.0, 'fail', 1, NULL, NULL, NULL),
                 (1, 2, 0, 0, 'unjudged', 'failed', 0.5, 10.0, NULL, NULL, NULL, 'outcome', NULL),
                 (1, 2, 0, 1, 'unjudged', 'done', 0.75, 20.0, NULL, NULL, 0.125, 'judge',
                     'it gave up'),
                 (1, 3, 0, 0, 'unjudged', NULL, NULL, NULL, NULL, NULL, NULL, 'cancelled', NULL),
                 (1, 3, 0, 1, 'judging', 'done', 2.0, 40.0, NULL, NULL, NULL, NULL, NULL),
                 (1, 4, 0, 0, 'replayed', 'handed-back', 1.0, 50.0, NULL, NULL, NULL, NULL, NULL),
                 (1, 4, 0, 1, 'replaying', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL),
                 (1, 5, 0, 0, 'pending', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);
         INSERT INTO environments (dir) VALUES ('replays/environments/abc123-f1f1'),
             ('environment');",
    )
    .execute(before.pool())
    .await
    .unwrap();
    drop(before);
    write_files_of_0_24_0(work_dir.path());

    let db = Database::open(work_dir.path()).await.unwrap();

    let benchmarks = crate::benchmarks::Benchmarks::new(db.clone());
    let listed: Vec<(String, usize)> = benchmarks
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|listed| (listed.benchmark.name.to_string(), listed.tasks))
        .collect();
    assert_eq!(listed, [("board".into(), 2), ("rows".into(), 1)]);
    let board = benchmarks
        .show("1".parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    let tasks: Vec<(&str, String)> = board
        .tasks
        .iter()
        .map(|task| (task.name.as_str(), task.run.to_string()))
        .collect();
    assert_eq!(
        tasks,
        [
            ("lanes", "4/design-1".into()),
            ("columns", "3/design-1".into())
        ]
    );
    let benchmark_runs = crate::benchmarks::BenchmarkRuns::new(db.clone());
    let of_board: Vec<BenchmarkRunId> = benchmark_runs
        .of_benchmark("1".parse().unwrap())
        .await
        .unwrap()
        .iter()
        .map(|record| record.id)
        .collect();
    assert_eq!(of_board, ["2".parse().unwrap(), "1".parse().unwrap()]);
    let record = benchmark_runs
        .show("1".parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            record
                .tasks
                .iter()
                .map(|task| task.name.as_str())
                .collect::<Vec<_>>(),
            record
                .configurations
                .iter()
                .map(Name::as_str)
                .collect::<Vec<_>>(),
            record.state,
        ),
        (
            vec!["lanes"],
            vec!["as-is", "short"],
            BenchmarkRunState::Open
        )
    );
    let figures = |outcome: &str, money: Option<f64>, seconds: u64| TrialFigures {
        outcome: serde_json::from_value::<OutcomeName>(serde_json::json!(outcome)).unwrap(),
        money: money.map(|money| Usd::new(money).unwrap()),
        time: Duration::from_secs(seconds),
    };
    assert_eq!(
        record.trials.values().cloned().collect::<Vec<_>>(),
        [
            TrialState::Ended(TrialEnd::Scored {
                run: figures("done", Some(1.5), 60),
                score: ScoreFigures {
                    correctness: Verdict::Pass,
                    leak: false,
                    scorer_money: Usd::new(0.25),
                },
            }),
            TrialState::Ended(TrialEnd::Scored {
                run: figures("handed-back", None, 30),
                score: ScoreFigures {
                    correctness: Verdict::Fail,
                    leak: true,
                    scorer_money: None,
                },
            }),
            TrialState::Ended(TrialEnd::Unscored(NoScore::Outcome(figures(
                "failed",
                Some(0.5),
                10,
            )))),
            TrialState::Ended(TrialEnd::Unscored(NoScore::Scorer {
                run: figures("done", Some(0.75), 20),
                why: "it gave up".into(),
                scorer_money: Usd::new(0.125),
            })),
            TrialState::Ended(TrialEnd::Cancelled(Cancelled::BeforeRun)),
            TrialState::Scoring {
                run: figures("done", Some(2.0), 40),
            },
            TrialState::Ran {
                run: figures("handed-back", Some(1.0), 50),
            },
            TrialState::Running,
            TrialState::Pending,
        ]
    );
    let environments: Vec<String> = sqlx::query_scalar("SELECT dir FROM environments ORDER BY dir")
        .fetch_all(db.pool())
        .await
        .unwrap();
    assert_eq!(
        environments,
        ["environment", "trials/environments/abc123-f1f1"]
    );
    assert_files_under_benchmark_names(work_dir.path());
}

#[test_log::test(tokio::test)]
async fn a_move_of_the_work_directory_that_fails_leaves_the_schema_as_it_was_and_the_next_open_finishes_it()
 {
    let work_dir = tempfile::tempdir().unwrap();
    let before = Database::open_in(work_dir.path(), BEFORE_BENCHMARK_NAMES, Absent::Make)
        .await
        .unwrap();
    sqlx::raw_sql(
        "INSERT INTO suites (name, stage) VALUES ('board', 'design');
         INSERT INTO cases (name, suite, run, seq) VALUES ('lanes', 'board', '4/design-1', 1);
         INSERT INTO experiments (id, suite, launched_at, repetitions, state, ended_at)
             VALUES (1, 'board', '2026-11-17T12:00:00Z', 3, 'ended', '2026-11-17T14:00:00Z');
         INSERT INTO experiment_cases (experiment, seq, name) VALUES (1, 0, 'lanes');
         INSERT INTO experiment_configurations (experiment, seq, name)
             VALUES (1, 0, 'as-is'), (1, 1, 'short');",
    )
    .execute(before.pool())
    .await
    .unwrap();
    drop(before);
    write_files_of_0_24_0(work_dir.path());
    let in_the_way = work_dir.path().join("experiments/1/.kept/tasks");
    std::fs::write(&in_the_way, "").unwrap();

    let failed = Database::open(work_dir.path()).await;

    let Err(OpenError::Failed(e)) = failed else {
        panic!("{:?}", failed.map(|_| ()));
    };
    let message = format!("{e:#}");
    assert!(
        message.contains(".kept/cases and ") && message.contains(".kept/tasks exist"),
        "{message}"
    );
    let unchanged = Database::open_in(work_dir.path(), BEFORE_BENCHMARK_NAMES, Absent::Leave)
        .await
        .unwrap();
    let suites: Vec<String> = sqlx::query_scalar("SELECT name FROM suites")
        .fetch_all(unchanged.pool())
        .await
        .unwrap();
    assert_eq!(suites, ["board"]);
    drop(unchanged);
    std::fs::remove_file(work_dir.path().join("benchmark-runs/1/.kept/tasks")).unwrap();
    let db = Database::open(work_dir.path()).await.unwrap();
    let benchmarks: Vec<String> = crate::benchmarks::Benchmarks::new(db)
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|listed| listed.benchmark.name.to_string())
        .collect();
    assert_eq!(benchmarks, ["board"]);
    assert_files_under_benchmark_names(work_dir.path());
}

/// The schema of the release before benchmarks and tasks had IDs.
static BEFORE_IDS: &[Migration] = MIGRATIONS.split_at(12).0;

/// Opens a database in `work_dir` with the schema [`BEFORE_IDS`] makes, and
/// keeps `rows` in it.
#[track_caller]
fn before_ids<'a>(work_dir: &'a Path, rows: &'static str) -> impl Future<Output = ()> + 'a {
    let caller = Location::caller();
    async move {
        let before = Database::open_in(work_dir, BEFORE_IDS, Absent::Make)
            .await
            .unwrap_or_else(|e| panic!("opening the database at {caller}: {e:#}"));
        sqlx::raw_sql(rows)
            .execute(before.pool())
            .await
            .unwrap_or_else(|e| panic!("keeping the rows at {caller}: {e}"));
    }
}

/// The rows of a work directory before IDs: the benchmarks `board` and
/// `rows`; the tasks `lanes`, `stacks` and `columns`, captured in that order;
/// and benchmark run 1 on `board`, launched on `lanes` and on `gone`, a task
/// since removed, under `as-is` and `short`.
const ROWS_BEFORE_IDS: &str = "INSERT INTO benchmarks (name, stage)
        VALUES ('board', 'design'), ('rows', 'triage');
    INSERT INTO tasks (name, benchmark, run, seq) VALUES ('lanes', 'board', '4/design-1', 1),
        ('stacks', 'rows', '5/triage-1', 1), ('columns', 'board', '3/design-1', 2);
    INSERT INTO benchmark_runs (id, benchmark, launched_at, repetitions, state, ended_at)
        VALUES (1, 'board', '2026-11-17T12:00:00Z', 1, 'ended', '2026-11-17T14:00:00Z');
    INSERT INTO benchmark_run_tasks (benchmark_run, seq, name) VALUES (1, 0, 'lanes'), (1, 1, 'gone');
    INSERT INTO benchmark_run_configurations (benchmark_run, seq, name)
        VALUES (1, 0, 'as-is'), (1, 1, 'short');
    INSERT INTO trials (benchmark_run, repetition, task_seq, configuration_seq, state,
            outcome, money, seconds, correctness, leak, scorer_money, unscored, reason)
        VALUES (1, 1, 0, 0, 'scored', 'done', 1.5, 60.0, 'pass', 0, 0.25, NULL, NULL),
            (1, 1, 0, 1, 'unscored', 'failed', 0.5, 10.0, NULL, NULL, NULL, 'outcome', NULL),
            (1, 1, 1, 0, 'unscored', NULL, NULL, NULL, NULL, NULL, NULL, 'cancelled', NULL),
            (1, 1, 1, 1, 'unscored', NULL, NULL, NULL, NULL, NULL, NULL, 'cancelled', NULL);";

/// The files of the work directory of [`ROWS_BEFORE_IDS`], each holding its
/// own path.
const FILES_BEFORE_IDS: [&str; 10] = [
    "tasks/lanes/criteria.md",
    "tasks/stacks/criteria.md",
    "tasks/columns/criteria.md",
    "benchmark-runs/1/.kept/tasks/lanes/criteria.md",
    "benchmark-runs/1/.kept/tasks/gone/criteria.md",
    "benchmark-runs/1/.kept/configurations/short/config.toml",
    "benchmark-runs/1/as-is/lanes/1/task",
    "benchmark-runs/1/short/lanes/1/task",
    "benchmark-runs/1/as-is/gone/1/task",
    "trials/environments/abc123-f1f1/held",
];

/// Writes each of `paths` under `work_dir`, holding its path.
#[track_caller]
fn write_own_paths(work_dir: &Path, paths: &[&str]) {
    for path in paths {
        let file = work_dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, path).unwrap();
    }
}

/// The files under `work_dir`, each with what it holds.
#[track_caller]
fn files_with_text(work_dir: &Path) -> Vec<(String, String)> {
    files_under(work_dir)
        .into_iter()
        .map(|file| {
            let text = std::fs::read_to_string(work_dir.join(&file)).unwrap();
            (file, text)
        })
        .collect()
}

/// What [`FILES_BEFORE_IDS`] are at once each is at its ID's path, with what
/// they held.
fn files_by_ids() -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = [
        ("tasks/1/criteria.md", "tasks/lanes/criteria.md"),
        ("tasks/2/criteria.md", "tasks/stacks/criteria.md"),
        ("tasks/3/criteria.md", "tasks/columns/criteria.md"),
        (
            "benchmark-runs/1/.kept/tasks/1/criteria.md",
            "benchmark-runs/1/.kept/tasks/lanes/criteria.md",
        ),
        (
            "benchmark-runs/1/.kept/tasks/4/criteria.md",
            "benchmark-runs/1/.kept/tasks/gone/criteria.md",
        ),
        (
            "benchmark-runs/1/.kept/configurations/2/config.toml",
            "benchmark-runs/1/.kept/configurations/short/config.toml",
        ),
        (
            "benchmark-runs/1/1/1/1/task",
            "benchmark-runs/1/as-is/lanes/1/task",
        ),
        (
            "benchmark-runs/1/2/1/1/task",
            "benchmark-runs/1/short/lanes/1/task",
        ),
        (
            "benchmark-runs/1/1/4/1/task",
            "benchmark-runs/1/as-is/gone/1/task",
        ),
        (
            "trials/environments/abc123-f1f1/held",
            "trials/environments/abc123-f1f1/held",
        ),
    ]
    .into_iter()
    .map(|(path, text)| (path.to_string(), text.to_string()))
    .collect();
    files.sort();
    files
}

#[test_log::test(tokio::test)]
async fn a_work_directory_from_before_ids_opens_with_each_benchmark_task_copy_and_output_at_its_id()
{
    let work_dir = tempfile::tempdir().unwrap();
    before_ids(work_dir.path(), ROWS_BEFORE_IDS).await;
    write_own_paths(work_dir.path(), &FILES_BEFORE_IDS);

    let db = Database::open(work_dir.path()).await.unwrap();

    assert_eq!(files_with_text(work_dir.path()), files_by_ids());
    let benchmarks = crate::benchmarks::Benchmarks::new(db.clone());
    let listed: Vec<(String, String)> = benchmarks
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|listed| {
            (
                listed.benchmark.id.to_string(),
                listed.benchmark.name.to_string(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [("1".into(), "board".into()), ("2".into(), "rows".into())]
    );
    let board = benchmarks
        .show("1".parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    let tasks: Vec<(String, &str, String)> = board
        .tasks
        .iter()
        .map(|task| {
            (
                task.id.to_string(),
                task.name.as_str(),
                task.criteria.as_ref().unwrap().as_str().to_string(),
            )
        })
        .collect();
    assert_eq!(
        tasks,
        [
            ("1".into(), "lanes", "tasks/lanes/criteria.md".into()),
            ("3".into(), "columns", "tasks/columns/criteria.md".into())
        ]
    );
    let record = crate::benchmarks::BenchmarkRuns::new(db)
        .show("1".parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    let run_tasks: Vec<(String, &str)> = record
        .tasks
        .iter()
        .map(|task| (task.id.to_string(), task.name.as_str()))
        .collect();
    assert_eq!(run_tasks, [("1".into(), "lanes"), ("4".into(), "gone")]);
    assert_eq!(
        (
            record.benchmark.id.to_string(),
            record
                .configurations
                .iter()
                .map(Name::as_str)
                .collect::<Vec<_>>(),
            record.trials.len()
        ),
        ("1".into(), vec!["as-is", "short"], 4)
    );
    assert_eq!(
        benchmarks
            .removed_task("4".parse().unwrap())
            .await
            .unwrap()
            .map(|removed| removed.name.to_string()),
        Some("gone".into())
    );
}

#[test_log::test(tokio::test)]
async fn a_task_named_as_another_tasks_id_moves_to_its_own_id() {
    let work_dir = tempfile::tempdir().unwrap();
    before_ids(
        work_dir.path(),
        "INSERT INTO benchmarks (name, stage) VALUES ('board', 'design');
         INSERT INTO tasks (name, benchmark, run, seq) VALUES ('a', 'board', '1/design-1', 1),
             ('5', 'board', '2/design-1', 2), ('b', 'board', '3/design-1', 3),
             ('c', 'board', '4/design-1', 4), ('2', 'board', '5/design-1', 5);",
    )
    .await;
    write_own_paths(
        work_dir.path(),
        &[
            "tasks/a/criteria.md",
            "tasks/5/criteria.md",
            "tasks/b/criteria.md",
            "tasks/c/criteria.md",
            "tasks/2/criteria.md",
        ],
    );

    let _db = Database::open(work_dir.path()).await.unwrap();

    assert_eq!(
        files_with_text(work_dir.path()),
        [
            ("tasks/1/criteria.md", "tasks/a/criteria.md"),
            ("tasks/2/criteria.md", "tasks/5/criteria.md"),
            ("tasks/3/criteria.md", "tasks/b/criteria.md"),
            ("tasks/4/criteria.md", "tasks/c/criteria.md"),
            ("tasks/5/criteria.md", "tasks/2/criteria.md"),
        ]
        .map(|(path, text)| (path.to_string(), text.to_string()))
    );
}

#[test_log::test(tokio::test)]
async fn a_move_to_ids_that_fails_part_way_leaves_the_schema_as_it_was_and_the_next_open_finishes_it()
 {
    let work_dir = tempfile::tempdir().unwrap();
    before_ids(work_dir.path(), ROWS_BEFORE_IDS).await;
    write_own_paths(work_dir.path(), &FILES_BEFORE_IDS);
    let in_the_way = work_dir.path().join("benchmark-runs/1/2");
    std::fs::write(&in_the_way, "").unwrap();

    let failed = Database::open(work_dir.path()).await;

    let Err(OpenError::Failed(e)) = failed else {
        panic!("{:?}", failed.map(|_| ()));
    };
    assert!(format!("{e:#}").contains("benchmark-runs/1/2"), "{e:#}");
    let unchanged = Database::open_in(work_dir.path(), BEFORE_IDS, Absent::Leave)
        .await
        .unwrap();
    let benchmarks: Vec<String> = sqlx::query_scalar("SELECT name FROM benchmarks")
        .fetch_all(unchanged.pool())
        .await
        .unwrap();
    assert_eq!(benchmarks, ["board", "rows"]);
    drop(unchanged);
    std::fs::remove_file(&in_the_way).unwrap();
    let db = Database::open(work_dir.path()).await.unwrap();
    assert_eq!(files_with_text(work_dir.path()), files_by_ids());
    let record = crate::benchmarks::BenchmarkRuns::new(db)
        .show("1".parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record
            .tasks
            .iter()
            .map(|task| task.name.as_str())
            .collect::<Vec<_>>(),
        ["lanes", "gone"]
    );
}

/// A later change of the schema that makes `tasks` again, as it is.
const TASKS_MADE_AGAIN: Migration = Migration::Sql(
    "CREATE TABLE tasks_again (
        id INTEGER PRIMARY KEY CHECK (id > 0),
        benchmark INTEGER NOT NULL REFERENCES benchmarks (id),
        name TEXT NOT NULL,
        run TEXT NOT NULL,
        seq INTEGER NOT NULL CHECK (seq > 0),
        UNIQUE (benchmark, seq),
        UNIQUE (benchmark, name)
    ) STRICT;
    INSERT INTO tasks_again SELECT * FROM tasks;
    DROP TABLE tasks;
    ALTER TABLE tasks_again RENAME TO tasks;",
);

/// Keeps a task named `name` captured from `run` in `benchmark`, built in
/// the work directory of `benchmarks`.
#[track_caller]
fn kept_task<'a>(
    benchmarks: &'a crate::benchmarks::Benchmarks,
    benchmark: &'a crate::benchmarks::Benchmark,
    name: &'a str,
) -> impl Future<Output = crate::benchmarks::TaskId> + 'a {
    let caller = Location::caller();
    async move {
        std::fs::create_dir_all(benchmarks.tasks_dir()).unwrap();
        let built = tempfile::tempdir_in(benchmarks.tasks_dir()).unwrap();
        std::fs::write(built.path().join("criteria.md"), name).unwrap();
        let kept = benchmarks
            .keep_task(
                &crate::benchmarks::ChosenBenchmark::Existing(benchmark.clone()),
                &name.parse().unwrap(),
                &"4/design-1".parse().unwrap(),
                built.path(),
            )
            .await
            .unwrap_or_else(|e| panic!("keeping the task at {caller}: {e:#}"));
        let _moved = built.keep();
        kept.id
    }
}

#[test_log::test(tokio::test)]
async fn a_tasks_id_is_never_given_again_after_its_removal_nor_after_tasks_is_made_again() {
    let work_dir = tempfile::tempdir().unwrap();
    let db = Database::open(work_dir.path()).await.unwrap();
    let benchmarks = crate::benchmarks::Benchmarks::new(db);
    let board = benchmarks
        .make(&"board".parse().unwrap(), crate::task::TaskStage::Design)
        .await
        .unwrap();
    kept_task(&benchmarks, &board, "lanes").await;
    let columns = kept_task(&benchmarks, &board, "columns").await;
    benchmarks.remove_task(board.id, columns).await.unwrap();

    let after_removal = kept_task(&benchmarks, &board, "rows").await;
    drop(benchmarks);
    let made_again: &'static [Migration] = Box::leak(
        MIGRATIONS
            .iter()
            .copied()
            .chain([TASKS_MADE_AGAIN])
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    let again = crate::benchmarks::Benchmarks::new(
        Database::open_in(work_dir.path(), made_again, Absent::Leave)
            .await
            .unwrap(),
    );
    again.remove_task(board.id, after_removal).await.unwrap();
    let after_made_again = kept_task(&again, &board, "stacks").await;

    assert_eq!(
        [columns, after_removal, after_made_again].map(|id| id.to_string()),
        ["2", "3", "4"]
    );
}

#[test_log::test(tokio::test)]
async fn a_task_gets_no_id_that_names_a_directory_of_no_task() {
    let work_dir = tempfile::tempdir().unwrap();
    before_ids(
        work_dir.path(),
        "INSERT INTO benchmarks (name, stage) VALUES ('board', 'design');
         INSERT INTO tasks (name, benchmark, run, seq) VALUES ('lanes', 'board', '1/design-1', 1),
             ('columns', 'board', '2/design-1', 2);",
    )
    .await;
    write_own_paths(
        work_dir.path(),
        &[
            "tasks/1/criteria.md",
            "tasks/lanes/criteria.md",
            "tasks/columns/criteria.md",
        ],
    );

    let db = Database::open(work_dir.path()).await.unwrap();

    assert_eq!(
        files_with_text(work_dir.path()),
        [
            ("tasks/1/criteria.md", "tasks/1/criteria.md"),
            ("tasks/2/criteria.md", "tasks/lanes/criteria.md"),
            ("tasks/3/criteria.md", "tasks/columns/criteria.md"),
        ]
        .map(|(path, text)| (path.to_string(), text.to_string()))
    );
    let board = crate::benchmarks::Benchmarks::new(db)
        .show("1".parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    let tasks: Vec<(String, &str)> = board
        .tasks
        .iter()
        .map(|task| (task.id.to_string(), task.name.as_str()))
        .collect();
    assert_eq!(tasks, [("2".into(), "lanes"), ("3".into(), "columns")]);
}

#[test_log::test(tokio::test)]
async fn moves_left_part_way_are_not_carried_on_once_the_database_gives_others() {
    let work_dir = tempfile::tempdir().unwrap();
    before_ids(work_dir.path(), ROWS_BEFORE_IDS).await;
    write_own_paths(work_dir.path(), &FILES_BEFORE_IDS);
    let in_the_way = work_dir.path().join("benchmark-runs/1/2");
    std::fs::write(&in_the_way, "").unwrap();
    let failed = Database::open(work_dir.path()).await;
    assert!(failed.is_err(), "{:?}", failed.map(|_| ()));
    let before = Database::open_in(work_dir.path(), BEFORE_IDS, Absent::Leave)
        .await
        .unwrap();
    sqlx::raw_sql("DELETE FROM tasks WHERE name = 'stacks'")
        .execute(before.pool())
        .await
        .unwrap();
    drop(before);
    std::fs::remove_file(&in_the_way).unwrap();

    let refused = Database::open(work_dir.path()).await;

    let Err(OpenError::Failed(e)) = refused else {
        panic!("{:?}", refused.map(|_| ()));
    };
    assert!(
        format!("{e:#}").contains("are not those the database gives now"),
        "{e:#}"
    );
    assert!(work_dir.path().join("benchmark-runs/1/1/1/1/task").exists());
}

/// The schema before an automation was named by its path.
static BEFORE_AUTOMATION_PATHS: &[Migration] = MIGRATIONS.split_at(16).0;

#[test_log::test(tokio::test)]
async fn a_database_from_before_automations_were_named_by_their_paths_opens_with_each_named_by_its_file()
 {
    let work_dir = tempfile::tempdir().unwrap();
    let before = Database::open_in(work_dir.path(), BEFORE_AUTOMATION_PATHS, Absent::Make)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO automation_schedules (name, last_scheduled_run)
             VALUES ('weekly', '2026-11-16T06:00:00Z');
         INSERT INTO runs (kind, automation, n, started_by, started_at)
             VALUES ('automation', 'weekly', 3, 'hand', '2026-11-16T09:00:00Z');
         INSERT INTO runs (kind, item, stage, n, started_at)
             VALUES ('item', 7, 'triage', 1, '2026-11-16T10:00:00Z')",
    )
    .execute(before.pool())
    .await
    .unwrap();
    drop(before);

    let db = Database::open(work_dir.path()).await.unwrap();

    let last = LastScheduledRuns::load(&db).await.unwrap();
    assert_eq!(
        last.get(&"weekly.md".parse().unwrap()),
        Some(parse_time("2026-11-16T06:00:00Z").unwrap())
    );
    let mut ids: Vec<String> = crate::runs::Runs::new(db)
        .read()
        .await
        .unwrap()
        .iter()
        .map(|run| run.id.to_string())
        .collect();
    ids.sort();
    assert_eq!(ids, ["7/triage-1", "weekly.md/3"]);
}

static BEFORE_WORKFLOW_RUN_TRIGGERS: &[Migration] = MIGRATIONS.split_at(17).0;

#[test_log::test(tokio::test)]
async fn a_database_from_before_workflow_run_triggers_opens_with_its_runs_sessions_and_inputs() {
    let work_dir = tempfile::tempdir().unwrap();
    let before = Database::open_in(work_dir.path(), BEFORE_WORKFLOW_RUN_TRIGGERS, Absent::Make)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO runs (id, kind, automation, n, started_by, started_at, ended_at, ending)
             VALUES (1, 'automation', 'weekly.md', 3, 'hand', '2026-11-16T09:00:00Z',
                 '2026-11-16T09:10:00Z', 'done');
         INSERT INTO runs (id, kind, item, stage, n, started_at)
             VALUES (2, 'item', 7, 'triage', 1, '2026-11-16T10:00:00Z');
         INSERT INTO sessions (run, seq, dir) VALUES (2, 0, 'sessions/issue-7/1');
         INSERT INTO run_inputs (run, input) VALUES (2, '{}')",
    )
    .execute(before.pool())
    .await
    .unwrap();
    drop(before);

    let db = Database::open(work_dir.path()).await.unwrap();

    let runs = crate::runs::Runs::new(db.clone()).read().await.unwrap();
    let read: Vec<(String, Option<crate::runs::StartedBy>, usize)> = runs
        .iter()
        .map(|run| (run.id.to_string(), run.started().by(), run.sessions.len()))
        .collect();
    assert_eq!(
        read,
        [
            ("weekly.md/3".into(), Some(crate::runs::StartedBy::Hand), 0),
            ("7/triage-1".into(), None, 1),
        ]
    );
    sqlx::query("DELETE FROM runs WHERE id = 2")
        .execute(db.pool())
        .await
        .unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(left, 0, "a run's sessions go with it");
    let refused = sqlx::query(
        "INSERT INTO runs (kind, automation, n, started_by, started_at)
             VALUES ('automation', 'weekly.md', 3, 'hand', '2026-11-16T11:00:00Z')",
    )
    .execute(db.pool())
    .await;
    assert!(refused.is_err(), "an automation's runs are numbered once");
    let without_its_attempt = sqlx::query(
        "INSERT INTO runs (kind, automation, n, started_by, workflow_run, started_at)
             VALUES ('automation', 'fix-ci.md', 1, 'workflow-run', 9, '2026-11-16T11:00:00Z')",
    )
    .execute(db.pool())
    .await;
    assert!(
        without_its_attempt.is_err(),
        "a run an attempt started names its run and attempt"
    );
}
