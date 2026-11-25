//! A table in its card (`docs/specs/components.md`, Table), which the
//! `table_card` macro draws.

use askama::Template;

/// How a column's head and cells align (`docs/specs/components.md`, Table,
/// Columns).
pub(super) enum Align {
    Start,
    End,
}

impl Align {
    /// The class that aligns a head or a cell so.
    pub(super) fn class(&self) -> &'static str {
        match self {
            Align::Start => "text-left",
            Align::End => "text-right",
        }
    }
}

/// A table's rows, or that it has none (`docs/specs/components.md`, Table,
/// Empty).
pub(super) enum TableRows<R> {
    Empty(NoRows),
    Rows(Rows<R>),
}

/// At least one row.
pub(super) struct Rows<R>(Vec<R>);

impl<R> TableRows<R> {
    /// The rows `rows`, or, where there are none, "No" and `what` they are,
    /// in the plural, such as "tasks".
    pub(super) fn of(rows: Vec<R>, what: &'static str) -> TableRows<R> {
        if rows.is_empty() {
            TableRows::Empty(NoRows { what })
        } else {
            TableRows::Rows(Rows(rows))
        }
    }

    /// The same rows, or none, each made into `row(it)`.
    pub(super) fn map<'a, S>(&'a self, row: impl FnMut(&'a R) -> S) -> TableRows<S> {
        match self {
            TableRows::Empty(no_rows) => TableRows::Empty(*no_rows),
            TableRows::Rows(Rows(rows)) => TableRows::Rows(Rows(rows.iter().map(row).collect())),
        }
    }
}

impl<'a, R> IntoIterator for &'a Rows<R> {
    type Item = &'a R;
    type IntoIter = std::slice::Iter<'a, R>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// What a table with no rows shows in place of its `table`
/// (`docs/specs/components.md`, Table, Empty).
#[derive(Clone, Copy, Template)]
#[template(source = r#"<p class="px-4 pb-4">No {{ what }}</p>"#, ext = "html")]
pub(super) struct NoRows {
    /// What the rows are, in the plural, such as "tasks".
    what: &'static str,
}

impl askama::filters::HtmlSafe for NoRows {}

/// The part of a table card that the `table_card` macro asks its caller to
/// draw.
#[derive(PartialEq)]
pub(super) enum Part {
    /// The filters' fields, in the row under the card's top.
    Filters,
    /// The buttons, at the right of the card's top.
    Buttons,
    /// The table, or what shows in its place.
    Table,
}

/// A row's "View" (`docs/specs/components.md`, Table, A row's screen),
/// linking to the screen of what the row stands for, which the `row_view`
/// macro draws.
pub(super) struct View {
    pub(super) href: String,
    /// The row's name, which a screen reader reads after "View", such as
    /// "run 148/implement-1".
    pub(super) name: String,
}

/// What a row opens to in place (`docs/specs/benchmarks.md`, Benchmark run,
/// Opening in place), which the `fold_cell` macro draws in the row's leading
/// cell.
pub(super) enum Fold {
    Opens(Opens),
    Nothing,
}

/// The rows a row opens to, at least one, and what its button's accessible
/// name says it opens.
pub(super) struct Opens {
    rows: Vec<String>,
    label: String,
}

impl Fold {
    /// A row opening to the rows `rows`, their ids, by a button whose
    /// accessible name is `label`, such as "Show the tasks of opus-high";
    /// `Nothing` where `rows` is empty.
    pub(super) fn of(rows: Vec<String>, label: String) -> Fold {
        if rows.is_empty() {
            Fold::Nothing
        } else {
            Fold::Opens(Opens { rows, label })
        }
    }
}

impl Opens {
    /// The ids of the rows it opens, at least one.
    pub(super) fn rows(&self) -> &[String] {
        &self.rows
    }

    pub(super) fn label(&self) -> &str {
        &self.label
    }
}
