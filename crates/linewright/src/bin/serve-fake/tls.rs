//! A TLS front: HTTPS on a free port of `127.0.0.1`, with a self-signed
//! certificate for `127.0.0.1`, in front of a plain HTTP address.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer;

/// The address the front listens on and its certificate names.
const HOST: &str = "127.0.0.1";

/// A listener that takes TLS at [`Front::url`].
pub struct Front {
    listener: TcpListener,
    acceptor: TlsAcceptor,
}

impl Front {
    /// A front on a free port, with a certificate made for it.
    ///
    /// # Errors
    ///
    /// When the certificate cannot be made or no port can be bound.
    pub async fn bind() -> Result<Front> {
        let certified = rcgen::generate_simple_self_signed([HOST.to_string()])
            .context("generating serve-fake's certificate")?;
        let key = PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certified.cert.der().clone()], key.into())
            .context("serving serve-fake's certificate")?;
        Ok(Front {
            listener: TcpListener::bind((HOST, 0)).await?,
            acceptor: TlsAcceptor::from(Arc::new(config)),
        })
    }

    /// The HTTPS URL the front takes connections at.
    ///
    /// # Errors
    ///
    /// When the listener has no local address.
    pub fn url(&self) -> Result<String> {
        Ok(format!(
            "https://{HOST}:{}/",
            self.listener.local_addr()?.port()
        ))
    }

    /// Forwards each connection's bytes, decrypted, to `to` and back, as
    /// they come: an event stream's events pass as it sends them. It never
    /// returns.
    pub async fn forward(self, to: SocketAddr) {
        loop {
            match self.listener.accept().await {
                Ok((connection, _)) => {
                    let acceptor = self.acceptor.clone();
                    tokio::spawn(async move {
                        if let Err(e) = relay(acceptor, connection, to).await {
                            tracing::debug!("serve-fake's TLS front ended a connection: {e:#}");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("serve-fake's TLS front accepts no connection: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
}

/// Takes TLS on `connection` and passes its bytes to `to` and back until
/// either side closes.
async fn relay(acceptor: TlsAcceptor, connection: TcpStream, to: SocketAddr) -> Result<()> {
    let mut outside = acceptor.accept(connection).await?;
    let mut inside = TcpStream::connect(to).await?;
    tokio::io::copy_bidirectional(&mut outside, &mut inside).await?;
    Ok(())
}
