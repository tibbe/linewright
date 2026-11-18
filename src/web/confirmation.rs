//! A confirmation (`docs/specs/design_system.md`, Confirming): an action
//! that asks first, in a daisyUI `modal` over the page
//! (`docs/specs/web/design_system.md`, Navigation), drawn by the `confirm`
//! macro.

/// An action that asks before it acts.
pub(super) struct Confirmation {
    /// Its dialog's id, unique on the page and without `_`, which the id of
    /// its question adds.
    pub(super) id: String,
    /// The label of the button that asks, and of the button that goes ahead.
    pub(super) label: &'static str,
    /// What it asks, naming what it acts on and saying what follows.
    pub(super) question: String,
    /// The button that turns it down.
    pub(super) turn_down: TurnDown,
    /// The path going ahead posts to.
    pub(super) action: String,
    /// What going ahead posts, each a name and its value.
    pub(super) fields: Vec<(&'static str, String)>,
    /// The id of where the page says what came of going ahead.
    pub(super) said: &'static str,
}

/// The button that turns a confirmation down.
pub(super) enum TurnDown {
    /// For any other action.
    Cancel,
    /// For an action that cancels.
    KeepRunning,
}

impl TurnDown {
    /// Its button's label.
    pub(super) fn label(&self) -> &'static str {
        match self {
            TurnDown::Cancel => "Cancel",
            TurnDown::KeepRunning => "Keep running",
        }
    }
}
