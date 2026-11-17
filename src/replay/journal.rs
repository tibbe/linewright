use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::Mutex;

use super::{ATTACHMENTS, WRITES, attachment_link};
use crate::domain::{
    CheckFailure, IssueCloseReason, Login, MergeMethod, MergedPull, Number, Pull, Scope, Snapshot,
    TicketBranchName, Windows, label,
};
use crate::git::CommitId;
use crate::store::{
    AnyItem, AttachError, Blob, Identity, IssueRead, NewIssue, NewPull, PeoplesPullRead, PushError,
    PushMode, RecordedItem, RecordedProperties, RepoSettings, Store,
};

/// A [`Store`] that keeps each write made through it as plain files under
/// its output, in the order of the calls, and answers every read from the
/// store it wraps (`docs/specs/benchmarks.md`, The output of a replay).
///
/// Each write is kept as `writes/NNNN-<kind>.json`, numbered from `0001`,
/// holding its fields, with the Markdown it carries, if any, beside it as
/// `writes/NNNN-<kind>.md`. Both are written in full before the write
/// reaches the wrapped store, so the output holds every write begun before
/// the process ends. A write the output cannot keep fails, unmade.
///
/// Attachments are kept under `attachments/` instead of in the wrapped store.
pub struct Journal {
    inner: Arc<dyn Store>,
    out: PathBuf,
    /// The number of the last entry begun, so that no two share one.
    numbered: Mutex<u32>,
    /// Keeps two attachments from taking one name.
    attaching: Mutex<()>,
}

/// One write: its kind, as its file names give it, its fields, and the
/// Markdown it carries.
struct Entry<'a> {
    kind: &'static str,
    fields: Value,
    markdown: Option<&'a str>,
}

impl<'a> Entry<'a> {
    fn new(kind: &'static str, fields: Value) -> Entry<'a> {
        Entry {
            kind,
            fields,
            markdown: None,
        }
    }

    fn with_markdown(kind: &'static str, fields: Value, markdown: &'a str) -> Entry<'a> {
        Entry {
            kind,
            fields,
            markdown: Some(markdown),
        }
    }
}

impl Journal {
    /// Keeps the writes made through it under `out`, which it makes when it
    /// keeps the first, and passes every call on to `inner`.
    pub fn new(inner: Arc<dyn Store>, out: &Path) -> Journal {
        Journal {
            inner,
            out: out.to_path_buf(),
            numbered: Mutex::new(0),
            attaching: Mutex::new(()),
        }
    }

    /// Writes `entry` under `writes/`, its `.json` whole or not at all, and
    /// never over a file already there.
    async fn keep(&self, entry: Entry<'_>) -> Result<()> {
        let mut numbered = self.numbered.lock().await;
        *numbered += 1;
        let stem = format!("{:04}-{}", *numbered, entry.kind);
        let dir = self.out.join(WRITES);
        make_dir(&dir).await?;
        if let Some(markdown) = entry.markdown {
            write_new(&dir.join(format!("{stem}.md")), markdown.as_bytes()).await?;
        }
        let mut fields = serde_json::to_string_pretty(&entry.fields)?;
        fields.push('\n');
        let partial = dir.join(format!(".{stem}.json"));
        write_new(&partial, fields.as_bytes()).await?;
        let path = dir.join(format!("{stem}.json"));
        tokio::fs::hard_link(&partial, &path)
            .await
            .with_context(|| format!("writing {}", path.display()))?;
        tokio::fs::remove_file(&partial)
            .await
            .with_context(|| format!("removing {}", partial.display()))
    }

    /// Keeps `entry`, then makes the write `write`, which it does not make
    /// when it cannot keep the entry.
    async fn kept<T, E: From<anyhow::Error>>(
        &self,
        entry: Entry<'_>,
        write: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        self.keep(entry).await?;
        write.await
    }

    /// Copies `file` under `attachments/` as `name`, made safe as a file
    /// name and a relative URL, and gives the name it took: `name` again
    /// for the same contents, and a new one for others.
    async fn keep_attachment(&self, file: &Path, name: &str) -> Result<String> {
        let bytes = tokio::fs::read(file)
            .await
            .with_context(|| format!("reading {}", file.display()))?;
        let _attaching = self.attaching.lock().await;
        let dir = self.out.join(ATTACHMENTS);
        make_dir(&dir).await?;
        let name = safe_name(name);
        for candidate in candidates(&name) {
            let path = dir.join(&candidate);
            match tokio::fs::read(&path).await {
                Ok(existing) if existing == bytes => return Ok(candidate),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    write_new(&path, &bytes).await?;
                    return Ok(candidate);
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("reading {}", path.display()));
                }
            }
        }
        unreachable!("`candidates` never ends")
    }
}

async fn make_dir(dir: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("making {}", dir.display()))
}

/// Writes `contents` to a new file at `path`, failing when there is one.
async fn write_new(path: &Path, contents: &[u8]) -> Result<()> {
    let written = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await?;
        file.write_all(contents).await?;
        file.flush().await
    };
    written
        .await
        .with_context(|| format!("writing {}", path.display()))
}

/// Whether `c` can be in the name an attachment is kept under: an ASCII
/// letter, digit, `.`, `-` or `_`.
pub(super) fn in_safe_name(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')
}

/// `name` with each character other than an ASCII letter, digit, `.`, `-`
/// or `_` replaced by `-`, and a leading `.` too: `attachment` for an empty
/// `name`.
fn safe_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| if in_safe_name(c) { c } else { '-' })
        .collect();
    match safe.strip_prefix('.') {
        Some(rest) => format!("-{rest}"),
        None if safe.is_empty() => "attachment".to_string(),
        None => safe,
    }
}

/// `name`, then `name` with `-2`, `-3` and so on before its extension.
fn candidates(name: &str) -> impl Iterator<Item = String> + '_ {
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    std::iter::once(name.to_string()).chain((2u32..).map(move |n| format!("{stem}-{n}{extension}")))
}

fn logins(people: &[Login]) -> Vec<&str> {
    people.iter().map(Login::as_str).collect()
}

#[async_trait]
impl Store for Journal {
    async fn identity(&self) -> Result<Identity> {
        self.inner.identity().await
    }

    async fn check_reads(&self, token: &str) -> Result<()> {
        self.inner.check_reads(token).await
    }

    async fn settings(&self) -> Result<RepoSettings> {
        self.inner.settings().await
    }

    async fn record_properties(&self) -> Result<RecordedProperties> {
        self.inner.record_properties().await
    }

    async fn labels(&self) -> Result<Vec<label::RepoLabel>> {
        self.inner.labels().await
    }

    async fn create_label(&self, label: &label::Label) -> Result<()> {
        self.kept(
            Entry::new(
                "create-label",
                json!({"name": label.name, "description": label.description}),
            ),
            self.inner.create_label(label),
        )
        .await
    }

    async fn describe_label(&self, name: &str, description: &str) -> Result<()> {
        self.kept(
            Entry::new(
                "describe-label",
                json!({"name": name, "description": description}),
            ),
            self.inner.describe_label(name, description),
        )
        .await
    }

    async fn label_exists(&self, name: &str) -> Result<bool> {
        self.inner.label_exists(name).await
    }

    async fn snapshot(&self, windows: Windows, scope: &Scope) -> Result<Snapshot> {
        self.inner.snapshot(windows, scope).await
    }

    async fn ticket_branches_pushed(&self) -> Result<Vec<TicketBranchName>> {
        self.inner.ticket_branches_pushed().await
    }

    async fn issue(&self, number: Number) -> Result<IssueRead> {
        self.inner.issue(number).await
    }

    async fn pull(&self, number: Number) -> Result<Pull> {
        self.inner.pull(number).await
    }

    async fn any_item(&self, number: Number) -> Result<Option<AnyItem>> {
        self.inner.any_item(number).await
    }

    async fn linewright_pull(&self, number: Number) -> Result<Option<Pull>> {
        self.inner.linewright_pull(number).await
    }

    async fn peoples_pull(&self, number: Number) -> Result<PeoplesPullRead> {
        self.inner.peoples_pull(number).await
    }

    async fn merged_pull(&self, number: Number, windows: Windows) -> Result<Option<MergedPull>> {
        self.inner.merged_pull(number, windows).await
    }

    async fn diff(&self, pr: Number, paths: &[String]) -> Result<String> {
        self.inner.diff(pr, paths).await
    }

    async fn check_failures(&self, pr: Number, checks: &[String]) -> Result<Vec<CheckFailure>> {
        self.inner.check_failures(pr, checks).await
    }

    async fn read_file(&self, branch: &str, path: &str) -> Result<Option<String>> {
        self.inner.read_file(branch, path).await
    }

    async fn read_files(&self, branch: &str, paths: &[String]) -> Result<BTreeMap<String, Blob>> {
        self.inner.read_files(branch, paths).await
    }

    async fn list_files(&self, branch: &str, dir: &str) -> Result<Vec<String>> {
        self.inner.list_files(branch, dir).await
    }

    async fn clone_repo(&self, dir: &Path, branch: &str, config: &[(&str, &str)]) -> Result<()> {
        self.inner.clone_repo(dir, branch, config).await
    }

    async fn fetch(&self, dir: &Path) -> Result<()> {
        self.inner.fetch(dir).await
    }

    async fn push(
        &self,
        dir: &Path,
        commit: &CommitId,
        branch: &str,
        mode: PushMode,
    ) -> Result<(), PushError> {
        let lease = match &mode {
            PushMode::Replace => None,
            PushMode::Lease { expected } => Some(expected.as_str()),
        };
        self.kept(
            Entry::new(
                "push",
                json!({"branch": branch, "commit": commit.to_string(), "lease": lease}),
            ),
            self.inner.push(dir, commit, branch, mode),
        )
        .await
    }

    /// Keeps `file` under `attachments/` and gives its path from the
    /// writes, `../attachments/<name>`, without passing it on.
    async fn attach(
        &self,
        file: &Path,
        name: &str,
        _content_type: &str,
    ) -> Result<String, AttachError> {
        let name = self.keep_attachment(file, name).await?;
        Ok(attachment_link(&name))
    }

    async fn heads(&self, branches: &[String]) -> Result<BTreeMap<String, CommitId>> {
        self.inner.heads(branches).await
    }

    async fn delete_branch(&self, branch: &str) -> Result<()> {
        self.kept(
            Entry::new("delete-branch", json!({"branch": branch})),
            self.inner.delete_branch(branch),
        )
        .await
    }

    async fn add_label(&self, number: Number, label: &str) -> Result<()> {
        self.kept(
            Entry::new("add-label", json!({"number": number, "label": label})),
            self.inner.add_label(number, label),
        )
        .await
    }

    async fn remove_label(&self, number: Number, label: &str) -> Result<()> {
        self.kept(
            Entry::new("remove-label", json!({"number": number, "label": label})),
            self.inner.remove_label(number, label),
        )
        .await
    }

    async fn assign(&self, number: Number) -> Result<()> {
        self.kept(
            Entry::new("assign", json!({"number": number})),
            self.inner.assign(number),
        )
        .await
    }

    async fn unassign(&self, number: Number) -> Result<()> {
        self.kept(
            Entry::new("unassign", json!({"number": number})),
            self.inner.unassign(number),
        )
        .await
    }

    async fn assign_people(&self, issue: Number, people: &[Login]) -> Result<Vec<Login>> {
        self.kept(
            Entry::new(
                "assign-people",
                json!({"issue": issue, "people": logins(people)}),
            ),
            self.inner.assign_people(issue, people),
        )
        .await
    }

    async fn unassign_people(&self, issue: Number, people: &[Login]) -> Result<()> {
        self.kept(
            Entry::new(
                "unassign-people",
                json!({"issue": issue, "people": logins(people)}),
            ),
            self.inner.unassign_people(issue, people),
        )
        .await
    }

    async fn comment(&self, number: Number, body: &str) -> Result<()> {
        self.kept(
            Entry::with_markdown("comment", json!({"number": number}), body),
            self.inner.comment(number, body),
        )
        .await
    }

    async fn create_issue(&self, issue: NewIssue) -> Result<Number> {
        self.keep(Entry::with_markdown(
            "create-issue",
            json!({"title": issue.title, "labels": issue.labels}),
            &issue.body,
        ))
        .await?;
        self.inner.create_issue(issue).await
    }

    async fn edit_body(&self, number: Number, body: &str) -> Result<()> {
        self.kept(
            Entry::with_markdown("edit-body", json!({"number": number}), body),
            self.inner.edit_body(number, body),
        )
        .await
    }

    async fn close_issue(&self, issue: Number, reason: IssueCloseReason) -> Result<()> {
        self.kept(
            Entry::new("close-issue", json!({"issue": issue, "reason": reason})),
            self.inner.close_issue(issue, reason),
        )
        .await
    }

    async fn set_issue_type(&self, issue: Number, issue_type: &str) -> Result<()> {
        self.kept(
            Entry::new(
                "set-issue-type",
                json!({"issue": issue, "issue-type": issue_type}),
            ),
            self.inner.set_issue_type(issue, issue_type),
        )
        .await
    }

    async fn add_sub_issue(&self, parent: Number, child: Number) -> Result<()> {
        self.kept(
            Entry::new("add-sub-issue", json!({"parent": parent, "child": child})),
            self.inner.add_sub_issue(parent, child),
        )
        .await
    }

    async fn remove_sub_issue(&self, parent: Number, child: Number) -> Result<()> {
        self.kept(
            Entry::new(
                "remove-sub-issue",
                json!({"parent": parent, "child": child}),
            ),
            self.inner.remove_sub_issue(parent, child),
        )
        .await
    }

    async fn add_blocked_by(&self, issue: Number, blocker: Number) -> Result<()> {
        self.kept(
            Entry::new(
                "add-blocked-by",
                json!({"issue": issue, "blocker": blocker}),
            ),
            self.inner.add_blocked_by(issue, blocker),
        )
        .await
    }

    async fn open_pull(&self, pull: NewPull) -> Result<Number> {
        self.keep(Entry::with_markdown(
            "open-pull",
            json!({"head": pull.head, "title": pull.title}),
            &pull.body,
        ))
        .await?;
        self.inner.open_pull(pull).await
    }

    async fn mark_ready(&self, pr: Number) -> Result<()> {
        self.kept(
            Entry::new("mark-ready", json!({"pr": pr})),
            self.inner.mark_ready(pr),
        )
        .await
    }

    async fn close_pull(&self, pr: Number) -> Result<()> {
        self.kept(
            Entry::new("close-pull", json!({"pr": pr})),
            self.inner.close_pull(pr),
        )
        .await
    }

    async fn request_review(&self, pr: Number, reviewers: &[Login]) -> Result<()> {
        self.kept(
            Entry::new(
                "request-review",
                json!({"pr": pr, "reviewers": logins(reviewers)}),
            ),
            self.inner.request_review(pr, reviewers),
        )
        .await
    }

    async fn merge(&self, pr: Number, head: &str, method: MergeMethod) -> Result<()> {
        self.kept(
            Entry::new("merge", json!({"pr": pr, "head": head, "method": method})),
            self.inner.merge(pr, head, method),
        )
        .await
    }

    async fn reply_in_thread(&self, pr: Number, thread: &str, body: &str) -> Result<()> {
        self.kept(
            Entry::with_markdown("reply-in-thread", json!({"pr": pr, "thread": thread}), body),
            self.inner.reply_in_thread(pr, thread, body),
        )
        .await
    }

    async fn resolve_thread(&self, pr: Number, thread: &str) -> Result<()> {
        self.kept(
            Entry::new("resolve-thread", json!({"pr": pr, "thread": thread})),
            self.inner.resolve_thread(pr, thread),
        )
        .await
    }

    async fn record_items(&self, numbers: &BTreeSet<Number>) -> Result<Vec<RecordedItem>> {
        self.inner.record_items(numbers).await
    }

    async fn fetch_commits(&self, bare: &Path, heads: &BTreeMap<String, CommitId>) -> Result<()> {
        self.inner.fetch_commits(bare, heads).await
    }
}
