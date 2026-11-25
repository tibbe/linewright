//! The action that shows the board (`docs/specs/board.md`, Board), and what
//! it shows: the model that the page renders and the tool returns.

use schemars::JsonSchema;
use serde::Serialize;

use crate::access::{Permit, Read};
use crate::api::{self, Sources};
use crate::board::{self, Board, Card, CardStatus, Column, LaneHead, LaneHeading};
use crate::domain::{Login, Number, Timestamp};
use crate::managed::Tickets;
use crate::phrase::Phrase;

/// The board as its screen shows it (`docs/specs/board.md`, Board), its
/// lanes narrowed to the person chosen, if one is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct BoardScreen {
    /// The count of cards in Needs a person on the whole board, however it
    /// is narrowed.
    pub needing_a_person: usize,
    /// The person the board is narrowed to (`docs/specs/board.md`,
    /// Filtering by person).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person: Option<Login>,
    /// The people to choose from: each person some card on the whole board
    /// matches, and whoever is chosen, by login.
    pub people: Vec<Login>,
    /// The unfinished lanes, latest changed first, then the finished ones,
    /// latest finished first.
    pub lanes: Vec<LaneScreen>,
}

/// A lane: its head, and its cards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct LaneScreen {
    #[serde(flatten)]
    pub head: LaneHeadScreen,
    /// By column, left to right, and ascending by number in each.
    pub cards: Vec<CardScreen>,
}

/// What a lane's head shows (`docs/specs/board.md`, Lanes): the tracking
/// issue with its tickets closed of total, or the item worked as one; and
/// when the lane finished, if it did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct LaneHeadScreen {
    pub number: Number,
    pub title: String,
    /// Whether it is a pull request; a tracking issue never is.
    pub pull_request: bool,
    /// The tracking issue's tickets, for work cut into tickets.
    #[serde(flatten)]
    pub tickets: Option<Tickets>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished: Option<Timestamp>,
}

impl LaneHeadScreen {
    pub fn of(heading: &LaneHeading) -> LaneHeadScreen {
        let (number, title, pull_request, tickets) = match &heading.head {
            LaneHead::Tracking {
                number,
                title,
                tickets_closed,
                tickets,
                ..
            } => (
                *number,
                title,
                false,
                Some(Tickets {
                    tickets_closed: *tickets_closed,
                    tickets: *tickets,
                }),
            ),
            LaneHead::Item {
                number,
                title,
                pull_request,
            } => (*number, title, *pull_request, None),
        };
        LaneHeadScreen {
            number,
            title: title.clone(),
            pull_request,
            tickets,
            finished: heading.finished,
        }
    }
}

/// A card (`docs/specs/board.md`, Cards).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CardScreen {
    pub number: Number,
    pub title: String,
    pub pull_request: bool,
    pub column: Column,
    /// The line under its title, `None` for the title alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<Phrase>,
    /// Its card status, that of its line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<CardStatus>,
}

impl CardScreen {
    pub fn of(card: &Card, column: Column) -> CardScreen {
        CardScreen {
            number: card.number,
            title: card.title.clone(),
            pull_request: card.pull_request,
            column,
            line: board::card_line(card),
            status: card.status(),
        }
    }
}

impl BoardScreen {
    /// `board` as its screen shows it, narrowed to `person`, if set.
    pub fn of(board: &Board, person: Option<&Login>) -> BoardScreen {
        let narrowed = person.map(|person| board.narrowed(person));
        let mut people = board.people();
        if let Some(person) = person
            && !people.contains(person)
        {
            people.push(person.clone());
            people.sort();
        }
        let lanes = narrowed
            .as_ref()
            .unwrap_or(board)
            .lanes
            .iter()
            .map(|lane| LaneScreen {
                head: LaneHeadScreen::of(&lane.heading()),
                cards: Column::ALL
                    .into_iter()
                    .flat_map(|column| {
                        lane.cards(column)
                            .iter()
                            .map(move |card| CardScreen::of(card, column))
                    })
                    .collect(),
            })
            .collect();
        BoardScreen {
            needing_a_person: board.needing_a_person,
            person: person.cloned(),
            people,
            lanes,
        }
    }
}

/// The person the Person filter `given` chooses, none where it is empty or
/// blank (`docs/specs/board.md`, Filtering by person).
pub fn chosen_person(given: &str) -> Option<Login> {
    let given = given.trim();
    (!given.is_empty()).then(|| Login::new(given))
}

/// The board as its screen shows it now, narrowed to `person`, if set; an
/// empty one before the first poll.
pub async fn get_board(_: &Permit<Read>, sources: &Sources, person: Option<&Login>) -> BoardScreen {
    BoardScreen::of(&api::board_answer(sources).await, person)
}
