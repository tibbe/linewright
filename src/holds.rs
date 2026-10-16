//! The pull requests Linewright held for review, with what GitHub required
//! of each then: it holds one again only when that changes
//! (`docs/specs/pull_requests.md`, "Merging"). Bookkeeping Linewright keeps
//! under the work directory and no person reads, so a restart holds none
//! again.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::domain::{Login, Mergeability, Number, PullState, Requirement, Snapshot};
use crate::records;

/// What GitHub required of each pull request when Linewright last held it
/// for review.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Holds(BTreeMap<Number, Vec<Requirement>>);

impl Holds {
    /// Remembers that the pull request #`pr` was held for review for
    /// `requires`, in place of what it was held for before.
    pub fn hold(&mut self, pr: Number, requires: &[Requirement]) {
        self.0.insert(pr, requires.to_vec());
    }

    /// Whether the pull request #`pr` was last held for review for
    /// `requires`.
    pub fn is_held_for(&self, pr: Number, requires: &[Requirement]) -> bool {
        self.0.get(&pr).is_some_and(|held| held == requires)
    }

    /// Forgets the pull requests `open` rules out.
    pub fn retain(&mut self, open: impl Fn(Number) -> bool) {
        self.0.retain(|pr, _| open(*pr));
    }

    /// Takes as held the pull requests among `among` that `snapshot` shows
    /// held for review: opened by `linewright`, ready for review and blocked
    /// by GitHub. A Linewright that kept no record held them already.
    /// Returns those among them whose merge state GitHub has not worked out
    /// yet, to take once it has.
    pub fn take_shown(
        &mut self,
        snapshot: &Snapshot,
        linewright: &Login,
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
                Mergeability::Blocked { requires } => self.hold(pr.number, requires),
                Mergeability::Unknown => {
                    unknown.insert(pr.number);
                }
                Mergeability::Clean | Mergeability::Behind | Mergeability::Conflicting => {}
            }
        }
        unknown
    }

    /// The holds recorded under `work_dir`: `None` when there is no record,
    /// and when it is unreadable, which is logged.
    pub fn load(work_dir: &Path) -> Option<Holds> {
        records::load(&path(work_dir), WHAT)
    }

    /// Records these holds under `work_dir`, whole.
    pub fn save(&self, work_dir: &Path) -> Result<()> {
        records::save(&path(work_dir), self, WHAT)
    }
}

const WHAT: &str = "the pull requests held for review";

fn path(work_dir: &Path) -> PathBuf {
    work_dir.join("held-for-review.json")
}
