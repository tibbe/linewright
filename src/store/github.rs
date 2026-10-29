use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use base64::Engine;
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    AnyItem, AttachError, Blob, Categories, Identity, IssueRead, NewIssue, NewPull, PushError,
    PushMode, RepoSettings, Store,
};
use super::{job_log, rate_limit};
use crate::domain::{
    Assignment, BranchIssue, Check, CheckFailure, CheckLog, CheckState, ClosedItem, ClosedKind,
    ClosingPull, Comment, Filed, Issue, IssueCloseReason, IssueEvent, JobLog, Linked, Login,
    MergeMethod, Mergeability, MergedPull, Number, Parent, Pull, PullState, Push, Rebaseability,
    Repo, Requirement, Review, ReviewDecision, ReviewState, Snapshot, SubIssue, SubIssueState,
    Thread, TicketBranch, TicketBranchName, Timestamp, Windows, WorkFacts, closed_within, label,
};
use crate::git;

const API: &str = "https://api.github.com";

/// Where attachments are uploaded, as gh uploads them (`internal/attachments`
/// in <https://github.com/cli/cli>).
const ATTACHMENTS: &str = "https://uploads.github.com/user-attachments/assets";

/// In `gh`'s order (<https://cli.github.com/manual/gh_help_environment>).
pub const TOKEN_VARIABLES: [&str; 2] = ["GH_TOKEN", "GITHUB_TOKEN"];

/// The token agent sessions are given in place of Linewright's.
pub const SESSION_TOKEN_VARIABLE: &str = "LINEWRIGHT_SESSION_GITHUB_TOKEN";

#[derive(Clone)]
pub struct GitHub {
    http: reqwest::Client,
    token: String,
    owner: String,
    name: String,
    viewer: Viewer,
    /// The repository's numeric id, read once.
    repository_id: Arc<tokio::sync::OnceCell<u64>>,
}

macro_rules! closes_field {
    () => {
        "closingIssuesReferences(first: 20) { nodes { number } }"
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
    async fn read(response: reqwest::Response) -> Result<Answer> {
        Ok(Answer {
            status: response.status(),
            headers: response.headers().clone(),
            body: response.text().await?,
        })
    }
}

/// The answer to `request`, which `send` makes, made again once each rate
/// limit is waited out, at most [`rate_limit::RETRIES`] times: a rate limit
/// still in force after them fails it.
async fn waiting_out_rate_limits<F, Fut>(request: &str, mut send: F) -> Result<Answer>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Answer>>,
{
    let mut retries = 0;
    loop {
        let answer = send().await?;
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
                tokio::time::sleep(wait).await;
                retries += 1;
            }
            Some(_) => {
                return Err(ErrorResponse {
                    request: request.to_string(),
                    status: answer.status,
                    body: answer.body,
                }
                .into());
            }
        }
    }
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

#[derive(Clone, Deserialize)]
struct Viewer {
    login: String,
    id: u64,
}

impl GitHub {
    pub async fn connect(repo: &Repo, token: String) -> Result<GitHub> {
        // Follows "Certificate authorities" in docs/specs/operation.md:
        // reqwest's `rustls` feature verifies through the platform's
        // verifier, which reads the host's store alone. A feature or call
        // that adds roots, such as `webpki-roots`, `tls_certs_merge` or
        // `add_root_certificate`, would break it.
        let http = reqwest::Client::builder()
            .user_agent("linewright")
            .build()?;
        let mut github = GitHub {
            http,
            token,
            owner: repo.owner().into(),
            name: repo.name().into(),
            viewer: Viewer {
                login: String::new(),
                id: 0,
            },
            repository_id: Arc::default(),
        };
        github.viewer = github
            .rest(Method::GET, "/user", None)
            .await
            .context("reading the token's user")?;
        Ok(github)
    }

    pub async fn repo_get(&self, path: &str) -> Result<Value> {
        self.rest(Method::GET, &self.repo_path(path), None).await
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

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<reqwest::Response> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{API}{path}")
        };
        let mut request = self
            .http
            .request(method.clone(), &url)
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(body) = body {
            request = request.json(&body);
        }
        request
            .send()
            .await
            .with_context(|| format!("{method} {path}"))
    }

    async fn rest<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let request = format!("{method} {path}");
        let answer = waiting_out_rate_limits(&request, || async {
            Answer::read(self.send(method.clone(), path, body.clone()).await?).await
        })
        .await?;
        if !answer.status.is_success() {
            return Err(ErrorResponse {
                request,
                status: answer.status,
                body: answer.body,
            }
            .into());
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

    async fn rest_ok(&self, method: Method, path: &str, body: Option<Value>) -> Result<()> {
        self.rest::<Value>(method, path, body).await.map(|_| ())
    }

    async fn graphql<T: DeserializeOwned>(&self, query: &str, variables: Value) -> Result<T> {
        let response: GqlResponse<T> = self.graphql_response(query, variables).await?;
        if !response.errors.is_empty() {
            bail!("GraphQL errors: {}", Value::Array(response.errors));
        }
        response.data.context("GraphQL returned no data")
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

    /// Passed through the environment so the token never lands in a clone's
    /// configuration.
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

    /// The repository's numeric id, which the upload of an attachment names.
    async fn repository_id(&self) -> Result<u64> {
        #[derive(Deserialize)]
        struct Id {
            id: u64,
        }
        self.repository_id
            .get_or_try_init(|| async {
                let repo: Id = self.rest(Method::GET, &self.repo_path(""), None).await?;
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

    /// The issues `filter` finds, each read with `fields`.
    async fn issues<T: DeserializeOwned>(&self, fields: Fragment, filter: Value) -> Result<Vec<T>> {
        #[derive(Deserialize)]
        struct IssuesData<T> {
            issues: Page<T>,
        }
        let Fragment { name, text } = fields;
        let query = format!(
            "query($owner: String!, $name: String!, $filter: IssueFilters, $cursor: String) {{
              repository(owner: $owner, name: $name) {{
                issues(first: 20, after: $cursor, filterBy: $filter) {{
                  pageInfo {{ hasNextPage endCursor }}
                  nodes {{ ...{name} }}
                }}
              }}
            }}
            {text}"
        );
        let mut issues = vec![];
        let mut cursor = Value::Null;
        loop {
            let data: RepoData<IssuesData<T>> = self
                .graphql(
                    &query,
                    json!({"owner": self.owner, "name": self.name, "filter": filter, "cursor": cursor}),
                )
                .await?;
            let page = data.repository.issues;
            issues.extend(page.nodes);
            if !page.page_info.has_next_page {
                return Ok(issues);
            }
            cursor = json!(page.page_info.end_cursor);
        }
    }

    /// The closed issues updated since `since`, which hold every one closed
    /// since then, that the snapshot's filters would hold were they open.
    async fn closed_issues(&self, since: Timestamp) -> Result<Vec<ClosedItem>> {
        let linewright = &self.viewer.login;
        let mut closed: BTreeMap<Number, ClosedItem> = BTreeMap::new();
        // Closing updates an issue
        // (<https://docs.github.com/en/graphql/reference/input-objects#issuefilters>).
        for mut filter in in_view_filters(IssueState::Closed, linewright) {
            filter["since"] = json!(since);
            let issues: Vec<GqlClosedIssue> = self.issues(CLOSED_ISSUE_FIELDS, filter).await?;
            for issue in issues {
                if let Some(item) = issue.into_closed(&self.linewright()) {
                    closed.insert(item.number, item);
                }
            }
        }
        Ok(closed.into_values().collect())
    }

    /// The closed issues of `numbers` that the snapshot's filters would hold
    /// were they open, as [`Store::issue`] reads them, twenty to a query by
    /// aliases, which costs about a point
    /// (<https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api#calculating-a-rate-limit-score-before-running-the-call>).
    async fn closed_issues_numbered(&self, numbers: &BTreeSet<Number>) -> Result<Vec<ClosedItem>> {
        let numbers: Vec<Number> = numbers.iter().copied().collect();
        let mut closed = vec![];
        for batch in numbers.chunks(20) {
            let response: GqlResponse<RepoData<BTreeMap<String, Option<GqlClosedIssue>>>> = self
                .graphql_response(
                    &numbered_query(batch, CLOSED_ISSUE_FIELDS),
                    json!({"owner": self.owner, "name": self.name}),
                )
                .await?;
            closed.extend(closed_of_numbered(response, &self.linewright())?);
        }
        Ok(closed)
    }

    /// Linewright's pull requests closed without merging, among those
    /// updated since `since`.
    async fn closed_pulls(&self, since: Timestamp) -> Result<Vec<ClosedItem>> {
        // Fifty pull requests with one connection each cost 51 requests,
        // which rounds to 1 point.
        let query = concat!(
            "query($owner: String!, $name: String!, $cursor: String) {
              repository(owner: $owner, name: $name) {
                pullRequests(first: 50, after: $cursor, states: [CLOSED],
                             orderBy: {field: UPDATED_AT, direction: DESC}) {
                  pageInfo { hasNextPage endCursor }
                  nodes { number title closedAt updatedAt author { __typename login } ",
            closes_field!(),
            " }
                }
              }
            }"
        );
        let mut closed = vec![];
        let mut cursor = Value::Null;
        loop {
            let data: RepoData<PullsData<GqlClosedPull>> = self
                .graphql(
                    query,
                    json!({"owner": self.owner, "name": self.name, "cursor": cursor}),
                )
                .await?;
            let page = data.repository.pull_requests;
            let mut older = false;
            for pr in page.nodes {
                // Closing updates a pull request, so ordering by update time
                // finds everything closed since the cutoff.
                older |= pr.updated_at < since;
                closed.extend(pr.into_closed(&self.linewright()));
            }
            if older || !page.page_info.has_next_page {
                return Ok(closed);
            }
            cursor = json!(page.page_info.end_cursor);
        }
    }

    /// Number and author only: the full fields cost points per pull request,
    /// so only Linewright's are read in full.
    async fn open_pulls(&self) -> Result<Vec<GqlOpenPull>> {
        let query = "query($owner: String!, $name: String!, $cursor: String) {
              repository(owner: $owner, name: $name) {
                pullRequests(first: 100, after: $cursor, states: [OPEN]) {
                  pageInfo { hasNextPage endCursor }
                  nodes { number author { __typename login } }
                }
              }
            }";
        let mut pulls = vec![];
        let mut cursor = Value::Null;
        loop {
            let data: RepoData<PullsData<GqlOpenPull>> = self
                .graphql(
                    query,
                    json!({"owner": self.owner, "name": self.name, "cursor": cursor}),
                )
                .await?;
            let page = data.repository.pull_requests;
            pulls.extend(page.nodes);
            if !page.page_info.has_next_page {
                return Ok(pulls);
            }
            cursor = json!(page.page_info.end_cursor);
        }
    }

    async fn ticket_branches(
        &self,
        open_in_view: &BTreeMap<u64, GqlIssue>,
    ) -> Result<Vec<TicketBranch>> {
        #[derive(Deserialize)]
        struct Refs {
            refs: Page<GqlRef>,
        }
        let query = "query($owner: String!, $name: String!, $cursor: String) {
              repository(owner: $owner, name: $name) {
                refs(refPrefix: \"refs/heads/\", first: 100, after: $cursor) {
                  pageInfo { hasNextPage endCursor }
                  nodes {
                    name
                    target { ... on Commit { committer { user { login } } } }
                    associatedPullRequests(states: [OPEN], first: 1) { totalCount }
                  }
                }
              }
            }";
        let mut branches = vec![];
        let mut cursor = Value::Null;
        loop {
            let data: RepoData<Refs> = self
                .graphql(
                    query,
                    json!({"owner": self.owner, "name": self.name, "cursor": cursor}),
                )
                .await?;
            let page = data.repository.refs;
            for branch in page.nodes {
                let Some(name) = branch.pushed_for(&self.viewer.login) else {
                    continue;
                };
                let issue = if open_in_view.contains_key(&name.issue().0) {
                    BranchIssue::Held
                } else {
                    // No issue, as when the number is a pull request's, names
                    // no ticket.
                    let Some(read) = self.gql_issue(name.issue()).await? else {
                        continue;
                    };
                    BranchIssue::Read(Box::new(read.into_issue(&self.linewright())))
                };
                branches.push(TicketBranch {
                    open_pull_from_branch: branch.open_pull(),
                    name,
                    issue,
                });
            }
            if !page.page_info.has_next_page {
                return Ok(branches);
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
        let query = "query($owner: String!, $name: String!, $base: String!, $cursor: String) {
              repository(owner: $owner, name: $name) {
                pullRequests(first: 25, after: $cursor, states: [MERGED], baseRefName: $base,
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
            let mut older = false;
            for pr in page.nodes {
                // Merging updates a pull request, so ordering by update time
                // finds everything merged since the cutoff.
                if pr.updated_at < windows.read_since() {
                    older = true;
                    continue;
                }
                if let Some(pr) = self.merged_view(pr, base, windows).await? {
                    merged.push(pr);
                }
            }
            if older || !page.page_info.has_next_page {
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
        // Before `merged_of`, so that files are read only for those held.
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

    /// Oldest first
    /// (<https://docs.github.com/en/rest/repos/repos#list-repository-activities>).
    async fn pushes(&self, branch: &str) -> Result<Vec<Push>> {
        #[derive(Deserialize)]
        struct Activity {
            timestamp: Timestamp,
            #[serde(rename = "activity_type")]
            kind: String,
            actor: Option<RestUser>,
        }
        let path = self.repo_path(&format!("/activity?ref=refs/heads/{branch}&per_page=30"));
        let activity: Vec<Activity> = self.rest(Method::GET, &path, None).await?;
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

    async fn tracking_issues_of(&self, spec_change: Number) -> Result<Vec<Number>> {
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
        Ok(tracking_issues_among(
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

    async fn pull_view(&self, pr: GqlPull) -> Result<Pull> {
        let pushes = if pr.state == "OPEN" {
            self.pushes(&pr.head_ref_name).await?
        } else {
            vec![]
        };
        let rules = if pr.merge_state_status == "BLOCKED" {
            self.branch_rules(&pr.base_ref_name).await
        } else {
            BranchRules::default()
        };
        Ok(pr.into_pull(pushes, &rules))
    }

    /// Classic branch protection is readable by admins only; its required
    /// checks show only once reported, as required.
    async fn branch_rules(&self, branch: &str) -> BranchRules {
        let rules: Result<Vec<Value>> = self
            .rest(
                Method::GET,
                &self.repo_path(&format!("/rules/branches/{branch}")),
                None,
            )
            .await;
        match rules {
            Ok(rules) => BranchRules::from_rules(&rules),
            Err(e) => {
                // What the rules require is then read only where GitHub
                // reports it elsewhere.
                tracing::warn!(
                    branch,
                    error = format!("{e:#}"),
                    "reading the branch's rules"
                );
                BranchRules::default()
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

    async fn linear_history_required(&self, branch: &str) -> bool {
        if self.branch_rules(branch).await.linear_history {
            return true;
        }
        // Classic branch protection is readable by admins only.
        let protection: Result<Value> = self
            .rest(
                Method::GET,
                &self.repo_path(&format!("/branches/{branch}/protection")),
                None,
            )
            .await;
        protection.is_ok_and(|p| p["required_linear_history"]["enabled"] == json!(true))
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

/// What the rulesets on a branch require that GitHub does not always report
/// elsewhere.
#[derive(Default)]
struct BranchRules {
    /// Reported or not.
    checks: Vec<String>,
    code_owner_review: bool,
    linear_history: bool,
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
            linear_history: of_kind("required_linear_history").next().is_some(),
        }
    }
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

#[derive(Deserialize)]
struct GqlOpenPull {
    number: u64,
    author: Option<Actor>,
}

#[derive(Deserialize)]
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
    fn pushed_for(&self, linewright: &str) -> Option<TicketBranchName> {
        let name = TicketBranchName::parse(&self.name)?;
        let committer = self.target.as_ref()?.committer.as_ref()?.user.as_ref()?;
        (committer.login == linewright).then_some(name)
    }

    fn open_pull(&self) -> bool {
        self.associated_pull_requests.total_count > 0
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
/// partial list, so that no file under it goes unseen.
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

fn sub_issues(nodes: Option<Nodes<SubIssueNode>>) -> Vec<SubIssue> {
    let nodes = nodes.map(|n| n.nodes).unwrap_or_default();
    nodes
        .into_iter()
        .map(|n| SubIssue {
            number: Number(n.linked.number),
            author: login(n.author),
            state: if linked_of(&n.linked).open {
                SubIssueState::Open
            } else {
                SubIssueState::Closed { at: n.closed_at }
            },
        })
        .collect()
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

fn tracking_issues_among(
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
        .filter(|(_, filed)| *filed == Some(Filed::Tracking { spec_change }))
        .map(|(number, _)| number)
        .collect()
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
        "createdAt label { name }"
    };
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlLabeling {
    created_at: Timestamp,
    label: Name,
}

/// An issue's or pull request's labelings, assignments and reopenings, and
/// with `unlabelings` its unlabelings too, read as [`GqlEvent`]s.
macro_rules! assignment_timeline {
    () => {
        assignment_timeline!(@ "",)
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
// closed and merged ones, so that these do not crowd them out of the first
// page.
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
  closedByPullRequestsReferences(first: 20, includeClosedPrs: true, orderByState: true) { nodes { number state isDraft mergedAt closedAt updatedAt author { __typename login } } }
  ",
    // Only an issue's status labels need their removals.
    assignment_timeline!(unlabelings),
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
    author: Option<Actor>,
}

/// What a closed issue carries for the board, with the closer of its
/// latest closing: the pull request whose merge closed it, a commit, or none
/// when a person closed it
/// (<https://docs.github.com/en/graphql/reference/objects#closedevent>).
/// With its assignees, assignments and the pull requests closing it, which
/// tell whether Linewright worked it. Twenty issues with the six connections
/// each cost 121 requests, which rounds to 1 point
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
  closedByPullRequestsReferences(first: 20, includeClosedPrs: true) { nodes { number state isDraft mergedAt closedAt updatedAt author { __typename login } } }
  timelineItems(last: 1, itemTypes: [CLOSED_EVENT]) { nodes { ... on ClosedEvent { closer { __typename ... on PullRequest { number } } } } }
  assignments: timelineItems(last: 100, itemTypes: [ASSIGNED_EVENT, UNASSIGNED_EVENT]) {
    nodes {
      __typename
      ",
        assignment_fields!(),
        "
    }
  }
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
                filed: Filed::read(&author, &self.title, &self.body, linewright),
                sub_issues: sub_issues(self.sub_issues),
                labels: self.labels.nodes.into_iter().map(|l| l.name).collect(),
                merged_by,
                reason: self.state_reason.and_then(GqlStateReason::reason),
                work: WorkFacts {
                    author: Some(author),
                    assignees: logins(self.assignees),
                    events: events(self.assignments.nodes),
                    closed_by: closing_pulls(self.closed_by_pull_requests_references),
                },
            },
            title: self.title,
        })
    }
}

/// A query for the issues `numbers`, each read with `fields` under the alias
/// `i` and its number
/// (<https://docs.github.com/en/graphql/guides/using-the-explorer#aliases>).
fn numbered_query(numbers: &[Number], fields: Fragment) -> String {
    let Fragment { name, text } = fields;
    let issues: String = numbers
        .iter()
        .map(|Number(n)| format!("i{n}: issue(number: {n}) {{ ...{name} }} "))
        .collect();
    format!(
        "query($owner: String!, $name: String!) {{
          repository(owner: $owner, name: $name) {{ {issues}}}
        }}
        {text}"
    )
}

/// The closed issues `response` to a [`numbered_query`] holds that the
/// snapshot's filters would hold were they open. GitHub resolves a deleted
/// issue to null, with an error saying so.
fn closed_of_numbered(
    response: GqlResponse<RepoData<BTreeMap<String, Option<GqlClosedIssue>>>>,
    linewright: &Login,
) -> Result<Vec<ClosedItem>> {
    if !response.errors.iter().all(|e| e["type"] == "NOT_FOUND") {
        bail!("GraphQL errors: {}", Value::Array(response.errors));
    }
    let issues = response.data.context("GraphQL returned no data")?;
    Ok(issues
        .repository
        .into_values()
        .flatten()
        .filter(|issue| {
            in_view_if_open(
                &issue.labels,
                &issue.assignees,
                issue.author.as_ref(),
                linewright,
            )
        })
        .filter_map(|issue| issue.into_closed(linewright))
        .collect())
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
}

impl GqlClosedPull {
    /// `None` when it is not Linewright's.
    fn into_closed(self, linewright: &Login) -> Option<ClosedItem> {
        if login(self.author) != *linewright {
            return None;
        }
        Some(ClosedItem {
            number: Number(self.number),
            title: self.title,
            closed_at: self.closed_at?,
            updated_at: self.updated_at,
            kind: ClosedKind::Pull {
                closes: closes(self.closing_issues_references),
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
    #[serde(untagged)]
    Assignment(GqlAssignment),
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

/// The snapshot's filters, for the issues in `state`: must match
/// [`GqlIssue::in_view`].
fn in_view_filters(state: IssueState, linewright: &str) -> [Value; 3] {
    [
        json!({"states": [state], "labels": label::IN_VIEW}),
        json!({"states": [state], "assignee": linewright}),
        json!({"states": [state], "createdBy": linewright}),
    ]
}

/// Whether the snapshot would hold an issue with `labels`, `assignees` and
/// `author`, were it open: must match the snapshot's filters.
fn in_view_if_open(
    labels: &Nodes<Name>,
    assignees: &Nodes<LoginNode>,
    author: Option<&Actor>,
    linewright: &Login,
) -> bool {
    labels
        .nodes
        .iter()
        .any(|l| label::IN_VIEW.contains(&l.name.as_str()))
        || assignees
            .nodes
            .iter()
            .any(|a| a.login == linewright.as_str())
        || author.is_some_and(|a| a.login == linewright.as_str())
}

impl GqlIssue {
    /// Must match the snapshot's filters.
    fn in_view(&self, linewright: &str) -> bool {
        self.state == IssueState::Open && self.in_view_if_open(linewright)
    }

    /// Whether the snapshot would hold it, were it open.
    fn in_view_if_open(&self, linewright: &str) -> bool {
        in_view_if_open(
            &self.labels,
            &self.assignees,
            self.author.as_ref(),
            &Login::new(linewright),
        )
    }

    fn into_issue(self, linewright: &Login) -> Issue {
        let i = self;
        let author = login(i.author);
        let filed = Filed::read(&author, &i.title, &i.body, linewright);
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
            sub_issues: sub_issues(i.sub_issues),
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
        })
        .collect()
}

fn events(nodes: Vec<GqlEvent>) -> Vec<IssueEvent> {
    nodes
        .into_iter()
        .filter_map(|e| match e {
            GqlEvent::Labeled(labeling) => Some(IssueEvent::Labeled {
                label: labeling.label.name,
                at: labeling.created_at,
            }),
            GqlEvent::Unlabeled(labeling) => Some(IssueEvent::Unlabeled {
                label: labeling.label.name,
                at: labeling.created_at,
            }),
            GqlEvent::Reopened { created_at } => Some(IssueEvent::Reopened { at: created_at }),
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

const PULL_FIELDS: &str = concat!(
    "fragment PullFields on PullRequest {
  number title body createdAt updatedAt baseRefName headRefName headRefOid isDraft state mergedAt closedAt
  mergeable mergeStateStatus canBeRebased reviewDecision
  author { __typename login }
  ",
    closes_field!(),
    "
  labels(first: 50) { nodes { name } }
  assignees(first: 20) { nodes { login } }
  timelineItems(last: 100, itemTypes: [ASSIGNED_EVENT, UNASSIGNED_EVENT]) {
    nodes {
      __typename
      ",
    assignment_fields!(),
    "
    }
  }
  comments(last: 100) { nodes { author { __typename login } body createdAt } }
  reviews(last: 100) { nodes { author { __typename login } state body submittedAt } }
  reviewRequests(first: 20) {
    nodes { asCodeOwner requestedReviewer { __typename ... on User { login } } }
  }
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
);

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
    timeline_items: Nodes<GqlAssignment>,
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
    /// `IN_PROGRESS` (#549).
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
    /// GitHub reports no review decision when the rules require a code
    /// owner's review but no approvals, so the rules say that review is
    /// needed then.
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

    /// Names the code owners only when they are all users: a team can't be
    /// named, and naming only the users would leave out the team's members.
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
            // GitHub reports no merge state for a draft.
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
        Pull {
            number: Number(self.number),
            title: self.title,
            body: self.body,
            author: login(self.author),
            created_at: self.created_at,
            updated_at: self.updated_at,
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
            assignees: self
                .assignees
                .nodes
                .into_iter()
                .map(|a| Login::new(a.login))
                .collect(),
            assignments: self
                .timeline_items
                .nodes
                .into_iter()
                .filter_map(GqlAssignment::read)
                .collect(),
        }
    }
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
    assignment_timeline!(),
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
    async fn identity(&self) -> Result<Identity> {
        #[derive(Deserialize)]
        struct Repo {
            permissions: Option<BTreeMap<String, bool>>,
        }
        let repo: Repo = self.rest(Method::GET, &self.repo_path(""), None).await?;
        let can_push = repo
            .permissions
            .and_then(|p| p.get("push").copied())
            .unwrap_or(false);
        if !can_push {
            bail!(
                "{} cannot push to {}/{}: Linewright's account needs write access",
                self.viewer.login,
                self.owner,
                self.name
            );
        }
        Ok(Identity {
            login: self.linewright(),
            name: self.viewer.login.clone(),
            email: format!(
                "{}+{}@users.noreply.github.com",
                self.viewer.id, self.viewer.login
            ),
        })
    }

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

    async fn settings(&self) -> Result<RepoSettings> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Settings {
            issue_types: Option<Nodes<IssueType>>,
            merge_commit_allowed: bool,
            squash_merge_allowed: bool,
            rebase_merge_allowed: bool,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct IssueType {
            name: String,
            is_enabled: bool,
        }
        #[derive(Deserialize)]
        struct Repo {
            full_name: String,
            default_branch: String,
        }
        let repo: Repo = self.rest(Method::GET, &self.repo_path(""), None).await?;
        let settings: RepoData<Settings> = self
            .graphql(
                "query($owner: String!, $name: String!) {
                  repository(owner: $owner, name: $name) {
                    issueTypes(first: 25) { nodes { name isEnabled } }
                    mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed
                  }
                }",
                json!({"owner": self.owner, "name": self.name}),
            )
            .await?;
        let settings = settings.repository;
        let allowed_merge_methods = [
            (MergeMethod::Merge, settings.merge_commit_allowed),
            (MergeMethod::Squash, settings.squash_merge_allowed),
            (MergeMethod::Rebase, settings.rebase_merge_allowed),
        ]
        .into_iter()
        .filter_map(|(method, allowed)| allowed.then_some(method))
        .collect();
        let types: Vec<String> = settings
            .issue_types
            .map(|t| t.nodes)
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.is_enabled)
            .map(|t| t.name)
            .collect();
        let linear_history = self.linear_history_required(&repo.default_branch).await;
        let categories = if types.is_empty() {
            Categories::Labels
        } else {
            Categories::IssueTypes(types)
        };
        let full_name = repo
            .full_name
            .parse()
            .context("GitHub's full name of the repository")?;
        Ok(RepoSettings::new(
            full_name,
            repo.default_branch,
            linear_history,
            allowed_merge_methods,
            categories,
        ))
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

    async fn snapshot(&self, windows: Windows) -> Result<Snapshot> {
        let settings_branch: Value = self.rest(Method::GET, &self.repo_path(""), None).await?;
        let default_branch = settings_branch["default_branch"]
            .as_str()
            .context("the repository has a default branch")?
            .to_string();
        let linewright = &self.viewer.login;
        let mut issues: BTreeMap<u64, GqlIssue> = BTreeMap::new();
        for filter in in_view_filters(IssueState::Open, linewright) {
            // Twenty issues with the seven connections each cost 141
            // requests, which rounds to 1 point; fifty cost 4
            // (<https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api#calculating-a-rate-limit-score-before-running-the-call>).
            let open: Vec<GqlIssue> = self.issues(ISSUE_FIELDS, filter).await?;
            for issue in open {
                issues.insert(issue.number, issue);
            }
        }
        let mut pulls = vec![];
        for pr in self.open_pulls().await? {
            if self.by_linewright(pr.author.as_ref()) {
                pulls.push(self.pull(Number(pr.number)).await?);
            }
        }
        let closed: std::collections::BTreeSet<u64> = issues
            .values()
            .flat_map(|i| {
                i.closed_by_pull_requests_references
                    .iter()
                    .flat_map(|n| &n.nodes)
            })
            .filter(|p| {
                matches!(p.state.as_str(), "CLOSED" | "MERGED")
                    && self.by_linewright(p.author.as_ref())
            })
            .map(|p| p.number)
            .collect();
        for number in closed {
            pulls.push(self.pull(Number(number)).await?);
        }
        let closed_issues = self.closed_issues(windows.read_since()).await?;
        let closed_pulls = self.closed_pulls(windows.read_since()).await?;
        let mut snapshot = Snapshot {
            ticket_branches: self.ticket_branches(&issues).await?,
            merged: self.merged_within(&default_branch, windows).await?,
            closed: closed_within(closed_issues.into_iter().chain(closed_pulls), windows),
            default_branch,
            issues: issues
                .into_values()
                .map(|i| i.into_issue(&self.linewright()))
                .collect(),
            pulls,
        };
        let tickets = snapshot.unheld_closed_tickets(&self.linewright());
        for ticket in self.closed_issues_numbered(&tickets).await? {
            snapshot.put_closed(ticket);
        }
        Ok(snapshot)
    }

    async fn pull(&self, number: Number) -> Result<Pull> {
        let pr = self.gql_pull(number).await?;
        self.pull_view(pr.with_context(|| format!("there is no pull request {number}"))?)
            .await
    }

    async fn any_item(&self, number: Number) -> Result<Option<AnyItem>> {
        // GitHub resolves no issue for a pull request's number.
        if let Some(issue) = self.gql_issue(number).await? {
            let held = issue.in_view(&self.viewer.login);
            let issue = Box::new(issue.into_issue(&self.linewright()));
            return Ok(Some(if held {
                AnyItem::HeldIssue(issue)
            } else {
                AnyItem::LeftOutIssue(issue)
            }));
        }
        let Some(pr) = self.gql_pull(number).await? else {
            return Ok(None);
        };
        if pr.state != "MERGED" {
            return Ok(Some(AnyItem::Pull(Box::new(self.pull_view(pr).await?))));
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
            tracking_issues: self.tracking_issues_of(number).await?,
        }))
    }

    async fn linewright_pull(&self, number: Number) -> Result<Option<Pull>> {
        let Some(pr) = self.gql_pull(number).await? else {
            return Ok(None);
        };
        if !self.by_linewright(pr.author.as_ref()) {
            return Ok(None);
        }
        // Must match what the snapshot holds.
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
            let issue = self.gql_issue(Number(issue)).await?;
            held = issue.is_some_and(|i| i.in_view(&self.viewer.login));
        }
        if !held {
            return Ok(None);
        }
        Ok(Some(self.pull_view(pr).await?))
    }

    async fn issue(&self, number: Number) -> Result<IssueRead> {
        let Some(issue) = self.gql_issue(number).await? else {
            return Ok(IssueRead::Gone);
        };
        let open = issue.state == IssueState::Open;
        if !open && issue.in_view_if_open(&self.viewer.login) {
            let closed: Option<GqlClosedIssue> =
                self.issue_with(number, CLOSED_ISSUE_FIELDS).await?;
            if let Some(closed) = closed.and_then(|i| i.into_closed(&self.linewright())) {
                return Ok(IssueRead::Closed(Box::new(closed)));
            }
        }
        if !issue.in_view(&self.viewer.login) {
            return Ok(IssueRead::LeftOut { open });
        }
        // Must match what the snapshot holds.
        let mut pulls = vec![];
        for pr in issue
            .closed_by_pull_requests_references
            .iter()
            .flat_map(|n| &n.nodes)
            .filter(|p| self.by_linewright(p.author.as_ref()))
        {
            pulls.push(self.pull(Number(pr.number)).await?);
        }
        Ok(IssueRead::Held {
            issue: Box::new(issue.into_issue(&self.linewright())),
            pulls,
        })
    }

    async fn open_pull_closes(&self, number: Number) -> Result<Vec<Number>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Closing {
            state: String,
            closing_issues_references: Option<Nodes<NumberNode>>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct One {
            pull_request: Closing,
        }
        let query = concat!(
            "query($owner: String!, $name: String!, $number: Int!) {
              repository(owner: $owner, name: $name) { pullRequest(number: $number) { state ",
            closes_field!(),
            " } }
            }"
        );
        let data: Option<RepoData<One>> = self
            .graphql_found(
                query,
                json!({"owner": self.owner, "name": self.name, "number": number.0}),
            )
            .await?;
        Ok(data
            .map(|d| d.repository.pull_request)
            .filter(|pr| pr.state == "OPEN")
            .map(|pr| closes(pr.closing_issues_references))
            .unwrap_or_default())
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
                // A GitHub Actions check run's id is its job's id. Other
                // apps' check runs have no log Linewright can read.
                let actions = node["checkSuite"]["app"]["slug"] == "github-actions";
                let log = match node["databaseId"].as_u64().filter(|_| actions) {
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
        let response = self.send(Method::GET, &resource, None).await?;
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let text = response.text().await?;
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
        let response = self.send(Method::GET, &resource, None).await?;
        let status = response.status();
        // GitHub answers 404 for a branch or path it does not have, and 422
        // for a path that is not a tree.
        if status == StatusCode::NOT_FOUND || status == StatusCode::UNPROCESSABLE_ENTITY {
            return Ok(vec![]);
        }
        let text = response.text().await?;
        if !status.is_success() {
            bail!("listing {dir} on {branch}: {status}: {text}");
        }
        let tree = serde_json::from_str(&text)
            .with_context(|| format!("listing {dir} on {branch}: not a tree"))?;
        files_listed(tree).with_context(|| format!("listing {dir} on {branch}"))
    }

    async fn clone_repo(&self, dir: &Path, branch: &str) -> Result<()> {
        let url = format!("https://github.com/{}/{}.git", self.owner, self.name);
        git::clone(&url, dir, branch, &self.git_env()).await
    }

    async fn push(&self, dir: &Path, branch: &str, mode: PushMode) -> Result<(), PushError> {
        let refspec = format!("HEAD:refs/heads/{branch}");
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
        let answer = waiting_out_rate_limits(&request, || async {
            let upload = self.attach_request(repository_id, name, content_type, bytes.clone());
            Answer::read(upload.send().await.with_context(|| request.clone())?).await
        })
        .await?;
        attached(&request, answer)
    }

    async fn delete_branch(&self, branch: &str) -> Result<()> {
        let path = format!("/git/refs/heads/{}", percent_encode(branch));
        let response = self
            .send(Method::DELETE, &self.repo_path(&path), None)
            .await?;
        let status = response.status();
        branch_deleted(branch, status, &response.text().await?)
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
        let response = self
            .send(
                Method::DELETE,
                &self.repo_path(&format!(
                    "/issues/{}/labels/{}",
                    number.0,
                    percent_encode(label)
                )),
                None,
            )
            .await?;
        match response.status() {
            s if s.is_success() || s == StatusCode::NOT_FOUND => Ok(()),
            s => bail!(
                "removing {label} from {number}: {s}: {}",
                response.text().await?
            ),
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

    /// GitHub leaves out anyone it cannot assign, without an error
    /// (<https://docs.github.com/en/rest/issues/assignees#add-assignees-to-an-issue>),
    /// or refuses the request as invalid; one at a time, a refusal is the
    /// person's alone.
    async fn assign_people(&self, issue: Number, people: &[Login]) -> Result<Vec<Login>> {
        let mut refused = vec![];
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
        self.rest_ok(
            Method::POST,
            &self.repo_path(&format!("/issues/{}/comments", number.0)),
            Some(json!({"body": body})),
        )
        .await
    }

    async fn create_issue(&self, issue: NewIssue) -> Result<Number> {
        #[derive(Deserialize)]
        struct Created {
            number: u64,
        }
        let created: Created = self
            .rest(
                Method::POST,
                &self.repo_path("/issues"),
                Some(json!({"title": issue.title, "body": issue.body, "labels": issue.labels})),
            )
            .await?;
        Ok(Number(created.number))
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
        self.rest_ok(
            Method::POST,
            &self.repo_path(&format!("/issues/{}/sub_issues", parent.0)),
            Some(json!({"sub_issue_id": id})),
        )
        .await
    }

    async fn add_blocked_by(&self, issue: Number, blocker: Number) -> Result<()> {
        let id = self.issue_id(blocker).await?;
        self.rest_ok(
            Method::POST,
            &self.repo_path(&format!("/issues/{}/dependencies/blocked_by", issue.0)),
            Some(json!({"issue_id": id})),
        )
        .await
    }

    async fn open_pull(&self, pull: NewPull) -> Result<Number> {
        #[derive(Deserialize)]
        struct Created {
            number: u64,
        }
        let base: Value = self.rest(Method::GET, &self.repo_path(""), None).await?;
        let created: Created = self
            .rest(
                Method::POST,
                &self.repo_path("/pulls"),
                Some(json!({
                    "head": pull.head,
                    "base": base["default_branch"],
                    "title": pull.title,
                    "body": pull.body,
                    "draft": true,
                })),
            )
            .await?;
        Ok(Number(created.number))
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
        let response = self
            .send(
                Method::POST,
                &self.repo_path(&format!("/pulls/{}/requested_reviewers", pr.0)),
                Some(json!({"reviewers": reviewers})),
            )
            .await?;
        // A reviewer who is not a collaborator cannot be requested; the
        // review request is a courtesy, not the outcome.
        if response.status() == StatusCode::UNPROCESSABLE_ENTITY {
            let body = response.text().await?;
            tracing::warn!(pr = pr.0, body, "review request refused");
            return Ok(());
        }
        if !response.status().is_success() {
            bail!(
                "requesting review on {pr}: {}: {}",
                response.status(),
                response.text().await?
            );
        }
        Ok(())
    }

    async fn merge(&self, pr: Number, head: &str, method: MergeMethod) -> Result<()> {
        self.rest_ok(
            Method::PUT,
            &self.repo_path(&format!("/pulls/{}/merge", pr.0)),
            Some(json!({"sha": head, "merge_method": method})),
        )
        .await
        .map_err(|e| match e.downcast::<ErrorResponse>() {
            Ok(refused) => anyhow!(refused.message()),
            Err(e) => e,
        })
    }

    async fn reply_in_thread(&self, _pr: Number, thread: &str, body: &str) -> Result<()> {
        let _: Value = self
            .graphql(
                "mutation($thread: ID!, $body: String!) {
                  addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $thread, body: $body}) { clientMutationId }
                }",
                json!({"thread": thread, "body": body}),
            )
            .await?;
        Ok(())
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

    #[test]
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

    #[test]
    fn a_team_requested_as_code_owner_requires_a_code_owners_review_without_naming_it() {
        let pr = review_required(&json!([code_owner(&json!({"__typename": "Team"}))]));

        assert_eq!(
            displayed(&pr.requirements(&BranchRules::default())),
            ["a code owner's review"]
        );
    }

    #[test]
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

    #[test]
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

    #[test]
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
    #[test]
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
    /// reported `IN_PROGRESS` (#549).
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

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
    fn a_team_requested_as_code_owner_under_rules_requiring_its_review_is_required_unnamed() {
        let pr = no_review_decision(&json!([code_owner(&json!({"__typename": "Team"}))]));

        assert_eq!(
            pr.requirements(&code_owner_review_ruled()),
            [Requirement::CodeOwnerReview { owners: vec![] }]
        );
    }

    #[test]
    fn a_code_owner_requested_without_a_review_decision_or_a_rule_requiring_it_is_not_required() {
        let pr = no_review_decision(&json!([code_owner(
            &json!({"__typename": "User", "login": "tibbe"})
        )]));

        assert_eq!(pr.requirements(&BranchRules::default()), []);
    }

    #[test]
    fn rules_requiring_a_code_owners_review_require_none_when_no_code_owner_is_requested() {
        let pr = no_review_decision(&json!([{
            "asCodeOwner": false,
            "requestedReviewer": {"__typename": "User", "login": "alice"},
        }]));

        assert_eq!(pr.requirements(&code_owner_review_ruled()), []);
    }

    #[test]
    fn a_branchs_rules_give_its_required_checks_and_whether_a_code_owners_review_is_required() {
        // As `GET /repos/tibbe/linewright/rules/branches/main` returns them.
        let rules: Vec<Value> = serde_json::from_value(json!([
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

        let rules = BranchRules::from_rules(&rules);

        assert_eq!(rules.checks, ["check"]);
        assert!(rules.code_owner_review);
    }

    /// `fixtures/issue-reopened.json` is `IssueFields` as GitHub returned it
    /// for <https://github.com/rust-lang/cargo/issues/11777>, closed by a merged
    /// pull request and reopened.
    #[test]
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
                    at: at("2024-04-09T11:02:07Z"),
                },
                IssueEvent::Labeled {
                    label: "S-blocked-external".into(),
                    at: at("2024-04-10T01:18:27Z"),
                },
                IssueEvent::Reopened {
                    at: at("2024-09-19T12:01:27Z"),
                },
            ]
        );
    }

    #[test]
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
    #[test]
    fn a_pull_request_is_held_with_when_github_last_updated_it() {
        let pr: GqlPull =
            serde_json::from_str(include_str!("fixtures/pull-changes-requested.json")).unwrap();

        let pull = pr.into_pull(vec![], &BranchRules::default());

        assert_eq!(pull.updated_at, at("2026-10-17T14:04:59Z"));
    }

    /// `fixtures/merged-pull.json` is `MergedFields` as GitHub returned it
    /// for <https://github.com/tibbe/linewright/pull/477>, a spec change
    /// Linewright designed after its merge.
    #[test]
    fn a_merged_pull_request_is_held_with_when_github_last_updated_it() {
        let pr: GqlMerged =
            serde_json::from_str(include_str!("fixtures/merged-pull.json")).unwrap();

        let merged = pr.into_merged(at("2026-10-21T14:56:28Z"), vec![]);

        assert_eq!(merged.updated_at, at("2026-10-21T15:05:15Z"));
    }

    /// `fixtures/merged-pull.json`, as above: merged at 14:56:28, and
    /// labeled designed, which updated it, at 15:05:15.
    #[test]
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

    #[test]
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

    #[test]
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
    #[test]
    fn an_issues_sub_issues_are_read_with_who_opened_them_and_when_they_closed() {
        let nodes: Nodes<SubIssueNode> =
            serde_json::from_str(include_str!("fixtures/sub-issues.json")).unwrap();

        assert_eq!(
            sub_issues(Some(nodes)),
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
                state: SubIssueState::Closed {
                    at: Some(at(closed_at)),
                },
            })
        );
    }

    /// The person's issue of `fixtures/issue-reopened.json`, unassigned, as
    /// Linewright leaves an issue it cut: labeled designed alone.
    #[test]
    fn an_issue_a_person_opened_labeled_designed_alone_is_in_view_as_a_tracking_issue_may_be() {
        let mut issue: Value =
            serde_json::from_str(include_str!("fixtures/issue-reopened.json")).unwrap();
        issue["labels"] = json!({"nodes": [{"name": "linewright:designed"}]});
        let issue: GqlIssue = serde_json::from_value(issue).unwrap();

        assert!(issue.in_view("linewright"));
    }

    #[test]
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
            })
        );
    }

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
    fn a_spec_changes_tracking_issues_are_the_issues_linewright_filed_for_it_here() {
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
            tracking_issues_among(references, Number(5), &Login::new("linewright[bot]")),
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
    #[test]
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
                        filed: Some(Filed::Tracking {
                            spec_change: Number(386)
                        }),
                        sub_issues: vec![
                            ticket(388, "2026-10-20T13:49:33Z"),
                            ticket(389, "2026-10-20T14:10:32Z"),
                        ],
                        labels: vec![],
                        merged_by: None,
                        reason: Some(IssueCloseReason::Completed),
                        work: WorkFacts {
                            author: Some(Login::new("tibbe-bot")),
                            ..WorkFacts::default()
                        },
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
                        filed: None,
                        sub_issues: vec![],
                        labels: vec!["ready-for-agent".into(), "linewright:designed".into()],
                        merged_by: Some(Number(391)),
                        reason: Some(IssueCloseReason::Completed),
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
                            }],
                        },
                    },
                },
            ]
        );
    }

    /// `fixtures/closed-issues-numbered.json` is GitHub's response to
    /// `numbered_query` for #388, #719 and #99999 of
    /// <https://github.com/tibbe/linewright>: #388, a ticket its account
    /// opened, closed by the merge of #390; #719, an issue a person opened,
    /// unlabeled and assigned only to them; and #99999, which does not exist.
    #[test]
    fn closed_issues_read_by_number_are_those_the_snapshot_would_hold_were_they_open() {
        let response =
            serde_json::from_str(include_str!("fixtures/closed-issues-numbered.json")).unwrap();

        let closed = closed_of_numbered(response, &Login::new("tibbe-bot")).unwrap();

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

    /// `fixtures/closed-issue-worked.json` is `ClosedIssueFields` as GitHub
    /// returned it for <https://github.com/tibbe/linewright/issues/673>, a
    /// person's issue Linewright claimed, let go of, claimed again and closed
    /// with its merged pull request #675.
    #[test]
    fn a_closed_issue_is_held_with_its_assignees_assignments_and_the_pull_requests_closing_it() {
        let issue: GqlClosedIssue =
            serde_json::from_str(include_str!("fixtures/closed-issue-worked.json")).unwrap();

        let closed = issue.into_closed(&Login::new("tibbe-bot")).unwrap();

        let ClosedKind::Issue { work, .. } = closed.kind else {
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
            }]
        );
    }

    /// `fixtures/closed-issue-not-planned.json` is `ClosedIssueFields` as
    /// GitHub returned it for <https://github.com/tibbe/linewright/issues/305>,
    /// which its maintainer closed as not planned.
    #[test]
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
    #[test]
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

    /// `fixtures/closed-pulls.json` is the page of closed pull requests
    /// GitHub returned for <https://github.com/tibbe/linewright>: the one,
    /// #321, its maintainer closed without merging.
    #[test]
    fn linewrights_pull_requests_closed_unmerged_since_the_window_opened_are_held() {
        let page = || -> Page<GqlClosedPull> {
            serde_json::from_str(include_str!("fixtures/closed-pulls.json")).unwrap()
        };
        let closed = |since: &str, linewright: &str| -> Vec<ClosedItem> {
            let read = page()
                .nodes
                .into_iter()
                .filter_map(|pr| pr.into_closed(&Login::new(linewright)));
            closed_within(read, opened(since))
        };

        // The maintainer's account standing in for Linewright's.
        assert_eq!(
            closed("2026-10-18T07:00:00Z", "tibbe"),
            [ClosedItem {
                number: Number(321),
                title: "Restore the to-tickets skill to upstream, then make it non-interactive"
                    .into(),
                closed_at: at("2026-10-18T07:30:05Z"),
                updated_at: at("2026-10-18T07:30:05Z"),
                kind: ClosedKind::Pull { closes: vec![] },
            }]
        );
        assert_eq!(closed("2026-10-18T08:00:00Z", "tibbe"), []);
        assert_eq!(closed("2026-10-18T07:00:00Z", "tibbe-bot"), []);
    }

    /// `fixtures/labels.json` is the example response GitHub documents for
    /// listing a repository's labels
    /// (<https://docs.github.com/en/rest/issues/labels#list-labels-for-a-repository>).
    #[tokio::test]
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
    #[test]
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
    #[test]
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
    #[test]
    fn a_directory_is_listed_from_its_tree_at_the_head_of_the_branch() {
        assert_eq!(
            tree_path("7-a/b", ".linewright/automations"),
            "/git/trees/7-a%2Fb%3A.linewright%2Fautomations?recursive=1"
        );
    }

    /// `fixtures/linewright-tree.json` is what GitHub returned for
    /// `main:.linewright` on <https://github.com/tibbe/linewright>.
    #[test]
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
    #[test]
    fn a_tree_too_large_for_github_to_list_whole_is_an_error() {
        let tree =
            serde_json::from_str(include_str!("fixtures/linux-tree-truncated.json")).unwrap();

        assert!(files_listed(tree).is_err());
    }

    /// `fixtures/repository-files.json` is what GitHub returned for that
    /// query on <https://github.com/rust-lang/rust>, asking for a PNG
    /// image, a path with no file, a directory and the script `x`.
    #[test]
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
    #[test]
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
            token: token.to_string(),
            owner: "owner".into(),
            name: "repo".into(),
            viewer: Viewer {
                login: "linewright".into(),
                id: 1,
            },
            repository_id: Arc::default(),
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
    #[test]
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
    #[test]
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

    #[test]
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
    #[test]
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

    #[test]
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

    #[tokio::test(start_paused = true)]
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

        let answered = waiting_out_rate_limits(UPLOAD, || {
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
}
