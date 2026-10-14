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
use axum::extract::{Path as Resource, Query, Request, State};
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
use crate::domain::Number;
use crate::explain;
use crate::logs::{self, LogsError, NoSuchRun};
use crate::runs::{self, ListError, RepoWorkDir, Runs, Watched};
use crate::status::{self, Serving, View};
use crate::store::Store;
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

/// What `serve` answers from once it has polled: its view, and the store it
/// reads the items it does not track from.
#[derive(Clone)]
pub struct Served {
    /// Read only through [`Served::view_now`].
    view: View,
    store: Arc<dyn Store>,
    /// The runs in flight, whose activity goes on after the view was taken.
    runs: Vec<Watched>,
}

impl Served {
    /// The factory's `view`, with its runs in flight `runs`, and its store.
    pub fn new(view: View, store: Arc<dyn Store>, runs: Vec<Watched>) -> Served {
        Served { view, store, runs }
    }

    /// The view with the runs in flight as they stand now, so that their
    /// quiet time counts from their latest output.
    pub fn view_now(&self) -> View {
        View {
            in_flight: self.runs.iter().map(Watched::read).collect(),
            ..self.view.clone()
        }
    }
}

/// What the API answers from.
#[derive(Clone)]
pub struct Sources {
    pub serving: Serving,
    /// The factory's view and store, `None` until its first poll.
    pub served: watch::Receiver<Option<Served>>,
    /// Set with webhooks on.
    pub deliveries: Option<Tally>,
    pub clock: Arc<dyn Clock>,
    /// Where the repository's runs are recorded.
    pub work_dir: RepoWorkDir,
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
            .route("/runs", get(answer_runs))
            .route("/items/{number}", get(answer_item))
            .route("/logs", get(answer_logs))
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
    let view = sources.served.borrow().as_ref().map(Served::view_now);
    let answer = status::status(
        &sources.serving,
        webhooks,
        view.as_ref(),
        sources.clock.now(),
    );
    json_response(StatusCode::OK, &answer)
}

/// The runs the query asks for, read from their records as `runs` reads
/// them with no `serve` running, so that the answer is the same.
async fn answer_runs(State(sources): State<Sources>, Query(query): Query<runs::Query>) -> Response {
    answer(runs::answer(&sources.work_dir, &query, sources.clock.now()))
}

/// The transcript of the run the query names, read from the records as
/// `logs` reads it with no `serve` running, so that the answer is the same.
async fn answer_logs(State(sources): State<Sources>, Query(which): Query<logs::Which>) -> Response {
    answer(logs::answer(&sources.work_dir, &which))
}

/// Why there is no answer from the records, `serve`'s or a command's.
#[derive(Debug)]
pub enum Unanswered {
    /// The item or run asked about does not exist (`docs/specs/monitoring.md`,
    /// "Output").
    NoSuch {
        missing: Missing,
        error: String,
    },
    Failed(anyhow::Error),
}

impl From<ListError> for Unanswered {
    fn from(e: ListError) -> Unanswered {
        match e {
            ListError::NoRuns(no_runs) => Unanswered::NoSuch {
                missing: Missing::Item,
                error: no_runs.to_string(),
            },
            ListError::Other(e) => Unanswered::Failed(e),
        }
    }
}

impl From<LogsError> for Unanswered {
    fn from(e: LogsError) -> Unanswered {
        match e {
            LogsError::NoSuchRun(no_such) => Unanswered::NoSuch {
                missing: match no_such {
                    NoSuchRun::NoRuns(_) => Missing::Item,
                    NoSuchRun::NoRun(_) | NoSuchRun::NotOf { .. } => Missing::Run,
                },
                error: no_such.to_string(),
            },
            LogsError::Other(e) => Unanswered::Failed(e),
        }
    }
}

/// `answer` as `serve` sends it.
fn answer(answer: Result<impl serde::Serialize, impl Into<Unanswered>>) -> Response {
    match answer.map_err(Into::into) {
        Ok(value) => json_response(StatusCode::OK, &value),
        Err(Unanswered::NoSuch { missing, error }) => json_response(
            StatusCode::NOT_FOUND,
            &ErrorAnswer {
                error,
                missing: Some(missing),
            },
        ),
        Err(Unanswered::Failed(e)) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}"))
        }
    }
}

/// The answer to `explain N`: from the view for an item Linewright tracks,
/// or else from #N read from the store, with its runs read from their
/// records; not found for a number with no item, and unavailable before the
/// first poll.
async fn answer_item(
    State(sources): State<Sources>,
    Resource(number): Resource<Number>,
) -> Response {
    let served = sources.served.borrow().clone();
    let Some(served) = served else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "`serve` has not polled yet: ask again after its first poll",
        );
    };
    let (view, store) = (served.view_now(), served.store);
    let recorded = match Runs::new(&sources.work_dir.resolve()).read() {
        Ok(recorded) => recorded,
        Err(e) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}"));
        }
    };
    let now = sources.clock.now();
    if let Some(answer) = explain::explain(&view, &recorded, number, now) {
        return json_response(StatusCode::OK, &explain::Answer::Tracked(answer));
    }
    match store.any_item(number).await {
        Ok(Some(read)) => json_response(
            StatusCode::OK,
            &explain::explain_read(&view, &recorded, &read, now),
        ),
        Ok(None) => json_response(
            StatusCode::NOT_FOUND,
            &ErrorAnswer {
                error: format!("there is no issue or pull request {number}"),
                missing: Some(Missing::Item),
            },
        ),
        Err(e) => error_response(StatusCode::BAD_GATEWAY, &format!("reading {number}: {e:#}")),
    }
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
    json_response(
        status,
        &ErrorAnswer {
            error: message.to_string(),
            missing: None,
        },
    )
}

/// Why a monitoring command got no answer.
#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("`serve` is not running for {repo} on this host: nothing listens on {path}")]
    NotServing { repo: String, path: String },
    /// `serve` has no such item or run, as its answer says
    /// (`docs/specs/monitoring.md`, "Output").
    #[error("{0}")]
    NotFound(String),
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
    request(stream, resource).await
}

async fn request(stream: UnixStream, resource: &str) -> Result<Bytes, RequestError> {
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
    let answer = serde_json::from_slice::<ErrorAnswer>(&body);
    if let Ok(ErrorAnswer {
        error,
        missing: Some(_),
    }) = answer
    {
        return Err(RequestError::NotFound(error));
    }
    if !status.is_success() {
        let message = match answer {
            Ok(ErrorAnswer { error, .. }) => error,
            Err(_) => String::from_utf8_lossy(&body).into_owned(),
        };
        Err(anyhow::anyhow!("`serve` answered {status}: {message}"))?;
    }
    Ok(body)
}

/// The body of an answer that is an error.
#[derive(serde::Serialize, serde::Deserialize)]
struct ErrorAnswer {
    error: String,
    /// Set when the item or run asked about does not exist: unlike a
    /// resource that does not, such as one a newer client asks of an older
    /// `serve`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    missing: Option<Missing>,
}

/// What does not exist.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Missing {
    Item,
    Run,
}
