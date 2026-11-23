//! Linewright as the OAuth authorization server of the MCP server
//! (`docs/specs/access.md`, The authorization server, Tokens, Consent and
//! Authorization error): its metadata, registration, the authorization
//! request through Sign in to Consent, and the token endpoint.

use std::sync::Arc;

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tracing::{error, info};

use linewright_core::access::Caller;
use linewright_core::access::oauth::clients::Clients;
use linewright_core::access::oauth::store::{
    Answered, AuthorizationRequests, Decision, Tokens, Waiting,
};
use linewright_core::access::oauth::{
    AUTHORIZE_PATH, AuthorizeError, AuthorizeRequest, CannotSendBack, OAuthError, RedirectUri,
    RegistrationRequest, SentBack, Server, TokenForm, TokenRequest, check_authorize, client_name,
    registration,
};
use linewright_core::access::sessions::Resolved;
use linewright_core::api::Sources;

use super::caller::{CrossSite, session_cookie, sessions};
use super::card_page::CardPage;
use super::sign_in::{ReturnPath, refused_from_another_site, sign_in_path};

/// The field of Consent's form, and of its path's query, that names the
/// authorization request waiting for its person: none of the names an
/// authorization request from a client takes, such as `request`.
/// <https://www.rfc-editor.org/rfc/rfc9101#section-5>
const REQUEST: &str = "waiting";

/// The field of Consent's form that holds "allow" or "deny".
const DECISION: &str = "decision";

/// Consent's form, as it posts it, its fields named [`REQUEST`] and
/// [`DECISION`].
#[derive(Deserialize)]
struct ConsentForm {
    #[serde(rename = "waiting")]
    secret: String,
    decision: Decision,
}

/// The path of Consent of the authorization request `secret` names.
fn consent_path(secret: &str) -> String {
    format!("{AUTHORIZE_PATH}?{REQUEST}={secret}")
}

/// The secret of the authorization request whose Consent `return_to` is,
/// if it is one's.
pub(super) fn waiting_request(return_to: &ReturnPath) -> Option<&str> {
    return_to
        .as_str()
        .strip_prefix(AUTHORIZE_PATH)?
        .strip_prefix('?')?
        .strip_prefix(REQUEST)?
        .strip_prefix('=')
        .filter(|secret| !secret.contains(['&', '=']))
}

/// The authorization server metadata (RFC 8414).
pub(super) async fn server_metadata(Extension(server): Extension<Arc<Server>>) -> Response {
    json(StatusCode::OK, &server.server_metadata())
}

/// The MCP server's protected resource metadata (RFC 9728).
pub(super) async fn resource_metadata(Extension(server): Extension<Arc<Server>>) -> Response {
    json(StatusCode::OK, &server.resource_metadata())
}

/// `body` as JSON, with `status`.
fn json(status: StatusCode, body: &impl serde::Serialize) -> Response {
    let body = serde_json::to_string(body).expect("what Linewright answers serializes");
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// `body` as JSON, with `status`, kept out of every cache, as an answer
/// holding tokens is.
/// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13#section-3.2.3>
fn uncached_json(status: StatusCode, body: &impl serde::Serialize) -> Response {
    let mut response = json(status, body);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The answer to a client's request that Linewright refuses with `error`,
/// which it logs as `what` refused: 400.
/// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13#section-3.2.4>
fn refused(what: &str, error: &OAuthError) -> Response {
    info!(reason = %error, "refused {what}");
    uncached_json(StatusCode::BAD_REQUEST, error)
}

/// The answer to a request that failed for `e`, which it logs.
fn failed(what: &str, e: &anyhow::Error) -> Response {
    log_failed(what, e);
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{what} failed")).into_response()
}

/// The answer to a client's request that failed for `e`, which it logs:
/// 500, with the error a client reads.
/// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13#section-4.1.2.1>
fn failed_for_client(what: &str, e: &anyhow::Error) -> Response {
    log_failed(what, e);
    let error = OAuthError::new("server_error", format!("{what} failed"));
    uncached_json(StatusCode::INTERNAL_SERVER_ERROR, &error)
}

fn log_failed(what: &str, e: &anyhow::Error) {
    let error = format!("{e:#}");
    error!(error, "{what} failed");
}

/// Registers the client `body` describes as a public client
/// (`docs/specs/access.md`, The authorization server, Clients): 201 with
/// its registration, or 400 with why not.
/// <https://www.rfc-editor.org/rfc/rfc7591#section-3.2>
pub(super) async fn register(State(sources): State<Sources>, body: Bytes) -> Response {
    let request: RegistrationRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(e) => {
            let error = OAuthError::new(
                "invalid_client_metadata",
                format!("the registration does not read: {e}"),
            );
            return refused("a registration", &error);
        }
    };
    let (name, redirect_uris) = match registration(&request) {
        Ok(registered) => registered,
        Err(error) => return refused("a registration", &error),
    };
    let now = sources.clock.now();
    let client = match Clients::new(sources.runs.database().clone())
        .register(name, redirect_uris, now)
        .await
    {
        Ok(client) => client,
        Err(e) => return failed_for_client("registering a client", &e),
    };
    let mut registered = serde_json::json!({
        "client_id": client.id.as_str(),
        "client_id_issued_at": now.timestamp(),
        "redirect_uris": client.redirect_uris.iter().map(RedirectUri::as_str).collect::<Vec<_>>(),
        "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
    });
    if let Some(registered_name) = &client.name {
        registered["client_name"] = registered_name.as_str().into();
    }
    uncached_json(StatusCode::CREATED, &registered)
}

/// Answers a token request (`docs/specs/access.md`, Tokens): the tokens it
/// is issued, or 400 with why not.
pub(super) async fn token(
    State(sources): State<Sources>,
    Extension(server): Extension<Arc<Server>>,
    body: Bytes,
) -> Response {
    let form: TokenForm = match serde_urlencoded::from_bytes(&body) {
        Ok(form) => form,
        Err(e) => {
            let error = OAuthError::invalid_request(format!("the request does not read: {e}"));
            return refused("a token request", &error);
        }
    };
    let request = match TokenRequest::parse(form) {
        Ok(request) => request,
        Err(error) => return refused("a token request", &error),
    };
    let answered = Tokens::new(sources.runs.database().clone())
        .answer(&request, &server, sources.clock.now())
        .await;
    match answered {
        Ok(Ok(issued)) => uncached_json(StatusCode::OK, &issued),
        Ok(Err(error)) => refused(
            &format!("a token request of client {}", request.client_id),
            &error,
        ),
        Err(e) => failed_for_client("issuing tokens", &e),
    }
}

/// What Linewright says on Authorization error.
#[derive(Clone, Copy, Debug)]
enum CannotAuthorize {
    CannotSendBack(CannotSendBack),
    /// The authorization request waited too long, or was answered.
    NoLongerWaits,
}

impl std::fmt::Display for CannotAuthorize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CannotAuthorize::CannotSendBack(why) => why.fmt(f),
            CannotAuthorize::NoLongerWaits => {
                f.write_str("This authorization request has ended. Start again from the client.")
            }
        }
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "authorization_error.html")]
struct AuthorizationErrorPage {
    card: CardPage,
    message: String,
}

/// Authorization error saying `why`, answered with 400.
fn authorization_error(sources: &Sources, why: CannotAuthorize) -> Response {
    let page = AuthorizationErrorPage {
        card: CardPage::new(
            "Authorization error",
            "authorization-error",
            "Linewright cannot authorize this client",
            sources,
        ),
        message: why.to_string(),
    };
    (StatusCode::BAD_REQUEST, page).into_response()
}

#[derive(Template, WebTemplate)]
#[template(path = "consent.html")]
struct ConsentPage {
    card: CardPage,
    facts: Vec<(&'static str, String)>,
    request: String,
}

/// Consent of `waiting`, the request `secret` names, for `caller`, who
/// signed in for it (`docs/specs/access.md`, Consent).
fn consent(sources: &Sources, caller: &Caller, waiting: &Waiting, secret: &str) -> Response {
    let person = caller.person();
    ConsentPage {
        card: CardPage::new(
            "Consent",
            "consent",
            format!(
                "Allow {} to act as you?",
                client_name(waiting.client_name.as_deref())
            ),
            sources,
        ),
        facts: vec![
            ("Signed in as", person.shown_as.clone()),
            ("Sends you back to", waiting.redirect_uri.host().to_string()),
            ("Access", caller.role().name().to_string()),
        ],
        request: secret.to_string(),
    }
    .into_response()
}

/// Who the browser of `headers` signs in now, if anyone.
async fn browser_caller(sources: &Sources, headers: &HeaderMap) -> anyhow::Result<Option<Caller>> {
    let Some(cookie) = session_cookie(headers) else {
        return Ok(None);
    };
    Ok(
        match sessions(sources)
            .resolve(&cookie, sources.clock.now())
            .await?
        {
            Resolved::Caller(caller) => Some(caller),
            Resolved::Ended | Resolved::NoOne => None,
        },
    )
}

/// Keeps the browser's address from the client: an authorization request's
/// pages carry its secret.
fn without_referrer(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

/// An authorization request (`docs/specs/access.md`, The authorization
/// server, 2): one from a client goes to Sign in, then Consent, unless
/// Linewright refuses it; one that names a request waiting for its person
/// is Consent, for the person who signed in for it in this browser.
pub(super) async fn authorize(
    State(sources): State<Sources>,
    Extension(server): Extension<Arc<Server>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let query = query.unwrap_or_default();
    let answer = match consent_of(&query) {
        Some(secret) => consent_or_sign_in(&sources, &headers, &secret).await,
        None => from_client(&sources, &server, &query).await,
    };
    without_referrer(answer)
}

/// The secret of the authorization request whose Consent `query` asks
/// for, if it asks for one rather than being a client's request.
fn consent_of(query: &str) -> Option<String> {
    let mut pairs = form_urlencoded::parse(query.as_bytes());
    match (pairs.next(), pairs.next()) {
        (Some((name, secret)), None) if name == REQUEST => Some(secret.into_owned()),
        _ => None,
    }
}

/// Consent of the request `secret` names, for the person who signed in
/// for it in the browser of `headers`, or else Sign in for it.
async fn consent_or_sign_in(sources: &Sources, headers: &HeaderMap, secret: &str) -> Response {
    let now = sources.clock.now();
    let requests = AuthorizationRequests::new(sources.runs.database().clone());
    let waiting = match requests.get(secret, now).await {
        Ok(Some(waiting)) => waiting,
        Ok(None) => {
            info!("refused an authorization request that no longer waits");
            return authorization_error(sources, CannotAuthorize::NoLongerWaits);
        }
        Err(e) => return failed("reading an authorization request", &e),
    };
    let caller = match browser_caller(sources, headers).await {
        Ok(caller) => caller,
        Err(e) => return failed("reading a browser's sign-in", &e),
    };
    match caller {
        Some(caller) if waiting.signed_in_by(caller.person().id) => {
            consent(sources, &caller, &waiting, secret)
        }
        _ => to_sign_in(secret),
    }
}

/// Sign in, for the request `secret` names.
fn to_sign_in(secret: &str) -> Response {
    let return_to = ReturnPath::parse(Some(&consent_path(secret)));
    Redirect::to(&sign_in_path(&return_to, None)).into_response()
}

/// The authorization request `query` makes, from a client: to Sign in for
/// it once Linewright takes it, to the client with why not where it can
/// send the browser back, and else Authorization error.
async fn from_client(sources: &Sources, server: &Server, query: &str) -> Response {
    let now = sources.clock.now();
    let request = AuthorizeRequest::from_query(query);
    let client = match &request.client_id {
        Some(id) => {
            Clients::new(sources.runs.database().clone())
                .find(id, now)
                .await
        }
        None => Ok(None),
    };
    let client = match client {
        Ok(client) => client,
        Err(e) => return failed("reading a client", &e),
    };
    match check_authorize(&request, client.as_ref(), server) {
        Ok(checked) => {
            match AuthorizationRequests::new(sources.runs.database().clone())
                .begin(&checked, now)
                .await
            {
                Ok(secret) => to_sign_in(&secret),
                Err(e) => failed("keeping an authorization request", &e),
            }
        }
        Err(AuthorizeError::CannotSendBack(why)) => {
            info!(
                client = request.client_id.as_deref().unwrap_or_default(),
                reason = %why,
                "refused an authorization request"
            );
            authorization_error(sources, CannotAuthorize::CannotSendBack(why))
        }
        Err(AuthorizeError::SendBack(sent)) => {
            let SentBack {
                redirect_uri,
                state,
                error,
            } = *sent;
            info!(
                client = request.client_id.as_deref().unwrap_or_default(),
                reason = %error,
                "refused an authorization request, sending it back"
            );
            let back = server.sent_back_error(&redirect_uri, &error, state.as_deref());
            Redirect::to(back.as_str()).into_response()
        }
    }
}

/// Takes "Allow" or "Deny" on Consent (`docs/specs/access.md`, The
/// authorization server, 4 and 4a), from the web UI's own pages alone, by
/// the person who signed in for the request in this browser, and sends the
/// browser back to the client: with a code, or with `access_denied`. Each
/// is logged with the person and the client.
pub(super) async fn decide(
    State(sources): State<Sources>,
    Extension(server): Extension<Arc<Server>>,
    cross_site: Option<Extension<CrossSite>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(refused) = refused_from_another_site(cross_site, "a consent") {
        return refused;
    }
    let Ok(ConsentForm { secret, decision }) = serde_urlencoded::from_bytes(&body) else {
        return (StatusCode::BAD_REQUEST, "the form does not read").into_response();
    };
    let caller = match browser_caller(&sources, &headers).await {
        Ok(Some(caller)) => caller,
        Ok(None) => return without_referrer(to_sign_in(&secret)),
        Err(e) => return failed("reading a browser's sign-in", &e),
    };
    let person = caller.person();
    let answered = AuthorizationRequests::new(sources.runs.database().clone())
        .answer(&secret, person.id, decision, &server, sources.clock.now())
        .await;
    let answered = match answered {
        Ok(Some(answered)) => answered,
        Ok(None) => {
            info!(
                person = %person.id,
                "refused a consent to an authorization request that no longer waits"
            );
            return authorization_error(&sources, CannotAuthorize::NoLongerWaits);
        }
        Err(e) => return failed("answering an authorization request", &e),
    };
    let back = match &answered {
        Answered::Allowed {
            waiting,
            authorization,
            code,
        } => {
            info!(
                person = %person.id,
                identity = person.logged(),
                client = %waiting.client,
                name = client_name(waiting.client_name.as_deref()),
                authorization = %authorization,
                "allowed a client"
            );
            server.sent_back(
                &waiting.redirect_uri,
                &[("code", code)],
                waiting.state.as_deref(),
            )
        }
        Answered::Denied { waiting } => {
            info!(
                person = %person.id,
                identity = person.logged(),
                client = %waiting.client,
                name = client_name(waiting.client_name.as_deref()),
                "denied a client"
            );
            server.sent_back_error(
                &waiting.redirect_uri,
                &OAuthError::access_denied(),
                waiting.state.as_deref(),
            )
        }
    };
    without_referrer(Redirect::to(back.as_str()).into_response())
}
