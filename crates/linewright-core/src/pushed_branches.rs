//! Linewright's record of the ticket branches it pushed, so that a poll
//! reads those and not every branch of the repository
//! (`docs/specs/sources_of_work.md`, Polling).

use std::collections::BTreeSet;

use anyhow::{Context as _, Result};

use crate::database::{Database, time_text};
use crate::domain::{TicketBranchName, Timestamp};

/// The record, kept in the database: every handle on one database sees the
/// same record.
#[derive(Clone, Debug)]
pub struct PushedBranches {
    db: Database,
}

/// The row of `adopted` that says the branches pushed before the record
/// began are in it.
const ADOPTED: &str = "pushed-branches";

impl PushedBranches {
    pub fn new(db: Database) -> PushedBranches {
        PushedBranches { db }
    }

    /// Linewright pushed `branch` at `at`. A branch recorded already counts
    /// as recorded at `at` from then on.
    pub async fn record(&self, branch: &TicketBranchName, at: Timestamp) -> Result<()> {
        sqlx::query(
            "INSERT INTO pushed_branches (name, recorded_at) VALUES (?, ?)
             ON CONFLICT (name) DO UPDATE SET recorded_at = excluded.recorded_at",
        )
        .bind(branch.as_str())
        .bind(time_text(at))
        .execute(self.db.pool())
        .await
        .with_context(|| format!("recording that Linewright pushed {}", branch.as_str()))?;
        Ok(())
    }

    /// Every branch recorded.
    pub async fn names(&self) -> Result<BTreeSet<TicketBranchName>> {
        let names: Vec<String> = sqlx::query_scalar("SELECT name FROM pushed_branches")
            .fetch_all(self.db.pool())
            .await
            .context(READING)?;
        names
            .into_iter()
            .map(|name| TicketBranchName::try_from(name).map_err(anyhow::Error::msg))
            .collect::<Result<_>>()
            .context(READING)
    }

    /// Forgets `branch`, such as once Linewright deleted it.
    pub async fn forget(&self, branch: &TicketBranchName) -> Result<()> {
        sqlx::query("DELETE FROM pushed_branches WHERE name = ?")
            .bind(branch.as_str())
            .execute(self.db.pool())
            .await
            .with_context(|| format!("forgetting that Linewright pushed {}", branch.as_str()))?;
        Ok(())
    }

    /// Forgets each branch of `gone`, which a read that began at
    /// `read_started` found the repository does not have, unless it was
    /// recorded at or after that: a branch pushed during the read may be one
    /// the read missed.
    pub async fn forget_gone(
        &self,
        gone: &BTreeSet<TicketBranchName>,
        read_started: Timestamp,
    ) -> Result<()> {
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .context(FORGETTING)?;
        for branch in gone {
            sqlx::query("DELETE FROM pushed_branches WHERE name = ? AND recorded_at < ?")
                .bind(branch.as_str())
                .bind(time_text(read_started))
                .execute(&mut *tx)
                .await
                .context(FORGETTING)?;
        }
        tx.commit().await.context(FORGETTING)
    }

    /// Whether the branches pushed before the record began were adopted.
    pub async fn adopted(&self) -> Result<bool> {
        let adopted: Option<String> =
            sqlx::query_scalar("SELECT record FROM adopted WHERE record = ?")
                .bind(ADOPTED)
                .fetch_optional(self.db.pool())
                .await
                .context("reading whether Linewright's pushed branches were adopted")?;
        Ok(adopted.is_some())
    }

    /// Records `branches`, which a listing that began at `listed_at` found,
    /// as pushed then, unless they were adopted already: then it records
    /// nothing. A branch recorded already keeps the time it was recorded at.
    pub async fn adopt(
        &self,
        branches: &BTreeSet<TicketBranchName>,
        listed_at: Timestamp,
    ) -> Result<()> {
        let mut tx = self
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .context(ADOPTING)?;
        let marked = sqlx::query("INSERT OR IGNORE INTO adopted (record) VALUES (?)")
            .bind(ADOPTED)
            .execute(&mut *tx)
            .await
            .context(ADOPTING)?;
        if marked.rows_affected() == 0 {
            return Ok(());
        }
        for branch in branches {
            sqlx::query("INSERT OR IGNORE INTO pushed_branches (name, recorded_at) VALUES (?, ?)")
                .bind(branch.as_str())
                .bind(time_text(listed_at))
                .execute(&mut *tx)
                .await
                .context(ADOPTING)?;
        }
        tx.commit().await.context(ADOPTING)
    }
}

const READING: &str = "reading the branches Linewright pushed";

const FORGETTING: &str = "forgetting the branches Linewright pushed that are gone";

const ADOPTING: &str = "adopting the branches Linewright pushed";
