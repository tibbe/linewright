//! A screen's facts side by side, as a daisyUI `stats`
//! (`docs/specs/benchmarks.md`, Benchmark run and Task).

use super::html::{self, Html};

/// A fact: its label, and its value, [`ABSENT`] where the value is absent.
pub(super) struct Stat {
    pub(super) label: &'static str,
    pub(super) value: Html,
}

/// What a screen shows of a figure that is absent
/// (`docs/specs/design_system.md`, Figures, Absent).
pub(super) const ABSENT: &str = "–";

/// [`ABSENT`], as a value.
pub(super) fn absent() -> Html {
    html::escape(ABSENT)
}
