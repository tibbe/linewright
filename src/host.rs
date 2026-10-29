//! Reading a request's `Host`: the one place that says what a host name
//! is, ignoring case, port and a trailing dot, for every check made on it.

use std::net::IpAddr;

use axum::http::{HeaderMap, header};

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
    /// (`docs/specs/monitoring.md`, Commands).
    pub fn is_local(&self) -> bool {
        self.0 == "localhost" || self.ip().is_some_and(|address| address.is_loopback())
    }

    pub fn is_ip(&self) -> bool {
        self.ip().is_some()
    }

    fn ip(&self) -> Option<IpAddr> {
        self.0.parse().ok()
    }
}

/// The names the web UI answers besides the local ones and IP addresses:
/// the `ui-hosts` setting (`docs/specs/design_system.md`, Serving).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiHosts(Vec<HostName>);

impl UiHosts {
    /// From `names`, each a host name such as `linewright.example.com`,
    /// with what is wrong with the first that is not.
    pub fn new<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<UiHosts, String> {
        names
            .into_iter()
            .map(|name| {
                let refused = name.is_empty()
                    || name == "."
                    || name
                        .chars()
                        .any(|c| matches!(c, ':' | '/' | '*') || c.is_whitespace());
                if refused {
                    Err(format!(
                        "names {name:?}: it is a host name, such as `linewright.example.com`"
                    ))
                } else {
                    Ok(HostName::normalised(name))
                }
            })
            .collect::<Result<_, _>>()
            .map(UiHosts)
    }

    /// Whether the web UI answers a request whose `Host` is `host`: a local
    /// name, an IP address or one of these.
    pub fn answers(&self, host: &HostName) -> bool {
        host.is_local() || host.is_ip() || self.0.contains(host)
    }
}
