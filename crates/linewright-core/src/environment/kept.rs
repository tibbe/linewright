//! What is kept of the environment across restarts: a directory per build,
//! with its record and output, and the environment's row of the database,
//! naming the build that is the environment (`docs/specs/sessions.md`,
//! Building, Kept).

use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::Row as _;

use super::lifecycle::{Basis, Fingerprint};
use crate::database::{Database, parse_time, time_text};
use crate::domain::Timestamp;
use crate::transcript::lines::BuildMaintenanceLine;
use crate::tree;

/// A build's clone, in its directory.
pub const REPO: &str = "repo";
/// A build's temporary directory, in its directory.
pub const TMP: &str = "tmp";
/// A build's record, in its directory.
const RECORD: &str = "build.json";
/// A build's output, redacted, in its directory.
pub const OUTPUT: &str = "output.log";
/// The file a copy from a build holds a shared lock on, in its directory.
const LEASE: &str = "lease";

/// The builds' directories, in the environment's directory.
const BUILDS: &str = "builds";
/// The file the builds' lock is held on, in the environment's directory.
const LOCK: &str = "lock";
/// The builds of one repository's environment, under its directory.
#[derive(Clone, Debug)]
pub struct Builds {
    dir: PathBuf,
}

/// A build's number, counting up.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct BuildId(pub(super) u64);

impl std::fmt::Display for BuildId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for BuildId {
    type Err = std::num::ParseIntError;

    fn from_str(text: &str) -> Result<BuildId, std::num::ParseIntError> {
        text.parse().map(BuildId)
    }
}

/// The build that is the environment, as its row names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Built {
    pub id: BuildId,
    /// The commit of the default branch it was built from.
    pub commit: String,
    /// When its build started.
    pub at: Timestamp,
    pub fingerprint: Fingerprint,
}

impl Built {
    pub fn basis(&self) -> Basis {
        Basis {
            fingerprint: self.fingerprint.clone(),
            since: self.at,
        }
    }
}

/// A build's record: as a transcript's maintenance command is in
/// `logs --json`, with the commit it built from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BuildRecord {
    #[serde(flatten)]
    pub line: BuildMaintenanceLine,
    /// `None` until the default branch is cloned.
    pub commit: Option<String>,
    /// When it started.
    pub at: Timestamp,
    /// Why it failed before its command ended, redacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl BuildRecord {
    /// The record of a build of `command`, redacted, started `at`.
    pub fn started(command: String, at: Timestamp) -> BuildRecord {
        BuildRecord {
            line: BuildMaintenanceLine {
                command,
                ended: None,
            },
            commit: None,
            at,
            error: None,
        }
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self).expect("a build's record serializes");
        std::fs::write(dir.join(RECORD), text).context("writing the build's record")
    }

    pub fn read(dir: &Path) -> Option<BuildRecord> {
        let text = std::fs::read_to_string(dir.join(RECORD)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Whether its command ended, or an error ended it before.
    pub fn has_ended(&self) -> bool {
        self.error.is_some() || self.line.ended.is_some()
    }

    pub fn failed(&self) -> bool {
        self.error.is_some()
            || self
                .line
                .ended
                .as_ref()
                .is_some_and(|ended| ended.status.failed())
    }
}

/// The exclusive lock on the builds, held through a build and the naming of
/// the build that is the environment, and let go when dropped.
pub struct Lock(#[expect(dead_code, reason = "held for its lock")] File);

impl Builds {
    pub fn new(dir: PathBuf) -> Builds {
        Builds { dir }
    }

    pub fn build_dir(&self, id: BuildId) -> PathBuf {
        self.dir.join(BUILDS).join(id.0.to_string())
    }

    /// Takes the lock once no other build, in any process, holds it.
    pub async fn lock(&self) -> Result<Lock> {
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir).context("creating the environment's directory")?;
            let file = File::options()
                .create(true)
                .truncate(false)
                .write(true)
                .open(dir.join(LOCK))
                .context("opening the environment's lock")?;
            file.lock().context("locking the environment")?;
            Ok(Lock(file))
        })
        .await
        .context("locking the environment")?
    }

    /// Whether a build holds the lock, in any process.
    pub fn is_building(&self) -> Result<bool> {
        let file = match File::open(self.dir.join(LOCK)) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e).context("opening the environment's lock"),
        };
        match file.try_lock_shared() {
            Ok(()) => Ok(false),
            Err(std::fs::TryLockError::WouldBlock) => Ok(true),
            Err(std::fs::TryLockError::Error(e)) => {
                Err(e).context("looking at the environment's lock")
            }
        }
    }

    /// Whether the latest poll gave a setup command, `None` before the
    /// first.
    pub async fn setup_given(&self, db: &Database) -> Result<Option<bool>> {
        let given: Option<Option<bool>> =
            sqlx::query_scalar("SELECT setup_command FROM environments WHERE dir = ?")
                .bind(self.key(db)?)
                .fetch_optional(db.pool())
                .await
                .context("reading whether there is a setup command")?;
        Ok(given.flatten())
    }

    /// Keeps whether the latest poll gave a setup command.
    pub async fn keep_setup_given(&self, db: &Database, setup_command: bool) -> Result<()> {
        keep_setup_given(db.pool(), &self.key(db)?, setup_command)
            .await
            .context("keeping whether there is a setup command")
    }

    /// The environment's key in the database: its directory under the work
    /// directory.
    fn key(&self, db: &Database) -> Result<String> {
        key(db.work_dir(), &self.dir)
    }

    /// Forgets the build that is the environment, and whether there is a
    /// setup command.
    pub async fn forget(&self, db: &Database) -> Result<()> {
        sqlx::query("DELETE FROM environments WHERE dir = ?")
            .bind(self.key(db)?)
            .execute(db.pool())
            .await
            .context("forgetting the environment")?;
        Ok(())
    }

    /// The record of each build kept, by number.
    pub fn records(&self) -> Result<Vec<(BuildId, BuildRecord)>> {
        Ok(self
            .listed()?
            .into_iter()
            .filter_map(|id| Some((id, BuildRecord::read(&self.build_dir(id))?)))
            .collect())
    }

    /// The builds there are, by number.
    fn listed(&self) -> Result<Vec<BuildId>> {
        let dir = self.dir.join(BUILDS);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e).context("listing the environment's builds"),
        };
        let mut ids: Vec<BuildId> = entries
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
            .map(BuildId)
            .collect();
        ids.sort_unstable();
        Ok(ids)
    }

    /// The number of the next build, taken under the lock: after every
    /// build there is, and after the build named as the environment, even
    /// once its directory is gone.
    pub async fn next_id(&self, db: &Database, _: &Lock) -> Result<BuildId> {
        let named: Option<Option<i64>> =
            sqlx::query_scalar("SELECT build FROM environments WHERE dir = ?")
                .bind(self.key(db)?)
                .fetch_optional(db.pool())
                .await
                .context("reading the environment's state")?;
        let named = named.flatten().map_or(Ok(0), u64::try_from)?;
        let last = self.listed()?.last().map_or(0, |id| id.0);
        Ok(BuildId(last.max(named) + 1))
    }

    /// The build named as the environment, if its clone is there.
    pub async fn named(&self, db: &Database) -> Result<Option<Built>> {
        let row = sqlx::query(
            "SELECT build, built_from, built_at, fingerprint FROM environments
                WHERE dir = ? AND build IS NOT NULL",
        )
        .bind(self.key(db)?)
        .fetch_optional(db.pool())
        .await
        .context("reading the environment's state")?;
        let Some(row) = row else {
            return Ok(None);
        };
        let built = Built {
            id: BuildId(u64::try_from(row.try_get::<i64, _>("build")?)?),
            commit: row.try_get("built_from")?,
            at: parse_time(row.try_get("built_at")?)?,
            fingerprint: Fingerprint::kept(row.try_get("fingerprint")?),
        };
        Ok(self
            .build_dir(built.id)
            .join(REPO)
            .is_dir()
            .then_some(built))
    }

    /// Under the lock, the build named as the environment, once every other
    /// build is removed but the record of the latest that failed.
    pub async fn open(&self, db: &Database) -> Result<Option<Built>> {
        let _lock = self.lock().await?;
        let built = self.named(db).await?;
        let named = built.as_ref().map(|built| built.id);
        let latest_failed = self.latest_failed()?;
        for id in self.listed()? {
            let dir = self.build_dir(id);
            if Some(id) == named {
                remove(&dir.join(TMP))?;
            } else if Some(id) == latest_failed {
                remove(&dir.join(REPO))?;
                remove(&dir.join(TMP))?;
            } else {
                remove_unleased(&dir)?;
            }
        }
        Ok(built)
    }

    fn latest_failed(&self) -> Result<Option<BuildId>> {
        Ok(self.listed()?.into_iter().rev().find(|&id| {
            BuildRecord::read(&self.build_dir(id)).is_some_and(|record| record.failed())
        }))
    }

    /// Removes every build that failed but the latest.
    pub fn keep_only_the_latest_failed(&self) -> Result<()> {
        let latest = self.latest_failed()?;
        for id in self.listed()? {
            let dir = self.build_dir(id);
            let failed = BuildRecord::read(&dir).is_some_and(|record| record.failed());
            if failed && Some(id) != latest {
                remove(&dir)?;
            }
        }
        Ok(())
    }

    /// The output of the build `id`, redacted: `None` when its setup
    /// command never ran.
    pub fn output(&self, id: BuildId) -> Option<String> {
        std::fs::read_to_string(self.build_dir(id).join(OUTPUT)).ok()
    }

    /// Names `built` as the environment.
    pub async fn write_state(&self, db: &Database, built: &Built) -> Result<()> {
        write_state(db.pool(), &self.key(db)?, built)
            .await
            .context("naming the environment's build")
    }
}

/// Creates the directory of a build, `dir`.
pub async fn create(dir: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dir)
        .await
        .context("creating the build's directory")?;
    tokio::fs::write(dir.join(LEASE), "")
        .await
        .context("creating the build's lease")
}

/// A shared lock on the build `dir`, which [`remove_unleased`] waits for no
/// copy to hold.
pub struct Leased(#[expect(dead_code, reason = "held for its lock")] File);

/// Holds the build `dir` until the lease is dropped.
pub async fn lease(dir: &Path) -> Result<Leased> {
    let path = dir.join(LEASE);
    tokio::task::spawn_blocking(move || {
        let file = File::open(&path).context("opening the build's lease")?;
        file.lock_shared().context("leasing the build")?;
        Ok(Leased(file))
    })
    .await
    .context("leasing the build")?
}

/// Removes the build `dir`, unless a copy from it, in any process, holds
/// it.
pub fn remove_unleased(dir: &Path) -> Result<()> {
    let _held = match File::open(dir.join(LEASE)) {
        Ok(file) => match file.try_lock() {
            Ok(()) => Some(file),
            Err(std::fs::TryLockError::WouldBlock) => return Ok(()),
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e).context("locking the build's lease");
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).context("opening the build's lease"),
    };
    remove(dir)
}

/// Removes `path`, a directory, if it is there, within the bound of
/// [`tree::remove`].
pub fn remove(path: &Path) -> Result<()> {
    tree::remove(path)
}

/// The key in the database of the environment in `dir`: its path under
/// `work_dir`.
fn key(work_dir: &Path, dir: &Path) -> Result<String> {
    let under = dir.strip_prefix(work_dir).with_context(|| {
        format!(
            "the environment {} is outside the work directory {}",
            dir.display(),
            work_dir.display()
        )
    })?;
    under
        .to_str()
        .map(str::to_owned)
        .with_context(|| format!("the environment {} is not named in UTF-8", dir.display()))
}

async fn write_state<'c>(
    conn: impl sqlx::SqliteExecutor<'c>,
    key: &str,
    built: &Built,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO environments (dir, build, built_from, built_at, fingerprint)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT (dir) DO UPDATE SET build = excluded.build,
                built_from = excluded.built_from, built_at = excluded.built_at,
                fingerprint = excluded.fingerprint",
    )
    .bind(key)
    .bind(
        i64::try_from(built.id.0)
            .context("the build's number is beyond what the database keeps")?,
    )
    .bind(&built.commit)
    .bind(time_text(built.at))
    .bind(built.fingerprint.as_str())
    .execute(conn)
    .await?;
    Ok(())
}

async fn keep_setup_given<'c>(
    conn: impl sqlx::SqliteExecutor<'c>,
    key: &str,
    setup_command: bool,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO environments (dir, setup_command) VALUES (?, ?)
            ON CONFLICT (dir) DO UPDATE SET setup_command = excluded.setup_command",
    )
    .bind(key)
    .bind(setup_command)
    .execute(conn)
    .await?;
    Ok(())
}
