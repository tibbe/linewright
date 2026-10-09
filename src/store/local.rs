//! The local store: a repository and its issues and pull requests on disk,
//! standing in for GitHub in replay and tests. Its state lives in memory and
//! is exported as plain files (JSON, Markdown, diffs) a coding agent reads
//! directly. Besides the store interface it has the actions of people (open
//! an issue, comment, review, merge) that seed a case or drive a test.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Serialize;

use super::{Categories, CloseReason, Identity, NewIssue, NewPull, PushMode, RepoSettings, Store};
use crate::clock::Clock;
use crate::domain::{
    Check, CheckFailure, CheckState, Comment, Filed, Issue, IssueEvent, Linked, Login, MergeMethod,
    MergeMethods, Mergeability, MergedPull, Number, Pull, PullState, Push, Requirement, Review,
    ReviewState, Snapshot, Thread, Timestamp, closing_references,
};
use crate::git;

/// The repository a local store starts with.
pub struct Seed {
    pub default_branch: String,
    pub machine_user: Login,
    /// The repository's issue types, if it has them.
    pub issue_types: Option<Vec<String>>,
    /// Whether the default branch requires linear history.
    pub linear_history: bool,
    pub allowed_merge_methods: MergeMethods,
}

pub struct LocalStore {
    root: PathBuf,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
}

struct State {
    default_branch: String,
    machine_user: Login,
    issue_types: Option<Vec<String>>,
    linear_history: bool,
    allowed_merge_methods: MergeMethods,
    labels: BTreeSet<String>,
    next_number: u64,
    issues: BTreeMap<Number, IssueRecord>,
    pulls: BTreeMap<Number, PullRecord>,
}

#[derive(Clone, Serialize)]
struct IssueRecord {
    #[serde(flatten)]
    issue: Issue,
    issue_type: Option<String>,
    blocked_by: Vec<Number>,
    sub_issues: Vec<Number>,
}

#[derive(Clone, Serialize)]
pub struct CheckRecord {
    pub name: String,
    pub state: CheckState,
    pub log: String,
}

#[derive(Clone, Serialize)]
struct PullRecord {
    number: Number,
    title: String,
    body: String,
    author: Login,
    created_at: Timestamp,
    head_ref: String,
    state: PullState,
    labels: Vec<String>,
    pushes: Vec<Push>,
    comments: Vec<Comment>,
    reviews: Vec<Review>,
    threads: Vec<Thread>,
    requested_reviewers: Vec<Login>,
    /// What branch rules would require before the pull request merges, if
    /// they block it: the local store has no rules of its own.
    blocked: Option<Vec<Requirement>>,
    /// Checks by head commit: a push starts the head's checks afresh.
    checks: BTreeMap<String, Vec<CheckRecord>>,
    /// For a merged pull request: the default branch before and after.
    merge: Option<(String, String)>,
    files: Vec<String>,
}

impl LocalStore {
    pub async fn create(root: &Path, seed: Seed, clock: Arc<dyn Clock>) -> Result<LocalStore> {
        tokio::fs::create_dir_all(root).await?;
        let root = root.canonicalize()?;
        git::git(
            &root,
            &[
                "init",
                "--quiet",
                "--bare",
                "-b",
                &seed.default_branch,
                "repo.git",
            ],
        )
        .await?;
        Ok(LocalStore {
            root,
            clock,
            state: Mutex::new(State {
                default_branch: seed.default_branch,
                machine_user: seed.machine_user,
                issue_types: seed.issue_types,
                linear_history: seed.linear_history,
                allowed_merge_methods: seed.allowed_merge_methods,
                labels: BTreeSet::new(),
                next_number: 1,
                issues: BTreeMap::new(),
                pulls: BTreeMap::new(),
            }),
        })
    }

    /// The bare repository the store's clones come from.
    pub fn repository(&self) -> PathBuf {
        self.root.join("repo.git")
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("the local store's lock is not poisoned")
    }

    fn now(&self) -> Timestamp {
        self.clock.now()
    }

    // People's actions.

    /// Commits `files` (path, contents) as `author` on top of `branch`,
    /// creating it from the default branch when missing, and pushes it.
    pub async fn commit(
        &self,
        author: &str,
        branch: &str,
        files: &[(&str, &str)],
        message: &str,
    ) -> Result<String> {
        let work = tempfile::tempdir()?;
        let dir = work.path().join("clone");
        let default_branch = self.state().default_branch.clone();
        let url = self.url();
        let empty = git::git(
            &self.repository(),
            &["rev-parse", "--verify", "--quiet", &default_branch],
        )
        .await
        .is_err();
        if empty {
            git::git(work.path(), &["init", "--quiet", "-b", branch, "clone"]).await?;
            git::git(&dir, &["remote", "add", "origin", &url]).await?;
        } else {
            let has_branch = git::git(
                &self.repository(),
                &["rev-parse", "--verify", "--quiet", branch],
            )
            .await
            .is_ok();
            git::clone(
                &url,
                &dir,
                if has_branch { branch } else { &default_branch },
                &[],
            )
            .await?;
            git::create_branch(&dir, branch).await?;
        }
        git::set_identity(&dir, author, &format!("{author}@example.com")).await?;
        for (path, contents) in files {
            let path = dir.join(path);
            tokio::fs::create_dir_all(path.parent().expect("files live in a directory")).await?;
            tokio::fs::write(&path, contents).await?;
        }
        git::git(&dir, &["add", "--all"]).await?;
        git::git(&dir, &["commit", "--quiet", "-m", message]).await?;
        git::git(
            &dir,
            &["push", "--quiet", "origin", &format!("HEAD:{branch}")],
        )
        .await?;
        self.record_push(branch, &Login::new(author));
        git::head(&dir).await
    }

    pub fn open_issue(&self, author: &str, title: &str, body: &str, labels: &[&str]) -> Number {
        let now = self.now();
        let mut state = self.state();
        let number = state.take_number();
        state.issues.insert(
            number,
            IssueRecord {
                issue: Issue {
                    number,
                    title: title.into(),
                    body: body.into(),
                    author: Login::new(author),
                    open: true,
                    created_at: now,
                    labels: labels.iter().map(|l| l.to_string()).collect(),
                    assignees: vec![],
                    comments: vec![],
                    blocked_by: vec![],
                    sub_issues: vec![],
                    filed: None,
                    events: labels
                        .iter()
                        .map(|l| IssueEvent::Labeled {
                            label: l.to_string(),
                            at: now,
                        })
                        .collect(),
                },
                issue_type: None,
                blocked_by: vec![],
                sub_issues: vec![],
            },
        );
        number
    }

    pub fn comment_as(&self, author: &str, number: Number, body: &str) -> Result<()> {
        let comment = Comment {
            author: Login::new(author),
            body: body.into(),
            created_at: self.now(),
        };
        let mut state = self.state();
        if let Some(issue) = state.issues.get_mut(&number) {
            issue.issue.comments.push(comment);
        } else {
            state.pull_mut(number)?.comments.push(comment);
        }
        Ok(())
    }

    pub fn label_as(&self, number: Number, label: &str) -> Result<()> {
        let now = self.now();
        self.state().add_label(number, label, now)
    }

    pub fn unlabel_as(&self, number: Number, label: &str) -> Result<()> {
        self.state().remove_label(number, label)
    }

    pub fn assign_as(&self, issue: Number, login: &str) -> Result<()> {
        let mut state = self.state();
        let assignees = &mut state.issue_mut(issue)?.issue.assignees;
        let login = Login::new(login);
        if !assignees.contains(&login) {
            assignees.push(login);
        }
        Ok(())
    }

    pub fn unassign_as(&self, actor: &str, issue: Number, login: &str) -> Result<()> {
        let now = self.now();
        self.state()
            .unassign(issue, &Login::new(login), &Login::new(actor), now)
    }

    pub fn close_issue_as(&self, issue: Number) -> Result<()> {
        self.state().issue_mut(issue)?.issue.open = false;
        Ok(())
    }

    /// Fetches the commits of a git bundle into the repository, and points
    /// the default branch at `base`.
    pub async fn import_bundle(&self, bundle: &Path, base: &str) -> Result<()> {
        let bundle = bundle.canonicalize()?;
        let bundle = bundle.to_str().context("bundle paths are UTF-8")?;
        git::git(
            &self.repository(),
            &["fetch", "--quiet", bundle, "refs/*:refs/case/*"],
        )
        .await?;
        let default_branch = self.state().default_branch.clone();
        git::git(
            &self.repository(),
            &["update-ref", &format!("refs/heads/{default_branch}"), base],
        )
        .await?;
        Ok(())
    }

    /// A person's pull request whose merge moved the default branch to
    /// `merged`, a commit already in the repository.
    pub async fn merge_commit(
        &self,
        author: &str,
        title: &str,
        body: &str,
        merged: &str,
    ) -> Result<Number> {
        let default_branch = self.state().default_branch.clone();
        let before = self.rev(&default_branch).await?;
        let files: Vec<String> = git::git(
            &self.repository(),
            &["diff", "--name-only", &before, merged],
        )
        .await?
        .lines()
        .map(str::to_string)
        .collect();
        git::git(
            &self.repository(),
            &[
                "update-ref",
                &format!("refs/heads/{default_branch}"),
                merged,
            ],
        )
        .await?;
        let now = self.now();
        let mut state = self.state();
        let number = state.take_number();
        let branch = format!("change-{}", number.0);
        state.pulls.insert(
            number,
            PullRecord {
                state: PullState::Merged { at: now },
                merge: Some((before, merged.to_string())),
                files,
                ..PullRecord::new(number, title, body, Login::new(author), &branch, now)
            },
        );
        Ok(number)
    }

    /// A person's pull request touching `files`, merged into the default
    /// branch at once.
    pub async fn merge_change(
        &self,
        author: &str,
        title: &str,
        body: &str,
        files: &[(&str, &str)],
    ) -> Result<Number> {
        let branch = format!("change-{}", self.state().next_number);
        let default_branch = self.state().default_branch.clone();
        let before = self.rev(&default_branch).await?;
        let after = self.commit(author, &branch, files, title).await?;
        git::git(
            &self.repository(),
            &[
                "update-ref",
                &format!("refs/heads/{default_branch}"),
                &after,
            ],
        )
        .await?;
        let now = self.now();
        let mut state = self.state();
        let number = state.take_number();
        state.pulls.insert(
            number,
            PullRecord {
                state: PullState::Merged { at: now },
                merge: Some((before, after)),
                files: files.iter().map(|(path, _)| path.to_string()).collect(),
                ..PullRecord::new(number, title, body, Login::new(author), &branch, now)
            },
        );
        Ok(number)
    }

    /// A person merges an open pull request into the default branch, closing
    /// the issues it closes.
    pub async fn merge_pull(&self, pr: Number) -> Result<()> {
        self.merge_as(pr, "maintainer", MergeMethod::Merge).await
    }

    /// Merges an open pull request into the default branch with `method`,
    /// as GitHub does
    /// (<https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/about-merge-methods-on-github>),
    /// closing the issues it closes.
    async fn merge_as(&self, pr: Number, by: &str, method: MergeMethod) -> Result<()> {
        let (head_ref, title, default_branch) = {
            let mut state = self.state();
            let default_branch = state.default_branch.clone();
            let record = state.pull_mut(pr)?;
            (
                record.head_ref.clone(),
                record.title.clone(),
                default_branch,
            )
        };
        let before = self.rev(&default_branch).await?;
        let work = tempfile::tempdir()?;
        let dir = work.path().join("clone");
        git::clone(&self.url(), &dir, &default_branch, &[]).await?;
        git::set_identity(&dir, by, &format!("{by}@example.com")).await?;
        let head = format!("origin/{head_ref}");
        match method {
            MergeMethod::Merge => {
                let message = format!("Merge pull request {pr} from {head_ref}");
                git::git(
                    &dir,
                    &["merge", "--quiet", "--no-ff", "-m", &message, &head],
                )
                .await?;
            }
            MergeMethod::Squash => {
                git::git(&dir, &["merge", "--quiet", "--squash", &head]).await?;
                let message = format!("{title} ({pr})");
                git::git(&dir, &["commit", "--quiet", "-m", &message]).await?;
            }
            MergeMethod::Rebase => {
                git::git(&dir, &["checkout", "--quiet", "-B", "rebased", &head]).await?;
                git::git(&dir, &["rebase", "--quiet", &default_branch]).await?;
            }
        }
        git::git(
            &dir,
            &[
                "push",
                "--quiet",
                "origin",
                &format!("HEAD:{default_branch}"),
            ],
        )
        .await?;
        let after = git::head(&dir).await?;
        let files = git::changed_files(&dir, &before).await?;
        let now = self.now();
        let mut state = self.state();
        let record = state.pull_mut(pr)?;
        record.state = PullState::Merged { at: now };
        record.merge = Some((before, after));
        record.files = files;
        let closes = closing_references(&record.body);
        for issue in closes {
            if let Ok(issue) = state.issue_mut(issue) {
                issue.issue.open = false;
            }
        }
        Ok(())
    }

    /// Blocks the pull request's merge until `requires` is met, as branch
    /// rules on GitHub would, until [`LocalStore::allow_merge`].
    pub fn block_merge(&self, pr: Number, requires: Vec<Requirement>) {
        if let Ok(record) = self.state().pull_mut(pr) {
            record.blocked = Some(requires);
        }
    }

    /// Lifts [`LocalStore::block_merge`], as a person meeting what the rules
    /// require would.
    pub fn allow_merge(&self, pr: Number) {
        if let Ok(record) = self.state().pull_mut(pr) {
            record.blocked = None;
        }
    }

    pub fn close_pull_as(&self, pr: Number) -> Result<()> {
        let now = self.now();
        self.state().pull_mut(pr)?.state = PullState::Closed { at: now };
        Ok(())
    }

    /// A review, with an inline comment (path, body) opening a thread each.
    pub fn review_as(
        &self,
        author: &str,
        pr: Number,
        state: ReviewState,
        body: &str,
        inline: &[(&str, &str)],
    ) -> Result<()> {
        let now = self.now();
        let author = Login::new(author);
        let mut store = self.state();
        let record = store.pull_mut(pr)?;
        for (path, comment) in inline {
            let id = format!("T{}-{}", pr.0, record.threads.len() + 1);
            record.threads.push(Thread {
                id,
                path: path.to_string(),
                resolved: false,
                comments: vec![Comment {
                    author: author.clone(),
                    body: comment.to_string(),
                    created_at: now,
                }],
            });
        }
        record.reviews.push(Review {
            author,
            state,
            body: body.into(),
            submitted_at: now,
        });
        Ok(())
    }

    /// Sets the checks on the pull request's current head.
    pub async fn set_checks(&self, pr: Number, checks: Vec<CheckRecord>) -> Result<()> {
        let head_ref = self.state().pull_mut(pr)?.head_ref.clone();
        let head = self.rev(&head_ref).await?;
        self.state().pull_mut(pr)?.checks.insert(head, checks);
        Ok(())
    }

    // Reading the store's state, for tests.

    pub fn issue(&self, number: Number) -> Option<Issue> {
        let state = self.state();
        state.issues.get(&number).map(|r| state.issue_view(r))
    }

    pub fn issue_type(&self, number: Number) -> Option<String> {
        self.state()
            .issues
            .get(&number)
            .and_then(|r| r.issue_type.clone())
    }

    pub fn issues(&self) -> Vec<Issue> {
        let state = self.state();
        state.issues.values().map(|r| state.issue_view(r)).collect()
    }

    pub fn pull_labels(&self, pr: Number) -> Vec<String> {
        self.state()
            .pulls
            .get(&pr)
            .map(|p| p.labels.clone())
            .unwrap_or_default()
    }

    pub fn requested_reviewers(&self, pr: Number) -> Vec<Login> {
        self.state()
            .pulls
            .get(&pr)
            .map(|p| p.requested_reviewers.clone())
            .unwrap_or_default()
    }

    /// The pull request as a snapshot shows it.
    pub async fn snapshot_pull(&self, pr: Number) -> Pull {
        self.pull_view(pr).await.expect("the pull request exists")
    }

    pub fn pulls(&self) -> Vec<Number> {
        self.state().pulls.keys().copied().collect()
    }

    /// The contents of `path` at the head of `branch`, which has it.
    pub async fn file(&self, branch: &str, path: &str) -> Result<String> {
        Store::read_file(self, branch, path)
            .await?
            .with_context(|| format!("{branch} has no {path}"))
    }

    /// Writes the store's state as files under its root: `issues/<n>.json`
    /// and `.md`, `pulls/<n>.json`, `.md` and `.diff`.
    pub async fn export(&self) -> Result<()> {
        let issues = self.issues();
        let pulls: Vec<Number> = self.pulls();
        let issues_dir = self.root.join("issues");
        let pulls_dir = self.root.join("pulls");
        tokio::fs::create_dir_all(&issues_dir).await?;
        tokio::fs::create_dir_all(&pulls_dir).await?;
        for issue in issues {
            let n = issue.number.0;
            let issue_type = self.issue_type(issue.number);
            tokio::fs::write(
                issues_dir.join(format!("{n}.json")),
                serde_json::to_string_pretty(&issue)?,
            )
            .await?;
            tokio::fs::write(
                issues_dir.join(format!("{n}.md")),
                render_issue(&issue, issue_type.as_deref()),
            )
            .await?;
        }
        for number in pulls {
            let pull = self.pull_view(number).await?;
            let diff = self.pull_diff(number).await.unwrap_or_default();
            let n = number.0;
            tokio::fs::write(
                pulls_dir.join(format!("{n}.json")),
                serde_json::to_string_pretty(&pull)?,
            )
            .await?;
            tokio::fs::write(pulls_dir.join(format!("{n}.md")), render_pull(&pull)).await?;
            tokio::fs::write(pulls_dir.join(format!("{n}.diff")), diff).await?;
        }
        Ok(())
    }

    // Internals.

    fn url(&self) -> String {
        self.repository().to_string_lossy().into_owned()
    }

    async fn rev(&self, rev: &str) -> Result<String> {
        Ok(git::git(&self.repository(), &["rev-parse", rev])
            .await?
            .trim()
            .to_string())
    }

    fn record_push(&self, branch: &str, by: &Login) {
        let now = self.now();
        let mut state = self.state();
        for pull in state.pulls.values_mut() {
            if pull.head_ref == branch && matches!(pull.state, PullState::Open { .. }) {
                pull.pushes.push(Push {
                    by: by.clone(),
                    at: now,
                });
            }
        }
    }

    async fn pull_diff(&self, pr: Number) -> Result<String> {
        let (merge, head_ref, default_branch) = {
            let mut state = self.state();
            let default_branch = state.default_branch.clone();
            let record = state.pull_mut(pr)?;
            (
                record.merge.clone(),
                record.head_ref.clone(),
                default_branch,
            )
        };
        let range = match merge {
            Some((before, after)) => format!("{before}..{after}"),
            None => format!("{default_branch}...{head_ref}"),
        };
        git::git(&self.repository(), &["diff", &range]).await
    }

    async fn pull_view(&self, number: Number) -> Result<Pull> {
        let (record, default_branch) = {
            let mut state = self.state();
            let default_branch = state.default_branch.clone();
            (state.pull_mut(number)?.clone(), default_branch)
        };
        let head_sha = self.rev(&record.head_ref).await.unwrap_or_default();
        let mergeability = match (record.state, &record.blocked) {
            (PullState::Open { .. }, blocked) => {
                match self.mergeability(&default_branch, &record.head_ref).await? {
                    Mergeability::Clean if let Some(requires) = blocked => Mergeability::Blocked {
                        requires: requires.clone(),
                    },
                    other => other,
                }
            }
            _ => Mergeability::Clean,
        };
        let checks = record
            .checks
            .get(&head_sha)
            .map(|checks| {
                checks
                    .iter()
                    .map(|c| Check {
                        name: c.name.clone(),
                        state: c.state,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Pull {
            number,
            closes: closing_references(&record.body),
            title: record.title,
            body: record.body,
            author: record.author,
            created_at: record.created_at,
            head_ref: record.head_ref,
            head_sha,
            state: record.state,
            pushes: record.pushes,
            mergeability,
            checks,
            comments: record.comments,
            reviews: record.reviews,
            threads: record.threads,
        })
    }

    async fn mergeability(&self, default_branch: &str, head: &str) -> Result<Mergeability> {
        let repo = self.repository();
        if git::git(
            &repo,
            &["merge-base", "--is-ancestor", default_branch, head],
        )
        .await
        .is_ok()
        {
            return Ok(Mergeability::Clean);
        }
        let merges = git::git(&repo, &["merge-tree", "--write-tree", default_branch, head]).await;
        Ok(if merges.is_ok() {
            Mergeability::Behind
        } else {
            Mergeability::Conflicting
        })
    }
}

impl State {
    fn take_number(&mut self) -> Number {
        let number = Number(self.next_number);
        self.next_number += 1;
        number
    }

    fn issue_mut(&mut self, number: Number) -> Result<&mut IssueRecord> {
        self.issues
            .get_mut(&number)
            .with_context(|| format!("no issue {number}"))
    }

    fn pull_mut(&mut self, number: Number) -> Result<&mut PullRecord> {
        self.pulls
            .get_mut(&number)
            .with_context(|| format!("no pull request {number}"))
    }

    fn linked(&self, numbers: &[Number]) -> Vec<Linked> {
        numbers
            .iter()
            .map(|&number| Linked {
                number,
                open: self.issues.get(&number).is_some_and(|i| i.issue.open),
            })
            .collect()
    }

    fn issue_view(&self, record: &IssueRecord) -> Issue {
        let issue = &record.issue;
        Issue {
            blocked_by: self.linked(&record.blocked_by),
            sub_issues: self.linked(&record.sub_issues),
            filed: Filed::read(&issue.author, &issue.title, &issue.body, &self.machine_user),
            ..issue.clone()
        }
    }

    fn add_label(&mut self, number: Number, label: &str, now: Timestamp) -> Result<()> {
        if let Some(issue) = self.issues.get_mut(&number) {
            if !issue.issue.has_label(label) {
                issue.issue.labels.push(label.into());
            }
            issue.issue.events.push(IssueEvent::Labeled {
                label: label.into(),
                at: now,
            });
        } else {
            let labels = &mut self.pull_mut(number)?.labels;
            if !labels.iter().any(|l| l == label) {
                labels.push(label.into());
            }
        }
        Ok(())
    }

    fn remove_label(&mut self, number: Number, label: &str) -> Result<()> {
        if let Some(issue) = self.issues.get_mut(&number) {
            issue.issue.labels.retain(|l| l != label);
        } else {
            self.pull_mut(number)?.labels.retain(|l| l != label);
        }
        Ok(())
    }

    fn unassign(
        &mut self,
        issue: Number,
        login: &Login,
        actor: &Login,
        now: Timestamp,
    ) -> Result<()> {
        let issue = &mut self.issue_mut(issue)?.issue;
        if issue.is_assigned(login) {
            issue.assignees.retain(|a| a != login);
            issue.events.push(IssueEvent::Unassigned {
                assignee: login.clone(),
                actor: actor.clone(),
                at: now,
            });
        }
        Ok(())
    }
}

impl PullRecord {
    fn new(
        number: Number,
        title: &str,
        body: &str,
        author: Login,
        head_ref: &str,
        now: Timestamp,
    ) -> PullRecord {
        PullRecord {
            number,
            title: title.into(),
            body: body.into(),
            author,
            created_at: now,
            head_ref: head_ref.into(),
            state: PullState::Open { draft: true },
            labels: vec![],
            pushes: vec![],
            comments: vec![],
            reviews: vec![],
            threads: vec![],
            requested_reviewers: vec![],
            blocked: None,
            checks: BTreeMap::new(),
            merge: None,
            files: vec![],
        }
    }
}

#[async_trait]
impl Store for LocalStore {
    async fn identity(&self) -> Result<Identity> {
        let login = self.state().machine_user.clone();
        Ok(Identity {
            name: login.0.clone(),
            email: format!("{login}@users.noreply.github.com"),
            login,
        })
    }

    async fn settings(&self) -> Result<RepoSettings> {
        let state = self.state();
        let categories = match &state.issue_types {
            Some(types) => Categories::IssueTypes(types.clone()),
            None => Categories::Labels,
        };
        Ok(RepoSettings::new(
            state.default_branch.clone(),
            state.linear_history,
            state.allowed_merge_methods.clone(),
            categories,
        ))
    }

    async fn ensure_labels(&self, labels: &[(&str, &str)]) -> Result<()> {
        let mut state = self.state();
        for (name, _) in labels {
            state.labels.insert(name.to_string());
        }
        Ok(())
    }

    async fn snapshot(&self, merged_since: Timestamp) -> Result<Snapshot> {
        let (default_branch, issues, machine, pulls) = {
            let state = self.state();
            let issues: Vec<Issue> = state.issues.values().map(|r| state.issue_view(r)).collect();
            let pulls: Vec<(Number, Login)> = state
                .pulls
                .values()
                .map(|p| (p.number, p.author.clone()))
                .collect();
            (
                state.default_branch.clone(),
                issues,
                state.machine_user.clone(),
                pulls,
            )
        };
        let mut snapshot = Snapshot {
            default_branch: default_branch.clone(),
            issues,
            pulls: vec![],
            merged: vec![],
        };
        for (number, author) in pulls {
            let record = self.state().pull_mut(number)?.clone();
            if let PullState::Merged { at } = record.state
                && at >= merged_since
            {
                snapshot.merged.push(MergedPull {
                    number,
                    title: record.title.clone(),
                    body: record.body.clone(),
                    author: record.author.clone(),
                    merged_at: at,
                    base_ref: default_branch.clone(),
                    labels: record.labels.clone(),
                    files: record.files.clone(),
                });
            }
            if author == machine {
                snapshot.pulls.push(self.pull_view(number).await?);
            }
        }
        Ok(snapshot)
    }

    async fn pull(&self, number: Number) -> Result<Pull> {
        self.pull_view(number).await
    }

    async fn diff(&self, pr: Number, paths: &[String]) -> Result<String> {
        let (merge, head_ref, default_branch) = {
            let mut state = self.state();
            let default_branch = state.default_branch.clone();
            let record = state.pull_mut(pr)?;
            (
                record.merge.clone(),
                record.head_ref.clone(),
                default_branch,
            )
        };
        let range = match merge {
            Some((before, after)) => format!("{before}..{after}"),
            None => format!("{default_branch}...{head_ref}"),
        };
        let mut args = vec!["diff".to_string(), range, "--".into()];
        args.extend(paths.iter().cloned());
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        git::git(&self.repository(), &args).await
    }

    async fn check_failures(&self, pr: Number, checks: &[String]) -> Result<Vec<CheckFailure>> {
        let head_ref = self.state().pull_mut(pr)?.head_ref.clone();
        let head = self.rev(&head_ref).await?;
        let mut state = self.state();
        let records = state
            .pull_mut(pr)?
            .checks
            .get(&head)
            .cloned()
            .unwrap_or_default();
        Ok(records
            .into_iter()
            .filter(|c| checks.contains(&c.name))
            .map(|c| CheckFailure {
                name: c.name,
                summary: String::new(),
                log_tail: c.log,
            })
            .collect())
    }

    async fn read_file(&self, branch: &str, path: &str) -> Result<Option<String>> {
        let object = format!("{branch}:{path}");
        let repository = self.repository();
        if git::git(&repository, &["cat-file", "-e", &object])
            .await
            .is_err()
        {
            return Ok(None);
        }
        git::git(&repository, &["show", &object]).await.map(Some)
    }

    async fn list_files(&self, branch: &str, dir: &str) -> Result<Vec<String>> {
        let object = format!("{branch}:{dir}");
        let repository = self.repository();
        if git::git(&repository, &["cat-file", "-e", &object])
            .await
            .is_err()
        {
            return Ok(vec![]);
        }
        // Each entry is `<mode> <type> <object>\t<name>`
        // (<https://git-scm.com/docs/git-ls-tree#_output_format>).
        let listing = git::git(&repository, &["ls-tree", "-z", &object]).await?;
        Ok(listing
            .split('\0')
            .filter_map(|entry| {
                let (meta, name) = entry.split_once('\t')?;
                (meta.split(' ').nth(1) == Some("blob")).then(|| name.to_string())
            })
            .collect())
    }

    async fn clone_repo(&self, dir: &Path, branch: &str) -> Result<()> {
        git::clone(&self.url(), dir, branch, &[]).await
    }

    async fn push(&self, dir: &Path, branch: &str, mode: PushMode) -> Result<()> {
        let refspec = format!("HEAD:refs/heads/{branch}");
        let lease;
        let mut args = vec!["push", "--quiet", "origin", &refspec];
        match &mode {
            PushMode::Replace => args.push("--force"),
            PushMode::Lease { expected } => {
                lease = format!("--force-with-lease=refs/heads/{branch}:{expected}");
                args.push(&lease);
            }
        }
        git::git(dir, &args).await?;
        let machine = self.state().machine_user.clone();
        self.record_push(branch, &machine);
        Ok(())
    }

    async fn add_label(&self, number: Number, label: &str) -> Result<()> {
        let now = self.now();
        self.state().add_label(number, label, now)
    }

    async fn remove_label(&self, number: Number, label: &str) -> Result<()> {
        self.state().remove_label(number, label)
    }

    async fn assign(&self, issue: Number) -> Result<()> {
        let machine = self.state().machine_user.clone();
        self.assign_as(issue, machine.as_str())
    }

    async fn unassign(&self, issue: Number) -> Result<()> {
        let now = self.now();
        let mut state = self.state();
        let machine = state.machine_user.clone();
        state.unassign(issue, &machine, &machine, now)
    }

    async fn comment(&self, number: Number, body: &str) -> Result<()> {
        let machine = self.state().machine_user.clone();
        self.comment_as(machine.as_str(), number, body)
    }

    async fn create_issue(&self, issue: NewIssue) -> Result<Number> {
        let machine = self.state().machine_user.clone();
        let labels: Vec<&str> = issue.labels.iter().map(String::as_str).collect();
        Ok(self.open_issue(machine.as_str(), &issue.title, &issue.body, &labels))
    }

    async fn edit_body(&self, number: Number, body: &str) -> Result<()> {
        let mut state = self.state();
        if let Some(issue) = state.issues.get_mut(&number) {
            issue.issue.body = body.into();
        } else {
            state.pull_mut(number)?.body = body.into();
        }
        Ok(())
    }

    async fn close_issue(&self, issue: Number, _reason: CloseReason) -> Result<()> {
        self.close_issue_as(issue)
    }

    async fn set_issue_type(&self, issue: Number, issue_type: &str) -> Result<()> {
        self.state().issue_mut(issue)?.issue_type = Some(issue_type.into());
        Ok(())
    }

    async fn add_sub_issue(&self, parent: Number, child: Number) -> Result<()> {
        self.state().issue_mut(child)?;
        self.state().issue_mut(parent)?.sub_issues.push(child);
        Ok(())
    }

    async fn add_blocked_by(&self, issue: Number, blocker: Number) -> Result<()> {
        self.state().issue_mut(blocker)?;
        self.state().issue_mut(issue)?.blocked_by.push(blocker);
        Ok(())
    }

    async fn open_pull(&self, pull: NewPull) -> Result<Number> {
        if git::git(
            &self.repository(),
            &["rev-parse", "--verify", "--quiet", &pull.head],
        )
        .await
        .is_err()
        {
            bail!("no branch {}", pull.head);
        }
        let now = self.now();
        let mut state = self.state();
        let number = state.take_number();
        let author = state.machine_user.clone();
        state.pulls.insert(
            number,
            PullRecord::new(number, &pull.title, &pull.body, author, &pull.head, now),
        );
        Ok(number)
    }

    async fn mark_ready(&self, pr: Number) -> Result<()> {
        self.state().pull_mut(pr)?.state = PullState::Open { draft: false };
        Ok(())
    }

    async fn request_review(&self, pr: Number, reviewers: &[Login]) -> Result<()> {
        let mut state = self.state();
        let requested = &mut state.pull_mut(pr)?.requested_reviewers;
        for reviewer in reviewers {
            if !requested.contains(reviewer) {
                requested.push(reviewer.clone());
            }
        }
        Ok(())
    }

    async fn merge(&self, pr: Number, head: &str, method: MergeMethod) -> Result<()> {
        let (state, head_ref, blocked) = {
            let mut state = self.state();
            let record = state.pull_mut(pr)?;
            (
                record.state,
                record.head_ref.clone(),
                record.blocked.is_some(),
            )
        };
        if blocked {
            bail!("the branch rules block merging {pr}");
        }
        match state {
            PullState::Open { draft: false } => {}
            PullState::Open { draft: true } => bail!("{pr} is a draft"),
            _ => bail!("{pr} is not open"),
        }
        if self.rev(&head_ref).await? != head {
            bail!("the head of {pr} is no longer {head}");
        }
        let machine = self.state().machine_user.clone();
        self.merge_as(pr, machine.as_str(), method).await
    }

    async fn reply_in_thread(&self, pr: Number, thread: &str, body: &str) -> Result<()> {
        let now = self.now();
        let mut state = self.state();
        let machine = state.machine_user.clone();
        let thread = state
            .pull_mut(pr)?
            .threads
            .iter_mut()
            .find(|t| t.id == thread)
            .with_context(|| format!("no thread {thread} on {pr}"))?;
        thread.comments.push(Comment {
            author: machine,
            body: body.into(),
            created_at: now,
        });
        Ok(())
    }

    async fn resolve_thread(&self, pr: Number, thread: &str) -> Result<()> {
        let mut state = self.state();
        let thread = state
            .pull_mut(pr)?
            .threads
            .iter_mut()
            .find(|t| t.id == thread)
            .with_context(|| format!("no thread {thread} on {pr}"))?;
        thread.resolved = true;
        Ok(())
    }
}

fn render_comments(out: &mut String, comments: &[Comment]) {
    for comment in comments {
        let _ = writeln!(
            out,
            "---\n\n**@{}** at {}:\n\n{}\n",
            comment.author,
            comment.created_at.to_rfc3339(),
            comment.body.trim()
        );
    }
}

fn render_issue(issue: &Issue, issue_type: Option<&str>) -> String {
    let mut out = String::new();
    let numbers = |linked: &[Linked]| {
        linked
            .iter()
            .map(|l| format!("{}{}", l.number, if l.open { "" } else { " (closed)" }))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _ = writeln!(
        out,
        "# {} {}\n\n- State: {}\n- Author: @{}\n- Type: {}\n- Labels: {}\n- Assignees: {}\n- Blocked by: {}\n- Sub-issues: {}\n\n{}\n",
        issue.number,
        issue.title,
        if issue.open { "open" } else { "closed" },
        issue.author,
        issue_type.unwrap_or("none"),
        issue.labels.join(", "),
        issue
            .assignees
            .iter()
            .map(|a| a.0.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        numbers(&issue.blocked_by),
        numbers(&issue.sub_issues),
        issue.body.trim()
    );
    render_comments(&mut out, &issue.comments);
    out
}

fn render_pull(pull: &Pull) -> String {
    let mut out = String::new();
    let state = match pull.state {
        PullState::Open { draft: true } => "draft".to_string(),
        PullState::Open { draft: false } => "ready for review".to_string(),
        PullState::Merged { at } => format!("merged at {}", at.to_rfc3339()),
        PullState::Closed { at } => format!("closed at {}", at.to_rfc3339()),
    };
    let _ = writeln!(
        out,
        "# {} {}\n\n- State: {state}\n- Author: @{}\n- Branch: `{}`\n- Closes: {}\n- Checks: {}\n\n{}\n",
        pull.number,
        pull.title,
        pull.author,
        pull.head_ref,
        pull.closes
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        pull.checks
            .iter()
            .map(|c| format!("{} {:?}", c.name, c.state))
            .collect::<Vec<_>>()
            .join(", "),
        pull.body.trim()
    );
    render_comments(&mut out, &pull.comments);
    for review in &pull.reviews {
        let _ = writeln!(
            out,
            "---\n\n**Review by @{}** ({:?}) at {}:\n\n{}\n",
            review.author,
            review.state,
            review.submitted_at.to_rfc3339(),
            review.body.trim()
        );
    }
    for thread in &pull.threads {
        let _ = writeln!(
            out,
            "---\n\n### Thread `{}` on `{}`{}\n",
            thread.id,
            thread.path,
            if thread.resolved { " (resolved)" } else { "" }
        );
        render_comments(&mut out, &thread.comments);
    }
    out
}
