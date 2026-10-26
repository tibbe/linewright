//! Which variables of Linewright's environment an agent session is given:
//! "Sessions' environment" in `docs/specs/operation.md`.

use std::ffi::OsString;

use percent_encoding::percent_decode_str;

use crate::harness::claude::{CREDENTIAL_VARIABLES, HEADER_VARIABLES};
use crate::secrets::OWN_CREDENTIAL_VARIABLES;
use crate::variable_names::{self, VariableNames, matches};

const SETTING: &str = "session-env";

/// The spec's table, but for `PROXY_VARIABLES`: every session is given
/// these.
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
    "NO_PROXY",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "ANTHROPIC_*",
    "CLAUDE_*",
];

/// The table's proxy variables that hold a URL, which may hold a password.
/// `NO_PROXY` holds hosts and is in `TABLE`.
const PROXY_VARIABLES: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

fn in_table(name: &str) -> bool {
    TABLE
        .iter()
        .chain(&PROXY_VARIABLES)
        .any(|pattern| matches(pattern, name))
}

/// A credential the table passes, and the variable it comes from.
#[derive(Debug, PartialEq, Eq)]
pub struct Credential {
    pub variable: String,
    pub value: String,
}

/// The credentials the table passes among `environment`, each with the
/// variable it comes from: the value of a variable the harness reads a
/// credential from, the password in a proxy variable's URL, as written
/// and, when it differs and is UTF-8, percent-decoded, and each header's
/// value in a variable the harness reads request headers from, whole and,
/// when it opens with a scheme, without it.
pub fn credentials_passed(environment: &[(String, String)]) -> Vec<Credential> {
    let credential = |variable: &str, value: String| Credential {
        variable: variable.to_string(),
        value,
    };
    let mut credentials = Vec::new();
    for (name, value) in environment {
        if CREDENTIAL_VARIABLES.contains(&name.as_str()) {
            credentials.push(credential(name, value.clone()));
        } else if PROXY_VARIABLES.contains(&name.as_str())
            && let Some(password) = proxy_password(value)
        {
            credentials.push(credential(name, password.to_string()));
            if let Ok(decoded) = percent_decode_str(password).decode_utf8()
                && decoded != password
            {
                credentials.push(credential(name, decoded.into_owned()));
            }
        } else if HEADER_VARIABLES.contains(&name.as_str()) {
            for header_value in header_values(value) {
                credentials.push(credential(name, header_value.to_string()));
                if let Some(rest) = without_scheme(header_value) {
                    credentials.push(credential(name, rest.to_string()));
                }
            }
        }
    }
    credentials
}

/// The password in a proxy URL, as written. A URL without a scheme is
/// read as `http://`, as curl and reqwest read it, and the userinfo ends
/// at the authority's last `@`, as the WHATWG URL standard
/// (<https://url.spec.whatwg.org/#authority-state>), and so Claude Code,
/// reads it.
fn proxy_password(url: &str) -> Option<&str> {
    let rest = match url.split_once("://") {
        Some((scheme, rest))
            if scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) =>
        {
            rest
        }
        _ => url,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let (userinfo, _) = authority.rsplit_once('@')?;
    let (_, password) = userinfo.split_once(':')?;
    Some(password)
}

/// Each header's value in a variable the harness reads request headers
/// from, read as Claude Code reads it: a header per line, ending in `\n`
/// or `\r\n`, split at its first `:` and trimmed. Lines without a `:` or
/// a name are skipped.
fn header_values(headers: &str) -> impl Iterator<Item = &str> {
    headers.lines().filter_map(|line| {
        let (name, value) = line.split_once(':')?;
        (!name.trim().is_empty()).then(|| value.trim())
    })
}

/// What a header value leaves without the scheme it opens with, if any: an
/// RFC 9110 token, then one or more spaces, then the rest
/// (<https://www.rfc-editor.org/rfc/rfc9110#section-11.4>).
fn without_scheme(value: &str) -> Option<&str> {
    let (scheme, rest) = value.split_once(' ')?;
    let rest = rest.trim();
    let token = !scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c));
    (token && !rest.is_empty()).then_some(rest)
}

/// Whether `name` is a variable Linewright reads its own GitHub tokens or
/// webhook secret from, which `session-env` never passes.
fn never_passed(name: &str) -> bool {
    OWN_CREDENTIAL_VARIABLES.contains(&name)
}

/// The `session-env` setting: the variables sessions are given beyond the
/// table's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionEnv {
    names: VariableNames,
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
        let names: Vec<String> = names
            .into_iter()
            .map(|name| {
                if never_passed(&name) {
                    return Err(format!(
                        "the setting `{SETTING}` names `{name}`: sessions are never given \
                         Linewright's GitHub tokens or its webhook secret"
                    ));
                }
                variable_names::check(SETTING, &name)?;
                Ok(name)
            })
            .collect::<Result<_, _>>()?;
        Ok(SessionEnv {
            names: VariableNames::new(SETTING, names)?,
        })
    }

    fn passes(&self, name: &str) -> bool {
        in_table(name) || (!never_passed(name) && self.names.matches(name))
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
        SessionEnv::new(variable_names::comma_separated(names))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_passes_every_variable_the_harness_reads_a_credential_from() {
        for name in CREDENTIAL_VARIABLES {
            assert!(in_table(name), "{name}");
        }
    }

    #[test]
    fn the_table_passes_every_variable_the_harness_reads_request_headers_from() {
        for name in HEADER_VARIABLES {
            assert!(in_table(name), "{name}");
        }
    }
}
