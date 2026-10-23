//! Reading a request's `Host`: the one place that says what a host name
//! is, ignoring case and port, for every check made on it.

use std::net::IpAddr;

use axum::http::{HeaderMap, header};

/// A `Host` header's name: without the brackets of an IPv6 address or the
/// port, lower-cased.
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
        Some(HostName(name.to_ascii_lowercase()))
    }

    /// `localhost` or a loopback address
    /// (`docs/specs/architecture.md`, The local API).
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
