//! Linewright's record of what it wrote to each open pull request that
//! GitHub does not keep: when it last commented naming what GitHub requires
//! to merge it, and its failed merges (`docs/specs/pull_requests.md`,
//! Working a pull request).

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context as _, Result};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row as _, SqliteConnection};

use crate::database::{Database, parse_time, time_text};
use crate::domain::{Login, Mergeability, Number, PullState, Snapshot, Timestamp};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullRecords(BTreeMap<Number, PullRecord>);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PullRecord {
    commented: Option<Timestamp>,
    /// Oldest first.
    failed_merges: Vec<FailedMerge>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

    /// `None` until the record is first kept.
    pub async fn load(db: &Database) -> Result<Option<PullRecords>> {
        let mut tx = db.pool().begin().await.context(READING)?;
        let pool = &mut *tx;
        let kept: Option<String> = sqlx::query_scalar("SELECT record FROM kept WHERE record = ?")
            .bind(KEPT)
            .fetch_optional(&mut *pool)
            .await
            .context("reading whether the record of Linewright's pull requests is kept")?;
        if kept.is_none() {
            return Ok(None);
        }
        let mut records = PullRecords::default();
        let commented = sqlx::query("SELECT pr, commented_at FROM pull_records")
            .fetch_all(&mut *pool)
            .await
            .context(READING)?;
        for row in commented {
            let record = records.0.entry(pr_of(&row)?).or_default();
            record.commented = row
                .try_get::<Option<&str>, _>("commented_at")?
                .map(parse_time)
                .transpose()?;
        }
        let failed = sqlx::query("SELECT pr, reason, at FROM failed_merges ORDER BY pr, seq")
            .fetch_all(&mut *pool)
            .await
            .context("reading Linewright's failed merges")?;
        for row in failed {
            records
                .0
                .entry(pr_of(&row)?)
                .or_default()
                .failed_merges
                .push(FailedMerge {
                    reason: row.try_get("reason")?,
                    at: parse_time(row.try_get("at")?)?,
                });
        }
        Ok(Some(records))
    }

    /// Keeps this as the record, in place of the one kept, if any.
    pub async fn save(&self, db: &Database) -> Result<()> {
        let mut tx = db.pool().begin().await.context(SAVING)?;
        self.replace(&mut tx).await.context(SAVING)?;
        tx.commit().await.context(SAVING)
    }

    async fn replace(&self, conn: &mut SqliteConnection) -> Result<()> {
        sqlx::query("INSERT OR IGNORE INTO kept (record) VALUES (?)")
            .bind(KEPT)
            .execute(&mut *conn)
            .await?;
        sqlx::query("DELETE FROM pull_records")
            .execute(&mut *conn)
            .await?;
        for (pr, record) in &self.0 {
            let pr = i64::try_from(pr.0)
                .with_context(|| format!("{pr} is beyond what the database keeps"))?;
            sqlx::query("INSERT INTO pull_records (pr, commented_at) VALUES (?, ?)")
                .bind(pr)
                .bind(record.commented.map(time_text))
                .execute(&mut *conn)
                .await?;
            for (seq, failed) in (0_i64..).zip(&record.failed_merges) {
                sqlx::query("INSERT INTO failed_merges (pr, seq, reason, at) VALUES (?, ?, ?, ?)")
                    .bind(pr)
                    .bind(seq)
                    .bind(&failed.reason)
                    .bind(time_text(failed.at))
                    .execute(&mut *conn)
                    .await?;
            }
        }
        Ok(())
    }
}

const READING: &str = "reading the record of Linewright's pull requests";

const SAVING: &str = "recording Linewright's pull requests";

/// The row of `kept` that says the record is kept.
const KEPT: &str = "pull-records";

fn pr_of(row: &SqliteRow) -> Result<Number> {
    Ok(Number(u64::try_from(row.try_get::<i64, _>("pr")?)?))
}
