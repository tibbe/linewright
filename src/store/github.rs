//! GitHub as the store, through its REST and GraphQL APIs
//! (<https://docs.github.com/en/rest>, <https://docs.github.com/en/graphql>).
//! Clones and pushes go over HTTPS with the token in a header passed through
//! the environment, so it never lands in a clone's configuration.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use base64::Engine;
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::rate_limit;
use super::{Categories, CloseReason, Identity, NewIssue, NewPull, PushMode, RepoSettings, Store};
use crate::domain::{
    Check, CheckFailure, CheckState, Comment, Filed, Issue, IssueEvent, Linked, Login, MergeMethod,
    Mergeability, MergedPull, Number, Pull, PullState, Push, Review, ReviewState, Snapshot, Thread,
    Timestamp, label,
};
use crate::git;

const API: &str = "https://api.github.com";

/// The environment variables `gh` reads a token from, in its order
/// (<https://cli.github.com/manual/gh_help_environment>).
pub const TOKEN_VARIABLES: [&str; 2] = ["GH_TOKEN", "GITHUB_TOKEN"];

pub struct GitHub {
    http: reqwest::Client,
    token: String,
    owner: String,
    name: String,
    viewer: Viewer,
}

#[derive(Clone, Deserialize)]
struct Viewer {
    login: String,
    id: u64,
}

impl GitHub {
    /// Connects to `owner/name` as the token's user.
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

    /// A REST resource of the repository (`path` relative to it), for
    /// capturing cases.
    pub async fn repo_get(&self, path: &str) -> Result<Value> {
        self.rest(Method::GET, &self.repo_path(path), None).await
    }

    /// `owner/name`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    pub fn machine_user(&self) -> Login {
        self.machine()
    }

    fn machine(&self) -> Login {
        Login::new(&self.viewer.login)
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
                bail!("{method} {path}: {status}: {text}");
            }
            break text;
        };
        let text = if text.is_empty() { "null" } else { &text };
        serde_json::from_str(text).with_context(|| format!("decoding {method} {path}"))
    }

    async fn rest_ok(&self, method: Method, path: &str, body: Option<Value>) -> Result<()> {
        self.rest::<Value>(method, path, body).await.map(|_| ())
    }

    async fn graphql<T: DeserializeOwned>(&self, query: &str, variables: Value) -> Result<T> {
        #[derive(Deserialize)]
        struct Response<T> {
            data: Option<T>,
            #[serde(default)]
            errors: Vec<Value>,
        }
        let response: Response<T> = self
            .rest(
                Method::POST,
                "/graphql",
                Some(json!({"query": query, "variables": variables})),
            )
            .await?;
        if !response.errors.is_empty() {
            bail!("GraphQL errors: {}", Value::Array(response.errors));
        }
        response.data.context("GraphQL returned no data")
    }

    /// Git configuration in the environment that authenticates to GitHub.
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

    /// The repository's open pull requests, by number and author only: the
    /// full fields cost points per pull request, so only the machine user's
    /// are read in full.
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

    /// Every path a pull request touched, past the first page GraphQL gives.
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
                  nodes {
                    number title body mergedAt updatedAt baseRefName
                    author { __typename login }
                    labels(first: 50) { nodes { name } }
                    files(first: 100) { pageInfo { hasNextPage } nodes { path } }
                  }
                }
              }
            }";
        let mut merged = vec![];
        let mut cursor = Value::Null;
        loop {
            let data: RepoData<PullsData<GqlMerged>> = self
                .graphql(
                    query,
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
                let Some(merged_at) = pr.merged_at.filter(|at| *at >= since) else {
                    continue;
                };
                merged.push(MergedPull {
                    number: Number(pr.number),
                    title: pr.title,
                    body: pr.body,
                    author: login(pr.author),
                    merged_at,
                    base_ref: pr.base_ref_name,
                    labels: pr.labels.nodes.into_iter().map(|l| l.name).collect(),
                    files: match pr.files {
                        Some(files) if !files.page_info.has_next_page => {
                            files.nodes.into_iter().map(|f| f.path).collect()
                        }
                        _ => self.all_files(pr.number).await?,
                    },
                });
            }
            if older || !page.page_info.has_next_page {
                return Ok(merged);
            }
            cursor = json!(page.page_info.end_cursor);
        }
    }

    /// Pushes to the branch, oldest first, from the repository's activity
    /// (<https://docs.github.com/en/rest/repos/repos#list-repository-activities>).
    async fn pushes(&self, branch: &str) -> Result<Vec<Push>> {
        #[derive(Deserialize)]
        struct Activity {
            timestamp: Timestamp,
            activity_type: String,
            actor: Option<RestUser>,
        }
        let path = self.repo_path(&format!("/activity?ref=refs/heads/{branch}&per_page=30"));
        let activity: Vec<Activity> = self.rest(Method::GET, &path, None).await?;
        let mut pushes: Vec<Push> = activity
            .into_iter()
            .filter(|a| {
                matches!(
                    a.activity_type.as_str(),
                    "push" | "force_push" | "branch_creation"
                )
            })
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

    async fn pull_view(&self, pr: GqlPull) -> Result<Pull> {
        let pushes = if pr.state == "OPEN" {
            self.pushes(&pr.head_ref_name).await?
        } else {
            vec![]
        };
        Ok(pr.into_pull(pushes))
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
        #[derive(Deserialize)]
        struct Rule {
            #[serde(rename = "type")]
            kind: String,
        }
        let rules: Result<Vec<Rule>> = self
            .rest(
                Method::GET,
                &self.repo_path(&format!("/rules/branches/{branch}")),
                None,
            )
            .await;
        if rules.is_ok_and(|rules| rules.iter().any(|r| r.kind == "required_linear_history")) {
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

    async fn job_log_tail(&self, job: u64) -> Option<String> {
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
        let text = response.text().await.ok()?;
        let lines: Vec<&str> = text.lines().collect();
        Some(lines[lines.len().saturating_sub(200)..].join("\n"))
    }
}

#[derive(Deserialize)]
struct RestUser {
    login: String,
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
        .map(|n| Linked {
            number: Number(n.number),
            open: n.state == "OPEN",
        })
        .collect()
}

const ISSUE_FIELDS: &str = "fragment IssueFields on Issue {
  number title body createdAt state
  author { __typename login }
  labels(first: 50) { nodes { name } }
  assignees(first: 20) { nodes { login } }
  comments(last: 100) { nodes { author { __typename login } body createdAt } }
  subIssues(first: 100) { nodes { number state } }
  blockedBy(first: 50) { nodes { number state } }
  closedByPullRequestsReferences(first: 20, includeClosedPrs: true) { nodes { number state author { __typename login } } }
  timelineItems(last: 100, itemTypes: [LABELED_EVENT, UNASSIGNED_EVENT]) {
    nodes {
      __typename
      ... on LabeledEvent { createdAt label { name } }
      ... on UnassignedEvent {
        createdAt
        actor { __typename login }
        assignee { __typename ... on Actor { login } }
      }
    }
  }
}";

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
    sub_issues: Option<Nodes<LinkedNode>>,
    blocked_by: Option<Nodes<LinkedNode>>,
    closed_by_pull_requests_references: Option<Nodes<ClosingPull>>,
    timeline_items: Nodes<GqlEvent>,
}

#[derive(Deserialize)]
struct ClosingPull {
    number: u64,
    state: String,
    author: Option<Actor>,
}

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum GqlEvent {
    #[serde(rename_all = "camelCase")]
    LabeledEvent { created_at: Timestamp, label: Name },
    #[serde(rename_all = "camelCase")]
    UnassignedEvent {
        created_at: Timestamp,
        actor: Option<Actor>,
        assignee: Option<Value>,
    },
}

impl GqlIssue {
    fn into_issue(self, machine_user: &Login) -> Issue {
        let i = self;
        let author = login(i.author);
        let filed = Filed::read(&author, &i.title, &i.body, machine_user);
        let events = i
            .timeline_items
            .nodes
            .into_iter()
            .filter_map(|e| match e {
                GqlEvent::LabeledEvent { created_at, label } => Some(IssueEvent::Labeled {
                    label: label.name,
                    at: created_at,
                }),
                GqlEvent::UnassignedEvent {
                    created_at,
                    actor,
                    assignee,
                } => {
                    let assignee: Actor = serde_json::from_value(assignee?).ok()?;
                    Some(IssueEvent::Unassigned {
                        assignee: login(Some(assignee)),
                        actor: login(actor),
                        at: created_at,
                    })
                }
            })
            .collect();
        Issue {
            number: Number(i.number),
            title: i.title,
            body: i.body,
            author,
            open: i.state == "OPEN",
            created_at: i.created_at,
            labels: i.labels.nodes.into_iter().map(|l| l.name).collect(),
            assignees: i
                .assignees
                .nodes
                .into_iter()
                .map(|a| Login::new(a.login))
                .collect(),
            comments: i.comments.nodes.into_iter().map(Comment::from).collect(),
            blocked_by: linked(i.blocked_by),
            sub_issues: linked(i.sub_issues),
            events,
            filed,
        }
    }
}

const PULL_FIELDS: &str = "fragment PullFields on PullRequest {
  number title body createdAt headRefName headRefOid isDraft state mergedAt closedAt mergeable mergeStateStatus
  author { __typename login }
  closingIssuesReferences(first: 20) { nodes { number } }
  comments(last: 100) { nodes { author { __typename login } body createdAt } }
  reviews(last: 100) { nodes { author { __typename login } state body submittedAt } }
  reviewThreads(first: 100) {
    nodes { id path isResolved comments(first: 100) { nodes { author { __typename login } body createdAt } } }
  }
  statusCheckRollup {
    contexts(first: 100) {
      nodes {
        __typename
        ... on CheckRun { name status conclusion }
        ... on StatusContext { context state }
      }
    }
  }
}";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPull {
    number: u64,
    title: String,
    body: String,
    created_at: Timestamp,
    head_ref_name: String,
    head_ref_oid: String,
    is_draft: bool,
    state: String,
    merged_at: Option<Timestamp>,
    closed_at: Option<Timestamp>,
    mergeable: String,
    merge_state_status: String,
    author: Option<Actor>,
    closing_issues_references: Option<Nodes<NumberNode>>,
    comments: Nodes<GqlComment>,
    reviews: Nodes<GqlReview>,
    review_threads: Nodes<GqlThread>,
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
struct NumberNode {
    number: u64,
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
    CheckRun {
        name: String,
        status: String,
        conclusion: Option<String>,
    },
    StatusContext {
        context: String,
        state: String,
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
            } => Check {
                name: name.clone(),
                state: match (status.as_str(), conclusion.as_deref()) {
                    ("COMPLETED", Some("SUCCESS" | "NEUTRAL" | "SKIPPED")) => CheckState::Passed,
                    ("COMPLETED", _) => CheckState::Failed,
                    _ => CheckState::Pending,
                },
            },
            GqlContext::StatusContext { context, state } => Check {
                name: context.clone(),
                state: match state.as_str() {
                    "SUCCESS" => CheckState::Passed,
                    "FAILURE" | "ERROR" => CheckState::Failed,
                    _ => CheckState::Pending,
                },
            },
        }
    }
}

impl GqlPull {
    fn into_pull(self, pushes: Vec<Push>) -> Pull {
        let state = match (self.state.as_str(), self.merged_at, self.closed_at) {
            ("MERGED", Some(at), _) => PullState::Merged { at },
            ("CLOSED", _, Some(at)) => PullState::Closed { at },
            _ => PullState::Open {
                draft: self.is_draft,
            },
        };
        let mergeability = match (self.mergeable.as_str(), self.merge_state_status.as_str()) {
            ("CONFLICTING", _) | (_, "DIRTY") => Mergeability::Conflicting,
            (_, "BEHIND") => Mergeability::Behind,
            ("UNKNOWN", _) | (_, "UNKNOWN") => Mergeability::Unknown,
            _ => Mergeability::Clean,
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
            closes: self
                .closing_issues_references
                .map(|n| n.nodes.into_iter().map(|n| Number(n.number)).collect())
                .unwrap_or_default(),
            pushes,
            mergeability,
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
        }
    }
}

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
    files: Option<FilesPage>,
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
                "{} cannot push to {}/{}: the machine user needs write access",
                self.viewer.login,
                self.owner,
                self.name
            );
        }
        Ok(Identity {
            login: self.machine(),
            name: self.viewer.login.clone(),
            email: format!(
                "{}+{}@users.noreply.github.com",
                self.viewer.id, self.viewer.login
            ),
        })
    }

    async fn settings(&self) -> Result<RepoSettings> {
        #[derive(Deserialize)]
        struct Repo {
            default_branch: String,
        }
        let repo: Repo = self.rest(Method::GET, &self.repo_path(""), None).await?;
        // <https://docs.github.com/en/graphql/reference/objects#repository>
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
        let machine = &self.viewer.login;
        let mut issues: BTreeMap<u64, GqlIssue> = BTreeMap::new();
        for filter in [
            json!({"states": ["OPEN"], "labels": [label::NEEDS_TRIAGE, label::NEEDS_INFO, label::READY_FOR_AGENT]}),
            json!({"states": ["OPEN"], "assignee": machine}),
            json!({"states": ["OPEN"], "createdBy": machine}),
        ] {
            for issue in self.issues(filter).await? {
                issues.insert(issue.number, issue);
            }
        }
        let by_machine = |author: &Option<Actor>| {
            author
                .as_ref()
                .is_some_and(|a| &a.login == machine && a.typename != "Bot")
        };
        let mut pulls = vec![];
        for pr in self.open_pulls().await? {
            if by_machine(&pr.author) {
                pulls.push(self.pull(Number(pr.number)).await?);
            }
        }
        // The machine user's closed and merged pull requests for the issues in
        // view, however long ago: a refused pull request is not redone, and a
        // merged one is not redone while GitHub closes its issue.
        let closed: std::collections::BTreeSet<u64> = issues
            .values()
            .flat_map(|i| {
                i.closed_by_pull_requests_references
                    .iter()
                    .flat_map(|n| &n.nodes)
            })
            .filter(|p| matches!(p.state.as_str(), "CLOSED" | "MERGED") && by_machine(&p.author))
            .map(|p| p.number)
            .collect();
        for number in closed {
            pulls.push(self.pull(Number(number)).await?);
        }
        Ok(Snapshot {
            merged: self.merged_since(&default_branch, merged_since).await?,
            default_branch,
            issues: issues
                .into_values()
                .map(|i| i.into_issue(&self.machine_user()))
                .collect(),
            pulls,
        })
    }

    async fn pull(&self, number: Number) -> Result<Pull> {
        let query = format!(
            "query($owner: String!, $name: String!, $number: Int!) {{
              repository(owner: $owner, name: $name) {{ pullRequest(number: $number) {{ ...PullFields }} }}
            }}
            {PULL_FIELDS}"
        );
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct One {
            pull_request: GqlPull,
        }
        let data: RepoData<One> = self
            .graphql(
                &query,
                json!({"owner": self.owner, "name": self.name, "number": number.0}),
            )
            .await?;
        self.pull_view(data.repository.pull_request).await
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
                        ... on CheckRun { name databaseId title summary text }
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
                // A GitHub Actions check run's id is its job's id.
                let log_tail = match node["databaseId"].as_u64() {
                    Some(job) => self.job_log_tail(job).await.unwrap_or_default(),
                    None => String::new(),
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
                    log_tail,
                }
            } else {
                CheckFailure {
                    name,
                    summary: format!(
                        "{} ({})",
                        text(&node["description"]),
                        text(&node["targetUrl"])
                    ),
                    log_tail: String::new(),
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
        // <https://docs.github.com/en/rest/repos/contents#get-repository-content>
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
        // <https://docs.github.com/en/rest/repos/contents#get-repository-content>
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

    async fn push(&self, dir: &Path, branch: &str, mode: PushMode) -> Result<()> {
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
        .await?;
        Ok(())
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

    async fn assign(&self, issue: Number) -> Result<()> {
        self.rest_ok(
            Method::POST,
            &self.repo_path(&format!("/issues/{}/assignees", issue.0)),
            Some(json!({"assignees": [self.viewer.login]})),
        )
        .await
    }

    async fn unassign(&self, issue: Number) -> Result<()> {
        self.rest_ok(
            Method::DELETE,
            &self.repo_path(&format!("/issues/{}/assignees", issue.0)),
            Some(json!({"assignees": [self.viewer.login]})),
        )
        .await
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
        // <https://docs.github.com/en/rest/pulls/pulls#merge-a-pull-request>
        self.rest_ok(
            Method::PUT,
            &self.repo_path(&format!("/pulls/{}/merge", pr.0)),
            Some(json!({"sha": head, "merge_method": method})),
        )
        .await
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
