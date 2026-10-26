//! Where a setting or a brief file comes from (`GLOSSARY.md`, **Layer**).

use serde::{Deserialize, Serialize};

/// The sources a setting or a brief file can come from, first the one that
/// wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    /// The repository's files, such as an automation's front matter.
    Repository,
    /// The operator's flags, environment variables, config file and briefs.
    Operator,
    /// Linewright's defaults and the briefs it ships.
    Linewright,
}

/// A value and the layer it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sourced<T> {
    pub value: T,
    pub layer: Layer,
}

impl<T> Sourced<T> {
    pub fn new(value: T, layer: Layer) -> Sourced<T> {
        Sourced { value, layer }
    }
}
