use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::domain::{Login, Mergeability, Number, PullState, Requirement, Snapshot};
use crate::records;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Holds(BTreeMap<Number, Vec<Requirement>>);

impl Holds {
    pub fn hold(&mut self, pr: Number, requires: &[Requirement]) {
        self.0.insert(pr, requires.to_vec());
    }

    pub fn is_held_for(&self, pr: Number, requires: &[Requirement]) -> bool {
        self.0.get(&pr).is_some_and(|held| held == requires)
    }

    pub fn retain(&mut self, open: impl Fn(Number) -> bool) {
        self.0.retain(|pr, _| open(*pr));
    }

    /// A Linewright that kept no record held them already. Returns those
    /// whose merge state GitHub has not worked out yet, to take once it has.
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

    /// `None` when there is no record, and when it is unreadable, which is
    /// logged.
    pub fn load(work_dir: &Path) -> Option<Holds> {
        records::load(&path(work_dir), WHAT)
    }

    pub fn save(&self, work_dir: &Path) -> Result<()> {
        records::save(&path(work_dir), self, WHAT)
    }
}

const WHAT: &str = "the pull requests held for review";

fn path(work_dir: &Path) -> PathBuf {
    work_dir.join("held-for-review.json")
}
