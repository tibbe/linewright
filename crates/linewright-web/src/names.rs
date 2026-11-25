//! How a page names items: by their item name where Linewright knows the
//! title (`docs/specs/components.md`, Item name), and in a card's line by
//! number alone (`docs/specs/board.md`, Cards, Its line).

use linewright_core::domain::{ItemKind, Number, Snapshot};

use super::html::{self, Html};

/// How a page names the items of its phrases, in the repo `repo`.
#[derive(Clone, Copy)]
pub(crate) enum Naming<'a> {
    /// As a card's line names them: by number alone.
    CardLine { repo: &'a str },
    /// By number and title where `snapshot`, the snapshot read if any, holds
    /// the title, and by number alone otherwise.
    Titled {
        repo: &'a str,
        snapshot: Option<&'a Snapshot>,
    },
}

impl<'a> Naming<'a> {
    /// #`number`, with its title where a snapshot of `Titled` holds it,
    /// linking to its page, and a link to it on GitHub. Its markup is the
    /// same whether `kind` is an issue or a pull request.
    pub(crate) fn item(&self, number: Number, _kind: ItemKind) -> Html {
        match *self {
            Naming::Titled {
                repo,
                snapshot: Some(snapshot),
            } => match snapshot.title(number) {
                Some(title) => html::titled(number, title, repo),
                None => html::item(number, repo),
            },
            Naming::CardLine { repo } | Naming::Titled { repo, .. } => html::item(number, repo),
        }
    }

    pub(crate) fn repo(&self) -> &'a str {
        match *self {
            Naming::CardLine { repo } | Naming::Titled { repo, .. } => repo,
        }
    }
}
