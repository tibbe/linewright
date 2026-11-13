//! Removing a tree whatever it holds: [`remove`].

use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
#[cfg(target_os = "linux")]
use rayon::prelude::*;
use rustix::fs::{AtFlags, CWD, FileType};
use rustix::io::Errno;

use handle::{Directory, Opened};

/// Removes the entry at `path`, a directory with all it holds, or anything
/// else, a symlink included, by unlinking it: `Ok` where it is already gone.
pub(super) fn remove(path: &Path) -> Result<()> {
    // A trailing slash or `.` resolves a symlink before `O_NOFOLLOW` applies
    // (https://man7.org/linux/man-pages/man7/path_resolution.7.html), and
    // `components` drops both.
    let path: PathBuf = path.components().collect();
    let dir = match Directory::open_root(&path)? {
        Opened::Directory(dir) => dir,
        Opened::NotDirectory => {
            return Ok(gone_is_done(rustix::fs::unlinkat(
                CWD,
                &path,
                AtFlags::empty(),
            ))?);
        }
        Opened::Gone => return Ok(()),
    };
    let entries = dir.entries()?;
    remove_entries(&dir, &path, &entries)?;
    Ok(gone_is_done(rustix::fs::unlinkat(
        CWD,
        &path,
        AtFlags::REMOVEDIR,
    ))?)
}

/// An entry of a directory, by its name and its type as listing it gave.
struct Entry {
    name: CString,
    file_type: FileType,
}

/// Removes `entries` of `dir`, which is at `path`: in parallel on Linux, and
/// serially elsewhere, where on APFS a parallel unlink measured slower
/// (<https://docs.rs/crate/decmpfs/0.1.2/source/src/remove.rs>).
fn remove_entries(dir: &Directory, path: &Path, entries: &[Entry]) -> Result<()> {
    #[cfg(target_os = "linux")]
    return entries
        .par_iter()
        .with_max_len(1)
        .try_for_each(|entry| remove_entry(dir, path, entry));
    #[cfg(not(target_os = "linux"))]
    entries
        .iter()
        .try_for_each(|entry| remove_entry(dir, path, entry))
}

/// Removes `entry` of `parent`, which is at `parent_path`, and all it holds,
/// naming it in the error where it can't be removed.
fn remove_entry(parent: &Directory, parent_path: &Path, entry: &Entry) -> Result<()> {
    let path = || parent_path.join(OsStr::from_bytes(entry.name.to_bytes()));
    let removing = || format!("removing {}", path().display());
    let file_type = if entry.file_type == FileType::Unknown {
        match parent.file_type(&entry.name).with_context(removing)? {
            Some(file_type) => file_type,
            None => return Ok(()),
        }
    } else {
        entry.file_type
    };
    if file_type == FileType::Directory {
        match parent.open(&entry.name).with_context(removing)? {
            Opened::Directory(dir) => {
                let dir_path = path();
                let entries = dir.entries().with_context(removing)?;
                remove_entries(&dir, &dir_path, &entries)?;
                return parent
                    .unlink(&entry.name, AtFlags::REMOVEDIR)
                    .with_context(removing);
            }
            Opened::NotDirectory => {}
            Opened::Gone => return Ok(()),
        }
    }
    parent
        .unlink(&entry.name, AtFlags::empty())
        .with_context(removing)
}

/// `Ok` where `result` failed for want of the entry.
fn gone_is_done(result: rustix::io::Result<()>) -> rustix::io::Result<()> {
    match result {
        Err(Errno::NOENT) => Ok(()),
        result => result,
    }
}

/// Runs `op`, and where it was forbidden (`EACCES` or `EPERM`), runs `mend`
/// and then `op` once more: the second failure is the error, or the first
/// where `mend` fails.
fn retry_if_forbidden<T>(
    mut op: impl FnMut() -> rustix::io::Result<T>,
    mend: impl FnOnce() -> rustix::io::Result<()>,
) -> rustix::io::Result<T> {
    match op() {
        Err(e @ (Errno::ACCESS | Errno::PERM)) => match mend() {
            Ok(()) => op(),
            Err(_) => Err(e),
        },
        result => result,
    }
}

/// Handles of directories in the tree, which are all the walk changes the
/// mode of, but for a directory it names without following a symlink.
mod handle {
    use std::ffi::CStr;
    use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
    use std::path::Path;

    use rustix::fs::{AtFlags, CWD, Dir, FileType, Mode, OFlags};
    use rustix::io::Errno;

    use super::{Entry, gone_is_done, retry_if_forbidden};

    /// A directory in the tree, open without following a symlink.
    pub(super) struct Directory(OwnedFd);

    /// What opening an entry as a directory found.
    pub(super) enum Opened {
        Directory(Directory),
        /// Anything but a directory, such as a symlink to one.
        NotDirectory,
        Gone,
    }

    impl Directory {
        /// Opens `path`, giving its owner read, write and search permission
        /// where opening it was forbidden.
        pub(super) fn open_root(path: &Path) -> rustix::io::Result<Opened> {
            retry_if_forbidden(|| open(CWD, path), || give_owner_all_nofollow(CWD, path))
        }

        /// Opens the entry `name` of this directory, giving this directory
        /// and the entry their owner's read, write and search permission
        /// where opening it was forbidden.
        pub(super) fn open(&self, name: &CStr) -> rustix::io::Result<Opened> {
            retry_if_forbidden(
                || open(self.0.as_fd(), name),
                || {
                    self.give_owner_all()?;
                    give_owner_all_nofollow(self.0.as_fd(), name)
                },
            )
        }

        /// The entries of this directory, but `.` and `..`.
        pub(super) fn entries(&self) -> rustix::io::Result<Vec<Entry>> {
            retry_if_forbidden(|| self.list(), || self.give_owner_all())
        }

        fn list(&self) -> rustix::io::Result<Vec<Entry>> {
            let mut dir = Dir::new(rustix::io::dup(&self.0)?)?;
            dir.rewind();
            let mut entries = Vec::new();
            for entry in dir {
                let entry = entry?;
                let name = entry.file_name();
                if name != c"." && name != c".." {
                    entries.push(Entry {
                        name: name.to_owned(),
                        file_type: entry.file_type(),
                    });
                }
            }
            Ok(entries)
        }

        /// The type of the entry `name`, without following a symlink:
        /// `None` where it is gone.
        pub(super) fn file_type(&self, name: &CStr) -> rustix::io::Result<Option<FileType>> {
            let stat = || rustix::fs::statat(&self.0, name, AtFlags::SYMLINK_NOFOLLOW);
            match retry_if_forbidden(stat, || self.give_owner_all()) {
                Ok(stat) => Ok(Some(FileType::from_raw_mode(stat.st_mode))),
                Err(Errno::NOENT) => Ok(None),
                Err(e) => Err(e),
            }
        }

        /// Unlinks the entry `name`, `Ok` where it is gone.
        pub(super) fn unlink(&self, name: &CStr, flags: AtFlags) -> rustix::io::Result<()> {
            retry_if_forbidden(
                || gone_is_done(rustix::fs::unlinkat(&self.0, name, flags)),
                || self.give_owner_all(),
            )
        }

        fn give_owner_all(&self) -> rustix::io::Result<()> {
            let mode = rustix::fs::fstat(&self.0)?.st_mode;
            rustix::fs::fchmod(&self.0, with_owner_all(mode))
        }
    }

    /// The permission bits of `st_mode`, with the owner's read, write and
    /// search added.
    fn with_owner_all(st_mode: rustix::fs::RawMode) -> Mode {
        Mode::from_raw_mode(st_mode) | Mode::RWXU
    }

    /// Opens `name` relative to `at` as a directory, without following a
    /// symlink, and without blocking where it is a FIFO
    /// (<https://man7.org/linux/man-pages/man2/open.2.html>): on a symlink,
    /// `O_NOFOLLOW` fails with `ELOOP`, or `O_DIRECTORY` first with
    /// `ENOTDIR`, as on anything else that is not a directory.
    fn open<P: rustix::path::Arg>(at: BorrowedFd<'_>, name: P) -> rustix::io::Result<Opened> {
        let flags = OFlags::RDONLY
            | OFlags::DIRECTORY
            | OFlags::NOFOLLOW
            | OFlags::NONBLOCK
            | OFlags::CLOEXEC;
        match rustix::fs::openat(at, name, flags, Mode::empty()) {
            Ok(fd) => Ok(Opened::Directory(Directory(fd))),
            Err(Errno::NOTDIR | Errno::LOOP) => Ok(Opened::NotDirectory),
            Err(Errno::NOENT) => Ok(Opened::Gone),
            Err(e) => Err(e),
        }
    }

    /// Gives the owner of `name` relative to `at` read, write and search
    /// permission where it is a directory, and does nothing to anything
    /// else, a symlink included.
    ///
    /// Linux's `fchmodat` takes no flags, so rustix refuses
    /// `AT_SYMLINK_NOFOLLOW` there
    /// (<https://man7.org/linux/man-pages/man2/fchmodat.2.html>); glibc
    /// emulates the flag through `O_PATH` and `/proc/self/fd`
    /// (<https://sourceware.org/git/?p=glibc.git;a=blob;f=sysdeps/unix/sysv/linux/fchmodat.c>).
    /// Without `/proc`, it fails rather than follow a symlink.
    #[cfg(target_os = "linux")]
    fn give_owner_all_nofollow<P: rustix::path::Arg>(
        at: BorrowedFd<'_>,
        name: P,
    ) -> rustix::io::Result<()> {
        use std::os::fd::AsRawFd as _;
        let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let dir = match rustix::fs::openat(at, name, flags, Mode::empty()) {
            Ok(dir) => dir,
            Err(Errno::NOTDIR | Errno::LOOP | Errno::NOENT) => return Ok(()),
            Err(e) => return Err(e),
        };
        let mode = rustix::fs::fstat(&dir)?.st_mode;
        let through_proc = format!("/proc/self/fd/{}", dir.as_raw_fd());
        rustix::fs::chmod(through_proc, with_owner_all(mode))
    }

    /// Gives the owner of `name` relative to `at` read, write and search
    /// permission where it is a directory, and does nothing to anything
    /// else, a symlink included.
    #[cfg(not(target_os = "linux"))]
    fn give_owner_all_nofollow<P: rustix::path::Arg + Copy>(
        at: BorrowedFd<'_>,
        name: P,
    ) -> rustix::io::Result<()> {
        match rustix::fs::statat(at, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => {
                let mode = with_owner_all(stat.st_mode);
                rustix::fs::chmodat(at, name, mode, AtFlags::SYMLINK_NOFOLLOW)
            }
            Ok(_) | Err(Errno::NOENT) => Ok(()),
            Err(e) => Err(e),
        }
    }
}
