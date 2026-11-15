//! The environments of replays: one per case head and fingerprint of the
//! setup command, immutable once built, shared across processes and removed
//! once old (`docs/specs/benchmarks.md`, Replaying).

use std::fs::File;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::TimeDelta;
use tracing::{info, warn};

use super::kept::{self, Builds};
use super::lifecycle::Fingerprint;
use super::{Build, BuildDir, Builder, Source, Start, build_locked};
use crate::domain::Timestamp;
use crate::git::CommitId;
use crate::work_settings::SetupSettings;

/// The directory of the environments of replays, in a repository's work
/// directory `work_dir`.
pub fn dir(work_dir: &Path) -> PathBuf {
    work_dir.join("replays").join("environments")
}

/// The file a replay holds a shared lock on, in an environment's directory.
const HELD: &str = "held";
/// What the name of an environment's directory starts with once it is set
/// aside to be removed.
const REMOVED: &str = ".removed-";

/// An environment a replay uses, held while this or a [`Source`] it gave
/// lives.
pub struct InUse {
    source: Source,
    built_output: Option<String>,
}

impl InUse {
    /// What sessions copy the environment from: nothing without a setup
    /// command.
    pub fn source(&self) -> Source {
        self.source.clone()
    }

    /// The output of the build, redacted, when this replay built the
    /// environment and its setup command ran.
    pub fn built_output(&self) -> Option<&str> {
        self.built_output.as_deref()
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
    /// The output of the build, redacted: `None` when its setup command
    /// never ran.
    pub output: Option<String>,
}

/// Whether the environment of a replay whose build started at `built`, or
/// that has none, is removed at `now`: once older than `max_age`, and
/// whatever its age without a build.
pub fn outlived(built: Option<Timestamp>, now: Timestamp, max_age: TimeDelta) -> bool {
    built.is_none_or(|at| now - at > max_age)
}

/// Makes the environment of `head` with `setup` ready for a replay, once
/// every environment under `base` that is [`outlived`] at `now` and that no
/// replay holds is removed. It uses the environment of `head` and `setup`
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
    let swept = base.to_path_buf();
    if let Err(e) = tokio::task::spawn_blocking(move || sweep(&swept, now, max_age)).await {
        warn!(error = %e, "removing the old environments of replays failed");
    }
    let Some(settings) = setup else {
        return Ok(InUse {
            source: Source::default(),
            built_output: None,
        });
    };
    let fingerprint = builder.fingerprint(&settings);
    let dir = base.join(name(head, &fingerprint));
    let held = hold(dir.clone()).await.map_err(Unprepared::Unavailable)?;
    let builds = Builds::new(dir);
    let lock = builds.lock().await.map_err(Unprepared::Unavailable)?;
    let (id, built_output) = match builds.named() {
        Some(built) => (built.id, None),
        None => {
            let id = builds.next_id(&lock).map_err(Unprepared::Unavailable)?;
            info!(%head, "the environment of a replay is building");
            let start = Start::Commit(head);
            let built =
                build_locked(&builder, &builds, &lock, id, start, &settings, fingerprint).await;
            let output = builds.output(id);
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
        built_output,
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
/// that no replay holds, and what earlier removals left.
fn sweep(base: &Path, now: Timestamp, max_age: TimeDelta) {
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
        let set_aside = dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(REMOVED));
        let removed = if set_aside {
            kept::remove(&dir)
        } else {
            remove_if_outlived(base, &dir, now, max_age)
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

/// Removes the environment `dir` under `base` if it is [`outlived`] at
/// `now` and no replay holds it. It is set aside first, so that a replay
/// holding it meanwhile makes it anew.
fn remove_if_outlived(base: &Path, dir: &Path, now: Timestamp, max_age: TimeDelta) -> Result<()> {
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
    let built = Builds::new(dir.to_path_buf()).named().map(|built| built.at);
    if !outlived(built, now, max_age) {
        return Ok(());
    }
    info!(dir = %dir.display(), "an environment of a replay is removed");
    let aside = base.join(format!("{REMOVED}{}", uuid::Uuid::new_v4()));
    std::fs::rename(dir, &aside).context("setting the environment aside")?;
    kept::remove(&aside)
}
