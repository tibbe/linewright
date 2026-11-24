use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use base64::Engine;
use futures_util::future::try_join_all;
use linewright_core::store::{
    API_VERSION, AnyItem, AttachError, Blob, IssueRead, NewIssue, NewPull, PeoplesPullRead,
    Property, PushError, PushMode, RecordedItem, RecordedProperties, Resource, Store, holds,
    hour_of, properties, read_openings,
};
use reqwest::{Method, StatusCode};
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod job_log;
pub mod rate_limit;
use linewright_core::domain::{
    Assignment, BranchIssue, Check, CheckFailure, CheckLog, CheckState, ClosedItem, ClosedKind,
    ClosingPull, Comment, Filed, Issue, IssueCloseReason, IssueEvent, Item, JobLog, Labeling,
    Linked, Login, ManagedFacts, MergeMethod, Mergeability, MergedPull, Number, Opened, Opening,
    Parent, Pull, PullHeld, PullSigns, PullState, Push, Rebaseability, Repo, Requirement, Review,
    ReviewDecision, ReviewState, Scope, Snapshot, SpecChangeIssue, StatusLabel, SubIssue,
    SubIssueState, Thread, TicketBranch, TicketBranchName, Timestamp, Windows, WorkFacts,
    closed_within, label,
};
use linewright_core::git;
use linewright_core::http::{client_builder_trusting_the_host_store_alone, next_link};
use linewright_core::workflow_run::{
    AttemptConclusion, AttemptId, CompletedAttempt, WorkflowRunId,
};

const API: &str = "https://api.github.com";

/// Where attachments are uploaded, as gh uploads them (`internal/attachments`
/// in <https://github.com/cli/cli>).
const ATTACHMENTS: &str = "https://uploads.github.com/user-attachments/assets";

#[derive(Clone)]
pub struct GitHub {
    http: reqwest::Client,
    /// Where its API answers: [`API`], but for a test's server.
    api: String,
    token: String,
    owner: String,
    name: String,
    viewer: Viewer,
    /// The repository's numeric id, read once.
    repository_id: Arc<tokio::sync::OnceCell<u64>>,
    /// The openings read, by number: who opened an item, and what for,
    /// never changes.
    openings: Arc<std::sync::Mutex<BTreeMap<Number, Opening>>>,
    sending: Arc<Sending>,
    pushes_kept: Arc<PushesKept>,
    runs_listed: Arc<RunsListed>,
}

/// The most requests in flight at once. GitHub takes at most 100 and asks
/// clients to keep fewer
/// (<https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#about-secondary-rate-limits>).
const IN_FLIGHT: usize = 16;

/// How a store's requests are sent ([`waiting_out_rate_limits`]): at most
/// [`IN_FLIGHT`] at once, and none while a rate limit GitHub gave one of
/// them is in force, since GitHub may ban an integration that keeps sending
/// then
/// (<https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#exceeding-the-rate-limit>).
struct Sending {
    in_flight: tokio::sync::Semaphore,
    limited_until: std::sync::Mutex<Option<tokio::time::Instant>>,
}

impl Default for Sending {
    fn default() -> Self {
        Sending {
            in_flight: tokio::sync::Semaphore::new(IN_FLIGHT),
            limited_until: std::sync::Mutex::default(),
        }
    }
}

impl Sending {
    /// Holds back every request for `wait` from now, or longer when a
    /// limit given before lasts longer.
    fn limit_for(&self, wait: std::time::Duration) {
        let until = tokio::time::Instant::now() + wait;
        let mut limited = self
            .limited_until
            .lock()
            .expect("the rate limit is readable");
        *limited = Some(limited.map_or(until, |before| before.max(until)));
    }

    /// Once the rate limits given so far are waited out.
    async fn limits_waited_out(&self) {
        let until = *self
            .limited_until
            .lock()
            .expect("the rate limit is readable");
        if let Some(until) = until {
            tokio::time::sleep_until(until).await;
        }
    }
}

/// The pushes read of each branch, at the head they were read at: a
/// branch whose head has not moved has had no push since.
#[derive(Default)]
struct PushesKept(std::sync::Mutex<BTreeMap<String, PushesRead>>);

struct PushesRead {
    head: String,
    pushes: Vec<Push>,
}

impl PushesKept {
    fn kept(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, PushesRead>> {
        self.0.lock().expect("the pushes kept are readable")
    }

    /// The pushes kept of `branch`, if they were read at `head`.
    fn at(&self, branch: &str, head: &str) -> Option<Vec<Push>> {
        self.kept()
            .get(branch)
            .filter(|read| read.head == head)
            .map(|read| read.pushes.clone())
    }

    fn keep(&self, branch: &str, head: &str, pushes: Vec<Push>) {
        self.kept().insert(
            branch.to_string(),
            PushesRead {
                head: head.to_string(),
                pushes,
            },
        );
    }

    /// Forgets the pushes of every branch but `branches`.
    fn keep_only(&self, branches: &BTreeSet<&str>) {
        self.kept()
            .retain(|branch, _| branches.contains(branch.as_str()));
    }
}

/// The rules of each branch, read at most once for all the pull requests
/// read with it, unless the read fails.
#[derive(Default)]
struct RulesRead(std::sync::Mutex<BTreeMap<String, Arc<tokio::sync::OnceCell<Arc<BranchRules>>>>>);

impl RulesRead {
    fn of(&self, branch: &str) -> Arc<tokio::sync::OnceCell<Arc<BranchRules>>> {
        self.0
            .lock()
            .expect("the rules read are readable")
            .entry(branch.to_string())
            .or_default()
            .clone()
    }
}

macro_rules! closes_field {
    () => {
        "closingIssuesReferences(first: 20) { nodes { number } }"
    };
}

/// The reviews requested, read as [`GqlReviewRequest`]s.
macro_rules! review_requests_field {
    () => {
        "reviewRequests(first: 20) {
    nodes { asCodeOwner requestedReviewer { __typename ... on User { login } } }
  }"
    };
}

/// The reviews requested and who reviewed, read as [`GqlReviewRequest`]s
/// and [`GqlReviewAuthor`]s.
macro_rules! reviewing_fields {
    () => {
        concat!(
            review_requests_field!(),
            "
  reviews(last: 100) { nodes { author { __typename login } submittedAt } }"
        )
    };
}

macro_rules! assignment_fields {
    () => {
        "... on AssignedEvent {
        createdAt
        actor { __typename login }
        assignee { __typename ... on Actor { login } }
      }
      ... on UnassignedEvent {
        createdAt
        actor { __typename login }
        assignee { __typename ... on Actor { login } }
      }"
    };
}

macro_rules! labeling_fields {
    () => {
        "createdAt actor { __typename login } label { name }"
    };
}

/// A comment as a timeline item, read as a [`GqlEvent`].
macro_rules! comment_fields {
    () => {
        "... on IssueComment { createdAt author { __typename login } }"
    };
}

/// Who added each sub-issue ([`SubIssue::added_by`]), read as
/// [`GqlSubIssueAdded`]s. GitHub writes an add a moment after the sub-issue,
/// so a read in between has no adder.
macro_rules! sub_issue_adds {
    () => {
        "subIssueAdds: timelineItems(last: 100, itemTypes: [SUB_ISSUE_ADDED_EVENT]) { nodes { ... on SubIssueAddedEvent { actor { __typename login } subIssue { number } } } }"
    };
}

/// A pull request's labelings and assignments, which tell whether
/// Linewright worked it ([`PullSigns`]), and its comments, read as
/// [`GqlEvent`]s.
macro_rules! work_timeline {
    () => {
        concat!(
            "timelineItems(last: 100, itemTypes: [LABELED_EVENT, ASSIGNED_EVENT, ISSUE_COMMENT]) {
    nodes {
      __typename
      ... on LabeledEvent { ",
            labeling_fields!(),
            " }
      ",
            assignment_fields!(),
            "
      ",
            comment_fields!(),
            "
    }
  }"
        )
    };
}

#[derive(Debug)]
struct ErrorResponse {
    request: String,
    status: StatusCode,
    body: String,
}

impl ErrorResponse {
    fn message(&self) -> String {
        #[derive(Deserialize)]
        struct Body {
            message: String,
        }
        serde_json::from_str::<Body>(&self.body)
            .map(|b| b.message)
            .unwrap_or_else(|_| format!("{}: {}", self.status, self.body))
    }

    /// The message, followed by each of the errors it lists
    /// (<https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api>).
    fn reasons(&self) -> String {
        #[derive(Deserialize)]
        struct Body {
            message: String,
            #[serde(default)]
            errors: Vec<Cause>,
        }
        #[derive(Deserialize)]
        struct Cause {
            message: Option<String>,
            resource: Option<String>,
            field: Option<String>,
            code: Option<String>,
        }
        let Ok(body) = serde_json::from_str::<Body>(&self.body) else {
            return self.message();
        };
        let causes = body.errors.into_iter().filter_map(|cause| match cause {
            Cause {
                message: Some(message),
                ..
            } => Some(message),
            Cause {
                resource: Some(resource),
                field: Some(field),
                code: Some(code),
                ..
            } => Some(format!("{resource}.{field} is {code}")),
            Cause { code, .. } => code,
        });
        std::iter::once(body.message)
            .chain(causes)
            .collect::<Vec<_>>()
            .join("; ")
    }
}

impl std::fmt::Display for ErrorResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}: {}", self.request, self.status, self.body)
    }
}

impl std::error::Error for ErrorResponse {}

/// One answer GitHub gave.
struct Answer {
    status: StatusCode,
    headers: reqwest::header::HeaderMap,
    body: String,
}

impl Answer {
    /// GitHub's refusal of `request` this answer gives, or its failure to
    /// carry it out.
    fn refusal(self, request: String) -> ErrorResponse {
        ErrorResponse {
            request,
            status: self.status,
            body: self.body,
        }
    }

    async fn read(response: reqwest::Response) -> Result<Answer> {
        Ok(Answer {
            status: response.status(),
            headers: response.headers().clone(),
            body: response.text().await?,
        })
    }
}

/// The answer to `request`, which `send` makes as `sending` lets it, made
/// again once each rate limit is waited out, at most
/// [`rate_limit::RETRIES`] times: a rate limit still in force after them
/// fails it. A rate limit holds back every request of `sending` until it is
/// waited out.
async fn waiting_out_rate_limits<F, Fut>(
    request: &str,
    sending: &Sending,
    mut send: F,
) -> Result<Answer>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Answer>>,
{
    let mut retries = 0;
    loop {
        sending.limits_waited_out().await;
        let answer = {
            let _in_flight = sending.in_flight.acquire().await?;
            send().await?
        };
        let wait = rate_limit::wait(
            answer.status,
            &answer.headers,
            &answer.body,
            retries,
            SystemTime::now(),
        );
        match wait {
            None => return Ok(answer),
            Some(wait) if retries < rate_limit::RETRIES => {
                tracing::warn!(request, wait = %humantime::format_duration(wait), "rate limited");
                sending.limit_for(wait);
                retries += 1;
            }
            Some(_) => {
                return Err(answer.refusal(request.to_string()).into());
            }
        }
    }
}

/// How long a failed request waits before it is made again: GitHub asks for
/// at least a second between writes
/// (<https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api#pause-between-mutative-requests>).
const RETRY_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// How far before a write was sent [`retried_once`]'s look reaches back,
/// for the difference between this host's clock and GitHub's.
const CLOCK_SKEW: chrono::TimeDelta = chrono::TimeDelta::minutes(1);

/// Whether a retry could end `error` differently (runs.md, Ending,
/// **Retries**): GitHub gave no answer, as when the connection closed before
/// it, or answered with a server error. A refusal (4xx) would be given again.
fn retryable(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|e| !e.is_builder())
            || cause
                .downcast_ref::<ErrorResponse>()
                .is_some_and(|r| r.status.is_server_error())
    })
}

/// What `attempt`, the requests of `request`, gives, attempted once more
/// when it fails as a retry could end differently ([`retryable`]). GitHub
/// may have made what a write asked for though its answer was lost, so before
/// the second attempt `look` looks for it, and what it finds stands in for
/// that attempt.
async fn retried_once<T, A, AFut, L, LFut>(request: &str, mut attempt: A, look: L) -> Result<T>
where
    A: FnMut() -> AFut,
    AFut: Future<Output = Result<T>>,
    L: FnOnce() -> LFut,
    LFut: Future<Output = Result<Option<T>>>,
{
    let first = match attempt().await {
        Err(e) if retryable(&e) => e,
        done => return done,
    };
    tracing::warn!(request, error = format!("{first:#}"), "retrying");
    tokio::time::sleep(RETRY_WAIT).await;
    let found = look().await.with_context(|| {
        format!("{request} failed ({first:#}), and looking for what it made failed")
    })?;
    if let Some(found) = found {
        return Ok(found);
    }
    attempt()
        .await
        .with_context(|| format!("{request} failed twice, first with {first:#}, then"))
}

/// When a write about to be sent may have been made, by GitHub's clock.
fn since_sent() -> Timestamp {
    chrono::Utc::now() - CLOCK_SKEW
}

/// `time` as GitHub takes it in a query: `YYYY-MM-DDTHH:MM:SSZ`.
fn query_time(time: Timestamp) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The look of a request that makes nothing, which finds nothing.
async fn finds_nothing<T>() -> Result<Option<T>> {
    Ok(None)
}

/// The URL of the attachment GitHub gave `answer` to `request`, an upload
/// that is not rate limited. GitHub answers a token that cannot write to the
/// repository with 404, as gh says (`uploadPermissions` in
/// `internal/attachments` of <https://github.com/cli/cli>).
fn attached(request: &str, answer: Answer) -> Result<String, AttachError> {
    #[derive(Deserialize)]
    struct Uploaded {
        url: String,
    }
    if answer.status.is_success() {
        let uploaded: Uploaded =
            serde_json::from_str(&answer.body).with_context(|| format!("decoding {request}"))?;
        return Ok(uploaded.url);
    }
    let refusal = ErrorResponse {
        request: request.to_string(),
        status: answer.status,
        body: answer.body,
    };
    match refusal.status {
        StatusCode::UNPROCESSABLE_ENTITY => Err(AttachError::RefusedFile {
            reason: refusal.reasons(),
        }),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => {
            Err(AttachError::RefusedToken {
                reason: refusal.message(),
            })
        }
        _ => Err(AttachError::Failed(refusal.into())),
    }
}

/// An issue or pull request as GitHub lists it
/// (<https://docs.github.com/en/rest/issues/issues#list-repository-issues>).
#[derive(Deserialize)]
struct ListedIssue {
    number: u64,
    title: String,
    body: Option<String>,
    user: RestUser,
    created_at: Timestamp,
    /// Present on a pull request only.
    pull_request: Option<Value>,
}

/// <https://docs.github.com/en/rest/issues/comments#list-issue-comments>
#[derive(Deserialize)]
struct ListedComment {
    body: Option<String>,
    user: RestUser,
    created_at: Timestamp,
}

/// <https://docs.github.com/en/rest/pulls/pulls#list-pull-requests>
#[derive(Deserialize)]
struct ListedPull {
    number: u64,
    user: RestUser,
}

/// What GitHub answers a `POST` that made an issue or a pull request with.
#[derive(Deserialize)]
struct Made {
    number: Number,
}

/// An issue a link names, as GitHub lists sub-issues and blocking issues.
#[derive(Deserialize)]
struct ListedIssueId {
    id: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadComment {
    author: Option<Actor>,
    body: String,
    created_at: Timestamp,
}

#[derive(Deserialize)]
struct ThreadData {
    node: ThreadNode,
}

#[derive(Deserialize)]
struct ThreadNode {
    comments: Nodes<ThreadComment>,
}

/// The issue among `listed` that `linewright` opened as `issue` since
/// `since`: what a `create_issue` whose answer was lost made.
fn issue_made(
    listed: Vec<ListedIssue>,
    linewright: &Login,
    issue: &NewIssue,
    since: Timestamp,
) -> Option<Number> {
    listed
        .into_iter()
        .find(|listed| {
            listed.pull_request.is_none()
                && Login::new(listed.user.login.as_str()) == *linewright
                && listed.created_at >= since
                && listed.title == issue.title
                && listed.body.as_deref().unwrap_or_default() == issue.body
        })
        .map(|listed| Number(listed.number))
}

/// Whether `linewright` commented `body` among `listed` since `since`.
fn commented(listed: Vec<ListedComment>, linewright: &Login, body: &str, since: Timestamp) -> bool {
    listed.into_iter().any(|comment| {
        Login::new(comment.user.login.as_str()) == *linewright
            && comment.created_at >= since
            && comment.body.as_deref().unwrap_or_default() == body
    })
}

/// The pull request among `listed`, all from one head, that `linewright`
/// opened.
fn pull_made(listed: Vec<ListedPull>, linewright: &Login) -> Option<Number> {
    listed
        .into_iter()
        .find(|pull| Login::new(pull.user.login.as_str()) == *linewright)
        .map(|pull| Number(pull.number))
}

/// Whether `linewright` replied `body` in the thread of `comments` since
/// `since`.
fn replied(comments: Vec<ThreadComment>, linewright: &Login, body: &str, since: Timestamp) -> bool {
    comments.into_iter().any(|comment| {
        comment
            .author
            .is_some_and(|a| Login::new(a.login) == *linewright)
            && comment.created_at >= since
            && comment.body == body
    })
}

/// Whether `listed` holds the issue whose id is `id`.
fn linked_already(listed: &[ListedIssueId], id: u64) -> bool {
    listed.iter().any(|issue| issue.id == id)
}

fn gql_data<T>(response: GqlResponse<T>) -> Result<T> {
    if !response.errors.is_empty() {
        bail!("GraphQL errors: {}", Value::Array(response.errors));
    }
    response.data.context("GraphQL returned no data")
}

#[derive(Clone, Deserialize)]
struct Viewer {
    login: String,
}

/// The fields of a pull request that [`GqlPull`] reads, asking whether a
/// check is required of the pull request `$number`.
macro_rules! pull_fields {
    () => {
        concat!(
    "{
  number title body createdAt updatedAt baseRefName headRefName headRefOid isCrossRepository isDraft state mergedAt closedAt
  mergeable mergeStateStatus canBeRebased reviewDecision
  author { __typename login }
  ",
    closes_field!(),
    "
  labels(first: 50) { nodes { name } }
  assignees(first: 20) { nodes { login } }
  timelineItems(last: 100, itemTypes: [LABELED_EVENT, ASSIGNED_EVENT, UNASSIGNED_EVENT]) {
    nodes {
      __typename
      ... on LabeledEvent { ",
    labeling_fields!(),
    " }
      ",
    assignment_fields!(),
    "
    }
  }
  comments(last: 100) { nodes { author { __typename login } body createdAt } }
  reviews(last: 100) { nodes { author { __typename login } state body submittedAt } }
  ",
    review_requests_field!(),
    "
  reviewThreads(first: 100) {
    nodes { id path isResolved comments(first: 100) { nodes { author { __typename login } body createdAt } } }
  }
  statusCheckRollup {
    contexts(first: 100) {
      nodes {
        __typename
        ... on CheckRun { name status conclusion isRequired(pullRequestNumber: $number) }
        ... on StatusContext { context state isRequired(pullRequestNumber: $number) }
      }
    }
  }
}"
        )
    };
}

impl GitHub {
    pub async fn connect(repo: &Repo, token: String) -> Result<GitHub> {
        let http = client_builder_trusting_the_host_store_alone()
            .user_agent("linewright")
            .build()?;
        let mut github = GitHub {
            http,
            api: API.into(),
            token,
            owner: repo.owner().into(),
            name: repo.name().into(),
            viewer: Viewer {
                login: String::new(),
            },
            repository_id: Arc::default(),
            openings: Arc::default(),
            sending: Arc::default(),
            pushes_kept: Arc::default(),
            runs_listed: Arc::default(),
        };
        github.viewer = github
            .rest(Method::GET, "/user", None)
            .await
            .context("reading the token's user")?;
        Ok(github)
    }

    /// GitHub's answer to a REST request of `method` to `path` under the
    /// repository, such as `/issues`, sending `body`, if any: made once, with each rate limit
    /// waited out.
    pub async fn repo_request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        self.rest_once(method, &self.repo_path(path), body).await
    }

    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    pub fn linewright(&self) -> Login {
        Login::new(&self.viewer.login)
    }

    fn by_linewright(&self, author: Option<&Actor>) -> bool {
        author.is_some_and(|a| a.login == self.viewer.login && a.typename != "Bot")
    }

    fn repo_path(&self, rest: &str) -> String {
        format!("/repos/{}/{}{rest}", self.owner, self.name)
    }

    /// A request to `path`, under the API unless it is a full URL, sending
    /// `body`, if any.
    fn request(&self, method: Method, path: &str, body: Option<&Value>) -> reqwest::RequestBuilder {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{path}", self.api)
        };
        let request = self
            .http
            .request(method, &url)
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION);
        match body {
            Some(body) => request.json(body),
            None => request,
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<reqwest::Response> {
        self.request(method.clone(), path, body.as_ref())
            .send()
            .await
            .with_context(|| format!("{method} {path}"))
    }

    /// The answer to `request`, which `build` builds, with each rate limit
    /// waited out.
    async fn answer_to(
        &self,
        request: &str,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<Answer> {
        waiting_out_rate_limits(request, &self.sending, || async {
            let response = build().send().await.with_context(|| request.to_string())?;
            Answer::read(response).await
        })
        .await
    }

    /// The answer to the request, with each rate limit waited out.
    async fn answer(&self, method: &Method, path: &str, body: Option<&Value>) -> Result<Answer> {
        self.answer_to(&format!("{method} {path}"), || {
            self.request(method.clone(), path, body)
        })
        .await
    }

    /// The answer to `request`, which `build` builds and which makes
    /// nothing, made again once when a retry could end it differently
    /// ([`retried_once`]): when GitHub gives none or a server error, which
    /// fails it.
    async fn answer_retried_to(
        &self,
        request: &str,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<Answer> {
        retried_once(
            request,
            || async {
                let answer = self.answer_to(request, &build).await?;
                if answer.status.is_server_error() {
                    return Err(answer.refusal(request.to_string()).into());
                }
                Ok(answer)
            },
            finds_nothing,
        )
        .await
    }

    /// The answer to a request that makes nothing, as
    /// [`GitHub::answer_retried_to`] gives it.
    async fn answer_retried(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Answer> {
        self.answer_retried_to(&format!("{method} {path}"), || {
            self.request(method.clone(), path, body.as_ref())
        })
        .await
    }

    /// The request, made again once when a retry could end it differently
    /// ([`retried_once`]); not for a request that makes something.
    async fn rest<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        retried_once(
            &format!("{method} {path}"),
            || self.rest_once(method.clone(), path, body.clone()),
            finds_nothing,
        )
        .await
    }

    async fn rest_once<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let request = format!("{method} {path}");
        let answer = self.answer(&method, path, body.as_ref()).await?;
        if !answer.status.is_success() {
            return Err(answer.refusal(request).into());
        }
        let text = if answer.body.is_empty() {
            "null"
        } else {
            &answer.body
        };
        serde_json::from_str(text).with_context(|| format!("decoding {method} {path}"))
    }

    /// Adds (`POST`) or removes (`DELETE`) `people` as the issue's assignees,
    /// at most ten a request, as GitHub takes them
    /// (<https://docs.github.com/en/rest/issues/assignees>), and returns the
    /// issue's assignees after.
    async fn change_assignees(
        &self,
        method: Method,
        issue: Number,
        people: &[Login],
    ) -> Result<Vec<Login>> {
        const MAX_PER_REQUEST: usize = 10;
        #[derive(Deserialize)]
        struct Assignees {
            assignees: Vec<RestUser>,
        }
        let mut assignees = vec![];
        for batch in people.chunks(MAX_PER_REQUEST) {
            let changed: Assignees = self
                .rest(
                    method.clone(),
                    &self.repo_path(&format!("/issues/{}/assignees", issue.0)),
                    Some(json!({"assignees": batch})),
                )
                .await?;
            assignees = changed
                .assignees
                .into_iter()
                .map(|a| Login::new(a.login))
                .collect();
        }
        Ok(assignees)
    }

    /// GitHub's answer to a `GET` of `path` under the repository, such as
    /// `issues/7`, `None` when GitHub finds nothing there.
    async fn found(&self, path: &str) -> Result<Option<Value>> {
        let path = self.repo_path(&format!("/{path}"));
        let request = format!("GET {path}");
        let answer = self.answer_retried(Method::GET, &path, None).await?;
        if answer.status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !answer.status.is_success() {
            return Err(answer.refusal(request).into());
        }
        let found =
            serde_json::from_str(&answer.body).with_context(|| format!("decoding {request}"))?;
        Ok(Some(found))
    }

    /// Every page of the REST listing at `path` under the repository, such
    /// as `issues/7/comments`, joined.
    async fn listed(&self, path: &str) -> Result<Value> {
        const PER_PAGE: usize = 100;
        let listed: Vec<Value> = every_page(PER_PAGE, |page| async move {
            let path = self.repo_path(&format!("/{path}?per_page={PER_PAGE}&page={page}"));
            self.rest(Method::GET, &path, None).await
        })
        .await?;
        Ok(Value::Array(listed))
    }

    /// Issue or pull request #`number` as GitHub's REST API gives it.
    async fn record_item(&self, number: Number) -> Result<RecordedItem> {
        let path = |resource: Resource| resource.path(number);
        let issue = self
            .found(&path(Resource::Issue))
            .await?
            .with_context(|| format!("GitHub has no issue or pull request {number}"))?;
        // The issues API gives a pull request a `pull_request` field
        // (<https://docs.github.com/en/rest/issues/issues#get-an-issue>).
        let is_pull = issue.get("pull_request").is_some();
        let mut resources = BTreeMap::from([(path(Resource::Issue), issue)]);
        for listing in [Resource::Comments, Resource::BlockedBy] {
            resources.insert(path(listing), self.listed(&path(listing)).await?);
        }
        if let Some(parent) = self.found(&path(Resource::Parent)).await? {
            resources.insert(path(Resource::Parent), parent);
        }
        if is_pull {
            let pull = self
                .found(&path(Resource::Pull))
                .await?
                .with_context(|| format!("GitHub has no pull request {number}"))?;
            resources.insert(path(Resource::Pull), pull);
        }
        Ok(RecordedItem { number, resources })
    }

    async fn rest_ok(&self, method: Method, path: &str, body: Option<Value>) -> Result<()> {
        self.rest::<Value>(method, path, body).await.map(|_| ())
    }

    /// The data GitHub's GraphQL API answers `query` with, given
    /// `variables`, asked again once when a retry could end it differently;
    /// not for a mutation that makes something.
    pub async fn graphql<T: DeserializeOwned>(&self, query: &str, variables: Value) -> Result<T> {
        gql_data(self.graphql_response(query, variables).await?)
    }

    /// The answer to a mutation that makes something, made once.
    async fn graphql_once<T: DeserializeOwned>(&self, query: &str, variables: Value) -> Result<T> {
        gql_data(
            self.rest_once(
                Method::POST,
                "/graphql",
                Some(json!({"query": query, "variables": variables})),
            )
            .await?,
        )
    }

    /// `None` when GitHub resolves nothing the query names, such as a deleted
    /// issue (<https://docs.github.com/en/graphql/guides/forming-calls-with-graphql>).
    async fn graphql_found<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: Value,
    ) -> Result<Option<T>> {
        let response: GqlResponse<Value> = self.graphql_response(query, variables).await?;
        let not_found = |e: &Value| e["type"] == "NOT_FOUND";
        if !response.errors.is_empty() && response.errors.iter().all(not_found) {
            return Ok(None);
        }
        if !response.errors.is_empty() {
            bail!("GraphQL errors: {}", Value::Array(response.errors));
        }
        let data = response.data.context("GraphQL returned no data")?;
        Ok(Some(serde_json::from_value(data)?))
    }

    async fn graphql_response<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: Value,
    ) -> Result<GqlResponse<T>> {
        self.rest(
            Method::POST,
            "/graphql",
            Some(json!({"query": query, "variables": variables})),
        )
        .await
    }

    fn clone_url(&self) -> String {
        format!("https://github.com/{}/{}.git", self.owner, self.name)
    }

    /// The environment that has git authenticate to GitHub with the token,
    /// which no clone's configuration holds.
    fn git_env(&self) -> Vec<(String, String)> {
        let basic = base64::engine::general_purpose::STANDARD
            .encode(format!("x-access-token:{}", self.token));
        vec![
            ("GIT_CONFIG_COUNT".into(), "1".into()),
            (
                "GIT_CONFIG_KEY_0".into(),
                "http.https://github.com/.extraheader".into(),
            ),
            (
                "GIT_CONFIG_VALUE_0".into(),
                format!("AUTHORIZATION: basic {basic}"),
            ),
        ]
    }

    async fn issue_id(&self, number: Number) -> Result<u64> {
        #[derive(Deserialize)]
        struct Id {
            id: u64,
        }
        let issue: Id = self
            .rest(
                Method::GET,
                &self.repo_path(&format!("/issues/{}", number.0)),
                None,
            )
            .await?;
        Ok(issue.id)
    }

    /// What a `POST` of `body` to `path` makes, made at most once: when its
    /// answer is lost, `look` finds what it made ([`retried_once`]).
    async fn make<T, L, LFut>(&self, path: &str, body: Value, look: L) -> Result<T>
    where
        T: DeserializeOwned,
        L: FnOnce() -> LFut,
        LFut: Future<Output = Result<Option<T>>>,
    {
        retried_once(
            &format!("POST {path}"),
            || self.rest_once(Method::POST, path, Some(body.clone())),
            look,
        )
        .await
    }

    /// Links the issue whose id is `id` by a `POST` of `body` to
    /// `list_path`, a path under the repository that lists the issues linked
    /// so.
    async fn link(&self, list_path: &str, body: Value, id: u64) -> Result<()> {
        let path = self.repo_path(list_path);
        let IgnoredAny = self
            .make(&path, body, || async {
                let listed: Vec<ListedIssueId> = self
                    .rest(Method::GET, &format!("{path}?per_page=100"), None)
                    .await?;
                Ok(linked_already(&listed, id).then_some(IgnoredAny))
            })
            .await?;
        Ok(())
    }

    /// The repository's numeric id.
    async fn repository_id(&self) -> Result<u64> {
        #[derive(Deserialize)]
        struct Id {
            id: u64,
        }
        self.repository_id
            .get_or_try_init(|| async {
                let repo: Id = self
                    .rest_once(Method::GET, &self.repo_path(""), None)
                    .await?;
                Ok(repo.id)
            })
            .await
            .copied()
    }

    /// The upload of `bytes` as an attachment named `name`, as gh makes it,
    /// with `token` and not `Bearer` (<https://github.com/cli/cli/pull/14495>).
    fn attach_request(
        &self,
        repository_id: u64,
        name: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> reqwest::RequestBuilder {
        let query = form_urlencoded::Serializer::new(String::new())
            .append_pair("content_type", content_type)
            .append_pair("name", name)
            .append_pair("repository_id", &repository_id.to_string())
            .finish();
        self.http
            .post(format!("{ATTACHMENTS}?{query}"))
            .header("Authorization", format!("token {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("Content-Type", "application/octet-stream")
            .body(bytes)
    }

    /// The open items Linewright's own filters find
    /// (`docs/specs/sources_of_work.md`, Polling, Linewright's work): the
    /// issues with any of its labels and those assigned to it, through the
    /// repository's connections, and the issues and pull requests that
    /// involve it, through a search. All three are asked in one query
    /// ([`discovery_query`]), and each is paged to its end.
    async fn discovered(&self) -> Result<BTreeSet<Item>> {
        let mut cursors: BTreeMap<Finder, Option<String>> =
            Finder::ALL.iter().map(|&f| (f, None)).collect();
        let mut found = BTreeSet::new();
        while !cursors.is_empty() {
            let (query, variables) =
                discovery_query(&self.owner, &self.name, &self.viewer.login, &cursors);
            let data: Discovered = self.graphql(&query, variables).await?;
            let mut pages = BTreeMap::from([
                (Finder::Labeled, data.repository.labeled),
                (Finder::Assigned, data.repository.assigned),
                (Finder::Involved, data.involved),
            ]);
            for finder in cursors.keys().copied().collect::<Vec<_>>() {
                let page = pages
                    .remove(&finder)
                    .flatten()
                    .with_context(|| format!("GitHub gave no page of {finder:?} items"))?;
                found.extend(page.nodes.into_iter().map(GqlFound::item));
                match page.page_info.end_cursor {
                    Some(cursor) if page.page_info.has_next_page => {
                        cursors.insert(finder, Some(cursor));
                    }
                    _ => {
                        cursors.remove(&finder);
                    }
                }
            }
        }
        Ok(found)
    }

    /// The issues closed since `since` that Linewright may manage, and the
    /// spec changes' issues among them, from the closed issues updated since
    /// then that involve Linewright, found by a search
    /// (<https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests#search-by-when-an-issue-or-pull-request-was-created-or-last-updated>).
    /// It searches by `updated:`, since `closed:` leaves out some closed
    /// items.
    async fn closed_since(&self, since: Timestamp) -> Result<ClosedIssues> {
        #[derive(Deserialize)]
        struct Found {
            search: Page<GqlFactsIssue>,
        }
        let Fragment { name, text } = MANAGED_FACTS_FIELDS;
        let query = format!(
            "query($query: String!, $cursor: String) {{
              search(type: ISSUE, first: 25, after: $cursor, query: $query) {{
                pageInfo {{ hasNextPage endCursor }}
                nodes {{ ... on Issue {{ ...{name} }} }}
              }}
            }}
            {text}"
        );
        let search = format!(
            "repo:{} is:issue is:closed updated:>={} involves:{}",
            self.full_name(),
            query_time(since),
            self.viewer.login
        );
        let mut issues = vec![];
        let mut cursor = Value::Null;
        loop {
            let data: Found = self
                .graphql(&query, json!({"query": search, "cursor": cursor}))
                .await?;
            issues.extend(data.search.nodes);
            if !data.search.page_info.has_next_page {
                return Ok(closed_issues_among(issues, since, &self.linewright()));
            }
            cursor = json!(data.search.page_info.end_cursor);
        }
    }

    /// The items of `numbers` that GitHub has, `size` to a query that
    /// `query` makes of them, which reads each under the alias `i` and its
    /// number.
    async fn batched<T: DeserializeOwned>(
        &self,
        numbers: &BTreeSet<Number>,
        size: usize,
        query: impl Fn(&[Number]) -> String,
    ) -> Result<Vec<T>> {
        let numbers: Vec<Number> = numbers.iter().copied().collect();
        let batches = try_join_all(numbers.chunks(size).map(|batch| async {
            let response = self
                .graphql_response(
                    &query(batch),
                    json!({"owner": self.owner, "name": self.name}),
                )
                .await?;
            found_of_numbered(response)
        }))
        .await?;
        Ok(batches.into_iter().flatten().collect())
    }

    /// The issues or pull requests, as `kind` says, of `numbers` that GitHub
    /// has, each read with `fields`, fifteen to a query by aliases. Fifteen
    /// with eight connections each cost 121 requests, which rounds to 1 point
    /// (<https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api#calculating-a-rate-limit-score-before-running-the-call>).
    async fn numbered<T: DeserializeOwned>(
        &self,
        kind: ItemKind,
        numbers: &BTreeSet<Number>,
        fields: Fragment,
    ) -> Result<Vec<T>> {
        self.batched(numbers, 15, |batch| numbered_query(kind, batch, fields))
            .await
    }

    /// The closed issues of `numbers` that Linewright manages
    /// ([`ClosedItem::candidate`]), as [`Store::issue`] reads them.
    async fn closed_issues_numbered(&self, numbers: &BTreeSet<Number>) -> Result<Vec<ClosedItem>> {
        let issues: Vec<GqlClosedIssue> = self
            .numbered(ItemKind::Issue, numbers, CLOSED_ISSUE_FIELDS)
            .await?;
        Ok(managed_closed(issues, &self.linewright()))
    }

    /// Those of the pull requests `numbers` that Linewright worked and that
    /// closed without merging.
    async fn closed_pulls_numbered(&self, numbers: &BTreeSet<Number>) -> Result<Vec<ClosedItem>> {
        let pulls: Vec<GqlNumberedPull> = self
            .numbered(ItemKind::PullRequest, numbers, CLOSED_PULL_FIELDS)
            .await?;
        Ok(closed_unmerged(pulls, &self.linewright()))
    }

    /// The openings of those of `numbers` that GitHub has, read once each.
    async fn openings(&self, numbers: BTreeSet<Number>) -> Result<Vec<Opening>> {
        let unread: BTreeSet<Number> = {
            let read = self
                .openings
                .lock()
                .expect("the openings read are readable");
            numbers
                .iter()
                .filter(|number| !read.contains_key(number))
                .copied()
                .collect()
        };
        let items: Vec<GqlOpening> = self
            .numbered(ItemKind::Any, &unread, OPENING_FIELDS)
            .await?;
        let linewright = self.linewright();
        let mut read = self
            .openings
            .lock()
            .expect("the openings read are readable");
        for item in items {
            let opening = item.into_opening(&linewright);
            read.insert(opening.number, opening);
        }
        Ok(numbers
            .iter()
            .filter_map(|n| read.get(n).cloned())
            .collect())
    }

    /// The pull requests closed without merging that Linewright worked,
    /// whoever opened them, among those updated since `since`.
    async fn closed_pulls(&self, since: Timestamp) -> Result<Vec<ClosedItem>> {
        // <https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api#calculating-a-rate-limit-score-before-running-the-call>:
        // twenty pull requests with the five connections of
        // `ClosedPullFields` each cost 101 requests, which rounds to 1 point.
        let Fragment { name, text } = CLOSED_PULL_FIELDS;
        let query = format!(
            "query($owner: String!, $name: String!, $cursor: String) {{
              repository(owner: $owner, name: $name) {{
                pullRequests(first: 20, after: $cursor, states: [CLOSED],
                             orderBy: {{field: UPDATED_AT, direction: DESC}}) {{
                  pageInfo {{ hasNextPage endCursor }}
                  nodes {{ ...{name} }}
                }}
              }}
            }}
            {text}"
        );
        let mut closed = vec![];
        let mut cursor = Value::Null;
        loop {
            let data: RepoData<PullsData<GqlClosedPull>> = self
                .graphql(
                    &query,
                    json!({"owner": self.owner, "name": self.name, "cursor": cursor}),
                )
                .await?;
            let page = data.repository.pull_requests;
            let mut older = false;
            for pr in page.nodes {
                older |= pr.updated_at < since;
                closed.extend(pr.into_closed(&self.linewright()));
            }
            if older || !page.page_info.has_next_page {
                return Ok(closed);
            }
            cursor = json!(page.page_info.end_cursor);
        }
    }

    /// Those of `names` that the repository has and Linewright pushed, each
    /// with its issue: held, when `held` holds its number, or else as read.
    /// Also those of `names` the repository does not have.
    async fn ticket_branches(
        &self,
        names: &BTreeSet<TicketBranchName>,
        held: &BTreeSet<Number>,
    ) -> Result<(Vec<TicketBranch>, BTreeSet<TicketBranchName>)> {
        let NamedRefs { pushed, gone } = self.named_refs(names).await?;
        let unheld: BTreeSet<Number> = pushed
            .iter()
            .map(|(name, _)| name.issue())
            .filter(|issue| !held.contains(issue))
            .collect();
        let read: BTreeMap<Number, Issue> = self
            .numbered::<GqlIssue>(ItemKind::Issue, &unheld, ISSUE_FIELDS)
            .await?
            .into_iter()
            .map(|issue| {
                let issue = issue.into_issue(&self.linewright());
                (issue.number, issue)
            })
            .collect();
        let mut branches = vec![];
        for (name, open_pull_from_branch) in pushed {
            let issue = if held.contains(&name.issue()) {
                BranchIssue::Held
            } else {
                let Some(issue) = read.get(&name.issue()) else {
                    continue;
                };
                BranchIssue::Read(Box::new(issue.clone()))
            };
            branches.push(TicketBranch {
                name,
                issue,
                open_pull_from_branch,
            });
        }
        Ok((branches, gone))
    }

    /// The branches `names` names, read by name [`REFS_A_REQUEST`] to a
    /// request.
    async fn named_refs(&self, names: &BTreeSet<TicketBranchName>) -> Result<NamedRefs> {
        let names: Vec<&TicketBranchName> = names.iter().collect();
        let answers = try_join_all(names.chunks(REFS_A_REQUEST).map(|batch| async move {
            let (query, variables) = named_refs_query(&self.owner, &self.name, batch);
            let data: RepoData<BTreeMap<String, Option<GqlRef>>> =
                self.graphql(&query, variables).await?;
            anyhow::Ok((batch, data))
        }))
        .await?;
        let mut read = NamedRefs::default();
        for (batch, data) in answers {
            for (i, name) in batch.iter().enumerate() {
                match data.repository.get(&format!("b{i}")) {
                    Some(Some(branch)) => read.pushed.extend(branch.pushed(&self.viewer.login)),
                    Some(None) => {
                        read.gone.insert((*name).clone());
                    }
                    None => bail!("GitHub did not answer for the branch {}", name.as_str()),
                }
            }
        }
        Ok(read)
    }

    /// Each branch of the repository Linewright pushed for a ticket, with
    /// whether a pull request is open from it, found by listing every
    /// branch.
    async fn pushed_refs(&self) -> Result<Vec<(TicketBranchName, bool)>> {
        #[derive(Deserialize)]
        struct Refs {
            refs: Page<GqlRef>,
        }
        let query = format!(
            "query($owner: String!, $name: String!, $cursor: String) {{
              repository(owner: $owner, name: $name) {{
                refs(refPrefix: \"refs/heads/\", first: {REFS_A_REQUEST}, after: $cursor) {{
                  pageInfo {{ hasNextPage endCursor }}
                  nodes {{ ...{} }}
                }}
              }}
            }}
            {}",
            REF_FIELDS.name, REF_FIELDS.text
        );
        let mut pushed = vec![];
        let mut cursor = Value::Null;
        loop {
            let data: RepoData<Refs> = self
                .graphql(
                    &query,
                    json!({"owner": self.owner, "name": self.name, "cursor": cursor}),
                )
                .await?;
            let page = data.repository.refs;
            pushed.extend(
                page.nodes
                    .iter()
                    .filter_map(|branch| branch.pushed(&self.viewer.login)),
            );
            if !page.page_info.has_next_page {
                return Ok(pushed);
            }
            cursor = json!(page.page_info.end_cursor);
        }
    }

    async fn all_files(&self, pr: u64) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct File {
            filename: String,
        }
        let mut paths = vec![];
        for page in 1..=30 {
            let files: Vec<File> = self
                .rest(
                    Method::GET,
                    &self.repo_path(&format!("/pulls/{pr}/files?per_page=100&page={page}")),
                    None,
                )
                .await?;
            let done = files.len() < 100;
            paths.extend(files.into_iter().map(|f| f.filename));
            if done {
                break;
            }
        }
        Ok(paths)
    }

    /// The pull requests merged into `base` that `windows` holds.
    async fn merged_within(&self, base: &str, windows: Windows) -> Result<Vec<MergedPull>> {
        // <https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api#calculating-a-rate-limit-score-before-running-the-call>:
        // fourteen pull requests with the seven connections of
        // `MergedFields` each cost 99 requests, which rounds to 1 point.
        let query = "query($owner: String!, $name: String!, $base: String!, $cursor: String) {
              repository(owner: $owner, name: $name) {
                pullRequests(first: 14, after: $cursor, states: [MERGED], baseRefName: $base,
                             orderBy: {field: UPDATED_AT, direction: DESC}) {
                  pageInfo { hasNextPage endCursor }
                  nodes { ...MergedFields }
                }
              }
            }";
        let query = format!("{query}\n{MERGED_FIELDS}");
        let mut merged = vec![];
        let mut cursor = Value::Null;
        loop {
            let data: RepoData<PullsData<GqlMerged>> = self
                .graphql(
                    &query,
                    json!({"owner": self.owner, "name": self.name, "base": base, "cursor": cursor}),
                )
                .await?;
            let page = data.repository.pull_requests;
            let (within, older): (Vec<_>, Vec<_>) = page
                .nodes
                .into_iter()
                .partition(|pr| pr.updated_at >= windows.read_since());
            let read = try_join_all(
                within
                    .into_iter()
                    .map(|pr| self.merged_view(pr, base, windows)),
            )
            .await?;
            merged.extend(read.into_iter().flatten());
            if !older.is_empty() || !page.page_info.has_next_page {
                return Ok(merged);
            }
            cursor = json!(page.page_info.end_cursor);
        }
    }

    async fn merged_view(
        &self,
        pr: GqlMerged,
        base: &str,
        windows: Windows,
    ) -> Result<Option<MergedPull>> {
        match pr.held_merged_at(base, windows) {
            Some(merged_at) => Ok(Some(self.merged_of(pr, merged_at).await?)),
            None => Ok(None),
        }
    }

    async fn merged_of(&self, mut pr: GqlMerged, merged_at: Timestamp) -> Result<MergedPull> {
        let files = match pr.files.take() {
            Some(files) if !files.page_info.has_next_page => {
                files.nodes.into_iter().map(|f| f.path).collect()
            }
            _ => self.all_files(pr.number).await?,
        };
        Ok(pr.into_merged(merged_at, files))
    }

    /// The pushes to `branch`, at `head`, oldest first, read from its
    /// activity
    /// (<https://docs.github.com/en/rest/repos/repos#list-repository-activities>)
    /// unless they were read at `head` already. They are kept once the
    /// activity's newest entry is at `head`.
    async fn pushes(&self, branch: &str, head: &str) -> Result<Vec<Push>> {
        #[derive(Deserialize)]
        struct Activity {
            after: Option<String>,
            timestamp: Timestamp,
            #[serde(rename = "activity_type")]
            kind: String,
            actor: Option<RestUser>,
        }
        if let Some(pushes) = self.pushes_kept.at(branch, head) {
            return Ok(pushes);
        }
        let path = self.repo_path(&format!("/activity?ref=refs/heads/{branch}&per_page=30"));
        let activity: Vec<Activity> = self.rest(Method::GET, &path, None).await?;
        let at_head = activity
            .first()
            .is_some_and(|newest| newest.after.as_deref() == Some(head));
        let mut pushes: Vec<Push> = activity
            .into_iter()
            .filter(|a| matches!(a.kind.as_str(), "push" | "force_push" | "branch_creation"))
            .filter_map(|a| {
                Some(Push {
                    by: Login::new(a.actor?.login),
                    at: a.timestamp,
                })
            })
            .collect();
        pushes.reverse();
        if at_head {
            self.pushes_kept.keep(branch, head, pushes.clone());
        }
        Ok(pushes)
    }

    async fn gql_pull(&self, number: Number) -> Result<Option<GqlPull>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct One {
            pull_request: GqlPull,
        }
        let query = format!(
            "query($owner: String!, $name: String!, $number: Int!) {{
              repository(owner: $owner, name: $name) {{ pullRequest(number: $number) {{ ...PullFields }} }}
            }}
            {PULL_FIELDS}"
        );
        let data: Option<RepoData<One>> = self
            .graphql_found(
                &query,
                json!({"owner": self.owner, "name": self.name, "number": number.0}),
            )
            .await?;
        Ok(data.map(|d| d.repository.pull_request))
    }

    async fn gql_merged(&self, number: Number) -> Result<Option<GqlMerged>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct One {
            pull_request: GqlMerged,
        }
        let query = format!(
            "query($owner: String!, $name: String!, $number: Int!) {{
              repository(owner: $owner, name: $name) {{ pullRequest(number: $number) {{ ...MergedFields }} }}
            }}
            {MERGED_FIELDS}"
        );
        let data: Option<RepoData<One>> = self
            .graphql_found(
                &query,
                json!({"owner": self.owner, "name": self.name, "number": number.0}),
            )
            .await?;
        Ok(data.map(|d| d.repository.pull_request))
    }

    async fn spec_change_issues_of(&self, spec_change: Number) -> Result<Vec<Number>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct References {
            timeline_items: Nodes<CrossReference>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct One {
            pull_request: References,
        }
        let query = format!(
            "query($owner: String!, $name: String!, $number: Int!) {{
              repository(owner: $owner, name: $name) {{ pullRequest(number: $number) {{
                {CROSS_REFERENCES_FIELD}
              }} }}
            }}"
        );
        let data: RepoData<One> = self
            .graphql(
                &query,
                json!({"owner": self.owner, "name": self.name, "number": spec_change.0}),
            )
            .await?;
        let references = data.repository.pull_request.timeline_items;
        Ok(spec_change_issues_among(
            references,
            spec_change,
            &self.linewright(),
        ))
    }

    async fn gql_issue(&self, number: Number) -> Result<Option<GqlIssue>> {
        self.issue_with(number, ISSUE_FIELDS).await
    }

    /// Issue #`number`, read with `fields`.
    async fn issue_with<T: DeserializeOwned>(
        &self,
        number: Number,
        fields: Fragment,
    ) -> Result<Option<T>> {
        #[derive(Deserialize)]
        struct One<T> {
            issue: T,
        }
        let Fragment { name, text } = fields;
        let query = format!(
            "query($owner: String!, $name: String!, $number: Int!) {{
              repository(owner: $owner, name: $name) {{ issue(number: $number) {{ ...{name} }} }}
            }}
            {text}"
        );
        let data: Option<RepoData<One<T>>> = self
            .graphql_found(
                &query,
                json!({"owner": self.owner, "name": self.name, "number": number.0}),
            )
            .await?;
        Ok(data.map(|d| d.repository.issue))
    }

    /// `pr`, with its pushes and, when it is blocked, the rules of the
    /// branch it merges into, read once in `rules`.
    async fn pull_view(&self, pr: GqlPull, rules: &RulesRead) -> Result<Pull> {
        let pushes = async {
            if pr.pushes_readable() {
                self.pushes(&pr.head_ref_name, &pr.head_ref_oid).await
            } else {
                Ok(vec![])
            }
        };
        let branch_rules = async {
            if pr.merge_state_status == "BLOCKED" {
                Ok(self.branch_rules(&pr.base_ref_name, rules).await)
            } else {
                Ok(Arc::default())
            }
        };
        let (pushes, branch_rules) = tokio::try_join!(pushes, branch_rules)?;
        Ok(pr.into_pull(pushes, &branch_rules))
    }

    /// The rules GitHub applies to `branch`
    /// (<https://docs.github.com/en/rest/repos/rules#get-rules-for-a-branch>),
    /// `None` when GitHub refuses them for the repository's plan, as it does
    /// for a private repository on a free plan
    /// (<https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets>).
    /// An error on any other answer, GitHub's [`ErrorResponse`] among them.
    async fn rules(&self, branch: &str) -> Result<Option<Vec<Value>>> {
        let rules = self
            .rest(
                Method::GET,
                &self.repo_path(&format!("/rules/branches/{}", percent_encode(branch))),
                None,
            )
            .await;
        match rules {
            Ok(rules) => Ok(Some(rules)),
            Err(e) => match e.downcast_ref::<ErrorResponse>() {
                Some(refusal)
                    if refusal.status == StatusCode::FORBIDDEN
                        && refusal.message().starts_with(PLAN_REFUSAL) =>
                {
                    Ok(None)
                }
                _ => Err(e),
            },
        }
    }

    /// What the rulesets on `branch` require: nothing when GitHub does not
    /// give them. They are read unless `rules` holds them.
    async fn branch_rules(&self, branch: &str, rules: &RulesRead) -> Arc<BranchRules> {
        let cell = rules.of(branch);
        let read = cell
            .get_or_try_init(|| async {
                let given = self.rules(branch).await?;
                anyhow::Ok(Arc::new(BranchRules::from_rules(
                    &given.unwrap_or_default(),
                )))
            })
            .await;
        match read {
            Ok(read) => read.clone(),
            Err(e) => {
                tracing::warn!(
                    branch,
                    error = format!("{e:#}"),
                    "reading the branch's rules"
                );
                Arc::default()
            }
        }
    }

    async fn pull_node_id(&self, pr: Number) -> Result<String> {
        #[derive(Deserialize)]
        struct Node {
            node_id: String,
        }
        let node: Node = self
            .rest(
                Method::GET,
                &self.repo_path(&format!("/pulls/{}", pr.0)),
                None,
            )
            .await?;
        Ok(node.node_id)
    }

    /// The page of runs listed at `url`, read again only when it changed:
    /// GitHub answers 304 to the `ETag` of the page kept for `url`
    /// (<https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api#use-conditional-requests-if-appropriate>).
    async fn runs_page(&self, url: &str) -> Result<RunsPage> {
        let kept = self.runs_listed.kept(url);
        let request = format!("GET {url}");
        let answer = self
            .answer_retried_to(&request, || {
                let sent = self.request(Method::GET, url, None);
                match kept.as_ref().and_then(|kept| kept.etag.as_deref()) {
                    Some(etag) => sent.header("If-None-Match", etag),
                    None => sent,
                }
            })
            .await?;
        let page = match (answer.status, kept) {
            (StatusCode::NOT_MODIFIED, Some(kept)) => kept,
            (status, _) if status.is_success() => RunsPage {
                etag: header(&answer.headers, "etag"),
                next: header(&answer.headers, "link").and_then(|link| next_link(&link)),
                body: answer.body,
            },
            _ => return Err(answer.refusal(request).into()),
        };
        self.runs_listed.keep(url, page.clone());
        Ok(page)
    }

    /// Every run GitHub lists as created within `window`, with the URL of
    /// each page read. A range GitHub counts more than [`RUNS_LISTED_AT_MOST`]
    /// runs in is read in halves.
    async fn runs_created(
        &self,
        window: Created,
    ) -> Result<(Vec<RestWorkflowRun>, BTreeSet<String>)> {
        let now = chrono::Utc::now();
        let mut ranges = vec![window];
        let mut runs = vec![];
        let mut read = BTreeSet::new();
        while let Some(range) = ranges.pop() {
            let query: String = form_urlencoded::Serializer::new(String::new())
                .append_pair("created", &range.query())
                .append_pair("per_page", &RUNS_PER_PAGE.to_string())
                .finish();
            let mut url = Some(self.repo_path(&format!("/actions/runs?{query}")));
            let mut first = true;
            while let Some(at) = url {
                let page = self.runs_page(&at).await?;
                let listed: RunsListing = serde_json::from_str(&page.body)
                    .with_context(|| format!("decoding GET {at}"))?;
                read.insert(at);
                if first && listed.total_count > RUNS_LISTED_AT_MOST {
                    if let Some((earlier, later)) = range.halves(now) {
                        ranges.extend([earlier, later]);
                        break;
                    }
                    tracing::warn!(
                        created = range.query(),
                        runs = listed.total_count,
                        "GitHub lists only {RUNS_LISTED_AT_MOST} of the runs created in a second"
                    );
                }
                first = false;
                runs.extend(listed.workflow_runs);
                url = page.next;
            }
        }
        Ok((runs, read))
    }

    /// Attempt `id` of `run`, if it has completed: `None` too when GitHub no
    /// longer has it, as after the run is deleted.
    async fn earlier_attempt(
        &self,
        run: &RestWorkflowRun,
        id: AttemptId,
    ) -> Result<Option<CompletedAttempt>> {
        let path = format!("actions/runs/{}/attempts/{}", id.run, id.attempt);
        let Some(attempt) = self.found(&path).await? else {
            return Ok(None);
        };
        let attempt: RestWorkflowRun = serde_json::from_value(attempt)
            .with_context(|| format!("decoding GET {}", self.repo_path(&format!("/{path}"))))?;
        Ok(attempt.completed(run))
    }

    async fn failed_step_output(&self, job: u64) -> Option<String> {
        let response = self
            .send(
                Method::GET,
                &self.repo_path(&format!("/actions/jobs/{job}/logs")),
                None,
            )
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        Some(job_log::failed_step_output(&response.text().await.ok()?))
    }
}

/// How the message of GitHub's refusal of a feature for the repository's plan
/// begins, such as "Upgrade to GitHub Pro or make this repository public to
/// enable this feature.": GitHub documents no other mark of it.
const PLAN_REFUSAL: &str = "Upgrade to GitHub";

/// What the rulesets on a branch require that GitHub does not always report
/// elsewhere.
#[derive(Debug, Default, PartialEq)]
struct BranchRules {
    /// Reported or not.
    checks: Vec<String>,
    code_owner_review: bool,
}

impl BranchRules {
    fn from_rules(rules: &[Value]) -> Self {
        let of_kind = |kind: &'static str| {
            rules
                .iter()
                .filter(move |r| r["type"] == kind)
                .map(|r| &r["parameters"])
        };
        BranchRules {
            checks: of_kind("required_status_checks")
                .filter_map(|p| p["required_status_checks"].as_array())
                .flatten()
                .filter_map(|c| c["context"].as_str().map(String::from))
                .collect(),
            code_owner_review: of_kind("pull_request")
                .any(|p| p["require_code_owner_review"] == json!(true)),
        }
    }
}

/// The most runs GitHub lists for one filter
/// (<https://docs.github.com/en/rest/actions/workflow-runs#list-workflow-runs-for-a-repository>).
const RUNS_LISTED_AT_MOST: u64 = 1000;

/// How many runs a page of a listing of them holds, the most GitHub gives.
const RUNS_PER_PAGE: u64 = 100;

/// A page of runs as GitHub listed it at a URL, with its `ETag`, if GitHub
/// gave one, and the URL of the next page.
#[derive(Clone)]
struct RunsPage {
    etag: Option<String>,
    body: String,
    next: Option<String>,
}

/// The page of runs last listed at each URL a poll read.
#[derive(Default)]
struct RunsListed(std::sync::Mutex<BTreeMap<String, RunsPage>>);

impl RunsListed {
    fn listed(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, RunsPage>> {
        self.0.lock().expect("the runs listed are readable")
    }

    fn kept(&self, url: &str) -> Option<RunsPage> {
        self.listed().get(url).cloned()
    }

    fn keep(&self, url: &str, page: RunsPage) {
        self.listed().insert(url.to_string(), page);
    }

    /// Forgets every page but those listed at `urls`.
    fn keep_only(&self, urls: &BTreeSet<String>) {
        self.listed().retain(|url, _| urls.contains(url));
    }
}

/// A range of the times GitHub created runs at, as its `created` filter
/// takes it
/// (<https://docs.github.com/en/search-github/getting-started-with-searching-on-github/understanding-the-search-syntax#query-for-dates>).
#[derive(Clone, Copy, Debug)]
struct Created {
    /// The first time in it, a whole second.
    from: Timestamp,
    /// The last time in it, inclusive and a whole second; none for every
    /// time from `from` on.
    to: Option<Timestamp>,
}

impl Created {
    fn query(self) -> String {
        match self.to {
            None => format!(">={}", query_time(self.from)),
            Some(to) => format!("{}..{}", query_time(self.from), query_time(to)),
        }
    }

    /// The range split in two at a time after `from`, taking `now` as the
    /// end of a range with none: at the hour of its middle when that is
    /// after `from`, and else at its middle. None for a range of one second.
    fn halves(self, now: Timestamp) -> Option<(Created, Created)> {
        use chrono::DurationRound as _;
        let second = chrono::TimeDelta::seconds(1);
        let end = self.to.unwrap_or(now);
        if end - self.from < second {
            return None;
        }
        let middle = (self.from + (end - self.from) / 2)
            .duration_trunc(second)
            .ok()?;
        let middle = Some(hour_of(middle))
            .filter(|hour| *hour > self.from)
            .unwrap_or(middle)
            .max(self.from + second);
        Some((
            Created {
                from: self.from,
                to: Some(middle - second),
            },
            Created {
                from: middle,
                to: self.to,
            },
        ))
    }
}

/// <https://docs.github.com/en/rest/actions/workflow-runs#list-workflow-runs-for-a-repository>
#[derive(Deserialize)]
struct RunsListing {
    total_count: u64,
    workflow_runs: Vec<RestWorkflowRun>,
}

/// A run at its latest attempt, or one of its attempts
/// (<https://docs.github.com/en/rest/actions/workflow-runs#get-a-workflow-run-attempt>).
#[derive(Deserialize)]
struct RestWorkflowRun {
    id: WorkflowRunId,
    name: Option<String>,
    run_attempt: std::num::NonZeroU32,
    status: Option<RunStatus>,
    conclusion: Option<AttemptConclusion>,
    /// When GitHub created the run, for the run; when it created the
    /// attempt, for an attempt.
    created_at: Timestamp,
    updated_at: Timestamp,
    head_branch: Option<String>,
    head_sha: String,
    /// The repository the run is of.
    repository: RestRepository,
    /// The repository of the commit it ran on: another for a fork's.
    head_repository: Option<RestRepository>,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum RunStatus {
    Completed,
    #[serde(other)]
    NotCompleted,
}

#[derive(Deserialize, PartialEq)]
struct RestRepository {
    full_name: String,
}

impl RestWorkflowRun {
    fn id(&self) -> AttemptId {
        AttemptId {
            run: self.id,
            attempt: self.run_attempt,
        }
    }

    /// The ids of the attempts before this one, leaving out those `known`.
    fn earlier(&self, known: &BTreeSet<AttemptId>) -> Vec<AttemptId> {
        (1..self.run_attempt.get())
            .filter_map(std::num::NonZeroU32::new)
            .map(|attempt| AttemptId {
                run: self.id,
                attempt,
            })
            .filter(|id| !known.contains(id))
            .collect()
    }

    /// This attempt of `run`, as listed, if it has completed.
    fn completed(&self, run: &RestWorkflowRun) -> Option<CompletedAttempt> {
        if self.status != Some(RunStatus::Completed) {
            return None;
        }
        let id = self.id();
        let ours = self.head_repository.as_ref() == Some(&self.repository);
        Some(CompletedAttempt {
            id,
            workflow: run.name.clone().unwrap_or_default(),
            run_created_at: run.created_at,
            completed_at: self.updated_at,
            conclusion: self.conclusion.clone()?,
            branch: self.head_branch.clone().filter(|_| ours),
            commit: self.head_sha.clone(),
            link: format!(
                "https://github.com/{}/actions/runs/{}/attempts/{}",
                self.repository.full_name, id.run, id.attempt
            ),
        })
    }
}

/// The latest attempt of each of `runs`, which may list a run twice as a
/// new run shifts the pages read.
fn latest_attempts(runs: Vec<RestWorkflowRun>) -> Vec<RestWorkflowRun> {
    let mut latest: BTreeMap<WorkflowRunId, RestWorkflowRun> = BTreeMap::new();
    for run in runs {
        if latest
            .get(&run.id)
            .is_none_or(|kept| kept.run_attempt < run.run_attempt)
        {
            latest.insert(run.id, run);
        }
    }
    latest.into_values().collect()
}

/// The value of the header `name`, if it is text.
fn header(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    Some(headers.get(name)?.to_str().ok()?.to_string())
}

#[derive(Deserialize)]
struct RestUser {
    login: String,
}

#[derive(Deserialize)]
struct GqlResponse<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<Value>,
}

/// An object as `files_query` reads it: an object that is not a blob, such
/// as a directory, has none of its fields.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlObject {
    text: Option<String>,
    #[serde(default)]
    is_binary: Option<bool>,
    #[serde(default)]
    is_truncated: Option<bool>,
}

impl GqlObject {
    /// The file this object is, or `None` when it is not a blob.
    fn blob(self) -> Option<Blob> {
        match self {
            GqlObject {
                is_truncated: Some(true),
                ..
            } => Some(Blob::TooLarge),
            GqlObject {
                is_binary: Some(true),
                ..
            } => Some(Blob::Binary),
            GqlObject {
                text: Some(text), ..
            } => Some(Blob::Text(text)),
            GqlObject {
                is_binary: Some(false),
                ..
            } => Some(Blob::TooLarge),
            GqlObject { .. } => None,
        }
    }
}

const BLOB_FIELDS: &str = "... on Blob { text isBinary isTruncated }";

/// The query that reads the files of `paths` at the head of `branch` at
/// once, the one at `paths[i]` under the alias `fi`, and its variables.
fn files_query(owner: &str, name: &str, branch: &str, paths: &[String]) -> (String, Value) {
    let aliases: Vec<String> = (0..paths.len()).map(|i| format!("f{i}")).collect();
    let declared: String = aliases
        .iter()
        .map(|alias| format!(", ${alias}: String!"))
        .collect();
    let objects: String = aliases
        .iter()
        .map(|alias| format!("\n    {alias}: object(expression: ${alias}) {{ {BLOB_FIELDS} }}"))
        .collect();
    let query = format!(
        "query($owner: String!, $name: String!{declared}) {{\n  repository(owner: $owner, name: $name) {{{objects}\n  }}\n}}"
    );
    let mut variables = json!({"owner": owner, "name": name});
    for (alias, path) in aliases.iter().zip(paths) {
        variables[alias] = json!(format!("{branch}:{path}"));
    }
    (query, variables)
}

/// The files `files_query` read for `paths`, by path.
fn files_read(
    mut objects: BTreeMap<String, Option<GqlObject>>,
    paths: &[String],
) -> BTreeMap<String, Blob> {
    paths
        .iter()
        .enumerate()
        .filter_map(|(i, path)| {
            let blob = objects.remove(&format!("f{i}")).flatten()?.blob()?;
            Some((path.clone(), blob))
        })
        .collect()
}

#[derive(Deserialize)]
struct RepoData<T> {
    repository: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    page_info: PageInfo,
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullsData<T> {
    pull_requests: Page<T>,
}

/// What tells among which pull requests a snapshot holds an open one.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPullSigns {
    author: Option<Actor>,
    assignees: Nodes<LoginNode>,
    timeline_items: Nodes<GqlEvent>,
}

impl GqlPullSigns {
    /// Among which pull requests the snapshot holds it ([`PullSigns::held`]).
    fn held(self, linewright: &Login) -> Option<PullHeld> {
        PullSigns {
            author: &login(self.author),
            assignees: &logins(self.assignees),
            events: &events(self.timeline_items.nodes),
        }
        .held(linewright)
    }
}

/// What tells whether to read a pull request in full among the people's
/// ([`Store::peoples_pull`]): its [`GqlPullSigns`], with its state and the
/// issues it closes. Its three connections cost 1 point.
const PEOPLES_PULL_FIELDS: &str = concat!(
    "fragment PeoplesPullFields on PullRequest {
  number state author { __typename login }
  assignees(first: 20) { nodes { login } }
  ",
    closes_field!(),
    "
  ",
    work_timeline!(),
    "
}"
);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPeoplesPull {
    state: GqlPullState,
    closing_issues_references: Option<Nodes<NumberNode>>,
    #[serde(flatten)]
    signs: GqlPullSigns,
}

impl GqlPeoplesPull {
    /// The issues it closes while it is open, and whether to read it in
    /// full: while it is open, among the people's pull requests.
    fn read(self, linewright: &Login) -> (Vec<Number>, bool) {
        if self.state != GqlPullState::Open {
            return (vec![], false);
        }
        let peoples = self.signs.held(linewright) == Some(PullHeld::Peoples);
        (closes(self.closing_issues_references), peoples)
    }
}

#[derive(Clone, Deserialize)]
struct Actor {
    #[serde(rename = "__typename")]
    typename: String,
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlRef {
    name: String,
    target: Option<GqlRefTarget>,
    associated_pull_requests: Count,
}

#[derive(Deserialize)]
struct GqlRefTarget {
    committer: Option<GqlCommitter>,
}

#[derive(Deserialize)]
struct GqlCommitter {
    user: Option<LoginNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Count {
    total_count: u64,
}

impl GqlRef {
    /// The ticket it is the branch of, with whether a pull request is open
    /// from it, when `linewright` pushed it: it is named for a ticket and
    /// `linewright` committed its head.
    fn pushed(&self, linewright: &str) -> Option<(TicketBranchName, bool)> {
        let name = TicketBranchName::parse(&self.name)?;
        let committer = self.target.as_ref()?.committer.as_ref()?.user.as_ref()?;
        let open_pull = self.associated_pull_requests.total_count > 0;
        (committer.login == linewright).then_some((name, open_pull))
    }
}

/// The path, under the repository, of every entry under `dir` at the head of
/// `branch` (<https://docs.github.com/en/rest/git/trees#get-a-tree>), the
/// tree named in git's `<rev>:<path>` form.
fn tree_path(branch: &str, dir: &str) -> String {
    format!(
        "/git/trees/{}?recursive=1",
        percent_encode(&format!("{branch}:{dir}"))
    )
}

#[derive(Deserialize)]
struct GitTree {
    tree: Vec<GitTreeEntry>,
    truncated: bool,
}

#[derive(Deserialize)]
struct GitTreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: TreeEntryKind,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum TreeEntryKind {
    Blob,
    /// A tree or a submodule's commit.
    #[serde(other)]
    Other,
}

/// The path of each file in `tree`. A truncated tree is an error, not a
/// partial list.
fn files_listed(tree: GitTree) -> Result<Vec<String>> {
    if tree.truncated {
        bail!("the tree is too large for GitHub to list whole");
    }
    Ok(tree
        .tree
        .into_iter()
        .filter(|e| e.kind == TreeEntryKind::Blob)
        .map(|e| e.path)
        .collect())
}

/// A branch GitHub does not have is deleted already; GitHub answers 422 for
/// it, as it does for a branch it refuses to delete
/// (<https://docs.github.com/en/rest/git/refs#delete-a-reference>).
fn branch_deleted(branch: &str, status: StatusCode, body: &str) -> Result<()> {
    #[derive(Deserialize)]
    struct Error {
        message: String,
    }
    let missing = || {
        serde_json::from_str::<Error>(body).is_ok_and(|e| e.message == "Reference does not exist")
    };
    if status.is_success() || (status == StatusCode::UNPROCESSABLE_ENTITY && missing()) {
        Ok(())
    } else {
        bail!("deleting branch {branch}: {status}: {body}")
    }
}

/// `text` as one segment of a URL's path or a query value.
fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The path, under the repository, of the label `name`
/// (<https://docs.github.com/en/rest/issues/labels>).
fn label_path(name: &str) -> String {
    format!("/labels/{}", percent_encode(name))
}

/// The method, path under the repository and body of the request that sets
/// the description of the label `name`
/// (<https://docs.github.com/en/rest/issues/labels#update-a-label>).
fn describe_label_request(name: &str, description: &str) -> (Method, String, Value) {
    (
        Method::PATCH,
        label_path(name),
        json!({"description": description}),
    )
}

/// A login as REST spells it: GraphQL leaves the `[bot]` suffix off bots.
fn login(actor: Option<Actor>) -> Login {
    match actor {
        Some(actor) if actor.typename == "Bot" => Login::new(format!("{}[bot]", actor.login)),
        Some(actor) => Login::new(actor.login),
        None => Login::new("ghost"),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlComment {
    author: Option<Actor>,
    body: String,
    created_at: Timestamp,
}

impl From<GqlComment> for Comment {
    fn from(c: GqlComment) -> Comment {
        Comment {
            author: login(c.author),
            body: c.body,
            created_at: c.created_at,
        }
    }
}

#[derive(Deserialize)]
struct Name {
    name: String,
}

/// A label as GitHub's REST API returns it
/// (<https://docs.github.com/en/rest/issues/labels>).
#[derive(Clone, Deserialize)]
struct RestLabel {
    name: String,
    description: Option<String>,
}

impl From<RestLabel> for label::RepoLabel {
    fn from(label: RestLabel) -> label::RepoLabel {
        label::RepoLabel {
            name: label.name,
            description: label.description,
        }
    }
}

/// Every item of a REST listing GitHub pages by `per_page` and `page`
/// (<https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api>),
/// fetching `page(n)` from the first page until one comes back short.
async fn every_page<T, F, Fut>(per_page: usize, mut page: F) -> Result<Vec<T>>
where
    F: FnMut(usize) -> Fut,
    Fut: Future<Output = Result<Vec<T>>>,
{
    let mut items = vec![];
    for n in 1.. {
        let got = page(n).await?;
        let done = got.len() < per_page;
        items.extend(got);
        if done {
            break;
        }
    }
    Ok(items)
}

#[derive(Deserialize)]
struct LoginNode {
    login: String,
}

#[derive(Deserialize)]
struct LinkedNode {
    number: u64,
    state: String,
}

fn linked(nodes: Option<Nodes<LinkedNode>>) -> Vec<Linked> {
    nodes
        .map(|n| n.nodes)
        .unwrap_or_default()
        .into_iter()
        .map(|node| linked_of(&node))
        .collect()
}

fn linked_of(node: &LinkedNode) -> Linked {
    Linked {
        number: Number(node.number),
        open: node.state == "OPEN",
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubIssueNode {
    #[serde(flatten)]
    linked: LinkedNode,
    author: Option<Actor>,
    closed_at: Option<Timestamp>,
}

/// The sub-issues of `nodes`, each with who last added it among `adds`, the
/// additions their parent's timeline holds: none where it holds none.
fn sub_issues(
    nodes: Option<Nodes<SubIssueNode>>,
    adds: Option<Nodes<GqlSubIssueAdded>>,
) -> Vec<SubIssue> {
    let nodes = nodes.map(|n| n.nodes).unwrap_or_default();
    let adds = adds.map(|a| a.nodes).unwrap_or_default();
    nodes
        .into_iter()
        .map(|n| SubIssue {
            number: Number(n.linked.number),
            author: login(n.author),
            added_by: added_by(&adds, n.linked.number),
            state: if linked_of(&n.linked).open {
                SubIssueState::Open
            } else {
                SubIssueState::Closed { at: n.closed_at }
            },
        })
        .collect()
}

/// Who last added sub-issue #`number` among `adds`, oldest first.
fn added_by(adds: &[GqlSubIssueAdded], number: u64) -> Option<Login> {
    adds.iter()
        .rev()
        .find(|add| add.sub_issue.as_ref().is_some_and(|s| s.number == number))
        .map(|add| login(add.actor.clone()))
}

#[derive(Deserialize)]
struct LinkedIssueNode {
    number: u64,
    title: String,
    body: String,
    author: Option<Actor>,
}

impl LinkedIssueNode {
    fn number_and_filed(self, linewright: &Login) -> (Number, Option<Filed>) {
        let filed = Filed::read(&login(self.author), &self.title, &self.body, linewright);
        (Number(self.number), filed)
    }

    fn into_parent(self, linewright: &Login) -> Parent {
        let (number, filed) = self.number_and_filed(linewright);
        Parent::new(number, filed)
    }
}

const CROSS_REFERENCES_FIELD: &str =
    "timelineItems(last: 100, itemTypes: [CROSS_REFERENCED_EVENT]) {
    nodes {
      ... on CrossReferencedEvent {
        isCrossRepository
        source { __typename ... on Issue { number title body author { __typename login } } }
      }
    }
  }";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrossReference {
    is_cross_repository: bool,
    source: ReferenceSource,
}

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum ReferenceSource {
    Issue(LinkedIssueNode),
    PullRequest,
}

fn spec_change_issues_among(
    references: Nodes<CrossReference>,
    spec_change: Number,
    linewright: &Login,
) -> Vec<Number> {
    references
        .nodes
        .into_iter()
        .filter(|reference| !reference.is_cross_repository)
        .filter_map(|reference| match reference.source {
            ReferenceSource::Issue(issue) => Some(issue.number_and_filed(linewright)),
            ReferenceSource::PullRequest => None,
        })
        .filter(|(_, filed)| filed.and_then(Filed::spec_change) == Some(spec_change))
        .map(|(number, _)| number)
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlLabeling {
    created_at: Timestamp,
    actor: Option<Actor>,
    label: Name,
}

/// An issue's or pull request's labelings, assignments and reopenings, and
/// with `unlabelings` its unlabelings too, or with `comments` its comments
/// too, read as [`GqlEvent`]s.
macro_rules! assignment_timeline {
    (comments) => {
        assignment_timeline!(@ ", ISSUE_COMMENT",
            "
      ",
            comment_fields!(),
        )
    };
    (unlabelings) => {
        assignment_timeline!(@ ", UNLABELED_EVENT",
            "
      ... on UnlabeledEvent { ",
            labeling_fields!(),
            " }",
        )
    };
    (@ $types:literal, $($fragments:tt)*) => {
        concat!(
            "timelineItems(last: 100, itemTypes: [LABELED_EVENT, ASSIGNED_EVENT, UNASSIGNED_EVENT, REOPENED_EVENT",
            $types,
            "]) {
    nodes {
      __typename
      ... on LabeledEvent { ",
            labeling_fields!(),
            " }",
            $($fragments)*
            "
      ... on ReopenedEvent { createdAt }
      ",
            assignment_fields!(),
            "
    }
  }"
        )
    };
}

/// A GraphQL fragment: its name, and the text that defines it.
#[derive(Clone, Copy)]
struct Fragment {
    name: &'static str,
    text: &'static str,
}

// `orderByState` lists the open pull requests that close an issue before the
// closed and merged ones:
// <https://docs.github.com/en/graphql/reference/objects#issue>
const ISSUE_FIELDS: Fragment = Fragment {
    name: "IssueFields",
    text: concat!(
        "fragment IssueFields on Issue {
  number title body createdAt updatedAt state
  author { __typename login }
  labels(first: 50) { nodes { name } }
  assignees(first: 20) { nodes { login } }
  comments(last: 100) { nodes { author { __typename login } body createdAt } }
  subIssues(first: 100) { nodes { number state closedAt author { __typename login } } }
  parent { number title body author { __typename login } }
  blockedBy(first: 50) { nodes { number state } }
  closedByPullRequestsReferences(first: 20, includeClosedPrs: true, orderByState: true) { nodes { number state isDraft mergedAt closedAt updatedAt baseRefName author { __typename login } } }
  ",
    assignment_timeline!(unlabelings),
    "
  ",
    sub_issue_adds!(),
    "
}"
    ),
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlIssue {
    number: u64,
    title: String,
    body: String,
    created_at: Timestamp,
    updated_at: Timestamp,
    state: IssueState,
    author: Option<Actor>,
    labels: Nodes<Name>,
    assignees: Nodes<LoginNode>,
    comments: Nodes<GqlComment>,
    sub_issues: Option<Nodes<SubIssueNode>>,
    parent: Option<LinkedIssueNode>,
    blocked_by: Option<Nodes<LinkedNode>>,
    closed_by_pull_requests_references: Option<Nodes<GqlClosingPull>>,
    timeline_items: Nodes<GqlEvent>,
    /// `None` in a payload recorded before Linewright read it.
    #[serde(default)]
    sub_issue_adds: Option<Nodes<GqlSubIssueAdded>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlClosingPull {
    number: u64,
    state: String,
    #[serde(default)]
    is_draft: bool,
    merged_at: Option<Timestamp>,
    closed_at: Option<Timestamp>,
    updated_at: Timestamp,
    /// Empty, as not known, in a payload recorded before Linewright read it.
    #[serde(default)]
    base_ref_name: String,
    author: Option<Actor>,
}

/// What a closed issue carries, with the closer of its
/// latest closing: the pull request whose merge closed it, a commit, or none
/// when a person closed it
/// (<https://docs.github.com/en/graphql/reference/objects#closedevent>).
/// With its assignees, labelings, assignments, comments and the pull
/// requests closing it, which tell whether Linewright worked it, its
/// reopenings, which with those pull requests' merges tell when it entered
/// Done, and who added its sub-issues. Fifteen issues with the seven
/// connections each cost 106 requests, which rounds to 1 point
/// (<https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api#calculating-a-rate-limit-score-before-running-the-call>).
const CLOSED_ISSUE_FIELDS: Fragment = Fragment {
    name: "ClosedIssueFields",
    text: concat!(
        "fragment ClosedIssueFields on Issue {
  number title body closedAt updatedAt stateReason
  author { __typename login }
  labels(first: 50) { nodes { name } }
  assignees(first: 20) { nodes { login } }
  subIssues(first: 100) { nodes { number state closedAt author { __typename login } } }
  parent { number title body author { __typename login } }
  closedByPullRequestsReferences(first: 20, includeClosedPrs: true) { nodes { number state isDraft mergedAt closedAt updatedAt baseRefName author { __typename login } } }
  timelineItems(last: 1, itemTypes: [CLOSED_EVENT]) { nodes { ... on ClosedEvent { closer { __typename ... on PullRequest { number } } } } }
  assignments: timelineItems(last: 100, itemTypes: [LABELED_EVENT, ASSIGNED_EVENT, UNASSIGNED_EVENT, REOPENED_EVENT, ISSUE_COMMENT]) {
    nodes {
      __typename
      ... on LabeledEvent { ",
        labeling_fields!(),
        " }
      ... on ReopenedEvent { createdAt }
      ",
        assignment_fields!(),
        "
      ",
        comment_fields!(),
        "
    }
  }
  ",
        sub_issue_adds!(),
        "
}"
    ),
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlClosedIssue {
    number: u64,
    title: String,
    body: String,
    closed_at: Option<Timestamp>,
    updated_at: Timestamp,
    state_reason: Option<GqlStateReason>,
    author: Option<Actor>,
    labels: Nodes<Name>,
    assignees: Nodes<LoginNode>,
    sub_issues: Option<Nodes<SubIssueNode>>,
    parent: Option<LinkedIssueNode>,
    closed_by_pull_requests_references: Option<Nodes<GqlClosingPull>>,
    timeline_items: Nodes<GqlClosedEvent>,
    assignments: Nodes<GqlEvent>,
    /// As [`GqlIssue::sub_issue_adds`].
    #[serde(default)]
    sub_issue_adds: Option<Nodes<GqlSubIssueAdded>>,
}

#[derive(Deserialize)]
struct GqlClosedEvent {
    closer: Option<GqlCloser>,
}

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum GqlCloser {
    PullRequest {
        number: u64,
    },
    #[serde(other)]
    Other,
}

impl GqlClosedIssue {
    /// `None` when GitHub gives no time it closed.
    fn into_closed(self, linewright: &Login) -> Option<ClosedItem> {
        let closed_at = self.closed_at?;
        let author = login(self.author);
        let merged_by = self
            .timeline_items
            .nodes
            .into_iter()
            .last()
            .and_then(|event| match event.closer? {
                GqlCloser::PullRequest { number } => Some(Number(number)),
                GqlCloser::Other => None,
            });
        Some(ClosedItem {
            number: Number(self.number),
            closed_at,
            updated_at: self.updated_at,
            kind: ClosedKind::Issue {
                parent: self.parent.map(|p| p.into_parent(linewright)),
                facts: ManagedFacts {
                    filed: Filed::read(&author, &self.title, &self.body, linewright),
                    sub_issues: sub_issues(self.sub_issues, self.sub_issue_adds),
                    labels: self.labels.nodes.into_iter().map(|l| l.name).collect(),
                    work: WorkFacts {
                        author: Some(author),
                        assignees: logins(self.assignees),
                        events: events(self.assignments.nodes),
                        closed_by: closing_pulls(self.closed_by_pull_requests_references),
                    },
                },
                merged_by,
                reason: self.state_reason.and_then(GqlStateReason::reason),
            },
            title: self.title,
        })
    }
}

/// Which a number names, an issue or a pull request.
#[derive(Clone, Copy)]
enum ItemKind {
    Issue,
    PullRequest,
    /// Either.
    Any,
}

/// A query for the issues or pull requests `numbers`, as `kind` says, each
/// read with `fields` under the alias `i` and its number
/// (<https://docs.github.com/en/graphql/guides/using-the-explorer#aliases>).
fn numbered_query(kind: ItemKind, numbers: &[Number], fields: Fragment) -> String {
    let Fragment { name, text } = fields;
    let item = match kind {
        ItemKind::Issue => "issue",
        ItemKind::PullRequest => "pullRequest",
        ItemKind::Any => "issueOrPullRequest",
    };
    let items: String = numbers
        .iter()
        .map(|Number(n)| format!("i{n}: {item}(number: {n}) {{ ...{name} }} "))
        .collect();
    format!(
        "query($owner: String!, $name: String!) {{
          repository(owner: $owner, name: $name) {{ {items}}}
        }}
        {text}"
    )
}

/// The branches GitHub reads in one request, by name or a page of a
/// listing: a page holds at most 100
/// (<https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api#node-limit>).
const REFS_A_REQUEST: usize = 100;

/// What tells whether Linewright pushed a branch for a ticket
/// ([`GqlRef::pushed`]).
const REF_FIELDS: Fragment = Fragment {
    name: "RefFields",
    text: "fragment RefFields on Ref {
  name
  target { ... on Commit { committer { user { login } } } }
  associatedPullRequests(states: [OPEN], first: 1) { totalCount }
}",
};

/// The branches of `names` read by name ([`GitHub::named_refs`]).
#[derive(Default)]
struct NamedRefs {
    /// Those the repository has and Linewright pushed, with whether a pull
    /// request is open from each.
    pushed: Vec<(TicketBranchName, bool)>,
    /// Those the repository does not have.
    gone: BTreeSet<TicketBranchName>,
}

/// The query, with its variables, reading `branches` of the repository
/// `owner`/`name` by name: the branch `branches[i]` under the alias `bi`
/// and in the variable `$bi`
/// (<https://docs.github.com/en/graphql/reference/objects#repository>, `ref`).
/// A branch the repository does not have reads as null.
fn named_refs_query(owner: &str, name: &str, branches: &[&TicketBranchName]) -> (String, Value) {
    let mut parameters = String::new();
    let mut refs = String::new();
    let mut variables = serde_json::Map::new();
    variables.insert("owner".into(), json!(owner));
    variables.insert("name".into(), json!(name));
    for (i, branch) in branches.iter().enumerate() {
        parameters.push_str(&format!(", $b{i}: String!"));
        refs.push_str(&format!(
            "b{i}: ref(qualifiedName: $b{i}) {{ ...{} }} ",
            REF_FIELDS.name
        ));
        variables.insert(
            format!("b{i}"),
            json!(format!("refs/heads/{}", branch.as_str())),
        );
    }
    let query = format!(
        "query($owner: String!, $name: String!{parameters}) {{
          repository(owner: $owner, name: $name) {{ {refs}}}
        }}
        {}",
        REF_FIELDS.text
    );
    (query, Value::Object(variables))
}

/// The items `response` to a [`numbered_query`] holds. GitHub resolves a
/// deleted one to null, with an error saying so.
fn found_of_numbered<T>(
    response: GqlResponse<RepoData<BTreeMap<String, Option<T>>>>,
) -> Result<Vec<T>> {
    if !response.errors.iter().all(|e| e["type"] == "NOT_FOUND") {
        bail!("GraphQL errors: {}", Value::Array(response.errors));
    }
    let items = response.data.context("GraphQL returned no data")?;
    Ok(items.repository.into_values().flatten().collect())
}

/// Those of `issues` that `linewright` may manage
/// ([`ClosedItem::candidate`]).
fn managed_closed(issues: Vec<GqlClosedIssue>, linewright: &Login) -> Vec<ClosedItem> {
    issues
        .into_iter()
        .filter_map(|issue| issue.into_closed(linewright))
        .filter(|item| item.candidate(linewright))
        .collect()
}

/// What tells who opened an issue or pull request, and what for
/// ([`Opening`]). Fifteen with the one connection each cost 16 requests,
/// which comes to 1 point
/// (<https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api#calculating-a-rate-limit-score-before-running-the-call>).
const OPENING_FIELDS: Fragment = Fragment {
    name: "OpeningFields",
    text: concat!(
        "fragment OpeningFields on IssueOrPullRequest {
  __typename
  ... on Issue { number title body author { __typename login } parent { number } }
  ... on PullRequest { number author { __typename login } ",
        closes_field!(),
        " }
}"
    ),
};

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum GqlOpening {
    Issue {
        number: u64,
        title: String,
        body: String,
        author: Option<Actor>,
        parent: Option<NumberNode>,
    },
    #[serde(rename_all = "camelCase")]
    PullRequest {
        number: u64,
        author: Option<Actor>,
        closing_issues_references: Option<Nodes<NumberNode>>,
    },
}

impl GqlOpening {
    fn into_opening(self, linewright: &Login) -> Opening {
        let opening = |number: u64, author: Login, opened: Opened<'_>| {
            Opening::new(Number(number), author, opened, linewright)
        };
        match self {
            GqlOpening::Issue {
                number,
                title,
                body,
                author,
                parent,
            } => {
                let author = login(author);
                let filed = Filed::read(&author, &title, &body, linewright);
                let parent = parent.map(|parent| Parent::new(Number(parent.number), None));
                opening(number, author, Opened::Issue { parent, filed })
            }
            GqlOpening::PullRequest {
                number,
                author,
                closing_issues_references,
            } => {
                let closes = closes(closing_issues_references);
                opening(number, login(author), Opened::Pull { closes: &closes })
            }
        }
    }
}

/// A closed pull request, with what tells whether Linewright worked it and
/// who is assigned to it, requested to review it and reviewed it. Twenty
/// with the five connections each cost 101 requests, which rounds to 1
/// point
/// (<https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api#calculating-a-rate-limit-score-before-running-the-call>).
const CLOSED_PULL_FIELDS: Fragment = Fragment {
    name: "ClosedPullFields",
    text: concat!(
        "fragment ClosedPullFields on PullRequest {
  number title state closedAt updatedAt
  author { __typename login }
  assignees(first: 20) { nodes { login } }
  ",
        closes_field!(),
        "
  ",
        reviewing_fields!(),
        "
  ",
        work_timeline!(),
        "
}"
    ),
};

#[derive(Deserialize)]
struct GqlNumberedPull {
    state: GqlPullState,
    #[serde(flatten)]
    pull: GqlClosedPull,
}

/// <https://docs.github.com/en/graphql/reference/enums#pullrequeststate>
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GqlPullState {
    Open,
    Closed,
    Merged,
}

/// Those of `pulls` that `linewright` worked and that closed without
/// merging.
fn closed_unmerged(pulls: Vec<GqlNumberedPull>, linewright: &Login) -> Vec<ClosedItem> {
    pulls
        .into_iter()
        .filter(|numbered| numbered.state == GqlPullState::Closed)
        .filter_map(|numbered| numbered.pull.into_closed(linewright))
        .collect()
}

/// What tells whether Linewright may manage an issue ([`ManagedFacts`]),
/// its comments among them, and whether and when it closed. Twenty-five
/// issues with the five connections each cost 126 requests, which rounds to
/// 1 point
/// (<https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api#calculating-a-rate-limit-score-before-running-the-call>).
const MANAGED_FACTS_FIELDS: Fragment = Fragment {
    name: "ManagedFactsFields",
    text: concat!(
        "fragment ManagedFactsFields on Issue {
  number title body state closedAt updatedAt
  author { __typename login }
  labels(first: 50) { nodes { name } }
  assignees(first: 20) { nodes { login } }
  subIssues(first: 100) { nodes { number state closedAt author { __typename login } } }
  closedByPullRequestsReferences(first: 20, includeClosedPrs: true) { nodes { number state isDraft mergedAt closedAt updatedAt baseRefName author { __typename login } } }
  timelineItems(last: 100, itemTypes: [LABELED_EVENT, ASSIGNED_EVENT, UNASSIGNED_EVENT, ISSUE_COMMENT]) {
    nodes {
      __typename
      ... on LabeledEvent { ",
        labeling_fields!(),
        " }
      ",
        assignment_fields!(),
        "
      ",
        comment_fields!(),
        "
    }
  }
}"
    ),
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlFactsIssue {
    number: u64,
    title: String,
    body: String,
    state: IssueState,
    closed_at: Option<Timestamp>,
    author: Option<Actor>,
    labels: Nodes<Name>,
    assignees: Nodes<LoginNode>,
    sub_issues: Option<Nodes<SubIssueNode>>,
    closed_by_pull_requests_references: Option<Nodes<GqlClosingPull>>,
    timeline_items: Nodes<GqlEvent>,
}

impl GqlFactsIssue {
    fn into_facts(self, linewright: &Login) -> ManagedFacts {
        let author = login(self.author);
        ManagedFacts {
            filed: Filed::read(&author, &self.title, &self.body, linewright),
            labels: self.labels.nodes.into_iter().map(|l| l.name).collect(),
            sub_issues: sub_issues(self.sub_issues, None),
            work: WorkFacts {
                author: Some(author),
                assignees: logins(self.assignees),
                events: events(self.timeline_items.nodes),
                closed_by: closing_pulls(self.closed_by_pull_requests_references),
            },
        }
    }
}

/// The filters a poll finds Linewright's open items through
/// ([`GitHub::discovered`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Finder {
    /// Open issues with any of Linewright's labels.
    Labeled,
    /// Open issues assigned to Linewright.
    Assigned,
    /// Open issues and pull requests that involve Linewright.
    Involved,
}

impl Finder {
    const ALL: [Finder; 3] = [Finder::Labeled, Finder::Assigned, Finder::Involved];
}

/// The labels that mark an issue for Linewright's work, or show it worked:
/// the status labels, `linewright:designed`, under its old name too, and
/// `linewright:stalled`.
fn linewrights_labels() -> Vec<&'static str> {
    StatusLabel::ALL
        .iter()
        .map(|status| status.label())
        .chain([label::DESIGNED, label::SPEC_PROCESSED, label::STALLED])
        .collect()
}

/// The query, with its variables, asking each finder of `cursors` for its
/// page after its cursor, under its alias: `labeled` and `assigned` on the
/// repository's `issues` connection, which GitHub answers fresh and whose
/// `labels` matches any of them
/// (<https://docs.github.com/en/graphql/reference/objects#repository>), and
/// `involved`, a search
/// (<https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests#search-by-a-user-thats-involved-in-an-issue-or-pull-request>).
fn discovery_query(
    owner: &str,
    name: &str,
    linewright: &str,
    cursors: &BTreeMap<Finder, Option<String>>,
) -> (String, Value) {
    let mut parameters = String::new();
    let mut repository = String::new();
    let mut search = String::new();
    let mut variables = serde_json::Map::new();
    let page = "pageInfo { hasNextPage endCursor } nodes";
    for (&finder, cursor) in cursors {
        match finder {
            Finder::Labeled => {
                parameters.push_str(", $labels: [String!], $labeled: String");
                repository.push_str(&format!(
                    "labeled: issues(first: 100, after: $labeled, states: [OPEN], labels: $labels) {{ {page} {{ __typename number }} }} "
                ));
                variables.insert("labels".into(), json!(linewrights_labels()));
                variables.insert("labeled".into(), json!(cursor));
            }
            Finder::Assigned => {
                parameters.push_str(", $assignee: String, $assigned: String");
                repository.push_str(&format!(
                    "assigned: issues(first: 100, after: $assigned, states: [OPEN], filterBy: {{assignee: $assignee}}) {{ {page} {{ __typename number }} }} "
                ));
                variables.insert("assignee".into(), json!(linewright));
                variables.insert("assigned".into(), json!(cursor));
            }
            Finder::Involved => {
                parameters.push_str(", $involves: String!, $involved: String");
                search = format!(
                    "involved: search(type: ISSUE, first: 100, after: $involved, query: $involves) {{ {page} {{ __typename ... on Issue {{ number }} ... on PullRequest {{ number }} }} }}"
                );
                variables.insert(
                    "involves".into(),
                    json!(format!("repo:{owner}/{name} is:open involves:{linewright}")),
                );
                variables.insert("involved".into(), json!(cursor));
            }
        }
    }
    variables.insert("owner".into(), json!(owner));
    variables.insert("name".into(), json!(name));
    let query = format!(
        "query($owner: String!, $name: String!{parameters}) {{
          repository(owner: $owner, name: $name) {{ __typename {repository}}}
          {search}
        }}"
    );
    (query, Value::Object(variables))
}

/// The answer to a [`discovery_query`]: the page of each finder it asked.
#[derive(Deserialize)]
struct Discovered {
    repository: DiscoveredIssues,
    involved: Option<Page<GqlFound>>,
}

#[derive(Deserialize)]
struct DiscoveredIssues {
    labeled: Option<Page<GqlFound>>,
    assigned: Option<Page<GqlFound>>,
}

/// An item a finder found: a search of `type: ISSUE` finds issues and pull
/// requests alone
/// (<https://docs.github.com/en/graphql/reference/enums#searchtype>).
#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum GqlFound {
    Issue { number: u64 },
    PullRequest { number: u64 },
}

impl GqlFound {
    fn item(self) -> Item {
        match self {
            GqlFound::Issue { number } => Item::Issue(Number(number)),
            GqlFound::PullRequest { number } => Item::Pull(Number(number)),
        }
    }
}

/// The pull requests read in one query. Ten with the connections of
/// `PullFields` each, a hundred comments in each of a hundred review threads
/// among them, cost about 1,100 requests, 11 points
/// (<https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api#calculating-a-rate-limit-score-before-running-the-call>).
const PULLS_A_QUERY: usize = 10;

/// A query for the pull requests `numbers`, each read with `PullFields`
/// under the alias `i` and its number, asking whether each check is required
/// of the pull request it is on.
fn pulls_query(numbers: &[Number]) -> String {
    let items: String = numbers
        .iter()
        .map(|Number(n)| {
            let fields = pull_fields!().replace("$number", &n.to_string());
            format!("i{n}: pullRequest(number: {n}) {fields} ")
        })
        .collect();
    format!(
        "query($owner: String!, $name: String!) {{
          repository(owner: $owner, name: $name) {{ {items}}}
        }}"
    )
}

/// What the open issues read give a snapshot ([`open_issues_held`]).
struct OpenIssues {
    /// Those it holds ([`holds`]).
    issues: Vec<Issue>,
    /// The spec changes' issues among them, held or not.
    spec_change_issues: Vec<SpecChangeIssue>,
    /// The pull requests closing those it holds that it reads: the open
    /// ones, and Linewright's closed or merged.
    closing_pulls: BTreeSet<Number>,
    /// Linewright's closed or merged pull requests among them.
    closed_by_linewright: BTreeSet<Number>,
}

/// What `open`, the open issues read, give a snapshot.
fn open_issues_held(open: Vec<Issue>, linewright: &Login) -> OpenIssues {
    let spec_change_issues = open
        .iter()
        .filter_map(|issue| {
            Some(SpecChangeIssue {
                number: issue.number,
                spec_change: issue.spec_change()?,
            })
        })
        .collect();
    let issues: Vec<Issue> = open
        .into_iter()
        .filter(|issue| holds(issue, linewright))
        .collect();
    let closing = issues.iter().flat_map(|issue| &issue.closed_by);
    let closed_by_linewright = closing
        .clone()
        .filter(|p| !p.is_open() && p.author == *linewright)
        .map(|p| p.number)
        .collect();
    let closing_pulls = closing
        .filter(|p| p.is_open() || p.author == *linewright)
        .map(|p| p.number)
        .collect();
    OpenIssues {
        issues,
        spec_change_issues,
        closing_pulls,
        closed_by_linewright,
    }
}

/// Linewright's pull requests and the people's among `read` that a snapshot
/// holds: the open ones as [`Pull::held`] tells, and Linewright's closed or
/// merged ones of `closed_by_linewright`.
fn pulls_held(
    read: Vec<Pull>,
    closed_by_linewright: &BTreeSet<Number>,
    linewright: &Login,
) -> (Vec<Pull>, Vec<Pull>) {
    let mut pulls = vec![];
    let mut peoples_pulls = vec![];
    for pull in read {
        match pull.held(linewright) {
            Some(PullHeld::Linewrights)
                if pull.is_open() || closed_by_linewright.contains(&pull.number) =>
            {
                pulls.push(pull);
            }
            Some(PullHeld::Peoples) => peoples_pulls.push(pull),
            Some(PullHeld::Linewrights) | None => {}
        }
    }
    (pulls, peoples_pulls)
}

/// What the issues found closed since a time give a snapshot
/// ([`closed_issues_among`]).
#[derive(Debug, Default, PartialEq, Eq)]
struct ClosedIssues {
    /// Those Linewright may manage.
    managed: BTreeSet<Number>,
    /// The spec changes' issues Linewright opened, managed or not.
    spec_change_issues: Vec<SpecChangeIssue>,
}

/// What `issues`, found closed and updated since `since`, give a snapshot:
/// those that closed since then.
fn closed_issues_among(
    issues: Vec<GqlFactsIssue>,
    since: Timestamp,
    linewright: &Login,
) -> ClosedIssues {
    let mut found = ClosedIssues::default();
    for issue in issues {
        if issue.state != IssueState::Closed || issue.closed_at.is_none_or(|at| at < since) {
            continue;
        }
        let number = Number(issue.number);
        let facts = issue.into_facts(linewright);
        if let Some(spec_change) = facts.filed.and_then(Filed::spec_change) {
            found.spec_change_issues.push(SpecChangeIssue {
                number,
                spec_change,
            });
        }
        if facts.candidate(false, linewright) {
            found.managed.insert(number);
        }
    }
    found
}

/// <https://docs.github.com/en/graphql/reference/enums#issuestatereason>
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GqlStateReason {
    Completed,
    NotPlanned,
    Duplicate,
    /// `REOPENED`, which an open issue carries, or one added since.
    #[serde(other)]
    Other,
}

impl GqlStateReason {
    fn reason(self) -> Option<IssueCloseReason> {
        match self {
            GqlStateReason::Completed => Some(IssueCloseReason::Completed),
            GqlStateReason::NotPlanned => Some(IssueCloseReason::NotPlanned),
            GqlStateReason::Duplicate => Some(IssueCloseReason::Duplicate),
            GqlStateReason::Other => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlClosedPull {
    number: u64,
    title: String,
    closed_at: Option<Timestamp>,
    updated_at: Timestamp,
    author: Option<Actor>,
    closing_issues_references: Option<Nodes<NumberNode>>,
    timeline_items: Nodes<GqlEvent>,
    /// Empty in a payload recorded before Linewright read them, as are its
    /// reviews requested and reviews.
    #[serde(default = "no_nodes")]
    assignees: Nodes<LoginNode>,
    #[serde(default = "no_nodes")]
    review_requests: Nodes<GqlReviewRequest>,
    #[serde(default = "no_nodes")]
    reviews: Nodes<GqlReviewAuthor>,
}

impl GqlClosedPull {
    /// `None` when Linewright did not work it ([`PullSigns::worked_by`]).
    fn into_closed(self, linewright: &Login) -> Option<ClosedItem> {
        let author = login(self.author);
        let worked = PullSigns {
            author: &author,
            assignees: &[],
            events: &events(self.timeline_items.nodes),
        }
        .worked_by(linewright);
        if !worked {
            return None;
        }
        Some(ClosedItem {
            number: Number(self.number),
            title: self.title,
            closed_at: self.closed_at?,
            updated_at: self.updated_at,
            kind: ClosedKind::Pull {
                closes: closes(self.closing_issues_references),
                author: Some(author),
                assignees: logins(self.assignees),
                review_requests: requested_users(&self.review_requests.nodes),
                reviewers: reviewers(self.reviews.nodes),
            },
        })
    }
}

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum GqlEvent {
    #[serde(rename = "LabeledEvent")]
    Labeled(GqlLabeling),
    #[serde(rename = "UnlabeledEvent")]
    Unlabeled(GqlLabeling),
    #[serde(rename = "ReopenedEvent", rename_all = "camelCase")]
    Reopened { created_at: Timestamp },
    #[serde(rename = "IssueComment", rename_all = "camelCase")]
    Commented {
        created_at: Timestamp,
        author: Option<Actor>,
    },
    #[serde(untagged)]
    Assignment(GqlAssignment),
}

/// A sub-issue added to an issue, by `actor`
/// (<https://docs.github.com/en/graphql/reference/objects#subissueaddedevent>).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlSubIssueAdded {
    actor: Option<Actor>,
    /// `None` where the reader may not see the sub-issue.
    sub_issue: Option<SubIssueNumber>,
}

#[derive(Deserialize)]
struct SubIssueNumber {
    number: u64,
}

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum GqlAssignment {
    #[serde(rename = "AssignedEvent")]
    Assigned(GqlAssigned),
    #[serde(rename = "UnassignedEvent")]
    Unassigned(GqlUnassigned),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlAssigned {
    created_at: Timestamp,
    actor: Option<Actor>,
    assignee: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlUnassigned {
    created_at: Timestamp,
    actor: Option<Actor>,
    assignee: Option<Value>,
}

/// `None` for an assignee with no login, such as a team.
fn assignee(value: Option<Value>) -> Option<Login> {
    let actor: Actor = serde_json::from_value(value?).ok()?;
    Some(login(Some(actor)))
}

impl GqlAssignment {
    fn read(self) -> Option<Assignment> {
        Some(match self {
            GqlAssignment::Assigned(e) => Assignment::Assigned {
                assignee: assignee(e.assignee)?,
                actor: login(e.actor),
                at: e.created_at,
            },
            GqlAssignment::Unassigned(e) => Assignment::Unassigned {
                assignee: assignee(e.assignee)?,
                actor: login(e.actor),
                at: e.created_at,
            },
        })
    }
}

/// <https://docs.github.com/en/graphql/reference/enums#issuestate>
#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum IssueState {
    Open,
    Closed,
}

impl GqlIssue {
    fn into_issue(self, linewright: &Login) -> Issue {
        let i = self;
        let author = login(i.author);
        let filed = Filed::read(&author, &i.title, &i.body, linewright);
        let sub_issues = sub_issues(i.sub_issues, i.sub_issue_adds);
        let events = events(i.timeline_items.nodes);
        Issue {
            number: Number(i.number),
            title: i.title,
            body: i.body,
            author,
            open: i.state == IssueState::Open,
            created_at: i.created_at,
            updated_at: i.updated_at,
            labels: i.labels.nodes.into_iter().map(|l| l.name).collect(),
            assignees: logins(i.assignees),
            comments: i.comments.nodes.into_iter().map(Comment::from).collect(),
            blocked_by: linked(i.blocked_by),
            sub_issues,
            parent: i.parent.map(|p| p.into_parent(linewright)),
            events,
            filed,
            closed_by: closing_pulls(i.closed_by_pull_requests_references),
        }
    }
}

fn closing_pulls(nodes: Option<Nodes<GqlClosingPull>>) -> Vec<ClosingPull> {
    nodes
        .map(|n| n.nodes)
        .unwrap_or_default()
        .into_iter()
        .map(|p| ClosingPull {
            number: Number(p.number),
            state: pull_state(&p.state, p.is_draft, p.merged_at, p.closed_at),
            author: login(p.author),
            updated_at: p.updated_at,
            base_ref: p.base_ref_name,
        })
        .collect()
}

fn events(nodes: Vec<GqlEvent>) -> Vec<IssueEvent> {
    nodes
        .into_iter()
        .filter_map(|e| match e {
            GqlEvent::Labeled(labeling) => Some(IssueEvent::Labeled {
                label: labeling.label.name,
                by: labeling.actor.map(|actor| login(Some(actor))),
                at: labeling.created_at,
            }),
            GqlEvent::Unlabeled(labeling) => Some(IssueEvent::Unlabeled {
                label: labeling.label.name,
                at: labeling.created_at,
            }),
            GqlEvent::Reopened { created_at } => Some(IssueEvent::Reopened { at: created_at }),
            GqlEvent::Commented { created_at, author } => Some(IssueEvent::Commented {
                by: login(author),
                at: created_at,
            }),
            GqlEvent::Assignment(assignment) => assignment.read().map(IssueEvent::Assignment),
        })
        .collect()
}

fn logins(nodes: Nodes<LoginNode>) -> Vec<Login> {
    nodes
        .nodes
        .into_iter()
        .map(|a| Login::new(a.login))
        .collect()
}

fn pull_state(
    state: &str,
    is_draft: bool,
    merged_at: Option<Timestamp>,
    closed_at: Option<Timestamp>,
) -> PullState {
    match (state, merged_at, closed_at) {
        ("MERGED", Some(at), _) => PullState::Merged { at },
        ("CLOSED", _, Some(at)) => PullState::Closed { at },
        _ => PullState::Open { draft: is_draft },
    }
}

const PULL_FIELDS: &str = concat!("fragment PullFields on PullRequest ", pull_fields!());

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPull {
    number: u64,
    title: String,
    body: String,
    created_at: Timestamp,
    updated_at: Timestamp,
    base_ref_name: String,
    head_ref_name: String,
    head_ref_oid: String,
    /// Whether its head is in another repository, such as a fork; `false`
    /// in a payload recorded before Linewright read it.
    #[serde(default)]
    is_cross_repository: bool,
    is_draft: bool,
    state: String,
    merged_at: Option<Timestamp>,
    closed_at: Option<Timestamp>,
    mergeable: String,
    merge_state_status: String,
    can_be_rebased: bool,
    review_decision: Option<GqlReviewDecision>,
    author: Option<Actor>,
    closing_issues_references: Option<Nodes<NumberNode>>,
    comments: Nodes<GqlComment>,
    reviews: Nodes<GqlReview>,
    review_requests: Nodes<GqlReviewRequest>,
    review_threads: Nodes<GqlThread>,
    status_check_rollup: Option<Rollup>,
    labels: Nodes<Name>,
    assignees: Nodes<LoginNode>,
    timeline_items: Nodes<GqlEvent>,
}

/// <https://docs.github.com/en/graphql/reference/enums#pullrequestreviewdecision>
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GqlReviewDecision {
    ChangesRequested,
    ReviewRequired,
    Approved,
    /// One added since.
    #[serde(other)]
    Other,
}

impl GqlReviewDecision {
    fn decision(self) -> Option<ReviewDecision> {
        match self {
            GqlReviewDecision::ChangesRequested => Some(ReviewDecision::ChangesRequested),
            GqlReviewDecision::ReviewRequired => Some(ReviewDecision::ReviewRequired),
            GqlReviewDecision::Approved => Some(ReviewDecision::Approved),
            GqlReviewDecision::Other => None,
        }
    }
}

#[derive(Deserialize)]
struct NumberNode {
    number: u64,
}

fn closes(references: Option<Nodes<NumberNode>>) -> Vec<Number> {
    references
        .map(|n| n.nodes.into_iter().map(|n| Number(n.number)).collect())
        .unwrap_or_default()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlReview {
    author: Option<Actor>,
    state: String,
    body: String,
    submitted_at: Option<Timestamp>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlReviewRequest {
    as_code_owner: bool,
    requested_reviewer: Option<GqlReviewer>,
}

/// A team is not named: its fields need the `read:org` scope, beyond the
/// token's (<https://docs.github.com/en/graphql/reference/objects#team>).
#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum GqlReviewer {
    User {
        login: String,
    },
    Team,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlThread {
    id: String,
    path: String,
    is_resolved: bool,
    comments: Nodes<GqlComment>,
}

#[derive(Deserialize)]
struct Rollup {
    contexts: Nodes<GqlContext>,
}

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum GqlContext {
    #[serde(rename_all = "camelCase")]
    CheckRun {
        name: String,
        status: String,
        conclusion: Option<String>,
        is_required: bool,
    },
    #[serde(rename_all = "camelCase")]
    StatusContext {
        context: String,
        state: String,
        is_required: bool,
    },
}

impl GqlContext {
    /// <https://docs.github.com/en/graphql/reference/checks#enum-checkconclusionstate>
    ///
    /// A check run's conclusion is its verdict, whatever its status says:
    /// GitHub sometimes still reports a run that has concluded as
    /// `IN_PROGRESS`, as for `css` on
    /// <https://github.com/tibbe/linewright/pull/543>.
    fn check(&self) -> Check {
        match self {
            GqlContext::CheckRun {
                name,
                status,
                conclusion,
                ..
            } => Check {
                name: name.clone(),
                state: match (status.as_str(), conclusion.as_deref()) {
                    (_, Some("SUCCESS" | "NEUTRAL" | "SKIPPED")) => CheckState::Passed,
                    (_, Some(_)) | ("COMPLETED", None) => CheckState::Failed,
                    _ => CheckState::Pending,
                },
            },
            GqlContext::StatusContext { context, state, .. } => Check {
                name: context.clone(),
                state: match state.as_str() {
                    "SUCCESS" => CheckState::Passed,
                    "FAILURE" | "ERROR" => CheckState::Failed,
                    _ => CheckState::Pending,
                },
            },
        }
    }

    fn is_required(&self) -> bool {
        match self {
            GqlContext::CheckRun { is_required, .. }
            | GqlContext::StatusContext { is_required, .. } => *is_required,
        }
    }
}

impl GqlPull {
    /// Whether its pushes are read: while it is open, from a branch of this
    /// repository, whose activity holds the pushes to its own branches
    /// alone, not a fork's.
    fn pushes_readable(&self) -> bool {
        self.state == "OPEN" && !self.is_cross_repository
    }

    /// What merging the pull request still requires under `rules`: the code
    /// owners' review where it is requested and GitHub decides a review is
    /// required or `rules` require it, otherwise an approval where GitHub
    /// decides a review is required or changes were requested; and each
    /// required check that has not passed.
    fn requirements(&self, rules: &BranchRules) -> Vec<Requirement> {
        let mut requires = vec![];
        match (self.review_decision, self.code_owner_review()) {
            (Some(GqlReviewDecision::ReviewRequired), Some(review)) => requires.push(review),
            (_, Some(review)) if rules.code_owner_review => requires.push(review),
            (Some(GqlReviewDecision::ReviewRequired | GqlReviewDecision::ChangesRequested), _) => {
                requires.push(Requirement::Approval);
            }
            _ => {}
        }
        let contexts = self
            .status_check_rollup
            .as_ref()
            .map_or(&[][..], |r| &r.contexts.nodes[..]);
        let passed: Vec<String> = contexts
            .iter()
            .map(GqlContext::check)
            .filter(|c| c.state == CheckState::Passed)
            .map(|c| c.name)
            .collect();
        let mut missing: Vec<String> = contexts
            .iter()
            .filter(|c| c.is_required())
            .map(|c| c.check().name)
            .chain(rules.checks.iter().cloned())
            .filter(|name| !passed.contains(name))
            .collect();
        missing.sort();
        missing.dedup();
        requires.extend(missing.into_iter().map(|name| Requirement::Check { name }));
        requires
    }

    /// The code owners' review requested, if any, naming the code owners
    /// only when they are all users.
    fn code_owner_review(&self) -> Option<Requirement> {
        let code_owners: Vec<Option<String>> = self
            .review_requests
            .nodes
            .iter()
            .filter(|r| r.as_code_owner)
            .filter_map(|r| match r.requested_reviewer.as_ref()? {
                GqlReviewer::User { login } => Some(Some(login.clone())),
                GqlReviewer::Team => Some(None),
                GqlReviewer::Other => None,
            })
            .collect();
        if code_owners.is_empty() {
            return None;
        }
        let users: Option<Vec<String>> = code_owners.into_iter().collect();
        Some(Requirement::CodeOwnerReview {
            owners: users.unwrap_or_default(),
        })
    }

    fn into_pull(self, pushes: Vec<Push>, rules: &BranchRules) -> Pull {
        let state = pull_state(&self.state, self.is_draft, self.merged_at, self.closed_at);
        let mergeability = match (self.mergeable.as_str(), self.merge_state_status.as_str()) {
            ("CONFLICTING", _) | (_, "DIRTY") => Mergeability::Conflicting,
            (_, "BEHIND") => Mergeability::Behind,
            (_, "BLOCKED") => Mergeability::Blocked {
                requires: self.requirements(rules),
            },
            // A draft's merge state is `DRAFT`, which says nothing of whether it
            // merges: <https://docs.github.com/en/graphql/reference/enums#mergestatestatus>
            ("UNKNOWN", _) | (_, "UNKNOWN" | "DRAFT") => Mergeability::Unknown,
            _ => Mergeability::Clean,
        };
        // `canBeRebased` is never null, so it is taken as not computed while
        // whether the pull request merges is not computed either
        // (<https://docs.github.com/en/graphql/reference/objects#pullrequest>).
        let rebaseability = match (self.mergeable.as_str(), self.can_be_rebased) {
            ("UNKNOWN", _) => Rebaseability::Unknown,
            (_, true) => Rebaseability::Rebaseable,
            (_, false) => Rebaseability::NotRebaseable,
        };
        let review_decision = self.review_decision.and_then(GqlReviewDecision::decision);
        let review_requests = requested_users(&self.review_requests.nodes);
        let (mut labelings, mut assignments) = (vec![], vec![]);
        for event in events(self.timeline_items.nodes) {
            match event {
                IssueEvent::Labeled { label, by, at } => {
                    labelings.push(Labeling { label, by, at });
                }
                IssueEvent::Assignment(assignment) => assignments.push(assignment),
                IssueEvent::Unlabeled { .. }
                | IssueEvent::Reopened { .. }
                | IssueEvent::Commented { .. } => {}
            }
        }
        Pull {
            number: Number(self.number),
            title: self.title,
            body: self.body,
            author: login(self.author),
            created_at: self.created_at,
            updated_at: self.updated_at,
            base_ref: self.base_ref_name,
            head_ref: self.head_ref_name,
            head_sha: self.head_ref_oid,
            state,
            closes: closes(self.closing_issues_references),
            pushes,
            mergeability,
            rebaseability,
            checks: self
                .status_check_rollup
                .map(|r| r.contexts.nodes.iter().map(GqlContext::check).collect())
                .unwrap_or_default(),
            comments: self.comments.nodes.into_iter().map(Comment::from).collect(),
            reviews: self
                .reviews
                .nodes
                .into_iter()
                .filter_map(|r| {
                    Some(Review {
                        author: login(r.author),
                        state: match r.state.as_str() {
                            "APPROVED" => ReviewState::Approved,
                            "CHANGES_REQUESTED" => ReviewState::ChangesRequested,
                            "COMMENTED" => ReviewState::Commented,
                            "DISMISSED" => ReviewState::Dismissed,
                            _ => return None,
                        },
                        body: r.body,
                        submitted_at: r.submitted_at?,
                    })
                })
                .collect(),
            threads: self
                .review_threads
                .nodes
                .into_iter()
                .map(|t| Thread {
                    id: t.id,
                    path: t.path,
                    resolved: t.is_resolved,
                    comments: t.comments.nodes.into_iter().map(Comment::from).collect(),
                })
                .collect(),
            review_decision,
            labels: self.labels.nodes.into_iter().map(|l| l.name).collect(),
            labelings,
            assignees: self
                .assignees
                .nodes
                .into_iter()
                .map(|a| Login::new(a.login))
                .collect(),
            assignments,
            review_requests,
        }
    }
}

/// The users of `requests` a review is requested of by name: a team is
/// left out.
fn requested_users(requests: &[GqlReviewRequest]) -> Vec<Login> {
    requests
        .iter()
        .filter_map(|request| match request.requested_reviewer.as_ref()? {
            GqlReviewer::User { login } => Some(Login::new(login)),
            GqlReviewer::Team | GqlReviewer::Other => None,
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlReviewAuthor {
    author: Option<Actor>,
    /// `None` for a pending review, which only its author sees.
    submitted_at: Option<Timestamp>,
}

/// The authors of the submitted `reviews`, each once, by login.
fn reviewers(reviews: Vec<GqlReviewAuthor>) -> Vec<Login> {
    let reviewers: BTreeSet<Login> = reviews
        .into_iter()
        .filter(|review| review.submitted_at.is_some())
        .map(|review| login(review.author))
        .collect();
    reviewers.into_iter().collect()
}

const MERGED_FIELDS: &str = concat!(
    "fragment MergedFields on PullRequest {
  number title body mergedAt updatedAt baseRefName
  author { __typename login }
  ",
    closes_field!(),
    "
  labels(first: 50) { nodes { name } }
  assignees(first: 20) { nodes { login } }
  files(first: 100) { pageInfo { hasNextPage } nodes { path } }
  ",
    reviewing_fields!(),
    "
  ",
    assignment_timeline!(comments),
    "
}"
);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlMerged {
    number: u64,
    title: String,
    body: String,
    merged_at: Option<Timestamp>,
    updated_at: Timestamp,
    base_ref_name: String,
    author: Option<Actor>,
    labels: Nodes<Name>,
    closing_issues_references: Option<Nodes<NumberNode>>,
    assignees: Nodes<LoginNode>,
    files: Option<FilesPage>,
    timeline_items: Nodes<GqlEvent>,
    /// Empty in a payload recorded before Linewright read them, as are its
    /// reviews.
    #[serde(default = "no_nodes")]
    review_requests: Nodes<GqlReviewRequest>,
    #[serde(default = "no_nodes")]
    reviews: Nodes<GqlReviewAuthor>,
}

fn no_nodes<T>() -> Nodes<T> {
    Nodes { nodes: vec![] }
}

impl GqlMerged {
    /// When it merged, if a snapshot over `windows` holds it among the pull
    /// requests merged into `base`.
    fn held_merged_at(&self, base: &str, windows: Windows) -> Option<Timestamp> {
        self.merged_at
            .filter(|at| windows.holds_merged(*at, self.updated_at))
            .filter(|_| self.base_ref_name == base)
    }

    fn into_merged(self, merged_at: Timestamp, files: Vec<String>) -> MergedPull {
        MergedPull {
            number: Number(self.number),
            title: self.title,
            body: self.body,
            author: login(self.author),
            merged_at,
            updated_at: self.updated_at,
            base_ref: self.base_ref_name,
            labels: self.labels.nodes.into_iter().map(|l| l.name).collect(),
            closes: closes(self.closing_issues_references),
            files,
            assignees: logins(self.assignees),
            events: events(self.timeline_items.nodes),
            review_requests: requested_users(&self.review_requests.nodes),
            reviewers: reviewers(self.reviews.nodes),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesPage {
    page_info: FilesPageInfo,
    nodes: Vec<Path_>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesPageInfo {
    has_next_page: bool,
}

#[derive(Deserialize)]
struct Path_ {
    path: String,
}

#[async_trait]
impl Store for GitHub {
    /// GitHub answers a token that cannot see a private repository as if
    /// it did not exist (<https://docs.github.com/en/rest/repos/repos#get-a-repository>).
    async fn check_reads(&self, token: &str) -> Result<()> {
        let reader = GitHub {
            token: token.to_string(),
            ..self.clone()
        };
        match reader.rest_ok(Method::GET, &self.repo_path(""), None).await {
            Ok(()) => Ok(()),
            Err(e) => match e.downcast_ref::<ErrorResponse>() {
                Some(response)
                    if [
                        StatusCode::UNAUTHORIZED,
                        StatusCode::FORBIDDEN,
                        StatusCode::NOT_FOUND,
                    ]
                    .contains(&response.status) =>
                {
                    bail!(
                        "the token cannot read {}/{}: it needs read access to the repository \
                         ({})",
                        self.owner,
                        self.name,
                        response.message()
                    )
                }
                _ => Err(e),
            },
        }
    }

    async fn record_properties(&self) -> Result<RecordedProperties> {
        let repository: Value = self.rest(Method::GET, &self.repo_path(""), None).await?;
        let account: Value = self.rest(Method::GET, "/user", None).await?;
        let settings: Value = self
            .graphql(
                properties::ISSUE_TYPES_AND_MERGE_METHODS_QUERY,
                json!({"owner": self.owner, "name": self.name}),
            )
            .await?;
        let branch = repository["default_branch"]
            .as_str()
            .context("GitHub gave the repository no default branch")?;
        let rules = match self.rules(branch).await? {
            Some(rules) => Value::Array(rules),
            None => {
                tracing::debug!(branch, "GitHub refuses the branch's rules for the plan");
                Value::Null
            }
        };
        // Classic branch protection is readable by admins only:
        // <https://docs.github.com/en/rest/branches/branch-protection#get-branch-protection>
        let protection = self
            .rest::<Value>(
                Method::GET,
                &self.repo_path(&format!("/branches/{branch}/protection")),
                None,
            )
            .await
            .unwrap_or(Value::Null);
        Ok([
            (Property::Repository, repository),
            (Property::Account, account),
            (Property::IssueTypesAndMergeMethods, settings),
            (Property::Rules, rules),
            (Property::Protection, protection),
        ]
        .into_iter()
        .collect())
    }

    async fn label_exists(&self, name: &str) -> Result<bool> {
        let response = self
            .send(Method::GET, &self.repo_path(&label_path(name)), None)
            .await?;
        match response.status() {
            s if s.is_success() => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            s => bail!("looking up label {name}: {s}: {}", response.text().await?),
        }
    }

    async fn labels(&self) -> Result<Vec<label::RepoLabel>> {
        const PER_PAGE: usize = 100;
        let labels: Vec<RestLabel> = every_page(PER_PAGE, |page| async move {
            let path = self.repo_path(&format!("/labels?per_page={PER_PAGE}&page={page}"));
            self.rest(Method::GET, &path, None).await
        })
        .await?;
        Ok(labels.into_iter().map(label::RepoLabel::from).collect())
    }

    async fn create_label(&self, label: &label::Label) -> Result<()> {
        self.rest_ok(
            Method::POST,
            &self.repo_path("/labels"),
            Some(json!({"name": label.name, "description": label.description, "color": "ededed"})),
        )
        .await
    }

    async fn describe_label(&self, name: &str, description: &str) -> Result<()> {
        let (method, path, body) = describe_label_request(name, description);
        self.rest_ok(method, &self.repo_path(&path), Some(body))
            .await
    }

    async fn snapshot(&self, windows: Windows, scope: &Scope) -> Result<Snapshot> {
        let linewright = self.linewright();
        let rules = RulesRead::default();
        let open_issues = async {
            let mut issue_numbers = BTreeSet::new();
            let mut pull_numbers = BTreeSet::new();
            for item in self
                .discovered()
                .await?
                .into_iter()
                .chain(scope.items.iter().copied())
            {
                match item {
                    Item::Issue(number) => issue_numbers.insert(number),
                    Item::Pull(number) => pull_numbers.insert(number),
                };
            }
            let open: Vec<Issue> = self
                .numbered::<GqlIssue>(ItemKind::Issue, &issue_numbers, ISSUE_FIELDS)
                .await?
                .into_iter()
                .map(|issue| issue.into_issue(&linewright))
                .filter(|issue| issue.open)
                .collect();
            let held = open_issues_held(open, &linewright);
            pull_numbers.extend(held.closing_pulls.iter().copied());
            anyhow::Ok((held, pull_numbers))
        };
        let default_branch = async {
            let repository: Value = self.rest(Method::GET, &self.repo_path(""), None).await?;
            let default_branch = repository["default_branch"]
                .as_str()
                .context("the repository has a default branch")?;
            anyhow::Ok(default_branch.to_string())
        };
        let ((held, pull_numbers), default_branch) = tokio::try_join!(open_issues, default_branch)?;
        let held_numbers = held.issues.iter().map(|issue| issue.number).collect();
        let open_pulls_then_closed = async {
            let open_pulls = self
                .batched::<GqlPull>(&pull_numbers, PULLS_A_QUERY, pulls_query)
                .await?;
            let closed_issues = async {
                let closed_since = self.closed_since(windows.read_since()).await?;
                let closed = self.closed_issues_numbered(&closed_since.managed).await?;
                anyhow::Ok((closed_since.spec_change_issues, closed))
            };
            tokio::try_join!(
                try_join_all(open_pulls.into_iter().map(|pr| self.pull_view(pr, &rules))),
                closed_issues,
                self.closed_pulls(windows.read_since()),
                self.merged_within(&default_branch, windows),
            )
        };
        let (
            (pulls_read, (closed_spec_change_issues, closed_issues), closed_pulls, merged),
            (ticket_branches, gone_branches),
        ) = tokio::try_join!(
            open_pulls_then_closed,
            self.ticket_branches(&scope.branches, &held_numbers),
        )?;
        self.pushes_kept
            .keep_only(&pulls_read.iter().map(|p| p.head_ref.as_str()).collect());
        let OpenIssues {
            issues,
            mut spec_change_issues,
            closing_pulls: _,
            closed_by_linewright,
        } = held;
        let (pulls, peoples_pulls) = pulls_held(pulls_read, &closed_by_linewright, &linewright);
        spec_change_issues.extend(closed_spec_change_issues);
        spec_change_issues.sort_by_key(|issue| issue.number);
        spec_change_issues.dedup_by_key(|issue| issue.number);
        let mut snapshot = Snapshot {
            ticket_branches,
            gone_branches,
            merged,
            spec_change_issues,
            closed: closed_within(closed_issues.into_iter().chain(closed_pulls), windows),
            openings: vec![],
            default_branch,
            issues,
            pulls,
            peoples_pulls,
        };
        let tickets = snapshot.unheld_closed_tickets(&linewright);
        for ticket in self.closed_issues_numbered(&tickets).await? {
            snapshot.put_closed(ticket);
        }
        let closing = snapshot.unheld_closing_pulls(&linewright);
        for pull in self.closed_pulls_numbered(&closing).await? {
            snapshot.put_closed(pull);
        }
        read_openings(&mut snapshot, &linewright, |unread| self.openings(unread)).await?;
        Ok(snapshot)
    }

    async fn pull(&self, number: Number) -> Result<Pull> {
        let pr = self.gql_pull(number).await?;
        self.pull_view(
            pr.with_context(|| format!("there is no pull request {number}"))?,
            &RulesRead::default(),
        )
        .await
    }

    async fn any_item(&self, number: Number) -> Result<Option<AnyItem>> {
        if let Some(issue) = self.gql_issue(number).await? {
            let linewright = self.linewright();
            let open = issue.state == IssueState::Open;
            let issue = issue.into_issue(&linewright);
            if !open {
                let closed: Option<GqlClosedIssue> =
                    self.issue_with(number, CLOSED_ISSUE_FIELDS).await?;
                let closing = closed
                    .and_then(|i| i.into_closed(&linewright))
                    .and_then(|closed| closed.issue_closing());
                if let Some(closing) = closing {
                    return Ok(Some(AnyItem::ClosedIssue {
                        issue: Box::new(issue),
                        closing,
                    }));
                }
            }
            return Ok(Some(AnyItem::issue(issue, &linewright)));
        }
        let Some(pr) = self.gql_pull(number).await? else {
            return Ok(None);
        };
        if pr.state != "MERGED" {
            return Ok(Some(AnyItem::Pull(Box::new(
                self.pull_view(pr, &RulesRead::default()).await?,
            ))));
        }
        let merged = self
            .gql_merged(number)
            .await?
            .with_context(|| format!("there is no pull request {number}"))?;
        let merged_at = merged
            .merged_at
            .with_context(|| format!("GitHub gives merged pull request {number} no merge time"))?;
        Ok(Some(AnyItem::Merged {
            pull: Box::new(self.merged_of(merged, merged_at).await?),
            spec_change_issues: self.spec_change_issues_of(number).await?,
        }))
    }

    async fn linewright_pull(&self, number: Number) -> Result<Option<Pull>> {
        let Some(pr) = self.gql_pull(number).await? else {
            return Ok(None);
        };
        if !self.by_linewright(pr.author.as_ref()) {
            return Ok(None);
        }
        let mut held = pr.state == "OPEN";
        let closes: Vec<u64> = pr
            .closing_issues_references
            .iter()
            .flat_map(|n| &n.nodes)
            .map(|n| n.number)
            .collect();
        for issue in closes {
            if held {
                break;
            }
            let linewright = self.linewright();
            let issue = self.gql_issue(Number(issue)).await?;
            held = issue.is_some_and(|i| holds(&i.into_issue(&linewright), &linewright));
        }
        if !held {
            return Ok(None);
        }
        Ok(Some(self.pull_view(pr, &RulesRead::default()).await?))
    }

    async fn peoples_pull(&self, number: Number) -> Result<PeoplesPullRead> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct One {
            pull_request: GqlPeoplesPull,
        }
        let query = format!(
            "query($owner: String!, $name: String!, $number: Int!) {{
              repository(owner: $owner, name: $name) {{ pullRequest(number: $number) {{ ...PeoplesPullFields }} }}
            }}
            {PEOPLES_PULL_FIELDS}"
        );
        let data: Option<RepoData<One>> = self
            .graphql_found(
                &query,
                json!({"owner": self.owner, "name": self.name, "number": number.0}),
            )
            .await?;
        let Some(pr) = data.map(|d| d.repository.pull_request) else {
            return Ok(PeoplesPullRead::default());
        };
        let (closes, peoples) = pr.read(&self.linewright());
        let pull = if peoples {
            Some(self.pull(number).await?)
        } else {
            None
        };
        Ok(PeoplesPullRead { closes, pull })
    }

    async fn ticket_branches_pushed(&self) -> Result<Vec<TicketBranchName>> {
        Ok(self
            .pushed_refs()
            .await?
            .into_iter()
            .map(|(name, _)| name)
            .collect())
    }

    async fn issue(&self, number: Number) -> Result<IssueRead> {
        let Some(issue) = self.gql_issue(number).await? else {
            return Ok(IssueRead::Gone);
        };
        let linewright = self.linewright();
        if issue.state != IssueState::Open {
            let closed: Option<GqlClosedIssue> =
                self.issue_with(number, CLOSED_ISSUE_FIELDS).await?;
            return Ok(
                match closed
                    .and_then(|i| i.into_closed(&linewright))
                    .filter(|closed| closed.candidate(&linewright))
                {
                    Some(closed) => IssueRead::Closed(Box::new(closed)),
                    None => IssueRead::LeftOut { open: false },
                },
            );
        }
        let issue = issue.into_issue(&linewright);
        if !holds(&issue, &linewright) {
            return Ok(IssueRead::LeftOut { open: true });
        }
        let mut pulls = vec![];
        for pr in issue.closed_by.iter().filter(|p| p.author == linewright) {
            pulls.push(self.pull(pr.number).await?);
        }
        Ok(IssueRead::Held {
            issue: Box::new(issue),
            pulls,
        })
    }

    async fn merged_pull(&self, number: Number, windows: Windows) -> Result<Option<MergedPull>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct One {
            default_branch_ref: Name,
            pull_request: GqlMerged,
        }
        let query = format!(
            "query($owner: String!, $name: String!, $number: Int!) {{
              repository(owner: $owner, name: $name) {{
                defaultBranchRef {{ name }}
                pullRequest(number: $number) {{ ...MergedFields }}
              }}
            }}
            {MERGED_FIELDS}"
        );
        let Some(data): Option<RepoData<One>> = self
            .graphql_found(
                &query,
                json!({"owner": self.owner, "name": self.name, "number": number.0}),
            )
            .await?
        else {
            return Ok(None);
        };
        let One {
            default_branch_ref,
            pull_request,
        } = data.repository;
        self.merged_view(pull_request, &default_branch_ref.name, windows)
            .await
    }

    async fn diff(&self, pr: Number, paths: &[String]) -> Result<String> {
        #[derive(Deserialize)]
        struct File {
            filename: String,
            status: String,
            patch: Option<String>,
        }
        let mut diff = String::new();
        for page in 1..=30 {
            let files: Vec<File> = self
                .rest(
                    Method::GET,
                    &self.repo_path(&format!("/pulls/{}/files?per_page=100&page={page}", pr.0)),
                    None,
                )
                .await?;
            let done = files.len() < 100;
            for file in files.into_iter().filter(|f| paths.contains(&f.filename)) {
                let name = &file.filename;
                diff.push_str(&format!(
                    "diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n"
                ));
                match file.patch {
                    Some(patch) => diff.push_str(&format!("{patch}\n")),
                    None => diff.push_str(&format!(
                        "({} file, patch too large to show)\n",
                        file.status
                    )),
                }
            }
            if done {
                break;
            }
        }
        Ok(diff)
    }

    async fn check_failures(&self, pr: Number, checks: &[String]) -> Result<Vec<CheckFailure>> {
        let query = "query($owner: String!, $name: String!, $number: Int!) {
              repository(owner: $owner, name: $name) {
                pullRequest(number: $number) {
                  statusCheckRollup {
                    contexts(first: 100) {
                      nodes {
                        __typename
                        ... on CheckRun { name databaseId title summary text checkSuite { app { slug } } }
                        ... on StatusContext { context description targetUrl }
                      }
                    }
                  }
                }
              }
            }";
        let data: Value = self
            .graphql(
                query,
                json!({"owner": self.owner, "name": self.name, "number": pr.0}),
            )
            .await?;
        let nodes = data["repository"]["pullRequest"]["statusCheckRollup"]["contexts"]["nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        let mut failures = vec![];
        for node in nodes {
            let name = if node["__typename"] == "CheckRun" {
                text(&node["name"])
            } else {
                text(&node["context"])
            };
            if !checks.contains(&name) {
                continue;
            }
            let failure = if node["__typename"] == "CheckRun" {
                let run_by_github_actions = node["checkSuite"]["app"]["slug"] == "github-actions";
                let log = match node["databaseId"]
                    .as_u64()
                    .filter(|_| run_by_github_actions)
                {
                    Some(job) => Some(CheckLog {
                        step_output: self.failed_step_output(job).await.unwrap_or_default(),
                        full: Some(JobLog {
                            repo: self.full_name(),
                            job,
                        }),
                    }),
                    None => None,
                };
                CheckFailure {
                    name,
                    summary: [
                        text(&node["title"]),
                        text(&node["summary"]),
                        text(&node["text"]),
                    ]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                    log,
                }
            } else {
                CheckFailure {
                    name,
                    summary: format!(
                        "{} ({})",
                        text(&node["description"]),
                        text(&node["targetUrl"])
                    ),
                    log: None,
                }
            };
            failures.push(failure);
        }
        Ok(failures)
    }

    async fn completed_attempts(
        &self,
        created_since: Timestamp,
        known: &BTreeSet<AttemptId>,
    ) -> Result<Vec<CompletedAttempt>> {
        let window = Created {
            from: hour_of(created_since),
            to: None,
        };
        let (runs, read) = self.runs_created(window).await?;
        self.runs_listed.keep_only(&read);
        let runs = latest_attempts(runs);
        let earlier = try_join_all(runs.iter().flat_map(|run| {
            run.earlier(known)
                .into_iter()
                .map(move |id| self.earlier_attempt(run, id))
        }))
        .await?;
        let latest = runs.iter().filter_map(|run| run.completed(run));
        Ok(earlier.into_iter().flatten().chain(latest).collect())
    }

    async fn read_file(&self, branch: &str, path: &str) -> Result<Option<String>> {
        #[derive(Deserialize)]
        struct Contents {
            content: String,
            encoding: String,
        }
        let path_segments: Vec<String> = path.split('/').map(percent_encode).collect();
        let resource = self.repo_path(&format!(
            "/contents/{}?ref={}",
            path_segments.join("/"),
            percent_encode(branch)
        ));
        let answer = self.answer_retried(Method::GET, &resource, None).await?;
        let status = answer.status;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let text = answer.body;
        if !status.is_success() {
            bail!("reading {path} on {branch}: {status}: {text}");
        }
        let contents: Contents = serde_json::from_str(&text)
            .with_context(|| format!("reading {path} on {branch}: not a file"))?;
        if contents.encoding != "base64" {
            bail!(
                "reading {path} on {branch}: the file comes as {:?}, not base64",
                contents.encoding
            );
        }
        let encoded: String = contents.content.split_whitespace().collect();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .with_context(|| format!("decoding {path} on {branch}"))?;
        String::from_utf8(bytes)
            .map(Some)
            .with_context(|| format!("{path} on {branch} is not UTF-8"))
    }

    async fn read_files(&self, branch: &str, paths: &[String]) -> Result<BTreeMap<String, Blob>> {
        if paths.is_empty() {
            return Ok(BTreeMap::new());
        }
        let (query, variables) = files_query(&self.owner, &self.name, branch, paths);
        let data: RepoData<BTreeMap<String, Option<GqlObject>>> =
            self.graphql(&query, variables).await?;
        Ok(files_read(data.repository, paths))
    }

    async fn list_files(&self, branch: &str, dir: &str) -> Result<Vec<String>> {
        let resource = self.repo_path(&tree_path(branch, dir));
        let answer = self.answer_retried(Method::GET, &resource, None).await?;
        let status = answer.status;
        // GitHub answers 404 for a branch or path it does not have, and 422
        // for a path that is not a tree:
        // <https://docs.github.com/en/rest/git/trees#get-a-tree>
        if status == StatusCode::NOT_FOUND || status == StatusCode::UNPROCESSABLE_ENTITY {
            return Ok(vec![]);
        }
        let text = answer.body;
        if !status.is_success() {
            bail!("listing {dir} on {branch}: {status}: {text}");
        }
        let tree = serde_json::from_str(&text)
            .with_context(|| format!("listing {dir} on {branch}: not a tree"))?;
        files_listed(tree).with_context(|| format!("listing {dir} on {branch}"))
    }

    async fn clone_repo(&self, dir: &Path, branch: &str, config: &[(&str, &str)]) -> Result<()> {
        git::clone(&self.clone_url(), dir, branch, config, &self.git_env()).await
    }

    async fn fetch(&self, dir: &Path) -> Result<()> {
        git::fetch(dir, &self.git_env()).await
    }

    async fn fetch_commits(
        &self,
        bare: &Path,
        heads: &BTreeMap<String, git::CommitId>,
    ) -> Result<()> {
        git::fetch_commits(&self.clone_url(), bare, heads, &self.git_env()).await
    }

    async fn push(
        &self,
        dir: &Path,
        commit: &git::CommitId,
        branch: &str,
        mode: PushMode,
    ) -> Result<(), PushError> {
        let refspec = format!("{commit}:refs/heads/{branch}");
        let lease = match &mode {
            PushMode::Replace => "--force".to_string(),
            PushMode::Lease { expected } => {
                format!("--force-with-lease=refs/heads/{branch}:{expected}")
            }
        };
        git::git_with(
            dir,
            &["push", "--quiet", "origin", &refspec, &lease],
            &self.git_env(),
        )
        .await
        .map_err(PushError::from_git)?;
        Ok(())
    }

    async fn attach(
        &self,
        file: &Path,
        name: &str,
        content_type: &str,
    ) -> Result<String, AttachError> {
        let repository_id = self.repository_id().await?;
        let bytes = tokio::fs::read(file)
            .await
            .with_context(|| format!("reading {}", file.display()))?;
        let request = format!("POST {ATTACHMENTS}");
        let answer = waiting_out_rate_limits(&request, &self.sending, || async {
            let upload = self.attach_request(repository_id, name, content_type, bytes.clone());
            Answer::read(upload.send().await.with_context(|| request.clone())?).await
        })
        .await?;
        attached(&request, answer)
    }

    async fn heads(&self, branches: &[String]) -> Result<BTreeMap<String, git::CommitId>> {
        #[derive(Deserialize)]
        struct Ref {
            object: RefObject,
        }
        #[derive(Deserialize)]
        struct RefObject {
            sha: String,
        }
        let mut heads = BTreeMap::new();
        for branch in branches {
            // <https://docs.github.com/en/rest/git/refs#get-a-reference>
            let path = format!("git/ref/heads/{}", percent_encode(branch));
            if let Some(found) = self.found(&path).await? {
                let found: Ref = serde_json::from_value(found)
                    .with_context(|| format!("decoding GET {path}"))?;
                heads.insert(
                    branch.clone(),
                    git::CommitId::from(found.object.sha.as_str()),
                );
            }
        }
        Ok(heads)
    }

    async fn delete_branch(&self, branch: &str) -> Result<()> {
        let path = format!("/git/refs/heads/{}", percent_encode(branch));
        let answer = self
            .answer_retried(Method::DELETE, &self.repo_path(&path), None)
            .await?;
        branch_deleted(branch, answer.status, &answer.body)
    }

    async fn add_label(&self, number: Number, label: &str) -> Result<()> {
        self.rest_ok(
            Method::POST,
            &self.repo_path(&format!("/issues/{}/labels", number.0)),
            Some(json!({"labels": [label]})),
        )
        .await
    }

    async fn remove_label(&self, number: Number, label: &str) -> Result<()> {
        let answer = self
            .answer_retried(
                Method::DELETE,
                &self.repo_path(&format!(
                    "/issues/{}/labels/{}",
                    number.0,
                    percent_encode(label)
                )),
                None,
            )
            .await?;
        match answer.status {
            s if s.is_success() || s == StatusCode::NOT_FOUND => Ok(()),
            s => bail!("removing {label} from {number}: {s}: {}", answer.body),
        }
    }

    /// A pull request is an issue to the assignees API
    /// (<https://docs.github.com/en/rest/issues/assignees>).
    async fn assign(&self, number: Number) -> Result<()> {
        self.rest_ok(
            Method::POST,
            &self.repo_path(&format!("/issues/{}/assignees", number.0)),
            Some(json!({"assignees": [self.viewer.login]})),
        )
        .await
    }

    async fn unassign(&self, number: Number) -> Result<()> {
        self.rest_ok(
            Method::DELETE,
            &self.repo_path(&format!("/issues/{}/assignees", number.0)),
            Some(json!({"assignees": [self.viewer.login]})),
        )
        .await
    }

    async fn assign_people(&self, issue: Number, people: &[Login]) -> Result<Vec<Login>> {
        let mut refused = vec![];
        // GitHub leaves out anyone it cannot assign, without an error, or
        // refuses the whole request as invalid:
        // <https://docs.github.com/en/rest/issues/assignees#add-assignees-to-an-issue>
        for person in people {
            let assigned = match self
                .change_assignees(Method::POST, issue, std::slice::from_ref(person))
                .await
            {
                Ok(assignees) => assignees.contains(person),
                Err(e) => match e.downcast_ref::<ErrorResponse>() {
                    Some(response)
                        if matches!(
                            response.status,
                            StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY
                        ) =>
                    {
                        false
                    }
                    _ => return Err(e),
                },
            };
            if !assigned {
                refused.push(person.clone());
            }
        }
        Ok(refused)
    }

    async fn unassign_people(&self, issue: Number, people: &[Login]) -> Result<()> {
        self.change_assignees(Method::DELETE, issue, people)
            .await
            .map(|_| ())
    }

    async fn comment(&self, number: Number, body: &str) -> Result<()> {
        let path = self.repo_path(&format!("/issues/{}/comments", number.0));
        let since = since_sent();
        let IgnoredAny = self
            .make(&path, json!({"body": body}), || async {
                let listed: Vec<ListedComment> = self
                    .rest(
                        Method::GET,
                        &format!("{path}?since={}&per_page=100", query_time(since)),
                        None,
                    )
                    .await?;
                Ok(commented(listed, &self.linewright(), body, since).then_some(IgnoredAny))
            })
            .await?;
        Ok(())
    }

    async fn create_issue(&self, issue: NewIssue) -> Result<Number> {
        let path = self.repo_path("/issues");
        let since = since_sent();
        let body = json!({"title": issue.title, "body": issue.body, "labels": issue.labels});
        let made: Made = self
            .make(&path, body, || async {
                let listed: Vec<ListedIssue> = self
                    .rest(
                        Method::GET,
                        &format!(
                            "{path}?creator={}&state=all&sort=created&direction=desc&since={}&per_page=100",
                            percent_encode(&self.viewer.login),
                            query_time(since)
                        ),
                        None,
                    )
                    .await?;
                let number = issue_made(listed, &self.linewright(), &issue, since);
                Ok(number.map(|number| Made { number }))
            })
            .await?;
        Ok(made.number)
    }

    async fn edit_body(&self, number: Number, body: &str) -> Result<()> {
        self.rest_ok(
            Method::PATCH,
            &self.repo_path(&format!("/issues/{}", number.0)),
            Some(json!({"body": body})),
        )
        .await
    }

    async fn close_issue(&self, issue: Number, reason: IssueCloseReason) -> Result<()> {
        let reason = match reason {
            IssueCloseReason::Completed => "completed",
            IssueCloseReason::NotPlanned => "not_planned",
            IssueCloseReason::Duplicate => "duplicate",
        };
        self.rest_ok(
            Method::PATCH,
            &self.repo_path(&format!("/issues/{}", issue.0)),
            Some(json!({"state": "closed", "state_reason": reason})),
        )
        .await
    }

    async fn set_issue_type(&self, issue: Number, issue_type: &str) -> Result<()> {
        self.rest_ok(
            Method::PATCH,
            &self.repo_path(&format!("/issues/{}", issue.0)),
            Some(json!({"type": issue_type})),
        )
        .await
    }

    async fn add_sub_issue(&self, parent: Number, child: Number) -> Result<()> {
        let id = self.issue_id(child).await?;
        self.link(
            &format!("/issues/{}/sub_issues", parent.0),
            json!({"sub_issue_id": id}),
            id,
        )
        .await
    }

    /// <https://docs.github.com/en/rest/issues/sub-issues#remove-sub-issue>
    async fn remove_sub_issue(&self, parent: Number, child: Number) -> Result<()> {
        let id = self.issue_id(child).await?;
        self.rest_ok(
            Method::DELETE,
            &self.repo_path(&format!("/issues/{}/sub_issue", parent.0)),
            Some(json!({"sub_issue_id": id})),
        )
        .await
    }

    async fn add_blocked_by(&self, issue: Number, blocker: Number) -> Result<()> {
        let id = self.issue_id(blocker).await?;
        self.link(
            &format!("/issues/{}/dependencies/blocked_by", issue.0),
            json!({"issue_id": id}),
            id,
        )
        .await
    }

    async fn open_pull(&self, pull: NewPull) -> Result<Number> {
        let base: Value = self.rest(Method::GET, &self.repo_path(""), None).await?;
        let path = self.repo_path("/pulls");
        let body = json!({
            "head": pull.head,
            "base": base["default_branch"],
            "title": pull.title,
            "body": pull.body,
            "draft": true,
        });
        let made: Made = self
            .make(&path, body, || async {
                let head = format!("{}:{}", self.owner, pull.head);
                let listed: Vec<ListedPull> = self
                    .rest(
                        Method::GET,
                        &format!("{path}?head={}&state=open", percent_encode(&head)),
                        None,
                    )
                    .await?;
                let number = pull_made(listed, &self.linewright());
                Ok(number.map(|number| Made { number }))
            })
            .await?;
        Ok(made.number)
    }

    async fn mark_ready(&self, pr: Number) -> Result<()> {
        let id = self.pull_node_id(pr).await?;
        let _: Value = self
            .graphql(
                "mutation($id: ID!) { markPullRequestReadyForReview(input: {pullRequestId: $id}) { clientMutationId } }",
                json!({"id": id}),
            )
            .await?;
        Ok(())
    }

    // https://docs.github.com/en/rest/pulls/pulls#update-a-pull-request
    async fn close_pull(&self, pr: Number) -> Result<()> {
        self.rest_ok(
            Method::PATCH,
            &self.repo_path(&format!("/pulls/{}", pr.0)),
            Some(json!({"state": "closed"})),
        )
        .await
    }

    async fn request_review(&self, pr: Number, reviewers: &[Login]) -> Result<()> {
        let answer = self
            .answer_retried(
                Method::POST,
                &self.repo_path(&format!("/pulls/{}/requested_reviewers", pr.0)),
                Some(json!({"reviewers": reviewers})),
            )
            .await?;
        // A reviewer who is not a collaborator cannot be requested:
        // <https://docs.github.com/en/rest/pulls/review-requests#request-reviewers-for-a-pull-request>
        if answer.status == StatusCode::UNPROCESSABLE_ENTITY {
            tracing::warn!(pr = pr.0, body = answer.body, "review request refused");
            return Ok(());
        }
        if !answer.status.is_success() {
            bail!(
                "requesting review on {pr}: {}: {}",
                answer.status,
                answer.body
            );
        }
        Ok(())
    }

    async fn merge(&self, pr: Number, head: &str, method: MergeMethod) -> Result<()> {
        self.rest_once::<Value>(
            Method::PUT,
            &self.repo_path(&format!("/pulls/{}/merge", pr.0)),
            Some(json!({"sha": head, "merge_method": method})),
        )
        .await
        .map(|_| ())
        .map_err(|e| match e.downcast::<ErrorResponse>() {
            Ok(refused) => anyhow!(refused.message()),
            Err(e) => e,
        })
    }

    async fn reply_in_thread(&self, _pr: Number, thread: &str, body: &str) -> Result<()> {
        let since = since_sent();
        retried_once(
            "POST /graphql addPullRequestReviewThreadReply",
            || async {
                self.graphql_once::<Value>(
                    "mutation($thread: ID!, $body: String!) {
                      addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $thread, body: $body}) { clientMutationId }
                    }",
                    json!({"thread": thread, "body": body}),
                )
                .await
                .map(|_| ())
            },
            || async {
                let data: ThreadData = self
                    .graphql(
                        "query($thread: ID!) {
                          node(id: $thread) {
                            ... on PullRequestReviewThread {
                              comments(last: 100) { nodes { author { __typename login } body createdAt } }
                            }
                          }
                        }",
                        json!({"thread": thread}),
                    )
                    .await?;
                let comments = data.node.comments.nodes;
                Ok(replied(comments, &self.linewright(), body, since).then_some(()))
            },
        )
        .await
    }

    async fn resolve_thread(&self, _pr: Number, thread: &str) -> Result<()> {
        let _: Value = self
            .graphql(
                "mutation($thread: ID!) { resolveReviewThread(input: {threadId: $thread}) { clientMutationId } }",
                json!({"thread": thread}),
            )
            .await?;
        Ok(())
    }

    async fn record_items(&self, numbers: &BTreeSet<Number>) -> Result<Vec<RecordedItem>> {
        let mut items = vec![];
        for &number in numbers {
            items.push(self.record_item(number).await?);
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {

    use chrono::TimeDelta;

    use super::*;

    fn review_required(requests: &Value) -> GqlPull {
        serde_json::from_value(json!({
            "number": 20, "title": "Do the ticket", "body": "",
            "createdAt": "2026-10-09T12:00:00Z", "updatedAt": "2026-10-09T12:00:00Z",
            "baseRefName": "main",
            "headRefName": "20-do-the-ticket", "headRefOid": "abc", "isDraft": false,
            "state": "OPEN", "mergedAt": null, "closedAt": null,
            "mergeable": "MERGEABLE", "mergeStateStatus": "BLOCKED", "canBeRebased": true,
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"__typename": "User", "login": "linewright"},
            "closingIssuesReferences": {"nodes": []},
            "comments": {"nodes": []},
            "reviews": {"nodes": []},
            "reviewRequests": {"nodes": requests},
            "reviewThreads": {"nodes": []},
            "statusCheckRollup": null,
            "labels": {"nodes": []},
            "assignees": {"nodes": []},
            "timelineItems": {"nodes": []},
        }))
        .unwrap()
    }

    #[test_log::test]
    fn a_pull_request_lists_its_assignees_and_who_assigned_and_unassigned_whom_when() {
        let mut pr = review_required(&json!([]));
        pr.assignees = serde_json::from_value(json!({"nodes": [{"login": "factory"}]})).unwrap();
        pr.timeline_items = serde_json::from_value(json!({"nodes": [
            {"__typename": "AssignedEvent", "createdAt": "2026-10-09T12:05:00Z", "actor": {"__typename": "User", "login": "maintainer"}, "assignee": {"__typename": "User", "login": "factory"}},
            {"__typename": "UnassignedEvent", "createdAt": "2026-10-09T12:10:00Z", "actor": {"__typename": "User", "login": "maintainer"}, "assignee": {"__typename": "User", "login": "factory"}},
            {"__typename": "AssignedEvent", "createdAt": "2026-10-09T12:15:00Z", "actor": {"__typename": "User", "login": "factory"}, "assignee": {"__typename": "User", "login": "factory"}},
            {"__typename": "AssignedEvent", "createdAt": "2026-10-09T12:16:00Z", "assignee": {"__typename": "Team"}},
        ]}))
        .unwrap();

        let pull = pr.into_pull(vec![], &BranchRules::default());

        assert_eq!(pull.assignees, [Login::new("factory")]);
        let at = |t: &str| t.parse::<Timestamp>().unwrap();
        assert_eq!(
            pull.assignments,
            [
                Assignment::Assigned {
                    assignee: Login::new("factory"),
                    actor: Login::new("maintainer"),
                    at: at("2026-10-09T12:05:00Z"),
                },
                Assignment::Unassigned {
                    assignee: Login::new("factory"),
                    actor: Login::new("maintainer"),
                    at: at("2026-10-09T12:10:00Z"),
                },
                Assignment::Assigned {
                    assignee: Login::new("factory"),
                    actor: Login::new("factory"),
                    at: at("2026-10-09T12:15:00Z"),
                },
            ]
        );
    }

    fn code_owner(reviewer: &Value) -> Value {
        json!({"asCodeOwner": true, "requestedReviewer": reviewer})
    }

    fn displayed(requires: &[Requirement]) -> Vec<String> {
        requires.iter().map(Requirement::to_string).collect()
    }

    #[test_log::test]
    fn a_team_requested_as_code_owner_requires_a_code_owners_review_without_naming_it() {
        let pr = review_required(&json!([code_owner(&json!({"__typename": "Team"}))]));

        assert_eq!(
            displayed(&pr.requirements(&BranchRules::default())),
            ["a code owner's review"]
        );
    }

    #[test_log::test]
    fn users_requested_as_code_owners_are_named() {
        let pr = review_required(&json!([
            code_owner(&json!({"__typename": "User", "login": "alice"})),
            code_owner(&json!({"__typename": "User", "login": "bob"})),
        ]));

        assert_eq!(
            displayed(&pr.requirements(&BranchRules::default())),
            ["a code owner's review (@alice or @bob)"]
        );
    }

    #[test_log::test]
    fn a_user_requested_as_code_owner_with_a_team_is_not_named_alone() {
        let pr = review_required(&json!([
            code_owner(&json!({"__typename": "User", "login": "alice"})),
            code_owner(&json!({"__typename": "Team"})),
        ]));

        assert_eq!(
            displayed(&pr.requirements(&BranchRules::default())),
            ["a code owner's review"]
        );
    }

    #[test_log::test]
    fn a_review_required_without_code_owners_requested_requires_an_approval() {
        let pr = review_required(&json!([{
            "asCodeOwner": false,
            "requestedReviewer": {"__typename": "User", "login": "alice"},
        }]));

        assert_eq!(
            pr.requirements(&BranchRules::default()),
            [Requirement::Approval]
        );
    }

    /// `fixtures/pull-changes-requested.json` is `PullFields` as GitHub
    /// returned it for <https://github.com/tibbe/linewright/pull/16>, on
    /// which its maintainer requested changes. The other decisions are
    /// GitHub's documented values
    /// (<https://docs.github.com/en/graphql/reference/enums#pullrequestreviewdecision>).
    #[test_log::test]
    fn a_pull_request_is_held_with_githubs_review_decision_and_none_where_github_gives_none() {
        let decision = |review_decision: Option<Value>| {
            let mut pr: Value =
                serde_json::from_str(include_str!("fixtures/pull-changes-requested.json")).unwrap();
            if let Some(review_decision) = review_decision {
                pr["reviewDecision"] = review_decision;
            }
            let pr: GqlPull = serde_json::from_value(pr).unwrap();
            pr.into_pull(vec![], &BranchRules::default())
                .review_decision
        };

        assert_eq!(decision(None), Some(ReviewDecision::ChangesRequested));
        assert_eq!(
            decision(Some(json!("REVIEW_REQUIRED"))),
            Some(ReviewDecision::ReviewRequired)
        );
        assert_eq!(
            decision(Some(json!("APPROVED"))),
            Some(ReviewDecision::Approved)
        );
        assert_eq!(decision(Some(json!(null))), None);
    }

    /// `fixtures/pull-check-run-concluded-in-progress.json` is `PullFields`
    /// as GitHub returned it for <https://github.com/tibbe/linewright/pull/543>,
    /// whose `css` check run finished with a conclusion but is still
    /// reported `IN_PROGRESS`.
    fn concluded_in_progress() -> Value {
        serde_json::from_str(include_str!(
            "fixtures/pull-check-run-concluded-in-progress.json"
        ))
        .unwrap()
    }

    /// `css`, the context `concluded_in_progress` reports `IN_PROGRESS`.
    fn css(pr: &mut Value) -> &mut Value {
        pr["statusCheckRollup"]["contexts"]["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|context| context["name"] == "css")
            .unwrap()
    }

    #[test_log::test]
    fn a_check_run_with_a_conclusion_is_finished_whatever_its_status() {
        let css_state = |status: &str, conclusion: Value| {
            let mut pr = concluded_in_progress();
            css(&mut pr)["status"] = json!(status);
            css(&mut pr)["conclusion"] = conclusion;
            let pr: GqlPull = serde_json::from_value(pr).unwrap();
            pr.into_pull(vec![], &BranchRules::default())
                .checks
                .into_iter()
                .find(|check| check.name == "css")
                .unwrap()
                .state
        };

        assert_eq!(
            css_state("IN_PROGRESS", json!("SUCCESS")),
            CheckState::Passed
        );
        assert_eq!(
            css_state("IN_PROGRESS", json!("FAILURE")),
            CheckState::Failed
        );
        assert_eq!(css_state("IN_PROGRESS", json!(null)), CheckState::Pending);
        assert_eq!(css_state("QUEUED", json!(null)), CheckState::Pending);
        assert_eq!(css_state("COMPLETED", json!("SUCCESS")), CheckState::Passed);
        assert_eq!(css_state("COMPLETED", json!("FAILURE")), CheckState::Failed);
    }

    #[test_log::test]
    fn a_required_check_run_that_concluded_successfully_in_progress_is_not_missing() {
        let pr: GqlPull = serde_json::from_value(concluded_in_progress()).unwrap();
        let rules = BranchRules {
            checks: vec!["css".into(), "ci".into()],
            ..BranchRules::default()
        };

        assert_eq!(pr.requirements(&rules), []);
    }

    fn no_review_decision(requests: &Value) -> GqlPull {
        let mut pr = review_required(requests);
        pr.review_decision = None;
        pr
    }

    fn code_owner_review_ruled() -> BranchRules {
        BranchRules {
            code_owner_review: true,
            ..BranchRules::default()
        }
    }

    #[test_log::test]
    fn a_code_owner_requested_under_rules_requiring_its_review_is_required_without_a_review_decision()
     {
        let pr = no_review_decision(&json!([code_owner(
            &json!({"__typename": "User", "login": "tibbe"})
        )]));

        assert_eq!(
            pr.requirements(&code_owner_review_ruled()),
            [Requirement::CodeOwnerReview {
                owners: vec!["tibbe".into()]
            }]
        );
    }

    #[test_log::test]
    fn a_team_requested_as_code_owner_under_rules_requiring_its_review_is_required_unnamed() {
        let pr = no_review_decision(&json!([code_owner(&json!({"__typename": "Team"}))]));

        assert_eq!(
            pr.requirements(&code_owner_review_ruled()),
            [Requirement::CodeOwnerReview { owners: vec![] }]
        );
    }

    #[test_log::test]
    fn a_code_owner_requested_without_a_review_decision_or_a_rule_requiring_it_is_not_required() {
        let pr = no_review_decision(&json!([code_owner(
            &json!({"__typename": "User", "login": "tibbe"})
        )]));

        assert_eq!(pr.requirements(&BranchRules::default()), []);
    }

    #[test_log::test]
    fn rules_requiring_a_code_owners_review_require_none_when_no_code_owner_is_requested() {
        let pr = no_review_decision(&json!([{
            "asCodeOwner": false,
            "requestedReviewer": {"__typename": "User", "login": "alice"},
        }]));

        assert_eq!(pr.requirements(&code_owner_review_ruled()), []);
    }

    #[test_log::test]
    fn a_branchs_rules_give_its_required_checks_and_whether_a_code_owners_review_is_required() {
        let linewright_main_rules_from_github: Vec<Value> = serde_json::from_value(json!([
            {"type": "deletion", "ruleset_source_type": "Repository",
             "ruleset_source": "tibbe/linewright", "ruleset_id": 1},
            {"type": "pull_request", "ruleset_source_type": "Repository",
             "ruleset_source": "tibbe/linewright", "ruleset_id": 1,
             "parameters": {
                "required_approving_review_count": 0,
                "dismiss_stale_reviews_on_push": false,
                "require_code_owner_review": true,
                "require_last_push_approval": false,
                "required_review_thread_resolution": false,
                "allowed_merge_methods": ["rebase"],
             }},
            {"type": "required_status_checks", "ruleset_source_type": "Repository",
             "ruleset_source": "tibbe/linewright", "ruleset_id": 1,
             "parameters": {
                "strict_required_status_checks_policy": false,
                "do_not_enforce_on_create": false,
                "required_status_checks": [{"context": "check", "integration_id": 15368}],
             }},
        ]))
        .unwrap();

        let rules = BranchRules::from_rules(&linewright_main_rules_from_github);

        assert_eq!(rules.checks, ["check"]);
        assert!(rules.code_owner_review);
    }

    /// `fixtures/issue-reopened.json` is `IssueFields` as GitHub returned it
    /// for <https://github.com/rust-lang/cargo/issues/11777>, closed by a merged
    /// pull request and reopened.
    #[test_log::test]
    fn an_issue_lists_its_reopenings_among_its_labelings() {
        let issue: GqlIssue =
            serde_json::from_str(include_str!("fixtures/issue-reopened.json")).unwrap();

        let issue = issue.into_issue(&Login::new("linewright"));

        let at = |t: &str| t.parse::<Timestamp>().unwrap();
        assert_eq!(
            issue.events,
            [
                IssueEvent::Labeled {
                    label: "A-documenting-cargo-itself".into(),
                    by: None,
                    at: at("2024-04-09T11:02:07Z"),
                },
                IssueEvent::Labeled {
                    label: "S-blocked-external".into(),
                    by: None,
                    at: at("2024-04-10T01:18:27Z"),
                },
                IssueEvent::Reopened {
                    at: at("2024-09-19T12:01:27Z"),
                },
            ]
        );
    }

    /// Each label applied among `events`, with who applied it.
    fn labelings(events: &[IssueEvent]) -> Vec<(&str, Option<&str>)> {
        events
            .iter()
            .filter_map(|event| {
                if let IssueEvent::Labeled { label, by, .. } = event {
                    Some((label.as_str(), by.as_ref().map(Login::as_str)))
                } else {
                    None
                }
            })
            .collect()
    }

    /// `fixtures/issue-handed-back.json` is `IssueFields` as GitHub returned
    /// it for <https://github.com/tibbe/linewright/issues/731>, which tibbe
    /// filed and labeled `needs-triage`, and Linewright, as `tibbe-bot`,
    /// triaged and handed back.
    #[test_log::test]
    fn an_issue_lists_who_applied_each_label() {
        let issue: GqlIssue =
            serde_json::from_str(include_str!("fixtures/issue-handed-back.json")).unwrap();

        let issue = issue.into_issue(&Login::new("tibbe-bot"));

        assert_eq!(
            labelings(&issue.events),
            [
                ("needs-triage", Some("tibbe")),
                ("enhancement", Some("tibbe-bot")),
                ("ready-for-human", Some("tibbe-bot")),
            ]
        );
    }

    #[test_log::test]
    fn an_issue_is_held_with_when_github_last_updated_it_and_each_pull_request_closing_it() {
        let issue: GqlIssue =
            serde_json::from_str(include_str!("fixtures/issue-reopened.json")).unwrap();

        let issue = issue.into_issue(&Login::new("linewright"));

        assert_eq!(issue.updated_at, at("2024-09-19T12:01:27Z"));
        assert_eq!(
            issue
                .closed_by
                .iter()
                .map(|pull| pull.updated_at)
                .collect::<Vec<_>>(),
            [at("2024-10-30T07:11:13Z")]
        );
    }

    /// `fixtures/pull-changes-requested.json`, as in
    /// [`a_pull_request_is_held_with_githubs_review_decision_and_none_where_github_gives_none`].
    #[test_log::test]
    fn a_pull_request_is_held_with_when_github_last_updated_it() {
        let pr: GqlPull =
            serde_json::from_str(include_str!("fixtures/pull-changes-requested.json")).unwrap();

        let pull = pr.into_pull(vec![], &BranchRules::default());

        assert_eq!(pull.updated_at, at("2026-10-17T14:04:59Z"));
    }

    /// `fixtures/pull-changes-requested.json`, as in
    /// [`a_pull_request_is_held_with_githubs_review_decision_and_none_where_github_gives_none`].
    #[test_log::test]
    fn a_pull_request_is_held_with_the_branch_it_merges_into() {
        let pr: GqlPull =
            serde_json::from_str(include_str!("fixtures/pull-changes-requested.json")).unwrap();

        let pull = pr.into_pull(vec![], &BranchRules::default());

        assert_eq!(pull.base_ref, "main");
    }

    /// `fixtures/merged-pull.json` is `MergedFields` as GitHub returned it
    /// for <https://github.com/tibbe/linewright/pull/477>, a spec change
    /// Linewright designed after its merge.
    #[test_log::test]
    fn a_merged_pull_request_is_held_with_when_github_last_updated_it() {
        let pr: GqlMerged =
            serde_json::from_str(include_str!("fixtures/merged-pull.json")).unwrap();

        let merged = pr.into_merged(at("2026-10-21T14:56:28Z"), vec![]);

        assert_eq!(merged.updated_at, at("2026-10-21T15:05:15Z"));
    }

    /// `fixtures/merged-pull.json`, as above: merged at 14:56:28, and
    /// labeled designed, which updated it, at 15:05:15.
    #[test_log::test]
    fn a_spec_change_merged_before_the_lookback_window_is_held_while_it_was_designed_within_the_done_window()
     {
        let pr: GqlMerged =
            serde_json::from_str(include_str!("fixtures/merged-pull.json")).unwrap();
        let now = at("2026-10-21T15:10:00Z");
        let lookback = TimeDelta::minutes(10);

        let designed_within = Windows::at(now, lookback, TimeDelta::minutes(10));
        let designed_before = Windows::at(now, lookback, TimeDelta::minutes(4));

        assert_eq!(
            pr.held_merged_at("main", designed_within),
            Some(at("2026-10-21T14:56:28Z"))
        );
        assert_eq!(pr.held_merged_at("main", designed_before), None);
        assert_eq!(pr.held_merged_at("release", designed_within), None);
    }

    #[test_log::test]
    fn an_issue_lists_its_labelings_unlabelings_and_assignments_with_who_made_them() {
        let issue: GqlIssue = serde_json::from_value(json!({
            "number": 12, "title": "Store gadgets", "body": "",
            "createdAt": "2026-10-09T12:00:00Z", "updatedAt": "2026-10-09T12:00:00Z",
            "state": "OPEN",
            "author": {"__typename": "User", "login": "maintainer"},
            "labels": {"nodes": []},
            "assignees": {"nodes": []},
            "comments": {"nodes": []},
            "timelineItems": {"nodes": [
                {"__typename": "LabeledEvent", "createdAt": "2026-10-09T12:01:00Z", "label": {"name": "ready-for-agent"}},
                {"__typename": "AssignedEvent", "createdAt": "2026-10-09T12:02:00Z", "actor": {"__typename": "User", "login": "maintainer"}, "assignee": {"__typename": "User", "login": "linewright"}},
                {"__typename": "AssignedEvent", "createdAt": "2026-10-09T12:02:30Z", "actor": {"__typename": "User", "login": "linewright"}, "assignee": {"__typename": "User", "login": "reporter"}},
                {"__typename": "UnassignedEvent", "createdAt": "2026-10-09T12:03:00Z", "actor": {"__typename": "User", "login": "maintainer"}, "assignee": {"__typename": "User", "login": "linewright"}},
                {"__typename": "UnlabeledEvent", "createdAt": "2026-10-09T12:04:00Z", "label": {"name": "ready-for-agent"}},
            ]},
        }))
        .unwrap();

        let issue = issue.into_issue(&Login::new("linewright"));

        let at = |t: &str| t.parse::<Timestamp>().unwrap();
        assert_eq!(
            issue.events,
            [
                IssueEvent::Labeled {
                    label: "ready-for-agent".into(),
                    by: None,
                    at: at("2026-10-09T12:01:00Z"),
                },
                IssueEvent::Assignment(Assignment::Assigned {
                    assignee: Login::new("linewright"),
                    actor: Login::new("maintainer"),
                    at: at("2026-10-09T12:02:00Z"),
                }),
                IssueEvent::Assignment(Assignment::Assigned {
                    assignee: Login::new("reporter"),
                    actor: Login::new("linewright"),
                    at: at("2026-10-09T12:02:30Z"),
                }),
                IssueEvent::Assignment(Assignment::Unassigned {
                    assignee: Login::new("linewright"),
                    actor: Login::new("maintainer"),
                    at: at("2026-10-09T12:03:00Z"),
                }),
                IssueEvent::Unlabeled {
                    label: "ready-for-agent".into(),
                    at: at("2026-10-09T12:04:00Z"),
                },
            ]
        );
    }

    #[test_log::test]
    fn a_merged_pull_request_lists_its_assignees_and_when_each_was_assigned_unassigned_and_labeled()
    {
        let pr: GqlMerged = serde_json::from_value(json!({
            "number": 5, "title": "Gadgets", "body": "",
            "mergedAt": "2026-10-09T12:00:00Z", "updatedAt": "2026-10-09T13:00:00Z",
            "baseRefName": "main",
            "author": {"__typename": "User", "login": "maintainer"},
            "closingIssuesReferences": {"nodes": []},
            "labels": {"nodes": [{"name": "linewright"}]},
            "assignees": {"nodes": [{"login": "linewright"}]},
            "files": {"pageInfo": {"hasNextPage": false}, "nodes": [{"path": "docs/spec.md"}]},
            "timelineItems": {"nodes": [
                {"__typename": "LabeledEvent", "createdAt": "2026-10-09T11:00:00Z",
                 "label": {"name": "linewright"}},
                {"__typename": "AssignedEvent", "createdAt": "2026-10-09T12:01:00Z",
                 "actor": {"__typename": "User", "login": "linewright"},
                 "assignee": {"__typename": "User", "login": "linewright"}},
                {"__typename": "UnassignedEvent", "createdAt": "2026-10-09T12:02:00Z",
                 "actor": {"__typename": "User", "login": "maintainer"},
                 "assignee": {"__typename": "User", "login": "linewright"}},
                {"__typename": "AssignedEvent", "createdAt": "2026-10-09T12:03:00Z",
                 "actor": {"__typename": "User", "login": "maintainer"},
                 "assignee": {"__typename": "User", "login": "linewright"}},
            ]},
        }))
        .unwrap();
        let at = |time: &str| time.parse::<Timestamp>().unwrap();

        let merged = pr.into_merged(at("2026-10-09T12:00:00Z"), vec!["docs/spec.md".into()]);

        assert_eq!(merged.assignees, [Login::new("linewright")]);
        assert_eq!(
            merged.events,
            [
                IssueEvent::Labeled {
                    label: "linewright".into(),
                    by: None,
                    at: at("2026-10-09T11:00:00Z"),
                },
                IssueEvent::Assignment(Assignment::Assigned {
                    assignee: Login::new("linewright"),
                    actor: Login::new("linewright"),
                    at: at("2026-10-09T12:01:00Z"),
                }),
                IssueEvent::Assignment(Assignment::Unassigned {
                    assignee: Login::new("linewright"),
                    actor: Login::new("maintainer"),
                    at: at("2026-10-09T12:02:00Z"),
                }),
                IssueEvent::Assignment(Assignment::Assigned {
                    assignee: Login::new("linewright"),
                    actor: Login::new("maintainer"),
                    at: at("2026-10-09T12:03:00Z"),
                }),
            ]
        );
    }

    fn ticket_with_parent(parent: &Value) -> Issue {
        ticket(parent, &json!([]))
    }

    fn ticket(parent: &Value, closing: &Value) -> Issue {
        let issue: GqlIssue = serde_json::from_value(json!({
            "number": 12, "title": "Store gadgets", "body": "Gadgets persist.",
            "createdAt": "2026-10-09T12:00:00Z", "updatedAt": "2026-10-09T12:00:00Z",
            "state": "OPEN",
            "author": {"__typename": "User", "login": "linewright"},
            "labels": {"nodes": [{"name": "ready-for-agent"}]},
            "assignees": {"nodes": []},
            "comments": {"nodes": []},
            "subIssues": {"nodes": []},
            "parent": parent,
            "blockedBy": {"nodes": []},
            "closedByPullRequestsReferences": {"nodes": closing},
            "timelineItems": {"nodes": []},
        }))
        .unwrap();
        issue.into_issue(&Login::new("linewright"))
    }

    /// `fixtures/sub-issues.json` is the `subIssues` connection GitHub
    /// returned for <https://github.com/tibbe/linewright/issues/373>, the
    /// tracking issue of tickets its account opened, all of them closed.
    #[test_log::test]
    fn an_issues_sub_issues_are_read_with_who_opened_them_and_when_they_closed() {
        let nodes: Nodes<SubIssueNode> =
            serde_json::from_str(include_str!("fixtures/sub-issues.json")).unwrap();

        assert_eq!(
            sub_issues(Some(nodes), None),
            [
                (374, "2026-10-20T07:25:56Z"),
                (375, "2026-10-20T07:37:18Z"),
                (376, "2026-10-20T07:59:39Z"),
                (377, "2026-10-20T09:35:32Z"),
                (378, "2026-10-20T12:02:46Z"),
            ]
            .map(|(number, closed_at)| SubIssue {
                number: Number(number),
                author: Login::new("tibbe-bot"),
                added_by: None,
                state: SubIssueState::Closed {
                    at: Some(at(closed_at)),
                },
            })
        );
    }

    /// `fixtures/issue-sub-issues-added.json` is `IssueFields` as GitHub
    /// returned it for <https://github.com/tibbe/linewright/issues/900>,
    /// whose sub-issues Linewright, as `tibbe-bot`, opened and added.
    #[test_log::test]
    fn an_issues_sub_issues_are_read_with_who_added_them() {
        let issue: GqlIssue =
            serde_json::from_str(include_str!("fixtures/issue-sub-issues-added.json")).unwrap();

        let issue = issue.into_issue(&Login::new("tibbe-bot"));

        let added: Vec<(u64, Option<&str>)> = issue
            .sub_issues
            .iter()
            .map(|s| (s.number.0, s.added_by.as_ref().map(Login::as_str)))
            .collect();
        assert_eq!(
            added,
            (901..=907)
                .map(|number| (number, Some("tibbe-bot")))
                .collect::<Vec<_>>()
        );
    }

    /// `fixtures/closed-issue-sub-issues-added.json` is `ClosedIssueFields`
    /// as GitHub returned it for
    /// <https://github.com/tibbe/linewright/issues/308>, whose sub-issues
    /// tibbe opened and added.
    #[test_log::test]
    fn a_closed_issues_sub_issues_are_read_with_who_added_them() {
        let issue: GqlClosedIssue =
            serde_json::from_str(include_str!("fixtures/closed-issue-sub-issues-added.json"))
                .unwrap();

        let closed = issue.into_closed(&Login::new("tibbe-bot")).unwrap();

        let ClosedKind::Issue { facts, .. } = closed.kind else {
            panic!("#308 is an issue");
        };
        let added: Vec<(u64, Option<&str>)> = facts
            .sub_issues
            .iter()
            .map(|s| (s.number.0, s.added_by.as_ref().map(Login::as_str)))
            .collect();
        assert_eq!(
            added,
            (300..=307)
                .map(|number| (number, Some("tibbe")))
                .collect::<Vec<_>>()
        );
    }

    #[test_log::test]
    fn a_ticket_is_closed_by_its_pull_requests_open_closed_or_merged_whoever_opened_them() {
        let issue = ticket(
            &Value::Null,
            &json!([
                {"number": 40, "state": "CLOSED", "isDraft": false, "mergedAt": null,
                 "closedAt": "2026-10-09T13:00:00Z",
                 "updatedAt": "2026-10-09T15:00:00Z",
                 "author": {"__typename": "User", "login": "alice"}},
                {"number": 41, "state": "MERGED", "isDraft": false,
                 "mergedAt": "2026-10-09T14:00:00Z", "closedAt": "2026-10-09T14:00:00Z",
                 "updatedAt": "2026-10-09T15:00:00Z",
                 "author": {"__typename": "User", "login": "alice"}},
                {"number": 43, "state": "OPEN", "isDraft": true, "mergedAt": null, "closedAt": null,
                 "updatedAt": "2026-10-09T15:00:00Z",
                 "author": {"__typename": "User", "login": "alice"}},
                {"number": 44, "state": "OPEN", "isDraft": false, "mergedAt": null, "closedAt": null,
                 "updatedAt": "2026-10-09T15:00:00Z",
                 "author": {"__typename": "Bot", "login": "renovate"}},
                {"number": 45, "state": "OPEN", "isDraft": false, "mergedAt": null, "closedAt": null,
                 "updatedAt": "2026-10-09T15:00:00Z",
                 "author": {"__typename": "User", "login": "linewright"}},
            ]),
        );
        let at = |time: &str| format!("2026-10-09T{time}:00Z").parse().unwrap();

        assert_eq!(
            issue.closed_by,
            [
                (40, "alice", PullState::Closed { at: at("13:00") }),
                (41, "alice", PullState::Merged { at: at("14:00") }),
                (43, "alice", PullState::Open { draft: true }),
                (44, "renovate[bot]", PullState::Open { draft: false }),
                (45, "linewright", PullState::Open { draft: false }),
            ]
            .map(|(number, author, state)| ClosingPull {
                number: Number(number),
                author: Login::new(author),
                state,
                updated_at: at("15:00"),
                base_ref: String::new(),
            })
        );
    }

    /// A pull request that closes a held issue is read fresh, whether a
    /// search has found it yet or not: the open ones, and Linewright's
    /// closed or merged.
    #[test_log::test]
    fn the_pull_requests_closing_a_held_issue_are_read_open_or_linewrights() {
        let pull = |number: u64, state: &str, at: Option<&str>, author: &str| {
            json!({"number": number, "state": state, "isDraft": false,
                   "mergedAt": if state == "MERGED" { at } else { None },
                   "closedAt": at, "updatedAt": "2026-10-09T15:00:00Z",
                   "author": {"__typename": "User", "login": author}})
        };
        let issue = ticket(
            &Value::Null,
            &json!([
                pull(40, "CLOSED", Some("2026-10-09T13:00:00Z"), "alice"),
                pull(41, "MERGED", Some("2026-10-09T14:00:00Z"), "linewright"),
                pull(43, "OPEN", None, "alice"),
                pull(45, "OPEN", None, "linewright"),
            ]),
        );

        let held = open_issues_held(vec![issue], &Login::new("linewright"));

        assert_eq!(held.closing_pulls, numbers(&[41, 43, 45]));
        assert_eq!(held.closed_by_linewright, numbers(&[41]));
    }

    #[test_log::test]
    fn a_ticket_whose_parent_is_a_tracking_issue_knows_the_spec_change() {
        let issue = ticket_with_parent(&json!({
            "number": 11, "title": "Spec change: Gadgets",
            "body": "Tickets for the spec change in #10.\n\n- #12 Store gadgets\n",
            "author": {"__typename": "User", "login": "linewright"},
        }));

        assert_eq!(
            issue.parent,
            Some(Parent {
                number: Number(11),
                spec_change: Some(Number(10)),
            })
        );
    }

    #[test_log::test]
    fn a_ticket_whose_parent_a_person_opened_knows_no_spec_change() {
        let issue = ticket_with_parent(&json!({
            "number": 11, "title": "Spec change: Gadgets",
            "body": "Tickets for the spec change in #10.",
            "author": {"__typename": "User", "login": "maintainer"},
        }));

        assert_eq!(
            issue.parent,
            Some(Parent {
                number: Number(11),
                spec_change: None,
            })
        );
    }

    #[test_log::test]
    fn a_ticket_without_a_parent_has_none() {
        assert_eq!(ticket_with_parent(&Value::Null).parent, None);
    }

    fn referenced_by(number: u64, author: &str, title: &str, body: &str, cross: bool) -> Value {
        json!({
            "isCrossRepository": cross,
            "source": {
                "__typename": "Issue", "number": number, "title": title, "body": body,
                "author": {"__typename": "Bot", "login": author},
            },
        })
    }

    #[test_log::test]
    fn a_spec_changes_issues_are_the_issues_linewright_filed_for_it_here() {
        let tracking = |number| {
            referenced_by(
                number,
                "linewright",
                "Spec change: Add invoices",
                "Tickets for the spec change in #5.\n\n- #9 Store invoices\n",
                false,
            )
        };
        let references: Nodes<CrossReference> = serde_json::from_value(json!({"nodes": [
            tracking(8),
            {"isCrossRepository": false, "source": {"__typename": "PullRequest"}},
            referenced_by(14, "maintainer", "Spec change: Add invoices",
                "Tickets for the spec change in #5.", false),
            referenced_by(15, "linewright", "Spec change: Add invoices",
                "Tickets for the spec change in #6.", false),
            referenced_by(16, "linewright", "Spec change: Add invoices",
                "Tickets for the spec change in #5.", true),
            tracking(12),
        ]}))
        .unwrap();

        assert_eq!(
            spec_change_issues_among(references, Number(5), &Login::new("linewright[bot]")),
            [Number(8), Number(12)]
        );
    }

    fn at(time: &str) -> Timestamp {
        time.parse().unwrap()
    }

    /// Both windows opened at `time`.
    fn opened(time: &str) -> Windows {
        Windows::at(at(time), TimeDelta::zero(), TimeDelta::zero())
    }

    /// `fixtures/closed-issues.json` is the page of `ClosedIssueFields`
    /// GitHub returned for the issues of <https://github.com/tibbe/linewright>
    /// its account opened, closed and updated since 2026-10-20T13:40:00Z:
    /// the tracking issue #387, which Linewright closed once its tickets
    /// were, and its tickets #388 and #389, each closed by its merged pull
    /// request.
    #[test_log::test]
    fn issues_closed_since_the_window_opened_are_held_with_the_pull_request_whose_merge_closed_them()
     {
        let page: Page<GqlClosedIssue> =
            serde_json::from_str(include_str!("fixtures/closed-issues.json")).unwrap();

        let read = page
            .nodes
            .into_iter()
            .filter_map(|issue| issue.into_closed(&Login::new("tibbe-bot")));
        let closed = closed_within(read, opened("2026-10-20T13:50:00Z"));

        let ticket = |number, closed_at| SubIssue {
            number: Number(number),
            author: Login::new("tibbe-bot"),
            added_by: None,
            state: SubIssueState::Closed {
                at: Some(at(closed_at)),
            },
        };
        assert_eq!(
            closed,
            [
                ClosedItem {
                    number: Number(387),
                    title: "Spec change: Say what removing `linewright:designed` does to an \
                            issue's tickets"
                        .into(),
                    closed_at: at("2026-10-20T14:10:39Z"),
                    updated_at: at("2026-10-20T14:10:39Z"),
                    kind: ClosedKind::Issue {
                        parent: None,
                        facts: ManagedFacts {
                            filed: Some(Filed::Tracking {
                                spec_change: Number(386)
                            }),
                            sub_issues: vec![
                                ticket(388, "2026-10-20T13:49:33Z"),
                                ticket(389, "2026-10-20T14:10:32Z"),
                            ],
                            labels: vec![],
                            work: WorkFacts {
                                author: Some(Login::new("tibbe-bot")),
                                ..WorkFacts::default()
                            }
                        },
                        merged_by: None,
                        reason: Some(IssueCloseReason::Completed)
                    },
                },
                ClosedItem {
                    number: Number(389),
                    title: "Close a discarded cut's pull requests, and keep sub-issues a person \
                            added"
                        .into(),
                    closed_at: at("2026-10-20T14:10:32Z"),
                    updated_at: at("2026-10-20T14:10:32Z"),
                    kind: ClosedKind::Issue {
                        parent: Some(Parent {
                            number: Number(387),
                            spec_change: Some(Number(386)),
                        }),
                        facts: ManagedFacts {
                            filed: None,
                            sub_issues: vec![],
                            labels: vec!["ready-for-agent".into(), "linewright:designed".into()],
                            work: WorkFacts {
                                author: Some(Login::new("tibbe-bot")),
                                assignees: vec![Login::new("tibbe-bot")],
                                events: vec![IssueEvent::Assignment(Assignment::Assigned {
                                    assignee: Login::new("tibbe-bot"),
                                    actor: Login::new("tibbe-bot"),
                                    at: at("2026-10-20T13:49:39Z"),
                                })],
                                closed_by: vec![ClosingPull {
                                    number: Number(391),
                                    author: Login::new("tibbe-bot"),
                                    state: PullState::Merged {
                                        at: at("2026-10-20T14:10:30Z"),
                                    },
                                    updated_at: at("2026-10-20T14:10:34Z"),
                                    base_ref: String::new(),
                                }],
                            }
                        },
                        merged_by: Some(Number(391)),
                        reason: Some(IssueCloseReason::Completed)
                    },
                },
            ]
        );
    }

    /// `fixtures/pull-changes-requested.json`, as in
    /// [`a_pull_request_is_held_with_githubs_review_decision_and_none_where_github_gives_none`]:
    /// a review is requested of its maintainer `tibbe` by name. A team is
    /// GitHub's documented `Team`
    /// (<https://docs.github.com/en/graphql/reference/unions#requestedreviewer>).
    #[test_log::test]
    fn a_pull_request_is_held_with_the_users_a_review_is_requested_of_leaving_out_teams() {
        let mut pr: Value =
            serde_json::from_str(include_str!("fixtures/pull-changes-requested.json")).unwrap();
        let held = |recorded: &Value| {
            let read: GqlPull = serde_json::from_value(recorded.clone()).unwrap();
            read.into_pull(vec![], &BranchRules::default())
                .review_requests
        };

        assert_eq!(held(&pr), [Login::new("tibbe")]);
        pr["reviewRequests"]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"asCodeOwner": false, "requestedReviewer": {"__typename": "Team"}}));
        assert_eq!(held(&pr), [Login::new("tibbe")]);
    }

    /// `fixtures/merged-pull-review-requested.json` is `MergedFields` as
    /// GitHub returned it for <https://github.com/tibbe/linewright/pull/862>,
    /// merged with a review still requested of its maintainer `tibbe`, who
    /// reviewed it, as its account `tibbe-bot` did twice. A team is as
    /// above.
    #[test_log::test]
    fn a_merged_pull_request_is_held_with_the_users_a_review_is_requested_of_and_who_reviewed_it() {
        let mut pr: Value =
            serde_json::from_str(include_str!("fixtures/merged-pull-review-requested.json"))
                .unwrap();
        pr["reviewRequests"]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"asCodeOwner": true, "requestedReviewer": {"__typename": "Team"}}));
        let pr: GqlMerged = serde_json::from_value(pr).unwrap();

        let merged = pr.into_merged(at("2026-10-30T20:52:00Z"), vec![]);

        assert_eq!(merged.review_requests, [Login::new("tibbe")]);
        assert_eq!(
            merged.reviewers,
            [Login::new("tibbe"), Login::new("tibbe-bot")]
        );
    }

    /// `fixtures/merged-pull.json`, as in
    /// [`a_merged_pull_request_is_held_with_when_github_last_updated_it`],
    /// recorded before Linewright read the reviews requested and the
    /// reviews.
    #[test_log::test]
    fn a_merged_pull_request_recorded_before_reviews_were_read_is_held_with_none() {
        let pr: GqlMerged =
            serde_json::from_str(include_str!("fixtures/merged-pull.json")).unwrap();

        let merged = pr.into_merged(at("2026-10-21T14:56:28Z"), vec![]);

        assert_eq!(merged.review_requests, []);
        assert_eq!(merged.reviewers, []);
    }

    /// `fixtures/closed-pull-assigned.json` is `ClosedPullFields` as GitHub
    /// returned it for <https://github.com/tibbe/linewright/pull/1044>,
    /// which its account `tibbe-bot` opened, assigned to itself and closed
    /// without merging, with no review requested or given.
    #[test_log::test]
    fn a_pull_request_closed_unmerged_is_held_with_its_assignees_reviews_requested_and_reviewers() {
        let mut pr: Value =
            serde_json::from_str(include_str!("fixtures/closed-pull-assigned.json")).unwrap();
        pr["reviewRequests"]["nodes"] = json!([
            {"asCodeOwner": false, "requestedReviewer": {"__typename": "User", "login": "tibbe"}},
            {"asCodeOwner": false, "requestedReviewer": {"__typename": "Team"}},
        ]);
        let pr: GqlClosedPull = serde_json::from_value(pr).unwrap();

        let closed = pr.into_closed(&Login::new("tibbe-bot")).unwrap();

        assert_eq!(
            closed.kind,
            ClosedKind::Pull {
                closes: vec![Number(1043)],
                author: Some(Login::new("tibbe-bot")),
                assignees: vec![Login::new("tibbe-bot")],
                review_requests: vec![Login::new("tibbe")],
                reviewers: vec![],
            }
        );
    }

    /// `fixtures/openings.json` is GitHub's response to `numbered_query`
    /// with `OpeningFields` for #1139, #1137, #1136 and #1122 of
    /// <https://github.com/tibbe/linewright>: #1139, a ticket its account
    /// `tibbe-bot` cut from #1137, the issue it opened for spec change
    /// #1136, which its maintainer `tibbe` opened; and #1122, the account's
    /// pull request closing #1117.
    #[test_log::test]
    fn an_opening_read_is_who_opened_the_item_and_what_for() {
        let response = serde_json::from_str(include_str!("fixtures/openings.json")).unwrap();
        let linewright = Login::new("tibbe-bot");

        let read: Vec<Opening> = found_of_numbered::<GqlOpening>(response)
            .unwrap()
            .into_iter()
            .map(|item| item.into_opening(&linewright))
            .collect();

        let opening = |number, author: &str, opened_for: &[u64]| Opening {
            number: Number(number),
            author: Login::new(author),
            opened_for: opened_for.iter().copied().map(Number).collect(),
        };
        assert_eq!(
            read,
            [
                opening(1122, "tibbe-bot", &[1117]),
                opening(1136, "tibbe", &[]),
                opening(1137, "tibbe-bot", &[1136]),
                opening(1139, "tibbe-bot", &[1137]),
            ]
        );
    }

    /// `fixtures/closed-issues-numbered.json` is GitHub's response to
    /// `numbered_query` for #388, #719 and #99999 of
    /// <https://github.com/tibbe/linewright>: #388, a ticket its account
    /// opened, closed by the merge of #390; #719, an issue a person opened,
    /// unlabeled and assigned only to them; and #99999, which does not exist.
    #[test_log::test]
    fn closed_issues_read_by_number_are_those_the_snapshot_would_hold_were_they_open() {
        let response =
            serde_json::from_str(include_str!("fixtures/closed-issues-numbered.json")).unwrap();

        let closed = managed_closed(
            found_of_numbered(response).unwrap(),
            &Login::new("tibbe-bot"),
        );

        let read: Vec<(Number, Timestamp, Option<Number>)> = closed
            .iter()
            .map(|item| {
                let ClosedKind::Issue { merged_by, .. } = item.kind else {
                    panic!("an issue");
                };
                (item.number, item.closed_at, merged_by)
            })
            .collect();
        assert_eq!(
            read,
            [(Number(388), at("2026-10-20T13:49:33Z"), Some(Number(390)))]
        );
    }

    /// The issues of a recorded page of `ManagedFactsFields`.
    fn facts_page(recorded: &str) -> Vec<GqlFactsIssue> {
        serde_json::from_str::<Page<GqlFactsIssue>>(recorded)
            .unwrap()
            .nodes
    }

    fn numbers(numbers: &[u64]) -> BTreeSet<Number> {
        numbers.iter().copied().map(Number).collect()
    }

    /// `fixtures/managed-facts-closed.json` is the page of
    /// `ManagedFactsFields` GitHub returned on 2026-10-29 at 22:28 for the
    /// closed issues of <https://github.com/tibbe/linewright> updated since
    /// 21:00 that day. Of these, #723, #732 and #759 are a person's, labeled
    /// by that person alone, so Linewright never worked them, and #746 closed
    /// before 21:30. #731 is a person's issue that Linewright, as
    /// `tibbe-bot`, claimed, labeled `ready-for-human` in a hand-back and let
    /// go of.
    #[test_log::test]
    fn the_issues_listed_closed_are_those_linewright_may_manage_that_closed_since_the_windows_opened()
     {
        let listed = closed_issues_among(
            facts_page(include_str!("fixtures/managed-facts-closed.json")),
            at("2026-10-29T21:30:00Z"),
            &Login::new("tibbe-bot"),
        );

        assert_eq!(
            listed.managed,
            numbers(&[731, 745, 775, 781, 786, 789, 791])
        );
    }

    /// `fixtures/managed-facts-spec-change-closed.json` is the
    /// `ManagedFactsFields` GitHub returned on 2026-11-01 for #485 of
    /// <https://github.com/tibbe/linewright>, the tracking issue Linewright
    /// filed for the spec change #484 before a spec change had an issue of
    /// its own, closed on 2026-10-22 at 07:09. It is among the spec changes'
    /// issues while the windows open before it closed, which a spec change's
    /// issue, opened after the spec change merged, does while the spec change
    /// is in the lookback window.
    #[test_log::test]
    fn a_spec_changes_issue_listed_closed_is_held_while_the_windows_open_before_its_closing() {
        let linewright = Login::new("tibbe-bot");
        let issue = || {
            vec![
                serde_json::from_str(include_str!(
                    "fixtures/managed-facts-spec-change-closed.json"
                ))
                .unwrap(),
            ]
        };

        let before = closed_issues_among(issue(), at("2026-10-22T00:00:00Z"), &linewright);
        let after = closed_issues_among(issue(), at("2026-10-23T00:00:00Z"), &linewright);

        assert_eq!(
            before.spec_change_issues,
            [SpecChangeIssue {
                number: Number(485),
                spec_change: Number(484),
            }]
        );
        assert_eq!(after, ClosedIssues::default());
    }

    /// The address of a server answering GraphQL at `/graphql` with what
    /// `answer` gives for each request's body.
    async fn serving_graphql<A, F>(answer: A) -> String
    where
        A: Fn(String) -> F + Clone + Send + Sync + 'static,
        F: Future<Output = String> + Send + 'static,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/graphql",
            axum::routing::post(move |request: String| answer(request)),
        );
        tokio::spawn(async move { axum::serve(listener, app).await });
        format!("http://{address}")
    }

    /// The address of a server answering reads of branches by name
    /// ([`named_refs_query`]) as GitHub answered one for
    /// <https://github.com/tibbe/linewright> on 2026-11-17,
    /// `fixtures/refs-named.json`: a branch named `*-gone` as its branch
    /// `1314-no-such-branch`, which it does not have, and any other as its
    /// branch `301-cutting-tickets`, which `tibbe` committed the head of with
    /// no pull request open, under the name asked. Each query asked is sent
    /// to `asked`.
    async fn serving_refs(asked: tokio::sync::mpsc::UnboundedSender<String>) -> String {
        serving_graphql(move |request: String| {
            let asked = asked.clone();
            async move {
                let recorded: Value =
                    serde_json::from_str(include_str!("fixtures/refs-named.json")).unwrap();
                let request: Value = serde_json::from_str(&request).unwrap();
                asked
                    .send(request["query"].as_str().unwrap().into())
                    .unwrap();
                let refs: serde_json::Map<String, Value> = request["variables"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .filter(|(variable, _)| !["owner", "name"].contains(&variable.as_str()))
                    .map(|(alias, qualified)| {
                        let name = qualified.as_str().unwrap().strip_prefix("refs/heads/");
                        let name = name.unwrap();
                        let mut answer = if name.ends_with("-gone") {
                            recorded["data"]["repository"]["b1"].clone()
                        } else {
                            recorded["data"]["repository"]["b0"].clone()
                        };
                        if !answer.is_null() {
                            answer["name"] = json!(name);
                        }
                        (alias.clone(), answer)
                    })
                    .collect();
                json!({"data": {"repository": refs}}).to_string()
            }
        })
        .await
    }

    fn github_serving_refs(api: String, linewright: &str) -> GitHub {
        GitHub {
            api,
            viewer: Viewer {
                login: linewright.into(),
            },
            ..github("token")
        }
    }

    /// `docs/specs/sources_of_work.md`, Polling: what a poll reads grows with
    /// the branches Linewright pushed, not with the repository's others.
    #[test_log::test(tokio::test)]
    async fn ticket_branches_are_read_by_name_a_hundred_to_a_request_without_listing_every_branch()
    {
        let (ask, mut asked) = tokio::sync::mpsc::unbounded_channel();
        let github = github_serving_refs(serving_refs(ask).await, "tibbe");
        let pushed: BTreeSet<TicketBranchName> = (1..=247)
            .map(|n| TicketBranchName::parse(&format!("{n}-pushed")).unwrap())
            .collect();
        let gone: BTreeSet<TicketBranchName> = (300..303)
            .map(|n| TicketBranchName::parse(&format!("{n}-gone")).unwrap())
            .collect();
        let names = pushed.iter().chain(&gone).cloned().collect();
        let held = pushed.iter().map(TicketBranchName::issue).collect();

        let (branches, read_gone) = github.ticket_branches(&names, &held).await.unwrap();

        let mut queries = vec![];
        while let Ok(query) = asked.try_recv() {
            queries.push(query);
        }
        assert_eq!(queries.len(), 3);
        assert!(queries.iter().all(|query| !query.contains("refs(")));
        let read: BTreeSet<TicketBranchName> =
            branches.iter().map(|branch| branch.name.clone()).collect();
        assert_eq!(read, pushed);
        assert!(branches.iter().all(|branch| !branch.open_pull_from_branch));
        assert_eq!(read_gone, gone);
    }

    #[test_log::test(tokio::test)]
    async fn a_branch_whose_head_linewright_did_not_commit_is_neither_its_ticket_branch_nor_gone() {
        let (ask, _asked) = tokio::sync::mpsc::unbounded_channel();
        let github = github_serving_refs(serving_refs(ask).await, "tibbe-bot");
        let names = [TicketBranchName::parse("301-cutting-tickets").unwrap()].into();

        let (branches, gone) = github
            .ticket_branches(&names, &numbers(&[301]))
            .await
            .unwrap();

        assert_eq!(branches, []);
        assert_eq!(gone, BTreeSet::new());
    }

    /// `fixtures/issue-sub-issues-added.json`: #900 as `IssueFields` reads
    /// it, Linewright's tracking issue for the spec change #899.
    fn tracking_issue_json() -> Value {
        serde_json::from_str(include_str!("fixtures/issue-sub-issues-added.json")).unwrap()
    }

    /// An issue as `IssueFields` reads it, made from [`tracking_issue_json`]:
    /// opened by `author`, labeled each of `labels` by who it names,
    /// assigned to `assignees` and commented on by `commenters`.
    fn issue_json(
        number: u64,
        author: &str,
        labels: &[(&str, &str)],
        assignees: &[&str],
        commenters: &[&str],
    ) -> Value {
        let mut issue = tracking_issue_json();
        issue["number"] = json!(number);
        issue["title"] = json!(format!("Issue {number}"));
        issue["body"] = json!("An issue.");
        issue["subIssues"]["nodes"] = json!([]);
        issue["subIssueAdds"]["nodes"] = json!([]);
        worked_json(&mut issue, author, labels, assignees, commenters);
        issue
    }

    /// A pull request as `PullFields` reads it, made from #543 of
    /// `fixtures/pull-check-run-concluded-in-progress.json` as
    /// [`issue_json`] makes an issue, in `state`, from the branch
    /// `NUMBER-work` at [`FORCE_PUSHED_HEAD`].
    fn pull_json(number: u64, author: &str, assignees: &[&str], state: &str) -> Value {
        let mut pull: Value = serde_json::from_str(include_str!(
            "fixtures/pull-check-run-concluded-in-progress.json"
        ))
        .unwrap();
        pull["number"] = json!(number);
        pull["headRefName"] = json!(format!("{number}-work"));
        pull["headRefOid"] = json!(FORCE_PUSHED_HEAD);
        pull["state"] = json!(state);
        if state == "CLOSED" {
            pull["closedAt"] = json!("2026-11-01T04:00:00Z");
        }
        pull["mergeStateStatus"] = json!("CLEAN");
        pull["closingIssuesReferences"]["nodes"] = json!([]);
        worked_json(&mut pull, author, &[], assignees, &[]);
        pull
    }

    fn worked_json(
        item: &mut Value,
        author: &str,
        labels: &[(&str, &str)],
        assignees: &[&str],
        commenters: &[&str],
    ) {
        let actor = |login: &str| json!({"__typename": "User", "login": login});
        let at = "2026-11-01T03:00:00Z";
        item["author"] = actor(author);
        item["labels"]["nodes"] = labels
            .iter()
            .map(|(name, _)| json!({"name": name}))
            .collect();
        item["assignees"]["nodes"] = assignees.iter().map(|a| json!({"login": a})).collect();
        item["comments"]["nodes"] = commenters
            .iter()
            .map(|c| json!({"author": actor(c), "body": "A comment.", "createdAt": at}))
            .collect();
        item["timelineItems"]["nodes"] = labels
            .iter()
            .map(|(name, by)| {
                json!({"__typename": "LabeledEvent", "createdAt": at, "actor": actor(by), "label": {"name": name}})
            })
            .collect();
    }

    /// A repository that a fake GitHub serves: its issues and pull requests
    /// as `IssueFields` and `PullFields` read them, and the names of its
    /// branches, each with its head committed by `tibbe-bot`. It answers a
    /// discovery `per_page` items to a page, each request `delay` after it
    /// arrives, and refuses the first `rules_refused` reads of `main`'s
    /// rules.
    #[derive(Clone)]
    struct FakeRepository {
        issues: Vec<Value>,
        pulls: Vec<Value>,
        branches: BTreeSet<String>,
        per_page: usize,
        delay: std::time::Duration,
        rules_refused: usize,
    }

    impl FakeRepository {
        fn open(item: &Value) -> bool {
            item["state"] == "OPEN"
        }

        fn logins(item: &Value, field: &str) -> Vec<String> {
            item[field]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|node| match field {
                    "comments" => node["author"]["login"].as_str().unwrap().to_string(),
                    _ => node["login"].as_str().unwrap().to_string(),
                })
                .collect()
        }

        /// The answer to a discovery query: GitHub's open issues with any of
        /// the labels asked, those assigned to the login asked, and the open
        /// issues and pull requests that `involves:` its login finds, its
        /// author's, assignee's or commenter's
        /// (<https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests#search-by-a-user-thats-involved-in-an-issue-or-pull-request>),
        /// `per_page` to a page.
        fn discovered(&self, query: &str, variables: &Value) -> Value {
            let page = |found: Vec<Value>, cursor: &Value| {
                let start: usize = cursor.as_str().map_or(0, |c| c.parse().unwrap());
                let end = (start + self.per_page).min(found.len());
                json!({
                    "pageInfo": {"hasNextPage": end < found.len(), "endCursor": end.to_string()},
                    "nodes": found[start..end],
                })
            };
            let number =
                |item: &Value, kind: &str| json!({"__typename": kind, "number": item["number"]});
            let mut repository = serde_json::Map::new();
            if query.contains("labeled: issues(") {
                let labels: Vec<&str> = variables["labels"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|l| l.as_str().unwrap())
                    .collect();
                let found = self
                    .issues
                    .iter()
                    .filter(|i| Self::open(i))
                    .filter(|i| {
                        i["labels"]["nodes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|l| labels.contains(&l["name"].as_str().unwrap()))
                    })
                    .map(|i| number(i, "Issue"))
                    .collect();
                repository.insert("labeled".into(), page(found, &variables["labeled"]));
            }
            if query.contains("assigned: issues(") {
                let assignee = variables["assignee"].as_str().unwrap().to_string();
                let found = self
                    .issues
                    .iter()
                    .filter(|i| Self::open(i) && Self::logins(i, "assignees").contains(&assignee))
                    .map(|i| number(i, "Issue"))
                    .collect();
                repository.insert("assigned".into(), page(found, &variables["assigned"]));
            }
            let mut data = json!({"repository": repository});
            if query.contains("involved: search(") {
                let search = variables["involves"].as_str().unwrap();
                let login = search
                    .strip_prefix("repo:tibbe/linewright is:open involves:")
                    .unwrap_or_else(|| panic!("searched {search}"))
                    .to_string();
                let involves = |item: &&Value| {
                    Self::open(item)
                        && (item["author"]["login"] == login.as_str()
                            || Self::logins(item, "assignees").contains(&login)
                            || Self::logins(item, "comments").contains(&login))
                };
                let found = (self
                    .issues
                    .iter()
                    .filter(involves)
                    .map(|i| number(i, "Issue")))
                .chain(
                    self.pulls
                        .iter()
                        .filter(involves)
                        .map(|p| number(p, "PullRequest")),
                )
                .collect();
                data["involved"] = page(found, &variables["involved"]);
            }
            data
        }

        /// The answer to a query of items by number: those of the kind
        /// asked it has, null for the others.
        fn numbered(&self, query: &str) -> Option<Value> {
            let (items, field) = if query.contains(": issue(number: ") {
                (&self.issues, ": issue(number: ")
            } else if query.contains(": pullRequest(number: ") {
                (&self.pulls, ": pullRequest(number: ")
            } else {
                return None;
            };
            let read_in_full = query.contains("...IssueFields") || field.contains("pull");
            let mut repository = serde_json::Map::new();
            for asked in query.split(field).skip(1) {
                let n: u64 = asked[..asked.find(')').unwrap()].parse().unwrap();
                let found = items
                    .iter()
                    .find(|i| i["number"] == n)
                    .filter(|_| read_in_full);
                repository.insert(format!("i{n}"), found.cloned().unwrap_or(Value::Null));
            }
            Some(json!({"repository": repository}))
        }

        /// The answer to a query of closed issues by number with
        /// `ClosedIssueFields`: #731 of `fixtures/closed-issue-handed-back.json`,
        /// null for the others.
        fn closed_numbered(query: &str) -> Option<Value> {
            if !query.contains("...ClosedIssueFields") {
                return None;
            }
            let closed: Value =
                serde_json::from_str(include_str!("fixtures/closed-issue-handed-back.json"))
                    .unwrap();
            let mut repository = serde_json::Map::new();
            for asked in query.split(": issue(number: ").skip(1) {
                let n: u64 = asked[..asked.find(')').unwrap()].parse().unwrap();
                let found = if n == 731 {
                    closed.clone()
                } else {
                    Value::Null
                };
                repository.insert(format!("i{n}"), found);
            }
            Some(json!({"repository": repository}))
        }

        /// The answer to a read of branches by name ([`named_refs_query`]):
        /// each of its branches as `fixtures/refs-named.json` reads
        /// `301-cutting-tickets`, under its own name and committed by
        /// `tibbe-bot`, and null for any other.
        fn refs(&self, variables: &Value) -> Value {
            let recorded: Value =
                serde_json::from_str(include_str!("fixtures/refs-named.json")).unwrap();
            let refs: serde_json::Map<String, Value> = variables
                .as_object()
                .unwrap()
                .iter()
                .filter(|(variable, _)| !["owner", "name"].contains(&variable.as_str()))
                .map(|(alias, qualified)| {
                    let name = qualified.as_str().unwrap().strip_prefix("refs/heads/");
                    let name = name.unwrap();
                    let mut answer = Value::Null;
                    if self.branches.contains(name) {
                        answer = recorded["data"]["repository"]["b0"].clone();
                        answer["name"] = json!(name);
                        answer["target"]["committer"]["user"]["login"] = json!("tibbe-bot");
                    }
                    (alias.clone(), answer)
                })
                .collect();
            json!({"repository": refs})
        }

        /// The answer to a read of a branch's activity: that of
        /// `fixtures/activity-force-pushed.json` before its branch was
        /// deleted, created and then force-pushed to
        /// [`FORCE_PUSHED_HEAD`]. The fixture is what GitHub answered on
        /// 2026-11-17 for the activity of
        /// <https://github.com/tibbe/linewright>'s branch
        /// `1313-give-a-poll-s-snapshot-the-scope-it-read`.
        fn activity() -> Value {
            let recorded: Vec<Value> =
                serde_json::from_str(include_str!("fixtures/activity-force-pushed.json")).unwrap();
            recorded
                .into_iter()
                .filter(|a| a["activity_type"] != "branch_deletion")
                .collect()
        }

        /// The answer to a request, as GitHub's: besides the items it holds,
        /// the closed issues of `fixtures/managed-facts-closed.json` to a
        /// search of closed issues, and `fixtures/merged-pull.json`, #477, as
        /// merged at 22:00 on 2026-10-29.
        fn answer(&self, request: &Value) -> Value {
            let query = request["query"].as_str().unwrap();
            let page = |nodes: Value| json!({"pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": nodes});
            let searched = request["variables"]["query"].as_str().unwrap_or_default();
            let mut merged: Value =
                serde_json::from_str(include_str!("fixtures/merged-pull.json")).unwrap();
            merged["mergedAt"] = json!("2026-10-29T22:00:00Z");
            merged["updatedAt"] = json!("2026-10-29T22:00:00Z");
            let closed: Value =
                serde_json::from_str(include_str!("fixtures/managed-facts-closed.json")).unwrap();
            let data = if query.contains("ref(qualifiedName: ") {
                self.refs(&request["variables"])
            } else if query.contains("involved: search(")
                || query.contains("labeled: issues(")
                || query.contains("assigned: issues(")
            {
                self.discovered(query, &request["variables"])
            } else if searched.contains("is:closed") {
                json!({"search": closed})
            } else if let Some(numbered) = Self::closed_numbered(query) {
                numbered
            } else if let Some(numbered) = self.numbered(query) {
                numbered
            } else if query.contains("states: [MERGED]") {
                json!({"repository": {"pullRequests": page(json!([merged]))}})
            } else if query.contains("pullRequests(") {
                json!({"repository": {"pullRequests": page(json!([]))}})
            } else {
                json!({"repository": {}})
            };
            json!({"data": data})
        }
    }

    /// The head `fixtures/activity-force-pushed.json` was force-pushed to.
    const FORCE_PUSHED_HEAD: &str = "3cc0a4093f900c861b510b38a264c2f16fe209b5";

    /// What a fake GitHub was sent: for each request, its method and path,
    /// and a GraphQL request's body; and the most requests it answered at
    /// once.
    #[derive(Default)]
    struct Sent {
        requests: std::sync::Mutex<Vec<(String, Value)>>,
        in_flight: std::sync::atomic::AtomicUsize,
        most_in_flight: std::sync::atomic::AtomicUsize,
    }

    impl Sent {
        fn requests(&self) -> std::sync::MutexGuard<'_, Vec<(String, Value)>> {
            self.requests.lock().unwrap()
        }

        /// How many of the requests asked for the path `path`.
        fn asked(&self, path: &str) -> usize {
            self.requests()
                .iter()
                .filter(|(sent, _)| sent.split(['?', ' ']).nth(1) == Some(path))
                .count()
        }
    }

    /// The address of a fake GitHub holding `repository`, which tells
    /// `sent` what it was sent.
    async fn serving_repository(repository: FakeRepository, sent: Arc<Sent>) -> String {
        use std::sync::atomic::Ordering;
        let repository = Arc::new(repository);
        let app = axum::Router::new().fallback(
            move |method: axum::http::Method, uri: axum::http::Uri, body: String| {
                let repository = repository.clone();
                let sent = sent.clone();
                async move {
                    let request: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    sent.requests()
                        .push((format!("{method} {uri}"), request.clone()));
                    let now = sent.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    sent.most_in_flight.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(repository.delay).await;
                    sent.in_flight.fetch_sub(1, Ordering::SeqCst);
                    let rules = "/repos/tibbe/linewright/rules/branches/main";
                    let answer = match uri.path() {
                        "/graphql" => repository.answer(&request).to_string(),
                        "/repos/tibbe/linewright" => json!({"default_branch": "main"}).to_string(),
                        "/repos/tibbe/linewright/activity" => {
                            FakeRepository::activity().to_string()
                        }
                        path if path == rules && sent.asked(rules) <= repository.rules_refused => {
                            let refusal =
                                json!({"message": "Resource not accessible by integration"});
                            return (axum::http::StatusCode::FORBIDDEN, refusal.to_string());
                        }
                        _ => "[]".to_string(),
                    };
                    (axum::http::StatusCode::OK, answer)
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        format!("http://{address}")
    }

    /// A GitHub store, as tibbe-bot, on tibbe/linewright, served by a fake
    /// GitHub holding `repository`, with what it was sent.
    async fn serving_fake(repository: FakeRepository) -> (GitHub, Arc<Sent>) {
        let sent = Arc::<Sent>::default();
        let github = GitHub {
            api: serving_repository(repository, sent.clone()).await,
            owner: "tibbe".into(),
            name: "linewright".into(),
            viewer: Viewer {
                login: "tibbe-bot".into(),
            },
            ..github("token")
        };
        (github, sent)
    }

    /// Whether `request` lists every open issue or pull request of the
    /// repository, rather than those a filter of Linewright's selects or the
    /// closed and merged ones: a REST listing of the issues or pull requests
    /// (<https://docs.github.com/en/rest/issues/issues#list-repository-issues>,
    /// <https://docs.github.com/en/rest/pulls/pulls#list-pull-requests>), or
    /// a GraphQL connection of them unfiltered.
    fn lists_every_open_item((sent, request): &(String, Value)) -> bool {
        let path = sent.split('?').next().unwrap();
        if path.ends_with("/issues") || path.ends_with("/pulls") {
            return true;
        }
        let Some(query) = request["query"].as_str() else {
            return false;
        };
        let narrowed = |connection: &str| {
            [
                "labels:",
                "filterBy: {assignee",
                "states: [CLOSED]",
                "states: [MERGED]",
            ]
            .iter()
            .any(|filter| connection.contains(filter))
        };
        ["issues(", "pullRequests("].iter().any(|listing| {
            query
                .split(listing)
                .skip(1)
                .any(|rest| !narrowed(&rest[..rest.find(')').unwrap()]))
        })
    }

    /// `docs/specs/repository.md`, Managed items, and
    /// `docs/specs/sources_of_work.md`, Polling: a snapshot holds the items
    /// Linewright manages, found through its own filters and the items it
    /// knows, and no request it makes lists every open issue or pull request.
    /// Among the open issues, a person labeled #1 `ready-for-agent` and
    /// assigned Linewright to #2; #3 and #6 carry only a person's labels,
    /// and Linewright only commented on #4. Linewright labeled #5 as a run
    /// on it ended, and a person has since replaced the label: no filter
    /// finds it, and Linewright knows it. #900 is Linewright's tracking
    /// issue. Of the pull requests, #10 is Linewright's, a person assigned
    /// Linewright to their #11, #12 is a person's alone and Linewright's #13,
    /// which it knows, is closed.
    #[test_log::test(tokio::test)]
    async fn a_snapshot_holds_the_managed_items_found_by_linewrights_filters_without_listing_every_open_item()
     {
        let bot = "tibbe-bot";
        let mut known = issue_json(5, "tibbe", &[("needs-info", bot)], &[], &[]);
        known["labels"]["nodes"] = json!([{"name": "question"}]);
        let repository = FakeRepository {
            issues: vec![
                issue_json(1, "tibbe", &[("ready-for-agent", "tibbe")], &[], &[]),
                issue_json(2, "tibbe", &[], &[bot], &[]),
                issue_json(3, "tibbe", &[("bug", "tibbe")], &[], &[]),
                issue_json(4, "tibbe", &[], &[], &[bot]),
                issue_json(6, "tibbe", &[("needs-info", "tibbe")], &[], &[]),
                known,
                tracking_issue_json(),
            ],
            pulls: vec![
                pull_json(10, bot, &[bot], "OPEN"),
                pull_json(11, "tibbe", &[bot], "OPEN"),
                pull_json(12, "tibbe", &[], "OPEN"),
                pull_json(13, bot, &[], "CLOSED"),
            ],
            branches: BTreeSet::new(),
            per_page: 2,
            delay: std::time::Duration::ZERO,
            rules_refused: 0,
        };
        let (github, sent) = serving_fake(repository).await;
        let scope = Scope {
            items: [Item::Issue(Number(5)), Item::Pull(Number(13))].into(),
            branches: BTreeSet::new(),
        };

        let snapshot = github
            .snapshot(opened("2026-10-29T21:00:00Z"), &scope)
            .await
            .unwrap();

        let issues: Vec<u64> = snapshot.issues.iter().map(|i| i.number.0).collect();
        assert_eq!(issues, [1, 2, 5, 900]);
        let pulls: Vec<u64> = snapshot.pulls.iter().map(|p| p.number.0).collect();
        assert_eq!(pulls, [10]);
        let peoples: Vec<u64> = snapshot.peoples_pulls.iter().map(|p| p.number.0).collect();
        assert_eq!(peoples, [11]);
        assert_eq!(
            snapshot.spec_change_issues,
            [SpecChangeIssue {
                number: Number(900),
                spec_change: Number(899),
            }]
        );
        let closed: Vec<u64> = snapshot.closed.iter().map(|i| i.number.0).collect();
        assert_eq!(closed, [731]);
        let merged: Vec<u64> = snapshot.merged.iter().map(|p| p.number.0).collect();
        assert_eq!(merged, [477]);
        let log = sent.requests();
        let listing: Vec<_> = log.iter().filter(|r| lists_every_open_item(r)).collect();
        assert!(listing.is_empty(), "listed every open item: {listing:#?}");
        let closed_searches: Vec<&str> = log
            .iter()
            .filter_map(|(_, request)| request["variables"]["query"].as_str())
            .filter(|searched| searched.contains("is:closed"))
            .collect();
        assert_eq!(
            closed_searches,
            [
                "repo:tibbe/linewright is:issue is:closed updated:>=2026-10-29T21:00:00Z involves:tibbe-bot"
            ]
        );
        let discovery = log
            .iter()
            .find_map(|(_, request)| request["query"].as_str().filter(|q| q.contains("search(")))
            .unwrap();
        assert!(
            ["labeled: issues(", "assigned: issues(", "involved: search("]
                .iter()
                .all(|finder| discovery.contains(finder)),
            "the first discovery asked {discovery}"
        );
    }

    /// `fixtures/discovery.json` is what GitHub answered on 2026-11-17 to
    /// the first discovery query for <https://github.com/tibbe/linewright>
    /// as `tibbe-bot`: the open issues #1302, #1315, #1316 and #1317, with
    /// its labels, assigned to it or involving it, each finder on one page.
    #[test_log::test(tokio::test)]
    async fn the_items_discovered_are_those_any_of_linewrights_filters_found() {
        let (ask, mut asked) = tokio::sync::mpsc::unbounded_channel();
        let api = serving_graphql(move |request: String| {
            ask.send(request).unwrap();
            async { include_str!("fixtures/discovery.json").to_string() }
        })
        .await;
        let github = github_serving_refs(api, "tibbe-bot");

        let found = github.discovered().await.unwrap();

        assert_eq!(
            found,
            [1302, 1315, 1316, 1317]
                .map(|n| Item::Issue(Number(n)))
                .into()
        );
        let mut requests = 0;
        while asked.try_recv().is_ok() {
            requests += 1;
        }
        assert_eq!(requests, 1);
    }

    /// A repository holding Linewright's work at the bound of
    /// `docs/specs/sources_of_work.md`, Polling, Reaction time: 100 items
    /// and 100 branches. A person labeled the open issues #1 to #50
    /// `ready-for-agent`; Linewright opened the pull requests #101 to #150,
    /// each blocked from merging into `main`, and pushed the branch `N-work`
    /// for each issue `N` of #1 to #100, of which #51 to #100 are closed.
    fn linewrights_work_at_the_bound(delay: std::time::Duration) -> (FakeRepository, Scope) {
        let bot = "tibbe-bot";
        let pulls = (101..=150)
            .map(|n| {
                let mut pull = pull_json(n, bot, &[bot], "OPEN");
                pull["mergeStateStatus"] = json!("BLOCKED");
                pull
            })
            .collect();
        let branches: BTreeSet<String> = (1..=100).map(|n| format!("{n}-work")).collect();
        let scope = Scope {
            items: BTreeSet::new(),
            branches: branches
                .iter()
                .map(|b| TicketBranchName::parse(b).unwrap())
                .collect(),
        };
        let repository = FakeRepository {
            issues: (1..=100)
                .map(|n| {
                    let mut issue =
                        issue_json(n, "tibbe", &[("ready-for-agent", "tibbe")], &[], &[]);
                    if n > 50 {
                        issue["state"] = json!("CLOSED");
                    }
                    issue
                })
                .collect(),
            pulls,
            branches,
            per_page: 100,
            delay,
            rules_refused: 0,
        };
        (repository, scope)
    }

    /// `docs/specs/sources_of_work.md`, Polling, Reaction time: at 100 items
    /// and branches, with GitHub answering each request within 1 s, a
    /// snapshot takes at most about 10 s, which is 10 rounds of requests,
    /// sent at most 16 at once. The default branch's rules are read once.
    #[test_log::test(tokio::test(flavor = "multi_thread", worker_threads = 4))]
    async fn a_snapshot_at_a_hundred_items_and_branches_takes_at_most_about_ten_rounds_of_requests_sixteen_at_once()
     {
        let round = std::time::Duration::from_millis(200);
        let (repository, scope) = linewrights_work_at_the_bound(round);
        let (github, sent) = serving_fake(repository).await;

        let started = std::time::Instant::now();
        let snapshot = github
            .snapshot(opened("2026-10-29T21:00:00Z"), &scope)
            .await
            .unwrap();
        let took = started.elapsed();

        assert_eq!(snapshot.issues.len(), 50);
        assert_eq!(snapshot.pulls.len(), 50);
        assert_eq!(snapshot.ticket_branches.len(), 100);
        assert!(
            took <= 10 * round,
            "took {took:?}, {} rounds",
            took.as_secs_f64() / round.as_secs_f64()
        );
        let most = sent
            .most_in_flight
            .load(std::sync::atomic::Ordering::SeqCst);
        assert!(most <= 16, "{most} requests in flight at once");
        assert_eq!(sent.asked("/repos/tibbe/linewright/rules/branches/main"), 1);
    }

    /// A failed read of a branch's rules is not kept for the other pull
    /// requests of the snapshot: the next one reads them again.
    #[test_log::test(tokio::test)]
    async fn a_failed_read_of_a_branchs_rules_is_read_again_for_the_next_pull_request() {
        let (mut repository, scope) = linewrights_work_at_the_bound(std::time::Duration::ZERO);
        repository.rules_refused = 1;
        let (github, sent) = serving_fake(repository).await;

        github
            .snapshot(opened("2026-10-29T21:00:00Z"), &scope)
            .await
            .unwrap();

        assert_eq!(sent.asked("/repos/tibbe/linewright/rules/branches/main"), 2);
    }

    /// A snapshot reads the closed and merged items once it has read the
    /// open ones, the open pull requests last among them, so an item that closes while it reads is read closed
    /// rather than neither open nor closed.
    #[test_log::test(tokio::test)]
    async fn a_snapshot_reads_the_closed_and_merged_items_once_it_has_read_the_open_ones() {
        let (repository, scope) = linewrights_work_at_the_bound(std::time::Duration::ZERO);
        let (github, sent) = serving_fake(repository).await;

        github
            .snapshot(opened("2026-10-29T21:00:00Z"), &scope)
            .await
            .unwrap();

        let log = sent.requests();
        let query = |(_, request): &(String, Value)| {
            request["query"].as_str().unwrap_or_default().to_string()
        };
        let reads_open_pulls = |request: &(String, Value)| {
            let query = query(request);
            query.contains(": pullRequest(number: ") && query.contains("mergeStateStatus")
        };
        let reads_closed = |request: &(String, Value)| {
            let searched = request.1["variables"]["query"].as_str().unwrap_or_default();
            let query = query(request);
            searched.contains("is:closed")
                || query.contains("states: [CLOSED]")
                || query.contains("states: [MERGED]")
        };
        let last_open = log.iter().rposition(reads_open_pulls).unwrap();
        let first_closed = log.iter().position(reads_closed).unwrap();
        assert!(
            last_open < first_closed,
            "read open items at {last_open}, closed at {first_closed}: {log:#?}"
        );
    }

    /// A pull request's pushes are kept by branch and head: a second poll
    /// with no head moved reads no pushes, and holds those the first read,
    /// oldest first.
    #[test_log::test(tokio::test)]
    async fn a_second_poll_with_no_head_moved_reads_no_pushes() {
        let (repository, scope) = linewrights_work_at_the_bound(std::time::Duration::ZERO);
        let (github, sent) = serving_fake(repository).await;
        let windows = opened("2026-10-29T21:00:00Z");
        github.snapshot(windows, &scope).await.unwrap();
        let first = sent.asked("/repos/tibbe/linewright/activity");

        let snapshot = github.snapshot(windows, &scope).await.unwrap();

        assert_eq!(first, 50);
        assert_eq!(sent.asked("/repos/tibbe/linewright/activity"), 50);
        let bot = Login::new("tibbe-bot");
        assert_eq!(
            snapshot.pulls[0].pushes,
            [
                Push {
                    by: bot.clone(),
                    at: at("2026-11-17T08:05:05Z"),
                },
                Push {
                    by: bot,
                    at: at("2026-11-17T08:11:56Z"),
                },
            ]
        );
    }

    /// A pull request's pushes are read again when its head moves, and
    /// while the newest entry of its activity is not at its head.
    #[test_log::test(tokio::test)]
    async fn a_pull_requests_pushes_are_read_again_when_its_head_moves_or_its_activity_lags_behind_it()
     {
        let (mut repository, scope) = linewrights_work_at_the_bound(std::time::Duration::ZERO);
        let (github, sent) = serving_fake(repository.clone()).await;
        let windows = opened("2026-10-29T21:00:00Z");
        github.snapshot(windows, &scope).await.unwrap();
        repository.pulls[0]["headRefOid"] = json!("a2b01b737fa17e9845c52d03004c216438a32724");
        let moved = GitHub {
            api: serving_repository(repository, sent.clone()).await,
            ..github.clone()
        };

        moved.snapshot(windows, &scope).await.unwrap();
        moved.snapshot(windows, &scope).await.unwrap();

        assert_eq!(sent.asked("/repos/tibbe/linewright/activity"), 50 + 1 + 1);
    }

    /// `fixtures/closed-issue-worked.json` is `ClosedIssueFields` as GitHub
    /// returned it for <https://github.com/tibbe/linewright/issues/673>, a
    /// person's issue Linewright claimed, let go of, claimed again and closed
    /// with its merged pull request #675.
    #[test_log::test]
    fn a_closed_issue_is_held_with_its_assignees_assignments_and_the_pull_requests_closing_it() {
        let issue: GqlClosedIssue =
            serde_json::from_str(include_str!("fixtures/closed-issue-worked.json")).unwrap();

        let closed = issue.into_closed(&Login::new("tibbe-bot")).unwrap();

        let ClosedKind::Issue {
            facts: ManagedFacts { work, .. },
            ..
        } = closed.kind
        else {
            panic!("an issue");
        };
        let bot = || Login::new("tibbe-bot");
        assert_eq!(work.author, Some(Login::new("tibbe")));
        assert_eq!(work.assignees, [bot()]);
        assert_eq!(
            work.events,
            [
                IssueEvent::Assignment(Assignment::Assigned {
                    assignee: bot(),
                    actor: bot(),
                    at: at("2026-10-27T05:29:42Z"),
                }),
                IssueEvent::Assignment(Assignment::Unassigned {
                    assignee: bot(),
                    actor: bot(),
                    at: at("2026-10-27T05:30:54Z"),
                }),
                IssueEvent::Assignment(Assignment::Assigned {
                    assignee: bot(),
                    actor: bot(),
                    at: at("2026-10-27T05:40:47Z"),
                }),
            ]
        );
        assert_eq!(
            work.closed_by,
            [ClosingPull {
                number: Number(675),
                author: bot(),
                state: PullState::Merged {
                    at: at("2026-10-27T05:54:12Z"),
                },
                updated_at: at("2026-10-27T05:54:14Z"),
                base_ref: String::new(),
            }]
        );
    }

    /// `fixtures/closed-issue-reopened.json` is `ClosedIssueFields` as GitHub
    /// returned it for <https://github.com/rust-lang/cargo/issues/15010>,
    /// which its merged pull request #16807 closed, someone reopened, and its
    /// merged pull request #17354 closed again.
    #[test_log::test]
    fn a_closed_issue_is_held_with_its_reopenings_and_the_branch_each_pull_request_closing_it_merged_into()
     {
        let issue: GqlClosedIssue =
            serde_json::from_str(include_str!("fixtures/closed-issue-reopened.json")).unwrap();

        let closed = issue.into_closed(&Login::new("tibbe-bot")).unwrap();

        let ClosedKind::Issue {
            facts: ManagedFacts { work, .. },
            ..
        } = closed.kind
        else {
            panic!("an issue");
        };
        let reopenings: Vec<&IssueEvent> = work
            .events
            .iter()
            .filter(|event| matches!(event, IssueEvent::Reopened { .. }))
            .collect();
        assert_eq!(
            reopenings,
            [&IssueEvent::Reopened {
                at: at("2026-07-20T15:02:16Z")
            }]
        );
        assert_eq!(
            work.closed_by
                .iter()
                .map(|pull| (pull.number.0, pull.base_ref.as_str()))
                .collect::<Vec<_>>(),
            [(16807, "master"), (17354, "master")]
        );
    }

    /// `fixtures/closed-issue-handed-back.json` is `ClosedIssueFields` as
    /// GitHub returned it for <https://github.com/tibbe/linewright/issues/731>,
    /// as in [`an_issue_lists_who_applied_each_label`].
    #[test_log::test]
    fn a_closed_issue_is_held_with_who_applied_each_label() {
        let issue: GqlClosedIssue =
            serde_json::from_str(include_str!("fixtures/closed-issue-handed-back.json")).unwrap();

        let closed = issue.into_closed(&Login::new("tibbe-bot")).unwrap();

        let ClosedKind::Issue {
            facts: ManagedFacts { work, .. },
            ..
        } = closed.kind
        else {
            panic!("an issue");
        };
        assert_eq!(
            labelings(&work.events),
            [
                ("needs-triage", Some("tibbe")),
                ("enhancement", Some("tibbe-bot")),
                ("ready-for-human", Some("tibbe-bot")),
            ]
        );
    }

    /// `fixtures/closed-issue-not-planned.json` is `ClosedIssueFields` as
    /// GitHub returned it for <https://github.com/tibbe/linewright/issues/305>,
    /// which its maintainer closed as not planned.
    #[test_log::test]
    fn an_issue_closed_as_not_planned_is_held_with_that_reason() {
        let issue: GqlClosedIssue =
            serde_json::from_str(include_str!("fixtures/closed-issue-not-planned.json")).unwrap();

        let closed = issue.into_closed(&Login::new("tibbe-bot")).unwrap();

        let ClosedKind::Issue { reason, .. } = closed.kind else {
            panic!("an issue");
        };
        assert_eq!(reason, Some(IssueCloseReason::NotPlanned));
    }

    /// The fixture with GitHub's other documented reasons
    /// (<https://docs.github.com/en/graphql/reference/enums#issuestatereason>).
    #[test_log::test]
    fn a_closed_issue_is_held_with_githubs_reason_and_none_where_github_gives_none() {
        let reason = |state_reason: Value| {
            let mut issue: Value =
                serde_json::from_str(include_str!("fixtures/closed-issue-not-planned.json"))
                    .unwrap();
            issue["stateReason"] = state_reason;
            let issue: GqlClosedIssue = serde_json::from_value(issue).unwrap();
            match issue.into_closed(&Login::new("tibbe-bot")).unwrap().kind {
                ClosedKind::Issue { reason, .. } => reason,
                ClosedKind::Pull { .. } => panic!("an issue"),
            }
        };

        assert_eq!(
            reason(json!("COMPLETED")),
            Some(IssueCloseReason::Completed)
        );
        assert_eq!(
            reason(json!("DUPLICATE")),
            Some(IssueCloseReason::Duplicate)
        );
        assert_eq!(reason(json!(null)), None);
    }

    /// The pull requests closed without merging of the page `recorded`,
    /// closed since `since`, that `linewright` worked.
    fn closed_pulls_held(recorded: &str, since: &str, linewright: &str) -> Vec<ClosedItem> {
        let page: Page<GqlClosedPull> = serde_json::from_str(recorded).unwrap();
        let read = page
            .nodes
            .into_iter()
            .filter_map(|pr| pr.into_closed(&Login::new(linewright)));
        closed_within(read, opened(since))
    }

    /// `fixtures/closed-pulls.json` is the page of closed pull requests
    /// GitHub returned for <https://github.com/tibbe/linewright>: the one,
    /// #321, its maintainer opened and closed without merging, which no one
    /// labeled, assigned or commented on.
    #[test_log::test]
    fn pull_requests_closed_unmerged_since_the_window_opened_are_held_when_linewright_opened_them()
    {
        let closed = |since, linewright| {
            closed_pulls_held(
                include_str!("fixtures/closed-pulls.json"),
                since,
                linewright,
            )
        };

        let maintainer_as_linewright = "tibbe";
        assert_eq!(
            closed("2026-10-18T07:00:00Z", maintainer_as_linewright),
            [ClosedItem {
                number: Number(321),
                title: "Restore the to-tickets skill to upstream, then make it non-interactive"
                    .into(),
                closed_at: at("2026-10-18T07:30:05Z"),
                updated_at: at("2026-10-18T07:30:05Z"),
                kind: ClosedKind::Pull {
                    closes: vec![],
                    author: Some(Login::new("tibbe")),
                    assignees: vec![],
                    review_requests: vec![],
                    reviewers: vec![],
                },
            }]
        );
        assert_eq!(closed("2026-10-18T08:00:00Z", maintainer_as_linewright), []);
        assert_eq!(closed("2026-10-18T07:00:00Z", "tibbe-bot"), []);
    }

    /// `fixtures/closed-pulls-commented.json` is the first page of closed
    /// pull requests GitHub returned for <https://github.com/cli/cli> on
    /// 2026-10-30. Of those closed since 1 October, its maintainer `babakks`
    /// only commented on #13318 and #14664, people's, and did nothing on the
    /// rest.
    #[test_log::test]
    fn pull_requests_closed_unmerged_that_linewright_only_commented_on_are_not_held() {
        let maintainer_as_linewright = "babakks";
        let closed = closed_pulls_held(
            include_str!("fixtures/closed-pulls-commented.json"),
            "2026-10-01T00:00:00Z",
            maintainer_as_linewright,
        );

        assert_eq!(closed, []);
    }

    /// `fixtures/open-pulls-listed.json` is the first page of the open pull
    /// requests listed for <https://github.com/cli/cli> on 2026-10-30. Of
    /// them, its maintainer `babakks` opened #11500, labeled #13013, was
    /// assigned by another to #13017, only commented on #14017, and did
    /// nothing on #9847.
    #[test_log::test]
    fn open_pull_requests_are_held_as_linewrights_own_or_as_a_persons_linewright_labeled_or_is_assigned_to()
     {
        let page: Page<Value> =
            serde_json::from_str(include_str!("fixtures/open-pulls-listed.json")).unwrap();

        let maintainer_as_linewright = Login::new("babakks");
        let held: BTreeMap<u64, Option<PullHeld>> = page
            .nodes
            .into_iter()
            .map(|pr| {
                let number = pr["number"].as_u64().unwrap();
                let signs: GqlPullSigns = serde_json::from_value(pr).unwrap();
                (number, signs.held(&maintainer_as_linewright))
            })
            .collect();

        let of = |numbers: &[u64]| -> Vec<Option<PullHeld>> {
            numbers.iter().map(|n| held[n]).collect()
        };
        assert_eq!(
            of(&[11500, 13013, 13017, 14017, 9847]),
            [
                Some(PullHeld::Linewrights),
                Some(PullHeld::Peoples),
                Some(PullHeld::Peoples),
                None,
                None,
            ]
        );
    }

    /// `fixtures/peoples-pulls.json` is `PeoplesPullFields` as GitHub
    /// returned it, by number, for pull requests of
    /// <https://github.com/cli/cli> on 2026-10-30: #13017, open, which
    /// `babakks` was assigned to; #9847, open, which `babakks` did nothing
    /// on; and #14664, which `babakks` commented on, closed.
    #[test_log::test]
    fn a_persons_pull_request_is_read_in_full_while_it_is_open_and_linewright_may_manage_it() {
        let response: GqlResponse<RepoData<BTreeMap<String, Option<GqlPeoplesPull>>>> =
            serde_json::from_str(include_str!("fixtures/peoples-pulls.json")).unwrap();
        let mut pulls = response.data.unwrap().repository;
        let mut read = |alias: &str| {
            pulls
                .remove(alias)
                .flatten()
                .unwrap()
                .read(&Login::new("babakks"))
        };

        assert_eq!(read("i13017"), (vec![Number(13016)], true));
        assert_eq!(read("i9847"), (vec![Number(9828)], false));
        assert_eq!(read("i14664"), (vec![], false));
    }

    /// `fixtures/pull-labeled.json` is `PullFields` as GitHub returned it
    /// for <https://github.com/tibbe/linewright/pull/817>, a spec change of
    /// `tibbe`'s that `tibbe-bot` assigned itself to and labeled designed.
    #[test_log::test]
    fn a_pull_request_lists_who_applied_each_label_when_and_is_worked_by_who_applied_one() {
        let pr: GqlPull = serde_json::from_str(include_str!("fixtures/pull-labeled.json")).unwrap();

        let pull = pr.into_pull(vec![], &BranchRules::default());

        assert_eq!(
            pull.labelings,
            [Labeling {
                label: "linewright:designed".into(),
                by: Some(Login::new("tibbe-bot")),
                at: at("2026-10-30T09:14:46Z"),
            }]
        );
        assert_eq!(pull.assignments.len(), 2);
        assert!(pull.worked_by(&Login::new("tibbe-bot")));
        assert!(!pull.worked_by(&Login::new("alice")));
    }

    /// `fixtures/pull-from-fork.json` is `PullFields` as GitHub returned it
    /// for <https://github.com/cli/cli/pull/14017>, open from a fork;
    /// `fixtures/pull-labeled.json`, as above, merged from a branch of the
    /// repository itself; and
    /// `fixtures/pull-check-run-concluded-in-progress.json` an open pull
    /// request from a branch of the repository itself.
    #[test_log::test]
    fn the_pushes_of_a_pull_request_are_read_only_while_it_is_open_from_a_branch_of_the_repository()
    {
        let read = |recorded: &str| serde_json::from_str::<GqlPull>(recorded).unwrap();

        assert!(!read(include_str!("fixtures/pull-from-fork.json")).pushes_readable());
        assert!(!read(include_str!("fixtures/pull-labeled.json")).pushes_readable());
        assert!(
            read(include_str!(
                "fixtures/pull-check-run-concluded-in-progress.json"
            ))
            .pushes_readable()
        );
    }

    /// `fixtures/merged-pull-commented.json` is `MergedFields` as GitHub
    /// returned it for <https://github.com/tibbe/linewright/pull/817>, as
    /// above, which `tibbe-bot` also commented on. Its timeline is kept to
    /// that comment.
    #[test_log::test]
    fn a_merged_pull_request_lists_its_comments_and_is_not_worked_by_who_only_commented_on_it() {
        let mut pr: Value =
            serde_json::from_str(include_str!("fixtures/merged-pull-commented.json")).unwrap();
        pr["timelineItems"]["nodes"]
            .as_array_mut()
            .unwrap()
            .retain(|item| item["__typename"] == "IssueComment");
        let pr: GqlMerged = serde_json::from_value(pr).unwrap();

        let merged = pr.into_merged(at("2026-10-30T09:05:41Z"), vec![]);

        assert_eq!(
            merged.events,
            [IssueEvent::Commented {
                by: Login::new("tibbe-bot"),
                at: at("2026-10-30T09:14:45Z"),
            }]
        );
        assert!(!merged.worked_by(&Login::new("tibbe-bot")));
    }

    /// `fixtures/managed-facts-commented.json` is the page of
    /// `ManagedFactsFields` GitHub returned for the open issues of
    /// <https://github.com/cli/cli> last updated on 2026-10-30. Its
    /// maintainer `mislav` only commented on #5773 of them; #14822, #14827,
    /// #14841, #14842, #14855, #14856 and #14868 are labeled `needs-triage`,
    /// which asks for the work of whoever triages.
    #[test_log::test]
    fn an_open_issue_linewright_only_commented_on_is_not_managed() {
        let maintainer_as_linewright = Login::new("mislav");

        let managed: BTreeSet<Number> =
            facts_page(include_str!("fixtures/managed-facts-commented.json"))
                .into_iter()
                .map(|issue| {
                    (
                        Number(issue.number),
                        issue.into_facts(&maintainer_as_linewright),
                    )
                })
                .filter(|(_, facts)| facts.candidate(true, &maintainer_as_linewright))
                .map(|(number, _)| number)
                .collect();

        assert_eq!(
            managed,
            numbers(&[14822, 14827, 14841, 14842, 14855, 14856, 14868])
        );
    }

    /// `fixtures/closed-issues-commented.json` is the page of
    /// `ClosedIssueFields` GitHub returned for the closed issues of
    /// <https://github.com/cli/cli> last updated on 2026-10-30. Of them, its
    /// maintainer `BagToad` only commented on #14711.
    #[test_log::test]
    fn a_closed_issue_linewright_only_commented_on_is_not_held() {
        let page: Page<GqlClosedIssue> =
            serde_json::from_str(include_str!("fixtures/closed-issues-commented.json")).unwrap();

        let maintainer_as_linewright = Login::new("BagToad");
        let closed = managed_closed(page.nodes, &maintainer_as_linewright);

        let held: Vec<Number> = closed.iter().map(|item| item.number).collect();
        assert_eq!(held, []);
    }

    /// `fixtures/labels.json` is the example response GitHub documents for
    /// listing a repository's labels
    /// (<https://docs.github.com/en/rest/issues/labels#list-labels-for-a-repository>).
    #[test_log::test(tokio::test)]
    async fn the_repositorys_labels_are_read_with_their_descriptions_from_every_page() {
        let documented: Vec<RestLabel> =
            serde_json::from_str(include_str!("fixtures/labels.json")).unwrap();
        let mut pages = vec![documented.clone(), documented[..1].to_vec(), documented].into_iter();
        let mut asked = vec![];

        let labels = every_page(2, |n| {
            asked.push(n);
            let page = pages.next().unwrap();
            async move { Ok(page) }
        })
        .await
        .unwrap();

        assert_eq!(asked, [1, 2]);
        let read = |name: &str, description: &str| label::RepoLabel {
            name: name.into(),
            description: Some(description.into()),
        };
        assert_eq!(
            labels
                .into_iter()
                .map(label::RepoLabel::from)
                .collect::<Vec<_>>(),
            [
                read("bug", "Something isn't working"),
                read("enhancement", "New feature or request"),
                read("bug", "Something isn't working"),
            ]
        );
    }

    /// As GitHub documents updating a label
    /// (<https://docs.github.com/en/rest/issues/labels#update-a-label>):
    /// `PATCH /repos/{owner}/{repo}/labels/{name}`, the name a path segment,
    /// with the new `description` in the body.
    #[test_log::test]
    fn a_label_named_with_a_colon_is_described_at_its_percent_encoded_name() {
        let (method, path, body) =
            describe_label_request("linewright:stalled", "Handed back in a stall");

        assert_eq!(method, Method::PATCH);
        assert_eq!(path, "/labels/linewright%3Astalled");
        assert_eq!(body, json!({"description": "Handed back in a stall"}));
    }

    /// One alias per path, each asking for the blob at `BRANCH:PATH`
    /// (<https://docs.github.com/en/graphql/reference/objects#repository>,
    /// `object`), the expression a variable of its own.
    #[test_log::test]
    fn files_are_read_in_one_query_an_alias_per_path() {
        let paths = [
            ".linewright/config.toml".to_string(),
            ".linewright/prompts/triage.md".to_string(),
        ];

        let (query, variables) = files_query("tibbe", "linewright", "main", &paths);

        let compact: String = query.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(
            compact,
            "query($owner: String!, $name: String!, $f0: String!, $f1: String!) { \
             repository(owner: $owner, name: $name) { \
             f0: object(expression: $f0) { ... on Blob { text isBinary isTruncated } } \
             f1: object(expression: $f1) { ... on Blob { text isBinary isTruncated } } } }"
        );
        assert_eq!(
            variables,
            json!({
                "owner": "tibbe",
                "name": "linewright",
                "f0": "main:.linewright/config.toml",
                "f1": "main:.linewright/prompts/triage.md",
            })
        );
    }

    /// As GitHub documents getting a tree
    /// (<https://docs.github.com/en/rest/git/trees#get-a-tree>):
    /// `GET /repos/{owner}/{repo}/git/trees/{tree_sha}?recursive=1`, the tree
    /// named as `BRANCH:DIR`, one percent-encoded path segment.
    #[test_log::test]
    fn a_directory_is_listed_from_its_tree_at_the_head_of_the_branch() {
        assert_eq!(
            tree_path("7-a/b", ".linewright/automations"),
            "/git/trees/7-a%2Fb%3A.linewright%2Fautomations?recursive=1"
        );
    }

    /// `fixtures/linewright-tree.json` is what GitHub returned for
    /// `main:.linewright` on <https://github.com/tibbe/linewright>.
    #[test_log::test]
    fn a_tree_lists_each_file_under_it_by_its_path_leaving_out_its_subtrees() {
        let tree = serde_json::from_str(include_str!("fixtures/linewright-tree.json")).unwrap();

        assert_eq!(
            files_listed(tree).unwrap(),
            [
                "automations/dependencies.md",
                "automations/image-tools.md",
                "automations/rust-toolchain.md",
            ]
        );
    }

    /// `fixtures/linux-tree-truncated.json` is what GitHub returned for
    /// `master` on <https://github.com/torvalds/linux>, cut here to its first
    /// three entries.
    #[test_log::test]
    fn a_tree_too_large_for_github_to_list_whole_is_an_error() {
        let tree =
            serde_json::from_str(include_str!("fixtures/linux-tree-truncated.json")).unwrap();

        assert!(files_listed(tree).is_err());
    }

    /// `fixtures/repository-files.json` is what GitHub returned for that
    /// query on <https://github.com/rust-lang/rust>, asking for a PNG
    /// image, a path with no file, a directory and the script `x`.
    #[test_log::test]
    fn files_read_are_by_path_with_binary_ones_marked_and_paths_with_no_file_left_out() {
        let paths = [
            "src/librustdoc/html/static/images/favicon-32x32.png",
            ".linewright/config.toml",
            "src/librustdoc",
            "x",
        ]
        .map(String::from);
        let files = files_read(
            files_response(include_str!("fixtures/repository-files.json")),
            &paths,
        );

        assert_eq!(
            files.keys().collect::<Vec<_>>(),
            ["src/librustdoc/html/static/images/favicon-32x32.png", "x"]
        );
        assert_eq!(
            files["src/librustdoc/html/static/images/favicon-32x32.png"],
            Blob::Binary
        );
        let Blob::Text(x) = &files["x"] else {
            panic!("{:?}", files["x"]);
        };
        assert!(x.starts_with("#!/bin/sh\n"), "{x}");
    }

    /// `fixtures/repository-file-truncated.json` is what GitHub returned for
    /// that query on <https://github.com/torvalds/linux>, asking for
    /// `MAINTAINERS` (951 kB), its text cut here to its first line.
    #[test_log::test]
    fn a_file_too_large_for_github_to_give_whole_is_read_as_too_large() {
        let paths = ["MAINTAINERS".to_string()];

        let files = files_read(
            files_response(include_str!("fixtures/repository-file-truncated.json")),
            &paths,
        );

        assert_eq!(files, BTreeMap::from([(paths[0].clone(), Blob::TooLarge)]));
    }

    fn files_response(recorded: &str) -> BTreeMap<String, Option<GqlObject>> {
        let response: GqlResponse<RepoData<BTreeMap<String, Option<GqlObject>>>> =
            serde_json::from_str(recorded).unwrap();
        response.data.unwrap().repository
    }

    fn github(token: &str) -> GitHub {
        GitHub {
            http: reqwest::Client::new(),
            api: API.into(),
            token: token.to_string(),
            owner: "owner".into(),
            name: "repo".into(),
            viewer: Viewer {
                login: "linewright".into(),
            },
            repository_id: Arc::default(),
            openings: Arc::default(),
            sending: Arc::default(),
            pushes_kept: Arc::default(),
            runs_listed: Arc::default(),
        }
    }

    fn answer(status: u16, headers: &[(&'static str, &str)], body: &str) -> Answer {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in headers {
            map.insert(*name, value.parse().unwrap());
        }
        Answer {
            status: StatusCode::from_u16(status).unwrap(),
            headers: map,
            body: body.to_string(),
        }
    }

    const UPLOAD: &str = "POST https://uploads.github.com/user-attachments/assets";

    /// As gh's `TestUpload` and `TestUploadAuthentication`
    /// (`internal/attachments/client_test.go` in cli/cli) expect it.
    #[test_log::test]
    fn an_attachment_is_uploaded_as_gh_uploads_it() {
        let request = github("github-token")
            .attach_request(1234, "shot.png", "image/png", b"the bytes".to_vec())
            .build()
            .unwrap();

        assert_eq!(request.method(), Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://uploads.github.com/user-attachments/assets?content_type=image%2Fpng&name=shot.png&repository_id=1234"
        );
        let header = |name: &str| request.headers()[name].to_str().unwrap().to_string();
        assert_eq!(header("Authorization"), "token github-token");
        assert_eq!(header("Content-Type"), "application/octet-stream");
        assert_eq!(header("Accept"), "application/vnd.github+json");
        assert_eq!(
            request.body().and_then(reqwest::Body::as_bytes),
            Some(&b"the bytes"[..])
        );
    }

    /// The answers in `fixtures/attachment-*.json` are gh's
    /// (`internal/attachments/client_test.go` in cli/cli), but for
    /// `attachment-forbidden.json`, whose message is GitHub's for a token
    /// that lacks a permission
    /// (<https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#resource-not-accessible>).
    #[test_log::test]
    fn an_uploaded_attachment_gives_its_url() {
        let url = attached(
            UPLOAD,
            answer(201, &[], include_str!("fixtures/attachment-uploaded.json")),
        );

        assert_eq!(
            url.unwrap(),
            "https://github.com/user-attachments/assets/be9b3920"
        );
    }

    #[test_log::test]
    fn a_file_github_refuses_is_refused_with_githubs_reason() {
        let refused = attached(
            UPLOAD,
            answer(422, &[], include_str!("fixtures/attachment-refused.json")),
        );

        let Err(AttachError::RefusedFile { reason }) = refused else {
            panic!("{refused:?}");
        };
        assert_eq!(
            reason,
            "Validation Failed; content_type is not included in the list of allowed content types"
        );
    }

    /// GitHub answers a token that cannot write with 404, as gh's
    /// `uploadPermissions` says.
    #[test_log::test]
    fn an_upload_not_found_refuses_the_token() {
        let refused = attached(
            UPLOAD,
            answer(404, &[], include_str!("fixtures/attachment-not-found.json")),
        );

        let Err(AttachError::RefusedToken { reason }) = refused else {
            panic!("{refused:?}");
        };
        assert_eq!(reason, "Not Found");
    }

    #[test_log::test]
    fn an_upload_forbidden_refuses_the_token() {
        let refused = attached(
            UPLOAD,
            answer(403, &[], include_str!("fixtures/attachment-forbidden.json")),
        );

        let Err(AttachError::RefusedToken { reason }) = refused else {
            panic!("{refused:?}");
        };
        assert_eq!(reason, "Resource not accessible by personal access token");
    }

    #[test_log::test(tokio::test(start_paused = true))]
    async fn a_rate_limited_upload_is_sent_again_once_the_limit_is_waited_out() {
        let start = tokio::time::Instant::now();

        let mut answers = vec![
            answer(
                429,
                &[("retry-after", "120")],
                include_str!("fixtures/attachment-rate-limited.json"),
            ),
            answer(201, &[], include_str!("fixtures/attachment-uploaded.json")),
        ]
        .into_iter();
        let mut sent = 0;

        let answered = waiting_out_rate_limits(UPLOAD, &Sending::default(), || {
            sent += 1;
            std::future::ready(Ok(answers.next().expect("an answer left")))
        })
        .await
        .unwrap();

        assert_eq!(
            attached(UPLOAD, answered).unwrap(),
            "https://github.com/user-attachments/assets/be9b3920"
        );
        assert_eq!(sent, 2);
        assert!(start.elapsed() >= std::time::Duration::from_secs(120));
    }

    /// A rate limit GitHub gives one request holds back the requests sent
    /// after it until it is waited out, as GitHub asks
    /// (<https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#exceeding-the-rate-limit>).
    #[test_log::test(tokio::test(start_paused = true))]
    async fn a_rate_limit_holds_back_the_requests_sent_after_it_until_it_is_waited_out() {
        let start = tokio::time::Instant::now();
        let sending = Sending::default();
        let mut answers = vec![
            answer(
                429,
                &[("retry-after", "120")],
                include_str!("fixtures/attachment-rate-limited.json"),
            ),
            answer(201, &[], include_str!("fixtures/attachment-uploaded.json")),
        ]
        .into_iter();
        let limited = waiting_out_rate_limits(UPLOAD, &sending, || {
            std::future::ready(Ok(answers.next().expect("an answer left")))
        });
        let mut sent_at = None;
        let after = async {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            waiting_out_rate_limits(UPLOAD, &sending, || {
                sent_at = Some(start.elapsed());
                std::future::ready(Ok(answer(
                    201,
                    &[],
                    include_str!("fixtures/attachment-uploaded.json"),
                )))
            })
            .await
        };

        let (limited, after) = tokio::join!(limited, after);

        limited.unwrap();
        after.unwrap();
        let sent_at = sent_at.unwrap();
        assert!(
            sent_at >= std::time::Duration::from_secs(120),
            "{sent_at:?}"
        );
    }
    /// [`Store::completed_attempts`] against a fake GitHub serving recorded
    /// workflow runs, as [`attempts_read`] checks it on each store.
    mod workflow_runs {
        use sha2::{Digest, Sha256};

        use super::*;
        use linewright_core::store::attempts_read::{self as read, LINEWRIGHT, REGISTER};

        /// What a fake GitHub holds of a repository's workflow runs.
        #[derive(Clone, Default)]
        struct Actions {
            /// Each run as GitHub gives it at its latest attempt, newest
            /// first once served.
            runs: Vec<Value>,
            /// Each earlier attempt as GitHub gives it, by its run and number.
            attempts: BTreeMap<(u64, u64), Value>,
        }

        /// The most runs GitHub lists for one filter
        /// (<https://docs.github.com/en/rest/actions/workflow-runs#list-workflow-runs-for-a-repository>).
        const LISTED_AT_MOST: usize = 1000;

        #[track_caller]
        fn recorded(json: &str) -> Value {
            serde_json::from_str(json).unwrap()
        }

        /// tibbe/linewright's run 40834616225 at its third attempt, with its
        /// first two.
        fn re_run_twice() -> Actions {
            Actions {
                runs: vec![recorded(
                    linewright_core::store::fixtures::WORKFLOW_RUN_RE_RUN_TWICE,
                )],
                attempts: BTreeMap::from([
                    (
                        (40_834_616_225, 1),
                        recorded(include_str!(
                            "fixtures/workflow-run-re-run-twice-attempt-1.json"
                        )),
                    ),
                    (
                        (40_834_616_225, 2),
                        recorded(include_str!(
                            "fixtures/workflow-run-re-run-twice-attempt-2.json"
                        )),
                    ),
                ]),
            }
        }

        fn fork_run_on_main() -> Value {
            recorded(linewright_core::store::fixtures::WORKFLOW_RUN_FORK_MAIN)
        }

        impl Actions {
            /// GitHub's answer to a listing of the runs with `query`: those
            /// created within its `created` range, in the order of `runs`,
            /// paged by its
            /// `per_page` and `page`, of at most [`LISTED_AT_MOST`], with the
            /// link to the next page, if any, under `base`.
            fn listing(&self, base: &str, path: &str, query: &str) -> (Value, Option<String>) {
                let params: BTreeMap<String, String> = form_urlencoded::parse(query.as_bytes())
                    .into_owned()
                    .collect();
                let range = &params["created"];
                let (from, to) = match range.strip_prefix(">=") {
                    Some(from) => (from, None),
                    None => {
                        let (from, to) = range.split_once("..").unwrap();
                        (from, Some(to))
                    }
                };
                let time = |time: &str| time.parse::<Timestamp>().unwrap();
                let matching: Vec<&Value> = self
                    .runs
                    .iter()
                    .filter(|run| {
                        let created = time(run["created_at"].as_str().unwrap());
                        created >= time(from) && to.is_none_or(|to| created <= time(to))
                    })
                    .collect();
                let per_page: usize = params.get("per_page").map_or(30, |n| n.parse().unwrap());
                let page: usize = params.get("page").map_or(1, |n| n.parse().unwrap());
                let listed = matching.len().min(LISTED_AT_MOST);
                let start = ((page - 1) * per_page).min(listed);
                let end = (page * per_page).min(listed);
                let body = json!({
                    "total_count": matching.len(),
                    "workflow_runs": matching[start..end],
                });
                let next = (end < listed).then(|| {
                    let mut params = params.clone();
                    params.insert("page".into(), (page + 1).to_string());
                    let next: String = form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(&params)
                        .finish();
                    format!("{base}{path}?{next}")
                });
                (body, next)
            }
        }

        /// Each request a fake GitHub answered: its path and query, and the
        /// status it answered with.
        type Answered = Arc<std::sync::Mutex<Vec<(String, u16)>>>;

        /// A GitHub store on `repo`, served by a fake GitHub holding
        /// `actions`, which gives each listing an `ETag` and answers a request
        /// sending it in `If-None-Match` with 304, as GitHub does
        /// (<https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api#use-conditional-requests-if-appropriate>);
        /// with what it answered.
        #[track_caller]
        fn serving_actions(
            repo: &str,
            actions: Actions,
        ) -> impl Future<Output = (GitHub, Answered)> {
            let caller = std::panic::Location::caller();
            let repo = repo.to_string();
            async move {
                use axum::http::{HeaderMap, StatusCode as Status, Uri};
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .unwrap_or_else(|e| panic!("serving GitHub's Actions at {caller}: {e}"));
                let base = format!("http://{}", listener.local_addr().unwrap());
                let answered = Answered::default();
                let runs = format!("/repos/{repo}/actions/runs");
                let mut actions = actions;
                actions
                    .runs
                    .sort_by_key(|run| std::cmp::Reverse(run["created_at"].to_string()));
                let actions = Arc::new(actions);
                let app = axum::Router::new().fallback({
                    let (base, answered) = (base.clone(), answered.clone());
                    move |uri: Uri, headers: HeaderMap| {
                        let (base, answered, runs) = (base.clone(), answered.clone(), runs.clone());
                        let actions = actions.clone();
                        async move {
                            let path = uri.path();
                            let query = uri.query().unwrap_or_default();
                            let (status, link, body) = if path == runs {
                                let (body, next) = actions.listing(&base, path, query);
                                let body = body.to_string();
                                let etag = format!("W/\"{}\"", hex::encode(Sha256::digest(&body)));
                                if headers
                                    .get("If-None-Match")
                                    .is_some_and(|sent| sent == &*etag)
                                {
                                    (Status::NOT_MODIFIED, None, (etag, String::new()))
                                } else {
                                    let link = next.map(|next| format!("<{next}>; rel=\"next\""));
                                    (Status::OK, link, (etag, body))
                                }
                            } else {
                                let attempt = path
                                    .strip_prefix(&format!("{runs}/"))
                                    .and_then(|rest| rest.split_once("/attempts/"))
                                    .and_then(|(run, n)| {
                                        actions.attempts.get(&(run.parse().ok()?, n.parse().ok()?))
                                    });
                                match attempt {
                                    Some(attempt) => {
                                        (Status::OK, None, (String::new(), attempt.to_string()))
                                    }
                                    None => (Status::NOT_FOUND, None, (String::new(), "{}".into())),
                                }
                            };
                            answered
                                .lock()
                                .unwrap()
                                .push((uri.to_string(), status.as_u16()));
                            let (etag, body) = body;
                            let mut answer = axum::http::Response::builder().status(status);
                            if !etag.is_empty() {
                                answer = answer.header("ETag", etag);
                            }
                            if let Some(link) = link {
                                answer = answer.header("Link", link);
                            }
                            answer.body(axum::body::Body::from(body)).unwrap()
                        }
                    }
                });
                tokio::spawn(async move { axum::serve(listener, app).await });
                let (owner, name) = repo.split_once('/').unwrap();
                let github = GitHub {
                    api: base,
                    owner: owner.into(),
                    name: name.into(),
                    ..github("token")
                };
                (github, answered)
            }
        }

        /// The requests answered for the path `path`, with their statuses.
        fn asked(answered: &Answered, path: &str) -> Vec<(String, u16)> {
            answered
                .lock()
                .unwrap()
                .iter()
                .filter(|(uri, _)| uri.split('?').next() == Some(path))
                .cloned()
                .collect()
        }

        #[test_log::test(tokio::test)]
        async fn every_attempt_of_a_run_re_run_twice_is_read_fetching_each_earlier_one_once() {
            let (github, answered) = serving_actions(LINEWRIGHT, re_run_twice()).await;

            read::every_attempt_of_a_run_re_run_twice_is_read(&github).await;

            for n in 1..=2 {
                let path = format!("/repos/{LINEWRIGHT}/actions/runs/40834616225/attempts/{n}");
                assert_eq!(asked(&answered, &path).len(), 1, "{path}");
            }
        }

        #[test_log::test(tokio::test)]
        async fn a_run_in_progress_is_left_out() {
            let mut actions = re_run_twice();
            actions.runs.push(recorded(include_str!(
                "fixtures/workflow-run-in-progress.json"
            )));
            let (github, _) = serving_actions(LINEWRIGHT, actions).await;

            read::every_attempt_of_a_run_re_run_twice_is_read(&github).await;
        }

        #[test_log::test(tokio::test)]
        async fn a_forks_run_on_a_branch_named_main_reads_with_no_branch() {
            let actions = Actions {
                runs: vec![fork_run_on_main()],
                ..Actions::default()
            };
            let (github, _) = serving_actions(REGISTER, actions).await;

            read::the_attempt_is_read(&github, read::fork_run_on_main()).await;
        }

        #[test_log::test(tokio::test)]
        async fn a_conclusion_outside_the_nine_reads_as_other() {
            let mut run = fork_run_on_main();
            run["conclusion"] = json!(read::UNKNOWN_CONCLUSION);
            let actions = Actions {
                runs: vec![run],
                ..Actions::default()
            };
            let (github, _) = serving_actions(REGISTER, actions).await;

            read::the_attempt_is_read(&github, read::concluded_outside_the_nine()).await;
        }

        /// GitHub answers 304 to a listing that has not changed since the
        /// `ETag` sent
        /// (<https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api#use-conditional-requests-if-appropriate>):
        /// a poll in the same hour lists the runs at the same URL.
        #[test_log::test(tokio::test)]
        async fn a_poll_with_nothing_new_is_answered_304() {
            let (github, answered) = serving_actions(LINEWRIGHT, re_run_twice()).await;

            read::a_poll_with_nothing_new_reads_the_same(&github).await;

            let listings = asked(&answered, &format!("/repos/{LINEWRIGHT}/actions/runs"));
            let statuses: Vec<u16> = listings.iter().map(|(_, status)| *status).collect();
            assert_eq!(statuses, [200, 304]);
            let query = listings[0].0.split_once('?').unwrap().1;
            let created = form_urlencoded::parse(query.as_bytes())
                .find(|(name, _)| name == "created")
                .unwrap()
                .1;
            assert_eq!(created, ">=2026-10-29T07:00:00Z");
        }

        /// GitHub lists at most 1000 runs for a filter
        /// (<https://docs.github.com/en/rest/actions/workflow-runs#list-workflow-runs-for-a-repository>),
        /// and lists none past them however many it counts.
        #[test_log::test(tokio::test)]
        async fn a_window_of_more_than_a_thousand_runs_is_read_in_narrower_ranges() {
            let run = recorded(linewright_core::store::fixtures::WORKFLOW_RUN_RE_RUN_TWICE);
            let since = read::re_run_created();
            let runs: Vec<Value> = (0..1200u64)
                .map(|n| {
                    let mut run = run.clone();
                    let created = since + TimeDelta::minutes(i64::try_from(n).unwrap());
                    run["id"] = json!(n + 1);
                    run["run_attempt"] = json!(1);
                    run["created_at"] = json!(query_time(created));
                    run
                })
                .collect();
            let (github, _) = serving_actions(
                LINEWRIGHT,
                Actions {
                    runs,
                    ..Actions::default()
                },
            )
            .await;

            let read = github
                .completed_attempts(since, &BTreeSet::new())
                .await
                .unwrap();

            let ids: BTreeSet<u64> = read.iter().map(|attempt| attempt.id.run.0).collect();
            assert_eq!(ids, (1..=1200).collect());
        }
    }
}

#[cfg(test)]
mod retry_tests {
    use std::cell::Cell;
    use std::time::Duration;

    use super::*;

    const CREATE: &str = "POST /repos/tibbe/linewright/issues";

    /// The error of a request whose connection closes before GitHub answers,
    /// as #934's did.
    #[track_caller]
    fn lost_answer() -> impl Future<Output = anyhow::Error> {
        let caller = std::panic::Location::caller();
        async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap_or_else(|e| panic!("binding at {caller}: {e}"));
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 1024];
                let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut request).await;
            });
            let github = GitHub {
                api: format!("http://{address}"),
                ..github()
            };
            let answered = github
                .rest_once::<Value>(
                    Method::POST,
                    "/repos/tibbe/linewright/issues",
                    Some(json!({})),
                )
                .await;
            answered.map_or_else(|e| e, |v| panic!("answered at {caller}: {v}"))
        }
    }

    fn github() -> GitHub {
        GitHub {
            http: reqwest::Client::new(),
            api: API.into(),
            token: "token".into(),
            owner: "tibbe".into(),
            name: "linewright".into(),
            viewer: Viewer {
                login: "tibbe-bot".into(),
            },
            repository_id: Arc::default(),
            openings: Arc::default(),
            sending: Arc::default(),
            pushes_kept: Arc::default(),
            runs_listed: Arc::default(),
        }
    }

    #[track_caller]
    fn answered(status: u16) -> anyhow::Error {
        ErrorResponse {
            request: CREATE.into(),
            status: StatusCode::from_u16(status).unwrap(),
            body: String::new(),
        }
        .into()
    }

    #[test_log::test(tokio::test(start_paused = true))]
    async fn a_lost_answer_whose_look_finds_the_write_gives_what_it_found_sending_nothing_again() {
        let mut tries = vec![Err(lost_answer().await)].into_iter();
        let sent = Cell::new(0);

        let made = retried_once(
            CREATE,
            || {
                sent.set(sent.get() + 1);
                std::future::ready(tries.next().expect("a try left"))
            },
            || async { Ok(Some(Number(934))) },
        )
        .await
        .unwrap();

        assert_eq!(made, Number(934));
        assert_eq!(sent.get(), 1);
    }

    #[test_log::test(tokio::test(start_paused = true))]
    async fn a_lost_answer_whose_look_finds_nothing_is_sent_once_more() {
        let mut tries = vec![Err(lost_answer().await), Ok(Number(934))].into_iter();
        let start = tokio::time::Instant::now();

        let made = retried_once(
            CREATE,
            || std::future::ready(tries.next().expect("a try left")),
            || async { Ok(None) },
        )
        .await
        .unwrap();

        assert_eq!(made, Number(934));
        assert!(start.elapsed() >= Duration::from_secs(1));
    }

    #[test_log::test(tokio::test(start_paused = true))]
    async fn a_server_error_is_retried_once() {
        let mut tries = vec![Err(answered(502)), Ok(Number(934))].into_iter();

        let made = retried_once(
            CREATE,
            || std::future::ready(tries.next().expect("a try left")),
            finds_nothing,
        )
        .await;

        assert_eq!(made.unwrap(), Number(934));
    }

    #[test_log::test(tokio::test(start_paused = true))]
    async fn a_refusal_is_not_retried() {
        let mut tries = vec![Err(answered(422))].into_iter();
        let looked = Cell::new(false);

        let made: Result<Number> = retried_once(
            CREATE,
            || std::future::ready(tries.next().expect("a try left")),
            || async {
                looked.set(true);
                Ok(None)
            },
        )
        .await;

        let refusal = made.unwrap_err();
        assert_eq!(
            refusal.downcast_ref::<ErrorResponse>().unwrap().status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(!looked.get());
    }

    #[test_log::test(tokio::test(start_paused = true))]
    async fn a_second_failure_fails_saying_both() {
        let mut tries = vec![Err(lost_answer().await), Err(answered(503))].into_iter();

        let made: Result<Number> = retried_once(
            CREATE,
            || std::future::ready(tries.next().expect("a try left")),
            finds_nothing,
        )
        .await;

        let error = format!("{:#}", made.unwrap_err());
        assert!(
            error.starts_with(&format!("{CREATE} failed twice")),
            "{error}"
        );
        assert!(
            error.contains("connection closed before message completed"),
            "{error}"
        );
        assert!(error.contains("503 Service Unavailable"), "{error}");
    }

    #[test_log::test(tokio::test(start_paused = true))]
    async fn a_failed_look_fails_saying_what_the_write_met() {
        let mut tries = vec![Err(lost_answer().await)].into_iter();

        let made: Result<Number> = retried_once(
            CREATE,
            || std::future::ready(tries.next().expect("a try left")),
            || async { Err(answered(500)) },
        )
        .await;

        let error = format!("{:#}", made.unwrap_err());
        assert!(
            error.contains("connection closed before message completed"),
            "{error}"
        );
        assert!(error.contains("500 Internal Server Error"), "{error}");
    }

    #[track_caller]
    fn at(time: &str) -> Timestamp {
        time.parse().unwrap()
    }

    /// `fixtures/issues-created.json` is what GitHub returned for the six
    /// issues and pull requests `tibbe-bot` opened last on
    /// <https://github.com/tibbe/linewright>, newest first, cut here to the
    /// fields read. Among them is #934, whose answer was lost.
    #[track_caller]
    fn issues_created() -> Vec<ListedIssue> {
        serde_json::from_str(include_str!("fixtures/issues-created.json")).unwrap()
    }

    /// The issue that would have been opened as the `index`th of
    /// `fixtures/issues-created.json`.
    #[track_caller]
    fn opened_as(index: usize) -> NewIssue {
        let listed: Vec<Value> =
            serde_json::from_str(include_str!("fixtures/issues-created.json")).unwrap();
        NewIssue {
            title: listed[index]["title"].as_str().unwrap().into(),
            body: listed[index]["body"].as_str().unwrap().into(),
            labels: vec!["ready-for-human".into()],
        }
    }

    #[track_caller]
    fn issue_934() -> NewIssue {
        opened_as(2)
    }

    #[test_log::test]
    fn an_issue_linewright_opened_since_the_request_is_found_by_its_title_and_body() {
        let made = issue_made(
            issues_created(),
            &Login::new("tibbe-bot"),
            &issue_934(),
            at("2026-11-01T09:29:22Z"),
        );

        assert_eq!(made, Some(Number(934)));
    }

    #[test_log::test]
    fn an_issue_someone_else_opened_is_not_taken_for_linewrights() {
        let made = issue_made(
            issues_created(),
            &Login::new("tibbe"),
            &issue_934(),
            at("2026-11-01T09:29:22Z"),
        );

        assert_eq!(made, None);
    }

    #[test_log::test]
    fn an_issue_opened_before_the_request_is_not_taken_for_its() {
        let made = issue_made(
            issues_created(),
            &Login::new("tibbe-bot"),
            &issue_934(),
            at("2026-11-01T09:31:00Z"),
        );

        assert_eq!(made, None);
    }

    #[test_log::test]
    fn a_pull_request_is_not_taken_for_an_issue() {
        let pull_935 = opened_as(0);

        let made = issue_made(
            issues_created(),
            &Login::new("tibbe-bot"),
            &pull_935,
            at("2026-11-01T09:45:00Z"),
        );

        assert_eq!(made, None);
    }

    /// `fixtures/issue-comments.json` is what GitHub returned for the
    /// comments on <https://github.com/tibbe/linewright/issues/930>, cut
    /// here to the fields read.
    #[track_caller]
    fn comments_on_930() -> Vec<ListedComment> {
        serde_json::from_str(include_str!("fixtures/issue-comments.json")).unwrap()
    }

    const HANDED_BACK: &str = "Recording the run's outcome failed: POST /repos/tibbe/linewright/issues: error sending request for url (https://api.github.com/repos/tibbe/linewright/issues): client error (SendRequest): connection closed before message completed";

    #[test_log::test]
    fn a_comment_linewright_made_since_the_request_is_found_by_its_body() {
        assert!(commented(
            comments_on_930(),
            &Login::new("tibbe-bot"),
            HANDED_BACK,
            at("2026-11-01T09:29:23Z")
        ));
    }

    #[test_log::test]
    fn a_comment_with_another_body_another_author_or_made_before_the_request_is_not_found() {
        let since = at("2026-11-01T09:29:23Z");

        assert!(!commented(
            comments_on_930(),
            &Login::new("tibbe-bot"),
            "Another body",
            since
        ));
        assert!(!commented(
            comments_on_930(),
            &Login::new("tibbe"),
            HANDED_BACK,
            since
        ));
        assert!(!commented(
            comments_on_930(),
            &Login::new("tibbe-bot"),
            HANDED_BACK,
            at("2026-11-01T09:31:00Z")
        ));
    }

    /// `fixtures/pulls-by-head.json` is what GitHub returned for the pull
    /// requests from `tibbe:931-work-a-pull-request-by-a-facts-record-an` on
    /// <https://github.com/tibbe/linewright>, cut here to the fields read.
    #[test_log::test]
    fn a_pull_request_is_found_by_its_head_when_linewright_opened_it() {
        let listed = || -> Vec<ListedPull> {
            serde_json::from_str(include_str!("fixtures/pulls-by-head.json")).unwrap()
        };

        assert_eq!(
            pull_made(listed(), &Login::new("tibbe-bot")),
            Some(Number(935))
        );
        assert_eq!(pull_made(listed(), &Login::new("tibbe")), None);
    }

    /// `fixtures/thread-comments.json` is what GitHub returned for the
    /// comments of a review thread on
    /// <https://github.com/tibbe/linewright/pull/862>: `tibbe`'s, then
    /// `tibbe-bot`'s reply.
    #[track_caller]
    fn thread_on_862() -> Vec<ThreadComment> {
        let response: GqlResponse<ThreadData> =
            serde_json::from_str(include_str!("fixtures/thread-comments.json")).unwrap();
        response.data.unwrap().node.comments.nodes
    }

    #[test_log::test]
    fn a_reply_linewright_made_since_the_request_is_found_by_its_body() {
        let since = at("2026-10-30T20:50:00Z");

        assert!(replied(
            thread_on_862(),
            &Login::new("tibbe-bot"),
            "Removed the comment from the test step.",
            since
        ));
        assert!(!replied(
            thread_on_862(),
            &Login::new("tibbe-bot"),
            "Don't narrate code or config.",
            since
        ));
        assert!(!replied(
            thread_on_862(),
            &Login::new("tibbe-bot"),
            "Removed the comment from the test step.",
            at("2026-10-30T20:52:00Z")
        ));
    }

    /// `fixtures/sub-issues-listed.json` and `fixtures/blocked-by.json` are
    /// what GitHub returned for the sub-issues of
    /// <https://github.com/tibbe/linewright/issues/900> and the issues
    /// blocking #744, cut here to the fields read.
    #[test_log::test]
    fn a_link_is_found_by_the_linked_issues_id() {
        let sub_issues = || -> Vec<ListedIssueId> {
            serde_json::from_str(include_str!("fixtures/sub-issues-listed.json")).unwrap()
        };
        let blocked_by: Vec<ListedIssueId> =
            serde_json::from_str(include_str!("fixtures/blocked-by.json")).unwrap();

        let issue_903_id = 6_142_173_193;
        let issue_743_id = 6_080_099_910;
        assert!(linked_already(&sub_issues(), issue_903_id));
        assert!(!linked_already(&sub_issues(), issue_743_id));
        assert!(linked_already(&blocked_by, issue_743_id));
    }
}

#[cfg(test)]
mod recording_tests {
    use axum::extract::{Path as UrlPath, Query};
    use axum::http::{HeaderMap, StatusCode as Status};

    use super::*;

    /// What GitHub answered, read with [`API_VERSION`], for each REST path
    /// under tibbe/linewright that recording #1130, #1155 and #1158 and the
    /// heads of `main` and `1158-no-such-branch` reads, on 2026-11-12, and
    /// that recording the properties reads, on 2026-11-15: each listing's
    /// first page, and its second as none.
    fn recorded(path: &str, page: Option<&str>) -> Option<(Status, &'static str)> {
        let found = |body| Some((Status::OK, body));
        let parentless = Some((
            Status::NOT_FOUND,
            include_str!("fixtures/rest-parent-not-found.json"),
        ));
        match (path, page) {
            (_, Some("2")) => found("[]"),
            ("issues/1158", None) => found(linewright_core::store::fixtures::REST_ISSUE_1158),
            ("issues/1158/comments", Some("1")) => {
                found(linewright_core::store::fixtures::REST_ISSUE_1158_COMMENTS)
            }
            ("issues/1158/parent", None) => {
                found(linewright_core::store::fixtures::REST_ISSUE_1158_PARENT)
            }
            ("issues/1158/dependencies/blocked_by", Some("1")) => {
                found(linewright_core::store::fixtures::REST_ISSUE_1158_BLOCKED_BY)
            }
            ("issues/1130", None) => found(linewright_core::store::fixtures::REST_ISSUE_1130),
            ("issues/1130/comments", Some("1")) => {
                found(linewright_core::store::fixtures::REST_ISSUE_1130_COMMENTS)
            }
            ("issues/1130/dependencies/blocked_by", Some("1")) => {
                found(linewright_core::store::fixtures::REST_ISSUE_1130_BLOCKED_BY)
            }
            ("issues/1155", None) => found(linewright_core::store::fixtures::REST_ISSUE_1155),
            ("issues/1155/comments", Some("1")) => {
                found(linewright_core::store::fixtures::REST_ISSUE_1155_COMMENTS)
            }
            ("issues/1155/dependencies/blocked_by", Some("1")) => {
                found(linewright_core::store::fixtures::REST_ISSUE_1155_BLOCKED_BY)
            }
            ("pulls/1155", None) => found(linewright_core::store::fixtures::REST_PULL_1155),
            ("issues/1130/parent" | "issues/1155/parent", None) => parentless,
            ("git/ref/heads/main", None) => found(include_str!("fixtures/rest-ref-main.json")),
            ("rules/branches/main", None) => {
                found(linewright_core::store::fixtures::REST_RULES_MAIN)
            }
            ("branches/main/protection", None) => Some((
                Status::NOT_FOUND,
                include_str!("fixtures/rest-protection-not-found.json"),
            )),
            ("git/ref/heads/1158-no-such-branch", None) => Some((
                Status::NOT_FOUND,
                include_str!("fixtures/rest-ref-not-found.json"),
            )),
            _ => None,
        }
    }

    /// A GitHub store on tibbe/linewright, served [`recorded`]'s answers to
    /// requests made with [`API_VERSION`], and refused any other.
    #[track_caller]
    fn serving_recorded() -> impl Future<Output = GitHub> {
        serving_recorded_with_rules(None)
    }

    /// [`serving_recorded`]'s store, answering a request for `main`'s rules
    /// with `rules` in place of the recorded list, when given.
    #[track_caller]
    fn serving_recorded_with_rules(
        rules: Option<(Status, &'static str)>,
    ) -> impl Future<Output = GitHub> {
        let caller = std::panic::Location::caller();
        async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap_or_else(|e| panic!("serving the recorded answers at {caller}: {e}"));
            serving_on(listener, rules)
        }
    }

    /// What GitHub answered, read with [`API_VERSION`], for `main`'s rules on
    /// tibbe/linewright-contract, a private repository on a free plan, on
    /// 2026-11-16.
    fn rules_refused_by_plan() -> (Status, &'static str) {
        (
            Status::FORBIDDEN,
            include_str!("fixtures/rest-rules-refused-by-plan.json"),
        )
    }

    /// [`serving_recorded_with_rules`]'s store, served on `listener`, with
    /// the repository, tibbe-bot's account and the answer to
    /// [`properties::ISSUE_TYPES_AND_MERGE_METHODS_QUERY`] as GitHub gave them on 2026-11-15,
    /// and `main`'s rules as `rules`, when given.
    fn serving_on(
        listener: tokio::net::TcpListener,
        rules: Option<(Status, &'static str)>,
    ) -> GitHub {
        fn pinned(
            headers: &HeaderMap,
            answer: Option<(Status, &'static str)>,
        ) -> (Status, &'static str) {
            if headers
                .get("X-GitHub-Api-Version")
                .map(axum::http::HeaderValue::as_bytes)
                != Some(API_VERSION.as_bytes())
            {
                return (Status::BAD_REQUEST, "unpinned API version");
            }
            answer.unwrap_or((Status::IM_A_TEAPOT, "not recorded"))
        }
        async fn answer(
            UrlPath(path): UrlPath<String>,
            Query(query): Query<BTreeMap<String, String>>,
            headers: HeaderMap,
        ) -> (Status, &'static str) {
            pinned(
                &headers,
                recorded(&path, query.get("page").map(String::as_str)),
            )
        }
        let fixed = |body: &'static str| {
            move |headers: HeaderMap| async move { pinned(&headers, Some((Status::OK, body))) }
        };
        let address = listener
            .local_addr()
            .expect("a bound listener has an address");
        let mut app = axum::Router::new().route(
            "/repos/tibbe/linewright/{*path}",
            axum::routing::get(answer),
        );
        if let Some(rules) = rules {
            app = app.route(
                "/repos/tibbe/linewright/rules/branches/main",
                axum::routing::get(move |headers: HeaderMap| async move {
                    pinned(&headers, Some(rules))
                }),
            );
        }
        let app = app
            .route(
                "/repos/tibbe/linewright",
                axum::routing::get(fixed(linewright_core::store::fixtures::REST_REPOSITORY)),
            )
            .route(
                "/user",
                axum::routing::get(fixed(linewright_core::store::fixtures::REST_USER)),
            )
            .route(
                "/graphql",
                axum::routing::post(fixed(
                    linewright_core::store::fixtures::GRAPHQL_ISSUE_TYPES_AND_MERGE_METHODS,
                )),
            );
        tokio::spawn(async move { axum::serve(listener, app).await });
        GitHub {
            http: reqwest::Client::new(),
            api: format!("http://{address}"),
            token: "token".into(),
            owner: "tibbe".into(),
            name: "linewright".into(),
            viewer: Viewer {
                login: "tibbe-bot".into(),
            },
            repository_id: Arc::default(),
            openings: Arc::default(),
            sending: Arc::default(),
            pushes_kept: Arc::default(),
            runs_listed: Arc::default(),
        }
    }

    #[track_caller]
    fn resource(recorded: &str) -> Value {
        serde_json::from_str(recorded).unwrap()
    }

    #[test_log::test(tokio::test)]
    async fn items_are_recorded_as_githubs_rest_api_gives_them() {
        let github = serving_recorded().await;
        let numbers = [1130, 1155, 1158].into_iter().map(Number).collect();

        let items = github.record_items(&numbers).await.unwrap();

        let item = |number: u64, resources: &[(&str, &str)]| RecordedItem {
            number: Number(number),
            resources: resources
                .iter()
                .map(|(path, recorded)| (path.to_string(), resource(recorded)))
                .collect(),
        };
        assert_eq!(
            items,
            [
                item(
                    1130,
                    &[
                        (
                            "issues/1130",
                            linewright_core::store::fixtures::REST_ISSUE_1130
                        ),
                        (
                            "issues/1130/comments",
                            linewright_core::store::fixtures::REST_ISSUE_1130_COMMENTS
                        ),
                        (
                            "issues/1130/dependencies/blocked_by",
                            linewright_core::store::fixtures::REST_ISSUE_1130_BLOCKED_BY
                        ),
                    ]
                ),
                item(
                    1155,
                    &[
                        (
                            "issues/1155",
                            linewright_core::store::fixtures::REST_ISSUE_1155
                        ),
                        (
                            "issues/1155/comments",
                            linewright_core::store::fixtures::REST_ISSUE_1155_COMMENTS
                        ),
                        (
                            "issues/1155/dependencies/blocked_by",
                            linewright_core::store::fixtures::REST_ISSUE_1155_BLOCKED_BY
                        ),
                        (
                            "pulls/1155",
                            linewright_core::store::fixtures::REST_PULL_1155
                        ),
                    ]
                ),
                item(
                    1158,
                    &[
                        (
                            "issues/1158",
                            linewright_core::store::fixtures::REST_ISSUE_1158
                        ),
                        (
                            "issues/1158/comments",
                            linewright_core::store::fixtures::REST_ISSUE_1158_COMMENTS
                        ),
                        (
                            "issues/1158/parent",
                            linewright_core::store::fixtures::REST_ISSUE_1158_PARENT
                        ),
                        (
                            "issues/1158/dependencies/blocked_by",
                            linewright_core::store::fixtures::REST_ISSUE_1158_BLOCKED_BY
                        ),
                    ]
                ),
            ]
        );
    }

    #[test_log::test(tokio::test)]
    async fn the_properties_are_recorded_as_github_gives_them() {
        let github = serving_recorded().await;

        let recorded = github.record_properties().await.unwrap();

        let settings =
            resource(linewright_core::store::fixtures::GRAPHQL_ISSUE_TYPES_AND_MERGE_METHODS);
        assert_eq!(
            recorded,
            [
                (
                    Property::Repository,
                    resource(linewright_core::store::fixtures::REST_REPOSITORY)
                ),
                (
                    Property::Account,
                    resource(linewright_core::store::fixtures::REST_USER)
                ),
                (
                    Property::IssueTypesAndMergeMethods,
                    settings["data"].clone()
                ),
                (
                    Property::Rules,
                    resource(linewright_core::store::fixtures::REST_RULES_MAIN)
                ),
                (Property::Protection, Value::Null),
            ]
            .into_iter()
            .collect()
        );
    }

    #[test_log::test(tokio::test)]
    async fn the_rules_are_recorded_as_null_when_github_refuses_them_for_the_plan() {
        let github = serving_recorded_with_rules(Some(rules_refused_by_plan())).await;

        let recorded = github.record_properties().await.unwrap();

        assert_eq!(recorded.resource(Property::Rules), Some(&Value::Null));
    }

    #[test_log::test(tokio::test)]
    async fn recording_the_properties_fails_when_github_refuses_the_rules_for_another_reason() {
        let github = serving_recorded_with_rules(Some((
            Status::FORBIDDEN,
            include_str!("fixtures/attachment-forbidden.json"),
        )))
        .await;

        let error = github.record_properties().await.unwrap_err();

        assert_eq!(
            error.downcast_ref::<ErrorResponse>().unwrap().status,
            StatusCode::FORBIDDEN
        );
    }

    #[test_log::test(tokio::test)]
    async fn a_branch_has_no_rules_when_github_refuses_them_for_the_plan() {
        let github = serving_recorded_with_rules(Some(rules_refused_by_plan())).await;

        let rules = github.branch_rules("main", &RulesRead::default()).await;

        assert_eq!(*rules, BranchRules::default());
    }

    #[test_log::test(tokio::test)]
    async fn the_settings_and_identity_are_read_from_the_recorded_properties() {
        let github = serving_recorded().await;

        let settings = github.settings().await.unwrap();
        let identity = github.identity().await.unwrap();

        assert_eq!(settings.repo, "tibbe/linewright".parse::<Repo>().unwrap());
        assert_eq!(identity.login, Login::new("tibbe-bot"));
    }

    #[test_log::test(tokio::test)]
    async fn the_heads_are_of_the_branches_github_has() {
        let github = serving_recorded().await;

        let heads = github
            .heads(&["main".into(), "1158-no-such-branch".into()])
            .await
            .unwrap();

        assert_eq!(
            heads,
            BTreeMap::from([(
                "main".to_string(),
                git::CommitId::from("907d6f89ea31d416ede8b5230526b6d9b02c3901")
            )])
        );
    }
}
