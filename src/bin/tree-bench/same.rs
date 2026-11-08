//! Whether one tree is a faithful copy of another: the same entries, of the
//! same types, with the same modes, mtimes and symlink targets.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// What a faithful copy keeps of an entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The type and the permission bits.
    pub mode: u32,
    pub mtime: Mtime,
    pub target: Option<PathBuf>,
}

/// A modification time, to the nanosecond.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mtime {
    pub seconds: i64,
    pub nanoseconds: i64,
}

/// Every entry of a tree, its root included as the empty path, by its path
/// relative to the root.
pub type Entries = BTreeMap<PathBuf, Entry>;

/// How a copy differs from its original at one path.
#[derive(Debug, PartialEq)]
pub enum Difference {
    Missing(PathBuf),
    Added(PathBuf),
    Changed {
        path: PathBuf,
        original: Entry,
        copy: Entry,
    },
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Difference::Missing(path) => write!(f, "{}: missing", path.display()),
            Difference::Added(path) => write!(f, "{}: not in the original", path.display()),
            Difference::Changed {
                path,
                original,
                copy,
            } => write!(f, "{}: {original:?} copied as {copy:?}", path.display()),
        }
    }
}

/// How `copy` differs from `original`, or nothing where it is a faithful
/// copy.
pub fn differences(original: &Entries, copy: &Entries) -> Vec<Difference> {
    let mut differences = Vec::new();
    for (path, entry) in original {
        match copy.get(path) {
            None => differences.push(Difference::Missing(path.clone())),
            Some(copied) if copied != entry => differences.push(Difference::Changed {
                path: path.clone(),
                original: entry.clone(),
                copy: copied.clone(),
            }),
            Some(_) => {}
        }
    }
    for path in copy.keys().filter(|path| !original.contains_key(*path)) {
        differences.push(Difference::Added(path.clone()));
    }
    differences
}

/// Reads the entries of the tree at `root`.
pub fn entries(root: &Path) -> Result<Entries> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let path = root.join(&relative);
        let metadata =
            fs::symlink_metadata(&path).with_context(|| format!("reading {}", path.display()))?;
        let target = if metadata.is_symlink() {
            Some(fs::read_link(&path)?)
        } else {
            None
        };
        if metadata.is_dir() {
            for child in fs::read_dir(&path)? {
                pending.push(relative.join(child?.file_name()));
            }
        }
        entries.insert(
            relative,
            Entry {
                mode: metadata.mode(),
                mtime: Mtime {
                    seconds: metadata.mtime(),
                    nanoseconds: metadata.mtime_nsec(),
                },
                target,
            },
        );
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt as _;
    use std::process::Command;

    use super::*;

    const FILE: u32 = 0o100_644;
    const DIRECTORY: u32 = 0o040_755;
    const SYMLINK: u32 = 0o120_777;

    fn entry(mode: u32, seconds: i64, nanoseconds: i64, target: Option<&str>) -> Entry {
        Entry {
            mode,
            mtime: Mtime {
                seconds,
                nanoseconds,
            },
            target: target.map(PathBuf::from),
        }
    }

    fn original() -> Entries {
        Entries::from([
            (PathBuf::new(), entry(DIRECTORY, 100, 5, None)),
            ("file".into(), entry(FILE, 100, 1, None)),
            ("link".into(), entry(SYMLINK, 100, 2, Some("file"))),
        ])
    }

    #[test_log::test]
    fn finds_none_in_the_same_entries() {
        assert_eq!(differences(&original(), &original()), []);
    }

    #[test_log::test]
    fn finds_a_missing_entry_and_an_added_one() {
        let mut copy = original();
        copy.remove(Path::new("file"));
        copy.insert("extra".into(), entry(FILE, 100, 1, None));
        assert_eq!(
            differences(&original(), &copy),
            [
                Difference::Missing("file".into()),
                Difference::Added("extra".into())
            ]
        );
    }

    #[test_log::test]
    fn finds_a_changed_mode() {
        let mut copy = original();
        copy.insert("file".into(), entry(0o100_755, 100, 1, None));
        assert_eq!(
            differences(&original(), &copy),
            [Difference::Changed {
                path: "file".into(),
                original: entry(FILE, 100, 1, None),
                copy: entry(0o100_755, 100, 1, None),
            }]
        );
    }

    #[test_log::test]
    fn finds_a_changed_mtime_to_the_nanosecond() {
        let mut copy = original();
        copy.insert(PathBuf::new(), entry(DIRECTORY, 100, 6, None));
        assert_eq!(differences(&original(), &copy).len(), 1);
    }

    #[test_log::test]
    fn finds_a_changed_symlink_target() {
        let mut copy = original();
        copy.insert("link".into(), entry(SYMLINK, 100, 2, Some("other")));
        assert_eq!(differences(&original(), &copy).len(), 1);
    }

    #[test_log::test]
    fn finds_a_symlink_copied_as_its_target() {
        let mut copy = original();
        copy.insert("link".into(), entry(FILE, 100, 2, None));
        assert_eq!(differences(&original(), &copy).len(), 1);
    }

    /// `cp -a` keeps all a faithful copy keeps
    /// (<https://pubs.opengroup.org/onlinepubs/9799919799/utilities/cp.html>,
    /// `-p`, and GNU's `-a`
    /// <https://www.gnu.org/software/coreutils/manual/html_node/cp-invocation.html>).
    #[test_log::test]
    fn reads_the_same_entries_from_a_copy_by_cp_a() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original");
        fs::create_dir_all(original.join("sub")).unwrap();
        fs::write(original.join("sub/file"), "text").unwrap();
        fs::write(original.join("run"), "#!/bin/sh").unwrap();
        fs::set_permissions(original.join("run"), Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("sub/file", original.join("link")).unwrap();
        let copy = dir.path().join("copy");
        let status = Command::new("cp")
            .arg("-a")
            .arg(&original)
            .arg(&copy)
            .status()
            .unwrap();
        assert!(status.success());
        let entries = entries(&original).unwrap();
        assert_eq!(
            entries.keys().collect::<Vec<_>>(),
            ["", "link", "run", "sub", "sub/file"].map(Path::new)
        );
        assert_eq!(entries[Path::new("link")].target, Some("sub/file".into()));
        assert_eq!(entries[Path::new("run")].mode & 0o777, 0o755);
        assert_eq!(differences(&entries, &super::entries(&copy).unwrap()), []);
    }
}
