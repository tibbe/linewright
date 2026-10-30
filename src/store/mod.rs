pub mod github;
mod job_log;
pub mod local;
pub mod rate_limit;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, bail};
use async_trait::async_trait;

use crate::domain::{
    CheckFailure, ClosedItem, Issue, IssueCloseReason, Item, Login, Marking, MergeMethod,
    MergeMethods, MergedPull, Number, Pull, Repo, Snapshot, UpdateMethod, UpdateRule, Windows,
    label,
};
use crate::redaction::Redaction;

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
    pub repo: Repo,
    pub default_branch: String,
    pub update_rule: UpdateRule,
    pub merge_method: MergeMethod,
    pub allowed_merge_methods: MergeMethods,
    pub categories: Categories,
}

impl RepoSettings {
    pub fn new(
        repo: Repo,
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
            repo,
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
    #[error("GitHub's push protection refused the push over a secret: {secrets}")]
    PushProtection { secrets: FoundSecrets },
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

impl PushError {
    pub fn from_git(error: anyhow::Error) -> PushError {
        let output = format!("{error:#}");
        if let Some(reason) = missing_permission(&output) {
            return PushError::MissingPermission { reason };
        }
        match found_secrets(&output) {
            Some(secrets) => PushError::PushProtection { secrets },
            None => PushError::Failed(error),
        }
    }
}

/// Why the store did not attach a file.
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    /// GitHub refuses the file itself, such as for its size or type.
    #[error("GitHub refused the file: {reason}")]
    RefusedFile { reason: String },
    /// GitHub refuses Linewright's token.
    #[error("GitHub refused Linewright's token: {reason}")]
    RefusedToken { reason: String },
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// A secret GitHub's push protection found in a commit pushed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundSecret {
    /// The secret's type, as GitHub names it, such as `GitHub Personal
    /// Access Token`.
    pub kind: String,
    /// The commit's full name.
    pub commit: String,
    /// The file, followed by `:` and the line, as GitHub gives it.
    pub path: String,
}

/// The secrets GitHub's push protection found in a push, at least one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundSecrets(Vec<FoundSecret>);

impl FoundSecrets {
    pub fn iter(&self) -> impl Iterator<Item = &FoundSecret> {
        self.0.iter()
    }

    /// The secrets, with `redaction` applied to what GitHub says of them.
    #[must_use]
    pub fn redacted(&self, redaction: &Redaction) -> FoundSecrets {
        FoundSecrets(
            self.iter()
                .map(|secret| FoundSecret {
                    kind: redaction.apply(&secret.kind),
                    commit: redaction.apply(&secret.commit),
                    path: redaction.apply(&secret.path),
                })
                .collect(),
        )
    }
}

impl std::fmt::Display for FoundSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let found: Vec<String> = self
            .iter()
            .map(|s| format!("{} in the commit `{}`, at `{}`", s.kind, s.commit, s.path))
            .collect();
        write!(f, "{}", found.join("; "))
    }
}

/// The secrets named in the output of a `git push` GitHub's push protection
/// refused, each under a `—— <type> ——` heading with its `- commit:` and
/// `path:` locations
/// (<https://docs.github.com/code-security/secret-scanning/working-with-secret-scanning-and-push-protection/working-with-push-protection-from-the-command-line>).
/// A refusal that names none is not recognized.
fn found_secrets(output: &str) -> Option<FoundSecrets> {
    let mut lines = output
        .lines()
        .map(str::trim)
        .map(|line| line.strip_prefix("remote:").unwrap_or(line).trim());
    lines.find(|line| line.ends_with("GITHUB PUSH PROTECTION"))?;
    let mut found = vec![];
    let mut kind = "";
    let mut commit = None;
    for line in lines {
        if let Some(heading) = line.strip_prefix("——") {
            kind = heading.trim_matches(|c: char| c == '—' || c.is_whitespace());
        } else if let Some(name) = line.strip_prefix("- commit:") {
            commit = Some(name.trim());
        } else if let Some(path) = line.strip_prefix("path:")
            && let Some(commit) = commit.take()
            && !kind.is_empty()
        {
            found.push(FoundSecret {
                kind: kind.to_string(),
                commit: commit.to_string(),
                path: path.trim().to_string(),
            });
        }
    }
    (!found.is_empty()).then_some(FoundSecrets(found))
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
    /// Left out of the open issues, and closed: one a snapshot may hold
    /// among its closed items.
    Closed(Box<ClosedItem>),
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

/// Whether a snapshot holds open `issue`: whether `linewright` manages it
/// ([`Issue::managed`]), with the pull requests GitHub says close it. A store
/// does not know the required label, so it counts every issue marked, and
/// `managed::classify` leaves one that is not alone.
pub fn holds(issue: &Issue, linewright: &Login) -> bool {
    issue.open && issue.managed(linewright, &issue.closed_by, Marking::Marked)
}

impl AnyItem {
    /// `issue`, held or left out as [`holds`] tells.
    pub fn issue(issue: Issue, linewright: &Login) -> AnyItem {
        if holds(&issue, linewright) {
            AnyItem::HeldIssue(Box::new(issue))
        } else {
            AnyItem::LeftOutIssue(Box::new(issue))
        }
    }

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

    /// As [`Issue::is_stalled`].
    pub fn is_stalled(&self) -> bool {
        match self {
            AnyItem::HeldIssue(issue) | AnyItem::LeftOutIssue(issue) => issue.is_stalled(),
            AnyItem::Pull(pull) => pull.is_stalled(),
            AnyItem::Merged { pull, .. } => pull.is_stalled(),
        }
    }
}

/// A file's contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blob {
    Text(String),
    /// Not UTF-8 text.
    Binary,
    /// Too large for the store to give.
    TooLarge,
}

impl Blob {
    /// The file holding `bytes`.
    pub fn from_bytes(bytes: Vec<u8>) -> Blob {
        String::from_utf8(bytes).map_or(Blob::Binary, Blob::Text)
    }

    /// The file's text, or why it cannot be read as text.
    pub fn text(self) -> Result<String, &'static str> {
        match self {
            Blob::Text(text) => Ok(text),
            Blob::Binary => Err("it is not UTF-8 text"),
            Blob::TooLarge => Err("it is too large to read"),
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
    /// Every label in the repository.
    async fn labels(&self) -> Result<Vec<label::RepoLabel>>;
    async fn create_label(&self, label: &label::Label) -> Result<()>;
    /// Sets the description of the label the repository spells `name`.
    async fn describe_label(&self, name: &str, description: &str) -> Result<()>;
    /// Whether the repository has the label `name`. Adding a label by name
    /// can create it, so a label Linewright must not create is checked first.
    async fn label_exists(&self, name: &str) -> Result<bool>;

    /// The facts one poll reconciles, with the merged pull requests and
    /// closed items `windows` holds, every closed ticket of a tracking issue
    /// it holds ([`Snapshot::unheld_closed_tickets`]), Linewright's pull
    /// requests closed without merging that close one
    /// ([`Snapshot::unheld_closing_pulls`]) and every branch Linewright
    /// pushed for a ticket.
    async fn snapshot(&self, windows: Windows) -> Result<Snapshot>;
    /// The issue as a snapshot taken now sees it, but a closed one whenever
    /// it closed: whether a snapshot holds it closed depends on the rest it
    /// holds ([`Snapshot::keeps_closed`]).
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
    /// The pull request as a snapshot taken now, over `windows`, holds it
    /// among the merged ones: `None` when the snapshot leaves it out of them,
    /// as it does one the store does not have.
    async fn merged_pull(&self, number: Number, windows: Windows) -> Result<Option<MergedPull>>;
    /// The pull request's changes to `paths`, as a unified diff.
    async fn diff(&self, pr: Number, paths: &[String]) -> Result<String>;
    async fn check_failures(&self, pr: Number, checks: &[String]) -> Result<Vec<CheckFailure>>;

    /// The contents of `path` at the head of `branch`, or `None` when it has
    /// no such file.
    async fn read_file(&self, branch: &str, path: &str) -> Result<Option<String>>;
    /// At the head of `branch`, each file of `paths` by its path, leaving out
    /// the paths at which it has no file.
    async fn read_files(&self, branch: &str, paths: &[String]) -> Result<BTreeMap<String, Blob>>;
    /// The path, relative to `dir`, of every file in the directory `dir` and
    /// its subdirectories at the head of `branch`, such as
    /// `prompts/follow-ups/result-error.md`: none when it has no such directory.
    async fn list_files(&self, branch: &str, dir: &str) -> Result<Vec<String>>;
    async fn clone_repo(&self, dir: &Path, branch: &str) -> Result<()>;
    async fn push(&self, dir: &Path, branch: &str, mode: PushMode) -> Result<(), PushError>;
    /// Uploads `file` as an attachment of the repository's Markdown, named
    /// `name`, and returns the URL to put in place of its path
    /// (`docs/specs/runs.md`, "Attachments").
    async fn attach(
        &self,
        file: &Path,
        name: &str,
        content_type: &str,
    ) -> Result<String, AttachError>;
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
    async fn close_issue(&self, issue: Number, reason: IssueCloseReason) -> Result<()>;
    async fn set_issue_type(&self, issue: Number, issue_type: &str) -> Result<()>;
    async fn add_sub_issue(&self, parent: Number, child: Number) -> Result<()>;
    async fn add_blocked_by(&self, issue: Number, blocker: Number) -> Result<()>;

    /// Opens a draft pull request into the default branch.
    async fn open_pull(&self, pull: NewPull) -> Result<Number>;
    async fn mark_ready(&self, pr: Number) -> Result<()>;
    /// Closes the pull request unmerged.
    async fn close_pull(&self, pr: Number) -> Result<()>;
    async fn request_review(&self, pr: Number, reviewers: &[Login]) -> Result<()>;
    /// Merges the pull request into the default branch with `method`, as
    /// Linewright, if its head is still the commit `head`.
    async fn merge(&self, pr: Number, head: &str, method: MergeMethod) -> Result<()>;
    async fn reply_in_thread(&self, pr: Number, thread: &str, body: &str) -> Result<()>;
    async fn resolve_thread(&self, pr: Number, thread: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_githubs_push_protection_refuses_names_each_secret_by_its_type_commit_and_path() {
        let output = include_str!("fixtures/push-protection.txt");
        let error = PushError::from_git(anyhow::anyhow!(
            "git push --quiet origin HEAD:refs/heads/344-send-the-result-back-to-the-session-when \
             --force failed: {output}"
        ));

        let PushError::PushProtection { secrets } = error else {
            panic!("not recognized as push protection: {error:?}");
        };
        assert_eq!(
            secrets.iter().collect::<Vec<_>>(),
            [&FoundSecret {
                kind: "GitHub Personal Access Token".to_string(),
                commit: "8479b4565e1a22d92de1bb6edb317cdc4dc64212".to_string(),
                path: "tests/integration/push_protection.rs:22".to_string(),
            }]
        );
    }
}
