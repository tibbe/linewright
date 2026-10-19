//! Linewright's credentials: the variables of its environment named as one
//! (`docs/specs/architecture.md`, the redaction decision).

use std::ffi::OsString;

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

/// The credentials among `vars`, Linewright's environment as
/// [`std::env::vars_os`] gives it. Empty values are skipped, as every text
/// holds the empty string, and so are names or values that are not UTF-8,
/// which no text Linewright handles holds as they are.
pub fn credentials(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Vec<Credential> {
    vars.into_iter()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(name, value)| {
            !value.is_empty() && SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
        })
        .map(|(name, value)| Credential { name, value })
        .collect()
}
