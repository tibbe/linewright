//! Copying and removing a directory tree, each within the margin of the
//! fastest known way (`docs/specs/runs.md`, Performance), which
//! `scripts/check bench` checks on the host it runs on.

use std::collections::HashMap;
use std::fs::{self, Metadata, Permissions};
use std::io;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use rustix::fs::{AtFlags, CWD, Timespec, Timestamps};

mod removal;

/// Copies the directory `from` to `to`, which does not exist, sharing disk
/// space where the filesystem allows, within the margin of the fastest
/// known way (`docs/specs/runs.md`, Performance).
///
/// On failure, `to` may hold a partial copy, which is the caller's to
/// remove.
pub async fn copy(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    #[cfg(target_os = "macos")]
    if clone_whole(from, to).await? {
        return Ok(());
    }
    walk(from, to).await
}

/// Copies the directory `from` to `to`, which does not exist, entry by
/// entry, keeping what `cp -a` keeps: [`copy`] where one call can't clone
/// the whole tree.
async fn walk(from: &Path, to: &Path) -> Result<()> {
    let walk = Walk::default();
    let _stop = StopOnDrop(Arc::clone(&walk.stopped));
    let (from, to) = (from.to_owned(), to.to_owned());
    tokio::task::spawn_blocking(move || walk.entry(&from, &to)).await?
}

/// Stops a [`Walk`] once dropped, as [`copy`]'s future is where its caller
/// stops waiting for it.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// A copy of a tree in progress, keeping what `cp -a` keeps
/// (<https://www.gnu.org/software/coreutils/manual/html_node/cp-invocation.html>):
/// modes, access and modification times, symlinks' included, symlinks, and
/// hard links within the tree.
#[derive(Default)]
struct Walk {
    /// The copy of each file linked more than once.
    linked: Mutex<HashMap<Inode, FirstCopy>>,
    /// Whether to copy no more entries.
    stopped: Arc<AtomicBool>,
}

/// A file, by its device and inode number.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Inode {
    device: u64,
    number: u64,
}

impl From<&Metadata> for Inode {
    fn from(metadata: &Metadata) -> Inode {
        Inode {
            device: metadata.dev(),
            number: metadata.ino(),
        }
    }
}

/// The first copy of a file linked more than once, once it is made.
type FirstCopy = Arc<Mutex<Option<PathBuf>>>;

impl Walk {
    /// Copies the entry `from` to `to`, which does not exist, a directory's
    /// entries in parallel: an error once the walk is stopped.
    fn entry(&self, from: &Path, to: &Path) -> Result<()> {
        if self.stopped.load(Ordering::Relaxed) {
            bail!("the copy was stopped");
        }
        let copying = || format!("copying {}", from.display());
        let metadata = fs::symlink_metadata(from).with_context(copying)?;
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            let entries = fs::create_dir(to)
                .and_then(|()| fs::read_dir(from)?.collect::<io::Result<Vec<_>>>())
                .with_context(copying)?;
            entries
                .par_iter()
                .try_for_each(|entry| self.entry(&entry.path(), &to.join(entry.file_name())))?;
            return keep_mode(to, &metadata)
                .and_then(|()| keep_times(to, &metadata))
                .with_context(copying);
        }
        let copied = if file_type.is_symlink() {
            fs::read_link(from)
                .and_then(|target| std::os::unix::fs::symlink(target, to))
                .and_then(|()| keep_times(to, &metadata))
        } else if !file_type.is_file() {
            special(from, to)
        } else if metadata.nlink() > 1 {
            self.linked_file(from, to, &metadata)
        } else {
            file(from, to, &metadata)
        };
        copied.with_context(copying)
    }

    /// Copies the file `from`, linked more than once, to `to`: a hard link
    /// to its first copy, once there is one.
    fn linked_file(&self, from: &Path, to: &Path, metadata: &Metadata) -> io::Result<()> {
        let copy = Arc::clone(
            self.linked
                .lock()
                .expect("no copy panics holding it")
                .entry(Inode::from(metadata))
                .or_default(),
        );
        let mut copy = copy.lock().expect("no copy panics holding it");
        match &*copy {
            Some(first) => fs::hard_link(first, to),
            None => {
                file(from, to, metadata)?;
                *copy = Some(to.to_owned());
                Ok(())
            }
        }
    }
}

/// Copies the regular file `from`, whose metadata is `metadata`, to `to`,
/// cloning it where the filesystem can: `FICLONE` on Linux
/// (<https://man7.org/linux/man-pages/man2/ioctl_ficlone.2.html>) and
/// `clonefile(2)` on macOS
/// (<https://keith.github.io/xcode-man-pages/clonefile.2.html>).
fn file(from: &Path, to: &Path, metadata: &Metadata) -> io::Result<()> {
    reflink_copy::reflink_or_copy(from, to)?;
    keep_mode(to, metadata)?;
    keep_times(to, metadata)
}

/// Copies `from`, neither a directory, a symlink nor a regular file, such as
/// a FIFO or a socket, with `cp -a`, which makes such files anew
/// (<https://pubs.opengroup.org/onlinepubs/9799919799/utilities/cp.html>,
/// `-R`), out of the reach of a terminal's Ctrl-C, as
/// [`crate::child::command`]'s programs are.
fn special(from: &Path, to: &Path) -> io::Result<()> {
    let output = std::process::Command::new("cp")
        .process_group(0)
        .arg("-a")
        .arg(from)
        .arg(to)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(())
}

/// Gives `to` the permission bits of `metadata`.
fn keep_mode(to: &Path, metadata: &Metadata) -> io::Result<()> {
    fs::set_permissions(to, Permissions::from_mode(metadata.mode() & 0o7777))
}

/// Gives `to`, or the symlink `to` itself, the access and modification
/// times of `metadata`.
fn keep_times(to: &Path, metadata: &Metadata) -> io::Result<()> {
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

/// Clones the directory `from` to `to`, which does not exist, in one call:
/// `false`, leaving `to` absent, where the volume can't clone this copy.
///
/// APFS clones a whole hierarchy in one `clonefile(2)` call, failing with
/// `ENOTSUP` where the volume can't clone and `EXDEV` across volumes
/// (<https://keith.github.io/xcode-man-pages/clonefile.2.html>), where
/// `cp -c` and `copyfile(3)` clone each entry in turn.
/// `reflink_copy::reflink` is that call on macOS
/// (<https://docs.rs/reflink-copy/latest/reflink_copy/fn.reflink.html>).
#[cfg(target_os = "macos")]
async fn clone_whole(from: &Path, to: &Path) -> Result<bool> {
    use rustix::io::Errno;
    let (from, to) = (from.to_owned(), to.to_owned());
    tokio::task::spawn_blocking(move || match reflink_copy::reflink(&from, &to) {
        Ok(()) => Ok(true),
        // std reads `ENOTSUP` as no `ErrorKind` on macOS, only `EOPNOTSUPP`
        // (https://github.com/rust-lang/rust/pull/158580).
        Err(e) if matches!(Errno::from_io_error(&e), Some(Errno::NOTSUP | Errno::XDEV)) => {
            remove(&to)?;
            Ok(false)
        }
        Err(e) => Err(e).context("running clonefile"),
    })
    .await?
}

/// Removes the tree at `path` and all it holds, within the margin of the
/// fastest known way (`docs/specs/runs.md`, Performance), whatever its
/// entries are (directories, regular files, symlinks, sockets, FIFOs and
/// device nodes) and whatever their owner permissions, where the user
/// removing them owns them. Never blocks on an entry, such as a FIFO with no
/// writer, and blocks until it is done. Runs in parallel on Linux and
/// serially elsewhere.
///
/// It may give the owner of a directory in the tree read, write and search
/// permission, but never changes a file's mode, never follows a symlink and
/// changes nothing outside `path`: where `path`, or an entry in it, is a
/// symlink, it removes the link and leaves what it points to.
///
/// `Ok` where `path` is already gone, or where an entry goes while it runs.
/// The error, `removing {path}`, has as its cause the entry it could not
/// remove.
pub fn remove(path: &Path) -> Result<()> {
    removal::remove(path).with_context(|| format!("removing {}", path.display()))
}

/// `None` where `result` failed for want of the file.
pub(crate) fn unless_missing<T>(result: std::io::Result<T>) -> std::io::Result<Option<T>> {
    match result {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    /// The file type and permission bits of `path`, itself where it is a
    /// symlink.
    #[track_caller]
    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode()
    }

    /// Through the walk: macOS's whole-tree `clonefile(2)` clones each link of
    /// a file as a file of its own.
    #[test_log::test(tokio::test)]
    async fn copies_a_file_linked_in_many_directories_as_one_file_linked_as_often() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        fs::create_dir_all(from.join("0")).unwrap();
        fs::write(from.join("0/file"), "text").unwrap();
        for i in 1..50 {
            fs::create_dir(from.join(i.to_string())).unwrap();
            fs::hard_link(from.join("0/file"), from.join(format!("{i}/file"))).unwrap();
        }
        let to = dir.path().join("to");

        walk(&from, &to).await.unwrap();

        let original = fs::metadata(from.join("0/file")).unwrap();
        let first = fs::metadata(to.join("0/file")).unwrap();
        assert_ne!(first.ino(), original.ino(), "a copy, not a link out");
        assert_eq!(first.nlink(), 50);
        for i in 1..50 {
            let copied = fs::metadata(to.join(format!("{i}/file"))).unwrap();
            assert_eq!(copied.ino(), first.ino(), "{i}/file");
        }
        assert_eq!(fs::read_to_string(to.join("7/file")).unwrap(), "text");
    }

    #[test_log::test(tokio::test)]
    async fn copies_a_read_only_directory_with_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        fs::create_dir_all(from.join("read-only/sub")).unwrap();
        fs::write(from.join("read-only/sub/file"), "text").unwrap();
        fs::set_permissions(
            from.join("read-only/sub/file"),
            Permissions::from_mode(0o640),
        )
        .unwrap();
        fs::set_permissions(from.join("read-only"), Permissions::from_mode(0o555)).unwrap();
        let to = dir.path().join("to");

        let copied = copy(&from, &to).await;

        let read_only = [
            mode(&to.join("read-only")),
            mode(&to.join("read-only/sub/file")),
        ];
        for tree in [&from, &to] {
            fs::set_permissions(tree.join("read-only"), Permissions::from_mode(0o755)).unwrap();
        }
        copied.unwrap();
        assert_eq!(read_only, [0o040_555, 0o100_640]);
    }

    #[test_log::test(tokio::test)]
    async fn makes_a_fifo_anew() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        fs::create_dir(&from).unwrap();
        let status = Command::new("mkfifo")
            .args(["-m", "640"])
            .arg(from.join("fifo"))
            .status()
            .unwrap();
        assert!(status.success());
        let to = dir.path().join("to");

        copy(&from, &to).await.unwrap();

        assert_eq!(mode(&to.join("fifo")), 0o010_640);
    }

    #[test_log::test]
    fn remove_removes_a_tree_and_all_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        for d in 0..20 {
            let sub = tree.join(format!("d{d}")).join("nested");
            fs::create_dir_all(&sub).unwrap();
            for f in 0..20 {
                fs::write(sub.join(format!("f{f}")), "x").unwrap();
            }
        }

        remove(&tree).unwrap();

        assert!(!tree.exists());
    }

    #[test_log::test]
    fn remove_is_ok_where_the_tree_is_already_gone() {
        let dir = tempfile::tempdir().unwrap();

        remove(&dir.path().join("gone")).unwrap();
    }

    #[test_log::test]
    fn remove_leaves_what_a_symlink_in_the_tree_points_to() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("kept"), "x").unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir(&tree).unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("link")).unwrap();

        remove(&tree).unwrap();

        assert!(!tree.exists());
        assert!(outside.join("kept").exists());
    }

    #[test_log::test]
    fn remove_removes_a_symlink_and_leaves_what_it_points_to() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("kept"), "x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        remove(&link).unwrap();

        assert!(link.symlink_metadata().is_err());
        assert!(target.join("kept").exists());
    }

    #[test_log::test]
    fn remove_removes_a_symlink_named_with_a_trailing_slash_and_leaves_what_it_points_to() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("kept"), "x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mut with_slash = link.clone().into_os_string();
        with_slash.push("/");

        remove(Path::new(&with_slash)).unwrap();

        assert!(link.symlink_metadata().is_err());
        assert!(target.join("kept").exists());
    }

    /// Whether the tests run as root, for whom no permission bits forbid
    /// anything, so a case about them shows nothing.
    fn running_as_root() -> bool {
        rustix::process::geteuid().is_root()
    }

    #[track_caller]
    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
    }

    #[test_log::test]
    fn remove_removes_a_socket() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        let _top = std::os::unix::net::UnixListener::bind(tree.join("top.sock")).unwrap();
        let _sub = std::os::unix::net::UnixListener::bind(tree.join("sub/sub.sock")).unwrap();

        remove(&tree).unwrap();

        assert!(tree.symlink_metadata().is_err());
    }

    #[test_log::test]
    fn remove_removes_a_fifo_without_waiting_for_a_writer() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        for fifo in ["fifo", "sub/fifo"] {
            let status = Command::new("mkfifo")
                .arg(tree.join(fifo))
                .status()
                .unwrap();
            assert!(status.success());
        }
        let (done, removed) = std::sync::mpsc::channel();
        let removing = tree.clone();
        std::thread::spawn(move || done.send(remove(&removing).map_err(|e| format!("{e:#}"))));

        let removed = removed.recv_timeout(std::time::Duration::from_secs(10));

        removed.expect("remove returns").unwrap();
        assert!(tree.symlink_metadata().is_err());
    }

    #[test_log::test]
    fn remove_removes_a_file_its_owner_cant_read_or_write() {
        if running_as_root() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        for file in ["file", "sub/file"] {
            fs::write(tree.join(file), "x").unwrap();
            set_mode(&tree.join(file), 0o000);
        }

        remove(&tree).unwrap();

        assert!(tree.symlink_metadata().is_err());
    }

    /// Owner permissions that each lack what listing a directory or
    /// removing what it holds takes: read, write or search.
    const MODES_LACKING_OWNER_PERMISSION: [u32; 4] = [0o000, 0o500, 0o600, 0o300];

    #[test_log::test]
    fn remove_removes_directories_their_owner_lacks_permissions_on() {
        if running_as_root() {
            return;
        }
        for lacking in MODES_LACKING_OWNER_PERMISSION {
            let dir = tempfile::tempdir().unwrap();
            let tree = dir.path().join("tree");
            fs::create_dir_all(tree.join("empty")).unwrap();
            fs::create_dir_all(tree.join("full/deeper/deepest")).unwrap();
            fs::write(tree.join("full/file"), "x").unwrap();
            fs::write(tree.join("full/deeper/file"), "x").unwrap();
            fs::write(tree.join("full/deeper/deepest/file"), "x").unwrap();
            for sub in ["full/deeper/deepest", "full/deeper", "full", "empty"] {
                set_mode(&tree.join(sub), lacking);
            }

            let removed = remove(&tree);

            removed.unwrap_or_else(|e| panic!("mode {lacking:o}: {e:#}"));
            assert!(tree.symlink_metadata().is_err(), "mode {lacking:o}");
        }
    }

    #[test_log::test]
    fn remove_removes_a_root_its_owner_lacks_permissions_on() {
        if running_as_root() {
            return;
        }
        for lacking in MODES_LACKING_OWNER_PERMISSION {
            for holding in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let tree = dir.path().join("tree");
                fs::create_dir(&tree).unwrap();
                if holding {
                    fs::create_dir(tree.join("sub")).unwrap();
                    fs::write(tree.join("file"), "x").unwrap();
                }
                set_mode(&tree, lacking);

                let removed = remove(&tree);

                let case = format!("mode {lacking:o}, holding entries: {holding}");
                removed.unwrap_or_else(|e| panic!("{case}: {e:#}"));
                assert!(tree.symlink_metadata().is_err(), "{case}");
            }
        }
    }

    #[test_log::test]
    fn remove_leaves_the_mode_of_a_read_only_directory_a_symlink_in_the_tree_points_to() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("kept"), "x").unwrap();
        set_mode(&outside, 0o500);
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("sub/link")).unwrap();
        set_mode(&tree.join("sub"), 0o500);

        let removed = remove(&tree);

        let outside_mode = mode(&outside);
        set_mode(&outside, 0o700);
        removed.unwrap();
        assert!(tree.symlink_metadata().is_err());
        assert_eq!(outside_mode, 0o040_500);
        assert!(outside.join("kept").exists());
    }

    #[test_log::test]
    fn remove_fails_naming_the_tree_where_the_directory_holding_it_forbids_removing_it() {
        if running_as_root() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("parent");
        let tree = parent.join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        fs::write(tree.join("sub/file"), "x").unwrap();
        set_mode(&tree.join("sub"), 0o000);
        set_mode(&parent, 0o500);

        let removed = remove(&tree);

        set_mode(&parent, 0o700);
        let error = format!("{:#}", removed.unwrap_err());
        assert!(
            error.starts_with(&format!("removing {}: ", tree.display())),
            "{error}"
        );
        assert!(error.contains("Permission denied"), "{error}");
    }
}
