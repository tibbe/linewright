use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use base64::Engine;
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{
    AnyItem, Categories, CloseReason, Identity, IssueRead, NewIssue, NewPull, PushError, PushMode,
    RepoSettings, Store,
};
use super::{job_log, rate_limit};
use crate::domain::{
    Assignment, BranchIssue, Check, CheckFailure, CheckLog, CheckState, ClosingPull, Comment,
    Filed, Issue, IssueEvent, JobLog, Linked, Login, MergeMethod, Mergeability, MergedPull, Number,
    Parent, Pull, PullState, Push, Rebaseability, Requirement, Review, ReviewState, Snapshot,
    SubIssue, Thread, TicketBranch, TicketBranchName, Timestamp, label,
};
use crate::git;

const API: &str = "https://api.github.com";

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
}

impl std::fmt::Display for ErrorResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}: {}", self.request, self.status, self.body)
    }
}

impl std::error::Error for ErrorResponse {}

#[derive(Clone, Deserialize)]
struct Viewer {
    login: String,
    id: u64,
}

impl GitHub {
    pub async fn connect(repo: &str, token: String) -> Result<GitHub> {
        let (owner, name) = repo
            .split_once('/')
            .with_context(|| format!("the repository {repo:?} is not owner/name"))?;
        let http = reqwest::Client::builder()
            .user_agent("linewright")
            .build()?;
        let mut github = GitHub {
            http,
            token,
            owner: owner.into(),
            name: name.into(),
            viewer: Viewer {
                login: String::new(),
                id: 0,
            },
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
        let mut retries = 0;
        let text = loop {
            let response = self.send(method.clone(), path, body.clone()).await?;
            let status = response.status();
            let headers = response.headers().clone();
            let text = response.text().await?;
            let wait = rate_limit::wait(status, &headers, &text, retries, SystemTime::now());
            if let Some(wait) = wait.filter(|_| retries < rate_limit::RETRIES) {
                tracing::warn!(%method, path, wait = %humantime::format_duration(wait), "rate limited");
                tokio::time::sleep(wait).await;
                retries += 1;
                continue;
            }
            if !status.is_success() || wait.is_some() {
                return Err(ErrorResponse {
                    request: format!("{method} {path}"),
                    status,
                    body: text,
                }
                .into());
            }
            break text;
        };
        let text = if text.is_empty() { "null" } else { &text };
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

    async fn issues(&self, filter: Value) -> Result<Vec<GqlIssue>> {
        // Twenty issues with the seven connections each has cost 141
        // requests, which rounds to 1 point; fifty cost 4
        // (<https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api#calculating-a-rate-limit-score-before-running-the-call>).
        let query = format!(
            "query($owner: String!, $name: String!, $filter: IssueFilters, $cursor: String) {{
              repository(owner: $owner, name: $name) {{
                issues(first: 20, after: $cursor, filterBy: $filter) {{
                  pageInfo {{ hasNextPage endCursor }}
                  nodes {{ ...IssueFields }}
                }}
              }}
            }}
            {ISSUE_FIELDS}"
        );
        let mut issues = vec![];
        let mut cursor = Value::Null;
        loop {
            let data: RepoData<IssuesData> = self
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

    async fn merged_since(&self, base: &str, since: Timestamp) -> Result<Vec<MergedPull>> {
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
                if pr.updated_at < since {
                    older = true;
                    continue;
                }
                if let Some(pr) = self.merged_view(pr, base, since).await? {
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
        since: Timestamp,
    ) -> Result<Option<MergedPull>> {
        let Some(merged_at) = pr.merged_at.filter(|at| *at >= since) else {
            return Ok(None);
        };
        if pr.base_ref_name != base {
            return Ok(None);
        }
        Ok(Some(self.merged_of(pr, merged_at).await?))
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
        #[derive(Deserialize)]
        struct One {
            issue: GqlIssue,
        }
        let query = format!(
            "query($owner: String!, $name: String!, $number: Int!) {{
              repository(owner: $owner, name: $name) {{ issue(number: $number) {{ ...IssueFields }} }}
            }}
            {ISSUE_FIELDS}"
        );
        let data: Option<RepoData<One>> = self
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
struct IssuesData {
    issues: Page<GqlIssue>,
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
struct SubIssueNode {
    #[serde(flatten)]
    linked: LinkedNode,
    author: Option<Actor>,
}

fn sub_issues(nodes: Option<Nodes<SubIssueNode>>) -> Vec<SubIssue> {
    let nodes = nodes.map(|n| n.nodes).unwrap_or_default();
    nodes
        .into_iter()
        .map(|n| SubIssue {
            linked: linked_of(&n.linked),
            author: login(n.author),
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

// `orderByState` lists the open pull requests that close an issue before the
// closed and merged ones, so that these do not crowd them out of the first
// page.
const ISSUE_FIELDS: &str = concat!(
    "fragment IssueFields on Issue {
  number title body createdAt state
  author { __typename login }
  labels(first: 50) { nodes { name } }
  assignees(first: 20) { nodes { login } }
  comments(last: 100) { nodes { author { __typename login } body createdAt } }
  subIssues(first: 100) { nodes { number state author { __typename login } } }
  parent { number title body author { __typename login } }
  blockedBy(first: 50) { nodes { number state } }
  closedByPullRequestsReferences(first: 20, includeClosedPrs: true, orderByState: true) { nodes { number state isDraft mergedAt closedAt author { __typename login } } }
  ",
    // Only an issue's status labels need their removals.
    assignment_timeline!(unlabelings),
    "
}"
);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlIssue {
    number: u64,
    title: String,
    body: String,
    created_at: Timestamp,
    state: String,
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
    author: Option<Actor>,
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

impl GqlIssue {
    /// Must match the snapshot's filters.
    fn in_view(&self, linewright: &str) -> bool {
        self.state == "OPEN"
            && (self
                .labels
                .nodes
                .iter()
                .any(|l| label::IN_VIEW.contains(&l.name.as_str()))
                || self.assignees.nodes.iter().any(|a| a.login == linewright)
                || self.author.as_ref().is_some_and(|a| a.login == linewright))
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
            open: i.state == "OPEN",
            created_at: i.created_at,
            labels: i.labels.nodes.into_iter().map(|l| l.name).collect(),
            assignees: logins(i.assignees),
            comments: i.comments.nodes.into_iter().map(Comment::from).collect(),
            blocked_by: linked(i.blocked_by),
            sub_issues: sub_issues(i.sub_issues),
            parent: i.parent.map(|p| p.into_parent(linewright)),
            events,
            filed,
            closed_by: i
                .closed_by_pull_requests_references
                .map(|n| n.nodes)
                .unwrap_or_default()
                .into_iter()
                .map(|p| ClosingPull {
                    number: Number(p.number),
                    state: pull_state(&p.state, p.is_draft, p.merged_at, p.closed_at),
                    author: login(p.author),
                })
                .collect(),
        }
    }
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

macro_rules! closes_field {
    () => {
        "closingIssuesReferences(first: 20) { nodes { number } }"
    };
}

const PULL_FIELDS: &str = concat!(
    "fragment PullFields on PullRequest {
  number title body createdAt baseRefName headRefName headRefOid isDraft state mergedAt closedAt
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
    review_decision: Option<String>,
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
                    ("COMPLETED", Some("SUCCESS" | "NEUTRAL" | "SKIPPED")) => CheckState::Passed,
                    ("COMPLETED", _) => CheckState::Failed,
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
        match (self.review_decision.as_deref(), self.code_owner_review()) {
            (Some("REVIEW_REQUIRED"), Some(review)) => requires.push(review),
            (_, Some(review)) if rules.code_owner_review => requires.push(review),
            (Some("REVIEW_REQUIRED" | "CHANGES_REQUESTED"), _) => {
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
        Pull {
            number: Number(self.number),
            title: self.title,
            body: self.body,
            author: login(self.author),
            created_at: self.created_at,
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
    fn into_merged(self, merged_at: Timestamp, files: Vec<String>) -> MergedPull {
        MergedPull {
            number: Number(self.number),
            title: self.title,
            body: self.body,
            author: login(self.author),
            merged_at,
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
        Ok(RepoSettings::new(
            repo.default_branch,
            linear_history,
            allowed_merge_methods,
            categories,
        ))
    }

    async fn label_exists(&self, name: &str) -> Result<bool> {
        let response = self
            .send(
                Method::GET,
                &self.repo_path(&format!("/labels/{}", percent_encode(name))),
                None,
            )
            .await?;
        match response.status() {
            s if s.is_success() => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            s => bail!("looking up label {name}: {s}: {}", response.text().await?),
        }
    }

    async fn ensure_labels(&self, labels: &[(&str, &str)]) -> Result<()> {
        let mut existing: Vec<String> = vec![];
        for page in 1.. {
            let names: Vec<Name> = self
                .rest(
                    Method::GET,
                    &self.repo_path(&format!("/labels?per_page=100&page={page}")),
                    None,
                )
                .await?;
            let done = names.len() < 100;
            existing.extend(names.into_iter().map(|n| n.name.to_lowercase()));
            if done {
                break;
            }
        }
        for (name, description) in labels {
            if !existing.contains(&name.to_lowercase()) {
                self.rest_ok(
                    Method::POST,
                    &self.repo_path("/labels"),
                    Some(json!({"name": name, "description": description, "color": "ededed"})),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn snapshot(&self, merged_since: Timestamp) -> Result<Snapshot> {
        let settings_branch: Value = self.rest(Method::GET, &self.repo_path(""), None).await?;
        let default_branch = settings_branch["default_branch"]
            .as_str()
            .context("the repository has a default branch")?
            .to_string();
        let linewright = &self.viewer.login;
        let mut issues: BTreeMap<u64, GqlIssue> = BTreeMap::new();
        // Must match `GqlIssue::in_view`.
        for filter in [
            json!({"states": ["OPEN"], "labels": label::IN_VIEW}),
            json!({"states": ["OPEN"], "assignee": linewright}),
            json!({"states": ["OPEN"], "createdBy": linewright}),
        ] {
            for issue in self.issues(filter).await? {
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
        Ok(Snapshot {
            ticket_branches: self.ticket_branches(&issues).await?,
            merged: self.merged_since(&default_branch, merged_since).await?,
            default_branch,
            issues: issues
                .into_values()
                .map(|i| i.into_issue(&self.linewright()))
                .collect(),
            pulls,
        })
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
        if !issue.in_view(&self.viewer.login) {
            return Ok(IssueRead::LeftOut {
                open: issue.state == "OPEN",
            });
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

    async fn merged_pull(
        &self,
        number: Number,
        merged_since: Timestamp,
    ) -> Result<Option<MergedPull>> {
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
        self.merged_view(pull_request, &default_branch_ref.name, merged_since)
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

    async fn list_files(&self, branch: &str, dir: &str) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Entry {
            name: String,
            #[serde(rename = "type")]
            kind: String,
        }
        let dir_segments: Vec<String> = dir.split('/').map(percent_encode).collect();
        let resource = self.repo_path(&format!(
            "/contents/{}?ref={}",
            dir_segments.join("/"),
            percent_encode(branch)
        ));
        let response = self.send(Method::GET, &resource, None).await?;
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Ok(vec![]);
        }
        let text = response.text().await?;
        if !status.is_success() {
            bail!("listing {dir} on {branch}: {status}: {text}");
        }
        let entries: Vec<Entry> = serde_json::from_str(&text)
            .with_context(|| format!("listing {dir} on {branch}: not a directory"))?;
        Ok(entries
            .into_iter()
            .filter(|e| e.kind == "file")
            .map(|e| e.name)
            .collect())
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

    async fn close_issue(&self, issue: Number, reason: CloseReason) -> Result<()> {
        let reason = match reason {
            CloseReason::Completed => "completed",
            CloseReason::NotPlanned => "not_planned",
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
    use super::*;

    fn review_required(requests: &Value) -> GqlPull {
        serde_json::from_value(json!({
            "number": 20, "title": "Do the ticket", "body": "",
            "createdAt": "2026-10-09T12:00:00Z", "baseRefName": "main",
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
    fn an_issue_lists_its_labelings_unlabelings_and_assignments_with_who_made_them() {
        let issue: GqlIssue = serde_json::from_value(json!({
            "number": 12, "title": "Store gadgets", "body": "",
            "createdAt": "2026-10-09T12:00:00Z", "state": "OPEN",
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
            "createdAt": "2026-10-09T12:00:00Z", "state": "OPEN",
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
    /// tracking issue of tickets its account opened.
    #[test]
    fn an_issues_sub_issues_are_read_with_who_opened_them() {
        let nodes: Nodes<SubIssueNode> =
            serde_json::from_str(include_str!("fixtures/sub-issues.json")).unwrap();

        assert_eq!(
            sub_issues(Some(nodes)),
            [
                (374, false),
                (375, false),
                (376, false),
                (377, false),
                (378, true),
            ]
            .map(|(number, open)| SubIssue {
                linked: Linked {
                    number: Number(number),
                    open,
                },
                author: Login::new("tibbe-bot"),
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
                 "author": {"__typename": "User", "login": "alice"}},
                {"number": 41, "state": "MERGED", "isDraft": false,
                 "mergedAt": "2026-10-09T14:00:00Z", "closedAt": "2026-10-09T14:00:00Z",
                 "author": {"__typename": "User", "login": "alice"}},
                {"number": 43, "state": "OPEN", "isDraft": true, "mergedAt": null, "closedAt": null,
                 "author": {"__typename": "User", "login": "alice"}},
                {"number": 44, "state": "OPEN", "isDraft": false, "mergedAt": null, "closedAt": null,
                 "author": {"__typename": "Bot", "login": "renovate"}},
                {"number": 45, "state": "OPEN", "isDraft": false, "mergedAt": null, "closedAt": null,
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
}
