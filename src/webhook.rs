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

use crate::domain::{Item, Number};

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    Issue(Number),
    PullRequest(Number),
    /// The pull requests a check run or check suite ran for; it may be none.
    Checks(Vec<Number>),
    /// The branch pushed to, or those whose head's status changed.
    Branches(Vec<String>),
}

impl Subject {
    /// The issues and pull requests a refresh reads again for it. Checks and
    /// branches name none yet: what CI and pushes change is left to the poll.
    fn items(&self) -> Vec<Item> {
        match self {
            Subject::Issue(number) => vec![Item::Issue(*number)],
            Subject::PullRequest(number) => vec![Item::Pull(*number)],
            Subject::Checks(_) | Subject::Branches(_) => vec![],
        }
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Subject::Issue(number) => write!(f, "issue {number}"),
            Subject::PullRequest(number) => write!(f, "pull request {number}"),
            Subject::Checks(numbers) => list(f, ("pull request", "pull requests"), numbers),
            Subject::Branches(branches) => list(f, ("branch", "branches"), branches),
        }
    }
}

/// `items` after the word for one of them or for more.
fn list(
    f: &mut fmt::Formatter<'_>,
    (one, more): (&str, &str),
    items: &[impl fmt::Display],
) -> fmt::Result {
    let items: Vec<String> = items.iter().map(ToString::to_string).collect();
    match items.as_slice() {
        [] => write!(f, "no {one}"),
        [item] => write!(f, "{one} {item}"),
        items => write!(f, "{more} {}", items.join(", ")),
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
    /// A check run or check suite, by the pull requests it ran for.
    #[derive(Deserialize)]
    struct CheckPullRequests {
        pull_requests: Vec<Numbered>,
    }
    #[derive(Deserialize)]
    struct CheckRunEvent {
        check_run: CheckPullRequests,
    }
    #[derive(Deserialize)]
    struct CheckSuiteEvent {
        check_suite: CheckPullRequests,
    }
    #[derive(Deserialize)]
    struct Branch {
        name: String,
    }
    #[derive(Deserialize)]
    struct StatusEvent {
        branches: Vec<Branch>,
    }
    #[derive(Deserialize)]
    struct PushEvent {
        #[serde(rename = "ref")]
        reference: String,
    }
    let pull_requests = |check: CheckPullRequests| {
        Subject::Checks(
            check
                .pull_requests
                .into_iter()
                .map(|pr| pr.number)
                .collect(),
        )
    };
    let subject = match event {
        "issues" | "issue_comment" => {
            let IssueEvent { issue } = from_slice(body)?;
            match issue.pull_request {
                Some(_) => Subject::PullRequest(issue.number),
                None => Subject::Issue(issue.number),
            }
        }
        "pull_request" | "pull_request_review" | "pull_request_review_comment" => {
            let PullRequestEvent { pull_request } = from_slice(body)?;
            Subject::PullRequest(pull_request.number)
        }
        "check_run" => pull_requests(from_slice::<CheckRunEvent>(body)?.check_run),
        "check_suite" => pull_requests(from_slice::<CheckSuiteEvent>(body)?.check_suite),
        "status" => {
            let StatusEvent { branches } = from_slice(body)?;
            Subject::Branches(branches.into_iter().map(|branch| branch.name).collect())
        }
        "push" => {
            let PushEvent { reference } = from_slice(body)?;
            match reference.strip_prefix("refs/heads/") {
                Some(branch) => Subject::Branches(vec![branch.to_string()]),
                None => return Ok(Delivery::Ignored),
            }
        }
        _ => return Ok(Delivery::Ignored),
    };
    Ok(Delivery::Accepted(subject))
}

/// The items accepted deliveries named that the factory has yet to read
/// again: the listener adds to them and the factory takes them, so deliveries
/// never race a poll, and those that arrive while it reads coalesce.
#[derive(Clone, Default)]
pub struct Pending(Arc<PendingItems>);

#[derive(Default)]
struct PendingItems {
    items: Mutex<BTreeSet<Item>>,
    arrived: Notify,
}

impl Pending {
    fn add(&self, items: Vec<Item>) {
        if items.is_empty() {
            return;
        }
        self.0.items.lock().unwrap().extend(items);
        self.0.arrived.notify_one();
    }

    /// Waits until a delivery names an item, and takes every item named
    /// since the last take.
    pub async fn next(&self) -> BTreeSet<Item> {
        loop {
            let items = self.take();
            if !items.is_empty() {
                return items;
            }
            self.0.arrived.notified().await;
        }
    }

    /// Takes every item named since the last take.
    pub fn take(&self) -> BTreeSet<Item> {
        std::mem::take(&mut *self.0.items.lock().unwrap())
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
            pending.add(subject.items());
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
