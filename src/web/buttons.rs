//! What a button is (`docs/specs/design_system.md`, Words, Buttons;
//! `docs/specs/web/design_system.md`, Buttons and Making), and the buttons
//! a screen gives for itself, which the shell draws (`docs/specs/shell.md`,
//! The screen's buttons).

/// A verb a button acts by, with its label and, through the `verb_icon`
/// macro, its icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verb {
    Cancel,
    AddTask,
    Remove,
}

impl Verb {
    pub(super) fn label(self) -> &'static str {
        match self {
            Verb::Cancel => "Cancel",
            Verb::AddTask => "Add task",
            Verb::Remove => "Remove",
        }
    }
}

/// A button among a screen's buttons. It opens its page's dialog `dialog`,
/// which the page draws outside every region a stream sends, so that an
/// update leaves the dialog and what the reader typed in it as they were.
pub(super) enum ScreenButton {
    /// "New", a `btn-primary`, with no icon.
    New { dialog: &'static str },
    /// A verb's label after its icon.
    Act { verb: Verb, dialog: String },
}
