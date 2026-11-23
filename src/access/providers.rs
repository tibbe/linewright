//! The identity providers Linewright signs people in through, as the
//! operator configures them (`docs/specs/access.md`, Signing in).

use std::fmt;

use crate::host::domain_name;

/// The variable Linewright reads Google's client secret from
/// (`docs/specs/operation.md`, Configuration).
pub const GOOGLE_SECRET_VARIABLE: &str = "LINEWRIGHT_GOOGLE_CLIENT_SECRET";

/// The variable Linewright reads GitHub's client secret from
/// (`docs/specs/operation.md`, Configuration).
pub const GITHUB_SECRET_VARIABLE: &str = "LINEWRIGHT_GITHUB_CLIENT_SECRET";

/// An identity provider Linewright can sign people in through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Google,
    GitHub,
}

impl Provider {
    /// Its name, as the spec gives it.
    pub fn name(self) -> &'static str {
        match self {
            Provider::Google => "Google",
            Provider::GitHub => "GitHub",
        }
    }

    /// Its name in `manager`, before the `:`.
    fn key(self) -> &'static str {
        match self {
            Provider::Google => "google",
            Provider::GitHub => "github",
        }
    }

    /// The one that is not it.
    #[must_use]
    pub fn other(self) -> Provider {
        match self {
            Provider::Google => Provider::GitHub,
            Provider::GitHub => Provider::Google,
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

/// The identity providers Linewright offers: at least one.
#[derive(Clone, Debug)]
pub enum Providers {
    Google(Google),
    GitHub(GitHub),
    Both(Google, GitHub),
}

impl Providers {
    /// The providers among `google` and `github`, `None` when neither is.
    pub fn of(google: Option<Google>, github: Option<GitHub>) -> Option<Providers> {
        match (google, github) {
            (Some(google), Some(github)) => Some(Providers::Both(google, github)),
            (Some(google), None) => Some(Providers::Google(google)),
            (None, Some(github)) => Some(Providers::GitHub(github)),
            (None, None) => None,
        }
    }

    pub fn google(&self) -> Option<&Google> {
        match self {
            Providers::Google(google) | Providers::Both(google, _) => Some(google),
            Providers::GitHub(_) => None,
        }
    }

    /// Whether `provider` is one of them.
    pub fn offer(&self, provider: Provider) -> bool {
        match provider {
            Provider::Google => self.google().is_some(),
            Provider::GitHub => self.github().is_some(),
        }
    }

    pub fn github(&self) -> Option<&GitHub> {
        match self {
            Providers::GitHub(github) | Providers::Both(_, github) => Some(github),
            Providers::Google(_) => None,
        }
    }
}

/// Google, as Linewright's OAuth client of it.
#[derive(Clone, Debug)]
pub struct Google {
    pub client_id: ClientId,
    pub secret: ClientSecret,
    pub signup: SignUpDomains,
}

/// GitHub, as Linewright's OAuth client of it.
#[derive(Clone, Debug)]
pub struct GitHub {
    pub client_id: ClientId,
    pub secret: ClientSecret,
    pub signup: SignUpOrgs,
}

/// The client ID a provider gave Linewright: never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientId(String);

impl ClientId {
    /// `None` for an empty `id`, which is no client ID.
    pub fn new(id: String) -> Option<ClientId> {
        (!id.is_empty()).then_some(ClientId(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The client secret a provider gave Linewright: never empty, and never
/// shown by `Debug`.
#[derive(Clone)]
pub struct ClientSecret(String);

impl ClientSecret {
    /// `None` for an empty `secret`, which is no secret.
    pub fn new(secret: String) -> Option<ClientSecret> {
        (!secret.is_empty()).then_some(ClientSecret(secret))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ClientSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClientSecret([redacted])")
    }
}

/// Google's sign-up rule, `google-signup-domains`: the domains, lower-cased
/// and without a trailing dot, whose verified emails meet it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignUpDomains(Vec<String>);

impl SignUpDomains {
    /// The rule of the one domain `domain`, such as `example.com`, or what
    /// is wrong with it.
    pub fn of(domain: &str) -> Result<SignUpDomains, String> {
        domain_name(domain)
            .map(|domain| SignUpDomains(vec![domain]))
            .ok_or_else(|| format!("names {domain:?}: it is a domain, such as `example.com`"))
    }

    /// These domains and `more`'s, in order and once each.
    #[must_use]
    pub fn and(&self, more: &SignUpDomains) -> SignUpDomains {
        SignUpDomains(
            crate::layer::list([&self.0, &more.0], |domain| domain)
                .into_iter()
                .cloned()
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn domains(&self) -> &[String] {
        &self.0
    }
}

/// GitHub's sign-up rule, `github-signup-orgs`: the organizations and
/// teams whose members meet it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignUpOrgs(Vec<Membership>);

/// An organization, or a team of one, that GitHub's sign-up rule names,
/// lower-cased.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Membership {
    organization: String,
    team: Option<String>,
}

impl Membership {
    pub fn organization(&self) -> &str {
        &self.organization
    }

    /// The team's slug, `None` for the whole organization.
    pub fn team(&self) -> Option<&str> {
        self.team.as_deref()
    }

    /// The organization or the team `name` names, `org` or `org/team`.
    fn parse(name: &str) -> Option<Membership> {
        let (organization, team) = match name.split_once('/') {
            None => (name, None),
            Some((organization, team)) => {
                let slug = !team.is_empty()
                    && team
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
                (organization, Some(slug.then(|| team.to_ascii_lowercase())?))
            }
        };
        Some(Membership {
            organization: github_login(organization)?,
            team,
        })
    }
}

impl SignUpOrgs {
    /// The rule of the one organization or team `name`, `org` or
    /// `org/team`, or what is wrong with it.
    pub fn of(name: &str) -> Result<SignUpOrgs, String> {
        Membership::parse(name)
            .map(|membership| SignUpOrgs(vec![membership]))
            .ok_or_else(|| {
                format!(
                    "names {name:?}: it is an organization, such as `acme`, or a team of one, \
                     such as `acme/core`"
                )
            })
    }

    /// These organizations and teams and `more`'s, in order and once each.
    #[must_use]
    pub fn and(&self, more: &SignUpOrgs) -> SignUpOrgs {
        SignUpOrgs(
            crate::layer::list([&self.0, &more.0], |membership| membership)
                .into_iter()
                .cloned()
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn memberships(&self) -> &[Membership] {
        &self.0
    }
}

/// The longest login GitHub gives.
const LONGEST_LOGIN: usize = 39;

/// `login` lower-cased, when it is a GitHub login: letters, digits and
/// single hyphens, neither starting nor ending it.
pub(crate) fn github_login(login: &str) -> Option<String> {
    let login_characters = !login.is_empty()
        && login.len() <= LONGEST_LOGIN
        && !login.starts_with('-')
        && !login.ends_with('-')
        && !login.contains("--")
        && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    login_characters.then(|| login.to_ascii_lowercase())
}
