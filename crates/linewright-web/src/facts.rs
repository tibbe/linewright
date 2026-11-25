//! A page's facts about one thing (`docs/specs/components.md`, Facts).

use linewright_core::domain::{ItemKind, Number};
use linewright_core::phrase::Status;

use super::html::Html;

/// A fact: its label, and its value a line at a time. `Words` is what its
/// words are made of, `Html` as the page draws them.
pub(super) struct Fact<Words = Html> {
    pub label: &'static str,
    pub lines: Vec<Vec<Piece<Words>>>,
}

/// A piece of a fact's line.
pub(super) enum Piece<Words = Html> {
    Text(Words),
    /// A status: a dot in its colour, then its icon and words.
    Status {
        status: Status,
        words: Words,
    },
    /// The items a fact lists, the whole of its line
    /// (`docs/specs/board.md`, Item, Related items).
    Items(Vec<RelatedItem>),
    /// Markup of its own, such as a build of the environment's.
    Markup(Html),
}

/// An item a fact lists (`docs/specs/board.md`, Item, Related items), in
/// the repo `repo`.
pub(super) enum RelatedItem {
    /// An item that Linewright has read, on one line: the icon of an issue
    /// or a pull request, then its card status's icon, if its card shows
    /// one, in the status's colour, then its title, cut to fit its line,
    /// and its number. A tracking issue has a `rest`, the rest of its
    /// lane's head after them, where the head has more.
    Read {
        kind: ItemKind,
        status: Option<Status>,
        number: Number,
        title: String,
        repo: String,
        rest: Option<Html>,
    },
    /// An item that Linewright has not read, named by its number alone.
    Unread { number: Number, repo: String },
}

impl<Words> Fact<Words> {
    /// The fact with its words drawn by `draw`, piece by piece.
    pub(super) fn map<Drawn>(self, mut draw: impl FnMut(Words) -> Drawn) -> Fact<Drawn> {
        Fact {
            label: self.label,
            lines: self
                .lines
                .into_iter()
                .map(|line| {
                    line.into_iter()
                        .map(|piece| match piece {
                            Piece::Text(words) => Piece::Text(draw(words)),
                            Piece::Status { status, words } => Piece::Status {
                                status,
                                words: draw(words),
                            },
                            Piece::Items(items) => Piece::Items(items),
                            Piece::Markup(markup) => Piece::Markup(markup),
                        })
                        .collect()
                })
                .collect(),
        }
    }
}
