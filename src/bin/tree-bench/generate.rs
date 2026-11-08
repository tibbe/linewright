//! A tree shaped like an environment: a git repository whose sources are
//! committed, and a `target` of build output with many small files, a few
//! large ones and symlinks.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

/// How many build output files share a directory.
const FILES_PER_DIRECTORY: usize = 10;

/// How many directories share a parent.
const DIRECTORIES_PER_PARENT: usize = 100;

/// How many large files, such as binaries, the build output holds.
const LARGE_FILES: usize = 4;

/// The size of a large file in a tree of 100k files, scaled to the tree's.
const LARGE_FILE_BYTES: usize = 64 << 20;

/// One source file in this many is committed rather than build output.
const SOURCE_SHARE: usize = 20;

/// Generates at `dir`, which does not exist, a tree of about `files` files
/// of 1–20 KB, the same for the same `files`.
pub fn tree(dir: &Path, files: usize) -> Result<()> {
    let mut bytes = PseudoRandomBytes(0x9E37_79B9_7F4A_7C15);
    fs::create_dir_all(dir)?;
    git(dir, &["init", "--quiet"])?;
    fs::write(dir.join(".gitignore"), "/target/\n")?;
    let sources = files / SOURCE_SHARE;
    for i in 0..sources {
        let path = dir.join(format!("src/m{:03}/f{:02}.rs", i / 20, i % 20));
        write(&path, &mut bytes)?;
    }
    git(dir, &["add", "."])?;
    git(
        dir,
        &[
            "-c",
            "user.name=tree-bench",
            "-c",
            "user.email=tree-bench@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "--message",
            "Sources",
        ],
    )?;
    let debug = dir.join("target/debug");
    for i in 0..files - sources {
        let directory = i / FILES_PER_DIRECTORY;
        let path = debug.join(format!(
            "build/p{:03}/d{:02}/o{}",
            directory / DIRECTORIES_PER_PARENT,
            directory % DIRECTORIES_PER_PARENT,
            i % FILES_PER_DIRECTORY
        ));
        write(&path, &mut bytes)?;
    }
    let large = LARGE_FILE_BYTES / 100_000 * files;
    for i in 0..LARGE_FILES {
        let path = debug.join(format!("deps/bin{i}"));
        fs::create_dir_all(debug.join("deps"))?;
        fs::write(&path, bytes.take(large))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        std::os::unix::fs::symlink(format!("deps/bin{i}"), debug.join(format!("bin{i}")))?;
    }
    Ok(())
}

fn write(path: &Path, bytes: &mut PseudoRandomBytes) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let size = 1024 + usize::try_from(bytes.next() % (19 * 1024))?;
    fs::write(path, bytes.take(size))?;
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    if !output.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Pseudo-random bytes, by xorshift64
/// (<https://www.jstatsoft.org/article/view/v008i14>), so that a filesystem
/// that compresses stores them at their size.
struct PseudoRandomBytes(u64);

impl PseudoRandomBytes {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn take(&mut self, size: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(size + 8);
        while out.len() < size {
            out.extend_from_slice(&self.next().to_le_bytes());
        }
        out.truncate(size);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::same;

    #[test_log::test]
    fn makes_about_as_many_files_as_asked_in_a_clean_repository() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        super::tree(&tree, 1000).unwrap();
        let output = Command::new("git")
            .arg("-C")
            .arg(&tree)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "");
        let (directories, files): (Vec<_>, Vec<_>) = same::entries(&tree.join("target"))
            .unwrap()
            .into_values()
            .partition(|entry| entry.mode & 0o170_000 == 0o040_000);
        let (directories, files) = (directories.len(), files.len());
        assert!((950..1050).contains(&files), "{files} files");
        assert!(
            (90..120).contains(&directories),
            "{directories} directories"
        );
    }
}
