//! Linewright's secrets: the values of the variables it reads a credential
//! from, and of those the `secrets` setting names
//! (`docs/specs/architecture.md`, the redaction decision).

use std::ffi::OsString;

use crate::git::Addition;
use crate::redaction::Redaction;
use crate::store::github::{SESSION_TOKEN_VARIABLE, TOKEN_VARIABLES};
use crate::variable_names::VariableNames;
use crate::webhook;

pub const SETTING: &str = "secrets";

/// The variables Linewright reads its own GitHub tokens and webhook secret
/// from.
pub const OWN_CREDENTIAL_VARIABLES: [&str; 4] = [
    TOKEN_VARIABLES[0],
    TOKEN_VARIABLES[1],
    SESSION_TOKEN_VARIABLE,
    webhook::SECRET_VARIABLE,
];

/// The variable the harness reads its Anthropic API key from.
const ANTHROPIC_API_KEY_VARIABLE: &str = "ANTHROPIC_API_KEY";

/// Whether `name` is one of the variables Linewright reads a credential
/// from, as `docs/specs/operation.md`'s Credentials bullet names them.
fn reads_a_credential_from(name: &str) -> bool {
    OWN_CREDENTIAL_VARIABLES.contains(&name) || name == ANTHROPIC_API_KEY_VARIABLE
}

/// A variable of Linewright's environment whose value is a secret, and
/// the value, which is never empty.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret {
    name: String,
    value: String,
}

impl Secret {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret({})", self.name)
    }
}

/// The fewest characters a secret is redacted and refused with: a shorter
/// one would be found in text by accident.
const SHORTEST: usize = 6;

/// Linewright's secrets, and their redaction.
#[derive(Clone, Debug, Default)]
pub struct Secrets {
    /// The secrets of at least `SHORTEST` characters.
    list: Vec<Secret>,
    /// The variables of the shorter ones, sorted.
    short: Vec<String>,
    redaction: Redaction,
}

impl Secrets {
    /// The secrets among `vars`, Linewright's environment as
    /// [`std::env::vars_os`] gives it, `names` being the `secrets` setting.
    /// Empty values are skipped, as every text holds the empty string, and
    /// so are names or values that are not UTF-8, which no text Linewright
    /// handles holds as they are. A secret shorter than `SHORTEST`
    /// characters is set aside in [`Secrets::short`], neither redacted nor
    /// refused.
    ///
    /// A name in `names` that is not a prefix and whose variable is unset or
    /// empty is an error.
    pub fn of(
        names: &VariableNames,
        vars: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> Result<Secrets, String> {
        let vars: Vec<(OsString, OsString)> = vars.into_iter().collect();
        if let Some(name) = names.exact().find(|name| {
            !vars
                .iter()
                .any(|(variable, value)| variable == name && !value.is_empty())
        }) {
            return Err(format!(
                "the setting `{SETTING}` names `{name}`, which is unset or empty: \
                 set it, or take it out of `{SETTING}`"
            ));
        }
        let all: Vec<Secret> = vars
            .into_iter()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .filter(|(name, value)| {
                !value.is_empty() && (reads_a_credential_from(name) || names.matches(name))
            })
            .map(|(name, value)| Secret { name, value })
            .collect();
        let (list, short): (Vec<Secret>, Vec<Secret>) = all
            .into_iter()
            .partition(|secret| secret.value.chars().count() >= SHORTEST);
        let mut short: Vec<String> = short.into_iter().map(|secret| secret.name).collect();
        short.sort();
        Ok(Secrets {
            redaction: Redaction::of(&list),
            list,
            short,
        })
    }

    /// The variables whose secrets are too short to redact or refuse,
    /// sorted.
    pub fn short(&self) -> &[String] {
        &self.short
    }

    pub fn redaction(&self) -> &Redaction {
        &self.redaction
    }

    /// The first of `additions` that holds one of the secrets.
    pub fn held(&self, additions: &[Addition]) -> Option<Held> {
        additions.iter().find_map(|addition| {
            let secret = self
                .list
                .iter()
                .find(|c| addition.text.contains(c.value()))?;
            Some(Held {
                variable: secret.name.clone(),
                file: addition.file.clone(),
                commit: addition.commit.clone(),
            })
        })
    }
}

/// A secret a commit adds to a file, named but not given.
#[derive(Debug, PartialEq, Eq)]
pub struct Held {
    pub variable: String,
    pub file: String,
    /// The commit's abbreviated name.
    pub commit: String,
}
