use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use anyhow::{Context, bail};
use axum::Router;
use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, info_span, warn};

use crate::clock::Clock;
use crate::domain::{Item, Number, Snapshot, Timestamp};

pub const SECRET_VARIABLE: &str = "LINEWRIGHT_WEBHOOK_SECRET";

#[derive(Clone, Debug)]
pub struct Webhooks {
    pub address: Address,
    pub secret: Secret,
}

#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Option<Secret> {
        (!value.is_empty()).then_some(Secret(value))
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    /// Without the brackets of an IPv6 address.
    host: String,
    port: u16,
}

impl Address {
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Subject {
    Item(Item),
    Commit(String),
    Push(String),
}

impl Subject {
    pub fn items(&self, snapshot: &Snapshot) -> Vec<Item> {
        let open = snapshot.pulls.iter().filter(|pull| pull.is_open());
        match self {
            Subject::Item(item) => vec![*item],
            Subject::Commit(sha) => open
                .filter(|pull| pull.head_sha == *sha)
                .map(|pull| Item::Pull(pull.number))
                .collect(),
            Subject::Push(branch) if *branch == snapshot.default_branch => {
                open.map(|pull| Item::Pull(pull.number)).collect()
            }
            Subject::Push(_) => vec![],
        }
    }
}

impl From<Item> for Subject {
    fn from(item: Item) -> Subject {
        Subject::Item(item)
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Subject::Item(item) => write!(f, "{item}"),
            Subject::Commit(sha) => write!(f, "commit {sha}"),
            Subject::Push(branch) => write!(f, "a push to branch {branch}"),
        }
    }
}

#[derive(Debug)]
pub enum Delivery {
    Accepted(Subject),
    Ignored,
}

#[derive(Debug, thiserror::Error)]
pub enum Refusal {
    #[error("it has no signature (`X-Hub-Signature-256`)")]
    NoSignature,
    #[error("its signature does not match the secret")]
    WrongSignature,
    #[error(transparent)]
    Unreadable(#[from] Unreadable),
}

impl Refusal {
    /// Only a signed delivery's says why, so a sender without the secret
    /// learns nothing.
    pub fn response(&self) -> (StatusCode, String) {
        match self {
            Refusal::NoSignature | Refusal::WrongSignature => {
                (StatusCode::UNAUTHORIZED, String::new())
            }
            Refusal::Unreadable(reason) => {
                (StatusCode::BAD_REQUEST, format!("refused: {reason}\n"))
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Unreadable {
    #[error("it names no event (`X-GitHub-Event`)")]
    NoEvent,
    #[error(
        "its content type (`Content-Type`) is {}, and Linewright reads only `application/json` and `application/x-www-form-urlencoded`",
        .0.as_deref().map_or("missing".into(), |got| format!("`{got}`"))
    )]
    ContentType(Option<String>),
    #[error("it is form-encoded and has no `payload` field")]
    NoPayloadField,
    #[error("Linewright cannot tell what its `{event}` payload names: {error}")]
    Payload {
        event: String,
        error: serde_json::Error,
    },
}

/// <https://docs.github.com/en/webhooks/webhook-events-and-payloads>
#[derive(Clone, Copy)]
enum Event {
    Issue,
    PullRequest,
    CheckRun,
    CheckSuite,
    Status,
    Push,
}

impl Event {
    fn parse(name: &str) -> Option<Event> {
        Some(match name {
            "issues" | "issue_comment" => Event::Issue,
            "pull_request" | "pull_request_review" | "pull_request_review_comment" => {
                Event::PullRequest
            }
            "check_run" => Event::CheckRun,
            "check_suite" => Event::CheckSuite,
            "status" => Event::Status,
            "push" => Event::Push,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Headers<'a> {
    pub event: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub signature: Option<&'a str>,
}

/// <https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries>
pub fn receive(secret: &Secret, headers: Headers, body: &[u8]) -> Result<Delivery, Refusal> {
    use hmac::Mac;
    let signature = headers.signature.ok_or(Refusal::NoSignature)?;
    let signature = signature
        .strip_prefix("sha256=")
        .and_then(|hex| hex::decode(hex).ok())
        .ok_or(Refusal::WrongSignature)?;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.0.as_bytes())
        .expect("HMAC takes a key of any length");
    mac.update(body);
    // In constant time, so the signature cannot be found by timing.
    mac.verify_slice(&signature)
        .map_err(|_| Refusal::WrongSignature)?;
    let name = headers.event.ok_or(Unreadable::NoEvent)?;
    let Some(table_event) = Event::parse(name) else {
        return Ok(Delivery::Ignored);
    };
    let payload = Format::parse(headers.content_type)?.payload(body)?;
    read_delivery(table_event, &payload).map_err(|error| {
        Unreadable::Payload {
            event: name.to_string(),
            error,
        }
        .into()
    })
}

/// <https://docs.github.com/en/webhooks/using-webhooks/creating-webhooks#creating-a-repository-webhook>
#[derive(Clone, Copy)]
enum Format {
    Json,
    Form,
}

impl Format {
    /// <https://www.rfc-editor.org/rfc/rfc9110#name-media-type>
    fn parse(content_type: Option<&str>) -> Result<Format, Unreadable> {
        let unreadable = || Unreadable::ContentType(content_type.map(str::to_string));
        let media_type = content_type
            .and_then(|header| header.split(';').next())
            .map(str::trim)
            .ok_or_else(unreadable)?;
        if media_type.eq_ignore_ascii_case("application/json") {
            Ok(Format::Json)
        } else if media_type.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
            Ok(Format::Form)
        } else {
            Err(unreadable())
        }
    }

    fn payload(self, body: &[u8]) -> Result<Cow<'_, [u8]>, Unreadable> {
        match self {
            Format::Json => Ok(Cow::Borrowed(body)),
            Format::Form => form_urlencoded::parse(body)
                .find(|(name, _)| name == "payload")
                .map(|(_, payload)| Cow::Owned(payload.into_owned().into_bytes()))
                .ok_or(Unreadable::NoPayloadField),
        }
    }
}

fn read_delivery(event: Event, body: &[u8]) -> serde_json::Result<Delivery> {
    use serde_json::from_slice;
    #[derive(Deserialize)]
    struct Numbered {
        number: Number,
    }
    #[derive(Deserialize)]
    struct Issue {
        number: Number,
        pull_request: Option<serde::de::IgnoredAny>,
    }
    #[derive(Deserialize)]
    struct IssueEvent {
        issue: Issue,
    }
    #[derive(Deserialize)]
    struct PullRequestEvent {
        pull_request: Numbered,
    }
    #[derive(Deserialize)]
    struct Check {
        head_sha: String,
    }
    #[derive(Deserialize)]
    struct CheckRunEvent {
        check_run: Check,
    }
    #[derive(Deserialize)]
    struct CheckSuiteEvent {
        check_suite: Check,
    }
    #[derive(Deserialize)]
    struct StatusEvent {
        sha: String,
    }
    #[derive(Deserialize)]
    struct PushEvent {
        #[serde(rename = "ref")]
        reference: String,
    }
    let subject = match event {
        Event::Issue => {
            let IssueEvent { issue } = from_slice(body)?;
            Subject::Item(match issue.pull_request {
                Some(_) => Item::Pull(issue.number),
                None => Item::Issue(issue.number),
            })
        }
        Event::PullRequest => {
            let PullRequestEvent { pull_request } = from_slice(body)?;
            Subject::Item(Item::Pull(pull_request.number))
        }
        Event::CheckRun => Subject::Commit(from_slice::<CheckRunEvent>(body)?.check_run.head_sha),
        Event::CheckSuite => {
            Subject::Commit(from_slice::<CheckSuiteEvent>(body)?.check_suite.head_sha)
        }
        Event::Status => Subject::Commit(from_slice::<StatusEvent>(body)?.sha),
        Event::Push => {
            let PushEvent { reference } = from_slice(body)?;
            match reference.strip_prefix("refs/heads/") {
                Some(branch) => Subject::Push(branch.to_string()),
                None => return Ok(Delivery::Ignored),
            }
        }
    };
    Ok(Delivery::Accepted(subject))
}

/// The listener adds to it and the factory takes it, so deliveries never race
/// a poll, and those that arrive while it reads coalesce.
#[derive(Clone, Default)]
pub struct Pending(Arc<PendingSubjects>);

#[derive(Default)]
struct PendingSubjects {
    subjects: Mutex<BTreeSet<Subject>>,
    arrived: Notify,
}

impl Pending {
    fn add(&self, subject: Subject) {
        self.0.subjects.lock().unwrap().insert(subject);
        self.0.arrived.notify_one();
    }

    pub async fn next(&self) -> BTreeSet<Subject> {
        loop {
            let subjects = self.take();
            if !subjects.is_empty() {
                return subjects;
            }
            self.0.arrived.notified().await;
        }
    }

    pub fn take(&self) -> BTreeSet<Subject> {
        std::mem::take(&mut *self.0.subjects.lock().unwrap())
    }
}

/// Ignored deliveries count as accepted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Received {
    pub last_accepted: Option<Timestamp>,
    pub refused: u64,
    /// Absent from the answers of a `serve` from before it was kept.
    #[serde(default)]
    pub last_refused: Option<Timestamp>,
}

impl Received {
    /// When a delivery was refused with none accepted since, if one was
    /// (`docs/specs/design_system.md`, Holds and problems).
    pub fn refused_since_accepted(&self) -> Option<Timestamp> {
        self.last_refused.filter(|refused| {
            self.last_accepted
                .is_none_or(|accepted| accepted < *refused)
        })
    }
}

#[derive(Clone)]
pub struct Tally {
    received: Arc<Mutex<Received>>,
    clock: Arc<dyn Clock>,
}

impl Tally {
    pub fn get(&self) -> Received {
        *self.received.lock().unwrap()
    }

    fn accepted(&self) {
        self.received.lock().unwrap().last_accepted = Some(self.clock.now());
    }

    fn refused(&self) {
        let mut received = self.received.lock().unwrap();
        received.refused += 1;
        received.last_refused = Some(self.clock.now());
    }
}

pub struct Listener {
    address: SocketAddr,
    tally: Tally,
    stop: CancellationToken,
    task: JoinHandle<()>,
}

/// <https://docs.github.com/en/webhooks/webhook-events-and-payloads#payload-cap>
const PAYLOAD_CAP: usize = 25 * 1024 * 1024;

impl Listener {
    pub async fn bind(
        webhooks: &Webhooks,
        pending: Pending,
        clock: Arc<dyn Clock>,
    ) -> anyhow::Result<Listener> {
        let address = &webhooks.address;
        let socket = TcpListener::bind((address.host(), address.port()))
            .await
            .with_context(|| format!("listening on the setting `webhook-address`, {address}"))?;
        let local = socket.local_addr()?;
        let secret = webhooks.secret.clone();
        let tally = Tally {
            received: Arc::default(),
            clock,
        };
        let router = Router::new()
            .fallback({
                let tally = tally.clone();
                move |headers, body| {
                    std::future::ready(answer(&secret, &pending, &tally, &headers, &body))
                }
            })
            .layer(DefaultBodyLimit::max(PAYLOAD_CAP));
        let stop = CancellationToken::new();
        let task = tokio::spawn({
            let stop = stop.clone();
            async move {
                let served = axum::serve(socket, router)
                    .with_graceful_shutdown(stop.cancelled_owned())
                    .await;
                if let Err(e) = served {
                    error!(error = %e, "receiving webhook deliveries failed");
                }
            }
        });
        info!(address = %local, "listening for webhook deliveries");
        Ok(Listener {
            address: local,
            tally,
            stop,
            task,
        })
    }

    /// The address it listens on, with the port the system chose for port 0.
    pub fn local_address(&self) -> SocketAddr {
        self.address
    }

    pub fn tally(&self) -> Tally {
        self.tally.clone()
    }

    pub async fn stop(self) {
        self.stop.cancel();
        let _ = self.task.await;
    }
}

fn answer(
    secret: &Secret,
    pending: &Pending,
    tally: &Tally,
    headers: &HeaderMap,
    body: &Bytes,
) -> (StatusCode, String) {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let event = header("X-GitHub-Event");
    let span = info_span!(
        "webhook_delivery",
        event = %event.unwrap_or("none"),
        delivery = %header("X-GitHub-Delivery").unwrap_or("none"),
    );
    let _entered = span.enter();
    let read_by = Headers {
        event,
        content_type: header("Content-Type"),
        signature: header("X-Hub-Signature-256"),
    };
    match receive(secret, read_by, body) {
        Ok(Delivery::Accepted(subject)) => {
            info!(%subject, "accepted");
            pending.add(subject);
        }
        Ok(Delivery::Ignored) => info!("ignored: not a change Linewright acts on"),
        Err(refusal) => {
            warn!(reason = %refusal, "refused");
            tally.refused();
            return refusal.response();
        }
    }
    tally.accepted();
    (StatusCode::ACCEPTED, String::new())
}
