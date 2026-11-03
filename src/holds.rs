//! The holds (`docs/specs/runs.md`, Holds): which are in force at a poll,
//! which runs and claims each holds, and their rank.

use std::cmp::Ordering;

use crate::domain::Timestamp;
use crate::reconcile::{Memory, Wait};
use crate::repo_files::{self, FileError, Unusable};
use crate::work_settings::RunKind;

/// A hold, ordered by its rank.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hold {
    Drain,
    UsageLimit { resets_at: Timestamp },
    Environment,
    File(FileError),
}

impl Hold {
    /// `self` against `other` by rank alone, under which every file ranks
    /// the same.
    pub fn cmp_rank(&self, other: &Hold) -> Ordering {
        match (self, other) {
            (Hold::File(_), Hold::File(_)) => Ordering::Equal,
            _ => self.cmp(other),
        }
    }

    /// The hold `wait` is for, if it is for one.
    pub fn of(wait: &Wait) -> Option<Hold> {
        match wait {
            Wait::Drain => Some(Hold::Drain),
            Wait::UsageLimit { resets_at } => Some(Hold::UsageLimit {
                resets_at: *resets_at,
            }),
            Wait::Environment => Some(Hold::Environment),
            Wait::RepositoryFile(file) => Some(Hold::File(file.clone())),
            Wait::Run { .. }
            | Wait::FreeRun { .. }
            | Wait::Blocker { .. }
            | Wait::Assignee { .. }
            | Wait::ClosingPullRequest { .. }
            | Wait::PullRequest { .. }
            | Wait::Ci { .. }
            | Wait::MergeState { .. }
            | Wait::Tickets { .. }
            | Wait::PeopleAsked
            | Wait::Maintainer
            | Wait::Unassigned { .. }
            | Wait::RequiredLabel { .. }
            | Wait::Relabel { .. }
            | Wait::Review { .. }
            | Wait::Requirements { .. } => None,
        }
    }
}

impl From<Hold> for Wait {
    fn from(hold: Hold) -> Wait {
        match hold {
            Hold::Drain => Wait::Drain,
            Hold::UsageLimit { resets_at } => Wait::UsageLimit { resets_at },
            Hold::Environment => Wait::Environment,
            Hold::File(file) => Wait::RepositoryFile(file),
        }
    }
}

/// The holds in force at a poll.
#[derive(Clone, Debug)]
pub struct InForce<'a> {
    drain: bool,
    usage_limit: Option<Timestamp>,
    environment: bool,
    unusable: &'a [Unusable],
}

impl<'a> InForce<'a> {
    /// The holds in force at `now`, by `memory` and the files of the
    /// repository's Linewright cannot use, `unusable`.
    pub fn of(memory: &Memory, now: Timestamp, unusable: &'a [Unusable]) -> InForce<'a> {
        InForce {
            drain: memory.drain.is_some(),
            usage_limit: memory
                .usage_limit
                .filter(|_| memory.holds_starts(now))
                .map(|limit| limit.resets_at),
            environment: memory.awaits_environment,
            unusable,
        }
    }

    /// Every hold over `run`, in rank order, the files in the order of
    /// `unusable`.
    pub fn holding(&self, run: &RunKind) -> Vec<Hold> {
        let mut holds: Vec<Hold> = [
            self.drain.then_some(Hold::Drain),
            self.usage_limit
                .map(|resets_at| Hold::UsageLimit { resets_at }),
            self.environment.then_some(Hold::Environment),
        ]
        .into_iter()
        .flatten()
        .chain(
            repo_files::holding(self.unusable, run)
                .into_iter()
                .map(|file| Hold::File(file.clone())),
        )
        .collect();
        holds.sort_by(Hold::cmp_rank);
        holds
    }

    /// The hold over every claim, if one is in force.
    pub fn holds_claims(&self) -> Option<Hold> {
        self.drain.then_some(Hold::Drain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first column of each row of the spec's Holds table.
    #[track_caller]
    fn spec_rows() -> Vec<String> {
        let runs = include_str!("../docs/specs/runs.md");
        let section = runs
            .split("\n## Holds\n")
            .nth(1)
            .expect("runs.md has a Holds section");
        section
            .lines()
            .skip_while(|line| !line.starts_with('|'))
            .take_while(|line| line.starts_with('|'))
            .skip(2)
            .map(|line| line.split('|').nth(1).unwrap().trim().to_string())
            .collect()
    }

    #[test_log::test]
    fn holds_rank_as_the_spec_lists_them() {
        let file = Hold::File(FileError {
            path: ".linewright/config.toml".into(),
            error: "invalid".into(),
        });
        let mut holds = [
            (file, "a file"),
            (Hold::Environment, "the environment"),
            (
                Hold::UsageLimit {
                    resets_at: "2026-11-03T14:30:00Z".parse().unwrap(),
                },
                "the usage limit",
            ),
            (Hold::Drain, "a drain"),
        ];
        holds.sort();
        let ranked: Vec<&str> = holds.iter().map(|(_, row)| *row).collect();
        assert_eq!(ranked, spec_rows());
    }
}
