//! Copying and removing a directory tree, each within the margin of the
//! fastest known way (`docs/specs/runs.md`, Performance).

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::child;

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
    // GNU cp clones each file with `--reflink=auto`
    // (https://www.gnu.org/software/coreutils/manual/html_node/cp-invocation.html),
    // and macOS's with `-c`, falling back to a plain copy where cloning is
    // not supported (https://keith.github.io/xcode-man-pages/cp.1.html).
    let clone = if cfg!(target_os = "macos") {
        "-c"
    } else {
        "--reflink=auto"
    };
    let output = child::command("cp")
        .args(["-a", clone])
        .arg(from)
        .arg(to)
        .output()
        .await
        .context("running cp")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
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

/// Removes the directory `path` and all it holds, as far as it is there,
/// within the margin of the fastest known way (`docs/specs/runs.md`,
/// Performance): `Ok` where `path` is already gone. Blocks until it is
/// done.
pub fn remove(path: &Path) -> Result<()> {
    // On macOS, `removefile(3)`
    // (https://keith.github.io/xcode-man-pages/removefile.3.html) unlinks
    // entry by entry, and on APFS measured slower than `remove_dir_all`, as
    // did a parallel unlink
    // (https://docs.rs/crate/decmpfs/0.1.2/source/src/remove.rs).
    unless_missing(std::fs::remove_dir_all(path))
        .with_context(|| format!("removing {}", path.display()))?;
    Ok(())
}

/// `None` where `result` failed for want of the file.
pub(crate) fn unless_missing<T>(result: std::io::Result<T>) -> std::io::Result<Option<T>> {
    match result {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
