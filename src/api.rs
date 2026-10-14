//! The local API: `serve` answers questions about itself over HTTP with
//! JSON bodies on a Unix socket, and the monitoring commands are its clients
//! (`docs/specs/architecture.md`, "The local API").

use std::fs::{DirBuilder, File, Permissions, TryLockError};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::TokioIo;
use sha2::Digest;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::clock::Clock;
use crate::status::{self, Serving, View};
use crate::webhook::Tally;

/// The per-user directory the sockets are in: `linewright` under the user's
/// runtime directory, `XDG_RUNTIME_DIR`, when it is set, or else
/// `linewright-<uid>` under the temporary directory `temp_dir`.
pub fn socket_dir(runtime_dir: Option<PathBuf>, temp_dir: &Path, uid: u32) -> PathBuf {
    match runtime_dir.filter(|dir| !dir.as_os_str().is_empty()) {
        Some(dir) => dir.join("linewright"),
        None => temp_dir.join(format!("linewright-{uid}")),
    }
}

/// The socket of the `serve` of `repo` in `dir`, named for the repository,
/// which GitHub names case-insensitively, by a hash that keeps the path
/// short whatever the name's length.
pub fn socket_path(dir: &Path, repo: &str) -> PathBuf {
    let digest = sha2::Sha256::digest(repo.to_lowercase().as_bytes());
    dir.join(format!("{}.sock", &hex::encode(digest)[..16]))
}

/// Why `serve` cannot take the socket.
#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    #[error("a `serve` already runs for {repo} on this host; stop it first (its socket is {path})")]
    AlreadyServing { repo: String, path: String },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// The socket a `serve` listens on, held for as long as it lives.
pub struct Claimed {
    listener: UnixListener,
    path: PathBuf,
    /// Locked while held. The kernel releases the lock when the process
    /// ends however it ends, so a socket a dead `serve` left behind never
    /// blocks the next.
    lock: File,
}

/// Takes the socket of the `serve` of `repo` in `dir`, only `serve`'s user
/// able to open it, unless another `serve` holds it.
pub fn claim(dir: &Path, repo: &str) -> Result<Claimed, ClaimError> {
    private_dir(dir)?;
    let path = socket_path(dir, repo);
    let lock_path = path.with_extension("lock");
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path.display()))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(ClaimError::AlreadyServing {
                repo: repo.to_string(),
                path: path.display().to_string(),
            });
        }
        Err(TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("locking {}", lock_path.display()))?;
        }
    }
    // Left behind by a `serve` that died.
    remove_if_present(&path)?;
    let listener =
        UnixListener::bind(&path).with_context(|| format!("listening on {}", path.display()))?;
    std::fs::set_permissions(&path, Permissions::from_mode(0o600))
        .with_context(|| format!("restricting {} to its owner", path.display()))?;
    Ok(Claimed {
        listener,
        path,
        lock,
    })
}

/// Removes the file at `path`, if there is one.
fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Makes `dir`, readable only by its owner, or checks that it is so and
/// owned by this user.
fn private_dir(dir: &Path) -> Result<()> {
    match DirBuilder::new().recursive(false).mode(0o700).create(dir) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).with_context(|| format!("creating {}", dir.display())),
    }
    let metadata =
        std::fs::symlink_metadata(dir).with_context(|| format!("reading {}", dir.display()))?;
    if !metadata.is_dir() || metadata.uid() != rustix::process::getuid().as_raw() {
        bail!("{} is not a directory of this user's", dir.display());
    }
    if metadata.mode() & 0o077 != 0 {
        bail!(
            "{} is open to other users: make it readable by its owner only",
            dir.display()
        );
    }
    Ok(())
}

/// What the API answers from.
#[derive(Clone)]
pub struct Sources {
    pub serving: Serving,
    /// The factory's view, `None` until its first poll.
    pub view: watch::Receiver<Option<View>>,
    /// Set with webhooks on.
    pub deliveries: Option<Tally>,
    pub clock: Arc<dyn Clock>,
}

/// The API's server, until stopped.
pub struct Api {
    path: PathBuf,
    stop: CancellationToken,
    task: JoinHandle<()>,
    /// Held until the socket is removed.
    lock: File,
}

impl Api {
    /// Answers on the `claimed` socket from `sources`.
    pub fn serve(claimed: Claimed, sources: Sources) -> Api {
        let Claimed {
            listener,
            path,
            lock,
        } = claimed;
        let router = Router::new()
            .route("/status", get(answer_status))
            .fallback(|| async { error_response(StatusCode::NOT_FOUND, "no such resource") })
            .layer(middleware::from_fn(local_only))
            .with_state(sources);
        let stop = CancellationToken::new();
        let task = tokio::spawn({
            let stop = stop.clone();
            async move {
                let served = axum::serve(listener, router)
                    .with_graceful_shutdown(stop.cancelled_owned())
                    .await;
                if let Err(e) = served {
                    error!(error = %e, "answering on the local API failed");
                }
            }
        });
        info!(socket = %path.display(), "answering the monitoring commands");
        Api {
            path,
            stop,
            task,
            lock,
        }
    }

    /// Stops answering, once the requests being answered are, and removes
    /// the socket.
    pub async fn stop(self) {
        self.stop.cancel();
        let _ = self.task.await;
        if let Err(e) = remove_if_present(&self.path) {
            error!(
                error = format!("{e:#}"),
                "removing the local API's socket failed"
            );
        }
        drop(self.lock);
    }
}

async fn answer_status(State(sources): State<Sources>) -> Response {
    let webhooks = sources.deliveries.as_ref().map(Tally::get);
    let view = sources.view.borrow().clone();
    let answer = status::status(
        &sources.serving,
        webhooks,
        view.as_ref(),
        sources.clock.now(),
    );
    json_response(StatusCode::OK, &answer)
}

/// Refuses a request whose `Host` is not local, such as one a web page
/// sends through a tunnel to the socket under a name it controls.
async fn local_only(request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok());
    if !host.is_some_and(is_local) {
        return error_response(
            StatusCode::FORBIDDEN,
            "only requests whose `Host` is local are answered",
        );
    }
    next.run(request).await
}

/// Whether `host`, a `Host` header, names this host by a loopback name or
/// address, with or without a port.
fn is_local(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(bracketed) => bracketed.split_once(']').map(|(address, _)| address),
        None => Some(host.split_once(':').map_or(host, |(name, _)| name)),
    };
    name.is_some_and(|name| {
        name.eq_ignore_ascii_case("localhost")
            || name
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
}

fn json_response(status: StatusCode, body: &impl serde::Serialize) -> Response {
    let body = serde_json::to_string(body).expect("the answers serialize");
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn error_response(status: StatusCode, message: &str) -> Response {
    json_response(status, &serde_json::json!({ "error": message }))
}

/// Why a monitoring command got no answer.
#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("`serve` is not running for {repo} on this host: nothing listens on {path}")]
    NotServing { repo: String, path: String },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Asks the `serve` of `repo` whose socket is in `dir` for `resource`, such
/// as `/status`, and returns its answer's body.
pub async fn get_answer(dir: &Path, repo: &str, resource: &str) -> Result<Bytes, RequestError> {
    let path = socket_path(dir, repo);
    let stream = match UnixStream::connect(&path).await {
        Ok(stream) => stream,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(RequestError::NotServing {
                repo: repo.to_string(),
                path: path.display().to_string(),
            });
        }
        Err(e) => Err(e).with_context(|| format!("connecting to {}", path.display()))?,
    };
    Ok(request(stream, resource).await?)
}

async fn request(stream: UnixStream, resource: &str) -> Result<Bytes> {
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .context("talking to `serve`")?;
    tokio::spawn(connection);
    let request = hyper::Request::get(resource)
        .header(header::HOST, "localhost")
        .body(Empty::<Bytes>::new())
        .context("building the request")?;
    let response = sender
        .send_request(request)
        .await
        .context("asking `serve`")?;
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .context("reading `serve`'s answer")?
        .to_bytes();
    if !status.is_success() {
        bail!(
            "`serve` answered {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    Ok(body)
}
