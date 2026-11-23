//! The people Linewright signs in (`docs/specs/access.md`, People).

use std::fmt;

use super::providers::{Provider, Providers, github_login};
use crate::host::domain_name;

/// The identity the `manager` setting names, of a provider Linewright
/// offers: lower-cased, as Linewright compares it ignoring case
/// (`docs/specs/access.md`, Signing in, Matching an identity).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagerName {
    provider: Provider,
    /// The email for Google, the login for GitHub.
    identity: String,
}

/// Why a `manager` setting is no [`ManagerName`].
#[derive(Debug, PartialEq, Eq)]
pub enum NotAManager {
    /// It names no identity, with why.
    Malformed(&'static str),
    /// It names an identity of this provider, which Linewright does not
    /// offer.
    NotOffered(Provider),
}

impl ManagerName {
    /// The identity `name` names, `google:EMAIL` or `github:LOGIN`, the
    /// provider ignoring case, when it is of one of `providers`.
    pub fn parse(name: &str, providers: &Providers) -> Result<ManagerName, NotAManager> {
        let (provider, identity) = written(name).map_err(NotAManager::Malformed)?;
        if providers.offer(provider) {
            Ok(ManagerName { provider, identity })
        } else {
            Err(NotAManager::NotOffered(provider))
        }
    }

    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// The email for Google, the login for GitHub, lower-cased.
    pub fn identity(&self) -> &str {
        &self.identity
    }
}

/// The provider and the identity, lower-cased, that `name` writes, or why it
/// writes none.
fn written(name: &str) -> Result<(Provider, String), &'static str> {
    const FORM: &str = "it is `google:` and an email, or `github:` and a login";
    let (provider, identity) = name.split_once(':').ok_or(FORM)?;
    if provider.eq_ignore_ascii_case("google") {
        let email = identity
            .split_once('@')
            .filter(|(local, _)| !local.is_empty() && !local.contains(char::is_whitespace))
            .and_then(|(local, domain)| Some(format!("{local}@{}", domain_name(domain)?)))
            .ok_or("`google:` is followed by an email, such as `google:alice@example.com`")?;
        Ok((Provider::Google, email.to_ascii_lowercase()))
    } else if provider.eq_ignore_ascii_case("github") {
        let login = github_login(identity)
            .ok_or("`github:` is followed by a login, such as `github:octocat`")?;
        Ok((Provider::GitHub, login))
    } else {
        Err(FORM)
    }
}

impl fmt::Display for ManagerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.provider, self.identity)
    }
}
