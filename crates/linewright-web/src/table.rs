//! A table in its card (`docs/specs/components.md`, Table), which the
//! `table_card` macro draws.

/// What a column holds, as far as it decides how the column aligns.
#[derive(Clone, Copy)]
pub(super) enum Column {
    Money,
    Duration,
    Other,
}

/// How a column's head and cells align.
pub(super) enum Align {
    Start,
    End,
}

impl Align {
    /// Amounts of money and durations align right, every other column
    /// left.
    pub(super) fn of(column: Column) -> Align {
        match column {
            Column::Money | Column::Duration => Align::End,
            Column::Other => Align::Start,
        }
    }

    /// The class that aligns a head or a cell so.
    pub(super) fn class(&self) -> &'static str {
        match self {
            Align::Start => "text-left",
            Align::End => "text-right",
        }
    }
}

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
