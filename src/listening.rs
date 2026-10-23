//! Running an HTTP listener, the socket's or a TCP port's, until stopped.

use std::fmt;
use std::str::FromStr;

use anyhow::{Context, bail};
use axum::Router;
use axum::serve::Listener;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::error;

/// A listener being served, until [`Listening::stop`].
pub struct Listening {
    stop: CancellationToken,
    task: JoinHandle<()>,
}

impl Listening {
    /// Serves `router` on `listener`, logging what failed as `what` if
    /// serving does.
    pub fn spawn<L>(listener: L, router: Router, what: &'static str) -> Listening
    where
        L: Listener,
        L::Addr: fmt::Debug,
    {
        let stop = CancellationToken::new();
        let task = tokio::spawn({
            let stop = stop.clone();
            async move {
                let served = axum::serve(listener, router)
                    .with_graceful_shutdown(stop.cancelled_owned())
                    .await;
                if let Err(e) = served {
                    error!(error = %e, "{what} failed");
                }
            }
        });
        Listening { stop, task }
    }

    /// Stops listening, and returns once the answers being given are.
    pub async fn stop(self) {
        self.stop.cancel();
        let _ = self.task.await;
    }
}

/// A host and port to listen on, the host resolved when it is bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    /// Without the brackets of an IPv6 address.
    host: String,
    port: u16,
}

impl Address {
    /// Listens on it, with an error naming the setting it is from.
    pub async fn bind(&self, setting: &str) -> anyhow::Result<TcpListener> {
        TcpListener::bind((self.host.as_str(), self.port))
            .await
            .with_context(|| format!("listening on the setting `{setting}`, {self}"))
    }
}

impl FromStr for Address {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> anyhow::Result<Address> {
        let (host, port) = value
            .rsplit_once(':')
            .context("it is a host and port, such as `0.0.0.0:8080`")?;
        let host = match host.strip_prefix('[') {
            Some(bracketed) => bracketed.strip_suffix(']'),
            None if host.contains(':') => None,
            None => Some(host),
        }
        .context("an IPv6 host is in brackets, such as `[::1]:8080`")?;
        if host.is_empty() {
            bail!("it has no host: listen on every address with `0.0.0.0`");
        }
        let port = port
            .parse()
            .with_context(|| format!("the port {port:?} is not a number from 0 to 65535"))?;
        Ok(Address {
            host: host.to_string(),
            port,
        })
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}
