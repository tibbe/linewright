//! The clones runs work in, each a directory under the work directory's
//! `clones` (`docs/specs/runs.md`, "A clone's end").

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tempfile::TempDir;

use crate::winding_up::{Remaining, WindingUp};

/// Where clones are made, and who removes them.
#[derive(Clone)]
pub struct Clones {
    dir: PathBuf,
    winding_up: WindingUp,
}

impl Clones {
    /// The clones in `work_dir`, removed through `winding_up`.
    pub fn new(work_dir: &Path, winding_up: WindingUp) -> Clones {
        Clones {
            dir: work_dir.join("clones"),
            winding_up,
        }
    }

    /// A new, empty directory for a clone, named `prefix` and a random
    /// suffix.
    pub async fn make(&self, prefix: &str) -> Result<CloneDir> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .context("creating the clones directory")?;
        let temp = tempfile::Builder::new()
            .prefix(&format!("{prefix}-"))
            .tempdir_in(&self.dir)
            .context("creating a clone directory")?;
        Ok(CloneDir {
            temp: Some(temp),
            first: None,
            winding_up: self.winding_up.clone(),
        })
    }
}

/// A clone's directory, removed through [`WindingUp`] once dropped, however
/// far its making got.
pub struct CloneDir {
    /// `None` once dropped.
    temp: Option<TempDir>,
    first: Option<Box<dyn FnOnce() + Send>>,
    winding_up: WindingUp,
}

impl CloneDir {
    pub fn path(&self) -> &Path {
        self.temp.as_ref().expect("held until dropped").path()
    }

    /// The directory's name in `clones`.
    pub fn name(&self) -> String {
        self.path()
            .file_name()
            .expect("made in `clones`")
            .to_string_lossy()
            .into_owned()
    }

    /// Has `first` run, off the runtime's threads, before the directory is
    /// removed, after any given earlier.
    pub fn then_remove(&mut self, first: impl FnOnce() + Send + 'static) {
        let earlier = self.first.take();
        self.first = Some(Box::new(move || {
            if let Some(earlier) = earlier {
                earlier();
            }
            first();
        }));
    }
}

impl Drop for CloneDir {
    fn drop(&mut self) {
        let name = self.name();
        let temp = self.temp.take();
        let first = self.first.take();
        self.winding_up
            .spawn_blocking(Remaining::Clone { name }, move || {
                if let Some(first) = first {
                    first();
                }
                drop(temp);
            });
    }
}
