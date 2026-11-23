//! Where a setting or a brief file comes from (`GLOSSARY.md`, **Layer**).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The sources a setting or a brief file can come from, first the one that
/// wins.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    /// A trial's configuration (`docs/specs/benchmarks.md`,
    /// Configurations).
    Configuration,
    /// The repository's files, such as an automation's front matter.
    Repository,
    /// The operator's flags, environment variables, config file and briefs.
    Operator,
    /// Linewright's defaults and the briefs it ships.
    Linewright,
}

impl Layer {
    /// Whose a setting or a brief file is, coming from this layer.
    pub fn whose(self) -> &'static str {
        match self {
            Layer::Configuration => "the configuration's",
            Layer::Repository => "the repository's",
            Layer::Operator => "the operator's",
            Layer::Linewright => "Linewright's own",
        }
    }
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

/// A list setting's entries across its layers, given highest first: every
/// layer's entries, highest first, without repeats, an entry being the same
/// as another when their `key`s are (`docs/specs/operation.md`,
/// Configuration, Combining layers).
pub fn list<T, K: PartialEq>(
    layers: impl IntoIterator<Item = impl IntoIterator<Item = T>>,
    key: impl Fn(&T) -> &K,
) -> Vec<T> {
    let mut entries: Vec<T> = Vec::new();
    for entry in layers.into_iter().flatten() {
        if !entries.iter().any(|kept| key(kept) == key(&entry)) {
            entries.push(entry);
        }
    }
    entries
}

/// A table setting's names across its layers, given highest first: every
/// layer's names, each with the highest layer's value
/// (`docs/specs/operation.md`, Configuration, Combining layers).
pub fn table<K: Ord, V>(
    layers: impl IntoIterator<Item = impl IntoIterator<Item = (K, V)>>,
) -> BTreeMap<K, V> {
    let mut names = BTreeMap::new();
    for (name, value) in layers.into_iter().flatten() {
        names.entry(name).or_insert(value);
    }
    names
}
