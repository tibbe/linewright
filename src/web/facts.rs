//! A page's facts about one thing (`docs/specs/design_system.md`, Facts, and
//! `docs/specs/web/components.md`, Facts).

use crate::phrase::Status;

use super::html::Html;
use super::icons::ItemKind;

/// A fact: its label, and its value a line at a time. `Words` is what its
/// words are made of, `Html` as the page draws them.
pub(super) struct Fact<Words = Html> {
    pub label: &'static str,
    pub lines: Vec<Vec<Piece<Words>>>,
}

/// A piece of a fact's line.
pub(super) enum Piece<Words = Html> {
    Text(Words),
    /// A status: its icon and words in its colour, nothing behind them.
    Status {
        status: Status,
        words: Words,
    },
    /// An item as its card is (`docs/specs/board.md`, Item,
    /// Related items): its card status's icon, if it has one, then the
    /// icon of an issue or a pull request, then its heading.
    Item {
        status: Option<Status>,
        kind: ItemKind,
        heading: Words,
    },
    /// A tracking issue as its lane's head is.
    LaneHead(Words),
    /// Markup of its own, such as a build of the environment's.
    Markup(Html),
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
                            Piece::Item {
                                status,
                                kind,
                                heading,
                            } => Piece::Item {
                                status,
                                kind,
                                heading: draw(heading),
                            },
                            Piece::LaneHead(heading) => Piece::LaneHead(draw(heading)),
                            Piece::Markup(markup) => Piece::Markup(markup),
                        })
                        .collect()
                })
                .collect(),
        }
    }
}
