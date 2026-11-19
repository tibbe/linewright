//! A table in its card (`docs/specs/design_system.md`, Tables;
//! `docs/specs/web/design_system.md`, Tables), which the `table_card` macro
//! draws.

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
    /// The filters' fields, in the bar.
    Filters,
    /// The buttons, in the bar after the filters.
    Buttons,
    /// The table, or what shows in its place.
    Table,
}
