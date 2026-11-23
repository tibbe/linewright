//! Copying a tree by walking it, file by file, keeping modes, times and
//! symlinks as `cp -a` does, directories' times set after their contents.
//! Unlike `cp -a`, it copies each hard link as a file of its own.

use std::fs::{self, FileType, Permissions};
use std::io;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;

use rayon::prelude::*;
use rustix::fs::{AtFlags, CWD, Timespec, Timestamps};

/// Whether a walk copies a directory's entries one after another or in
/// parallel.
#[derive(Clone, Copy)]
pub enum Walk {
    Serial,
    Parallel,
}

/// Copies one regular file's contents to a path that does not exist.
pub type CopyFile = fn(&Path, &Path) -> io::Result<()>;

/// Copies the directory `from` to `to`, which does not exist, each regular
/// file by `file`.
pub fn copy(from: &Path, to: &Path, walk: Walk, file: CopyFile) -> io::Result<()> {
    fs::create_dir(to)?;
    let entries = fs::read_dir(from)?.collect::<io::Result<Vec<_>>>()?;
    let each = |entry: &fs::DirEntry| {
        copy_entry(
            &entry.path(),
            &to.join(entry.file_name()),
            entry.file_type()?,
            walk,
            file,
        )
    };
    match walk {
        Walk::Serial => entries.iter().try_for_each(each)?,
        Walk::Parallel => entries.par_iter().try_for_each(each)?,
    }
    keep_attributes(from, to)
}

fn copy_entry(
    from: &Path,
    to: &Path,
    file_type: FileType,
    walk: Walk,
    file: CopyFile,
) -> io::Result<()> {
    if file_type.is_dir() {
        copy(from, to, walk, file)
    } else if file_type.is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
        keep_attributes(from, to)
    } else {
        file(from, to)?;
        keep_attributes(from, to)
    }
}

/// Gives `to` the mode, unless it is a symlink, and the times of `from`.
fn keep_attributes(from: &Path, to: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(from)?;
    if !metadata.is_symlink() {
        fs::set_permissions(to, Permissions::from_mode(metadata.mode()))?;
    }
    let times = Timestamps {
        last_access: Timespec {
            tv_sec: metadata.atime(),
            tv_nsec: metadata.atime_nsec(),
        },
        last_modification: Timespec {
            tv_sec: metadata.mtime(),
            tv_nsec: metadata.mtime_nsec(),
        },
    };
    rustix::fs::utimensat(CWD, to, &times, AtFlags::SYMLINK_NOFOLLOW)?;
    Ok(())
}

/// Clones the file where the filesystem can, or else copies it:
/// `FICLONE` on Linux
/// (<https://man7.org/linux/man-pages/man2/ioctl_ficlone.2.html>) and
/// `clonefile(2)` on macOS
/// (<https://keith.github.io/xcode-man-pages/clonefile.2.html>).
pub fn clone_or_copy(from: &Path, to: &Path) -> io::Result<()> {
    reflink_copy::reflink_or_copy(from, to)?;
    Ok(())
}

/// Copies the file with `copy_file_range(2)`, which the kernel turns into a
/// clone where the filesystem can
/// (<https://man7.org/linux/man-pages/man2/copy_file_range.2.html>).
#[cfg(target_os = "linux")]
pub fn copy_file_range(from: &Path, to: &Path) -> io::Result<()> {
    let source = fs::File::open(from)?;
    let destination = fs::File::create_new(to)?;
    let mut left = source.metadata()?.len();
    while left > 0 {
        let chunk = usize::try_from(left).unwrap_or(usize::MAX);
        let copied = rustix::fs::copy_file_range(&source, None, &destination, None, chunk)?;
        if copied == 0 {
            break;
        }
        left -= copied as u64;
    }
    Ok(())
}
