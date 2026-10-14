//! The local API: `serve` answers questions about itself over HTTP with
//! JSON bodies on a Unix socket, and the monitoring commands are its clients
//! (`docs/specs/architecture.md`, "The local API").

use std::fs::{DirBuilder, File, Permissions, TryLockError};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::body::Bytes;
use axum::extract::{Path as Resource, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use futures_util::StreamExt as _;
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
use crate::logs::{self, FollowQuery, Following, LogsError, NoSuchRun, Which};
use crate::runs::{self, ListError, RepoWorkDir, Runs, Watched};
use crate::status::{self, Serving, View};
use crate::store::Store;
use crate::transcript::{self, After, FollowedLine, Made, ReadTo};
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
        let stop = CancellationToken::new();
        let router = Router::new()
            .route("/status", get(answer_status))
            .route("/runs", get(answer_runs))
            .route("/items/{number}", get(answer_item))
            .route("/logs", get(answer_logs))
            .route("/logs/follow", get(follow_logs))
            .fallback(|| async { error_response(StatusCode::NOT_FOUND, "no such resource") })
            .layer(middleware::from_fn(local_only))
            // Ends the transcripts being followed, so that stopping waits
            // only for the answers being given.
            .layer(Extension(stop.clone()))
            .with_state(sources);
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

/// The type of an event of a followed transcript that is a line of it.
pub const LINE_EVENT: &str = "line";
/// The type of the event that ends a followed transcript once its run has.
pub const END_EVENT: &str = "end";
/// The type of the event that ends a followed transcript that could not be
/// followed further, saying why.
pub const ERROR_EVENT: &str = "error";
/// The header in which a reader names the last event it saw.
const LAST_EVENT_ID: &str = "last-event-id";

/// The transcript of the run the query names as server-sent events, each
/// line of it a [`LINE_EVENT`] whose ID is an [`EventId`], then an
/// [`END_EVENT`] once the run has ended, or an [`ERROR_EVENT`] if it cannot
/// be followed further. A reader that names the last event it saw in
/// `Last-Event-ID` is sent the lines after it.
async fn follow_logs(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Query(query): Query<FollowQuery>,
    headers: HeaderMap,
) -> Response {
    let last = headers.get(LAST_EVENT_ID).map(|id| {
        id.to_str()
            .ok()
            .and_then(|id| id.parse::<Resume>().ok())
            .ok_or("`Last-Event-ID` is not an event ID of a transcript")
    });
    let (which, after) = match last {
        None => (query.which, None),
        Some(Ok(Resume { run, after })) => (
            Which::Run {
                id: run,
                item: query.which.item(),
            },
            Some(after),
        ),
        Some(Err(e)) => return error_response(StatusCode::BAD_REQUEST, e),
    };
    let started = match Following::start(&sources.work_dir, &which, query.verbosity, after) {
        Ok(started) => started,
        Err(e) => return answer(Err::<(), _>(e)),
    };
    let events = futures_util::stream::unfold(Some(started), move |following| {
        let stopping = stopping.clone();
        async move {
            let mut following = following?;
            let lines = tokio::select! {
                lines = following.next() => lines,
                () = stopping.cancelled() => return None,
            };
            let events: Vec<Event> = match lines {
                Ok(Some(lines)) => lines
                    .iter()
                    .map(|FollowedLine { made, line }| {
                        let id = EventId {
                            run: following.run().clone(),
                            made: *made,
                        };
                        Event::default()
                            .event(LINE_EVENT)
                            .id(id.to_string())
                            .data(transcript::json_line(line).trim_end())
                    })
                    .collect(),
                Ok(None) => {
                    let end = serde_json::json!({"run": following.run()});
                    let end = Event::default().event(END_EVENT).data(end.to_string());
                    return Some((vec![end], None));
                }
                Err(e) => {
                    let e = format!("{e:#}");
                    error!(error = e, "following a transcript failed");
                    let error = ErrorAnswer {
                        error: e,
                        missing: None,
                    };
                    let error = serde_json::to_string(&error).expect("the answers serialize");
                    return Some((vec![Event::default().event(ERROR_EVENT).data(error)], None));
                }
            };
            Some((events, Some(following)))
        }
    })
    .flat_map(|events| {
        futures_util::stream::iter(events.into_iter().map(Ok::<_, std::convert::Infallible>))
    });
    Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// The ID of a line of a transcript being followed:
/// `<run>:<session>.<line>.<nth>`, the run's ID and where in it the line
/// was made (see [`Made`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventId {
    pub run: runs::RunId,
    pub made: Made,
}

impl std::str::FromStr for EventId {
    type Err = String;

    fn from_str(id: &str) -> Result<EventId, String> {
        match id.parse()? {
            Resume {
                run,
                after: After::Line(made),
            } => Ok(EventId { run, made }),
            _ => Err(format!("{id:?} is not an event ID")),
        }
    }
}

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Resume::from(self.clone()).fmt(f)
    }
}

/// Where a reader resumes a transcript's stream, as it names it in
/// `Last-Event-ID`: after a line, by its [`EventId`], or after every line
/// made once the transcript was read to a point, as
/// `<run>:<session>.<line>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resume {
    pub run: runs::RunId,
    pub after: After,
}

impl From<EventId> for Resume {
    fn from(id: EventId) -> Resume {
        Resume {
            run: id.run,
            after: After::Line(id.made),
        }
    }
}

impl std::str::FromStr for Resume {
    type Err = String;

    fn from_str(id: &str) -> Result<Resume, String> {
        let not_one = || format!("{id:?} is not an event ID");
        let (run, made) = id.rsplit_once(':').ok_or_else(not_one)?;
        let numbers: Vec<usize> = made
            .split('.')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| not_one())?;
        let after = match numbers[..] {
            [session, line] => After::Read(ReadTo { session, line }),
            [session, line, nth] => After::Line(Made {
                read: ReadTo { session, line },
                nth,
            }),
            _ => return Err(not_one()),
        };
        Ok(Resume {
            run: run.parse()?,
            after,
        })
    }
}

impl std::fmt::Display for Resume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.after {
            After::Read(ReadTo { session, line }) => {
                write!(f, "{}:{session}.{line}", self.run)
            }
            After::Line(Made {
                read: ReadTo { session, line },
                nth,
            }) => write!(f, "{}:{session}.{line}.{nth}", self.run),
        }
    }
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

/// `query` as the query string of a request for it.
pub fn query_string(query: &impl serde::Serialize) -> String {
    serde_urlencoded::to_string(query).expect("a query serializes")
}

/// Asks the `serve` of `repo` whose socket is in `dir` for `resource`, such
/// as `/status`, and returns its answer's body.
pub async fn get_answer(dir: &Path, repo: &str, resource: &str) -> Result<Bytes, RequestError> {
    let response = send(connect(dir, repo).await?, resource, None).await?;
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .context("reading `serve`'s answer")?
        .to_bytes();
    check(status, &body)?;
    Ok(body)
}

/// An event of a stream of server-sent events.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerEvent {
    pub id: Option<String>,
    /// Its type, `message` unless it says.
    pub event: String,
    pub data: String,
}

/// Asks the `serve` of `repo` whose socket is in `dir` for `resource` as
/// server-sent events, after the event `last_id` if one is given, and hands
/// each to `on_event` as it comes until the stream ends, or fails with the
/// error of an [`ERROR_EVENT`]. Docs:
/// <https://html.spec.whatwg.org/multipage/server-sent-events.html>.
pub async fn get_events(
    dir: &Path,
    repo: &str,
    resource: &str,
    last_id: Option<&str>,
    mut on_event: impl FnMut(ServerEvent),
) -> Result<(), RequestError> {
    let response = send(connect(dir, repo).await?, resource, last_id).await?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let content_type = content_type.as_deref();
    let mut body = response.into_body();
    if !status.is_success() {
        let body = body.collect().await.context("reading `serve`'s answer")?;
        return check(status, &body.to_bytes());
    }
    if !content_type.is_some_and(|value| value.starts_with("text/event-stream")) {
        Err(anyhow::anyhow!("`serve` did not answer with events"))?;
    }
    let mut unread = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.context("reading `serve`'s events")?;
        let Some(data) = frame.data_ref() else {
            continue;
        };
        unread.extend_from_slice(data);
        while let Some(end) = unread.windows(2).position(|pair| pair == b"\n\n") {
            let block: Vec<u8> = unread.drain(..end + 2).collect();
            match parse_event(&String::from_utf8_lossy(&block)) {
                Some(event) if event.event == ERROR_EVENT => {
                    let error = serde_json::from_str::<ErrorAnswer>(&event.data)
                        .map_or(event.data, |answer| answer.error);
                    Err(anyhow::anyhow!("`serve` could not go on: {error}"))?;
                }
                Some(event) => on_event(event),
                None => {}
            }
        }
    }
    Ok(())
}

/// The event `block` of a stream holds, if it holds one rather than only
/// comments.
fn parse_event(block: &str) -> Option<ServerEvent> {
    let mut event = ServerEvent {
        event: "message".into(),
        ..ServerEvent::default()
    };
    let mut data: Option<String> = None;
    for line in block.lines() {
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "id" => event.id = Some(value.to_string()),
            "event" => event.event = value.to_string(),
            "data" => match &mut data {
                Some(data) => {
                    data.push('\n');
                    data.push_str(value);
                }
                None => data = Some(value.to_string()),
            },
            _ => {}
        }
    }
    event.data = data?;
    Some(event)
}

/// Connects to the socket of the `serve` of `repo` in `dir`.
async fn connect(dir: &Path, repo: &str) -> Result<UnixStream, RequestError> {
    let path = socket_path(dir, repo);
    match UnixStream::connect(&path).await {
        Ok(stream) => Ok(stream),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            Err(RequestError::NotServing {
                repo: repo.to_string(),
                path: path.display().to_string(),
            })
        }
        Err(e) => Err(e).with_context(|| format!("connecting to {}", path.display()))?,
    }
}

/// Sends `GET resource` on `stream`, naming `last_id` as the last event seen
/// if one is given.
async fn send(
    stream: UnixStream,
    resource: &str,
    last_id: Option<&str>,
) -> Result<hyper::Response<hyper::body::Incoming>, RequestError> {
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .context("talking to `serve`")?;
    tokio::spawn(connection);
    let mut request = hyper::Request::get(resource).header(header::HOST, "localhost");
    if let Some(id) = last_id {
        request = request.header(LAST_EVENT_ID, id);
    }
    let request = request
        .body(Empty::<Bytes>::new())
        .context("building the request")?;
    Ok(sender
        .send_request(request)
        .await
        .context("asking `serve`")?)
}

/// Fails unless `status` and `body` are an answer rather than an error.
fn check(status: StatusCode, body: &Bytes) -> Result<(), RequestError> {
    let answer = serde_json::from_slice::<ErrorAnswer>(body);
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
            Err(_) => String::from_utf8_lossy(body).into_owned(),
        };
        Err(anyhow::anyhow!("`serve` answered {status}: {message}"))?;
    }
    Ok(())
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
