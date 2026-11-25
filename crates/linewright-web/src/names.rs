//! How a page names the items of its phrases: by their title and number
//! where Linewright knows the title (`docs/specs/board.md`, Item, Items in
//! Waiting for and in what keeps #N out), and in a card's line by number
//! alone (`docs/specs/board.md`, Cards, Its line).

use linewright_core::domain::{ItemKind, Number, Snapshot};

use super::html::{self, Html};
use super::item_parts::{CardLineNumber, InlineItem, ItemNumberLink};

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
    /// with a pull request's icon before it in a card's line; or its title,
    /// wrapping as the text around it does, then its number, where
    /// `Titled`'s snapshot holds its title.
    pub(crate) fn item(&self, number: Number, kind: ItemKind) -> Html {
        match *self {
            Naming::CardLine { repo } => html::rendered(&CardLineNumber { number, repo, kind }),
            Naming::Titled { repo, .. } => match self.title(number) {
                Some(title) => html::rendered(&InlineItem::of(number, title, repo)),
                None => html::rendered(&ItemNumberLink::alone(number, repo)),
            },
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
