//! Linewright's own records: one SQLite database in the work directory, which
//! every Linewright process on the host may have open at once.

use std::cmp::Ordering;
use std::fs::{File, TryLockError};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Sqlite, SqlitePool, Transaction};

/// The database's file, in the work directory.
const DATABASE_FILE: &str = "linewright.db";

/// The file beside the database that every handle holds a shared `flock`
/// on, and that a handle changing the schema holds exclusively.
const LOCK_FILE: &str = "linewright.db.lock";

/// How long a statement waits for another connection's write lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// A change of the schema, from the version before it to its own.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "no module keeps its records in the database")
)]
pub(crate) enum Migration {
    Sql(&'static str),
    /// Rust code, given the schema change's transaction and the work
    /// directory.
    Steps(for<'a> fn(&'a mut Transaction<'static, Sqlite>, &'a Path) -> StepsFuture<'a>),
}

pub(crate) type StepsFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// Every change of the schema, in order: the schema's version is how many of
/// them a database has had. A released migration never changes; a later
/// change is a new entry.
static MIGRATIONS: &[Migration] = &[RELEASES_TABLE];

/// The table of the release that made each schema version.
const RELEASES_TABLE: Migration = Migration::Sql(
    "CREATE TABLE releases (
        version INTEGER PRIMARY KEY,
        release TEXT NOT NULL
    ) STRICT",
);

/// A handle on the work directory's database. It holds a shared `flock` on
/// the lock file beside the database for as long as it, or a clone of it,
/// lives.
#[derive(Clone)]
pub struct Database {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "no module keeps its records in the database")
    )]
    pool: SqlitePool,
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
    /// Opens the work directory's database, making it when there is none,
    /// and brings its schema to this release's.
    pub async fn open(work_dir: &Path) -> Result<Database, OpenError> {
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

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "no module keeps its records in the database")
    )]
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
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
        match settle(&pool, &lock, held, work_dir, migrations).await {
            Ok(()) => Ok(Database { pool, _lock: lock }),
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
#[derive(Clone)]
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
    work_dir: &Path,
    migrations: &'static [Migration],
) -> Result<(), OpenError> {
    match standing(pool, migrations).await? {
        Standing::Current if held == Held::Shared => return Ok(()),
        Standing::Current => {}
        Standing::Behind(_) if held == Held::Shared && !lock.try_exclusive()? => {
            return Err(OpenError::OthersOpen);
        }
        Standing::Behind(version) => migrate(pool, work_dir, version, migrations).await?,
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
    work_dir: &Path,
    version: u32,
    migrations: &'static [Migration],
) -> anyhow::Result<()> {
    let mut tx = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .context("beginning the database's schema change")?;
    for (index, migration) in migrations.iter().enumerate().skip(version as usize) {
        match migration {
            Migration::Sql(sql) => sqlx::raw_sql(sqlx::AssertSqlSafe(*sql))
                .execute(&mut *tx)
                .await
                .map(drop)
                .map_err(anyhow::Error::new),
            Migration::Steps(steps) => steps(&mut tx, work_dir).await,
        }
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

fn schema_version(migrations: &[Migration]) -> u32 {
    u32::try_from(migrations.len()).expect("fewer than 2^32 migrations")
}

#[cfg(test)]
mod tests;
