//! Who a request to the web UI is from. A request of an unsafe method that
//! its browser does not show came from the web UI's own pages gets no permit
//! for an action that changes something, so that none runs for it.

use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::access::{Caller, LevelMarker, NeedsLevel, Permit, Read};
use crate::actions::Action;
use crate::api::Sources;

/// A request of an unsafe method that its browser does not show came from
/// the web UI's own pages, and why.
#[derive(Clone, Copy, Debug)]
struct CrossSite {
    reason: &'static str,
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

/// Whoever reaches the web UI's port, while it signs no one in.
impl<S: Sync> FromRequestParts<S> for Asking {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _: &S,
    ) -> impl Future<Output = Result<Asking, Self::Rejection>> {
        std::future::ready(Ok(Asking {
            caller: Caller::anyone(),
            cross_site: parts.extensions.get::<CrossSite>().copied(),
        }))
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
            permit: Permit::<Read>::from_request_parts(parts, sources).await?,
            sources: sources.clone(),
        })
    }
}

/// The permit to read of the request's caller, or else 403 with why not.
impl<S: Sync> FromRequestParts<S> for Permit<Read> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Permit<Read>, Response> {
        let Ok(asking) = Asking::from_request_parts(parts, state).await;
        asking
            .caller
            .permit::<Read>()
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
