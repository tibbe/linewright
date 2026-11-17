//! The web UI's writes: capturing a run, making a suite and removing a case
//! from one. Each is refused unless its request came from the web UI's own
//! pages, and answered with what the page says of it.

use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use tracing::warn;

use super::html;

/// A write the web UI takes.
#[derive(Clone, Copy, Debug)]
pub(super) enum Write {
    Capture,
    MakeSuite,
    RemoveCase,
}

impl Write {
    /// How the page says it did not happen, as in "Not captured: why".
    fn not_done(self) -> &'static str {
        match self {
            Write::Capture => "Not captured",
            Write::MakeSuite => "Not made",
            Write::RemoveCase => "Not removed",
        }
    }

    /// It, as a noun, as in "a capture is taken only from …".
    fn noun(self) -> &'static str {
        match self {
            Write::Capture => "a capture",
            Write::MakeSuite => "a new suite",
            Write::RemoveCase => "a case's removal",
        }
    }

    /// What the page says of it not happening, for `why`.
    pub(super) fn refused(self, status: StatusCode, why: &str) -> Refused {
        let said = format!("{}: {}", self.not_done(), html::escape(why));
        Refused {
            status,
            said: alert("alert-error", &said).0,
        }
    }

    /// Refused, logged with the path `uri`, unless `headers` show the
    /// request came from the web UI's own pages.
    pub(super) fn same_origin(self, headers: &HeaderMap, uri: &Uri) -> Result<(), Refused> {
        match not_same_origin(headers) {
            None => Ok(()),
            Some(reason) => {
                warn!(path = %uri.path(), reason, "refused {}", self.noun());
                Err(self.refused(
                    StatusCode::FORBIDDEN,
                    &format!("{} is taken only from the web UI's own pages", self.noun()),
                ))
            }
        }
    }

    /// The form `body` holds, or else what the page says of it not reading.
    pub(super) fn read_form<T: serde::de::DeserializeOwned>(
        self,
        body: &[u8],
    ) -> Result<T, Refused> {
        serde_urlencoded::from_bytes(body).map_err(|e| {
            self.refused(
                StatusCode::BAD_REQUEST,
                &format!("the form does not read: {e}"),
            )
        })
    }
}

/// A write that did not happen: the status it is answered with, and what
/// the page says of it.
pub(super) struct Refused {
    status: StatusCode,
    said: String,
}

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        (self.status, axum::response::Html(self.said)).into_response()
    }
}

/// `said`, in an alert of `kind`.
pub(super) fn alert(kind: &str, said: &str) -> axum::response::Html<String> {
    axum::response::Html(format!(
        r#"<p class="alert alert-soft {kind}"><span>{said}</span></p>"#
    ))
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
