//! Which variables of Linewright's environment an agent session is given,
//! and the values set for it: "Environment" in
//! `docs/specs/sessions.md`.

use std::collections::BTreeMap;
use std::ffi::OsString;

use percent_encoding::percent_decode_str;

use crate::harness::Variables;
use crate::layer;
use crate::secrets::OWN_CREDENTIAL_VARIABLES;
use crate::store::TOKEN_VARIABLES;
use crate::variable_names::{self, VariableNames, matches};

/// The session's own temporary directory.
pub const TEMPORARY_DIRECTORY: &str = "TMPDIR";
/// Where `gh` reads its configuration from: <https://cli.github.com/manual/gh_help_environment>.
pub const GH_CONFIG_DIR: &str = "GH_CONFIG_DIR";

/// The variables of Linewright's own that it sets for every session.
const SET_FOR_EVERY_SESSION: [&str; 3] =
    [TEMPORARY_DIRECTORY, TOKEN_VARIABLES[0], TOKEN_VARIABLES[1]];

/// Every variable Linewright may set for a session of the harness whose
/// variables are `harness` that `session-env` may not set: all but
/// `GH_CONFIG_DIR`, whose value in a trial replaces `session-env`'s.
fn not_set_by_operator(harness: &Variables) -> impl Iterator<Item = &'static str> {
    SET_FOR_EVERY_SESSION
        .into_iter()
        .chain(harness.settings.iter().copied())
}

/// The setting naming the variables passed.
const PASS_SETTING: &str = "session-pass-env";
/// The setting naming the values set.
const SET_SETTING: &str = "session-env";

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

/// The variables Linewright sets for a session of the harness whose
/// variables are `harness`, given whether its `gh` reaches GitHub through a
/// gate, as in a trial.
pub fn variables_set_for_sessions(harness: &Variables, gh_through_gate: bool) -> Vec<&'static str> {
    not_set_by_operator(harness)
        .chain(gh_through_gate.then_some(GH_CONFIG_DIR))
        .collect()
}

/// Whether the table passes the variable `name`.
pub fn table_passes(name: &str) -> bool {
    in_table(name)
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
/// when it opens with a scheme, without it. The harness's variables are
/// `harness`.
pub fn credentials_passed(
    environment: &[(String, String)],
    harness: &Variables,
) -> Vec<Credential> {
    let credential = |variable: &str, value: String| Credential {
        variable: variable.to_string(),
        value,
    };
    let mut credentials = Vec::new();
    for (name, value) in environment {
        if harness.credentials.contains(&name.as_str()) {
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
        } else if harness.headers.contains(&name.as_str()) {
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

/// Whether `name` is a variable Linewright reads one of its own credentials
/// from, which `session-pass-env` never passes.
fn never_passed(name: &str) -> bool {
    OWN_CREDENTIAL_VARIABLES.contains(&name)
}

/// The `session-pass-env` setting: the variables of Linewright's
/// environment sessions are given beyond the table's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassEnv {
    names: VariableNames,
}

/// One layer's `session-env`, or the layers' together: the values sessions
/// are given, by name, taken as written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetEnv {
    values: BTreeMap<String, String>,
}

/// What sessions are given beyond what Linewright sets itself: the whole
/// pipeline's, or what a stage's table adds to it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionEnv {
    pass: PassEnv,
    set: SetEnv,
}

/// The variables of Linewright's environment that sessions are given. Only
/// `SessionEnv::split` makes one that holds any.
#[derive(Clone, Debug, Default)]
pub struct Given(Vec<(OsString, OsString)>);

impl Given {
    pub fn iter(&self) -> impl Iterator<Item = (&OsString, &OsString)> {
        self.0.iter().map(|(name, value)| (name, value))
    }

    /// Gives `set`'s values, each replacing a variable of the same name.
    fn set(&mut self, set: &SetEnv) {
        self.0.retain(|(name, _)| {
            name.to_str()
                .is_none_or(|name| !set.values.contains_key(name))
        });
        self.0.extend(
            set.values
                .iter()
                .map(|(name, value)| (name.into(), value.into())),
        );
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

impl PassEnv {
    /// Refuses a name `session-pass-env` never passes, and a `*` anywhere but at
    /// a name's end.
    pub fn new(names: impl IntoIterator<Item = String>) -> Result<PassEnv, String> {
        let names: Vec<String> = names
            .into_iter()
            .map(|name| {
                if never_passed(&name) {
                    return Err(format!(
                        "the setting `{PASS_SETTING}` names `{name}`: sessions are never given \
                         Linewright's own credentials"
                    ));
                }
                variable_names::check(PASS_SETTING, &name)?;
                Ok(name)
            })
            .collect::<Result<_, _>>()?;
        Ok(PassEnv {
            names: VariableNames::new(PASS_SETTING, names)?,
        })
    }

    /// These names and `more`'s.
    #[must_use]
    pub fn and(&self, more: &PassEnv) -> PassEnv {
        PassEnv {
            names: self.names.and(&more.names),
        }
    }

    fn passes(&self, name: &str) -> bool {
        in_table(name) || (!never_passed(name) && self.names.matches(name))
    }
}

/// The setting as a flag or variable takes it: names separated by commas.
impl std::str::FromStr for PassEnv {
    type Err = String;

    fn from_str(names: &str) -> Result<PassEnv, String> {
        PassEnv::new(variable_names::comma_separated(names))
    }
}

impl SetEnv {
    /// One layer's values. Refuses a name given twice, a name or value an
    /// environment can't hold, and a name Linewright may set itself for
    /// sessions of the harness whose variables are `harness`.
    pub fn new(
        entries: impl IntoIterator<Item = (String, String)>,
        harness: &Variables,
    ) -> Result<SetEnv, String> {
        let mut values = BTreeMap::new();
        for (name, value) in entries {
            let cannot_hold = |what: &str| {
                Err(format!(
                    "the setting `{SET_SETTING}` sets {what}, which an environment can't hold"
                ))
            };
            if name.is_empty() {
                return cannot_hold("a variable with an empty name");
            }
            if name.contains('=') {
                return cannot_hold(&format!("`{name}`, a name with `=`"));
            }
            if name.contains('\0') {
                return cannot_hold(&format!("{name:?}, a name with a NUL byte"));
            }
            if value.contains('\0') {
                return cannot_hold(&format!("`{name}` to a value with a NUL byte"));
            }
            if not_set_by_operator(harness).any(|own| own == name) {
                return Err(format!(
                    "the setting `{SET_SETTING}` sets `{name}`: Linewright sets `{name}` for \
                     sessions itself"
                ));
            }
            if values.contains_key(&name) {
                return Err(format!("the setting `{SET_SETTING}` sets `{name}` twice"));
            }
            values.insert(name, value);
        }
        Ok(SetEnv { values })
    }

    /// The layer the flag gives, once per entry, each `NAME=value` split at
    /// its first `=`, for sessions of the harness whose variables are
    /// `harness`.
    pub fn from_flags(entries: &[String], harness: &Variables) -> Result<SetEnv, String> {
        SetEnv::new(
            entries
                .iter()
                .map(|entry| match entry.split_once('=') {
                    Some((name, value)) => Ok((name.to_string(), value.to_string())),
                    None => Err(format!(
                        "the setting `{SET_SETTING}` is given {entry:?}, not `NAME=value`; \
                         `{PASS_SETTING}` names variables to pass"
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?,
            harness,
        )
    }

    /// Every name set here or in `lower`, with this value where both set
    /// one, as a table combines.
    #[must_use]
    pub fn over(&self, lower: &SetEnv) -> SetEnv {
        SetEnv {
            values: layer::table([self.values.clone(), lower.values.clone()]),
        }
    }

    /// The values set, by name.
    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.values
    }

    /// The names set, sorted.
    pub fn names(&self) -> Vec<String> {
        self.values.keys().cloned().collect()
    }
}

impl SessionEnv {
    pub fn new(pass: PassEnv, set: SetEnv) -> SessionEnv {
        SessionEnv { pass, set }
    }

    /// The names `session-env` sets, sorted.
    pub fn set_names(&self) -> Vec<String> {
        self.set.names()
    }

    /// This and what a stage's table adds to it, the stage's value winning
    /// for a name both set.
    #[must_use]
    pub fn and(&self, stage: &SessionEnv) -> SessionEnv {
        SessionEnv {
            pass: self.pass.and(&stage.pass),
            set: stage.set.over(&self.set),
        }
    }

    /// `environment` split by what sessions are given, the values set
    /// replacing variables of the same name passed. `set_by_linewright`
    /// names the variables Linewright sets for sessions itself, which are
    /// not withheld whatever their values here.
    pub fn split(
        &self,
        environment: impl IntoIterator<Item = (OsString, OsString)>,
        set_by_linewright: &[&str],
    ) -> Split {
        let mut split = Split::default();
        for (name, value) in environment {
            match name.to_str() {
                Some(text) if self.pass.passes(text) => split.given.0.push((name, value)),
                Some(text) if set_by_linewright.contains(&text) => {}
                _ => split.withheld.push(name.to_string_lossy().into_owned()),
            }
        }
        split.withheld.sort();
        split.given.set(&self.set);
        split
    }
}
