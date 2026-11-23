//! Linewright as the OAuth authorization server of the MCP server
//! (`docs/specs/access.md`, The authorization server, Tokens, Consent and
//! Authorization error): its metadata, registration, the authorization
//! request through Consent to its sign-in, and the token endpoint.

use std::sync::Arc;

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tracing::{error, info};

use linewright_core::access::oauth::clients::{Clients, Found};
use linewright_core::access::oauth::store::{Asked, AuthorizationRequests, SignedIn, Tokens};
use linewright_core::access::oauth::{
    AUTHORIZE_PATH, AuthorizeError, AuthorizeRequest, CannotSendBack, OAuthError, RedirectUri,
    RegistrationRequest, SentBack, Server, TokenForm, TokenRequest, check_authorize, client_name,
    registration,
};
use linewright_core::access::people::Person;
use linewright_core::api::Sources;

use super::caller::CrossSite;
use super::card_page::CardPage;
use super::sign_in::{refused_from_another_site, sign_in_for_authorization_path};

/// The field of Consent's form, and of its path's query, that holds the
/// handle of its authorization request: none of the names an authorization
/// request from a client takes, such as `request`.
/// <https://www.rfc-editor.org/rfc/rfc9101#section-5>
const REQUEST: &str = "waiting";

/// The field of Consent's form that holds "allow" or "deny".
const DECISION: &str = "decision";

/// What a person decides on Consent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Decision {
    Allow,
    Deny,
}

/// Consent's form, as it posts it, its fields named [`REQUEST`] and
/// [`DECISION`].
#[derive(Deserialize)]
struct ConsentForm {
    #[serde(rename = "waiting")]
    handle: String,
    decision: Decision,
}

/// The path of Consent of the authorization request `handle` finds.
fn consent_path(handle: &str) -> String {
    format!("{AUTHORIZE_PATH}?{REQUEST}={handle}")
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
    /// The authorization request ended: denied, signed in for, or 10
    /// minutes after its client made it.
    Ended,
}

impl std::fmt::Display for CannotAuthorize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CannotAuthorize::CannotSendBack(why) => why.fmt(f),
            CannotAuthorize::Ended => {
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
    without_referrer((StatusCode::BAD_REQUEST, page).into_response())
}

#[derive(Template, WebTemplate)]
#[template(path = "consent.html")]
struct ConsentPage {
    card: CardPage,
    facts: Vec<(&'static str, String)>,
    /// The warning, where there is one.
    warning: Option<&'static str>,
    request: String,
}

/// What Consent warns of a request whose redirect URI is a loopback one.
const LOOPBACK_WARNING: &str =
    "This client is an app on this computer. Linewright cannot check which app.";

/// Consent of `asked`, the request `handle` finds (`docs/specs/access.md`,
/// Consent).
fn consent(sources: &Sources, asked: &Asked, handle: &str) -> Response {
    let facts = asked
        .client_host
        .clone()
        .map(|host| ("Client", host))
        .into_iter()
        .chain([("Sends you back to", asked.redirect_uri.host().to_string())])
        .collect();
    ConsentPage {
        card: CardPage::new(
            "Consent",
            "consent",
            format!(
                "Allow {} to act as you?",
                client_name(asked.client_name.as_deref())
            ),
            sources,
        ),
        facts,
        warning: asked.redirect_uri.is_loopback().then_some(LOOPBACK_WARNING),
        request: handle.to_string(),
    }
    .into_response()
}

/// Keeps the browser's address from the client: an authorization request's
/// pages carry its handle.
fn without_referrer(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

/// An authorization request (`docs/specs/access.md`, The authorization
/// server): one from a client goes to Consent, unless Linewright refuses
/// it; one that names a request is its Consent, until the request ends.
/// Neither needs a sign-in.
pub(super) async fn authorize(
    State(sources): State<Sources>,
    Extension(server): Extension<Arc<Server>>,
    RawQuery(query): RawQuery,
) -> Response {
    let query = query.unwrap_or_default();
    let answer = match consent_of(&query) {
        Some(handle) => consent_of_request(&sources, &handle).await,
        None => from_client(&sources, &server, &query).await,
    };
    without_referrer(answer)
}

/// The handle of the authorization request whose Consent `query` asks
/// for, if it asks for one rather than being a client's request.
fn consent_of(query: &str) -> Option<String> {
    let mut pairs = form_urlencoded::parse(query.as_bytes());
    match (pairs.next(), pairs.next()) {
        (Some((name, handle)), None) if name == REQUEST => Some(handle.into_owned()),
        _ => None,
    }
}

/// Consent of the request `handle` finds, or Authorization error once it
/// has ended.
async fn consent_of_request(sources: &Sources, handle: &str) -> Response {
    let requests = AuthorizationRequests::new(sources.runs.database().clone());
    match requests.asked(handle, sources.clock.now()).await {
        Ok(Some(asked)) => consent(sources, &asked, handle),
        Ok(None) => ended(sources, "Consent"),
        Err(e) => failed("reading an authorization request", &e),
    }
}

/// Authorization error for `what`, such as "Consent", of a request that has
/// ended, which it logs.
pub(super) fn ended(sources: &Sources, what: &str) -> Response {
    info!("refused {what} of an authorization request that has ended");
    authorization_error(sources, CannotAuthorize::Ended)
}

/// The authorization request `query` makes, from a client: to Consent once
/// Linewright takes it, to the client with why not where it can send the
/// browser back, and else Authorization error.
async fn from_client(sources: &Sources, server: &Server, query: &str) -> Response {
    let now = sources.clock.now();
    let request = AuthorizeRequest::from_query(query);
    let client = match &request.client_id {
        Some(id) => {
            Clients::new(sources.runs.database().clone())
                .find(id, now)
                .await
        }
        None => Ok(Found::Unknown),
    };
    let client = match client {
        Ok(client) => client,
        Err(e) => return failed("reading a client", &e),
    };
    match check_authorize(&request, &client, server) {
        Ok(checked) => {
            match AuthorizationRequests::new(sources.runs.database().clone())
                .begin(&checked, now)
                .await
            {
                Ok(handle) => Redirect::to(&consent_path(&handle)).into_response(),
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
/// authorization server, 1, and When a request ends), from the web UI's
/// own pages alone, and logs it with the client. "Allow" gives the browser
/// the request's new handle in `__Host-linewright-authorization` and sends it to
/// Sign in for the authorization; "Deny" ends the request and sends the
/// browser back to the client with `access_denied`.
pub(super) async fn decide(
    State(sources): State<Sources>,
    Extension(server): Extension<Arc<Server>>,
    cross_site: Option<Extension<CrossSite>>,
    body: Bytes,
) -> Response {
    if let Some(refused) = refused_from_another_site(cross_site, "a consent") {
        return refused;
    }
    let Ok(ConsentForm { handle, decision }) = serde_urlencoded::from_bytes(&body) else {
        return (StatusCode::BAD_REQUEST, "the form does not read").into_response();
    };
    let now = sources.clock.now();
    let requests = AuthorizationRequests::new(sources.runs.database().clone());
    let answer = match decision {
        Decision::Allow => match requests.allow(&handle, now).await {
            Ok(Some((asked, allowed))) => {
                info!(
                    client = %asked.client,
                    name = client_name(asked.client_name.as_deref()),
                    "allowed a client"
                );
                (
                    [(header::SET_COOKIE, allowed.set_cookie())],
                    Redirect::to(&sign_in_for_authorization_path()),
                )
                    .into_response()
            }
            Ok(None) => ended(&sources, "a decision on Consent"),
            Err(e) => failed("allowing an authorization request", &e),
        },
        Decision::Deny => match requests.deny(&handle, now).await {
            Ok(Some(asked)) => {
                info!(
                    client = %asked.client,
                    name = client_name(asked.client_name.as_deref()),
                    "denied a client"
                );
                let back = server.sent_back_error(
                    &asked.redirect_uri,
                    &OAuthError::access_denied(),
                    asked.state.as_deref(),
                );
                Redirect::to(back.as_str()).into_response()
            }
            Ok(None) => ended(&sources, "a decision on Consent"),
            Err(e) => failed("denying an authorization request", &e),
        },
    };
    without_referrer(answer)
}

/// Ends the request that `allowed`, the handle in the browser's
/// `__Host-linewright-authorization`, finds, with `person` just signed in for it
/// (`docs/specs/access.md`, The authorization server, 2): sends the
/// browser back to the client with a code, and logs the authorization; or
/// Authorization error once the request has ended.
pub(super) async fn signed_in_for(
    sources: &Sources,
    server: &Server,
    allowed: &str,
    person: &Person,
) -> Response {
    let signed_in = AuthorizationRequests::new(sources.runs.database().clone())
        .sign_in(allowed, person.id, server, sources.clock.now())
        .await;
    let SignedIn {
        asked,
        authorization,
        code,
    } = match signed_in {
        Ok(Some(signed_in)) => signed_in,
        Ok(None) => return ended(sources, "a sign-in"),
        Err(e) => return failed("authorizing a client", &e),
    };
    info!(
        person = %person.id,
        identity = person.logged(),
        client = %asked.client,
        name = client_name(asked.client_name.as_deref()),
        authorization = %authorization,
        "authorized a client"
    );
    let back = server.sent_back(
        &asked.redirect_uri,
        &[("code", &code)],
        asked.state.as_deref(),
    );
    without_referrer(Redirect::to(back.as_str()).into_response())
}
