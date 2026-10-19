//! Linewright's credentials: the variables of its environment named as one
//! (`docs/specs/architecture.md`, the redaction decision).

use std::ffi::OsString;

use crate::git::Addition;
use crate::redaction::Redaction;

const SUFFIXES: [&str; 4] = ["_TOKEN", "_KEY", "_SECRET", "_PASSWORD"];

/// A variable of Linewright's environment named as a credential, and its
/// value, which is never empty.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    name: String,
    value: String,
}

impl Credential {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credential({})", self.name)
    }
}

/// Linewright's credentials, and their redaction.
#[derive(Clone, Debug, Default)]
pub struct Credentials {
    list: Vec<Credential>,
    redaction: Redaction,
}

impl Credentials {
    /// The credentials among `vars`, Linewright's environment as
    /// [`std::env::vars_os`] gives it. Empty values are skipped, as every
    /// text holds the empty string, and so are names or values that are not
    /// UTF-8, which no text Linewright handles holds as they are.
    pub fn of(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Credentials {
        let list: Vec<Credential> = vars
            .into_iter()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .filter(|(name, value)| {
                !value.is_empty() && SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
            })
            .map(|(name, value)| Credential { name, value })
            .collect();
        Credentials {
            redaction: Redaction::of(&list),
            list,
        }
    }

    pub fn redaction(&self) -> &Redaction {
        &self.redaction
    }

    /// The first of `additions` that holds one of the credentials.
    pub fn held(&self, additions: &[Addition]) -> Option<Held> {
        additions.iter().find_map(|addition| {
            let credential = self
                .list
                .iter()
                .find(|c| addition.text.contains(c.value()))?;
            Some(Held {
                variable: credential.name.clone(),
                file: addition.file.clone(),
                commit: addition.commit.clone(),
            })
        })
    }
}

/// A credential a commit adds to a file, named but not given.
#[derive(Debug, PartialEq, Eq)]
pub struct Held {
    pub variable: String,
    pub file: String,
    /// The commit's abbreviated name.
    pub commit: String,
}
