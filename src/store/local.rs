use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use super::{
    AnyItem, AttachError, Blob, Categories, Identity, IssueRead, NewIssue, NewPull,
    PeoplesPullRead, PushError, PushMode, RepoSettings, Store,
};
use crate::clock::Clock;
use crate::domain::{
    Assignment, BranchIssue, Check, CheckFailure, CheckLog, CheckState, ClosedItem, ClosedKind,
    ClosingPull, Comment, Filed, Issue, IssueCloseReason, IssueClosing, IssueEvent, Labeling,
    Linked, Login, MergeMethod, MergeMethods, Mergeability, MergedPull, Number, Parent, Pull,
    PullHeld, PullSigns, PullState, Push, Rebaseability, Repo, Requirement, Review, ReviewDecision,
    ReviewState, Snapshot, SubIssue, SubIssueState, Thread, TicketBranch, TicketBranchName,
    Timestamp, Windows, closed_within, closing_references, label, pull_events,
};
use crate::git::{self, Merged};

mod history;

use history::{MergeOf, NewCommit, Replayed};

/// The directory, beside the exported `issues/` and `pulls/`, that keeps
/// the attachments.
const ATTACHMENTS: &str = "attachments";

pub struct Seed {
    pub repo: Repo,
    pub default_branch: String,
    pub linewright: Login,
    pub issue_types: Option<Vec<String>>,
    pub linear_history: bool,
    pub allowed_merge_methods: MergeMethods,
}

pub struct LocalStore {
    root: PathBuf,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
}

struct State {
    repo: Repo,
    default_branch: String,
    linewright: Login,
    issue_types: Option<Vec<String>>,
    linear_history: bool,
    allowed_merge_methods: MergeMethods,
    /// Each label by name, with its description.
    labels: BTreeMap<String, String>,
    next_number: u64,
    issues: BTreeMap<Number, IssueRecord>,
    pulls: BTreeMap<Number, PullRecord>,
    snapshots_taken: usize,
    items_read: usize,
    unreadable: Option<String>,
    pushes_refused: Option<String>,
    attachments_refused: Option<AttachRefusal>,
    /// GitHub tokens that cannot read the repository.
    unreadable_by: Vec<String>,
    /// People GitHub would refuse to assign, such as those without access.
    unassignable: Vec<Login>,
    after_comment: Option<Box<dyn FnOnce() + Send>>,
    after_labeling: Option<AfterLabeling>,
    pause: Option<Pause>,
}

/// A write of Linewright's that [`LocalStore::pause_after`] can pause a
/// store call after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Write {
    AddLabel {
        number: Number,
        label: String,
    },
    RemoveLabel {
        number: Number,
        label: String,
    },
    Unassign {
        number: Number,
    },
    AssignPeople {
        number: Number,
    },
    Comment {
        number: Number,
    },
    OpenPull {
        head: String,
    },
    Push {
        branch: String,
    },
    /// An upload of the attachment named `name`.
    Attach {
        name: String,
    },
}

/// How [`LocalStore::refuse_attachments`] has GitHub refuse an upload.
#[derive(Clone, Debug)]
pub enum AttachRefusal {
    /// Over the file itself, for `reason`.
    File(String),
    /// Over Linewright's token, for `reason`.
    Token(String),
}

/// The store's end of a [`Paused`].
struct Pause {
    after: Write,
    reached: oneshot::Sender<()>,
    released: oneshot::Receiver<Released>,
}

/// How a [`Paused`] store call returns once released.
enum Released {
    Ok,
    /// With an error, the write still applied, as when GitHub's answer to a
    /// write is lost.
    Failed,
}

/// A store call paused after a write, from [`LocalStore::pause_after`].
/// Dropping it releases the call.
pub struct Paused {
    reached: oneshot::Receiver<()>,
    release: oneshot::Sender<Released>,
}

impl Paused {
    /// Returns once the write has landed.
    pub async fn reached(&mut self) {
        (&mut self.reached)
            .await
            .expect("the paused write is reached before the store is dropped or paused elsewhere");
    }

    /// Lets the paused store call return.
    pub fn release(self) {
        // The call is gone only if the store was dropped, and then there is
        // nothing to release.
        let _ = self.release.send(Released::Ok);
    }

    /// Lets the paused store call return an error, its write applied.
    pub fn fail(self) {
        let _ = self.release.send(Released::Failed);
    }
}

/// What to run once Linewright next adds `label` to `number`.
struct AfterLabeling {
    number: Number,
    label: String,
    then: Box<dyn FnOnce() + Send>,
}

#[derive(Clone, Serialize)]
struct IssueRecord {
    /// Its `open` is [`IssueRecord::is_open`]'s.
    #[serde(flatten)]
    issue: Issue,
    issue_type: Option<String>,
    blocked_by: Vec<Number>,
    sub_issues: Vec<Number>,
    /// Set while the issue is closed.
    closing: Option<Closing>,
}

#[derive(Clone, Copy, Serialize)]
struct Closing {
    at: Timestamp,
    /// The pull request whose merge closed the issue.
    merged_by: Option<Number>,
    reason: IssueCloseReason,
}

impl IssueRecord {
    /// Its closing, while it is closed.
    fn issue_closing(&self) -> Option<IssueClosing> {
        self.closing.map(|closing| IssueClosing {
            at: closing.at,
            merged_by: closing.merged_by,
            reason: Some(closing.reason),
        })
    }
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
    updated_at: Timestamp,
    /// The branch it merges into: the default branch unless
    /// [`LocalStore::retarget`] changed it.
    base_ref: String,
    head_ref: String,
    state: PullState,
    labels: Vec<String>,
    /// Oldest first.
    labelings: Vec<Labeling>,
    assignees: Vec<Login>,
    /// Oldest first.
    assignments: Vec<Assignment>,
    pushes: Vec<Push>,
    comments: Vec<Comment>,
    reviews: Vec<Review>,
    threads: Vec<Thread>,
    requested_reviewers: Vec<Login>,
    /// Whether the branch rules require a review of it, which gives it a
    /// review decision.
    review_ruled: bool,
    blocked: Option<Vec<Requirement>>,
    merge_state_pending: bool,
    refused: Option<String>,
    /// By head commit: a push starts the head's checks afresh.
    checks: BTreeMap<String, Vec<CheckRecord>>,
    /// For a merged pull request: the default branch before and after.
    merge: Option<(String, String)>,
    files: Vec<String>,
}

impl LocalStore {
    pub async fn create(root: &Path, seed: Seed, clock: Arc<dyn Clock>) -> Result<LocalStore> {
        tokio::fs::create_dir_all(root).await?;
        let root = root.canonicalize()?;
        // Refs in a reftable rather than a file each under `refs/heads/`: on
        // a case-insensitive file system, as macOS's are by default, the
        // files would make `1-Gadgets` and `1-gadgets` the same branch.
        git::git(
            &root,
            &[
                "init",
                "--quiet",
                "--bare",
                "--ref-format=reftable",
                "-b",
                &seed.default_branch,
                "repo.git",
            ],
        )
        .await
        .context("the local store needs git 2.45 or later")?;
        Ok(LocalStore {
            root,
            clock,
            state: Mutex::new(State {
                repo: seed.repo,
                default_branch: seed.default_branch,
                linewright: seed.linewright,
                issue_types: seed.issue_types,
                linear_history: seed.linear_history,
                allowed_merge_methods: seed.allowed_merge_methods,
                labels: BTreeMap::new(),
                next_number: 1,
                issues: BTreeMap::new(),
                pulls: BTreeMap::new(),
                snapshots_taken: 0,
                items_read: 0,
                unreadable: None,
                pushes_refused: None,
                attachments_refused: None,
                unreadable_by: vec![],
                unassignable: vec![],
                after_comment: None,
                after_labeling: None,
                pause: None,
            }),
        })
    }

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

    pub async fn commit(
        &self,
        author: &str,
        branch: &str,
        files: &[(&str, &str)],
        message: &str,
    ) -> Result<String> {
        let files: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(path, text)| (*path, text.as_bytes()))
            .collect();
        self.commit_bytes(author, branch, &files, message).await
    }

    /// As [`LocalStore::commit`], with files that need not be text.
    pub async fn commit_bytes(
        &self,
        author: &str,
        branch: &str,
        files: &[(&str, &[u8])],
        message: &str,
    ) -> Result<String> {
        self.write_commit(author, branch, files, &[], message).await
    }

    /// Commits the removal of the files at `paths` on `branch`.
    pub async fn remove(
        &self,
        author: &str,
        branch: &str,
        paths: &[&str],
        message: &str,
    ) -> Result<String> {
        self.write_commit(author, branch, &[], paths, message).await
    }

    async fn write_commit(
        &self,
        author: &str,
        branch: &str,
        files: &[(&str, &[u8])],
        removed: &[&str],
        message: &str,
    ) -> Result<String> {
        let default_branch = self.state().default_branch.clone();
        let commit = history::write_commit(
            &self.repository(),
            NewCommit {
                branch,
                default_branch: &default_branch,
                author,
                message,
                files,
                removed,
            },
        )
        .await?;
        self.record_push(branch, &Login::new(author));
        Ok(commit)
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
                    updated_at: now,
                    labels: labels.iter().map(ToString::to_string).collect(),
                    assignees: vec![],
                    comments: vec![],
                    blocked_by: vec![],
                    sub_issues: vec![],
                    parent: None,
                    filed: None,
                    closed_by: vec![],
                    events: labels
                        .iter()
                        .map(|l| IssueEvent::Labeled {
                            label: l.to_string(),
                            by: Some(Login::new(author)),
                            at: now,
                        })
                        .collect(),
                },
                issue_type: None,
                blocked_by: vec![],
                sub_issues: vec![],
                closing: None,
            },
        );
        number
    }

    pub fn comment_as(&self, author: &str, number: Number, body: &str) -> Result<()> {
        let now = self.now();
        let comment = Comment {
            author: Login::new(author),
            body: body.into(),
            created_at: now,
        };
        match self.state().item_written(number, now)? {
            Written::Issue(record) => record.issue.comments.push(comment),
            Written::Pull(pull) => pull.comments.push(comment),
        }
        Ok(())
    }

    /// `actor` labels the issue or pull request #`number` `label`.
    pub fn label_as(&self, actor: &str, number: Number, label: &str) -> Result<()> {
        let now = self.now();
        self.state()
            .add_label(number, label, &Login::new(actor), now)
    }

    pub fn unlabel_as(&self, number: Number, label: &str) -> Result<()> {
        let now = self.now();
        self.state().remove_label(number, label, now)
    }

    /// `actor` assigns `login` to the issue or pull request #`number`.
    pub fn assign_as(&self, actor: &str, number: Number, login: &str) -> Result<()> {
        let now = self.now();
        self.state()
            .assign(number, &Login::new(login), &Login::new(actor), now)
    }

    /// `actor` unassigns `login` from the issue or pull request #`number`.
    pub fn unassign_as(&self, actor: &str, number: Number, login: &str) -> Result<()> {
        let now = self.now();
        self.state()
            .unassign(number, &Login::new(login), &Login::new(actor), now)
    }

    /// As GitHub closes it by default.
    pub fn close_issue_as_completed(&self, issue: Number) -> Result<()> {
        self.close_issue_as(issue, IssueCloseReason::Completed)
    }

    pub fn close_issue_as(&self, issue: Number, reason: IssueCloseReason) -> Result<()> {
        let now = self.now();
        self.state()
            .issue_written(issue, now)?
            .close(now, None, reason);
        Ok(())
    }

    pub fn reopen_issue_as(&self, issue: Number) -> Result<()> {
        let now = self.now();
        let mut state = self.state();
        let record = state.issue_written(issue, now)?;
        record.closing = None;
        record.issue.events.push(IssueEvent::Reopened { at: now });
        Ok(())
    }

    pub async fn import_bundle(&self, bundle: &Path, base: &str) -> Result<()> {
        let bundle = bundle.canonicalize()?;
        let default_branch = self.state().default_branch.clone();
        history::import_bundle(&self.repository(), &bundle, &default_branch, base).await
    }

    /// `merged` is a commit already in the repository.
    pub async fn merge_commit(
        &self,
        author: &str,
        title: &str,
        body: &str,
        merged: &str,
    ) -> Result<Number> {
        let default_branch = self.state().default_branch.clone();
        let before = self.rev(&default_branch).await?;
        let files = self.changed_files(&before, merged).await?;
        history::move_ref(&self.repository(), &default_branch, merged, &before).await?;
        let now = self.now();
        let mut state = self.state();
        let number = state.take_number();
        let branch = format!("change-{}", number.0);
        let record = PullRecord {
            state: PullState::Merged { at: now },
            merge: Some((before, merged.to_string())),
            files,
            ..PullRecord::new(
                &state,
                number,
                title,
                body,
                Login::new(author),
                &branch,
                now,
            )
        };
        state.pulls.insert(number, record);
        Ok(number)
    }

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
        history::move_ref(&self.repository(), &default_branch, &after, &before).await?;
        let now = self.now();
        let mut state = self.state();
        let number = state.take_number();
        let record = PullRecord {
            state: PullState::Merged { at: now },
            merge: Some((before, after)),
            files: files.iter().map(|(path, _)| path.to_string()).collect(),
            ..PullRecord::new(
                &state,
                number,
                title,
                body,
                Login::new(author),
                &branch,
                now,
            )
        };
        state.pulls.insert(number, record);
        Ok(number)
    }

    pub async fn merge_pull(&self, pr: Number) -> Result<()> {
        self.merge_and_close(pr, "maintainer", MergeMethod::Merge)
            .await
    }

    /// As [`LocalStore::merge_pull`], up to GitHub closing the issues it
    /// closes, which it does moments after the merge.
    pub async fn merge_pull_before_its_issues_close(&self, pr: Number) -> Result<()> {
        self.merge_as(pr, "maintainer", MergeMethod::Merge).await?;
        Ok(())
    }

    /// Has #`pr` merge into `base`, a branch the repository holds, as
    /// changing a pull request's base does
    /// (<https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/proposing-changes-to-your-work-with-pull-requests/changing-the-base-branch-of-a-pull-request>).
    pub fn retarget(&self, pr: Number, base: &str) -> Result<()> {
        let now = self.now();
        self.state().pull_written(pr, now)?.base_ref = base.into();
        Ok(())
    }

    /// As GitHub merges
    /// (<https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/about-merge-methods-on-github>),
    /// returning when it merged.
    async fn merge_as(&self, pr: Number, by: &str, method: MergeMethod) -> Result<Timestamp> {
        let (head_ref, title, base_ref) = {
            let state = self.state();
            let record = state.pull(pr)?;
            (
                record.head_ref.clone(),
                record.title.clone(),
                record.base_ref.clone(),
            )
        };
        let before = self.rev(&base_ref).await?;
        let repo = self.repository();
        let after = history::merge(
            &repo,
            MergeOf {
                into: &base_ref,
                before: &before,
                head: &head_ref,
                method,
                by,
                title: &title,
                pr,
            },
        )
        .await?;
        let files = self.changed_files(&before, &after).await?;
        let now = self.now();
        let mut state = self.state();
        let record = state.pull_written(pr, now)?;
        record.state = PullState::Merged { at: now };
        record.merge = Some((before, after));
        record.files = files;
        Ok(now)
    }

    /// As [`LocalStore::merge_as`], then GitHub closing the issues #`pr`
    /// closes.
    async fn merge_and_close(&self, pr: Number, by: &str, method: MergeMethod) -> Result<()> {
        let merged_at = self.merge_as(pr, by, method).await?;
        self.close_issues_merged_by(pr, merged_at)
    }

    /// As GitHub closes the issues a pull request closes once it merged
    /// into the default branch, and only then, at `now`
    /// (<https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/linking-a-pull-request-to-an-issue>).
    fn close_issues_merged_by(&self, pr: Number, now: Timestamp) -> Result<()> {
        let mut state = self.state();
        let record = state.pull(pr)?;
        if record.base_ref != state.default_branch {
            return Ok(());
        }
        for issue in closing_references(&record.body) {
            if let Ok(issue) = state.issue_written(issue, now) {
                issue.close(now, Some(pr), IssueCloseReason::Completed);
            }
        }
        Ok(())
    }

    // The branch rules and GitHub's working out of whether a pull request
    // merges are no writes to it, so they leave when it was last updated.
    pub fn block_merge(&self, pr: Number, requires: Vec<Requirement>) {
        if let Some(record) = self.state().pulls.get_mut(&pr) {
            record.blocked = Some(requires);
        }
    }

    /// Has the branch rules require a review of the pull request.
    pub fn require_review(&self, pr: Number) {
        if let Some(record) = self.state().pulls.get_mut(&pr) {
            record.review_ruled = true;
        }
    }

    pub fn allow_merge(&self, pr: Number) {
        if let Some(record) = self.state().pulls.get_mut(&pr) {
            record.blocked = None;
        }
    }

    pub fn pend_merge_state(&self, pr: Number) {
        if let Some(record) = self.state().pulls.get_mut(&pr) {
            record.merge_state_pending = true;
        }
    }

    pub fn work_out_merge_state(&self, pr: Number) {
        if let Some(record) = self.state().pulls.get_mut(&pr) {
            record.merge_state_pending = false;
        }
    }

    pub fn refuse_merges(&self, pr: Number, reason: &str) {
        if let Some(record) = self.state().pulls.get_mut(&pr) {
            record.refused = Some(reason.to_string());
        }
    }

    /// `output` is what `git push` reports.
    pub fn refuse_pushes(&self, output: &str) {
        self.state().pushes_refused = Some(output.to_string());
    }

    pub fn accept_pushes(&self) {
        self.state().pushes_refused = None;
    }

    /// Refuses every upload of an attachment from now on, as `refusal`
    /// says.
    pub fn refuse_attachments(&self, refusal: AttachRefusal) {
        self.state().attachments_refused = Some(refusal);
    }

    pub fn accept_attachments(&self) {
        self.state().attachments_refused = None;
    }

    /// Refuses to let `token` read the repository.
    pub fn refuse_reads_by(&self, token: &str) {
        self.state().unreadable_by.push(token.to_string());
    }

    /// Refuses to assign `login` to anything from now on.
    pub fn refuse_to_assign(&self, login: &str) {
        self.state().unassignable.push(Login::new(login));
    }

    pub fn after_linewrights_next_comment(&self, then: impl FnOnce() + Send + 'static) {
        self.state().after_comment = Some(Box::new(then));
    }

    /// Runs `then` once Linewright next adds `label` to `number`.
    pub fn after_labeling(
        &self,
        number: Number,
        label: &str,
        then: impl FnOnce() + Send + 'static,
    ) {
        self.state().after_labeling = Some(AfterLabeling {
            number,
            label: label.to_string(),
            then: Box::new(then),
        });
    }

    /// Pauses the store call that next makes `write`, once the write is
    /// applied, until the returned [`Paused`] is released. One pause is set
    /// at a time: a second replaces the first.
    pub fn pause_after(&self, write: Write) -> Paused {
        let (reached, on_reached) = oneshot::channel();
        let (release, released) = oneshot::channel();
        self.state().pause = Some(Pause {
            after: write,
            reached,
            released,
        });
        Paused {
            reached: on_reached,
            release,
        }
    }

    /// Waits for the pause set on `write`, if any, to be released, and fails
    /// if it is released failing.
    async fn paused(&self, write: Write) -> Result<()> {
        let pause = {
            let mut state = self.state();
            match state.pause.take() {
                Some(pause) if pause.after == write => Some(pause),
                other => {
                    state.pause = other;
                    None
                }
            }
        };
        let Some(pause) = pause else {
            return Ok(());
        };
        let _ = pause.reached.send(());
        // A dropped `Paused` releases the call as well.
        match pause.released.await {
            Ok(Released::Failed) => bail!("the store call failed after {:?}", pause.after),
            Ok(Released::Ok) | Err(_) => Ok(()),
        }
    }

    pub fn accept_merges(&self, pr: Number) {
        if let Some(record) = self.state().pulls.get_mut(&pr) {
            record.refused = None;
        }
    }

    pub async fn open_pull_as(
        &self,
        author: &str,
        branch: &str,
        title: &str,
        body: &str,
        files: &[(&str, &str)],
    ) -> Result<Number> {
        self.commit(author, branch, files, title).await?;
        let now = self.now();
        let mut state = self.state();
        let number = state.take_number();
        let record = PullRecord {
            state: PullState::Open { draft: false },
            ..PullRecord::new(&state, number, title, body, Login::new(author), branch, now)
        };
        state.pulls.insert(number, record);
        Ok(number)
    }

    pub fn close_pull_as(&self, pr: Number) -> Result<()> {
        let now = self.now();
        self.state().pull_written(pr, now)?.state = PullState::Closed { at: now };
        Ok(())
    }

    /// Each `inline` (path, body) opens a thread.
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
        let record = store.pull_written(pr, now)?;
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

    /// On the pull request's current head.
    pub async fn set_checks(&self, pr: Number, checks: Vec<CheckRecord>) -> Result<()> {
        let head_ref = self.state().pull(pr)?.head_ref.clone();
        let head = self.rev(&head_ref).await?;
        let now = self.now();
        self.state()
            .pull_written(pr, now)?
            .checks
            .insert(head, checks);
        Ok(())
    }

    pub fn create_label(&self, name: &str) {
        self.state().labels.insert(name.to_string(), String::new());
    }

    pub fn has_label(&self, name: &str) -> bool {
        self.state().labels.contains_key(name)
    }

    pub fn label_description(&self, name: &str) -> Option<String> {
        self.state().labels.get(name).cloned()
    }

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

    pub fn pull_assignees(&self, pr: Number) -> Vec<Login> {
        self.state()
            .pulls
            .get(&pr)
            .map(|p| p.assignees.clone())
            .unwrap_or_default()
    }

    pub fn requested_reviewers(&self, pr: Number) -> Vec<Login> {
        self.state()
            .pulls
            .get(&pr)
            .map(|p| p.requested_reviewers.clone())
            .unwrap_or_default()
    }

    pub async fn snapshot_pull(&self, pr: Number) -> Pull {
        self.pull_view(pr).await.expect("the pull request exists")
    }

    pub fn pulls(&self) -> Vec<Number> {
        self.state().pulls.keys().copied().collect()
    }

    pub fn snapshots_taken(&self) -> usize {
        self.state().snapshots_taken
    }

    pub fn items_read(&self) -> usize {
        self.state().items_read
    }

    pub fn fail_reads(&self, reason: &str) {
        self.state().unreadable = Some(reason.into());
    }

    pub fn restore_reads(&self) {
        self.state().unreadable = None;
    }

    fn readable(&self) -> Result<()> {
        match &self.state().unreadable {
            Some(reason) => bail!("{reason}"),
            None => Ok(()),
        }
    }

    fn read_item(&self) -> Result<()> {
        self.readable()?;
        self.state().items_read += 1;
        Ok(())
    }

    pub async fn file(&self, branch: &str, path: &str) -> Result<String> {
        Store::read_file(self, branch, path)
            .await?
            .with_context(|| format!("{branch} has no {path}"))
    }

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

    async fn rev(&self, rev: &str) -> Result<String> {
        Ok(git::git(&self.repository(), &["rev-parse", rev])
            .await?
            .trim()
            .to_string())
    }

    async fn changed_files(&self, before: &str, after: &str) -> Result<Vec<String>> {
        Ok(
            git::git(&self.repository(), &["diff", "--name-only", before, after])
                .await?
                .lines()
                .map(str::to_string)
                .collect(),
        )
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
                pull.updated_at = now;
            }
        }
    }

    /// A snapshot of the local store holds every issue.
    async fn ticket_branches(&self) -> Result<Vec<TicketBranch>> {
        let heads = git::git(
            &self.repository(),
            &[
                "for-each-ref",
                "--format=%(refname:lstrip=2)%09%(committername)",
                "refs/heads",
            ],
        )
        .await?;
        let state = self.state();
        Ok(heads
            .lines()
            .filter_map(|line| {
                let (name, committer) = line.split_once('\t')?;
                let branch = TicketBranchName::parse(name)?;
                let record = state.issues.get(&branch.issue())?;
                if committer != state.linewright.as_str() {
                    return None;
                }
                let open_pull_from_branch = state
                    .pulls
                    .values()
                    .any(|p| p.is_open() && p.head_ref == name);
                Some(TicketBranch {
                    name: branch,
                    issue: if record.is_open() {
                        BranchIssue::Held
                    } else {
                        BranchIssue::Read(Box::new(state.issue_view(record)))
                    },
                    open_pull_from_branch,
                })
            })
            .collect())
    }

    async fn pull_diff(&self, pr: Number) -> Result<String> {
        let (merge, head_ref, default_branch) = {
            let state = self.state();
            let default_branch = state.default_branch.clone();
            let record = state.pull(pr)?;
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
            let state = self.state();
            let default_branch = state.default_branch.clone();
            (state.pull(number)?.clone(), default_branch)
        };
        let head_sha = self.rev(&record.head_ref).await.unwrap_or_default();
        // Whether the head contains the default branch, which both its
        // mergeability and its rebaseability start from.
        let up_to_date = match record.state {
            PullState::Open { .. } => git::git(
                &self.repository(),
                &[
                    "merge-base",
                    "--is-ancestor",
                    &default_branch,
                    &record.head_ref,
                ],
            )
            .await
            .is_ok(),
            PullState::Merged { .. } | PullState::Closed { .. } => false,
        };
        let mergeability = match record.state {
            PullState::Open { .. } if record.merge_state_pending => Mergeability::Unknown,
            PullState::Open { .. } => {
                let found = self
                    .mergeability(&default_branch, &record.head_ref, up_to_date)
                    .await?;
                match &record.blocked {
                    Some(requires) if found == Mergeability::Clean => Mergeability::Blocked {
                        requires: requires.clone(),
                    },
                    Some(_) | None => found,
                }
            }
            PullState::Merged { .. } | PullState::Closed { .. } => Mergeability::Clean,
        };
        let rebaseability = match record.state {
            PullState::Open { .. } => {
                self.rebaseability(&default_branch, &record.head_ref, up_to_date)
                    .await?
            }
            PullState::Merged { .. } | PullState::Closed { .. } => Rebaseability::Unknown,
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
        let review_decision = record.review_decision();
        Ok(Pull {
            number,
            closes: closing_references(&record.body),
            title: record.title,
            body: record.body,
            author: record.author,
            created_at: record.created_at,
            updated_at: record.updated_at,
            base_ref: record.base_ref,
            head_ref: record.head_ref,
            head_sha,
            state: record.state,
            pushes: record.pushes,
            mergeability,
            rebaseability,
            checks,
            comments: record.comments,
            reviews: record.reviews,
            threads: record.threads,
            labels: record.labels,
            labelings: record.labelings,
            assignees: record.assignees,
            assignments: record.assignments,
            review_decision,
        })
    }

    /// `up_to_date` says whether `head` contains the default branch.
    async fn mergeability(
        &self,
        default_branch: &str,
        head: &str,
        up_to_date: bool,
    ) -> Result<Mergeability> {
        let repo = self.repository();
        if up_to_date {
            return Ok(Mergeability::Clean);
        }
        Ok(
            match git::merge_tree(&repo, None, default_branch, head).await? {
                Merged::Clean(_) => Mergeability::Behind,
                Merged::Conflicted(_) => Mergeability::Conflicting,
            },
        )
    }

    /// Whether the commits of `head` replay onto the default branch without
    /// a conflict, as a rebase merge replays them: a merge commit that
    /// resolved a conflict is dropped, and its resolution with it.
    /// `up_to_date` says whether `head` contains the default branch.
    async fn rebaseability(
        &self,
        default_branch: &str,
        head: &str,
        up_to_date: bool,
    ) -> Result<Rebaseability> {
        let repo = self.repository();
        let range = format!("{default_branch}..{head}");
        let merges = git::git(&repo, &["rev-list", "--merges", "--count", &range]).await?;
        if up_to_date && merges.trim() == "0" {
            return Ok(Rebaseability::Rebaseable);
        }
        Ok(
            match history::replay(&repo, default_branch, head, "rebaser").await? {
                Replayed::Onto(_) => Rebaseability::Rebaseable,
                Replayed::Conflict => Rebaseability::NotRebaseable,
            },
        )
    }
}

/// The URL of the local store's repository at `repo`.
fn url(repo: &Path) -> Result<&str> {
    repo.to_str().context("the local store's path is UTF-8")
}

impl State {
    fn take_number(&mut self) -> Number {
        let number = Number(self.next_number);
        self.next_number += 1;
        number
    }

    /// The issue #`number`, written to at `now`, which moves when it was
    /// last updated as GitHub's `updatedAt` moves on every write.
    fn issue_written(&mut self, number: Number, now: Timestamp) -> Result<&mut IssueRecord> {
        self.item_written(number, now)?
            .issue()
            .with_context(|| format!("no issue {number}"))
    }

    /// The pull request #`number`, written to at `now`, as
    /// [`State::issue_written`].
    fn pull_written(&mut self, number: Number, now: Timestamp) -> Result<&mut PullRecord> {
        self.item_written(number, now)?
            .pull()
            .with_context(|| format!("no pull request {number}"))
    }

    /// The issue or pull request #`number`, written to at `now`, as
    /// [`State::issue_written`].
    fn item_written(&mut self, number: Number, now: Timestamp) -> Result<Written<'_>> {
        let mut item = self.item_mut(number)?;
        item.write(now);
        Ok(item)
    }

    /// The issue or pull request #`number`, written to only once
    /// [`Written::write`] says so.
    fn item_mut(&mut self, number: Number) -> Result<Written<'_>> {
        if let Some(record) = self.issues.get_mut(&number) {
            return Ok(Written::Issue(record));
        }
        self.pulls
            .get_mut(&number)
            .map(Written::Pull)
            .with_context(|| format!("no issue or pull request {number}"))
    }

    fn pull(&self, number: Number) -> Result<&PullRecord> {
        self.pulls
            .get(&number)
            .with_context(|| format!("no pull request {number}"))
    }

    fn linked(&self, numbers: &[Number]) -> Vec<Linked> {
        numbers
            .iter()
            .map(|&number| Linked {
                number,
                open: self.issues.get(&number).is_some_and(IssueRecord::is_open),
            })
            .collect()
    }

    fn issue_view(&self, record: &IssueRecord) -> Issue {
        let issue = &record.issue;
        Issue {
            open: record.is_open(),
            blocked_by: self.linked(&record.blocked_by),
            sub_issues: record
                .sub_issues
                .iter()
                .map(|number| {
                    let sub_issue = &self.issues[number];
                    SubIssue {
                        number: *number,
                        author: sub_issue.issue.author.clone(),
                        state: match sub_issue.closing {
                            None => SubIssueState::Open,
                            Some(closing) => SubIssueState::Closed {
                                at: Some(closing.at),
                            },
                        },
                    }
                })
                .collect(),
            parent: self
                .issues
                .values()
                .find(|r| r.sub_issues.contains(&issue.number))
                .map(|r| Parent::new(r.issue.number, self.filed(&r.issue))),
            filed: self.filed(issue),
            closed_by: self
                .pulls
                .values()
                .filter(|p| p.closes(issue.number))
                .map(|p| ClosingPull {
                    number: p.number,
                    author: p.author.clone(),
                    state: p.state,
                    updated_at: p.updated_at,
                    base_ref: p.base_ref.clone(),
                })
                .collect(),
            ..issue.clone()
        }
    }

    /// The issue as the snapshot holds it among its closed items: `None`
    /// when it is open.
    fn closed_issue(&self, record: &IssueRecord) -> Option<ClosedItem> {
        Some(ClosedItem::of_issue(
            &self.issue_view(record),
            record.issue_closing()?,
        ))
    }

    fn tracking_issues_of(&self, spec_change: Number) -> Vec<Number> {
        self.issues
            .values()
            .map(|record| &record.issue)
            .filter(|issue| self.filed(issue) == Some(Filed::Tracking { spec_change }))
            .map(|issue| issue.number)
            .collect()
    }

    fn filed(&self, issue: &Issue) -> Option<Filed> {
        Filed::read(&issue.author, &issue.title, &issue.body, &self.linewright)
    }

    fn add_label(&mut self, number: Number, label: &str, by: &Login, now: Timestamp) -> Result<()> {
        match self.item_mut(number)? {
            Written::Issue(record) if !record.issue.has_label(label) => {
                let issue = &mut record.issue;
                issue.updated_at = now;
                issue.labels.push(label.into());
                issue.events.push(IssueEvent::Labeled {
                    label: label.into(),
                    by: Some(by.clone()),
                    at: now,
                });
            }
            Written::Pull(pull) if !pull.has_label(label) => {
                pull.updated_at = now;
                pull.labels.push(label.into());
                pull.labelings.push(Labeling {
                    label: label.into(),
                    by: Some(by.clone()),
                    at: now,
                });
            }
            Written::Issue(_) | Written::Pull(_) => {}
        }
        Ok(())
    }

    fn remove_label(&mut self, number: Number, label: &str, now: Timestamp) -> Result<()> {
        match self.item_mut(number)? {
            Written::Issue(record) if record.issue.has_label(label) => {
                let issue = &mut record.issue;
                issue.updated_at = now;
                issue.labels.retain(|l| l != label);
                issue.events.push(IssueEvent::Unlabeled {
                    label: label.into(),
                    at: now,
                });
            }
            Written::Pull(pull) if pull.has_label(label) => {
                pull.updated_at = now;
                pull.labels.retain(|l| l != label);
            }
            Written::Issue(_) | Written::Pull(_) => {}
        }
        Ok(())
    }

    fn assign(
        &mut self,
        number: Number,
        login: &Login,
        actor: &Login,
        now: Timestamp,
    ) -> Result<()> {
        let assigned = Assignment::Assigned {
            assignee: login.clone(),
            actor: actor.clone(),
            at: now,
        };
        self.change_assignees(number, assigned, now)
    }

    fn unassign(
        &mut self,
        number: Number,
        login: &Login,
        actor: &Login,
        now: Timestamp,
    ) -> Result<()> {
        let unassigned = Assignment::Unassigned {
            assignee: login.clone(),
            actor: actor.clone(),
            at: now,
        };
        self.change_assignees(number, unassigned, now)
    }

    /// Records `change` on the issue or pull request #`number` at `now`,
    /// unless it changes nothing.
    fn change_assignees(
        &mut self,
        number: Number,
        change: Assignment,
        now: Timestamp,
    ) -> Result<()> {
        let mut item = self.item_mut(number)?;
        let changed = match &mut item {
            Written::Issue(record) => changed(&mut record.issue.assignees, &change),
            Written::Pull(pull) => changed(&mut pull.assignees, &change),
        };
        if !changed {
            return Ok(());
        }
        item.write(now);
        match item {
            Written::Issue(record) => record.issue.events.push(IssueEvent::Assignment(change)),
            Written::Pull(pull) => pull.assignments.push(change),
        }
        Ok(())
    }
}

/// An issue's or a pull request's record, to write to.
enum Written<'a> {
    Issue(&'a mut IssueRecord),
    Pull(&'a mut PullRecord),
}

impl<'a> Written<'a> {
    /// Moves when it was last updated to `now`, as a write does.
    fn write(&mut self, now: Timestamp) {
        match self {
            Written::Issue(record) => record.issue.updated_at = now,
            Written::Pull(pull) => pull.updated_at = now,
        }
    }

    fn issue(self) -> Option<&'a mut IssueRecord> {
        match self {
            Written::Issue(record) => Some(record),
            Written::Pull(_) => None,
        }
    }

    fn pull(self) -> Option<&'a mut PullRecord> {
        match self {
            Written::Pull(pull) => Some(pull),
            Written::Issue(_) => None,
        }
    }
}

/// Applies `change` to `assignees`, returning whether it changed them.
fn changed(assignees: &mut Vec<Login>, change: &Assignment) -> bool {
    let assignee = change.assignee();
    let assigned = assignees.contains(assignee);
    match change {
        Assignment::Assigned { .. } if !assigned => assignees.push(assignee.clone()),
        Assignment::Unassigned { .. } if assigned => assignees.retain(|a| a != assignee),
        Assignment::Assigned { .. } | Assignment::Unassigned { .. } => return false,
    }
    true
}

impl IssueRecord {
    fn is_open(&self) -> bool {
        self.closing.is_none()
    }

    fn close(&mut self, at: Timestamp, merged_by: Option<Number>, reason: IssueCloseReason) {
        self.closing = Some(Closing {
            at,
            merged_by,
            reason,
        });
    }
}

impl PullRecord {
    /// The pull request as the snapshot holds it among its closed items:
    /// `None` unless `linewright` worked it and it closed without merging.
    fn closed_unmerged(&self, linewright: &Login) -> Option<ClosedItem> {
        let PullState::Closed { at } = self.state else {
            return None;
        };
        let events = self.events();
        self.signs(&events)
            .worked_by(linewright)
            .then(|| ClosedItem {
                number: self.number,
                title: self.title.clone(),
                closed_at: at,
                updated_at: self.updated_at,
                kind: ClosedKind::Pull {
                    closes: closing_references(&self.body),
                    author: Some(self.author.clone()),
                },
            })
    }

    /// What tells whether Linewright worked it, or it asks for its work,
    /// with `events` its [`PullRecord::events`].
    fn signs<'a>(&'a self, events: &'a [IssueEvent]) -> PullSigns<'a> {
        PullSigns {
            author: &self.author,
            assignees: &self.assignees,
            events,
        }
    }

    /// Among which pull requests a snapshot holds it: unlike GitHub's
    /// snapshot, this one holds Linewright's open or not; a person's it
    /// holds while open, as [`PullSigns::held`] tells.
    fn held(&self, linewright: &Login) -> Option<PullHeld> {
        let events = self.events();
        match self.signs(&events).held(linewright) {
            Some(PullHeld::Peoples) if !self.is_open() => None,
            held => held,
        }
    }

    fn is_open(&self) -> bool {
        matches!(self.state, PullState::Open { .. })
    }

    /// As GitHub decides it: none unless the rules require a review, then
    /// changes requested in a reviewer's latest review, else an approval,
    /// else a review required.
    fn review_decision(&self) -> Option<ReviewDecision> {
        if !self.review_ruled {
            return None;
        }
        let mut latest: BTreeMap<&Login, ReviewState> = BTreeMap::new();
        for review in &self.reviews {
            match review.state {
                ReviewState::Commented => {}
                ReviewState::Dismissed => {
                    latest.remove(&review.author);
                }
                state @ (ReviewState::Approved | ReviewState::ChangesRequested) => {
                    latest.insert(&review.author, state);
                }
            }
        }
        let any = |held: ReviewState| latest.values().any(|state| *state == held);
        Some(if any(ReviewState::ChangesRequested) {
            ReviewDecision::ChangesRequested
        } else if any(ReviewState::Approved) {
            ReviewDecision::Approved
        } else {
            ReviewDecision::ReviewRequired
        })
    }

    fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }

    fn closes(&self, number: Number) -> bool {
        closing_references(&self.body).contains(&number)
    }

    /// Its labelings, assignments and comments, oldest first.
    fn events(&self) -> Vec<IssueEvent> {
        pull_events(&self.labelings, &self.assignments, &self.comments)
    }

    /// The pull request as the snapshot holds it among the merged ones:
    /// `None` unless it merged into `default_branch`.
    fn merged(&self, default_branch: &str) -> Option<MergedPull> {
        let PullState::Merged { at } = self.state else {
            return None;
        };
        if self.base_ref != default_branch {
            return None;
        }
        Some(MergedPull {
            number: self.number,
            title: self.title.clone(),
            body: self.body.clone(),
            author: self.author.clone(),
            merged_at: at,
            updated_at: self.updated_at,
            base_ref: self.base_ref.clone(),
            labels: self.labels.clone(),
            closes: closing_references(&self.body),
            files: self.files.clone(),
            assignees: self.assignees.clone(),
            events: self.events(),
        })
    }

    /// A pull request into `state`'s default branch.
    fn new(
        state: &State,
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
            updated_at: now,
            base_ref: state.default_branch.clone(),
            head_ref: head_ref.into(),
            state: PullState::Open { draft: true },
            labels: vec![],
            labelings: vec![],
            assignees: vec![],
            assignments: vec![],
            pushes: vec![],
            comments: vec![],
            reviews: vec![],
            threads: vec![],
            requested_reviewers: vec![],
            review_ruled: false,
            blocked: None,
            merge_state_pending: false,
            refused: None,
            checks: BTreeMap::new(),
            merge: None,
            files: vec![],
        }
    }
}

#[async_trait]
impl Store for LocalStore {
    async fn identity(&self) -> Result<Identity> {
        let login = self.state().linewright.clone();
        Ok(Identity {
            name: login.0.clone(),
            email: format!("{login}@users.noreply.github.com"),
            login,
        })
    }

    async fn check_reads(&self, token: &str) -> Result<()> {
        if self.state().unreadable_by.iter().any(|t| t == token) {
            bail!("the token cannot read the repository: it needs read access to it");
        }
        Ok(())
    }

    async fn settings(&self) -> Result<RepoSettings> {
        let state = self.state();
        let categories = match &state.issue_types {
            Some(types) => Categories::IssueTypes(types.clone()),
            None => Categories::Labels,
        };
        Ok(RepoSettings::new(
            state.repo.clone(),
            state.default_branch.clone(),
            state.linear_history,
            state.allowed_merge_methods.clone(),
            categories,
        ))
    }

    async fn labels(&self) -> Result<Vec<label::RepoLabel>> {
        Ok(self
            .state()
            .labels
            .iter()
            .map(|(name, description)| label::RepoLabel {
                name: name.clone(),
                description: Some(description.clone()).filter(|d| !d.is_empty()),
            })
            .collect())
    }

    async fn create_label(&self, label: &label::Label) -> Result<()> {
        self.state()
            .labels
            .insert(label.name.to_string(), label.description.to_string());
        Ok(())
    }

    async fn describe_label(&self, name: &str, description: &str) -> Result<()> {
        let mut state = self.state();
        let label = state
            .labels
            .get_mut(name)
            .with_context(|| format!("no label {name}"))?;
        *label = description.to_string();
        Ok(())
    }

    async fn label_exists(&self, name: &str) -> Result<bool> {
        Ok(self.has_label(name))
    }

    async fn snapshot(&self, windows: Windows) -> Result<Snapshot> {
        self.readable()?;
        let (default_branch, issues, closed, linewright, pulls) = {
            let mut state = self.state();
            state.snapshots_taken += 1;
            let issues: Vec<Issue> = state
                .issues
                .values()
                .filter(|r| r.is_open())
                .map(|r| state.issue_view(r))
                .collect();
            let closed_issues = state.issues.values().filter_map(|r| state.closed_issue(r));
            let closed_pulls = state
                .pulls
                .values()
                .filter_map(|p| p.closed_unmerged(&state.linewright));
            let closed = closed_within(closed_issues.chain(closed_pulls), windows);
            // Each pull request, with among which a snapshot holds it.
            let pulls: Vec<(Number, Option<PullHeld>)> = state
                .pulls
                .values()
                .map(|p| (p.number, p.held(&state.linewright)))
                .collect();
            (
                state.default_branch.clone(),
                issues,
                closed,
                state.linewright.clone(),
                pulls,
            )
        };
        let mut snapshot = Snapshot {
            default_branch: default_branch.clone(),
            issues,
            pulls: vec![],
            peoples_pulls: vec![],
            merged: vec![],
            closed,
            ticket_branches: self.ticket_branches().await?,
        };
        for (number, held) in pulls {
            if let Some(merged) = self.merged_pull(number, windows).await? {
                snapshot.merged.push(merged);
            }
            match held {
                Some(PullHeld::Peoples) => {
                    snapshot.peoples_pulls.push(self.pull_view(number).await?);
                }
                Some(PullHeld::Linewrights) => {
                    snapshot.pulls.push(self.pull_view(number).await?);
                }
                None => {}
            }
        }
        let state = self.state();
        for number in snapshot.unheld_closed_tickets(&linewright) {
            if let Some(ticket) = state
                .issues
                .get(&number)
                .and_then(|r| state.closed_issue(r))
            {
                snapshot.put_closed(ticket);
            }
        }
        for number in snapshot.unheld_closing_pulls(&linewright) {
            if let Some(pull) = state
                .pulls
                .get(&number)
                .and_then(|p| p.closed_unmerged(&linewright))
            {
                snapshot.put_closed(pull);
            }
        }
        Ok(snapshot)
    }

    async fn issue(&self, number: Number) -> Result<IssueRead> {
        self.read_item()?;
        let (issue, pulls) = {
            let state = self.state();
            let Some(record) = state.issues.get(&number) else {
                return Ok(IssueRead::Gone);
            };
            if let Some(closed) = state.closed_issue(record) {
                return Ok(IssueRead::Closed(Box::new(closed)));
            }
            let issue = state.issue_view(record);
            let linewright = &state.linewright;
            let pulls: Vec<Number> = state
                .pulls
                .values()
                .filter(|p| &p.author == linewright && p.closes(number))
                .map(|p| p.number)
                .collect();
            (issue, pulls)
        };
        let mut views = vec![];
        for pr in pulls {
            views.push(self.pull_view(pr).await?);
        }
        Ok(IssueRead::Held {
            issue: Box::new(issue),
            pulls: views,
        })
    }

    async fn pull(&self, number: Number) -> Result<Pull> {
        self.pull_view(number).await
    }

    async fn any_item(&self, number: Number) -> Result<Option<AnyItem>> {
        self.read_item()?;
        {
            let state = self.state();
            if let Some(record) = state.issues.get(&number) {
                if let Some(closing) = record.issue_closing() {
                    return Ok(Some(AnyItem::ClosedIssue {
                        issue: Box::new(state.issue_view(record)),
                        closing,
                    }));
                }
                return Ok(Some(AnyItem::issue(
                    state.issue_view(record),
                    &state.linewright,
                )));
            }
            let Some(record) = state.pulls.get(&number) else {
                return Ok(None);
            };
            if let Some(merged) = record.merged(&state.default_branch) {
                return Ok(Some(AnyItem::Merged {
                    pull: Box::new(merged),
                    tracking_issues: state.tracking_issues_of(number),
                }));
            }
        }
        Ok(Some(AnyItem::Pull(Box::new(self.pull_view(number).await?))))
    }

    async fn linewright_pull(&self, number: Number) -> Result<Option<Pull>> {
        self.read_item()?;
        let by_linewright = {
            let state = self.state();
            let Some(record) = state.pulls.get(&number) else {
                return Ok(None);
            };
            record.author == state.linewright
        };
        if !by_linewright {
            return Ok(None);
        }
        Ok(Some(self.pull_view(number).await?))
    }

    async fn peoples_pull(&self, number: Number) -> Result<PeoplesPullRead> {
        self.read_item()?;
        let (closes, peoples) = {
            let state = self.state();
            let Some(record) = state.pulls.get(&number).filter(|p| p.is_open()) else {
                return Ok(PeoplesPullRead::default());
            };
            let peoples = record.held(&state.linewright) == Some(PullHeld::Peoples);
            (closing_references(&record.body), peoples)
        };
        let pull = if peoples {
            Some(self.pull_view(number).await?)
        } else {
            None
        };
        Ok(PeoplesPullRead { closes, pull })
    }

    async fn merged_pull(&self, number: Number, windows: Windows) -> Result<Option<MergedPull>> {
        self.read_item()?;
        let state = self.state();
        let Some(record) = state.pulls.get(&number) else {
            return Ok(None);
        };
        Ok(record
            .merged(&state.default_branch)
            .filter(|merged| windows.holds_merged(merged.merged_at, merged.updated_at)))
    }

    async fn diff(&self, pr: Number, paths: &[String]) -> Result<String> {
        let (merge, head_ref, default_branch) = {
            let state = self.state();
            let default_branch = state.default_branch.clone();
            let record = state.pull(pr)?;
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
        let head_ref = self.state().pull(pr)?.head_ref.clone();
        let head = self.rev(&head_ref).await?;
        let state = self.state();
        let records = state
            .pull(pr)?
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
                log: Some(CheckLog {
                    step_output: c.log,
                    full: None,
                }),
            })
            .collect())
    }

    async fn read_file(&self, branch: &str, path: &str) -> Result<Option<String>> {
        let object = format!("{branch}:{path}");
        let repository = self.repository();
        match git::object(&repository, &object).await? {
            None => Ok(None),
            Some((kind, contents)) if kind == "blob" => {
                Ok(Some(String::from_utf8_lossy(&contents).into_owned()))
            }
            Some(_) => git::git(&repository, &["show", &object]).await.map(Some),
        }
    }

    async fn read_files(&self, branch: &str, paths: &[String]) -> Result<BTreeMap<String, Blob>> {
        let mut read = BTreeMap::new();
        if paths.is_empty() {
            return Ok(read);
        }
        let repository = self.repository();
        let mut args = vec!["ls-tree", "-r", "-z", branch, "--"];
        args.extend(paths.iter().map(String::as_str));
        // Each entry is `<mode> <type> <object>\t<name>`
        // (<https://git-scm.com/docs/git-ls-tree#_output_format>).
        let listing = git::git(&repository, &args).await?;
        for line in listing.split('\0') {
            let Some((meta, path)) = line.split_once('\t') else {
                continue;
            };
            let mut meta = meta.split(' ');
            let (Some("blob"), Some(object)) = (meta.nth(1), meta.next()) else {
                continue;
            };
            if paths.iter().any(|asked| asked == path) {
                let bytes = git::git_bytes(&repository, &["cat-file", "blob", object]).await?;
                read.insert(path.to_string(), Blob::from_bytes(bytes));
            }
        }
        Ok(read)
    }

    async fn list_files(&self, branch: &str, dir: &str) -> Result<Vec<String>> {
        let repository = self.repository();
        // Each entry is `<mode> <type> <object>\t<name>`
        // (<https://git-scm.com/docs/git-ls-tree#_output_format>). A
        // directory the branch lacks lists nothing; only a branch that
        // doesn't exist fails, and is checked for once it has.
        let prefix = format!("{dir}/");
        let listing =
            match git::git(&repository, &["ls-tree", "-r", "-z", branch, "--", &prefix]).await {
                Ok(listing) => listing,
                Err(error) => {
                    if git::git(&repository, &["cat-file", "-e", branch])
                        .await
                        .is_err()
                    {
                        return Ok(vec![]);
                    }
                    return Err(error);
                }
            };
        Ok(listing
            .split('\0')
            .filter_map(|entry| {
                let (meta, path) = entry.split_once('\t')?;
                let path = path.strip_prefix(&prefix)?;
                (meta.split(' ').nth(1) == Some("blob")).then(|| path.to_string())
            })
            .collect())
    }

    async fn clone_repo(&self, dir: &Path, branch: &str, config: &[(&str, &str)]) -> Result<()> {
        git::clone(url(&self.repository())?, dir, branch, config, &[]).await
    }

    async fn push(&self, dir: &Path, branch: &str, mode: PushMode) -> Result<(), PushError> {
        let refused = self.state().pushes_refused.clone();
        if let Some(output) = refused {
            // A refused push pauses as well, before GitHub's refusal returns.
            self.paused(Write::Push {
                branch: branch.into(),
            })
            .await
            .map_err(PushError::Failed)?;
            return Err(PushError::from_git(anyhow::anyhow!(
                "git push failed: {output}"
            )));
        }
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
        git::git(dir, &args).await.map_err(PushError::from_git)?;
        let linewright = self.state().linewright.clone();
        self.record_push(branch, &linewright);
        self.paused(Write::Push {
            branch: branch.into(),
        })
        .await
        .map_err(PushError::Failed)
    }

    /// Keeps each attachment once, named by its content's hash, and gives
    /// its path from the exported items (`docs/specs/replay.md`,
    /// "Attachments").
    async fn attach(
        &self,
        file: &Path,
        name: &str,
        _content_type: &str,
    ) -> Result<String, AttachError> {
        let refused = self.state().attachments_refused.clone();
        if let Some(refusal) = refused {
            // A refused upload pauses as well, before GitHub's refusal
            // returns.
            self.paused(Write::Attach { name: name.into() }).await?;
            return Err(match refusal {
                AttachRefusal::File(reason) => AttachError::RefusedFile { reason },
                AttachRefusal::Token(reason) => AttachError::RefusedToken { reason },
            });
        }
        let bytes = tokio::fs::read(file)
            .await
            .with_context(|| format!("reading {}", file.display()))?;
        let mut kept = hex::encode(Sha256::digest(&bytes));
        if let Some(extension) = Path::new(name).extension() {
            kept = format!("{kept}.{}", extension.to_string_lossy());
        }
        let dir = self.root.join(ATTACHMENTS);
        let path = dir.join(&kept);
        if !tokio::fs::try_exists(&path)
            .await
            .context("reading the attachments")?
        {
            tokio::fs::create_dir_all(&dir)
                .await
                .context("keeping an attachment")?;
            tokio::fs::write(&path, bytes)
                .await
                .context("keeping an attachment")?;
        }
        self.paused(Write::Attach { name: name.into() }).await?;
        Ok(format!("../{ATTACHMENTS}/{kept}"))
    }

    async fn delete_branch(&self, branch: &str) -> Result<()> {
        history::delete_branch(&self.repository(), branch).await
    }

    async fn add_label(&self, number: Number, label: &str) -> Result<()> {
        let now = self.now();
        let then = {
            let mut state = self.state();
            let linewright = state.linewright.clone();
            state.add_label(number, label, &linewright, now)?;
            match state.after_labeling.take() {
                Some(hook) if hook.number == number && hook.label == label => Some(hook.then),
                other => {
                    state.after_labeling = other;
                    None
                }
            }
        };
        if let Some(then) = then {
            then();
        }
        self.paused(Write::AddLabel {
            number,
            label: label.into(),
        })
        .await?;
        Ok(())
    }

    async fn remove_label(&self, number: Number, label: &str) -> Result<()> {
        let now = self.now();
        self.state().remove_label(number, label, now)?;
        self.paused(Write::RemoveLabel {
            number,
            label: label.into(),
        })
        .await?;
        Ok(())
    }

    async fn assign(&self, number: Number) -> Result<()> {
        let now = self.now();
        let mut state = self.state();
        let linewright = state.linewright.clone();
        state.assign(number, &linewright, &linewright, now)
    }

    async fn unassign(&self, number: Number) -> Result<()> {
        let now = self.now();
        {
            let mut state = self.state();
            let linewright = state.linewright.clone();
            state.unassign(number, &linewright, &linewright, now)?;
        }
        self.paused(Write::Unassign { number }).await?;
        Ok(())
    }

    async fn assign_people(&self, issue: Number, people: &[Login]) -> Result<Vec<Login>> {
        let now = self.now();
        let mut refused = vec![];
        {
            let mut state = self.state();
            let linewright = state.linewright.clone();
            for person in people {
                if state.unassignable.contains(person) {
                    refused.push(person.clone());
                } else {
                    state.assign(issue, person, &linewright, now)?;
                }
            }
        }
        self.paused(Write::AssignPeople { number: issue }).await?;
        Ok(refused)
    }

    async fn unassign_people(&self, issue: Number, people: &[Login]) -> Result<()> {
        let now = self.now();
        let mut state = self.state();
        let linewright = state.linewright.clone();
        for person in people {
            state.unassign(issue, person, &linewright, now)?;
        }
        Ok(())
    }

    async fn comment(&self, number: Number, body: &str) -> Result<()> {
        let linewright = self.state().linewright.clone();
        self.comment_as(linewright.as_str(), number, body)?;
        let then = self.state().after_comment.take();
        if let Some(then) = then {
            then();
        }
        self.paused(Write::Comment { number }).await?;
        Ok(())
    }

    async fn create_issue(&self, issue: NewIssue) -> Result<Number> {
        let linewright = self.state().linewright.clone();
        let labels: Vec<&str> = issue.labels.iter().map(String::as_str).collect();
        Ok(self.open_issue(linewright.as_str(), &issue.title, &issue.body, &labels))
    }

    async fn edit_body(&self, number: Number, body: &str) -> Result<()> {
        let now = self.now();
        match self.state().item_written(number, now)? {
            Written::Issue(record) => record.issue.body = body.into(),
            Written::Pull(pull) => pull.body = body.into(),
        }
        Ok(())
    }

    async fn close_issue(&self, issue: Number, reason: IssueCloseReason) -> Result<()> {
        self.close_issue_as(issue, reason)
    }

    async fn set_issue_type(&self, issue: Number, issue_type: &str) -> Result<()> {
        let now = self.now();
        self.state().issue_written(issue, now)?.issue_type = Some(issue_type.into());
        Ok(())
    }

    async fn add_sub_issue(&self, parent: Number, child: Number) -> Result<()> {
        let now = self.now();
        let mut state = self.state();
        state.issue_written(child, now)?;
        state.issue_written(parent, now)?.sub_issues.push(child);
        Ok(())
    }

    async fn add_blocked_by(&self, issue: Number, blocker: Number) -> Result<()> {
        let now = self.now();
        let mut state = self.state();
        state.issue_written(blocker, now)?;
        state.issue_written(issue, now)?.blocked_by.push(blocker);
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
        let number = {
            let mut state = self.state();
            let number = state.take_number();
            let author = state.linewright.clone();
            let record = PullRecord::new(
                &state,
                number,
                &pull.title,
                &pull.body,
                author,
                &pull.head,
                now,
            );
            state.pulls.insert(number, record);
            number
        };
        self.paused(Write::OpenPull { head: pull.head }).await?;
        Ok(number)
    }

    async fn mark_ready(&self, pr: Number) -> Result<()> {
        let now = self.now();
        self.state().pull_written(pr, now)?.state = PullState::Open { draft: false };
        Ok(())
    }

    async fn close_pull(&self, pr: Number) -> Result<()> {
        self.close_pull_as(pr)
    }

    async fn request_review(&self, pr: Number, reviewers: &[Login]) -> Result<()> {
        let now = self.now();
        let mut state = self.state();
        let requested = &mut state.pull_written(pr, now)?.requested_reviewers;
        for reviewer in reviewers {
            if !requested.contains(reviewer) {
                requested.push(reviewer.clone());
            }
        }
        Ok(())
    }

    async fn merge(&self, pr: Number, head: &str, method: MergeMethod) -> Result<()> {
        let (state, head_ref, blocked, refused) = {
            let state = self.state();
            let record = state.pull(pr)?;
            (
                record.state,
                record.head_ref.clone(),
                record.blocked.is_some(),
                record.refused.clone(),
            )
        };
        if blocked {
            bail!("the branch rules block merging {pr}");
        }
        if let Some(reason) = refused {
            bail!("{reason}");
        }
        match state {
            PullState::Open { draft: false } => {}
            PullState::Open { draft: true } => bail!("{pr} is a draft"),
            PullState::Merged { .. } | PullState::Closed { .. } => bail!("{pr} is not open"),
        }
        if self.rev(&head_ref).await? != head {
            bail!("the head of {pr} is no longer {head}");
        }
        if !self.state().allowed_merge_methods.contains(method) {
            bail!("the repository doesn't allow the merge method `{method}`");
        }
        let linewright = self.state().linewright.clone();
        self.merge_and_close(pr, linewright.as_str(), method).await
    }

    async fn reply_in_thread(&self, pr: Number, thread: &str, body: &str) -> Result<()> {
        let now = self.now();
        let mut state = self.state();
        let linewright = state.linewright.clone();
        let thread = state
            .pull_written(pr, now)?
            .threads
            .iter_mut()
            .find(|t| t.id == thread)
            .with_context(|| format!("no thread {thread} on {pr}"))?;
        thread.comments.push(Comment {
            author: linewright,
            body: body.into(),
            created_at: now,
        });
        Ok(())
    }

    async fn resolve_thread(&self, pr: Number, thread: &str) -> Result<()> {
        let now = self.now();
        let mut state = self.state();
        let thread = state
            .pull_written(pr, now)?
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
    let numbers = |linked: Vec<Linked>| {
        linked
            .into_iter()
            .map(|l| format!("{}{}", l.number, if l.open { "" } else { " (closed)" }))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _ = writeln!(
        out,
        "# {} {}\n\n- State: {}\n- Author: @{}\n- Type: {}\n- Labels: {}\n- Assignees: {}\n- Blocked by: {}\n- Sub-issues: {}\n- Parent: {}\n\n{}\n",
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
        numbers(issue.blocked_by.clone()),
        numbers(issue.sub_issues.iter().map(SubIssue::linked).collect()),
        issue
            .parent
            .map_or("none".to_string(), |p| p.number.to_string()),
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
        "# {} {}\n\n- State: {state}\n- Author: @{}\n- Assignees: {}\n- Branch: `{}`\n- Closes: {}\n- Checks: {}\n\n{}\n",
        pull.number,
        pull.title,
        pull.author,
        pull.assignees
            .iter()
            .map(Login::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        pull.head_ref,
        pull.closes
            .iter()
            .map(ToString::to_string)
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
