//! The store: where every fact Linewright decides on lives and every outcome
//! it records goes (`docs/specs/architecture.md`, "The store is an interface").
//! GitHub is the production store; a local store of plain files serves replay
//! and tests.

pub mod github;
mod job_log;
pub mod local;
pub mod rate_limit;

use std::path::Path;

use anyhow::{Result, bail};
use async_trait::async_trait;

use crate::domain::{
    CheckFailure, Issue, Login, MergeMethod, MergeMethods, MergedPull, Number, Pull, Snapshot,
    Timestamp, UpdateMethod, UpdateRule,
};

/// Who Linewright acts as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub login: Login,
    /// The author identity of the commits it makes.
    pub name: String,
    pub email: String,
}

/// How issues are categorized: the repository's issue types where it has
/// them, otherwise the `bug` and `enhancement` labels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Categories {
    IssueTypes(Vec<String>),
    Labels,
}

impl Categories {
    pub fn names(&self) -> Vec<String> {
        match self {
            Categories::IssueTypes(types) => types.clone(),
            Categories::Labels => vec![
                crate::domain::label::BUG.into(),
                crate::domain::label::ENHANCEMENT.into(),
            ],
        }
    }
}

/// The repository settings Linewright follows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoSettings {
    pub default_branch: String,
    pub update_rule: UpdateRule,
    pub merge_method: MergeMethod,
    /// The merge methods the repository allows.
    pub allowed_merge_methods: MergeMethods,
    pub categories: Categories,
}

impl RepoSettings {
    /// The settings of a repository whose default branch requires linear
    /// history, or not, and that allows `allowed_merge_methods`: branches
    /// are updated and merged by rebasing where it requires linear history,
    /// and otherwise updated by merging, or by rebasing on a conflict where
    /// it allows rebase merging, and merged with the first method it allows
    /// (`docs/specs/pull_requests.md`, "Keeping up to date" and "Auto-merge").
    pub fn new(
        default_branch: String,
        linear_history: bool,
        allowed_merge_methods: MergeMethods,
        categories: Categories,
    ) -> Self {
        let (update_rule, merge_method) = if linear_history {
            (
                UpdateRule::Always(UpdateMethod::Rebase),
                MergeMethod::Rebase,
            )
        } else {
            let update_rule = if allowed_merge_methods.contains(MergeMethod::Rebase) {
                UpdateRule::RebaseOnConflict
            } else {
                UpdateRule::Always(UpdateMethod::Merge)
            };
            let first_allowed = MergeMethod::ALL
                .into_iter()
                .find(|&method| allowed_merge_methods.contains(method));
            // A repository allows at least one method
            // (<https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/about-merge-methods-on-github>).
            (update_rule, first_allowed.unwrap_or(MergeMethod::Merge))
        };
        RepoSettings {
            default_branch,
            update_rule,
            merge_method,
            allowed_merge_methods,
            categories,
        }
    }

    /// Fails unless auto-merge can merge with the merge method: the
    /// repository allows it, and `update_branch`, the `update-branch`
    /// setting, keeps branches mergeable with it (`docs/specs/repository.md`).
    pub fn check_auto_merge(&self, update_branch: Option<UpdateMethod>) -> Result<()> {
        let method = self.merge_method;
        let allowed = &self.allowed_merge_methods;
        if !allowed.contains(method) {
            bail!(
                "the repository doesn't allow the merge method `{method}`: it allows {allowed}; \
                 set `merge-method` to a method it allows"
            );
        }
        if method == MergeMethod::Rebase && update_branch == Some(UpdateMethod::Merge) {
            bail!(
                "`update-branch` `merge` cannot keep branches rebase-mergeable, and the merge \
                 method is `rebase`: set `update-branch` to `rebase`, or `merge-method` to \
                 another method the repository allows (it allows {allowed})"
            );
        }
        Ok(())
    }
}

/// How a push treats the branch it replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushMode {
    /// Replace whatever the branch holds: a fresh run's new branch.
    Replace,
    /// Only if the branch still points at the commit the run started from.
    Lease { expected: String },
}

/// Why an issue is closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    Completed,
    NotPlanned,
}

/// An issue, as a snapshot taken now sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueRead {
    /// One the snapshot holds.
    Held {
        issue: Box<Issue>,
        /// Linewright's pull requests that close the issue, which a
        /// snapshot holds with it.
        pulls: Vec<Pull>,
    },
    /// One the snapshot's filters leave out, though the issues it holds may
    /// link to it.
    LeftOut { open: bool },
    /// One the store does not have, such as a deleted issue.
    Gone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewIssue {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewPull {
    pub head: String,
    pub title: String,
    pub body: String,
}

#[async_trait]
pub trait Store: Send + Sync {
    /// Who Linewright acts as; fails when it cannot push to the repository.
    async fn identity(&self) -> Result<Identity>;
    async fn settings(&self) -> Result<RepoSettings>;
    /// Creates any of `labels` (name, description) that are missing.
    async fn ensure_labels(&self, labels: &[(&str, &str)]) -> Result<()>;
    /// Whether the repository has the label `name`. Adding a label by name
    /// can create it, so a label Linewright must not create is checked first.
    async fn label_exists(&self, name: &str) -> Result<bool>;

    /// The facts one poll reconciles, with pull requests merged since
    /// `merged_since`.
    async fn snapshot(&self, merged_since: Timestamp) -> Result<Snapshot>;
    /// The issue as a snapshot taken now sees it.
    async fn issue(&self, number: Number) -> Result<IssueRead>;
    async fn pull(&self, number: Number) -> Result<Pull>;
    /// The pull request as a snapshot taken now holds it among Linewright's pull
    /// requests: `None` when the snapshot leaves it out, as it does anyone else's
    /// and one the store does not have.
    async fn linewright_pull(&self, number: Number) -> Result<Option<Pull>>;
    /// The pull request as a snapshot taken now, with pull requests merged
    /// since `merged_since`, holds it among the merged ones: `None` when the
    /// snapshot leaves it out of them, as it does one the store does not
    /// have.
    async fn merged_pull(
        &self,
        number: Number,
        merged_since: Timestamp,
    ) -> Result<Option<MergedPull>>;
    /// The pull request's changes to `paths`, as a unified diff.
    async fn diff(&self, pr: Number, paths: &[String]) -> Result<String>;
    async fn check_failures(&self, pr: Number, checks: &[String]) -> Result<Vec<CheckFailure>>;

    /// The contents of `path` at the head of `branch`, or `None` when it has
    /// no such file.
    async fn read_file(&self, branch: &str, path: &str) -> Result<Option<String>>;
    /// The names of the files directly in the directory `dir` at the head of
    /// `branch`: none when it has no such directory.
    async fn list_files(&self, branch: &str, dir: &str) -> Result<Vec<String>>;
    /// Clones the repository into `dir`, with `branch` checked out.
    async fn clone_repo(&self, dir: &Path, branch: &str) -> Result<()>;
    /// Pushes `HEAD` of the clone in `dir` to `branch`.
    async fn push(&self, dir: &Path, branch: &str, mode: PushMode) -> Result<()>;

    async fn add_label(&self, number: Number, label: &str) -> Result<()>;
    /// Removes the label if the issue or pull request has it.
    async fn remove_label(&self, number: Number, label: &str) -> Result<()>;
    /// Adds Linewright as an assignee.
    async fn assign(&self, issue: Number) -> Result<()>;
    async fn unassign(&self, issue: Number) -> Result<()>;
    async fn comment(&self, number: Number, body: &str) -> Result<()>;
    async fn create_issue(&self, issue: NewIssue) -> Result<Number>;
    async fn edit_body(&self, number: Number, body: &str) -> Result<()>;
    async fn close_issue(&self, issue: Number, reason: CloseReason) -> Result<()>;
    async fn set_issue_type(&self, issue: Number, issue_type: &str) -> Result<()>;
    async fn add_sub_issue(&self, parent: Number, child: Number) -> Result<()>;
    async fn add_blocked_by(&self, issue: Number, blocker: Number) -> Result<()>;

    /// Opens a draft pull request into the default branch.
    async fn open_pull(&self, pull: NewPull) -> Result<Number>;
    async fn mark_ready(&self, pr: Number) -> Result<()>;
    async fn request_review(&self, pr: Number, reviewers: &[Login]) -> Result<()>;
    /// Merges the pull request into the default branch with `method`, as
    /// Linewright, if its head is still the commit `head`.
    async fn merge(&self, pr: Number, head: &str, method: MergeMethod) -> Result<()>;
    async fn reply_in_thread(&self, pr: Number, thread: &str, body: &str) -> Result<()>;
    async fn resolve_thread(&self, pr: Number, thread: &str) -> Result<()>;
}
