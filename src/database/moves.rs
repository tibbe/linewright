//! The moves of the work directory's files that a migration reads from the
//! database as it changes it ([`super::Migration::Moves`]), made in two
//! phases, so that a move whose target is another move's source cannot land
//! on what has not moved yet.

use std::fs::File;
use std::path::{Component, Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// The directory, under the work directory, that holds the plan of each
/// migration's moves, `V/` for the migration to version V, until that
/// migration is committed.
pub(super) const MOVING: &str = ".moving";

/// The file, in a migration's directory in [`MOVING`], of its moves, written
/// before the first of them.
const PLAN: &str = "moves.json";

/// The file, in a migration's directory in [`MOVING`], whose presence says
/// that every source there was is out of its place.
const TAKEN_OUT: &str = "taken-out";

/// A move of a source to a target, both relative to the work directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Move {
    from: PathBuf,
    to: PathBuf,
}

impl Move {
    /// The move a migration's query gives as `from` and `to`.
    ///
    /// # Errors
    ///
    /// When either is not a path of plain names under the work directory,
    /// in a directory of it.
    pub(super) fn of(from: String, to: String) -> anyhow::Result<Move> {
        let relative = |path: String| {
            let path = PathBuf::from(path);
            anyhow::ensure!(
                path.components().count() > 1
                    && path.components().all(|c| matches!(c, Component::Normal(_))),
                "{} is not a path in a directory of the work directory",
                path.display()
            );
            Ok(path)
        };
        Ok(Move {
            from: relative(from)?,
            to: relative(to)?,
        })
    }

    /// Where the `i`th move of the migration to `version` holds its source
    /// while it is out of place: beside it, so that it stays on its
    /// filesystem.
    fn held(&self, work_dir: &Path, version: u32, i: usize) -> PathBuf {
        let parent = self.from.parent().unwrap_or(Path::new(""));
        work_dir.join(parent).join(format!(".moving-{version}-{i}"))
    }
}

/// Carries out `moves`, the moves of the migration to `version`, in
/// `work_dir`, planning them first. First each source there is goes out of
/// its place, then each goes to its target, and the directories its source
/// left empty are removed. Each phase picks up where an earlier run of it
/// stopped, so that a migration rolled back by a failure is carried out
/// again from where it stopped. The moves are on disk once it returns.
///
/// # Errors
///
/// When a move fails, a target is taken, or an earlier run planned other
/// moves, as when another release changed the database since that run
/// failed, which each leave it for a person to settle.
pub(super) fn carry_out(work_dir: &Path, version: u32, moves: &[Move]) -> anyhow::Result<()> {
    let dir = work_dir.join(MOVING).join(version.to_string());
    let plan = dir.join(PLAN);
    if exists(&plan)? {
        let text = std::fs::read(&plan).with_context(|| format!("reading {}", plan.display()))?;
        let planned: Vec<Move> =
            serde_json::from_slice(&text).with_context(|| format!("reading {}", plan.display()))?;
        anyhow::ensure!(
            planned == moves,
            "the moves planned in {} are not those the database gives now, as it changed since \
             they began: restore the work directory from a backup made before them",
            plan.display()
        );
    } else if moves.is_empty() {
        return Ok(());
    } else {
        write_plan(&dir, moves)?;
    }
    let taken_out = dir.join(TAKEN_OUT);
    if !exists(&taken_out)? {
        for (i, m) in moves.iter().enumerate() {
            let (from, held) = (work_dir.join(&m.from), m.held(work_dir, version, i));
            if !exists(&held)? && exists(&from)? {
                rename(&from, &held)?;
            }
        }
        sync_dirs(moves.iter().filter_map(|m| m.from.parent()), work_dir)?;
        File::create(&taken_out).with_context(|| format!("making {}", taken_out.display()))?;
        sync_dir(&dir)?;
    }
    for (i, m) in moves.iter().enumerate() {
        let held = m.held(work_dir, version, i);
        if !exists(&held)? {
            continue;
        }
        let to = work_dir.join(&m.to);
        anyhow::ensure!(
            !exists(&to)?,
            "{} is in the way of {}, which was at {}: remove it",
            to.display(),
            held.display(),
            m.from.display()
        );
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("making {}", parent.display()))?;
        }
        rename(&held, &to)?;
    }
    for m in moves {
        if exists(&work_dir.join(&m.to))? {
            remove_emptied(work_dir, &m.from)?;
        }
    }
    sync_dirs(
        moves
            .iter()
            .flat_map(|m| [m.from.parent(), m.to.parent()])
            .flatten(),
        work_dir,
    )
}

/// Writes `moves` as the plan in `dir`, all of it or none.
fn write_plan(dir: &Path, moves: &[Move]) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("making {}", dir.display()))?;
    let planning = dir.join(format!("{PLAN}.new"));
    let file = File::create(&planning)
        .and_then(|mut file| {
            serde_json::to_writer(&mut file, moves)?;
            Ok(file)
        })
        .with_context(|| format!("writing {}", planning.display()))?;
    file.sync_all()
        .with_context(|| format!("writing {}", planning.display()))?;
    rename(&planning, &dir.join(PLAN))?;
    sync_dir(dir)
}

/// Removes from `work_dir`'s [`MOVING`] the plans of the migrations up to
/// `version`, which are committed, and [`MOVING`] itself once it is empty.
/// What cannot be removed is left, with a warning.
pub(super) fn clear(work_dir: &Path, version: u32) {
    let moving = work_dir.join(MOVING);
    let Ok(entries) = std::fs::read_dir(&moving) else {
        return;
    };
    for entry in entries.flatten() {
        let committed = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .is_some_and(|of| of <= version);
        if committed && let Err(e) = std::fs::remove_dir_all(entry.path()) {
            tracing::warn!(dir = %entry.path().display(), error = %e, "removing the plan of a migration's moves failed");
        }
    }
    let _left = std::fs::remove_dir(&moving);
}

/// Removes each directory above `from`, under `work_dir`, that is empty,
/// from the nearest up.
fn remove_emptied(work_dir: &Path, from: &Path) -> anyhow::Result<()> {
    let mut dir = from.parent();
    while let Some(parent) = dir.filter(|parent| !parent.as_os_str().is_empty()) {
        let path = work_dir.join(parent);
        match std::fs::remove_dir(&path) {
            Ok(()) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                return Ok(());
            }
            Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
        }
        dir = parent.parent();
    }
    Ok(())
}

/// Puts on disk the entries of each of `dirs` there is, relative to
/// `work_dir`.
fn sync_dirs<'a>(dirs: impl Iterator<Item = &'a Path>, work_dir: &Path) -> anyhow::Result<()> {
    let dirs: std::collections::BTreeSet<&Path> = dirs.collect();
    for dir in dirs {
        let dir = work_dir.join(dir);
        if exists(&dir)? {
            sync_dir(&dir)?;
        }
    }
    Ok(())
}

/// Puts on disk the entries of the directory `dir`.
/// <https://man7.org/linux/man-pages/man2/fsync.2.html>
fn sync_dir(dir: &Path) -> anyhow::Result<()> {
    File::open(dir)
        .and_then(|dir| dir.sync_all())
        .with_context(|| format!("putting {} on disk", dir.display()))
}

fn rename(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::rename(from, to)
        .with_context(|| format!("moving {} to {}", from.display(), to.display()))
}

/// Whether there is anything at `path`, a dangling symbolic link included.
fn exists(path: &Path) -> anyhow::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("looking for {}", path.display())),
    }
}
