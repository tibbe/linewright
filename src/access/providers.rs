//! The identity providers Linewright signs people in through, as the
//! operator configures them (`docs/specs/access.md`, Signing in).

#[cfg(feature = "fake-claude")]
pub mod fake;

use std::fmt;
use std::time::Duration;

use anyhow::{Context as _, bail};
use base64::Engine as _;
use serde::Deserialize;
use url::Url;

use super::people::{Claims, Identity};
use crate::domain::Timestamp;
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

    /// The provider whose [`Provider::key`] is `key`.
    pub(crate) fn named(key: &str) -> Option<Provider> {
        [Provider::Google, Provider::GitHub]
            .into_iter()
            .find(|provider| provider.key() == key)
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

/// Google, as Linewright's client of its sign-in.
#[derive(Clone, Debug)]
pub struct Google {
    pub client_id: ClientId,
    pub secret: ClientSecret,
    pub signup: SignUpDomains,
    pub endpoints: GoogleEndpoints,
}

/// Where Google answers: Google itself, or a fake of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoogleEndpoints {
    /// Where a browser is sent to sign in.
    pub authorize: Url,
    /// Where a code is exchanged for an ID token.
    pub token: Url,
    /// The `iss` its ID tokens may have.
    pub issuers: Vec<String>,
}

/// Google's own, from its discovery document:
/// <https://accounts.google.com/.well-known/openid-configuration>, and the
/// two forms of `iss` it gives:
/// <https://developers.google.com/identity/openid-connect/openid-connect#an-id-tokens-payload>
impl Default for GoogleEndpoints {
    fn default() -> GoogleEndpoints {
        GoogleEndpoints {
            authorize: Url::parse("https://accounts.google.com/o/oauth2/v2/auth")
                .expect("Google's URL parses"),
            token: Url::parse("https://oauth2.googleapis.com/token").expect("Google's URL parses"),
            issuers: vec![
                "https://accounts.google.com".into(),
                "accounts.google.com".into(),
            ],
        }
    }
}

/// What a sign-in begun at a provider is checked by when it comes back.
#[derive(Clone, Debug)]
pub struct Begun<'a> {
    pub state: &'a str,
    /// The PKCE code verifier.
    pub verifier: &'a str,
    pub nonce: &'a str,
}

/// How long Linewright waits for a provider's answer.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(10);

impl Google {
    /// Google of `client_id` `test` and the secret `s3cret`, with the
    /// sign-up rule `signup`, at Google's own endpoints.
    #[cfg(test)]
    pub(crate) fn for_tests(signup: SignUpDomains) -> Google {
        Google {
            client_id: ClientId::new("test".into()).unwrap(),
            secret: ClientSecret::new("s3cret".into()).unwrap(),
            signup,
            endpoints: GoogleEndpoints::default(),
        }
    }

    /// Where to send a browser to sign in with Google and come back to
    /// `callback`, for the sign-in `begun`, with S256 PKCE.
    /// <https://developers.google.com/identity/openid-connect/openid-connect#sendauthrequest>
    pub fn authorize_url(&self, callback: &Url, begun: &Begun) -> Url {
        let mut url = self.endpoints.authorize.clone();
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", self.client_id.as_str())
            .append_pair("redirect_uri", callback.as_str())
            .append_pair("scope", "openid email profile")
            .append_pair("state", begun.state)
            .append_pair("code_challenge", &super::tokens::s256(begun.verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("nonce", begun.nonce)
            .append_pair("prompt", "select_account");
        url
    }

    /// Exchanges `code`, which Google sent back to `callback` for the
    /// sign-in `begun`, for what its ID token says of the person, once it
    /// checks the token's `iss`, `aud`, `exp` at `now` and `nonce`. The ID
    /// token comes straight from Google over TLS, so its signature needs no
    /// check:
    /// <https://developers.google.com/identity/openid-connect/openid-connect#obtainuserinfo>
    pub async fn finish(
        &self,
        http: &reqwest::Client,
        code: &str,
        callback: &Url,
        begun: &Begun<'_>,
        now: Timestamp,
    ) -> anyhow::Result<Claims> {
        let form = serde_urlencoded::to_string([
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.secret.expose()),
            ("redirect_uri", callback.as_str()),
            ("code_verifier", begun.verifier),
        ])
        .context("writing the token request")?;
        let response = http
            .post(self.endpoints.token.clone())
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .header(reqwest::header::ACCEPT, "application/json")
            .body(form)
            .timeout(PROVIDER_TIMEOUT)
            .send()
            .await
            .context("asking Google for the ID token")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("reading Google's token response")?;
        if !status.is_success() {
            bail!("Google answered the token request with {status}: {body}");
        }
        let token: TokenResponse =
            serde_json::from_str(&body).context("reading Google's token response")?;
        let payload = token
            .id_token
            .split('.')
            .nth(1)
            .context("Google's ID token has no payload")?;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload.trim_end_matches('='))
            .context("decoding Google's ID token")?;
        let id: IdToken = serde_json::from_slice(&payload).context("reading Google's ID token")?;
        self.claims(id, begun.nonce, now)
    }

    /// What `id` says of the person, once its `iss`, `aud`, `exp` at `now`
    /// and `nonce` check out.
    fn claims(&self, id: IdToken, nonce: &str, now: Timestamp) -> anyhow::Result<Claims> {
        if !self.endpoints.issuers.contains(&id.iss) {
            bail!("the ID token is from {:?}, not Google", id.iss);
        }
        if !id.aud.names(self.client_id.as_str()) {
            bail!("the ID token is for {:?}, not this client", id.aud);
        }
        if id.exp <= now.timestamp() {
            bail!("the ID token expired");
        }
        if id.nonce.as_deref() != Some(nonce) {
            bail!("the ID token's nonce is not the sign-in's");
        }
        let email = id.email.context("the ID token has no email")?;
        Ok(Claims {
            identity: Identity {
                provider: Provider::Google,
                subject: id.sub,
            },
            shown_as: email,
            name: id.name.filter(|name| !name.trim().is_empty()),
            avatar: id
                .picture
                .and_then(|picture| Url::parse(&picture).ok())
                .filter(|picture| picture.scheme() == "https"),
            email_verified: id.email_verified.is_some_and(|verified| verified.is_true()),
        })
    }
}

/// Google's answer to a token request.
/// <https://developers.google.com/identity/openid-connect/openid-connect#exchangecode>
#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
}

/// The claims of Google's ID token that Linewright reads.
/// <https://developers.google.com/identity/openid-connect/openid-connect#an-id-tokens-payload>
#[derive(Deserialize)]
struct IdToken {
    iss: String,
    aud: Audience,
    exp: i64,
    sub: String,
    nonce: Option<String>,
    email: Option<String>,
    email_verified: Option<Verified>,
    name: Option<String>,
    picture: Option<String>,
}

/// The clients an ID token is for: one, as Google gives it, or several.
/// <https://openid.net/specs/openid-connect-core-1_0.html#IDToken>
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Several(Vec<String>),
}

impl Audience {
    /// Whether the token is for `client`.
    fn names(&self, client: &str) -> bool {
        match self {
            Audience::One(one) => one == client,
            Audience::Several(several) => several.iter().any(|one| one == client),
        }
    }
}

/// `email_verified`, which Google gives as a boolean, and in its documented
/// example as the text `"true"`.
/// <https://developers.google.com/identity/openid-connect/openid-connect#an-id-tokens-payload>
#[derive(Deserialize)]
#[serde(untagged)]
enum Verified {
    Boolean(bool),
    Text(String),
}

impl Verified {
    fn is_true(&self) -> bool {
        match self {
            Verified::Boolean(verified) => *verified,
            Verified::Text(text) => text == "true",
        }
    }
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::routing::post;

    use super::*;

    /// The example of an ID token's payload of Google's documentation:
    /// <https://developers.google.com/identity/openid-connect/openid-connect#an-id-tokens-payload>
    const GOOGLES_EXAMPLE: &str = r#"{
        "iss": "https://accounts.google.com",
        "azp": "1234987819200.apps.googleusercontent.com",
        "aud": "1234987819200.apps.googleusercontent.com",
        "sub": "10769150350006150715113082367",
        "at_hash": "HK6E_P6Dh8Y93mRNtsDB1Q",
        "hd": "example.com",
        "email": "jsmith@example.com",
        "email_verified": "true",
        "iat": 1353601026,
        "exp": 1353604926,
        "nonce": "0394852-3190485-2490358"
    }"#;

    /// The example's time between its `iat` and its `exp`.
    #[track_caller]
    fn while_valid() -> Timestamp {
        chrono::DateTime::from_timestamp(1_353_601_100, 0).unwrap()
    }

    /// A token endpoint answering each request, as Google's documentation
    /// shows its answer, with an ID token of `payload`, and the forms it was
    /// sent.
    /// <https://developers.google.com/identity/openid-connect/openid-connect#exchangecode>
    async fn token_endpoint(payload: serde_json::Value) -> (Url, Arc<Mutex<Vec<String>>>) {
        let sent = Arc::new(Mutex::new(vec![]));
        let id_token = format!(
            "eyJhbGciOiJSUzI1NiJ9.{}.signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
        );
        let router = axum::Router::new().route(
            "/token",
            post({
                let sent = sent.clone();
                move |body: String| async move {
                    sent.lock().unwrap().push(body);
                    serde_json::json!({
                        "access_token": "ya29.example",
                        "expires_in": 3599,
                        "token_type": "Bearer",
                        "id_token": id_token,
                    })
                    .to_string()
                }
            }),
        );
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(socket, router).await });
        (
            Url::parse(&format!("http://{address}/token")).unwrap(),
            sent,
        )
    }

    #[track_caller]
    fn google(token: Url) -> Google {
        Google {
            client_id: ClientId::new("1234987819200.apps.googleusercontent.com".into()).unwrap(),
            endpoints: GoogleEndpoints {
                token,
                ..GoogleEndpoints::default()
            },
            ..Google::for_tests(SignUpDomains::default())
        }
    }

    /// A sign-in begun with the code verifier of RFC 7636, Appendix B, and
    /// the nonce of Google's example.
    const BEGUN: Begun = Begun {
        state: "state",
        verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        nonce: "0394852-3190485-2490358",
    };

    #[track_caller]
    fn callback() -> Url {
        Url::parse("https://lw.example.com/auth/google/callback").unwrap()
    }

    #[test_log::test(tokio::test)]
    async fn a_finished_sign_in_gives_the_subject_and_the_verified_email() {
        let payload: serde_json::Value = serde_json::from_str(GOOGLES_EXAMPLE).unwrap();
        let (token, sent) = token_endpoint(payload).await;

        let claims = google(token)
            .finish(
                &reqwest::Client::new(),
                "4/P7q7W91",
                &callback(),
                &BEGUN,
                while_valid(),
            )
            .await
            .unwrap();

        assert_eq!(
            claims,
            Claims {
                identity: Identity {
                    provider: Provider::Google,
                    subject: "10769150350006150715113082367".into(),
                },
                shown_as: "jsmith@example.com".into(),
                name: None,
                avatar: None,
                email_verified: true,
            }
        );
        let form: Vec<(String, String)> =
            serde_urlencoded::from_str(&sent.lock().unwrap()[0]).unwrap();
        assert_eq!(
            form,
            [
                ("grant_type", "authorization_code"),
                ("code", "4/P7q7W91"),
                ("client_id", "1234987819200.apps.googleusercontent.com"),
                ("client_secret", "s3cret"),
                (
                    "redirect_uri",
                    "https://lw.example.com/auth/google/callback"
                ),
                (
                    "code_verifier",
                    "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
                ),
            ]
            .map(|(name, value)| (name.to_string(), value.to_string()))
        );
    }

    #[test_log::test(tokio::test)]
    async fn an_id_token_from_another_issuer_for_another_client_expired_or_of_another_nonce_is_refused()
     {
        let example: serde_json::Value = serde_json::from_str(GOOGLES_EXAMPLE).unwrap();
        let changed = |name: &str, value: serde_json::Value| {
            let mut payload = example.clone();
            payload[name] = value;
            payload
        };
        for (payload, now, why) in [
            (
                changed("iss", "https://evil.example".into()),
                while_valid(),
                "not Google",
            ),
            (
                changed("aud", "another.apps.googleusercontent.com".into()),
                while_valid(),
                "not this client",
            ),
            (
                example.clone(),
                chrono::DateTime::from_timestamp(1_353_604_926, 0).unwrap(),
                "expired",
            ),
            (changed("nonce", "another".into()), while_valid(), "nonce"),
        ] {
            let (token, _) = token_endpoint(payload).await;

            let refused = google(token)
                .finish(
                    &reqwest::Client::new(),
                    "4/P7q7W91",
                    &callback(),
                    &BEGUN,
                    now,
                )
                .await
                .unwrap_err();

            assert!(format!("{refused:#}").contains(why), "{refused:#}");
        }
    }

    #[test_log::test]
    fn the_authorization_request_asks_for_a_code_with_s256_pkce_and_the_nonce() {
        let url = google(GoogleEndpoints::default().token).authorize_url(&callback(), &BEGUN);

        let query: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        assert_eq!(
            url.as_str().split('?').next(),
            Some("https://accounts.google.com/o/oauth2/v2/auth")
        );
        assert_eq!(
            query,
            [
                ("response_type", "code"),
                ("client_id", "1234987819200.apps.googleusercontent.com"),
                (
                    "redirect_uri",
                    "https://lw.example.com/auth/google/callback"
                ),
                ("scope", "openid email profile"),
                ("state", "state"),
                (
                    "code_challenge",
                    "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
                ),
                ("code_challenge_method", "S256"),
                ("nonce", "0394852-3190485-2490358"),
                ("prompt", "select_account"),
            ]
            .map(|(name, value)| (name.to_string(), value.to_string()))
        );
    }
}
