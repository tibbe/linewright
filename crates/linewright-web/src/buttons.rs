//! What a button is (`docs/specs/design_system.md`, Words, Buttons;
//! `docs/specs/components.md`, Button and Form dialog;
//! `docs/specs/design_system.md`, Buttons, Icons), and the buttons a screen
//! gives for itself, which the shell draws (`docs/specs/shell.md`, The screen's
//! buttons).

use linewright_core::actions::Action;

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
    SignOut,
    Disable,
    Enable,
    Allow,
    Deny,
    /// "End" that ends an authorization.
    End,
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
            Verb::Allow => "Allow",
            Verb::Deny => "Deny",
            Verb::End => "End",
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
            Verb::Save | Verb::Allow => Icon::Check,
            Verb::CloseDialog | Verb::Deny => Icon::XMark,
            Verb::SignOut => Icon::ArrowRightStartOnRectangle,
            Verb::Disable => Icon::LockClosed,
            Verb::Enable => Icon::LockOpen,
            Verb::End => Icon::LinkSlash,
        }
    }

    /// What its button holds: its label after its icon.
    pub(super) fn labelled(self) -> Html {
        self.labelled_in(IconSize::Button)
    }

    /// Its label after its icon, sized for where it sits, such as an item
    /// of a `menu` that is a `button` but not a `btn`.
    pub(super) fn labelled_in(self, size: IconSize) -> Html {
        self.icon().svg(size).then(&html::escape(self.label()))
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
