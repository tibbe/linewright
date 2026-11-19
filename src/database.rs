//! Linewright's own records: one SQLite database in the work directory, which
//! every Linewright process on the host may have open at once.

use std::cmp::Ordering;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, SecondsFormat};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::domain::Timestamp;

/// The database's file, in the work directory.
const DATABASE_FILE: &str = "linewright.db";

/// The file beside the database that every handle holds a shared `flock`
/// on, and that a handle changing the schema holds exclusively.
const LOCK_FILE: &str = "linewright.db.lock";

/// How long a statement waits for another connection's write lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// A change of the schema, from the version before it to its own.
pub(crate) enum Migration {
    Sql(&'static str),
}

/// Every change of the schema, in order: the schema's version is how many of
/// them a database has had. The schema a released migration makes never
/// changes; a later change is a new entry.
static MIGRATIONS: &[Migration] = &[
    RELEASES_TABLE,
    Migration::Sql(RUNS_TABLES),
    Migration::Sql(BOOKKEEPING_TABLES),
    Migration::Sql(SESSION_COSTS),
    Migration::Sql(PUSHED_BRANCHES_TABLES),
    Migration::Sql(SUITES_TABLES),
    Migration::Sql(EXPERIMENTS_TABLES),
    Migration::Sql(EXPERIMENT_ENDS),
    Migration::Sql(CASES_IN_ORDER),
];

/// The table of the release that made each schema version.
const RELEASES_TABLE: Migration = Migration::Sql(
    "CREATE TABLE releases (
        version INTEGER PRIMARY KEY,
        release TEXT NOT NULL
    ) STRICT",
);

/// Each run outside a replay, its sessions and the input it recorded.
const RUNS_TABLES: &str = "CREATE TABLE runs (
        id INTEGER PRIMARY KEY,
        kind TEXT NOT NULL CHECK (kind IN ('item', 'automation')),
        item INTEGER,
        stage TEXT,
        automation TEXT,
        n INTEGER NOT NULL CHECK (n > 0),
        started_by TEXT CHECK (started_by IN ('schedule', 'hand')),
        started_at TEXT NOT NULL,
        ended_at TEXT,
        ending TEXT CHECK (ending IN ('done', 'handed-back', 'cancelled', 'usage-limit', 'failed')),
        answered_until TEXT,
        CHECK (
            (kind = 'item' AND item IS NOT NULL AND stage IS NOT NULL
                AND automation IS NULL AND started_by IS NULL)
            OR (kind = 'automation' AND automation IS NOT NULL AND started_by IS NOT NULL
                AND item IS NULL AND stage IS NULL)
        ),
        CHECK ((ended_at IS NULL) = (ending IS NULL))
    ) STRICT;
    CREATE UNIQUE INDEX runs_of_items ON runs (item, stage, n) WHERE kind = 'item';
    CREATE UNIQUE INDEX runs_of_automations ON runs (automation, n) WHERE kind = 'automation';
    CREATE TABLE sessions (
        run INTEGER NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
        seq INTEGER NOT NULL,
        dir TEXT NOT NULL,
        ran TEXT,
        PRIMARY KEY (run, seq)
    ) STRICT;
    CREATE TABLE run_inputs (
        run INTEGER PRIMARY KEY REFERENCES runs (id) ON DELETE CASCADE,
        input TEXT NOT NULL
    ) STRICT;";

/// Linewright's record of its pull requests, each automation's last
/// scheduled run and the state of each environment.
const BOOKKEEPING_TABLES: &str = "CREATE TABLE kept (
        record TEXT PRIMARY KEY CHECK (record IN ('pull-records'))
    ) STRICT;
    CREATE TABLE pull_records (
        pr INTEGER PRIMARY KEY,
        commented_at TEXT
    ) STRICT;
    CREATE TABLE failed_merges (
        pr INTEGER NOT NULL REFERENCES pull_records (pr) ON DELETE CASCADE,
        seq INTEGER NOT NULL,
        reason TEXT NOT NULL,
        at TEXT NOT NULL,
        PRIMARY KEY (pr, seq)
    ) STRICT;
    CREATE TABLE automation_schedules (
        name TEXT PRIMARY KEY,
        last_scheduled_run TEXT NOT NULL
    ) STRICT;
    CREATE TABLE environments (
        dir TEXT PRIMARY KEY,
        build INTEGER,
        built_from TEXT,
        built_at TEXT,
        fingerprint TEXT,
        setup_command INTEGER CHECK (setup_command IN (0, 1)),
        CHECK (
            (build IS NULL) = (built_from IS NULL)
            AND (build IS NULL) = (built_at IS NULL)
            AND (build IS NULL) = (fingerprint IS NULL)
        )
    ) STRICT;";

/// What each session cost, `NULL` for one recorded before costs were kept.
const SESSION_COSTS: &str = "ALTER TABLE sessions ADD COLUMN cost TEXT";

/// The ticket branches Linewright pushed, and whether those it pushed before
/// it kept this record are adopted into it.
const PUSHED_BRANCHES_TABLES: &str = "CREATE TABLE pushed_branches (
        name TEXT PRIMARY KEY,
        recorded_at TEXT NOT NULL
    ) STRICT;
    CREATE TABLE adopted (
        record TEXT PRIMARY KEY CHECK (record IN ('pushed-branches'))
    ) STRICT;";

/// The suites, and the case each holds by name, whose directory is in
/// `cases/`.
const SUITES_TABLES: &str = "CREATE TABLE suites (
        name TEXT PRIMARY KEY,
        stage TEXT NOT NULL CHECK (stage IN ('triage', 'design', 'implement'))
    ) STRICT;
    CREATE TABLE cases (
        name TEXT PRIMARY KEY,
        suite TEXT NOT NULL REFERENCES suites (name),
        run TEXT NOT NULL
    ) STRICT;
    CREATE INDEX cases_of_suites ON cases (suite);";

/// The experiments, each with the cases and configurations it was launched
/// with, in order, and its trials, whose copies and outputs are in
/// `experiments/`. A trial's row holds the figures of its replay and its
/// judge once its state has them, and only then: a trial cancelled before
/// its replay ended has none.
const EXPERIMENTS_TABLES: &str = "CREATE TABLE experiments (
        id INTEGER PRIMARY KEY CHECK (id > 0),
        suite TEXT NOT NULL REFERENCES suites (name),
        launched_at TEXT NOT NULL,
        repetitions INTEGER NOT NULL CHECK (repetitions > 0),
        state TEXT NOT NULL CHECK (state IN ('open', 'ended', 'cancelled'))
    ) STRICT;
    CREATE INDEX experiments_of_suites ON experiments (suite);
    CREATE TABLE experiment_cases (
        experiment INTEGER NOT NULL REFERENCES experiments (id),
        seq INTEGER NOT NULL CHECK (seq >= 0),
        name TEXT NOT NULL,
        PRIMARY KEY (experiment, seq),
        UNIQUE (experiment, name)
    ) STRICT;
    CREATE TABLE experiment_configurations (
        experiment INTEGER NOT NULL REFERENCES experiments (id),
        seq INTEGER NOT NULL CHECK (seq >= 0),
        name TEXT NOT NULL,
        PRIMARY KEY (experiment, seq),
        UNIQUE (experiment, name),
        CHECK ((seq = 0) = (name = 'as-is'))
    ) STRICT;
    CREATE TABLE trials (
        experiment INTEGER NOT NULL REFERENCES experiments (id),
        repetition INTEGER NOT NULL CHECK (repetition > 0),
        case_seq INTEGER NOT NULL,
        configuration_seq INTEGER NOT NULL,
        state TEXT NOT NULL CHECK (state IN
            ('pending', 'replaying', 'replayed', 'judging', 'judged', 'unjudged')),
        outcome TEXT CHECK (outcome IN ('done', 'handed-back', 'cancelled', 'usage-limit',
            'failed', 'in-flight', 'interrupted')),
        money REAL CHECK (money >= 0),
        seconds REAL CHECK (seconds >= 0),
        correctness TEXT CHECK (correctness IN ('pass', 'fail')),
        leak INTEGER CHECK (leak IN (0, 1)),
        judge_money REAL CHECK (judge_money >= 0),
        unjudged TEXT CHECK (unjudged IN ('outcome', 'judge', 'cancelled')),
        reason TEXT,
        PRIMARY KEY (experiment, repetition, case_seq, configuration_seq),
        FOREIGN KEY (experiment, case_seq) REFERENCES experiment_cases (experiment, seq),
        FOREIGN KEY (experiment, configuration_seq)
            REFERENCES experiment_configurations (experiment, seq),
        CHECK ((outcome IS NULL) = (seconds IS NULL)),
        CHECK (money IS NULL OR outcome IS NOT NULL),
        CHECK (state NOT IN ('pending', 'replaying') OR outcome IS NULL),
        CHECK (state NOT IN ('replayed', 'judging', 'judged') OR outcome IS NOT NULL),
        CHECK ((correctness IS NOT NULL) = (state = 'judged')),
        CHECK ((leak IS NOT NULL) = (state = 'judged')),
        CHECK (judge_money IS NULL OR state = 'judged' OR unjudged IS 'judge'),
        CHECK ((unjudged IS NOT NULL) = (state = 'unjudged')),
        CHECK (unjudged IS NOT 'outcome' AND unjudged IS NOT 'judge' OR outcome IS NOT NULL),
        CHECK ((reason IS NOT NULL) = (unjudged IS 'judge'))
    ) STRICT;";

/// When each experiment ended or was cancelled, kept from this version on: an
/// experiment that ended before has none, and an open one never has one.
const EXPERIMENT_ENDS: &str = "ALTER TABLE experiments
    ADD COLUMN ended_at TEXT CHECK (state <> 'open' OR ended_at IS NULL)";

/// Each case's place in its suite's order, the order of their captures: one
/// more than the highest in its suite at its capture. The cases kept before
/// were inserted in that order, so their row IDs give it.
const CASES_IN_ORDER: &str = "CREATE TABLE cases_in_order (
        name TEXT PRIMARY KEY,
        suite TEXT NOT NULL REFERENCES suites (name),
        run TEXT NOT NULL,
        seq INTEGER NOT NULL CHECK (seq > 0),
        UNIQUE (suite, seq)
    ) STRICT;
    INSERT INTO cases_in_order (name, suite, run, seq) SELECT name, suite, run, rowid FROM cases;
    DROP TABLE cases;
    ALTER TABLE cases_in_order RENAME TO cases;";

/// A handle on the work directory's database. It holds a shared `flock` on
/// the lock file beside the database for as long as it, or a clone of it,
/// lives.
#[derive(Clone, Debug)]
pub struct Database {
    pool: SqlitePool,
    work_dir: Arc<Path>,
    _lock: Lock,
}

/// Why the database could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error(
        "another Linewright process has the database open, and this release must change its schema: stop the other Linewright processes and start again"
    )]
    OthersOpen,
    #[error(
        "the database's schema (version {version}) was made by Linewright {release}, a newer release: to run this release, restore a backup of the work directory from before {release}"
    )]
    Newer { version: u32, release: String },
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// Whether opening makes the database when there is none.
#[derive(Clone, Copy)]
enum Absent {
    Make,
    Leave,
}

/// How a handle holds the lock while it opens the database.
#[derive(Clone, Copy, PartialEq)]
enum Held {
    Alone,
    Shared,
}

/// Where a database's schema stands against this release's. A newer one is
/// refused as it is read.
enum Standing {
    Current,
    Behind(u32),
}

impl Database {
    /// Opens the work directory's database, making it, and the work
    /// directory, when there is none, and brings its schema to this
    /// release's. It warns first when the work directory is on a network
    /// filesystem.
    pub async fn open(work_dir: &Path) -> Result<Database, OpenError> {
        std::fs::create_dir_all(work_dir)
            .with_context(|| format!("creating {}", work_dir.display()))?;
        if let Some(filesystem) = network_filesystem(work_dir) {
            tracing::warn!(
                work_dir = %work_dir.display(),
                filesystem,
                "the work directory is on a network filesystem: keep it on a local filesystem"
            );
        }
        Self::open_in(work_dir, MIGRATIONS, Absent::Make).await
    }

    /// Opens the work directory's database as [`Database::open`] does, or
    /// gives `None`, making nothing, when there is none.
    pub async fn open_existing(work_dir: &Path) -> Result<Option<Database>, OpenError> {
        if !exists(&work_dir.join(DATABASE_FILE))? {
            return Ok(None);
        }
        Self::open_in(work_dir, MIGRATIONS, Absent::Leave)
            .await
            .map(Some)
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The work directory the database is in.
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// Opens the database in `work_dir` with the schema `migrations` make,
    /// changing the schema only while no other handle has the database
    /// open. A database it made and could not open is removed.
    async fn open_in(
        work_dir: &Path,
        migrations: &'static [Migration],
        absent: Absent,
    ) -> Result<Database, OpenError> {
        let lock = Lock::open(work_dir.join(LOCK_FILE))?;
        let held = if lock.try_exclusive()? {
            Held::Alone
        } else {
            lock.shared().await?;
            Held::Shared
        };
        let path = work_dir.join(DATABASE_FILE);
        let made = !exists(&path)?;
        let pool = connect(&path, absent).await?;
        match settle(&pool, &lock, held, migrations).await {
            Ok(()) => Ok(Database {
                pool,
                work_dir: work_dir.into(),
                _lock: lock,
            }),
            Err(e) => {
                if made {
                    pool.close().await;
                    remove_made(&path);
                }
                Err(e)
            }
        }
    }
}

/// The lock file beside the database, open.
#[derive(Clone, Debug)]
struct Lock {
    file: Arc<File>,
    path: Arc<Path>,
}

impl Lock {
    fn open(path: PathBuf) -> anyhow::Result<Lock> {
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        Ok(Lock {
            file: Arc::new(file),
            path: path.into(),
        })
    }

    /// Takes the lock exclusively, or converts a shared lock to it, without
    /// blocking: `false` while another handle holds it.
    fn try_exclusive(&self) -> anyhow::Result<bool> {
        match self.file.try_lock() {
            Ok(()) => Ok(true),
            Err(TryLockError::WouldBlock) => Ok(false),
            Err(TryLockError::Error(e)) => {
                Err(anyhow::Error::new(e).context(format!("locking {}", self.path.display())))
            }
        }
    }

    /// Waits, off the runtime's threads, for the lock shared, or converts an
    /// exclusive lock to it.
    async fn shared(&self) -> anyhow::Result<()> {
        let lock = self.clone();
        tokio::task::spawn_blocking(move || {
            lock.file
                .lock_shared()
                .with_context(|| format!("locking {} shared", lock.path.display()))
        })
        .await
        .context("waiting for the database's lock")?
    }
}

/// Brings the schema to `migrations`', and leaves the lock shared. A handle
/// holding the lock shared changes the schema only once it converts the
/// lock to exclusive. Converting a lock can let another handle take it in
/// between, so the schema is read again once the lock is shared.
/// <https://man7.org/linux/man-pages/man2/flock.2.html>
async fn settle(
    pool: &SqlitePool,
    lock: &Lock,
    held: Held,
    migrations: &'static [Migration],
) -> Result<(), OpenError> {
    match standing(pool, migrations).await? {
        Standing::Current if held == Held::Shared => return Ok(()),
        Standing::Current => {}
        Standing::Behind(_) if held == Held::Shared && !lock.try_exclusive()? => {
            return Err(OpenError::OthersOpen);
        }
        Standing::Behind(version) => migrate(pool, version, migrations).await?,
    }
    lock.shared().await?;
    match standing(pool, migrations).await? {
        Standing::Current => Ok(()),
        Standing::Behind(_) => Err(OpenError::OthersOpen),
    }
}

/// Runs the migrations after `version` in one `BEGIN IMMEDIATE` transaction,
/// which ends by setting the version and recording this release as the one
/// that made it.
async fn migrate(
    pool: &SqlitePool,
    version: u32,
    migrations: &'static [Migration],
) -> anyhow::Result<()> {
    let mut tx = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .context("beginning the database's schema change")?;
    for (index, migration) in migrations.iter().enumerate().skip(version as usize) {
        let Migration::Sql(sql) = migration;
        sqlx::raw_sql(sqlx::AssertSqlSafe(*sql))
            .execute(&mut *tx)
            .await
            .with_context(|| format!("changing the database's schema to version {}", index + 1))?;
    }
    let target = schema_version(migrations);
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "PRAGMA user_version = {target}"
    )))
    .execute(&mut *tx)
    .await
    .context("setting the database's schema version")?;
    sqlx::query("INSERT INTO releases (version, release) VALUES (?, ?)")
        .bind(target)
        .bind(crate::VERSION)
        .execute(&mut *tx)
        .await
        .context("recording the release that made the database's schema")?;
    tx.commit()
        .await
        .context("committing the database's schema change")
}

/// Reads where the schema stands against the one `migrations` make, and
/// refuses a newer one, naming the release that made it.
async fn standing(pool: &SqlitePool, migrations: &[Migration]) -> Result<Standing, OpenError> {
    let version: u32 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(pool)
        .await
        .context("reading the database's schema version")?;
    match version.cmp(&schema_version(migrations)) {
        Ordering::Less => Ok(Standing::Behind(version)),
        Ordering::Equal => Ok(Standing::Current),
        Ordering::Greater => {
            let release = sqlx::query_scalar("SELECT release FROM releases WHERE version = ?")
                .bind(version)
                .fetch_one(pool)
                .await
                .with_context(|| {
                    format!("reading the release that made schema version {version}")
                })?;
            Err(OpenError::Newer { version, release })
        }
    }
}

async fn connect(path: &Path, absent: Absent) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(matches!(absent, Absent::Make))
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(BUSY_TIMEOUT)
        .foreign_keys(true);
    SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .with_context(|| format!("opening {}", path.display()))
}

fn exists(path: &Path) -> anyhow::Result<bool> {
    path.try_exists()
        .with_context(|| format!("looking for {}", path.display()))
}

/// Removes the database at `path` and the files SQLite keeps beside it in
/// WAL mode. <https://www.sqlite.org/wal.html#the_wal_file>
fn remove_made(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        let file = PathBuf::from(file);
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(path = %file.display(), error = %e, "removing a database that failed to open failed");
            }
        }
    }
}

/// The name of the network filesystem `path` is on, or `None` when it is on
/// a local one, or its filesystem cannot be told.
pub fn network_filesystem(path: &Path) -> Option<String> {
    let stat = rustix::fs::statfs(path).ok()?;
    network_filesystem_of(&stat).map(str::to_owned)
}

/// The magic numbers of `statfs(2)`'s `f_type` that name a network
/// filesystem. <https://man7.org/linux/man-pages/man2/statfs.2.html>
#[cfg(target_os = "linux")]
const NETWORK_FILESYSTEMS: &[(u32, &str)] = &[
    (0x6969, "nfs"),
    (0x517B, "smb"),
    (0xFF53_4D42, "cifs"),
    (0xFE53_4D42, "smb2"),
    (0x5346_414F, "afs"),
    (0x6B41_4653, "afs"),
    (0x7375_7245, "coda"),
    (0x564C, "ncp"),
    (0x00C3_6400, "ceph"),
    (0x0102_1997, "9p"),
    (0x7461_636F, "ocfs2"),
    (0x0116_1970, "gfs2"),
    (0x0BD0_0BD0, "lustre"),
    (0x2003_0528, "orangefs"),
];

#[cfg(target_os = "linux")]
fn network_filesystem_of(stat: &rustix::fs::StatFs) -> Option<&'static str> {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::unnecessary_cast,
        reason = "the magic numbers are 32 bits, which `f_type` holds wider or signed on some targets"
    )]
    let magic = stat.f_type as u32;
    NETWORK_FILESYSTEMS
        .iter()
        .find(|(network, _)| *network == magic)
        .map(|(_, name)| *name)
}

/// The names of `statfs(2)`'s `f_fstypename` that are network filesystems.
/// <https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/statfs.2.html>
#[cfg(target_os = "macos")]
const NETWORK_FILESYSTEMS: &[&str] = &["nfs", "smbfs", "afpfs", "webdav", "cifs", "ftp"];

#[cfg(target_os = "macos")]
fn network_filesystem_of(stat: &rustix::fs::StatFs) -> Option<&'static str> {
    let name: Vec<u8> = stat
        .f_fstypename
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| u8::from_ne_bytes(c.to_ne_bytes()))
        .collect();
    NETWORK_FILESYSTEMS
        .iter()
        .find(|network| network.as_bytes() == name)
        .copied()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn network_filesystem_of(_: &rustix::fs::StatFs) -> Option<&'static str> {
    None
}

/// A timestamp as the database keeps it: RFC 3339 in UTC, to the
/// nanosecond, so that the text of two times sorts as the times do.
pub(crate) fn time_text(at: Timestamp) -> String {
    at.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

/// A time the database keeps, as [`time_text`] gives it.
pub(crate) fn parse_time(at: &str) -> anyhow::Result<Timestamp> {
    Ok(DateTime::parse_from_rfc3339(at)
        .with_context(|| format!("{at:?} is not a time"))?
        .to_utc())
}

fn schema_version(migrations: &[Migration]) -> u32 {
    u32::try_from(migrations.len()).expect("fewer than 2^32 migrations")
}

#[cfg(test)]
mod tests;
