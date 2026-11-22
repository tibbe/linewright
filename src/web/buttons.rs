//! What a button is (`docs/specs/design_system.md`, Words, Buttons;
//! `docs/specs/web/design_system.md`, Buttons, Making and Icons), and the
//! buttons a screen gives for itself, which the shell draws
//! (`docs/specs/shell.md`, The screen's buttons).

use super::html::{self, Html};
use super::icons::{Icon, IconSize};

/// A verb a button acts by, with its label and its icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verb {
    /// "Cancel" that cancels work.
    CancelWork,
    AddTask,
    Remove,
    Create,
    RunBenchmark,
    AddConfiguration,
    KeepRunning,
    Edit,
    Save,
    /// "Cancel" that closes a dialog.
    CloseDialog,
}

impl Verb {
    pub(super) fn label(self) -> &'static str {
        match self {
            Verb::CancelWork | Verb::CloseDialog => "Cancel",
            Verb::AddTask => "Add task",
            Verb::Remove => "Remove",
            Verb::Create => "Create",
            Verb::RunBenchmark => "Run benchmark",
            Verb::AddConfiguration => "Add a configuration",
            Verb::KeepRunning => "Keep running",
            Verb::Edit => "Edit",
            Verb::Save => "Save",
        }
    }

    /// The icon before its label.
    pub(super) fn icon(self) -> Icon {
        match self {
            Verb::CancelWork => Icon::StopCircle,
            Verb::AddTask | Verb::Create | Verb::AddConfiguration => Icon::Plus,
            Verb::Remove => Icon::Trash,
            Verb::RunBenchmark | Verb::KeepRunning => Icon::Play,
            Verb::Edit => Icon::PencilSquare,
            Verb::Save => Icon::Check,
            Verb::CloseDialog => Icon::XMark,
        }
    }

    /// What its button holds: its label after its icon.
    pub(super) fn labelled(self) -> Html {
        self.icon()
            .svg(IconSize::Inline)
            .then(&html::escape(self.label()))
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
