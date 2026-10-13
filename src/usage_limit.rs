//! The usage limit `serve` waits out, recorded under the repository's work
//! directory for `status`, which runs as its own process, to read
//! (`docs/specs/operation.md`, "Usage limit"). Only `serve` records it.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::records;
use crate::runtime::UsageLimit;

const WHAT: &str = "the usage limit";

/// The usage limit recorded under `work_dir`, if one is.
pub fn load(work_dir: &Path) -> Option<UsageLimit> {
    records::load(&path(work_dir), WHAT)
}

/// Records `limit` under `work_dir` as the usage limit in force, or that
/// none is.
pub fn record(work_dir: &Path, limit: Option<UsageLimit>) -> Result<()> {
    match limit {
        Some(limit) => records::save(&path(work_dir), &limit, WHAT),
        None => records::remove(&path(work_dir), WHAT),
    }
}

fn path(work_dir: &Path) -> PathBuf {
    work_dir.join("usage-limit.json")
}
