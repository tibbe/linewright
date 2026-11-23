//! Reading a request's `Host` as a host name, ignoring case, port and a
//! trailing dot.

use std::net::IpAddr;

use axum::http::{HeaderMap, header};
use url::{Host, Url};

/// A `Host` header's name: without the brackets of an IPv6 address, the
/// port or a trailing dot, lower-cased.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostName(String);

impl HostName {
    /// The name in `headers`' `Host`, `None` when it is missing or
    /// unreadable.
    pub fn of(headers: &HeaderMap) -> Option<HostName> {
        headers
            .get(header::HOST)
            .and_then(|host| host.to_str().ok())
            .and_then(HostName::parse)
    }

    /// The name in the `Host` header value `host`.
    pub fn parse(host: &str) -> Option<HostName> {
        let name = match host.strip_prefix('[') {
            Some(bracketed) => bracketed.split_once(']').map(|(address, _)| address)?,
            None => host.split_once(':').map_or(host, |(name, _)| name),
        };
        Some(HostName::normalised(name))
    }

    fn normalised(name: &str) -> HostName {
        HostName(name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase())
    }

    /// `localhost` or a loopback address
    /// (`docs/specs/operation.md`, Monitoring commands).
    pub fn is_local(&self) -> bool {
        self.0 == "localhost" || self.is_loopback_address()
    }

    fn is_loopback_address(&self) -> bool {
        self.ip().is_some_and(|address| address.is_loopback())
    }

    fn ip(&self) -> Option<IpAddr> {
        self.0.parse().ok()
    }

    /// Whether it names the same host as `other`: the same address, however
    /// each writes it, or else the same name.
    fn same_host(&self, other: &HostName) -> bool {
        match (self.ip(), other.ip()) {
            (Some(address), Some(other)) => address == other,
            _ => self.0 == other.0,
        }
    }
}

/// `name` lower-cased and without a trailing dot, when it is a domain name:
/// dot-separated labels of letters, digits and hyphens, none starting or
/// ending with a hyphen.
pub fn domain_name(name: &str) -> Option<String> {
    let name = name.strip_suffix('.').unwrap_or(name);
    let labels = name.split('.').all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    labels.then(|| name.to_ascii_lowercase())
}

/// The URL people reach the web UI at, `public-url`
/// (`docs/specs/operation.md`, Serving the UI): HTTPS, or HTTP to a loopback
/// address, with nothing after its host and port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicUrl {
    url: Url,
    host: HostName,
}

impl PublicUrl {
    /// The public URL `given` is, with why it is none when it is not.
    pub fn parse(given: &str) -> Result<PublicUrl, String> {
        let url = Url::parse(given).map_err(|e| format!("it is not a URL: {e}"))?;
        let host = match url.host() {
            Some(Host::Domain(name)) => HostName(
                domain_name(name).ok_or_else(|| format!("its host {name:?} is no domain name"))?,
            ),
            Some(Host::Ipv4(address)) => HostName(address.to_string()),
            Some(Host::Ipv6(address)) => HostName(address.to_string()),
            None => return Err("it has no host".into()),
        };
        let secure = match url.scheme() {
            "https" => true,
            "http" => host.is_loopback_address(),
            _ => false,
        };
        if !secure {
            return Err("it is HTTPS, or HTTP to a loopback address such as \
                        `http://127.0.0.1:8081`"
                .into());
        }
        let bare = url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none();
        if !bare {
            return Err("it has nothing after its host and port".into());
        }
        Ok(PublicUrl { url, host })
    }

    /// The URL of `path` under it, such as `/auth/google/callback`.
    ///
    /// # Panics
    ///
    /// When `path` is no path a URL joins.
    pub fn join(&self, path: &str) -> Url {
        self.url.join(path).expect("a path joins a URL")
    }

    /// Its origin, with its port written out even where it is the scheme's
    /// default, such as `https://linewright.example:443`.
    /// <https://www.rfc-editor.org/rfc/rfc6454#section-4>
    pub fn origin(&self) -> String {
        let port = self
            .url
            .port_or_known_default()
            .expect("an HTTP or HTTPS URL has a port");
        let host = self.url.host_str().expect("a public URL has a host");
        format!("{}://{host}:{port}", self.url.scheme())
    }

    /// Whether the serialized origin `origin`, such as a request's `Origin`
    /// header, is its origin.
    /// <https://www.rfc-editor.org/rfc/rfc6454#section-6.2>
    pub fn is_origin(&self, origin: &str) -> bool {
        Url::parse(origin).is_ok_and(|given| given.origin() == self.url.origin())
    }

    /// Whether the web UI answers a request whose `Host` is `host`: this
    /// URL's host.
    pub fn answers(&self, host: &HostName) -> bool {
        self.host.same_host(host)
    }
}

impl std::fmt::Display for PublicUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.url.fmt(f)
    }
}
