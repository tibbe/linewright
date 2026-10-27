//! A page's facts about one thing (`docs/specs/design_system.md`, Facts),
//! which `ui::facts` draws (`docs/specs/web/design_system.md`, Facts).

use crate::phrase::Tone;

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
    /// A status: its icon and words in its colour, nothing behind them.
    Status {
        tone: Tone,
        words: Words,
    },
    /// An item as its card is (`docs/specs/design_system.md`, The item page,
    /// Related items): its card status's icon, if it has one, then its
    /// heading, with the pull request icon where it is one.
    Item {
        tone: Option<Tone>,
        pull_request: bool,
        heading: Words,
    },
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
                            Piece::Status { tone, words } => Piece::Status {
                                tone,
                                words: draw(words),
                            },
                            Piece::Item {
                                tone,
                                pull_request,
                                heading,
                            } => Piece::Item {
                                tone,
                                pull_request,
                                heading: draw(heading),
                            },
                        })
                        .collect()
                })
                .collect(),
        }
    }
}
