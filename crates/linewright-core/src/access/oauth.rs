//! Linewright as its own OAuth 2.1 authorization server for the MCP server
//! (`docs/specs/access.md`, The authorization server and Tokens): the
//! decisions on an authorization request and on a token request, made from
//! what the database holds, which [`store`] reads and writes, and the
//! clients, which [`clients`] registers and fetches.
//! <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13>

pub mod clients;
pub mod metadata_document;
pub mod store;

use std::fmt;

use chrono::TimeDelta;
use serde::{Deserialize, Serialize};
use url::{Host, Url};

use self::clients::Found;
use super::tokens::s256;
use crate::domain::Timestamp;
use crate::host::PublicUrl;

/// How long an access token works after Linewright issues it.
pub const ACCESS_TOKEN_LASTS: TimeDelta = TimeDelta::hours(1);

/// How long an authorization lasts from its sign-in.
pub const AUTHORIZATION_LASTS: TimeDelta = TimeDelta::days(30);

/// How long a code waits for its client to exchange it.
/// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13#section-4.1.2>
pub const CODE_LASTS: TimeDelta = TimeDelta::minutes(10);

/// The path of the MCP server, the one resource Linewright guards.
pub const MCP_PATH: &str = "/mcp";

/// Where clients find Linewright as their authorization server, all under
/// its public URL.
pub const AUTHORIZE_PATH: &str = "/oauth/authorize";
pub const TOKEN_PATH: &str = "/oauth/token";
pub const REGISTER_PATH: &str = "/oauth/register";
pub const SERVER_METADATA_PATH: &str = "/.well-known/oauth-authorization-server";
pub const RESOURCE_METADATA_PATH: &str = "/.well-known/oauth-protected-resource/mcp";

/// Linewright as an authorization server: its issuer and the MCP server's
/// URI, both from the public URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    public_url: PublicUrl,
}

impl Server {
    pub fn new(public_url: PublicUrl) -> Server {
        Server { public_url }
    }

    /// Its issuer identifier: the public URL, without the `/` a URL with no
    /// path has, as clients compare it with the URL they found it at.
    /// <https://www.rfc-editor.org/rfc/rfc8414#section-2>
    pub fn issuer(&self) -> String {
        self.public_url
            .join("/")
            .as_str()
            .trim_end_matches('/')
            .to_string()
    }

    /// The MCP server's URI, which every token is issued for (RFC 8707).
    pub fn resource(&self) -> Url {
        self.public_url.join(MCP_PATH)
    }

    /// Where its protected resource metadata is, which a request to the
    /// MCP server without a valid access token is pointed to.
    pub fn resource_metadata_url(&self) -> String {
        self.url(RESOURCE_METADATA_PATH)
    }

    fn url(&self, path: &str) -> String {
        self.public_url.join(path).to_string()
    }

    /// Its authorization server metadata.
    /// <https://www.rfc-editor.org/rfc/rfc8414#section-2>,
    /// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-client-id-metadata-document-00#section-5>
    pub fn server_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "issuer": self.issuer(),
            "authorization_endpoint": self.url(AUTHORIZE_PATH),
            "token_endpoint": self.url(TOKEN_PATH),
            "registration_endpoint": self.url(REGISTER_PATH),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
            "client_id_metadata_document_supported": true,
            "authorization_response_iss_parameter_supported": true,
        })
    }

    /// The MCP server's protected resource metadata.
    /// <https://www.rfc-editor.org/rfc/rfc9728#section-2>
    pub fn resource_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "resource": self.resource().as_str(),
            "authorization_servers": [self.issuer()],
            "bearer_methods_supported": ["header"],
            "resource_name": "Linewright",
        })
    }

    /// Whether it issues tokens for `resource`, the one a request asks for
    /// if any: none but the MCP server (`docs/specs/access.md`, The
    /// authorization server, Resource).
    /// <https://www.rfc-editor.org/rfc/rfc8707#section-2>
    fn takes_resource(&self, resource: Option<&str>) -> Result<(), OAuthError> {
        match resource {
            Some(resource) if resource != self.resource().as_str() => Err(OAuthError::new(
                "invalid_target",
                format!("{resource:?} is not the MCP server's URI"),
            )),
            _ => Ok(()),
        }
    }

    /// Where `redirect_uri` sends the browser back to its client with
    /// `params`, with the client's `state` and Linewright's issuer.
    /// <https://www.rfc-editor.org/rfc/rfc9207#section-2>
    pub fn sent_back(
        &self,
        redirect_uri: &RedirectUri,
        params: &[(&str, &str)],
        state: Option<&str>,
    ) -> Url {
        let mut url = redirect_uri.url.clone();
        {
            let mut query = url.query_pairs_mut();
            query.extend_pairs(params);
            if let Some(state) = state {
                query.append_pair("state", state);
            }
            query.append_pair("iss", &self.issuer());
        }
        url
    }

    /// Where `redirect_uri` sends the browser back to its client with
    /// `error`, and the client's `state`.
    /// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13#section-4.1.2.1>
    pub fn sent_back_error(
        &self,
        redirect_uri: &RedirectUri,
        error: &OAuthError,
        state: Option<&str>,
    ) -> Url {
        self.sent_back(
            redirect_uri,
            &[
                ("error", error.error),
                ("error_description", &error.error_description),
            ],
            state,
        )
    }
}

/// A client's ID: the one Linewright gave it as it registered, or the URL
/// of its metadata document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientId(String);

impl ClientId {
    pub fn of(id: String) -> ClientId {
        ClientId(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A redirect URI a client may register: HTTPS, or HTTP to a loopback
/// address, `localhost` included, without a fragment, kept as the client
/// gave it (`docs/specs/access.md`, The authorization server, Loopback).
/// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13#section-2.3.1>
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedirectUri {
    given: String,
    url: Url,
    /// What it is but for its port, for an HTTP one to a loopback address
    /// without user information.
    portless: Option<Portless>,
}

/// A loopback redirect URI but for its port: its host, and what follows its
/// authority as the client gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Portless {
    host: Host,
    after_authority: String,
}

impl Portless {
    /// What `given`, which parses to `url`, is but for its port, if it is
    /// an HTTP redirect URI to a loopback address without user information.
    fn of(given: &str, url: &Url) -> Option<Portless> {
        const SCHEME: &str = "http://";
        let user = !url.username().is_empty() || url.password().is_some();
        if url.scheme() != "http" || !to_loopback(url) || user {
            return None;
        }
        let rest = given
            .get(..SCHEME.len())
            .filter(|scheme| scheme.eq_ignore_ascii_case(SCHEME))
            .map(|_| &given[SCHEME.len()..])?;
        let authority_ends = rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len());
        Some(Portless {
            host: url.host()?.to_owned(),
            after_authority: rest[authority_ends..].to_string(),
        })
    }
}

impl RedirectUri {
    /// The redirect URI `given` is, or else why not.
    pub fn parse(given: &str) -> Result<RedirectUri, String> {
        let url = Url::parse(given).map_err(|e| format!("{given:?} is not a URL: {e}"))?;
        let secure = match url.scheme() {
            "https" => url.host().is_some(),
            "http" => to_loopback(&url),
            _ => false,
        };
        if !secure {
            return Err(format!(
                "{given:?} is neither HTTPS nor HTTP to a loopback address"
            ));
        }
        if url.fragment().is_some() {
            return Err(format!("{given:?} has a fragment"));
        }
        Ok(RedirectUri {
            given: given.to_string(),
            portless: Portless::of(given, &url),
            url,
        })
    }

    /// It, as the client gave it.
    pub fn as_str(&self) -> &str {
        &self.given
    }

    /// The redirect URI `requested` is, if a request may give it for this
    /// registered one: exactly this one, but that an HTTP one to a loopback
    /// address may have any port, or none, on the same host.
    /// <https://www.rfc-editor.org/rfc/rfc8252#section-7.3>
    pub fn admits(&self, requested: &str) -> Option<RedirectUri> {
        let requested = RedirectUri::parse(requested).ok()?;
        let admitted = requested.given == self.given
            || (self.portless.is_some() && requested.portless == self.portless);
        admitted.then_some(requested)
    }

    /// Its host.
    pub fn host(&self) -> &str {
        self.url.host_str().unwrap_or_default()
    }

    /// Whether it leads to an app on the person's own computer: to a
    /// loopback address, `localhost` among them (`docs/specs/access.md`,
    /// The authorization server, Loopback).
    pub fn is_loopback(&self) -> bool {
        to_loopback(&self.url)
    }
}

/// Whether `url` is to a loopback address, `localhost` included.
fn to_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// A client: its ID, the name it registered, and its redirect URIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client {
    pub id: ClientId,
    pub name: Option<String>,
    pub redirect_uris: Vec<RedirectUri>,
}

/// What a client without a name is called.
pub const UNNAMED: &str = "an unnamed client";

/// `name`, or else [`UNNAMED`].
pub fn client_name(name: Option<&str>) -> &str {
    name.unwrap_or(UNNAMED)
}

/// An error Linewright sends a client: its code and why.
/// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13#section-4.1.2.1>,
/// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13#section-3.2.4>
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OAuthError {
    pub error: &'static str,
    pub error_description: String,
}

impl OAuthError {
    pub fn new(error: &'static str, description: impl Into<String>) -> OAuthError {
        OAuthError {
            error,
            error_description: description.into(),
        }
    }

    pub fn invalid_request(description: impl Into<String>) -> OAuthError {
        OAuthError::new("invalid_request", description)
    }

    fn invalid_grant(description: impl Into<String>) -> OAuthError {
        OAuthError::new("invalid_grant", description)
    }

    /// The error Deny sends the client (`docs/specs/access.md`, The
    /// authorization server, 4a).
    pub fn access_denied() -> OAuthError {
        OAuthError::new("access_denied", "the person denied the client")
    }
}

impl fmt::Display for OAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.error, self.error_description)
    }
}

/// An authorization request, as the client sends the browser with it.
/// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13#section-4.1.1>
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthorizeRequest {
    pub response_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub state: Option<String>,
    pub resource: Option<String>,
    /// The first parameter it gives more than once, which it then holds
    /// none of.
    pub repeated: Option<String>,
}

impl AuthorizeRequest {
    /// The request `query` makes, ignoring the parameters it does not take.
    /// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13#section-3.1>
    pub fn from_query(query: &str) -> AuthorizeRequest {
        let mut request = AuthorizeRequest::default();
        let mut given = std::collections::BTreeMap::<String, Vec<String>>::new();
        for (name, value) in form_urlencoded::parse(query.as_bytes()) {
            given
                .entry(name.into_owned())
                .or_default()
                .push(value.into_owned());
        }
        let mut take = |name: &str| match given.remove(name).as_deref() {
            Some([value]) => Some(value.clone()),
            Some(_) => {
                request.repeated.get_or_insert_with(|| name.to_string());
                None
            }
            None => None,
        };
        let response_type = take("response_type");
        let client_id = take("client_id");
        let redirect_uri = take("redirect_uri");
        let code_challenge = take("code_challenge");
        let code_challenge_method = take("code_challenge_method");
        let state = take("state");
        let resource = take("resource");
        AuthorizeRequest {
            response_type,
            client_id,
            redirect_uri,
            code_challenge,
            code_challenge_method,
            state,
            resource,
            ..request
        }
    }
}

/// An `S256` PKCE code challenge: a SHA-256, base64url-encoded without
/// padding.
/// <https://www.rfc-editor.org/rfc/rfc7636#section-4.2>
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Challenge(String);

impl Challenge {
    /// The challenge `given` is, if it is one.
    pub fn parse(given: &str) -> Option<Challenge> {
        let hash = given.len() == 43
            && given
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        hash.then(|| Challenge(given.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `verifier` is a PKCE code verifier whose challenge it is.
    /// <https://www.rfc-editor.org/rfc/rfc7636#section-4.1>,
    /// <https://www.rfc-editor.org/rfc/rfc7636#section-4.6>
    fn verified_by(&self, verifier: &str) -> bool {
        (43..=128).contains(&verifier.len())
            && verifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
            && s256(verifier) == self.0
    }
}

/// An authorization request Linewright takes: the client, where it sends
/// the browser back, and the PKCE challenge of the code it gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub client: ClientId,
    pub redirect_uri: RedirectUri,
    pub challenge: Challenge,
    pub state: Option<String>,
}

/// Why Linewright does not take an authorization request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthorizeError {
    /// Linewright cannot send the browser back, and shows Authorization
    /// error (`docs/specs/access.md`, The authorization server, 2a).
    CannotSendBack(CannotSendBack),
    /// Linewright sends the browser back to `redirect_uri` with `error`.
    SendBack(Box<SentBack>),
}

/// An authorization request's error, and where Linewright sends it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentBack {
    pub redirect_uri: RedirectUri,
    pub state: Option<String>,
    pub error: OAuthError,
}

/// Why Linewright cannot send the browser back to the client
/// (`docs/specs/access.md`, Authorization error, The message).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CannotSendBack {
    #[error("Linewright does not know this client.")]
    UnknownClient,
    #[error("Linewright cannot use this client's metadata document.")]
    UnusableMetadataDocument,
    #[error("This is not a redirect URI of this client.")]
    NotARedirectUri,
}

/// Whether Linewright takes `request` from `client`, what Linewright found
/// for its `client_id`, for the resource that `server` guards: a redirect
/// URI that one the client registered admits, which the request keeps as it
/// gave it, the code grant, an `S256` PKCE challenge, and no resource but
/// the MCP server's (`docs/specs/access.md`, The authorization server).
pub fn check_authorize(
    request: &AuthorizeRequest,
    client: &Found,
    server: &Server,
) -> Result<Checked, AuthorizeError> {
    let client = match client {
        Found::Client(client) if request.client_id.as_deref() == Some(client.id.as_str()) => client,
        Found::UnusableDocument => {
            return Err(AuthorizeError::CannotSendBack(
                CannotSendBack::UnusableMetadataDocument,
            ));
        }
        Found::Client(_) | Found::Unknown => {
            return Err(AuthorizeError::CannotSendBack(
                CannotSendBack::UnknownClient,
            ));
        }
    };
    let redirect_uri = request
        .redirect_uri
        .as_deref()
        .and_then(|requested| {
            client
                .redirect_uris
                .iter()
                .find_map(|registered| registered.admits(requested))
        })
        .ok_or(AuthorizeError::CannotSendBack(
            CannotSendBack::NotARedirectUri,
        ))?;
    let send_back = |error| {
        AuthorizeError::SendBack(Box::new(SentBack {
            redirect_uri: redirect_uri.clone(),
            state: request.state.clone(),
            error,
        }))
    };
    if let Some(name) = &request.repeated {
        return Err(send_back(OAuthError::invalid_request(format!(
            "`{name}` is given more than once"
        ))));
    }
    if request.response_type.as_deref() != Some("code") {
        return Err(send_back(OAuthError::new(
            "unsupported_response_type",
            "the response type is `code`",
        )));
    }
    let Some(challenge) = &request.code_challenge else {
        return Err(send_back(OAuthError::invalid_request(
            "a PKCE code challenge is required",
        )));
    };
    if request.code_challenge_method.as_deref() != Some("S256") {
        return Err(send_back(OAuthError::invalid_request(
            "the PKCE method is `S256`",
        )));
    }
    let Some(challenge) = Challenge::parse(challenge) else {
        return Err(send_back(OAuthError::invalid_request(
            "the PKCE code challenge is not an `S256` one",
        )));
    };
    server
        .takes_resource(request.resource.as_deref())
        .map_err(send_back)?;
    Ok(Checked {
        client: client.id.clone(),
        redirect_uri,
        challenge,
        state: request.state.clone(),
    })
}

/// A token request's form, as the client posts it.
/// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13#section-3.2.2>
#[derive(Clone, Debug, Deserialize)]
pub struct TokenForm {
    pub grant_type: Option<String>,
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_verifier: Option<String>,
    pub client_id: Option<String>,
    pub refresh_token: Option<String>,
    pub resource: Option<String>,
}

/// A token request of a grant Linewright takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenRequest {
    pub grant: Grant,
    pub client_id: ClientId,
    pub resource: Option<String>,
}

/// What a token request exchanges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Grant {
    Code {
        code: String,
        redirect_uri: Option<String>,
        verifier: Option<String>,
    },
    Refresh {
        token: String,
    },
}

impl TokenRequest {
    /// The request `form` makes, or else the error it is answered with.
    pub fn parse(form: TokenForm) -> Result<TokenRequest, OAuthError> {
        let client_id = form
            .client_id
            .map(ClientId)
            .ok_or_else(|| OAuthError::invalid_request("a public client sends its `client_id`"))?;
        let grant = match form.grant_type.as_deref() {
            Some("authorization_code") => Grant::Code {
                code: form
                    .code
                    .ok_or_else(|| OAuthError::invalid_request("the code is missing"))?,
                redirect_uri: form.redirect_uri,
                verifier: form.code_verifier,
            },
            Some("refresh_token") => Grant::Refresh {
                token: form
                    .refresh_token
                    .ok_or_else(|| OAuthError::invalid_request("the refresh token is missing"))?,
            },
            Some(other) => {
                return Err(OAuthError::new(
                    "unsupported_grant_type",
                    format!("Linewright does not take the grant {other:?}"),
                ));
            }
            None => return Err(OAuthError::invalid_request("the grant type is missing")),
        };
        Ok(TokenRequest {
            grant,
            client_id,
            resource: form.resource,
        })
    }
}

/// An authorization's number (`docs/specs/access.md`, The authorization
/// server, An authorization's ID).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthorizationId(i64);

impl AuthorizationId {
    pub fn row(self) -> i64 {
        self.0
    }

    pub(crate) fn of_row(row: i64) -> AuthorizationId {
        AuthorizationId(row)
    }
}

impl std::str::FromStr for AuthorizationId {
    type Err = String;

    fn from_str(id: &str) -> Result<AuthorizationId, String> {
        match id.parse::<i64>() {
            Ok(n) if n > 0 && !id.starts_with('+') => Ok(AuthorizationId(n)),
            _ => Err(format!("{id:?} is no authorization's ID")),
        }
    }
}

impl fmt::Display for AuthorizationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The authorization a code or a refresh token is of, as the database
/// holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredAuthorization {
    pub id: AuthorizationId,
    pub client: ClientId,
    pub ends_at: Timestamp,
    pub ended: bool,
}

/// When an authorization whose person signed in for it at `signed_in_at`
/// ends, unless it ends earlier (`docs/specs/access.md`, The authorization
/// server, How long it lasts).
pub fn authorization_ends(signed_in_at: Timestamp) -> Timestamp {
    signed_in_at + AUTHORIZATION_LASTS
}

impl StoredAuthorization {
    /// Whether it ended, or ran out, by `now`.
    fn over(&self, now: Timestamp) -> bool {
        self.ended || self.ends_at <= now
    }
}

/// Where a refresh token stands in its authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshState {
    /// The latest issued, which works once.
    Current,
    /// One that a token request used, whose second use ends its
    /// authorization.
    Used,
}

impl RefreshState {
    pub(crate) fn name(self) -> &'static str {
        match self {
            RefreshState::Current => "current",
            RefreshState::Used => "used",
        }
    }

    pub(crate) fn named(name: &str) -> Option<RefreshState> {
        [RefreshState::Current, RefreshState::Used]
            .into_iter()
            .find(|state| state.name() == name)
    }
}

/// What the database holds of the code or the refresh token a token
/// request presents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stored {
    /// Nothing: Linewright never issued it.
    Unknown,
    Code {
        authorization: StoredAuthorization,
        challenge: Challenge,
        redirect_uri: RedirectUri,
        expires_at: Timestamp,
        used: bool,
    },
    Refresh {
        authorization: StoredAuthorization,
        state: RefreshState,
    },
}

/// What Linewright does with a token request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenDecision {
    /// Issues an access token that works until `access_until`, and a
    /// refresh token, which becomes its authorization's current one, in
    /// `authorization`, for `used`.
    Issue {
        authorization: AuthorizationId,
        used: Used,
        access_until: Timestamp,
    },
    /// Ends `authorization`, and answers with `error`.
    End {
        authorization: AuthorizationId,
        error: OAuthError,
    },
    Refuse(OAuthError),
}

/// What a token request that is issued tokens used up, which works no
/// more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Used {
    Code,
    /// The current refresh token, which becomes a used one.
    Refresh,
}

/// What Linewright does at `now` with `request`, which presents what
/// `stored` holds, for the resource that `server` guards
/// (`docs/specs/access.md`, The authorization server: Grants, A code, An
/// access token, An ended authorization and A second use).
pub fn exchange(
    request: &TokenRequest,
    stored: &Stored,
    now: Timestamp,
    server: &Server,
) -> TokenDecision {
    let refuse = TokenDecision::Refuse;
    if let Err(error) = server.takes_resource(request.resource.as_deref()) {
        return refuse(error);
    }
    let issue = |authorization: &StoredAuthorization, used| TokenDecision::Issue {
        authorization: authorization.id,
        used,
        access_until: (now + ACCESS_TOKEN_LASTS).min(authorization.ends_at),
    };
    match (&request.grant, stored) {
        (
            Grant::Code {
                redirect_uri: sent_redirect,
                verifier,
                ..
            },
            Stored::Code {
                authorization,
                challenge,
                redirect_uri,
                expires_at,
                used,
            },
        ) => {
            if authorization.over(now) {
                return refuse(OAuthError::invalid_grant("the authorization ended"));
            }
            if *used {
                return TokenDecision::End {
                    authorization: authorization.id,
                    error: OAuthError::invalid_grant(
                        "the code was used before, which ends its authorization",
                    ),
                };
            }
            if authorization.client != request.client_id {
                return refuse(OAuthError::invalid_grant("the code is another client's"));
            }
            if *expires_at <= now {
                return refuse(OAuthError::invalid_grant("the code expired"));
            }
            if sent_redirect.as_deref() != Some(redirect_uri.as_str()) {
                return refuse(OAuthError::invalid_grant(
                    "the redirect URI is not the authorization request's",
                ));
            }
            let Some(verifier) = verifier else {
                return refuse(OAuthError::invalid_request(
                    "the PKCE code verifier is missing",
                ));
            };
            if !challenge.verified_by(verifier) {
                return refuse(OAuthError::invalid_grant(
                    "the PKCE code verifier does not match the challenge",
                ));
            }
            issue(authorization, Used::Code)
        }
        (
            Grant::Refresh { .. },
            Stored::Refresh {
                authorization,
                state,
            },
        ) => {
            if authorization.over(now) {
                return refuse(OAuthError::invalid_grant("the authorization ended"));
            }
            if *state == RefreshState::Used {
                return TokenDecision::End {
                    authorization: authorization.id,
                    error: OAuthError::invalid_grant(
                        "the refresh token was used before, which ends its authorization",
                    ),
                };
            }
            if authorization.client != request.client_id {
                return refuse(OAuthError::invalid_grant(
                    "the refresh token is another client's",
                ));
            }
            issue(authorization, Used::Refresh)
        }
        (Grant::Code { .. }, _) => refuse(OAuthError::invalid_grant("no such code")),
        (Grant::Refresh { .. }, _) => refuse(OAuthError::invalid_grant("no such refresh token")),
    }
}

/// Whether an access token that works until `expires_at`, of
/// `authorization`, acts at `now`: no longer once it expired or its
/// authorization is over (`docs/specs/access.md`, The authorization server,
/// An access token and An ended authorization).
pub fn access_works(
    expires_at: Timestamp,
    authorization: &StoredAuthorization,
    now: Timestamp,
) -> bool {
    expires_at > now && !authorization.over(now)
}

/// A client's registration (RFC 7591), as it posts it.
/// <https://www.rfc-editor.org/rfc/rfc7591#section-2>
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RegistrationRequest {
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    pub client_name: Option<String>,
}

/// The longest client name Linewright keeps, in characters.
const LONGEST_NAME: usize = 200;

/// The most redirect URIs Linewright keeps of a client.
const MOST_REDIRECT_URIS: usize = 20;

/// The name and redirect URIs `request` registers, or else why Linewright
/// refuses it: a redirect URI that is not HTTPS or HTTP to a loopback
/// address, or none (`docs/specs/access.md`, The authorization server,
/// Loopback). What else it asks for, such as a client secret, it does
/// not get: every client is public.
pub fn registration(
    request: &RegistrationRequest,
) -> Result<(Option<String>, Vec<RedirectUri>), OAuthError> {
    let redirect_uris = redirect_uris(&request.redirect_uris)
        .map_err(|why| OAuthError::new("invalid_redirect_uri", why))?;
    let name = client_name_of(request.client_name.clone())
        .map_err(|why| OAuthError::new("invalid_client_metadata", why))?;
    Ok((name, redirect_uris))
}

/// The name a client gives as `given`, none for an empty one, or else why
/// Linewright does not keep it.
pub(crate) fn client_name_of(given: Option<String>) -> Result<Option<String>, String> {
    let name = given.filter(|name| !name.is_empty());
    if name
        .as_ref()
        .is_some_and(|name| name.chars().count() > LONGEST_NAME)
    {
        return Err(format!(
            "the client name is longer than {LONGEST_NAME} characters"
        ));
    }
    Ok(name)
}

/// The redirect URIs of `given`, or else why one is none.
pub(crate) fn redirect_uris(given: &[String]) -> Result<Vec<RedirectUri>, String> {
    if given.is_empty() {
        return Err("a client registers at least one redirect URI".into());
    }
    if given.len() > MOST_REDIRECT_URIS {
        return Err(format!(
            "a client registers at most {MOST_REDIRECT_URIS} redirect URIs"
        ));
    }
    given.iter().map(|uri| RedirectUri::parse(uri)).collect()
}

#[cfg(test)]
mod tests;
