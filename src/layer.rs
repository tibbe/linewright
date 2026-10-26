//! Where a setting or a prompt file comes from (`GLOSSARY.md`, **Layer**).

use serde::{Deserialize, Serialize};

/// The sources a setting or a prompt file can come from, first the one that
/// wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    /// The repository's files, such as an automation's front matter.
    Repository,
    /// The operator's flags, environment variables, config file and prompts.
    Operator,
    /// Linewright's defaults and the prompts it ships.
    Linewright,
}

impl Layer {
    /// Whose a setting or a prompt file is, coming from this layer.
    pub fn whose(self) -> &'static str {
        match self {
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

/// Where a session's setting came from: a layer, or, for a model or effort
/// none set, the harness's choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    Layer(Layer),
    Harness,
}

impl Source {
    /// Whose a setting is, coming from here.
    pub fn whose(self) -> &'static str {
        match self {
            Source::Layer(layer) => layer.whose(),
            Source::Harness => "the harness's",
        }
    }
}

const HARNESS: &str = "harness";

/// Flat, as a layer's name or `harness`.
impl Serialize for Source {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Source::Layer(layer) => layer.serialize(serializer),
            Source::Harness => serializer.serialize_str(HARNESS),
        }
    }
}

impl<'de> Deserialize<'de> for Source {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Source, D::Error> {
        let name = String::deserialize(deserializer)?;
        if name == HARNESS {
            return Ok(Source::Harness);
        }
        Layer::deserialize(serde::de::value::StrDeserializer::<D::Error>::new(&name))
            .map(Source::Layer)
    }
}
