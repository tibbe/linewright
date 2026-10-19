//! Which variables of Linewright's environment an agent session is given:
//! "Sessions' environment" in `docs/specs/operation.md`.

use std::ffi::OsString;

use crate::store::github::{SESSION_TOKEN_VARIABLE, TOKEN_VARIABLES};
use crate::webhook::SECRET_VARIABLE;

/// The spec's table: every session is given these.
const TABLE: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "LC_*",
    "TZ",
    "TERM",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "ANTHROPIC_*",
    "CLAUDE_*",
];

/// The variables Linewright reads its own GitHub tokens and webhook secret
/// from, which `session-env` never passes.
const NEVER_PASSED: [&str; 4] = [
    TOKEN_VARIABLES[0],
    TOKEN_VARIABLES[1],
    SESSION_TOKEN_VARIABLE,
    SECRET_VARIABLE,
];

fn never_passed(name: &str) -> bool {
    NEVER_PASSED.contains(&name)
}

/// `pattern` is a variable's name, or a prefix when it ends in `*`.
fn matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == pattern,
    }
}

/// The `session-env` setting: the variables sessions are given beyond the
/// table's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionEnv {
    patterns: Vec<String>,
}

/// The variables of Linewright's environment that sessions are given. Only
/// `SessionEnv::split` makes one that holds any.
#[derive(Clone, Debug, Default)]
pub struct Given(Vec<(OsString, OsString)>);

impl Given {
    pub fn iter(&self) -> impl Iterator<Item = (&OsString, &OsString)> {
        self.0.iter().map(|(name, value)| (name, value))
    }
}

/// Linewright's environment, split by what sessions are given.
#[derive(Debug, Default)]
pub struct Split {
    pub given: Given,
    /// The names of the rest, sorted, but for those Linewright sets for
    /// sessions itself.
    pub withheld: Vec<String>,
}

impl SessionEnv {
    /// Refuses a name `session-env` never passes, and a `*` anywhere but at
    /// a name's end.
    pub fn new(names: impl IntoIterator<Item = String>) -> Result<SessionEnv, String> {
        let patterns = names
            .into_iter()
            .map(|name| {
                if never_passed(&name) {
                    return Err(format!(
                        "the setting `session-env` names `{name}`: sessions are never given \
                         Linewright's GitHub tokens or its webhook secret"
                    ));
                }
                if name.trim_end_matches('*').contains('*') {
                    return Err(format!(
                        "the setting `session-env` names `{name}`: a `*` can only end a name"
                    ));
                }
                Ok(name)
            })
            .collect::<Result<_, _>>()?;
        Ok(SessionEnv { patterns })
    }

    fn passes(&self, name: &str) -> bool {
        TABLE.iter().any(|pattern| matches(pattern, name))
            || (!never_passed(name) && self.patterns.iter().any(|pattern| matches(pattern, name)))
    }

    /// `set_by_linewright` names the variables Linewright sets for sessions
    /// itself, which are not withheld whatever their values here.
    pub fn split(
        &self,
        environment: impl IntoIterator<Item = (OsString, OsString)>,
        set_by_linewright: &[&str],
    ) -> Split {
        let mut split = Split::default();
        for (name, value) in environment {
            match name.to_str() {
                Some(text) if self.passes(text) => split.given.0.push((name, value)),
                Some(text) if set_by_linewright.contains(&text) => {}
                _ => split.withheld.push(name.to_string_lossy().into_owned()),
            }
        }
        split.withheld.sort();
        split
    }
}

/// The setting's form as a flag or variable: names separated by commas.
impl std::str::FromStr for SessionEnv {
    type Err = String;

    fn from_str(names: &str) -> Result<SessionEnv, String> {
        SessionEnv::new(
            names
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(String::from),
        )
    }
}
