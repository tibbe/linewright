//! Bookkeeping Linewright keeps under the repository's work directory, each
//! record a JSON file that no person reads.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tracing::warn;

/// The record at `path`, `None` when it is missing, and when it is
/// unreadable, which is logged as ignoring `what`.
pub(crate) fn load<T: DeserializeOwned>(path: &Path, what: &str) -> Option<T> {
    let read = || -> Result<Option<T>> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    };
    read().unwrap_or_else(|e| {
        warn!(path = %path.display(), error = format!("{e:#}"), "ignoring {what}");
        None
    })
}

/// Records `value` at `path`, whole: to a file beside the record, then
/// renamed over it, so a crash leaves the old record or the new one.
pub(crate) fn save(path: &Path, value: &impl Serialize, what: &str) -> Result<()> {
    let dir = path.parent().context("the record has a directory")?;
    std::fs::create_dir_all(dir).context("creating the work directory")?;
    let mut file =
        tempfile::NamedTempFile::new_in(dir).with_context(|| format!("writing {what}"))?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.persist(path)
        .with_context(|| format!("writing {what}"))?;
    Ok(())
}

/// Removes the record at `path`; one that is missing is already removed.
pub(crate) fn remove(path: &Path, what: &str) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("removing {what}"))
        }
        _ => Ok(()),
    }
}
