use crate::common;

use common::*;
use linewright::clock::Clock;
use linewright::domain::{
    Assignment, CheckState, IssueCloseReason, Login, MergeMethod, Number, Pull, ReviewState,
    Timestamp,
};
use linewright::store::{Blob, Entry, Store, Tree};
use std::collections::BTreeMap;

async fn snapshot_pull(world: &World, pr: Number) -> Pull {
    let snapshot = world.store.snapshot(Timestamp::MIN_UTC).await.unwrap();
    snapshot.pull(pr).expect("the snapshot holds it").clone()
}

#[tokio::test]
async fn a_pull_request_lists_its_assignees_and_who_assigned_and_unassigned_whom_when() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    let opened = snapshot_pull(&world, pr).await.assignments.len();

    let start = world.clock.now();
    world
        .store
        .unassign_as("maintainer", pr, "factory")
        .unwrap();
    let unassigned = world.clock.now();
    world.store.assign_as("lead", pr, "maintainer").unwrap();
    let end = world.clock.now();

    let pull = snapshot_pull(&world, pr).await;
    assert_eq!(pull.assignees, [Login::new("maintainer")]);
    let changes = &pull.assignments[opened..];
    assert!(
        matches!(
            changes,
            [
                Assignment::Unassigned { assignee, actor, at: first },
                Assignment::Assigned { assignee: second_assignee, actor: second_actor, at: second },
            ] if assignee.as_str() == "factory"
                && actor.as_str() == "maintainer"
                && second_assignee.as_str() == "maintainer"
                && second_actor.as_str() == "lead"
                && start < *first && *first < unassigned
                && unassigned < *second && *second < end
        ),
        "{changes:?}"
    );
}

#[tokio::test]
async fn a_pull_request_read_again_lists_its_assignments_as_a_snapshot_does() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.store.unassign(pr).await.unwrap();
    world.store.assign(pr).await.unwrap();

    let read = world.store.linewright_pull(pr).await.unwrap();

    assert_eq!(read, Some(snapshot_pull(&world, pr).await));
    assert_eq!(read.unwrap().assignees, [Login::new("factory")]);
}

#[tokio::test]
async fn unassigning_someone_a_pull_request_is_not_assigned_to_records_nothing() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    let before = snapshot_pull(&world, pr).await.assignments;

    world.store.unassign_as("maintainer", pr, "alice").unwrap();

    assert_eq!(snapshot_pull(&world, pr).await.assignments, before);
}

#[tokio::test]
async fn branches_whose_names_differ_only_in_case_are_kept_apart() {
    let world = World::new().await;
    let branches = ["1-Gadgets", "1-gadgets"];

    for branch in branches {
        let contents = format!("{branch}\n");
        world
            .store
            .commit(
                LINEWRIGHT,
                branch,
                &[("src/gadgets.txt", &contents)],
                "Gadgets",
            )
            .await
            .unwrap();
    }

    for branch in branches {
        let contents = world.store.file(branch, "src/gadgets.txt").await.unwrap();
        assert_eq!(contents, format!("{branch}\n"));
    }
}

#[tokio::test]
async fn a_commit_keeps_files_whose_paths_need_quoting_in_git() {
    let world = World::new().await;
    let paths = [
        "\"quoted\".txt",
        "new\nline.txt",
        "back\\slash.txt",
        "with space.txt",
        "naïve.txt",
    ];
    let files: Vec<(&str, &str)> = paths.iter().map(|path| (*path, "kept\n")).collect();

    world
        .store
        .commit(LINEWRIGHT, "1-paths", &files, "Paths")
        .await
        .unwrap();

    for path in paths {
        assert_eq!(world.store.file("1-paths", path).await.unwrap(), "kept\n");
    }
}

/// When the snapshot says GitHub last updated the issue #`number`, open or
/// closed.
async fn updated_at(world: &World, number: Number) -> Timestamp {
    let snapshot = world.store.snapshot(Timestamp::MIN_UTC).await.unwrap();
    snapshot.updated_at(number).expect("the snapshot holds it")
}

#[tokio::test]
async fn every_write_to_an_issue_moves_when_it_was_last_updated() {
    let world = World::new().await;
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &["needs-triage"]);
    let blocker = world
        .store
        .open_issue("maintainer", "First", "Before.", &["needs-triage"]);
    let store = &world.store;

    let mut last = updated_at(&world, issue).await;
    let mut written = async |what: &str| {
        let now = updated_at(&world, issue).await;
        assert!(now > last, "{what} left it at {last}");
        last = now;
    };
    store.comment(issue, "A comment.").await.unwrap();
    written("a comment").await;
    store.comment_as("alice", issue, "Another.").unwrap();
    written("a person's comment").await;
    store.add_label(issue, "bug").await.unwrap();
    written("a label").await;
    store.remove_label(issue, "bug").await.unwrap();
    written("a label removed").await;
    store.assign(issue).await.unwrap();
    written("an assignment").await;
    store.unassign(issue).await.unwrap();
    written("an unassignment").await;
    store.edit_body(issue, "Edited.").await.unwrap();
    written("an edit").await;
    store.set_issue_type(issue, "Bug").await.unwrap();
    written("an issue type").await;
    store.add_blocked_by(issue, blocker).await.unwrap();
    written("a blocker").await;
    store
        .close_issue(issue, IssueCloseReason::NotPlanned)
        .await
        .unwrap();
    written("a close").await;
    store.reopen_issue_as(issue).unwrap();
    written("a reopening").await;
}

#[tokio::test]
async fn every_write_to_a_pull_request_moves_when_it_was_last_updated() {
    let world = World::new().await;
    let issue = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    let pr = world.persons_pull_closing(issue).await;
    let store = &world.store;

    let mut last = world.store.snapshot_pull(pr).await.updated_at;
    let mut written = async |what: &str| {
        let now = world.store.snapshot_pull(pr).await.updated_at;
        assert!(now > last, "{what} left it at {last}");
        last = now;
    };
    store.comment(pr, "A comment.").await.unwrap();
    written("a comment").await;
    store.add_label(pr, "bug").await.unwrap();
    written("a label").await;
    store.remove_label(pr, "bug").await.unwrap();
    written("a label removed").await;
    store.assign(pr).await.unwrap();
    written("an assignment").await;
    store
        .edit_body(pr, &format!("Closes {issue}, edited."))
        .await
        .unwrap();
    written("an edit").await;
    store
        .commit(
            "alice",
            &format!("alice-closes-{}", issue.0),
            &[("src/more.txt", "more\n")],
            "More",
        )
        .await
        .unwrap();
    written("a push").await;
    store
        .review_as(
            "bob",
            pr,
            ReviewState::ChangesRequested,
            "Fix it.",
            &[("src/thing.txt", "Here.")],
        )
        .unwrap();
    written("a review").await;
    let thread = format!("T{}-1", pr.0);
    store.reply_in_thread(pr, &thread, "Done.").await.unwrap();
    written("a reply").await;
    store.resolve_thread(pr, &thread).await.unwrap();
    written("a resolved thread").await;
    store
        .request_review(pr, &[Login::new("bob")])
        .await
        .unwrap();
    written("a review request").await;
    store
        .set_checks(pr, vec![check("test", CheckState::Passed, "")])
        .await
        .unwrap();
    written("a check").await;
    store.merge_pull(pr).await.unwrap();
    written("a merge").await;
    assert_eq!(
        updated_at(&world, issue).await,
        last,
        "the merge closed the issue"
    );
}

#[tokio::test]
async fn closing_a_pull_request_moves_when_it_was_last_updated() {
    let world = World::new().await;
    let issue = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    let pr = world.persons_pull_closing(issue).await;
    let before = world.store.snapshot_pull(pr).await.updated_at;

    world.store.close_pull_as(pr).unwrap();

    assert!(world.store.snapshot_pull(pr).await.updated_at > before);
}

#[tokio::test]
async fn a_pull_request_merged_by_rebase_puts_its_changes_on_the_default_branch() {
    let world = World::new().await;
    let (pr, head) = behind_pull(&world).await;

    world
        .store
        .merge(pr, &head, MergeMethod::Rebase)
        .await
        .unwrap();

    assert_eq!(
        world.store.file("main", "src/thing.txt").await.unwrap(),
        "thing\n"
    );
    assert_eq!(
        world.store.file("main", "src/other.txt").await.unwrap(),
        "other\n"
    );
}

/// The last commit on `rev`, in `git log`'s `format`.
fn store_log(world: &World, format: &str, rev: &str) -> String {
    let output = std::process::Command::new("git")
        .args(["log", "-1", &format!("--format={format}"), rev])
        .current_dir(world.store.repository())
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

/// A pull request opened on a default branch that has moved on since.
async fn behind_pull(world: &World) -> (Number, String) {
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    let pr = world.persons_pull_closing(issue).await;
    world
        .store
        .merge_change(
            "maintainer",
            "Do another thing",
            "Elsewhere.",
            &[("src/other.txt", "other\n")],
        )
        .await
        .unwrap();
    let head = world.store.snapshot_pull(pr).await.head_sha;
    (pr, head)
}

#[tokio::test]
async fn a_pull_request_merged_by_rebase_keeps_its_author_with_the_merger_as_committer() {
    let world = World::new().await;
    let (pr, head) = behind_pull(&world).await;
    let authored = store_log(&world, "%an <%ae> %ad", &head);

    world
        .store
        .merge(pr, &head, MergeMethod::Rebase)
        .await
        .unwrap();

    assert_eq!(
        store_log(&world, "%an <%ae> %ad", "main"),
        authored,
        "the author, as the pull request's commit has it"
    );
    assert_eq!(
        store_log(&world, "%cn %s%n%P", "main"),
        format!(
            "{LINEWRIGHT} Do the thing\n{}\n",
            store_log(&world, "%H", "main~1").trim()
        )
    );
}

#[tokio::test]
async fn a_pull_request_merged_by_rebase_adds_no_commit_for_changes_already_on_the_default_branch()
{
    let world = World::new().await;
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    let pr = world.persons_pull_closing(issue).await;
    world
        .store
        .merge_change(
            "maintainer",
            "Do the thing and another",
            "The same, and more.",
            &[("src/thing.txt", "thing\n"), ("src/other.txt", "other\n")],
        )
        .await
        .unwrap();
    let before = store_log(&world, "%H", "main");
    let head = world.store.snapshot_pull(pr).await.head_sha;

    world
        .store
        .merge(pr, &head, MergeMethod::Rebase)
        .await
        .unwrap();

    assert_eq!(store_log(&world, "%H", "main"), before);
}

#[tokio::test]
async fn a_pull_request_merged_by_rebase_skips_a_change_the_default_branch_has_and_built_on() {
    let world = World::new().await;
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    let pr = world.persons_pull_closing(issue).await;
    world
        .store
        .merge_change(
            "maintainer",
            "Do the thing too",
            "The same change.",
            &[("src/thing.txt", "thing\n")],
        )
        .await
        .unwrap();
    world
        .store
        .merge_change(
            "maintainer",
            "Do more of the thing",
            "Builds on it.",
            &[("src/thing.txt", "thing\nmore\n")],
        )
        .await
        .unwrap();
    let before = store_log(&world, "%H", "main");
    let head = world.store.snapshot_pull(pr).await.head_sha;

    world
        .store
        .merge(pr, &head, MergeMethod::Rebase)
        .await
        .unwrap();

    assert_eq!(store_log(&world, "%H", "main"), before);
}

#[tokio::test]
async fn a_pull_request_merged_with_a_merge_commit_has_both_tips_as_parents() {
    let world = World::new().await;
    let (pr, head) = behind_pull(&world).await;
    let before = store_log(&world, "%H", "main");
    let head_ref = world.store.snapshot_pull(pr).await.head_ref;

    world
        .store
        .merge(pr, &head, MergeMethod::Merge)
        .await
        .unwrap();

    assert_eq!(
        store_log(&world, "%an %cn %s%n%P", "main"),
        format!(
            "{LINEWRIGHT} {LINEWRIGHT} Merge pull request {pr} from {head_ref}\n{} {head}\n",
            before.trim()
        )
    );
    assert_eq!(
        world.store.file("main", "src/thing.txt").await.unwrap(),
        "thing\n"
    );
}

#[tokio::test]
async fn a_pull_request_squashed_is_one_commit_titled_after_it() {
    let world = World::new().await;
    let (pr, head) = behind_pull(&world).await;
    let before = store_log(&world, "%H", "main");

    world
        .store
        .merge(pr, &head, MergeMethod::Squash)
        .await
        .unwrap();

    assert_eq!(
        store_log(&world, "%an %cn %s%n%P", "main"),
        format!("{LINEWRIGHT} {LINEWRIGHT} Do the thing ({pr})\n{before}")
    );
    assert_eq!(
        world.store.file("main", "src/thing.txt").await.unwrap(),
        "thing\n"
    );
}

#[tokio::test]
async fn the_files_asked_for_and_a_directorys_are_read_from_the_head_of_the_branch_as_deep_as_asked()
 {
    let world = World::new().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[
                (".linewright/config.toml", "model = \"opus\"\n"),
                (".linewright/automations/hourly.md", "Every hour.\n"),
                (".linewright/prompts/triage.md", "Triage it.\n"),
                (".linewright/prompts/.editorconfig", "root = true\n"),
                (".linewright/prompts/parts/facts.md", "Facts.\n"),
                (".linewright/prompts/parts/x/y.md", "Too deep.\n"),
                (".linewright/prompts/parts/x/z/w.md", "Deeper.\n"),
            ],
            "Configure Linewright",
        )
        .await
        .unwrap();
    world
        .store
        .commit(
            "maintainer",
            "7-elsewhere",
            &[
                (".linewright/config.toml", "effort = \"low\"\n"),
                (".linewright/prompts/design.md", "Design it.\n"),
            ],
            "Configure it elsewhere",
        )
        .await
        .unwrap();
    let paths = [".linewright/config.toml", ".linewright/missing.toml"].map(String::from);

    let read = world
        .store
        .read_files("main", &paths, prompts_tree())
        .await
        .unwrap();

    assert_eq!(
        read.files,
        BTreeMap::from([(
            ".linewright/config.toml".to_string(),
            Blob::Text("model = \"opus\"\n".into())
        )])
    );
    let text = |text: &str| Entry::File(Blob::Text(text.into()));
    assert_eq!(
        read.entries,
        BTreeMap::from([
            (".editorconfig".to_string(), text("root = true\n")),
            ("parts/facts.md".to_string(), text("Facts.\n")),
            ("parts/x".to_string(), Entry::Unlisted),
            ("triage.md".to_string(), text("Triage it.\n")),
        ])
    );
}

fn prompts_tree() -> Tree<'static> {
    Tree {
        dir: ".linewright/prompts",
        depth: 2,
    }
}

#[tokio::test]
async fn a_file_that_is_not_utf_8_text_is_read_as_binary() {
    let world = World::new().await;
    world
        .store
        .commit_bytes(
            "maintainer",
            "main",
            &[(
                ".linewright/prompts/triage.md",
                b"Triage \xff it.\n".as_slice(),
            )],
            "Add a prompt in Latin-1",
        )
        .await
        .unwrap();

    let read = world
        .store
        .read_files("main", &[], prompts_tree())
        .await
        .unwrap();

    assert_eq!(
        read.entries,
        BTreeMap::from([("triage.md".to_string(), Entry::File(Blob::Binary))])
    );
}

#[tokio::test]
async fn every_file_under_a_directory_is_listed_by_its_path_under_it() {
    let world = World::new().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[
                (".linewright/config.toml", "model = \"opus\"\n"),
                (".linewright/prompts/triage.md", "Triage it.\n"),
                (".linewright/prompts/parts/facts.md", "Facts.\n"),
                ("README.md", "Read me.\n"),
            ],
            "Configure Linewright",
        )
        .await
        .unwrap();

    let mut files = world.store.list_files("main", ".linewright").await.unwrap();
    files.sort();

    assert_eq!(
        files,
        ["config.toml", "prompts/parts/facts.md", "prompts/triage.md"]
    );
    assert_eq!(
        world
            .store
            .list_files("main", ".linewright/automations")
            .await
            .unwrap(),
        Vec::<String>::new()
    );
}
