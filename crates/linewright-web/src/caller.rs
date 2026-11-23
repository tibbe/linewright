//! Who a request to the web UI is from: the person its browser's session
//! cookie signs in, as their record has them at the time of the request
//! (`docs/specs/access.md`, Roles, At each request). A request of an unsafe
//! method that its browser does not show came from the web UI's own pages
//! gets no permit for an action that changes something, so that none runs
//! for it.

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use tokio_util::sync::CancellationToken;
use tracing::error;

use linewright_core::access::sessions::{Resolved, SESSION_COOKIE, SessionCookie, Sessions};
use linewright_core::access::{Caller, LevelMarker, NeedsLevel, Permit, Read};
use linewright_core::actions::Action;
use linewright_core::api::Sources;

use super::sign_in::{Reason, ReturnPath, sign_in_path};
use super::{cookie_headers, cookie_values};

/// A request of an unsafe method that its browser does not show came from
/// the web UI's own pages, and why.
#[derive(Clone, Copy, Debug)]
pub(super) struct CrossSite {
    pub(super) reason: &'static str,
}

impl CrossSite {
    /// Whether the request of `parts` is one, and why.
    pub(super) fn of(parts: &Parts) -> Option<CrossSite> {
        parts.extensions.get::<CrossSite>().copied()
    }
}

/// The browser's session cookie, as a request sent it, if it sent one.
pub(super) fn session_cookie(headers: &HeaderMap) -> Option<SessionCookie> {
    cookie_values(cookie_headers(headers), SESSION_COOKIE)
        .into_iter()
        .next()
        .map(SessionCookie::of)
}

/// Resolves who each request is from by its browser's session cookie, for
/// [`Asking`] and [`Watch`]. A request from no one is sent to Sign in, to
/// come back once signed in, when it asks for a screen, and is answered
/// with 401 otherwise.
pub(super) async fn signed_in(
    State(sources): State<Sources>,
    mut request: Request,
    next: Next,
) -> Response {
    let cookie = session_cookie(request.headers());
    let resolved = match &cookie {
        Some(cookie) => sessions(&sources)
            .resolve(cookie, sources.clock.now())
            .await
            .map_err(|e| unread(&e)),
        None => Ok(Resolved::NoOne),
    };
    match (resolved, cookie) {
        (Ok(Resolved::Caller(caller)), Some(cookie)) => {
            request.extensions_mut().insert(caller);
            request.extensions_mut().insert(cookie);
            next.run(request).await
        }
        (Ok(Resolved::Ended), _) => not_signed_in(&request, Some(Reason::Ended)),
        (Ok(Resolved::Caller(_) | Resolved::NoOne), _) => not_signed_in(&request, None),
        (Err(failed), _) => failed,
    }
}

/// The answer to a request whose browser's sign-in did not read, which it
/// logs: 500, with why.
fn unread(e: &anyhow::Error) -> Response {
    let error = format!("{e:#}");
    error!(error, "reading a browser's sign-in failed");
    (StatusCode::INTERNAL_SERVER_ERROR, error).into_response()
}

/// The answer to `request` from a browser signed in as no one: to Sign in
/// for a screen, saying `reason` where there is one, and 401 otherwise.
fn not_signed_in(request: &Request, reason: Option<Reason>) -> Response {
    let screen = matches!(*request.method(), Method::GET | Method::HEAD)
        && !request.uri().path().starts_with("/events/");
    if !screen {
        return sign_in_first();
    }
    let return_to = ReturnPath::parse(
        request
            .uri()
            .path_and_query()
            .map(axum::http::uri::PathAndQuery::as_str),
    );
    let mut response = Redirect::to(&sign_in_path(&return_to, reason)).into_response();
    if reason.is_some() {
        response.headers_mut().insert(
            header::SET_COOKIE,
            SessionCookie::removed()
                .parse()
                .expect("the cookie is a header value"),
        );
    }
    response
}

/// The answer to a request that needs a sign-in its browser does not have.
fn sign_in_first() -> Response {
    (StatusCode::UNAUTHORIZED, "Sign in to Linewright.").into_response()
}

/// The browsers' sign-ins in `sources`' database.
pub(super) fn sessions(sources: &Sources) -> Sessions {
    Sessions::new(sources.runs.database().clone())
}

/// Marks each request of an unsafe method whose headers do not show it came
/// from the web UI's own pages, which then gets no permit for an action
/// that changes something.
/// <https://www.rfc-editor.org/rfc/rfc9110#section-9.2.1>
pub(super) async fn cross_site(mut request: Request, next: Next) -> Response {
    if !request.method().is_safe()
        && let Some(reason) = not_same_origin(request.headers())
    {
        request.extensions_mut().insert(CrossSite { reason });
    }
    next.run(request).await
}

/// Who a request is from, and whether it came from the web UI's own pages.
pub(super) struct Asking {
    caller: Caller,
    cross_site: Option<CrossSite>,
}

/// Why a request gets no permit for an action.
pub(super) enum NotPermitted {
    /// It did not come from the web UI's own pages, for the reason given.
    CrossSite(&'static str),
    NeedsLevel(NeedsLevel),
}

impl Asking {
    /// Its caller's permit of the level `L` for `action`, asked to change
    /// `changing`, or else why not: for an action that changes something,
    /// none for a request that did not come from the web UI's own pages.
    pub(super) fn permit<L: LevelMarker>(
        &self,
        action: Action,
        changing: &str,
    ) -> Result<Permit<L>, NotPermitted> {
        if let Some(CrossSite { reason }) = self.cross_site
            && !action.read_only()
        {
            return Err(NotPermitted::CrossSite(reason));
        }
        action
            .permit::<L>(&self.caller, changing)
            .map_err(NotPermitted::NeedsLevel)
    }
}

/// The person [`signed_in`] found, or else 401.
impl<S: Sync> FromRequestParts<S> for Asking {
    type Rejection = Response;

    fn from_request_parts(
        parts: &mut Parts,
        _: &S,
    ) -> impl Future<Output = Result<Asking, Response>> {
        let caller = parts.extensions.get::<Caller>().cloned();
        let cross_site = CrossSite::of(parts);
        std::future::ready(match caller {
            Some(caller) => Ok(Asking { caller, cross_site }),
            None => Err(sign_in_first()),
        })
    }
}

/// What ends an event stream: `serve` stopping, or its reader's browser no
/// longer signing anyone in (`docs/specs/access.md`, Roles, At each
/// request).
#[derive(Clone)]
pub(super) struct Watch {
    stopping: CancellationToken,
    sessions: Sessions,
    clock: std::sync::Arc<dyn linewright_core::clock::Clock>,
    cookie: SessionCookie,
}

impl Watch {
    /// Ends once `serve` stops.
    pub(super) fn stopping(&self) -> &CancellationToken {
        &self.stopping
    }

    /// Whether the browser still signs someone in, as its record has them
    /// now. A sign-in that does not read, which it logs, signs no one in.
    pub(super) async fn signs_someone_in(&self) -> bool {
        self.caller().await.is_some()
    }

    /// Who the browser signs in, as their record has them now, if anyone.
    /// A sign-in that does not read, which it logs, signs no one in.
    pub(super) async fn caller(&self) -> Option<Caller> {
        match self.sessions.resolve(&self.cookie, self.clock.now()).await {
            Ok(Resolved::Caller(caller)) => Some(caller),
            Ok(Resolved::Ended | Resolved::NoOne) => None,
            Err(e) => {
                unread(&e);
                None
            }
        }
    }
}

/// The watch of a request [`signed_in`] let through, or else 401.
impl FromRequestParts<Sources> for Watch {
    type Rejection = Response;

    fn from_request_parts(
        parts: &mut Parts,
        sources: &Sources,
    ) -> impl Future<Output = Result<Watch, Response>> {
        let stopping = parts.extensions.get::<CancellationToken>().cloned();
        let cookie = parts.extensions.get::<SessionCookie>().cloned();
        std::future::ready(match (stopping, cookie) {
            (Some(stopping), Some(cookie)) => Ok(Watch {
                stopping,
                sessions: sessions(sources),
                clock: sources.clock.clone(),
                cookie,
            }),
            _ => Err(sign_in_first()),
        })
    }
}

/// The permit to read of a request's caller, with what it reads.
pub(super) struct Reader {
    pub(super) permit: Permit<Read>,
    pub(super) sources: Sources,
}

/// The reader of the request's caller, or else 403 with why not.
impl FromRequestParts<Sources> for Reader {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, sources: &Sources) -> Result<Reader, Response> {
        Ok(Reader {
            permit: PermitToRead::from_request_parts(parts, sources).await?.0,
            sources: sources.clone(),
        })
    }
}

/// The permit to read of the request's caller, or else 403 with why not.
struct PermitToRead(Permit<Read>);

impl<S: Sync> FromRequestParts<S> for PermitToRead {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<PermitToRead, Response> {
        let asking = Asking::from_request_parts(parts, state).await?;
        asking
            .caller
            .permit::<Read>()
            .map(PermitToRead)
            .map_err(|needs| (StatusCode::FORBIDDEN, needs.to_string()).into_response())
    }
}

/// Why `headers` do not show a request came from a page of the same origin,
/// if they don't: the browser's `Sec-Fetch-Site`, or where it lacks one, an
/// `Origin` whose host and port are the `Host`'s.
/// <https://www.w3.org/TR/fetch-metadata/#sec-fetch-site-header>,
/// <https://www.rfc-editor.org/rfc/rfc6454#section-7>
fn not_same_origin(headers: &HeaderMap) -> Option<&'static str> {
    if let Some(site) = headers.get(SEC_FETCH_SITE) {
        return (site != "same-origin").then_some("its Sec-Fetch-Site is not same-origin");
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Some("it has neither Sec-Fetch-Site nor Origin");
    };
    let host = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok());
    let authority = origin
        .to_str()
        .ok()
        .and_then(|origin| origin.split_once("://"))
        .filter(|(scheme, _)| matches!(*scheme, "http" | "https"))
        .map(|(_, authority)| authority);
    match (authority, host) {
        (Some(authority), Some(host)) if authority.eq_ignore_ascii_case(host) => None,
        _ => Some("its Origin is not its Host"),
    }
}

const SEC_FETCH_SITE: &str = "sec-fetch-site";

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    header::HeaderName::from_static(name),
                    header::HeaderValue::from_static(value),
                )
            })
            .collect()
    }

    #[test_log::test]
    fn a_request_is_same_origin_by_its_sec_fetch_site_or_its_origin_and_host() {
        let same = [
            vec![
                ("sec-fetch-site", "same-origin"),
                ("host", "localhost:8080"),
            ],
            vec![
                ("sec-fetch-site", "same-origin"),
                ("origin", "https://lw.example.com"),
                ("host", "127.0.0.1:8080"),
            ],
            vec![
                ("origin", "http://localhost:8080"),
                ("host", "localhost:8080"),
            ],
            vec![
                ("origin", "https://LW.example.com"),
                ("host", "lw.example.com"),
            ],
        ];
        for pairs in same {
            assert_eq!(not_same_origin(&headers(&pairs)), None, "{pairs:?}");
        }
        let not = [
            vec![("sec-fetch-site", "cross-site"), ("host", "localhost:8080")],
            vec![("sec-fetch-site", "same-site"), ("host", "localhost:8080")],
            vec![("sec-fetch-site", "none"), ("host", "localhost:8080")],
            vec![
                ("sec-fetch-site", "cross-site"),
                ("origin", "http://localhost:8080"),
                ("host", "localhost:8080"),
            ],
            vec![
                ("origin", "http://evil.example"),
                ("host", "localhost:8080"),
            ],
            vec![
                ("origin", "http://localhost:9090"),
                ("host", "localhost:8080"),
            ],
            vec![("origin", "http://localhost"), ("host", "localhost:8080")],
            vec![("origin", "null"), ("host", "localhost:8080")],
            vec![("origin", "http://localhost:8080")],
            vec![("host", "localhost:8080")],
        ];
        for pairs in not {
            assert!(not_same_origin(&headers(&pairs)).is_some(), "{pairs:?}");
        }
    }
}
