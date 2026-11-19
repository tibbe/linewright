//! The environments of trials: one per task head and fingerprint of the
//! setup command, immutable once built, shared across processes and removed
//! once old (`docs/specs/benchmarks.md`, Trials).

use std::fs::File;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::TimeDelta;
use tracing::{info, warn};

use super::kept::{self, Builds};
use super::lifecycle::Fingerprint;
use super::{Build, BuildDir, Builder, Source, Start, build_locked};
use crate::database::Database;
use crate::domain::Timestamp;
use crate::git::CommitId;
use crate::work_settings::SetupSettings;

/// The directory of the environments of trials, in a repository's work
/// directory `work_dir`.
pub fn dir(work_dir: &Path) -> PathBuf {
    work_dir.join(crate::trial::TRIALS).join("environments")
}

/// The file a trial holds a shared lock on, in an environment's directory.
const HELD: &str = "held";
/// What the name of an environment's directory starts with once it is set
/// aside to be removed.
const REMOVED: &str = ".removed-";

/// An environment a trial uses, held while this or a [`Source`] it gave
/// lives.
pub struct InUse {
    source: Source,
    built: Option<BuildLog>,
}

/// What a build of a trial's environment printed, and how long it took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildLog {
    /// Redacted.
    pub output: String,
    pub time: Duration,
}

impl InUse {
    /// What sessions copy the environment from: nothing without a setup
    /// command.
    pub fn source(&self) -> Source {
        self.source.clone()
    }

    /// The output of the build and its time, when this trial built the
    /// environment and its setup command ran.
    pub fn built(&self) -> Option<&BuildLog> {
        self.built.as_ref()
    }
}

/// Why no environment was made ready.
#[derive(Debug, thiserror::Error)]
pub enum Unprepared {
    #[error(transparent)]
    Failed(#[from] BuildFailed),
    /// The environment's directory or locks could not be had.
    #[error("the environment cannot be had: {0:#}")]
    Unavailable(anyhow::Error),
}

/// The build of an environment failed.
#[derive(Debug, thiserror::Error)]
#[error("the build of the environment failed: {why}")]
pub struct BuildFailed {
    /// Why, redacted.
    pub why: String,
    /// The output of the build and its time: `None` when its setup command
    /// never ran.
    pub output: Option<BuildLog>,
}

/// Whether the environment of a trial whose build started at `built`, or
/// that has none, is removed at `now`: once older than `max_age`, and
/// whatever its age without a build.
pub fn outlived(built: Option<Timestamp>, now: Timestamp, max_age: TimeDelta) -> bool {
    built.is_none_or(|at| now - at > max_age)
}

/// Whether `name` is the name of an environment's directory set aside to be
/// removed.
fn is_set_aside(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| name.starts_with(REMOVED))
}

/// Makes the environment of `head` with `setup` ready for a trial, once
/// every environment under `base` that is [`outlived`] at `now` and that no
/// trial holds is removed. It uses the environment of `head` and `setup`
/// there is, waits for the build of it another process has in progress, or
/// builds it. Without a setup command there is no environment.
///
/// # Errors
///
/// [`Unprepared::Failed`] when the build fails, which leaves no
/// environment, and [`Unprepared::Unavailable`] when the environment's
/// directory or locks cannot be had.
pub async fn prepare(
    base: &Path,
    head: &CommitId,
    builder: Builder,
    setup: Option<SetupSettings>,
    now: Timestamp,
    max_age: TimeDelta,
) -> Result<InUse, Unprepared> {
    sweep(base, &builder.database, now, max_age).await;
    let Some(settings) = setup else {
        return Ok(InUse {
            source: Source::default(),
            built: None,
        });
    };
    let fingerprint = builder.fingerprint(&settings);
    let dir = base.join(name(head, &fingerprint));
    let held = hold(dir.clone()).await.map_err(Unprepared::Unavailable)?;
    let builds = Builds::new(dir);
    let lock = builds.lock().await.map_err(Unprepared::Unavailable)?;
    let named = builds
        .named(&builder.database)
        .await
        .map_err(Unprepared::Unavailable)?;
    let (id, built) = match named {
        Some(built) => (built.id, None),
        None => {
            let id = builds
                .next_id(&builder.database, &lock)
                .await
                .map_err(Unprepared::Unavailable)?;
            info!(%head, "the environment of a replay is building");
            let start = Start::Commit(head);
            let started = Instant::now();
            let built =
                build_locked(&builder, &builds, &lock, id, start, &settings, fingerprint).await;
            let time = started.elapsed();
            let output = builds.output(id).map(|output| BuildLog { output, time });
            match built {
                Ok(built) => (built.id, output),
                Err(why) => return Err(BuildFailed { why, output }.into()),
            }
        }
    };
    drop(lock);
    let build = BuildDir::held(Build::new(&builds, id, &builder.winding_up), held);
    Ok(InUse {
        source: Source::of(build),
        built,
    })
}

/// The name of the directory of the environment of `head` and
/// `fingerprint`.
fn name(head: &CommitId, fingerprint: &Fingerprint) -> String {
    format!("{head}-{}", fingerprint.as_str())
}

/// A shared lock on an environment's directory, which keeps it from being
/// removed.
#[derive(Debug)]
pub(super) struct Held(#[expect(dead_code, reason = "held for its lock")] File);

/// Holds the environment's directory `dir`, making it if missing. A
/// directory set aside before the lock is taken is made again.
async fn hold(dir: PathBuf) -> Result<Held> {
    tokio::task::spawn_blocking(move || {
        loop {
            std::fs::create_dir_all(&dir).context("creating the environment's directory")?;
            let path = dir.join(HELD);
            let file = match File::options()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
            {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e).context("opening the environment's hold"),
            };
            file.lock_shared().context("holding the environment")?;
            if is_at(&file, &path)? {
                return Ok(Held(file));
            }
        }
    })
    .await
    .context("holding the environment")?
}

/// Whether `file` is the file at `path`, and not one moved or removed from
/// there.
fn is_at(file: &File, path: &Path) -> Result<bool> {
    let there = match std::fs::metadata(path) {
        Ok(there) => there,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).context("looking at the environment's hold"),
    };
    let held = file
        .metadata()
        .context("looking at the environment's hold")?;
    Ok((held.dev(), held.ino()) == (there.dev(), there.ino()))
}

/// Removes each environment under `base` that is [`outlived`] at `now` and
/// that no trial holds, with its row of `db`, and what earlier removals
/// left.
async fn sweep(base: &Path, db: &Database, now: Timestamp, max_age: TimeDelta) {
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            warn!(error = %e, "listing the environments of replays failed");
            return;
        }
    };
    for entry in entries {
        let dir = match entry {
            Ok(entry) => entry.path(),
            Err(e) => {
                warn!(error = %e, "listing the environments of replays failed");
                continue;
            }
        };
        let set_aside = dir.file_name().is_some_and(is_set_aside);
        let removed = if set_aside {
            remove_off_runtime(dir.clone()).await
        } else {
            remove_if_outlived(base, &dir, db, now, max_age).await
        };
        if let Err(e) = removed {
            warn!(
                error = format!("{e:#}"),
                dir = %dir.display(),
                "removing an environment of a replay failed"
            );
        }
    }
}

/// Removes the environment `dir` under `base`, and its row of `db`, if it
/// is [`outlived`] at `now` and no trial holds it. It is set aside first,
/// so that a trial holding it meanwhile makes it anew.
async fn remove_if_outlived(
    base: &Path,
    dir: &Path,
    db: &Database,
    now: Timestamp,
    max_age: TimeDelta,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let path = dir.join(HELD);
    let _held = match File::open(&path) {
        Ok(file) => match file.try_lock() {
            Ok(()) if is_at(&file, &path)? => Some(file),
            Ok(()) | Err(std::fs::TryLockError::WouldBlock) => return Ok(()),
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e).context("locking the environment's hold");
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).context("opening the environment's hold"),
    };
    let builds = Builds::new(dir.to_path_buf());
    let built = builds.named(db).await?.map(|built| built.at);
    if !outlived(built, now, max_age) {
        return Ok(());
    }
    info!(dir = %dir.display(), "an environment of a replay is removed");
    builds.forget(db).await?;
    let aside = base.join(format!("{REMOVED}{}", uuid::Uuid::new_v4()));
    std::fs::rename(dir, &aside).context("setting the environment aside")?;
    remove_off_runtime(aside).await
}

/// Removes `dir`, as [`kept::remove`] does, on a thread where blocking is
/// allowed.
async fn remove_off_runtime(dir: PathBuf) -> Result<()> {
    tokio::task::spawn_blocking(move || kept::remove(&dir))
        .await
        .context("removing an environment of a replay")?
}
