//! A trial's writes, kept as plain files in order (`docs/specs/benchmarks.md`,
//! The output of a trial).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use linewright_core::domain::{
    Assignment, IssueCloseReason, IssueEvent, Login, MergeMethod, PullState, ReviewState, Scope,
    Windows, label,
};
use linewright_core::git::CommitId;
use linewright_core::store::local::LocalStore;
use linewright_core::store::{AnyItem, NewIssue, NewPull, PushMode, Store};
use linewright_core::testing::*;
use linewright_core::trial::Journal;
use serde_json::{Value, json};

/// A journal of the writes to a store whose default branch, `main`, holds a
/// README.
struct Journaled {
    dir: tempfile::TempDir,
    store: Arc<LocalStore>,
    out: PathBuf,
    journal: Journal,
}

async fn journaled() -> Journaled {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("making a directory failed: {e}"));
    let store =
        Arc::new(store_holding(&dir.path().join("store"), &[("README.md", "Widgets\n")]).await);
    let out = dir.path().join("out");
    let journal = Journal::new(store.clone(), &out);
    Journaled {
        dir,
        store,
        out,
        journal,
    }
}

impl Journaled {
    /// The names of the files under `writes/`, in order: none when there is
    /// no such directory.
    fn writes(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.out.join("writes")) else {
            return vec![];
        };
        let mut names: Vec<String> = entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn markdown(&self, name: &str) -> String {
        let path = self.out.join("writes").join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    }

    fn entry(&self, name: &str) -> Value {
        serde_json::from_str(&self.markdown(name)).unwrap()
    }

    /// The contents of the file `reference`, relative to `writes/`, refers
    /// to.
    fn attached(&self, reference: &str) -> Vec<u8> {
        let relative = reference
            .strip_prefix("../")
            .unwrap_or_else(|| panic!("{reference} is not in the output"));
        std::fs::read(self.out.join(relative))
            .unwrap_or_else(|e| panic!("reading {reference}: {e}"))
    }

    /// A file at `name` holding `contents`, outside the output.
    fn file(&self, name: &str, contents: &[u8]) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// Clones `main` through the journal into `clone` and commits on top,
    /// giving the commit.
    async fn commit_in_a_clone(&self, clone: &Path) -> CommitId {
        self.journal
            .clone_repo(
                clone,
                "main",
                &[
                    ("user.name", "Factory"),
                    ("user.email", "factory@example.com"),
                ],
            )
            .await
            .unwrap_or_else(|e| panic!("cloning failed: {e:#}"));
        commit_in(clone, &[("gadgets.md", "Gadgets\n")], "Add gadgets");
        CommitId::from(git_in(clone, &["rev-parse", "HEAD"]).trim())
    }
}

fn assignments(events: &[IssueEvent]) -> Vec<(bool, &str)> {
    events
        .iter()
        .filter_map(|event| match event {
            IssueEvent::Assignment(Assignment::Assigned { assignee, .. }) => {
                Some((true, assignee.as_str()))
            }
            IssueEvent::Assignment(Assignment::Unassigned { assignee, .. }) => {
                Some((false, assignee.as_str()))
            }
            IssueEvent::Labeled { .. }
            | IssueEvent::Unlabeled { .. }
            | IssueEvent::Reopened { .. }
            | IssueEvent::Commented { .. } => None,
        })
        .collect()
}

#[test_log::test(tokio::test)]
async fn each_write_leaves_one_numbered_entry_in_call_order_and_reaches_the_store() {
    let journaled = journaled().await;
    let (store, journal) = (&journaled.store, &journaled.journal);
    let clone = journaled.dir.path().join("clone");
    let head = journaled.commit_in_a_clone(&clone).await;
    let maintainer = [Login::new("maintainer")];
    let old = store.open_linewrights_pull("old", "Old", "Old work.");

    journal.create_label(&label::ALL[0]).await.unwrap();
    journal
        .describe_label(label::ALL[0].name, "Not assessed")
        .await
        .unwrap();
    let issue = journal
        .create_issue(NewIssue {
            title: "Add gadgets".into(),
            body: "Gadgets, please.".into(),
            labels: vec!["bug".into()],
        })
        .await
        .unwrap();
    let ticket = journal
        .create_issue(NewIssue {
            title: "Gadget ticket".into(),
            body: "The first gadget.".into(),
            labels: vec![],
        })
        .await
        .unwrap();
    journal.add_label(issue, "enhancement").await.unwrap();
    journal.remove_label(issue, "bug").await.unwrap();
    journal.assign(issue).await.unwrap();
    journal.unassign(issue).await.unwrap();
    journal.assign_people(issue, &maintainer).await.unwrap();
    journal.unassign_people(issue, &maintainer).await.unwrap();
    journal.comment(issue, "Designed.").await.unwrap();
    journal
        .edit_body(issue, "Gadgets, designed.")
        .await
        .unwrap();
    journal.set_issue_type(issue, "Bug").await.unwrap();
    journal.add_sub_issue(issue, ticket).await.unwrap();
    let subs_added = store.issue(issue).unwrap().sub_issues.len();
    journal.remove_sub_issue(issue, ticket).await.unwrap();
    journal.add_blocked_by(issue, ticket).await.unwrap();
    journal
        .close_issue(ticket, IssueCloseReason::NotPlanned)
        .await
        .unwrap();
    journal
        .push(&clone, &head, "gadgets", PushMode::Replace)
        .await
        .unwrap();
    let pr = journal
        .open_pull(NewPull {
            head: "gadgets".into(),
            title: "Add gadgets".into(),
            body: "Adds gadgets.".into(),
        })
        .await
        .unwrap();
    journal.request_review(pr, &maintainer).await.unwrap();
    store
        .review_as(
            "maintainer",
            pr,
            ReviewState::Commented,
            "",
            &[("gadgets.md", "Why?")],
        )
        .unwrap();
    let thread = format!("T{}-1", pr.0);
    journal
        .reply_in_thread(pr, &thread, "Because.")
        .await
        .unwrap();
    journal.resolve_thread(pr, &thread).await.unwrap();
    journal.mark_ready(pr).await.unwrap();
    let before_merging = store.snapshot_pull(pr).await;
    journal.merge(pr, &head, MergeMethod::Squash).await.unwrap();
    journal.delete_branch("gadgets").await.unwrap();
    journal.close_pull(old).await.unwrap();

    assert_eq!(
        journaled.writes(),
        [
            "0001-create-label.json",
            "0002-describe-label.json",
            "0003-create-issue.json",
            "0003-create-issue.md",
            "0004-create-issue.json",
            "0004-create-issue.md",
            "0005-add-label.json",
            "0006-remove-label.json",
            "0007-assign.json",
            "0008-unassign.json",
            "0009-assign-people.json",
            "0010-unassign-people.json",
            "0011-comment.json",
            "0011-comment.md",
            "0012-edit-body.json",
            "0012-edit-body.md",
            "0013-set-issue-type.json",
            "0014-add-sub-issue.json",
            "0015-remove-sub-issue.json",
            "0016-add-blocked-by.json",
            "0017-close-issue.json",
            "0018-push.json",
            "0019-open-pull.json",
            "0019-open-pull.md",
            "0020-request-review.json",
            "0021-reply-in-thread.json",
            "0021-reply-in-thread.md",
            "0022-resolve-thread.json",
            "0023-mark-ready.json",
            "0024-merge.json",
            "0025-delete-branch.json",
            "0026-close-pull.json",
        ]
    );
    assert_eq!(
        store.label_description(label::ALL[0].name).as_deref(),
        Some("Not assessed")
    );
    let issue_now = store.issue(issue).unwrap();
    assert_eq!(issue_now.body, "Gadgets, designed.");
    assert_eq!(labels(&issue_now), ["enhancement"]);
    assert_eq!(
        assignments(&issue_now.events),
        [
            (true, LINEWRIGHT),
            (false, LINEWRIGHT),
            (true, "maintainer"),
            (false, "maintainer"),
        ]
    );
    assert_eq!(last_comment(&issue_now), "Designed.");
    assert_eq!(subs_added, 1);
    assert_eq!(issue_now.sub_issues, []);
    assert_eq!(
        issue_now
            .blocked_by
            .iter()
            .map(|b| b.number)
            .collect::<Vec<_>>(),
        [ticket]
    );
    assert!(!store.issue(ticket).unwrap().open);
    assert_eq!(store.pushed_to("gadgets"), [head.to_string()]);
    assert_eq!(before_merging.review_requests, maintainer);
    assert_eq!(before_merging.state, PullState::Open { draft: false });
    let replied = &before_merging.threads[0];
    assert!(replied.resolved);
    assert_eq!(replied.comments.last().unwrap().body, "Because.");
    let merged = Store::any_item(&**store, pr).await.unwrap();
    assert!(
        matches!(merged, Some(AnyItem::Merged { .. })),
        "{pr} is not merged: {merged:?}"
    );
    assert!(
        store
            .heads(&["gadgets".to_string()])
            .await
            .unwrap()
            .is_empty()
    );
    let closed = Store::any_item(&**store, old).await.unwrap();
    assert!(
        matches!(&closed, Some(AnyItem::Pull(pull)) if matches!(pull.state, PullState::Closed { .. })),
        "{old} is not closed: {closed:?}"
    );
}

#[test_log::test(tokio::test)]
async fn an_entry_holds_the_writes_fields_and_its_markdown_lies_beside_it() {
    let journaled = journaled().await;
    let journal = &journaled.journal;
    let clone = journaled.dir.path().join("clone");
    let head = journaled.commit_in_a_clone(&clone).await;

    let issue = journal
        .create_issue(NewIssue {
            title: "Add gadgets".into(),
            body: "Gadgets, **please**.".into(),
            labels: vec!["bug".into()],
        })
        .await
        .unwrap();
    journal.comment(issue, "Designed.\n").await.unwrap();
    journal
        .push(
            &clone,
            &head,
            "gadgets",
            PushMode::Lease {
                expected: String::new(),
            },
        )
        .await
        .unwrap();
    journal
        .assign_people(issue, &[Login::new("maintainer")])
        .await
        .unwrap();

    assert_eq!(
        journaled.entry("0001-create-issue.json"),
        json!({"title": "Add gadgets", "labels": ["bug"]})
    );
    assert_eq!(
        journaled.markdown("0001-create-issue.md"),
        "Gadgets, **please**."
    );
    assert_eq!(journaled.entry("0002-comment.json"), json!({"number": 1}));
    assert_eq!(journaled.markdown("0002-comment.md"), "Designed.\n");
    assert_eq!(
        journaled.entry("0003-push.json"),
        json!({"branch": "gadgets", "commit": head.to_string(), "lease": ""})
    );
    assert_eq!(
        journaled.entry("0004-assign-people.json"),
        json!({"issue": 1, "people": ["maintainer"]})
    );
}

#[test_log::test(tokio::test)]
async fn an_attachment_is_a_file_in_the_output_that_markdown_refers_to_by_a_relative_path() {
    let journaled = journaled().await;
    let journal = &journaled.journal;
    let shot = journaled.file("shot.png", b"\x89PNG one");
    let issue = journaled
        .store
        .open_issue("maintainer", "Gadgets", "Please.", &[]);

    let reference = journal
        .attach(&shot, "shot.png", "image/png")
        .await
        .unwrap();
    journal
        .comment(issue, &format!("![the page]({reference})"))
        .await
        .unwrap();

    assert_eq!(reference, "../attachments/shot.png");
    assert_eq!(
        journaled.markdown("0001-comment.md"),
        "![the page](../attachments/shot.png)"
    );
    assert_eq!(journaled.attached(&reference), b"\x89PNG one");
}

#[test_log::test(tokio::test)]
async fn attachments_of_one_name_with_other_contents_are_each_kept() {
    let journaled = journaled().await;
    let journal = &journaled.journal;
    let first = journaled.file("first.png", b"one");
    let second = journaled.file("second.png", b"two");

    let one = journal
        .attach(&first, "shot.png", "image/png")
        .await
        .unwrap();
    let again = journal
        .attach(&first, "shot.png", "image/png")
        .await
        .unwrap();
    let two = journal
        .attach(&second, "shot.png", "image/png")
        .await
        .unwrap();

    assert_eq!(one, "../attachments/shot.png");
    assert_eq!(again, "../attachments/shot.png");
    assert_eq!(two, "../attachments/shot-2.png");
    assert_eq!(journaled.attached(&one), b"one");
    assert_eq!(journaled.attached(&two), b"two");
    assert_eq!(journaled.writes(), Vec::<String>::new());
}

#[test_log::test(tokio::test)]
async fn an_attachment_named_with_spaces_is_referred_to_by_a_path_markdown_can_hold() {
    let journaled = journaled().await;
    let shot = journaled.file("shot.png", b"one");

    let reference = journaled
        .journal
        .attach(&shot, "the page (after).png", "image/png")
        .await
        .unwrap();

    assert_eq!(reference, "../attachments/the-page--after-.png");
    assert_eq!(journaled.attached(&reference), b"one");
}

#[test_log::test(tokio::test)]
async fn reads_are_answered_by_the_store_and_leave_no_entry() {
    let journaled = journaled().await;
    let (store, journal) = (&journaled.store, &journaled.journal);
    let issue = store.open_issue("maintainer", "Gadgets", "Please.", &["bug"]);
    let main = ["main".to_string()];

    let read = journal.issue(issue).await.unwrap();
    let any = journal.any_item(issue).await.unwrap();
    let snapshot = journal
        .snapshot(Windows::EVER, &Scope::default())
        .await
        .unwrap();
    let labels = journal.labels().await.unwrap();
    let settings = journal.settings().await.unwrap();
    let properties = journal.record_properties().await.unwrap();
    let heads = journal.heads(&main).await.unwrap();
    let file = journal.read_file("main", "README.md").await.unwrap();

    assert_eq!(read, Store::issue(&**store, issue).await.unwrap());
    assert_eq!(any, store.any_item(issue).await.unwrap());
    assert_eq!(
        snapshot,
        store
            .snapshot(Windows::EVER, &Scope::default())
            .await
            .unwrap()
    );
    assert_eq!(labels, store.labels().await.unwrap());
    assert_eq!(settings, store.settings().await.unwrap());
    assert_eq!(properties, store.record_properties().await.unwrap());
    assert_eq!(heads, store.heads(&main).await.unwrap());
    assert_eq!(file.as_deref(), Some("Widgets\n"));
    assert_eq!(journaled.writes(), Vec::<String>::new());
}
