//! What a button is (`docs/specs/design_system.md`, Words, Buttons;
//! `docs/specs/web/design_system.md`, Buttons, Making and Icons), and the
//! buttons a screen gives for itself, which the shell draws
//! (`docs/specs/shell.md`, The screen's buttons).

use super::html::{self, Html};
use super::icons::{Icon, IconSize};
use crate::actions::Action;

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
    SignOut,
    Disable,
    Enable,
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
            Verb::SignOut => "Sign out",
            Verb::Disable => "Disable",
            Verb::Enable => "Enable",
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
            Verb::SignOut => Icon::ArrowRightStartOnRectangle,
            Verb::Disable => Icon::LockClosed,
            Verb::Enable => Icon::LockOpen,
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
    /// "New", a `btn-primary`, with no icon, for `action`.
    New {
        dialog: &'static str,
        action: Action,
    },
    /// A verb's label after its icon, for `action`.
    Act {
        verb: Verb,
        dialog: String,
        action: Action,
    },
}

impl ScreenButton {
    /// The action it is for, which a reader's role must allow for the
    /// screen to show it (`docs/specs/access.md`, Actions).
    pub(super) fn action(&self) -> Action {
        match self {
            ScreenButton::New { action, .. } | ScreenButton::Act { action, .. } => *action,
        }
    }
}
