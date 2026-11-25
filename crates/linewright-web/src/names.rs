//! How a page names items: by their item name where Linewright knows the
//! title (`docs/specs/components.md`, Item name), and in a card's line by
//! number alone (`docs/specs/board.md`, Cards, Its line).

use linewright_core::domain::{ItemKind, Number, Snapshot};

use super::components::{Fit, ItemName, ItemNumber};
use super::html::{self, Html};

/// How a page names the items of its phrases, in the repo `repo`.
#[derive(Clone, Copy)]
pub(crate) enum Naming<'a> {
    /// As a card's line names them: by number alone, a pull request's with
    /// its icon.
    CardLine { repo: &'a str },
    /// By their item name where `snapshot`, the snapshot read if any, holds
    /// the title, and by number alone otherwise.
    Titled {
        repo: &'a str,
        snapshot: Option<&'a Snapshot>,
    },
}

impl<'a> Naming<'a> {
    /// Item `number`, of `kind`: its number alone, linking to it on GitHub,
    /// with a pull request's icon before it in a card's line; or its item
    /// name, wrapping as the text around it does, where `Titled`'s snapshot
    /// holds its title.
    pub(crate) fn item(&self, number: Number, kind: ItemKind) -> Html {
        self.item_fitting(number, kind, Fit::Wrap)
    }

    /// Item `number` as [`Naming::item`] names it, but an item name fitting
    /// its line as `fit` says.
    pub(crate) fn item_fitting(&self, number: Number, kind: ItemKind, fit: Fit) -> Html {
        match *self {
            Naming::CardLine { repo } => html::rendered(&ItemNumber {
                number,
                repo,
                kind: Some(kind),
            }),
            Naming::Titled { repo, snapshot } => {
                match snapshot.and_then(|snapshot| snapshot.title(number)) {
                    Some(title) => html::rendered(&ItemName {
                        number,
                        title,
                        repo,
                        fit,
                    }),
                    None => html::rendered(&ItemNumber {
                        number,
                        repo,
                        kind: None,
                    }),
                }
            }
        }
    }

    pub(crate) fn repo(&self) -> &'a str {
        match *self {
            Naming::CardLine { repo } | Naming::Titled { repo, .. } => repo,
        }
    }
}
