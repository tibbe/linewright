pub mod github;
mod job_log;
pub mod local;
pub mod rate_limit;

use std::path::Path;

use anyhow::{Result, bail};
use async_trait::async_trait;

use crate::domain::{
    CheckFailure, Issue, Item, Login, MergeMethod, MergeMethods, MergedPull, Number, Pull,
    Snapshot, Timestamp, UpdateMethod, UpdateRule,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub login: Login,
    pub name: String,
    pub email: String,
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoSettings {
    pub default_branch: String,
    pub update_rule: UpdateRule,
    pub merge_method: MergeMethod,
    pub allowed_merge_methods: MergeMethods,
    pub categories: Categories,
}

impl RepoSettings {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushMode {
    Replace,
    Lease { expected: String },
}

#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("GitHub refused the push for a missing permission: {reason}")]
    MissingPermission { reason: String },
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

impl PushError {
    pub fn from_git(error: anyhow::Error) -> PushError {
        match missing_permission(&format!("{error:#}")) {
            Some(reason) => PushError::MissingPermission { reason },
            None => PushError::Failed(error),
        }
    }
}

/// GitHub's reason in the output of a `git push` it refused because the
/// token lacks a permission: a protected file such as a workflow, reported as
/// ` ! [remote rejected] <ref> -> <branch> (refusing to allow … without …)`,
/// or no write access to the repository, reported as `remote: Permission to
/// <repo> denied to <user>.`. Refusals that do not name a missing permission
/// are not recognized.
fn missing_permission(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        if let Some(at) = line.find("[remote rejected] ") {
            let (_, reason) = line[at..].split_once(" (")?;
            let reason = reason.trim_end().strip_suffix(')').unwrap_or(reason);
            return (reason.starts_with("refusing to allow ") && reason.contains(" without "))
                .then(|| reason.to_string());
        }
        let at = line.find("remote: Permission to ")?;
        let reason = line[at..].strip_prefix("remote: ")?.trim_end();
        reason.contains(" denied to ").then(|| reason.to_string())
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    Completed,
    NotPlanned,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueRead {
    Held {
        issue: Box<Issue>,
        /// Linewright's pull requests that close the issue.
        pulls: Vec<Pull>,
    },
    LeftOut {
        open: bool,
    },
    Gone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnyItem {
    HeldIssue(Box<Issue>),
    LeftOutIssue(Box<Issue>),
    /// A pull request open, or closed without merging.
    Pull(Box<Pull>),
    Merged {
        pull: Box<MergedPull>,
        tracking_issues: Vec<Number>,
    },
}

impl AnyItem {
    pub fn number(&self) -> Number {
        match self {
            AnyItem::HeldIssue(issue) | AnyItem::LeftOutIssue(issue) => issue.number,
            AnyItem::Pull(pull) => pull.number,
            AnyItem::Merged { pull, .. } => pull.number,
        }
    }

    pub fn item(&self) -> Item {
        match self {
            AnyItem::HeldIssue(_) | AnyItem::LeftOutIssue(_) => Item::Issue(self.number()),
            AnyItem::Pull(_) | AnyItem::Merged { .. } => Item::Pull(self.number()),
        }
    }
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
    /// Fails, saying what it lacks, when `token` cannot read the repository.
    async fn check_reads(&self, token: &str) -> Result<()>;
    async fn settings(&self) -> Result<RepoSettings>;
    /// Creates any of `labels` (name, description) that are missing.
    async fn ensure_labels(&self, labels: &[(&str, &str)]) -> Result<()>;
    /// Whether the repository has the label `name`. Adding a label by name
    /// can create it, so a label Linewright must not create is checked first.
    async fn label_exists(&self, name: &str) -> Result<bool>;

    /// The facts one poll reconciles, with pull requests merged since
    /// `merged_since` and every branch Linewright pushed for a ticket.
    async fn snapshot(&self, merged_since: Timestamp) -> Result<Snapshot>;
    /// The issue as a snapshot taken now sees it.
    async fn issue(&self, number: Number) -> Result<IssueRead>;
    async fn pull(&self, number: Number) -> Result<Pull>;
    /// The issue or pull request #`number`, open or not, whoever opened it:
    /// `None` when the store has neither.
    async fn any_item(&self, number: Number) -> Result<Option<AnyItem>>;
    /// The pull request as a snapshot taken now holds it among Linewright's pull
    /// requests: `None` when the snapshot leaves it out, as it does anyone else's
    /// and one the store does not have.
    async fn linewright_pull(&self, number: Number) -> Result<Option<Pull>>;
    /// The issues the pull request #`number` closes while it is open,
    /// whoever opened it: none when it is not open, or the store has no such
    /// pull request.
    async fn open_pull_closes(&self, number: Number) -> Result<Vec<Number>>;
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
    async fn clone_repo(&self, dir: &Path, branch: &str) -> Result<()>;
    async fn push(&self, dir: &Path, branch: &str, mode: PushMode) -> Result<(), PushError>;
    /// Deletes `branch`, if the repository has it.
    async fn delete_branch(&self, branch: &str) -> Result<()>;

    async fn add_label(&self, number: Number, label: &str) -> Result<()>;
    /// Removes the label if the issue or pull request has it.
    async fn remove_label(&self, number: Number, label: &str) -> Result<()>;
    /// Adds Linewright as an assignee of the issue or pull request.
    async fn assign(&self, number: Number) -> Result<()>;
    async fn unassign(&self, number: Number) -> Result<()>;
    /// Assigns `people` to the issue, as Linewright, and returns those of
    /// them GitHub refused to assign.
    async fn assign_people(&self, issue: Number, people: &[Login]) -> Result<Vec<Login>>;
    /// Unassigns `people` from the issue, as Linewright.
    async fn unassign_people(&self, issue: Number, people: &[Login]) -> Result<()>;
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
