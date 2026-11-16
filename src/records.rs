use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

/// Records `value` at `path`, whole: a crash leaves the old record or the
/// new one.
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
