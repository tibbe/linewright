//! The records that earlier releases kept as files in the work directory,
//! read into the tables of the migration that makes them. The files stay
//! where they are.

use std::path::Path;

use anyhow::Context as _;
use sqlx::{Sqlite, Transaction};

use super::StepsFuture;
use crate::runs::{self, files::RunFiles};

/// Imports the work directory's runs and the inputs they recorded, from
/// where this release's predecessors kept them, logging and skipping a
/// record that does not read.
pub(super) fn runs<'a>(
    tx: &'a mut Transaction<'static, Sqlite>,
    work_dir: &'a Path,
) -> StepsFuture<'a> {
    Box::pin(async move {
        let files = RunFiles::new(work_dir);
        let legacy = tokio::task::spawn_blocking(move || files.read_legacy())
            .await
            .context("reading the runs' records")??;
        for (recorded, input) in &legacy {
            runs::import(tx, work_dir, recorded, input.as_ref())
                .await
                .with_context(|| format!("importing the run {}", recorded.id))?;
        }
        Ok(())
    })
}
