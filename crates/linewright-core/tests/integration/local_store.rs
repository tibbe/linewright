use linewright_core::domain::{
    IssueCloseReason, Mergeability, Number, Pull, Requirement, ReviewState, Scope, Timestamp,
    Windows,
};
use linewright_core::store::Store;
use linewright_core::testing::*;

async fn snapshot_pull(world: &World, pr: Number) -> Pull {
    let snapshot = world
        .store
        .snapshot(Windows::EVER, &Scope::default())
        .await
        .unwrap();
    snapshot.pull(pr).expect("the snapshot holds it").clone()
}

/// Seeding goes through git fast-import, which no contract reaches: they
/// push through the trait.
#[test_log::test(tokio::test)]
async fn branches_seeded_with_names_that_differ_only_in_case_are_kept_apart() {
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

/// Seeding goes through git fast-import, which no contract reaches: they
/// push through the trait.
#[test_log::test(tokio::test)]
async fn a_seeded_commit_keeps_files_whose_paths_git_fast_import_quotes() {
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
    let snapshot = world
        .store
        .snapshot(Windows::EVER, &Scope::default())
        .await
        .unwrap();
    snapshot.updated_at(number).expect("the snapshot holds it")
}

/// The sandbox can't hold this: the trait comments only as Linewright and
/// can't reopen, and the sandbox, a personal repository, has no issue types
/// (<https://docs.github.com/en/issues/tracking-your-work-with-issues/configuring-issues/managing-issue-types-in-an-organization>).
#[test_log::test(tokio::test)]
async fn a_persons_comment_an_issue_type_and_a_reopening_move_when_an_issue_was_last_updated() {
    let world = World::new().await;
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &["needs-triage"]);
    let store = &world.store;

    let mut last = updated_at(&world, issue).await;
    let mut written = async |what: &str| {
        let now = updated_at(&world, issue).await;
        assert!(now > last, "{what} left it at {last}");
        last = now;
    };
    store.comment_as("alice", issue, "Another.").unwrap();
    written("a person's comment").await;
    store.set_issue_type(issue, "Bug").await.unwrap();
    written("an issue type").await;
    store
        .close_issue(issue, IssueCloseReason::NotPlanned)
        .await
        .unwrap();
    written("a close").await;
    store.reopen_issue_as(issue).unwrap();
    written("a reopening").await;
}

/// The sandbox can't hold this: the trait pushes and reviews only as
/// Linewright, and a second account to act as another person is a secret
/// the daily automation doesn't have.
#[test_log::test(tokio::test)]
async fn a_persons_push_and_review_move_when_a_pull_request_was_last_updated() {
    let world = World::new().await;
    let issue = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    let pr = world.persons_pull_closing(issue).await;
    let store = &world.store;

    let mut last = store.snapshot_pull(pr).await.updated_at;
    let mut written = async |what: &str| {
        let now = store.snapshot_pull(pr).await.updated_at;
        assert!(now > last, "{what} left it at {last}");
        last = now;
    };
    store
        .commit(
            "alice",
            &format!("alice-closes-{}", issue.0),
            &[("src/more.txt", "more\n")],
            "More",
        )
        .await
        .unwrap();
    written("a person's push").await;
    store
        .review_as(
            "bob",
            pr,
            ReviewState::ChangesRequested,
            "Fix it.",
            &[("src/thing.txt", "Here.")],
        )
        .unwrap();
    written("a person's review").await;
}

#[test_log::test(tokio::test)]
async fn uploads_of_the_same_bytes_keep_one_file() {
    let world = World::new().await;
    let session = tempfile::tempdir().unwrap();
    let shot = session.path().join("shot.png");
    let again = session.path().join("again.png");
    std::fs::write(&shot, "the bytes").unwrap();
    std::fs::write(&again, "the bytes").unwrap();

    let first = world
        .store
        .attach(&shot, "shot.png", "image/png")
        .await
        .unwrap();
    let second = world
        .store
        .attach(&again, "again.png", "image/png")
        .await
        .unwrap();

    let named_by_the_sha256_of_the_bytes =
        "attachments/ed589341c9fd5da96aaff4c5efa008226282a716186608d5a032ac75895a9c2f.png";
    assert_eq!(first, named_by_the_sha256_of_the_bytes);
    assert_eq!(second, named_by_the_sha256_of_the_bytes);
    let root = world.dir.path().join("store");
    let kept: Vec<_> = std::fs::read_dir(root.join("attachments"))
        .unwrap()
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(
        std::fs::read_to_string(root.join(&first)).unwrap(),
        "the bytes"
    );
}

/// A world, requiring branches up to date, whose pull request, ready for
/// review, the default branch has moved past without a conflict.
async fn moved_past() -> (World, Number) {
    let mut world = World::with_repository(Repository::requiring_up_to_date(), |_| {}).await;
    let (_, pr) = world.ticket_in_review().await;
    world.store.mark_ready(pr).await.unwrap();
    world.move_default_branch_on().await;
    (world, pr)
}

/// The sandbox can't hold this: requiring branches up to date takes a
/// required status check, which applies to the whole repository and would
/// keep every other contract from merging.
#[test_log::test(tokio::test)]
async fn a_pull_request_behind_is_reported_behind_where_the_repository_requires_it_up_to_date() {
    let (world, pr) = moved_past().await;

    assert_eq!(
        snapshot_pull(&world, pr).await.mergeability,
        Mergeability::Behind
    );
}

/// The sandbox can't hold this: requiring an approval applies to the whole
/// repository, and would keep every other contract from merging, as
/// Linewright can't approve its own pull requests.
#[test_log::test(tokio::test)]
async fn a_merge_blocked_by_a_missing_requirement_is_reported_blocked_rather_than_behind() {
    let (world, pr) = moved_past().await;

    world.store.block_merge(pr, vec![Requirement::Approval]);

    assert_eq!(
        snapshot_pull(&world, pr).await.mergeability,
        Mergeability::Blocked {
            requires: vec![Requirement::Approval]
        }
    );
}
