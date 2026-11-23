//! A client's metadata document: the client registers by its URL, which is
//! its client ID, and Linewright fetches it, guarded against reaching
//! anything but the public internet over HTTPS: by the host's proxy where
//! there is one, and else by its own check of the address.
//! <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-client-id-metadata-document-00>

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{Context as _, bail};
use chrono::TimeDelta;
use hyper_util::client::proxy::matcher::{Intercept, Matcher};
use serde::Deserialize;
use url::Url;

use super::{Client, ClientId, client_name_of, redirect_uris};

/// The largest document Linewright reads.
/// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-client-id-metadata-document-00#section-6.6>
pub const LARGEST: usize = 5 * 1024;

/// How long a fetch may take.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// The longest Linewright keeps a document before it fetches it again.
pub const KEPT_AT_MOST: TimeDelta = TimeDelta::days(1);

/// The URL of a metadata document, which is its client's ID: HTTPS, with a
/// path, and without credentials, a fragment or a `.` or `..` segment.
/// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-client-id-metadata-document-00#section-3>
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentUrl(Url);

impl DocumentUrl {
    /// The URL of the metadata document `client_id` names, if it names one.
    pub fn parse(client_id: &str) -> Option<DocumentUrl> {
        document_url(client_id).map(DocumentUrl)
    }
}

fn document_url(client_id: &str) -> Option<Url> {
    let url = Url::parse(client_id).ok()?;
    let dot_segment = client_id
        .split(['/', '?'])
        .skip(3)
        .any(|segment| segment == "." || segment == "..");
    let named = url.scheme() == "https"
        && url.host().is_some()
        && url.path() != "/"
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && !dot_segment
        && url.as_str() == client_id;
    named.then_some(url)
}

/// Whether `address` is on the public internet: none of the loopback,
/// private, link-local, shared, documentation, benchmarking, multicast,
/// reserved or unspecified ranges, nor an IPv6 address standing for one.
/// <https://www.iana.org/assignments/iana-ipv4-special-registry/>,
/// <https://www.iana.org/assignments/iana-ipv6-special-registry/>
pub fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 88 && c == 99)
                || (a == 198 && (b == 18 || b == 19))
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let segments = v6.segments();
            let first = segments[0];
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || (first & 0xffc0) == 0xfec0
                || first == 0x2001 && segments[1] < 0x0200
                || first == 0x2001 && segments[1] == 0x0db8
                || first == 0x0100 && segments[1..4] == [0; 3]
                || segments[..6] == [0, 0, 0, 0, 0xffff, 0]
                || first == 0x0064 && segments[1] == 0xff9b
                || first == 0x2002
                || segments[..6] == [0; 6])
        }
    }
}

/// The headers of a response that say how long it stays fresh, each as it
/// came, where it came.
#[derive(Clone, Copy, Debug, Default)]
pub struct Freshness<'a> {
    pub cache_control: Option<&'a str>,
    pub expires: Option<&'a str>,
    pub date: Option<&'a str>,
}

impl Freshness<'_> {
    /// How long the response stays fresh, at most [`KEPT_AT_MOST`]: not at
    /// all with `no-store` or `no-cache`; else for its `max-age`; else
    /// from its `Date` to its `Expires`; else not at all.
    /// <https://www.rfc-editor.org/rfc/rfc9111#section-4.2.1>
    pub fn lasts(&self) -> TimeDelta {
        let mut max_age = None;
        for directive in self
            .cache_control
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
        {
            let (name, value) = directive.split_once('=').unwrap_or((directive, ""));
            match name.to_ascii_lowercase().as_str() {
                "no-store" | "no-cache" => return TimeDelta::zero(),
                "max-age" => max_age = value.trim_matches('"').parse::<i64>().ok(),
                _ => {}
            }
        }
        let http_date = |date: &str| chrono::DateTime::parse_from_rfc2822(date).ok();
        let lifetime = match (max_age, self.expires, self.date) {
            (Some(seconds), _, _) => TimeDelta::seconds(seconds),
            (None, Some(expires), Some(date)) => match (http_date(expires), http_date(date)) {
                (Some(expires), Some(date)) => expires - date,
                _ => TimeDelta::zero(),
            },
            _ => TimeDelta::zero(),
        };
        lifetime.max(TimeDelta::zero()).min(KEPT_AT_MOST)
    }
}

/// A metadata document's fields that Linewright reads.
#[derive(Deserialize)]
struct Document {
    client_id: String,
    client_name: Option<String>,
    #[serde(default)]
    redirect_uris: Vec<String>,
}

/// The client the document `body`, fetched from `url`, describes, or else
/// why it describes none: its `client_id` must be `url`, and its redirect
/// URIs those a client may register.
pub fn client_of(url: &Url, body: &[u8]) -> Result<Client, String> {
    let document: Document =
        serde_json::from_slice(body).map_err(|e| format!("it is not a client's metadata: {e}"))?;
    if document.client_id != url.as_str() {
        return Err(format!(
            "its client_id {:?} is not its URL",
            document.client_id
        ));
    }
    Ok(Client {
        id: ClientId::of(document.client_id),
        name: client_name_of(document.client_name)?,
        redirect_uris: redirect_uris(&document.redirect_uris)?,
    })
}

/// Fetches the document at `url` and gives its client, and how long it
/// stays fresh (`docs/specs/access.md`, The authorization server, Fetching
/// a metadata document). Where `host_proxy`, the host's proxy as
/// [`Matcher::from_env`] reads it, applies to `url` with HTTP to the
/// proxy, it fetches through the proxy, which the spec has refuse a
/// private or loopback address, and checks no address of its own. Else it
/// connects only to the address its host resolves to, and only when every
/// address it resolves to is public, giving up resolving after
/// [`TIMEOUT`]. Either way it follows no redirect, gives up fetching after
/// [`TIMEOUT`], and reads no more than [`LARGEST`].
pub async fn fetch(
    DocumentUrl(url): &DocumentUrl,
    host_proxy: &Matcher,
) -> anyhow::Result<(Client, TimeDelta)> {
    let mut builder = crate::http::client_builder_trusting_the_host_store_alone()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT);
    let uri: hyper::Uri = url.as_str().parse().context("it is not a URI")?;
    if let Some(proxy) = host_proxy.intercept(&uri).filter(spoken_over_http) {
        builder = builder.proxy(proxy_of(&proxy)?);
    } else {
        let host = url.host_str().context("it has no host")?;
        let port = url.port_or_known_default().unwrap_or(443);
        let address = public_address(host, port).await?;
        if url.domain().is_some() {
            builder = builder.resolve(host, address);
        }
    }
    let http = builder.build().context("building an HTTP client")?;
    let mut response = http
        .get(url.clone())
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .context("fetching it")?;
    if !response.status().is_success() {
        bail!("it answered {}", response.status());
    }
    let header = |name| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let fresh = Freshness {
        cache_control: header(reqwest::header::CACHE_CONTROL),
        expires: header(reqwest::header::EXPIRES),
        date: header(reqwest::header::DATE),
    }
    .lasts();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.context("reading it")? {
        body.extend_from_slice(&chunk);
        if body.len() > LARGEST {
            bail!("it is larger than {LARGEST} bytes");
        }
    }
    let client = client_of(url, &body).map_err(anyhow::Error::msg)?;
    Ok((client, fresh))
}

/// Whether Linewright can reach the proxy `intercept` names: over HTTP or
/// HTTPS, and not SOCKS, which its HTTP client is built without.
fn spoken_over_http(intercept: &Intercept) -> bool {
    matches!(intercept.uri().scheme_str(), Some("http" | "https"))
}

/// The proxy `intercept` names, with its credentials.
fn proxy_of(intercept: &Intercept) -> anyhow::Result<reqwest::Proxy> {
    let proxy =
        reqwest::Proxy::all(intercept.uri().to_string()).context("reading the host's proxy")?;
    Ok(match intercept.basic_auth() {
        Some(credentials) => proxy.custom_http_auth(credentials.clone()),
        None => proxy,
    })
}

/// The address `host` resolves to, on `port`, when every address it
/// resolves to is public.
async fn public_address(host: &str, port: u16) -> anyhow::Result<SocketAddr> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let addresses: Vec<SocketAddr> = match bare.parse::<IpAddr>() {
        Ok(address) => vec![SocketAddr::new(address, port)],
        Err(_) => tokio::time::timeout(TIMEOUT, tokio::net::lookup_host((bare, port)))
            .await
            .with_context(|| format!("resolving {host} took over {TIMEOUT:?}"))?
            .with_context(|| format!("resolving {host}"))?
            .collect(),
    };
    if let Some(private) = addresses.iter().find(|address| !is_public(address.ip())) {
        bail!("{host} resolves to {}, which is not public", private.ip());
    }
    addresses
        .first()
        .copied()
        .with_context(|| format!("{host} resolves to no address"))
}

#[cfg(test)]
mod tests {
    use super::super::RedirectUri;
    use super::*;

    const VSCODE: &str = "https://vscode.dev/oauth/client-metadata.json";
    const CLAUDE_CODE: &str = "https://claude.ai/oauth/claude-code-client-metadata";

    #[test_log::test]
    fn the_documents_of_vs_code_and_claude_code_are_their_clients() {
        let vscode = Url::parse(VSCODE).unwrap();
        let claude_code = Url::parse(CLAUDE_CODE).unwrap();

        let vscode = client_of(
            &vscode,
            include_bytes!("fixtures/vscode-client-metadata.json"),
        )
        .unwrap();
        let claude_code = client_of(
            &claude_code,
            include_bytes!("fixtures/claude-code-client-metadata.json"),
        )
        .unwrap();

        assert_eq!(vscode.id.as_str(), VSCODE);
        assert_eq!(vscode.name.as_deref(), Some("Visual Studio Code"));
        assert_eq!(
            vscode
                .redirect_uris
                .iter()
                .map(RedirectUri::as_str)
                .collect::<Vec<_>>(),
            ["http://127.0.0.1:33418/", "https://vscode.dev/redirect"]
        );
        assert_eq!(claude_code.name.as_deref(), Some("Claude Code"));
        assert_eq!(
            claude_code
                .redirect_uris
                .iter()
                .map(RedirectUri::as_str)
                .collect::<Vec<_>>(),
            ["http://localhost/callback", "http://127.0.0.1/callback"]
        );
    }

    #[test_log::test]
    fn a_document_whose_client_id_is_not_its_url_describes_no_client() {
        let elsewhere = Url::parse("https://evil.example/oauth/client-metadata.json").unwrap();

        let refused = client_of(
            &elsewhere,
            include_bytes!("fixtures/vscode-client-metadata.json"),
        );

        assert!(refused.unwrap_err().contains("is not its URL"));
    }

    #[test_log::test]
    fn a_document_with_a_redirect_uri_a_client_may_not_register_describes_no_client() {
        let url = Url::parse("https://agent.example/client").unwrap();
        let body = serde_json::json!({
            "client_id": "https://agent.example/client",
            "redirect_uris": ["https://agent.example/cb", "http://agent.example/cb"],
        });

        assert!(client_of(&url, body.to_string().as_bytes()).is_err());
    }

    #[test_log::test]
    fn a_client_id_names_a_document_by_an_https_url_with_a_path() {
        assert!(DocumentUrl::parse(VSCODE).is_some());
        assert!(DocumentUrl::parse(CLAUDE_CODE).is_some());
        for not in [
            "c-1b2c3",
            "http://vscode.dev/oauth/client-metadata.json",
            "https://vscode.dev",
            "https://vscode.dev/",
            "https://user:pass@vscode.dev/client",
            "https://vscode.dev/client#x",
            "https://vscode.dev/a/../client",
            "https://vscode.dev/a/./client",
            "HTTPS://vscode.dev/client",
        ] {
            assert!(DocumentUrl::parse(not).is_none(), "{not}");
        }
    }

    #[test_log::test]
    fn only_a_public_address_is_reached() {
        for public in ["93.184.215.14", "2606:2800:21f:cb07:6820:80da:af6b:8b2c"] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "::1",
            "::",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "192.88.99.1",
            "fec0::1",
            "2001::1",
            "2001:1ff::1",
            "100::1",
            "::ffff:0:a00:1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
    }

    fn cache_control(cache_control: &str) -> TimeDelta {
        Freshness {
            cache_control: Some(cache_control),
            ..Freshness::default()
        }
        .lasts()
    }

    /// The `Cache-Control` of VS Code's document and of Claude Code's, as
    /// each answered.
    #[test_log::test]
    fn a_document_stays_fresh_for_its_max_age_at_most_a_day() {
        assert_eq!(
            cache_control("no-store,no-cache,max-age=0"),
            TimeDelta::zero()
        );
        assert_eq!(
            cache_control("public, max-age=300"),
            TimeDelta::seconds(300)
        );
        assert_eq!(cache_control("max-age=31536000"), TimeDelta::days(1));
        assert_eq!(cache_control("public"), TimeDelta::zero());
        assert_eq!(Freshness::default().lasts(), TimeDelta::zero());
    }

    /// RFC 9111's own `Expires` and `Date`, an hour apart.
    /// <https://www.rfc-editor.org/rfc/rfc9111#section-5.3>
    #[test_log::test]
    fn without_a_max_age_a_document_stays_fresh_from_its_date_until_it_expires() {
        let expires = Freshness {
            cache_control: None,
            expires: Some("Thu, 01 Dec 1994 16:00:00 GMT"),
            date: Some("Thu, 01 Dec 1994 15:00:00 GMT"),
        };

        assert_eq!(expires.lasts(), TimeDelta::hours(1));
        assert_eq!(
            Freshness {
                cache_control: Some("max-age=60"),
                ..expires
            }
            .lasts(),
            TimeDelta::seconds(60)
        );
        assert_eq!(
            Freshness {
                expires: Some("0"),
                ..expires
            }
            .lasts(),
            TimeDelta::zero()
        );
        assert_eq!(
            Freshness {
                date: None,
                ..expires
            }
            .lasts(),
            TimeDelta::zero()
        );
    }

    #[test_log::test(tokio::test)]
    async fn without_a_proxy_a_document_is_fetched_only_from_a_public_address() {
        for url in [
            "https://127.0.0.1/client",
            "https://[::1]/client",
            "https://localhost/client",
        ] {
            let refused = fetch(
                &DocumentUrl::parse(url).unwrap(),
                &Matcher::builder().build(),
            )
            .await;
            assert!(
                refused.unwrap_err().to_string().contains("not public"),
                "{url}"
            );
        }
    }

    /// A proxy that answers each `CONNECT` with 403, as one that refuses a
    /// private address does.
    struct RefusingProxy {
        url: String,
        /// The head of each request it was asked, as it came.
        asked: tokio::sync::mpsc::UnboundedReceiver<String>,
    }

    #[track_caller]
    fn refusing_proxy() -> impl Future<Output = RefusingProxy> {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let caller = std::panic::Location::caller();
        async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap_or_else(|e| panic!("binding the proxy at {caller}: {e}"));
            let url = format!("http://{}", listener.local_addr().unwrap());
            let (ask, asked) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let mut head = Vec::new();
                    let mut byte = [0; 1];
                    while !head.ends_with(b"\r\n\r\n")
                        && stream.read(&mut byte).await.unwrap_or(0) == 1
                    {
                        head.push(byte[0]);
                    }
                    let _ = ask.send(String::from_utf8_lossy(&head).into_owned());
                    let _ = stream
                        .write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n")
                        .await;
                }
            });
            RefusingProxy { url, asked }
        }
    }

    /// `docs/specs/access.md`, The authorization server, Fetching a
    /// metadata document.
    #[test_log::test(tokio::test)]
    async fn through_the_hosts_proxy_a_document_is_fetched_with_no_address_check_of_its_own() {
        let mut proxy = refusing_proxy().await;
        let host_proxy = Matcher::builder().https(proxy.url.clone()).build();

        for (url, connect) in [
            ("https://localhost/client", "CONNECT localhost:443 HTTP/1.1"),
            (
                "https://127.0.0.1:8443/client",
                "CONNECT 127.0.0.1:8443 HTTP/1.1",
            ),
        ] {
            let refused = fetch(&DocumentUrl::parse(url).unwrap(), &host_proxy).await;

            assert!(refused.is_err(), "{url}");
            let head = proxy.asked.try_recv().unwrap_or_default();
            assert_eq!(head.lines().next(), Some(connect), "{url}");
        }
    }

    #[test_log::test(tokio::test)]
    async fn the_proxys_credentials_go_to_the_proxy() {
        let mut proxy = refusing_proxy().await;
        let with_credentials = proxy.url.replace("http://", "http://bot:p%40ss@");
        let host_proxy = Matcher::builder().all(with_credentials).build();

        let refused = fetch(
            &DocumentUrl::parse("https://localhost/client").unwrap(),
            &host_proxy,
        )
        .await;

        assert!(refused.is_err());
        let head = proxy.asked.try_recv().unwrap_or_default();
        assert!(
            head.to_ascii_lowercase()
                .contains("\r\nproxy-authorization: basic ym90onbac3m=\r\n"),
            "{head}"
        );
    }

    /// A SOCKS proxy, and a host `NO_PROXY` names, are fetched from
    /// straight.
    #[test_log::test(tokio::test)]
    async fn a_document_the_proxy_does_not_reach_is_fetched_straight() {
        let mut proxy = refusing_proxy().await;
        let socks = proxy.url.replace("http://", "socks5h://");
        for host_proxy in [
            Matcher::builder()
                .all(proxy.url.clone())
                .no("localhost")
                .build(),
            Matcher::builder().all(socks).build(),
        ] {
            let refused = fetch(
                &DocumentUrl::parse("https://localhost/client").unwrap(),
                &host_proxy,
            )
            .await;

            assert!(
                refused.unwrap_err().to_string().contains("not public"),
                "{host_proxy:?}"
            );
        }
        assert!(proxy.asked.try_recv().is_err());
    }
}
