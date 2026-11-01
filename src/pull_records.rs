//! Linewright's record of what it wrote to each open pull request that
//! GitHub does not keep: when it last commented naming what GitHub requires
//! to merge it, and its failed merges (`docs/specs/pull_requests.md`,
//! Working a pull request).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::domain::{Login, Mergeability, Number, PullState, Snapshot, Timestamp};
use crate::records;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PullRecords(BTreeMap<Number, PullRecord>);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct PullRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commented: Option<Timestamp>,
    /// Oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    failed_merges: Vec<FailedMerge>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedMerge {
    /// GitHub's reason.
    pub reason: String,
    pub at: Timestamp,
}

impl PullRecords {
    /// Linewright commented on `pr` at `at` naming what GitHub requires.
    pub fn commented(&mut self, pr: Number, at: Timestamp) {
        self.0.entry(pr).or_default().commented = Some(at);
    }

    pub fn last_commented(&self, pr: Number) -> Option<Timestamp> {
        self.0.get(&pr).and_then(|record| record.commented)
    }

    pub fn merge_failed(&mut self, pr: Number, reason: String, at: Timestamp) {
        let record = self.0.entry(pr).or_default();
        record.failed_merges.push(FailedMerge { reason, at });
    }

    /// Oldest first.
    pub fn failed_merges(&self, pr: Number) -> &[FailedMerge] {
        self.0
            .get(&pr)
            .map_or(&[], |record| record.failed_merges.as_slice())
    }

    pub fn retain(&mut self, open: impl Fn(Number) -> bool) {
        self.0.retain(|pr, _| open(*pr));
    }

    /// A Linewright that kept no record commented already, at `now`, on each
    /// pull request of its `among` ready for review that GitHub blocks.
    /// Returns those whose merge state GitHub has not worked out yet, to
    /// take once it has.
    pub fn take_shown(
        &mut self,
        snapshot: &Snapshot,
        linewright: &Login,
        now: Timestamp,
        among: impl Fn(Number) -> bool,
    ) -> BTreeSet<Number> {
        let mut unknown = BTreeSet::new();
        let ready = snapshot.pulls.iter().filter(|p| {
            &p.author == linewright
                && p.state == (PullState::Open { draft: false })
                && among(p.number)
        });
        for pr in ready {
            match &pr.mergeability {
                Mergeability::Blocked { .. } => self.commented(pr.number, now),
                Mergeability::Unknown => {
                    unknown.insert(pr.number);
                }
                Mergeability::Clean | Mergeability::Behind | Mergeability::Conflicting => {}
            }
        }
        unknown
    }

    /// `None` when there is no record, and when it is unreadable, which is
    /// logged.
    pub fn load(work_dir: &Path) -> Option<PullRecords> {
        records::load(&path(work_dir), WHAT)
    }

    pub fn save(&self, work_dir: &Path) -> Result<()> {
        records::save(&path(work_dir), self, WHAT)
    }
}

const WHAT: &str = "the record of Linewright's pull requests";

/// Not the requirements-keyed `held-for-review.json` an earlier release
/// kept, which reads as no record.
fn path(work_dir: &Path) -> PathBuf {
    work_dir.join("pull-requests.json")
}
