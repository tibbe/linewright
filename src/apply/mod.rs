//! The decisions of applying a session's result: given the result, what
//! Linewright read and the results of earlier writes, the content of the
//! next writes. The run (`crate::run`) makes the writes. Nothing here makes
//! I/O, reads a clock or logs.

pub mod filing;
pub mod hand_back;
pub mod pull_work;
pub mod triage;

use crate::domain::{Number, StatusLabel, closing_references};

/// The writes that change what an item asks for, which a run makes last
/// (docs/specs/repository.md, Writes, "Order").
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Asks {
    pub designed: Option<Designed>,
    /// Label the item [`crate::domain::label::STALLED`].
    pub stall: bool,
    pub status: Option<StatusTo>,
    /// Unassign Linewright from the item.
    pub unassign: bool,
}

/// Whether the item is to carry [`crate::domain::label::DESIGNED`].
#[derive(Debug, PartialEq, Eq)]
pub enum Designed {
    Add,
    Remove,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StatusTo {
    /// Hold this status label and no other.
    Set(StatusLabel),
    /// Hold no status label, as a tracking issue does.
    Clear,
}

/// `body` with a `Closes` line for each of `closes` it does not reference.
pub(crate) fn closing(body: &str, closes: &[Number]) -> String {
    let referenced = closing_references(body);
    let missing: Vec<String> = closes
        .iter()
        .filter(|issue| !referenced.contains(issue))
        .map(|issue| format!("Closes {issue}"))
        .collect();
    if missing.is_empty() {
        body.trim().to_string()
    } else {
        format!("{}\n\n{}", body.trim(), missing.join("\n"))
    }
}
