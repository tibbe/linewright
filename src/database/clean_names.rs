//! The migration that cleans each name kept before Linewright cleaned names
//! (`docs/specs/benchmarks.md`, Names and IDs).

use std::collections::HashSet;

use anyhow::Context as _;
use futures_util::future::BoxFuture;
use sqlx::SqliteConnection;

use super::Migration;
use crate::name::Name;

pub(super) const CLEAN_NAMES: Migration = Migration::Rows(clean_names);

/// A table of names: what a row of it is called by its kind and ID, and
/// within what no two rows hold the same name.
struct Table {
    name: &'static str,
    kind: &'static str,
    /// The SQL of a row's ID as a person sees it.
    shown_id: &'static str,
    scope: Scope,
}

/// Within what no two rows of a [`Table`] hold the same name.
enum Scope {
    Table,
    /// The rows of one value of the column.
    Column(&'static str),
    /// No row but itself.
    Row,
}

impl Scope {
    /// The SQL of the scope of a row's name.
    fn sql(&self) -> &'static str {
        match self {
            Scope::Table => "0",
            Scope::Column(column) => column,
            Scope::Row => "rowid",
        }
    }
}

const TABLES: [Table; 4] = [
    Table {
        name: "benchmarks",
        kind: "Benchmark",
        shown_id: "id",
        scope: Scope::Table,
    },
    Table {
        name: "tasks",
        kind: "Task",
        shown_id: "id",
        scope: Scope::Column("benchmark"),
    },
    Table {
        name: "removed_tasks",
        kind: "Task",
        shown_id: "id",
        scope: Scope::Row,
    },
    Table {
        name: "benchmark_run_configurations",
        kind: "Configuration",
        shown_id: "seq + 1",
        scope: Scope::Column("benchmark_run"),
    },
];

/// A row of a [`Table`].
struct Row {
    rowid: i64,
    scope: i64,
    id: i64,
    name: String,
}

/// Gives each name of [`TABLES`] that is not clean the name [`cleaned`]
/// gives it.
fn clean_names(conn: &mut SqliteConnection) -> BoxFuture<'_, anyhow::Result<()>> {
    Box::pin(async move {
        for table in &TABLES {
            let reading = || format!("reading the names of {}", table.name);
            let rows: Vec<(i64, i64, i64, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT rowid, {scope}, {id}, name FROM {table} ORDER BY {id}",
                scope = table.scope.sql(),
                id = table.shown_id,
                table = table.name,
            )))
            .fetch_all(&mut *conn)
            .await
            .with_context(reading)?;
            let rows: Vec<Row> = rows
                .into_iter()
                .map(|(rowid, scope, id, name)| Row {
                    rowid,
                    scope,
                    id,
                    name,
                })
                .collect();
            for (rowid, name) in cleaned(table.kind, &rows) {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "UPDATE {} SET name = ? WHERE rowid = ?",
                    table.name
                )))
                .bind(&name)
                .bind(rowid)
                .execute(&mut *conn)
                .await
                .with_context(|| format!("cleaning a name of {}", table.name))?;
            }
        }
        Ok(())
    })
}

/// The new name of each of `rows`, in the order of their IDs, whose name is
/// not clean: its clean form, or else, when that is no name or another row
/// in its scope holds it, `kind` and its ID, with a number after them while
/// that too is held.
///
/// # Panics
///
/// Never: of the `rows.len() + 1` names by its ID, the other rows hold at
/// most `rows.len() - 1`.
fn cleaned(kind: &str, rows: &[Row]) -> Vec<(i64, String)> {
    let is_clean = |row: &Row| Name::parse(&row.name).is_ok_and(|name| name.as_str() == row.name);
    let mut taken: HashSet<(i64, String)> = rows
        .iter()
        .filter(|row| is_clean(row))
        .map(|row| (row.scope, row.name.clone()))
        .collect();
    let mut renames = vec![];
    for row in rows.iter().filter(|row| !is_clean(row)) {
        let free = |name: &String| !taken.contains(&(row.scope, name.clone()));
        let name = Name::parse(&row.name)
            .ok()
            .map(|name| name.as_str().to_string())
            .filter(free)
            .unwrap_or_else(|| {
                let by_id = format!("{kind} {}", row.id);
                std::iter::once(by_id.clone())
                    .chain((2..=rows.len() + 1).map(|n| format!("{by_id} ({n})")))
                    .find(free)
                    .expect("more names to choose from than rows that hold one")
            });
        tracing::info!(from = row.name, to = name, "cleaned a name");
        taken.insert((row.scope, name.clone()));
        renames.push((row.rowid, name));
    }
    renames
}
