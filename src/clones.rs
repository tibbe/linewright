//! The clones runs work in, each a directory under the work directory's
//! `clones` (`docs/specs/runs.md`, "A clone's end" and "Clones left
//! behind").
//!
//! A clone `clones/<name>/` is in use while a lock on `clones/<name>.lock`
//! is held. The lock file is made and locked before the directory is, and
//! removed after it is.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{File, TryLockError};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::tree::{self, unless_missing};
use crate::winding_up::{Remaining, WindingUp};

const LOCK_SUFFIX: &str = ".lock";

/// What a test runs at a point in making or sweeping clones.
#[cfg(test)]
type Hook = Arc<dyn Fn(&Path) + Send + Sync>;

/// Where clones are made, and who removes them.
#[derive(Clone)]
pub struct Clones {
    dir: PathBuf,
    winding_up: WindingUp,
    /// Runs on each lock file [`Clones::make`] makes, before it locks it.
    #[cfg(test)]
    on_lock_created: Option<Hook>,
    /// Runs on `clones` once [`Clones::sweep`] has listed it.
    #[cfg(test)]
    on_listed: Option<Hook>,
}

impl Clones {
    /// The clones in `work_dir`, removed through `winding_up`.
    pub fn new(work_dir: &Path, winding_up: WindingUp) -> Clones {
        Clones {
            dir: work_dir.join("clones"),
            winding_up,
            #[cfg(test)]
            on_lock_created: None,
            #[cfg(test)]
            on_listed: None,
        }
    }

    #[cfg(test)]
    fn on_lock_created(self, f: impl Fn(&Path) + Send + Sync + 'static) -> Clones {
        Clones {
            on_lock_created: Some(Arc::new(f)),
            ..self
        }
    }

    #[cfg(test)]
    fn on_listed(self, f: impl Fn(&Path) + Send + Sync + 'static) -> Clones {
        Clones {
            on_listed: Some(Arc::new(f)),
            ..self
        }
    }

    /// A new, empty directory for a clone, named `prefix` and a random
    /// suffix, locked so that no [`Clones::sweep`] removes it.
    pub async fn make(&self, prefix: &str) -> Result<CloneDir> {
        let clones = self.clone();
        let prefix = format!("{prefix}-");
        let (path, lock) = tokio::task::spawn_blocking(move || clones.make_locked(&prefix))
            .await
            .context("creating a clone directory")??;
        Ok(CloneDir {
            path,
            lock: Some(lock),
            before_removal: None,
            winding_up: self.winding_up.clone(),
        })
    }

    fn make_locked(&self, prefix: &str) -> Result<(PathBuf, Lock)> {
        std::fs::create_dir_all(&self.dir).context("creating the clones directory")?;
        loop {
            let (file, lock_path) = tempfile::Builder::new()
                .prefix(prefix)
                .suffix(LOCK_SUFFIX)
                .tempfile_in(&self.dir)
                .context("creating a clone's lock file")?
                .keep()
                .context("keeping a clone's lock file")?;
            #[cfg(test)]
            if let Some(f) = &self.on_lock_created {
                f(&lock_path);
            }
            let Some(lock) = Lock::take(&lock_path, file)? else {
                continue;
            };
            let path = clone_path(&lock_path);
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok((path, lock)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    remove_file(&lock_path)?;
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("creating {}", path.display()));
                }
            }
        }
    }

    /// Removes, in the background as [`Remaining::LeftClones`], every clone
    /// no running Linewright is using: each whose lock it can take, its lock
    /// file made where it has none.
    pub fn sweep(&self) {
        let clones = self.clone();
        self.winding_up
            .spawn_blocking(Remaining::LeftClones, move || {
                if let Err(e) = clones.remove_left_behind() {
                    warn!(error = format!("{e:#}"), "removing the clones left behind");
                }
            });
    }

    fn remove_left_behind(&self) -> Result<()> {
        let Some(entries) = unless_missing(std::fs::read_dir(&self.dir))
            .with_context(|| format!("listing {}", self.dir.display()))?
        else {
            return Ok(());
        };
        let mut names = BTreeSet::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("listing {}", self.dir.display()))?;
            match entry.file_type() {
                Ok(file_type) => names.extend(clone_name(&entry.file_name(), file_type.is_dir())),
                Err(e) => warn!(
                    error = %e,
                    path = %entry.path().display(),
                    "reading an entry of the clones directory"
                ),
            }
        }
        #[cfg(test)]
        if let Some(f) = &self.on_listed {
            f(&self.dir);
        }
        let mut left = Vec::new();
        for name in names {
            let path = self.dir.join(&name);
            match left_behind(&path) {
                Ok(Some(lock)) => left.push((name, path, lock)),
                Ok(None) => {}
                Err(e) => warn!(error = format!("{e:#}"), "locking a clone"),
            }
        }
        if left.is_empty() {
            return Ok(());
        }
        let began = Instant::now();
        info!(
            clones = ?left.iter().map(|(name, ..)| name.to_string_lossy()).collect::<Vec<_>>(),
            "removing the clones left behind"
        );
        for (_, path, _lock) in left {
            if let Err(e) = remove_clone(&path) {
                warn!(error = format!("{e:#}"), "removing a clone left behind");
            }
        }
        info!(took = ?began.elapsed(), "removed the clones left behind");
        Ok(())
    }
}

/// The name of the clone an entry of `clones` named `file_name` belongs to:
/// a directory is a clone, and a file `<name>.lock` its lock file.
fn clone_name(file_name: &OsStr, is_dir: bool) -> Option<OsString> {
    if is_dir {
        return Some(file_name.to_owned());
    }
    let name = file_name.to_str()?.strip_suffix(LOCK_SUFFIX)?;
    Some(name.into())
}

/// The lock of the clone at `clone`, made where it has none, unless a
/// running Linewright holds it.
fn left_behind(clone: &Path) -> Result<Option<Lock>> {
    let lock_path = lock_path(clone);
    let file = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path.display()))?;
    Lock::take(&lock_path, file)
}

/// An exclusive lock on a clone's lock file, held while its path still
/// names it.
struct Lock(#[expect(dead_code, reason = "held for its lock")] File);

impl Lock {
    /// Locks `file`, opened at `path`: `None` where another holds its lock
    /// or `path` no longer names it.
    fn take(path: &Path, file: File) -> Result<Option<Lock>> {
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("locking {}", path.display()));
            }
        }
        let held = file
            .metadata()
            .with_context(|| format!("reading {}", path.display()))?;
        let named = unless_missing(std::fs::metadata(path))
            .with_context(|| format!("reading {}", path.display()))?;
        Ok(named
            .is_some_and(|named| named.dev() == held.dev() && named.ino() == held.ino())
            .then_some(Lock(file)))
    }
}

/// The clone whose lock file, made by [`Clones::make`], is `lock`.
fn clone_path(lock: &Path) -> PathBuf {
    let name = lock
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(LOCK_SUFFIX))
        .expect("named from a prefix, a random suffix and the lock suffix");
    lock.with_file_name(name)
}

/// The lock file of the clone at `clone`.
fn lock_path(clone: &Path) -> PathBuf {
    let mut path = clone.as_os_str().to_owned();
    path.push(LOCK_SUFFIX);
    PathBuf::from(path)
}

/// Removes the clone at `clone`, its directory and then its lock file, as
/// far as they are there, its directory within the bound of
/// [`tree::remove`].
fn remove_clone(clone: &Path) -> Result<()> {
    tree::remove(clone)?;
    remove_file(&lock_path(clone))
}

/// Removes `path`, a file, if it is there.
fn remove_file(path: &Path) -> Result<()> {
    unless_missing(std::fs::remove_file(path))
        .with_context(|| format!("removing {}", path.display()))?;
    Ok(())
}

/// A clone's directory, removed through [`WindingUp`] once dropped, however
/// far its making got, and then its lock file, while holding its lock.
pub struct CloneDir {
    path: PathBuf,
    /// `None` once dropped.
    lock: Option<Lock>,
    /// What runs, off the runtime's threads, before the directory is
    /// removed.
    before_removal: Option<Box<dyn FnOnce() + Send>>,
    winding_up: WindingUp,
}

impl CloneDir {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The directory's name in `clones`.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .expect("made in `clones`")
            .to_string_lossy()
            .into_owned()
    }

    /// Has `before_removal` run, off the runtime's threads, before the
    /// directory is removed, after any given earlier.
    pub fn then_remove(&mut self, before_removal: impl FnOnce() + Send + 'static) {
        let earlier = self.before_removal.take();
        self.before_removal = Some(Box::new(move || {
            if let Some(earlier) = earlier {
                earlier();
            }
            before_removal();
        }));
    }
}

impl Drop for CloneDir {
    fn drop(&mut self) {
        let name = self.name();
        let path = self.path.clone();
        let lock = self.lock.take();
        let before_removal = self.before_removal.take();
        self.winding_up
            .spawn_blocking(Remaining::Clone { name }, move || {
                if let Some(before_removal) = before_removal {
                    before_removal();
                }
                if let Err(e) = remove_clone(&path) {
                    warn!(error = format!("{e:#}"), "removing a clone");
                }
                drop(lock);
            });
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::sync::{Arc, Mutex};

    use super::*;

    fn clones(work_dir: &Path) -> (Clones, WindingUp) {
        let winding_up = WindingUp::default();
        (Clones::new(work_dir, winding_up.clone()), winding_up)
    }

    #[test_log::test(tokio::test)]
    async fn a_sweep_leaves_a_clone_locked_by_another_handle() {
        let work_dir = tempfile::tempdir().unwrap();
        let (owner, _) = clones(work_dir.path());
        let clone = owner.make("issue-12").await.unwrap();
        let (sweeper, sweeping) = clones(work_dir.path());

        sweeper.sweep();
        sweeping.wait().await;

        assert!(clone.path().is_dir());
        assert!(
            work_dir
                .path()
                .join("clones")
                .join(format!("{}.lock", clone.name()))
                .is_file()
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_sweep_removes_a_clone_whose_lock_is_free() {
        let work_dir = tempfile::tempdir().unwrap();
        let dir = work_dir.path().join("clones");
        std::fs::create_dir_all(dir.join("issue-12-AbC123")).unwrap();
        File::create(dir.join("issue-12-AbC123.lock")).unwrap();
        let (sweeper, sweeping) = clones(work_dir.path());

        sweeper.sweep();
        sweeping.wait().await;

        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test_log::test(tokio::test)]
    async fn a_sweep_removes_a_clone_without_a_lock_file() {
        let work_dir = tempfile::tempdir().unwrap();
        let dir = work_dir.path().join("clones");
        std::fs::create_dir_all(dir.join("issue-12-AbC123").join("repo")).unwrap();
        let (sweeper, sweeping) = clones(work_dir.path());

        sweeper.sweep();
        sweeping.wait().await;

        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test_log::test(tokio::test)]
    async fn a_sweep_leaves_a_clone_whose_lock_file_its_listing_missed() {
        let work_dir = tempfile::tempdir().unwrap();
        let dir = work_dir.path().join("clones");
        std::fs::create_dir_all(dir.join("issue-12-AbC123")).unwrap();
        let held = Arc::new(Mutex::new(None));
        let (sweeper, sweeping) = clones(work_dir.path());
        let sweeper = sweeper.on_listed({
            let held = held.clone();
            move |listed: &Path| {
                let lock = File::create(listed.join("issue-12-AbC123.lock")).unwrap();
                lock.try_lock().unwrap();
                *held.lock().unwrap() = Some(lock);
            }
        });

        sweeper.sweep();
        sweeping.wait().await;

        assert!(held.lock().unwrap().is_some());
        assert!(dir.join("issue-12-AbC123").is_dir());
    }

    #[test_log::test(tokio::test)]
    async fn a_sweep_removes_a_free_lock_file_without_a_clone() {
        let work_dir = tempfile::tempdir().unwrap();
        let dir = work_dir.path().join("clones");
        std::fs::create_dir_all(&dir).unwrap();
        File::create(dir.join("issue-12-AbC123.lock")).unwrap();
        let (sweeper, sweeping) = clones(work_dir.path());

        sweeper.sweep();
        sweeping.wait().await;

        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test_log::test(tokio::test)]
    async fn a_sweep_with_no_clones_dir_does_nothing() {
        let work_dir = tempfile::tempdir().unwrap();
        let (sweeper, sweeping) = clones(work_dir.path());

        sweeper.sweep();
        sweeping.wait().await;

        assert!(!work_dir.path().join("clones").exists());
    }

    #[test_log::test(tokio::test)]
    async fn making_a_clone_whose_lock_file_is_removed_before_it_is_locked_takes_a_new_name() {
        let work_dir = tempfile::tempdir().unwrap();
        let removed = Arc::new(Mutex::new(None));
        let (owner, _) = clones(work_dir.path());
        let owner = owner.on_lock_created({
            let removed = removed.clone();
            move |lock: &Path| {
                let mut removed = removed.lock().unwrap();
                if removed.is_none() {
                    std::fs::remove_file(lock).unwrap();
                    *removed = Some(lock.to_owned());
                }
            }
        });

        let clone = owner.make("issue-12").await.unwrap();

        let removed = removed
            .lock()
            .unwrap()
            .clone()
            .expect("a lock file was made");
        let lock = work_dir
            .path()
            .join("clones")
            .join(format!("{}.lock", clone.name()));
        assert_ne!(lock, removed);
        assert!(!removed.exists());
        let (sweeper, sweeping) = clones(work_dir.path());
        sweeper.sweep();
        sweeping.wait().await;
        assert!(clone.path().is_dir());
        assert!(lock.is_file());
    }

    #[test_log::test(tokio::test)]
    async fn a_dropped_clone_is_removed_with_its_lock_file() {
        let work_dir = tempfile::tempdir().unwrap();
        let (owner, winding_up) = clones(work_dir.path());
        let clone = owner.make("issue-12").await.unwrap();
        std::fs::write(clone.path().join("file"), "x").unwrap();

        drop(clone);
        winding_up.wait().await;

        assert_eq!(
            std::fs::read_dir(work_dir.path().join("clones"))
                .unwrap()
                .count(),
            0
        );
    }
}
