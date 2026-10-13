//! Webhooks: GitHub's signed deliveries, received beside polling
//! (`docs/specs/product.md`, "Webhooks").

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
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, info_span, warn};

use crate::domain::{Item, Number, Snapshot};

/// The environment variable the webhook's secret comes from. The secret is a
/// credential, so it is never a flag or a key in the config file.
pub const SECRET_VARIABLE: &str = "LINEWRIGHT_WEBHOOK_SECRET";

/// What `serve` needs to receive deliveries: set exactly when the secret is.
#[derive(Clone, Debug)]
pub struct Webhooks {
    pub address: Address,
    pub secret: Secret,
}

/// The secret GitHub signs each delivery with, kept out of `Debug` output.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// The secret, unless `value` is empty: an empty one is unset, as with
    /// the tokens.
    pub fn new(value: String) -> Option<Secret> {
        (!value.is_empty()).then_some(Secret(value))
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// A host and port to listen on, such as `0.0.0.0:8080`, `localhost:8080`
/// or `[::1]:8080`.
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

/// What an accepted delivery names: what Linewright reads again for it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Subject {
    Item(Item),
    /// The commit a check run, check suite or status reports on.
    Commit(String),
    /// A push to the branch.
    Push(String),
}

impl Subject {
    /// The issues and pull requests a refresh reads again for it, found in
    /// `snapshot`, whose pull requests are the machine user's: the open ones
    /// whose head is the commit, and every open one for a push to the
    /// default branch.
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

/// A delivery whose signature matches, which is never refused.
#[derive(Debug)]
pub enum Delivery {
    /// One of the events Linewright acts on, and what it names.
    Accepted(Subject),
    /// Any other event, or a push to something other than a branch.
    Ignored,
    /// One whose event or payload cannot be read, such as one GitHub sends
    /// form-encoded.
    Unreadable(String),
}

/// Why a delivery is refused.
#[derive(Debug, thiserror::Error)]
pub enum Refusal {
    #[error("it has no signature (`X-Hub-Signature-256`)")]
    NoSignature,
    #[error("its signature does not match the secret")]
    WrongSignature,
}

/// Checks a delivery's signature against `secret`
/// (<https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries>)
/// and reads what it names from `body`.
pub fn receive(
    secret: &Secret,
    event: Option<&str>,
    signature: Option<&str>,
    body: &[u8],
) -> Result<Delivery, Refusal> {
    use hmac::Mac;
    let signature = signature.ok_or(Refusal::NoSignature)?;
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
    let Some(event) = event else {
        return Ok(Delivery::Unreadable(
            "it names no event (`X-GitHub-Event`)".into(),
        ));
    };
    Ok(read_delivery(event, body).unwrap_or_else(|error| {
        Delivery::Unreadable(format!(
            "its payload is not JSON as GitHub sends it with the content type \
             `application/json`: {error}"
        ))
    }))
}

/// What a delivery of `event` names, from the parts of its payload
/// (<https://docs.github.com/en/webhooks/webhook-events-and-payloads>) that
/// say which item changed.
fn read_delivery(event: &str, body: &[u8]) -> serde_json::Result<Delivery> {
    use serde::Deserialize;
    use serde_json::from_slice;
    #[derive(Deserialize)]
    struct Numbered {
        number: Number,
    }
    #[derive(Deserialize)]
    struct Issue {
        number: Number,
        /// Set when the issue is a pull request.
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
    /// A check run or check suite, by the commit it ran on.
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
        "issues" | "issue_comment" => {
            let IssueEvent { issue } = from_slice(body)?;
            Subject::Item(match issue.pull_request {
                Some(_) => Item::Pull(issue.number),
                None => Item::Issue(issue.number),
            })
        }
        "pull_request" | "pull_request_review" | "pull_request_review_comment" => {
            let PullRequestEvent { pull_request } = from_slice(body)?;
            Subject::Item(Item::Pull(pull_request.number))
        }
        "check_run" => Subject::Commit(from_slice::<CheckRunEvent>(body)?.check_run.head_sha),
        "check_suite" => Subject::Commit(from_slice::<CheckSuiteEvent>(body)?.check_suite.head_sha),
        "status" => Subject::Commit(from_slice::<StatusEvent>(body)?.sha),
        "push" => {
            let PushEvent { reference } = from_slice(body)?;
            match reference.strip_prefix("refs/heads/") {
                Some(branch) => Subject::Push(branch.to_string()),
                None => return Ok(Delivery::Ignored),
            }
        }
        _ => return Ok(Delivery::Ignored),
    };
    Ok(Delivery::Accepted(subject))
}

/// What accepted deliveries named that the factory has yet to read again:
/// the listener adds to it and the factory takes it, so deliveries never race
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

    /// Waits until a delivery names something, and takes everything named
    /// since the last take.
    pub async fn next(&self) -> BTreeSet<Subject> {
        loop {
            let subjects = self.take();
            if !subjects.is_empty() {
                return subjects;
            }
            self.0.arrived.notified().await;
        }
    }

    /// Takes everything named since the last take.
    pub fn take(&self) -> BTreeSet<Subject> {
        std::mem::take(&mut *self.0.subjects.lock().unwrap())
    }
}

/// The HTTP server `serve` runs with webhooks on, until stopped.
pub struct Listener {
    address: SocketAddr,
    stop: CancellationToken,
    task: JoinHandle<()>,
}

/// GitHub's cap on a delivery's payload
/// (<https://docs.github.com/en/webhooks/webhook-events-and-payloads#payload-cap>).
const PAYLOAD_CAP: usize = 25 * 1024 * 1024;

impl Listener {
    /// Listens on `webhooks.address`, or refuses when it cannot, adding what
    /// accepted deliveries name to `pending`.
    pub async fn bind(webhooks: &Webhooks, pending: Pending) -> anyhow::Result<Listener> {
        let address = &webhooks.address;
        let socket = TcpListener::bind((address.host(), address.port()))
            .await
            .with_context(|| format!("listening on the setting `webhook-address`, {address}"))?;
        let local = socket.local_addr()?;
        let secret = webhooks.secret.clone();
        let router = Router::new()
            .fallback(move |headers, body| answer(secret.clone(), pending.clone(), headers, body))
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
            stop,
            task,
        })
    }

    /// The address it listens on, with the port the system chose for port 0.
    pub fn local_address(&self) -> SocketAddr {
        self.address
    }

    /// Stops listening, once the deliveries being received are answered.
    pub async fn stop(self) {
        self.stop.cancel();
        let _ = self.task.await;
    }
}

/// Answers a delivery, and logs what became of it.
async fn answer(secret: Secret, pending: Pending, headers: HeaderMap, body: Bytes) -> StatusCode {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let event = header("X-GitHub-Event");
    let span = info_span!(
        "webhook_delivery",
        event = %event.unwrap_or("none"),
        delivery = %header("X-GitHub-Delivery").unwrap_or("none"),
    );
    let _entered = span.enter();
    match receive(&secret, event, header("X-Hub-Signature-256"), &body) {
        Ok(Delivery::Accepted(subject)) => {
            info!(%subject, "accepted");
            pending.add(subject);
        }
        Ok(Delivery::Ignored) => info!("ignored: not a change Linewright acts on"),
        Ok(Delivery::Unreadable(reason)) => warn!(%reason, "ignored"),
        Err(refusal) => {
            warn!(reason = %refusal, "refused");
            return StatusCode::UNAUTHORIZED;
        }
    }
    StatusCode::ACCEPTED
}
