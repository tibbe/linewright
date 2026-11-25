//! How a page names the items of its phrases: by their title and number
//! where Linewright knows the title (`docs/specs/board.md`, Item, Items in
//! Waiting for and in what keeps #N out), and in a card's line by number
//! alone (`docs/specs/board.md`, Cards, Its line).

use linewright_core::domain::{ItemKind, Number, Snapshot};

use super::html::{self, Html};
use super::item_parts::{CardLineNumber, ItemNumberLink, TextItem, Weight};

/// How a page names the items of its phrases, in the repo `repo`.
#[derive(Clone, Copy)]
pub(crate) enum Naming<'a> {
    /// As a card's line names them: by number alone, a pull request's with
    /// its icon.
    CardLine { repo: &'a str },
    /// By their title and number where `snapshot`, the snapshot read if
    /// any, holds the title, and by number alone otherwise.
    Titled {
        repo: &'a str,
        snapshot: Option<&'a Snapshot>,
    },
}

impl<'a> Naming<'a> {
    /// Item `number`, of `kind`: its number alone, linking to it on GitHub,
    /// with a pull request's icon before it in a card's line; or as
    /// [`titled_item`] names it, from `Titled`'s repo and snapshot.
    pub(crate) fn item(&self, number: Number, kind: ItemKind) -> Html {
        match *self {
            Naming::CardLine { repo } => html::rendered(&CardLineNumber { number, repo, kind }),
            Naming::Titled { repo, snapshot } => titled_item(repo, snapshot, number),
        }
    }

    /// Item `number`'s title, where `Titled`'s snapshot holds it.
    pub(crate) fn title(&self, number: Number) -> Option<&'a str> {
        match *self {
            Naming::CardLine { .. } => None,
            Naming::Titled { snapshot, .. } => snapshot.and_then(|snapshot| snapshot.title(number)),
        }
    }

    pub(crate) fn repo(&self) -> &'a str {
        match *self {
            Naming::CardLine { repo } | Naming::Titled { repo, .. } => repo,
        }
    }
}

/// Item `number` of the repo `repo`: its title, then a space, then its
/// number, as one text wrapping as the text around it does, where
/// `snapshot`, the snapshot read if any, holds its title; or else its
/// number alone, linking to it on GitHub.
pub(crate) fn titled_item(repo: &str, snapshot: Option<&Snapshot>, number: Number) -> Html {
    match snapshot.and_then(|snapshot| snapshot.title(number)) {
        Some(title) => html::rendered(&TextItem::of(number, title, repo, Weight::Inherit)),
        None => html::rendered(&ItemNumberLink::alone(number, repo)),
    }
}
