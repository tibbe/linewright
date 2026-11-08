//! The known ways to copy a tree and to remove one on this platform: those
//! it documents or other software uses (`docs/specs/runs.md`, Performance),
//! each with a link to its source. Linewright's own is first.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};
use clap::ValueEnum;

use crate::walk::{self, Walk};

/// A known way to copy a directory to a path that does not exist.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CopyWay {
    /// `linewright::tree::copy`.
    Linewright,
    /// `cp -a`, cloning each file where it can: GNU's with `--reflink=auto`
    /// (<https://www.gnu.org/software/coreutils/manual/html_node/cp-invocation.html>),
    /// macOS's with `-c` (<https://keith.github.io/xcode-man-pages/cp.1.html>).
    Cp,
    /// One `clonefile(2)` of the whole tree, which APFS can make
    /// (<https://keith.github.io/xcode-man-pages/clonefile.2.html>).
    #[cfg(target_os = "macos")]
    Clonefile,
    /// A serial walk cloning or copying each file, as uv does
    /// (<https://docs.astral.sh/uv/reference/settings/#link-mode>) and pnpm
    /// (<https://pnpm.io/settings#packageimportmethod>).
    SerialClone,
    /// A parallel walk, as fuc's `cpz` (<https://github.com/SUPERCILEX/fuc>)
    /// and xcp (<https://github.com/tarka/xcp>) make, cloning or copying each
    /// file as uv does (<https://docs.astral.sh/uv/reference/settings/#link-mode>).
    ParallelClone,
    /// A parallel walk copying each file with `copy_file_range(2)`, as xcp
    /// does (<https://github.com/tarka/xcp>).
    #[cfg(target_os = "linux")]
    ParallelCopyFileRange,
    /// `tar` piped into `tar`, as moby copies a tree
    /// (<https://github.com/moby/go-archive/blob/main/archive.go>,
    /// `CopyWithTar`), in the pax format, which keeps mtimes to the
    /// nanosecond, as `stat` gives them
    /// (<https://pubs.opengroup.org/onlinepubs/9799919799/utilities/pax.html>,
    /// pax Extended Header File Times).
    Tar,
}

/// A known way to remove a directory and all it holds.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RemoveWay {
    /// `linewright::tree::remove`.
    Linewright,
    /// Rust's `std::fs::remove_dir_all`
    /// (<https://doc.rust-lang.org/std/fs/fn.remove_dir_all.html>).
    Std,
    /// `rm -rf`
    /// (<https://pubs.opengroup.org/onlinepubs/9799919799/utilities/rm.html>).
    Rm,
    /// The `remove_dir_all` crate with its `parallel` feature
    /// (<https://docs.rs/remove_dir_all/1.0.0/remove_dir_all/>).
    RemoveDirAllParallel,
}

/// The names of the ways, Linewright's first.
pub fn names<T: ValueEnum>() -> Vec<String> {
    T::value_variants()
        .iter()
        .filter_map(clap::ValueEnum::to_possible_value)
        .map(|value| value.get_name().to_owned())
        .collect()
}

pub fn copy(way: CopyWay, from: &Path, to: &Path) -> Result<()> {
    match way {
        CopyWay::Linewright => {
            tokio::runtime::Runtime::new()?.block_on(linewright::tree::copy(from, to))
        }
        CopyWay::Cp => {
            let clone = if cfg!(target_os = "macos") {
                "-c"
            } else {
                "--reflink=auto"
            };
            run(Command::new("cp").args(["-a", clone]).arg(from).arg(to))
        }
        #[cfg(target_os = "macos")]
        CopyWay::Clonefile => Ok(reflink_copy::reflink(from, to)?),
        CopyWay::SerialClone => Ok(walk::copy(from, to, Walk::Serial, walk::clone_or_copy)?),
        CopyWay::ParallelClone => Ok(walk::copy(from, to, Walk::Parallel, walk::clone_or_copy)?),
        #[cfg(target_os = "linux")]
        CopyWay::ParallelCopyFileRange => {
            Ok(walk::copy(from, to, Walk::Parallel, walk::copy_file_range)?)
        }
        CopyWay::Tar => tar(from, to),
    }
}

/// Removes `path`, failing where any of it is left.
pub fn remove(way: RemoveWay, path: &Path) -> Result<()> {
    match way {
        RemoveWay::Linewright => linewright::tree::remove(path)?,
        RemoveWay::Std => std::fs::remove_dir_all(path)?,
        RemoveWay::Rm => run(Command::new("rm").arg("-rf").arg(path))?,
        RemoveWay::RemoveDirAllParallel => remove_dir_all::remove_dir_all(path)?,
    }
    if path.symlink_metadata().is_ok() {
        bail!("{} is still there", path.display());
    }
    Ok(())
}

fn tar(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir(to)?;
    let mut create = Command::new("tar")
        .arg("-C")
        .arg(from)
        .args(["-cf", "-", "--format=pax", "."])
        .stdout(Stdio::piped())
        .spawn()
        .context("running tar")?;
    let archive = create.stdout.take().context("no output from tar")?;
    let extracted = run(Command::new("tar")
        .arg("-C")
        .arg(to)
        .args(["-xpf", "-"])
        .stdin(archive));
    if !create.wait()?.success() {
        bail!("tar -c failed");
    }
    extracted
}

fn run(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("running {command:?}"))?;
    if !status.success() {
        bail!("{command:?}: {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{generate, same};

    #[test_log::test]
    fn every_copy_way_makes_a_faithful_copy() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        generate::tree(&tree, 300).unwrap();
        for way in CopyWay::value_variants() {
            let copy = dir.path().join(format!("{way:?}"));
            super::copy(*way, &tree, &copy).unwrap();
            assert_eq!(
                same::differences(
                    &same::entries(&tree).unwrap(),
                    &same::entries(&copy).unwrap()
                ),
                [],
                "{way:?}"
            );
        }
    }

    #[test_log::test]
    fn every_remove_way_removes_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        generate::tree(&tree, 300).unwrap();
        for way in RemoveWay::value_variants() {
            let copy = dir.path().join(format!("{way:?}"));
            super::copy(CopyWay::Linewright, &tree, &copy).unwrap();
            super::remove(*way, &copy).unwrap();
            assert!(copy.symlink_metadata().is_err(), "{way:?}");
        }
    }

    #[test_log::test]
    fn lists_linewright_s_way_first() {
        assert_eq!(names::<CopyWay>()[0], "linewright");
        assert_eq!(names::<RemoveWay>()[0], "linewright");
    }
}
