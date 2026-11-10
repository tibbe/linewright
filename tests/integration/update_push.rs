//! A run that brings its branch up to date and then changes it further
//! pushes the update on its own first and its other work right after:
//! `docs/specs/pull_requests.md`, "Keeping up to date", "Its own push".

use crate::common;
use crate::environment::set_up_with;
use crate::pipeline::resolve_in;
use crate::push_protection::{REFUSAL, commit_token, take_token_out};

use common::*;
use linewright::domain::{CheckState, MergeMethod, Number};
use linewright::git;
use linewright::reconcile::WorkKind;
use linewright::store::{PushMode, Store};
use serde_json::json;
use std::path::Path;

const SHARED: &str = "src/shared.txt";

/// A world whose default branch holds [`SHARED`], and the pull request of a
/// ticket in review, with each of `commits` writing [`SHARED`] pushed to its
/// branch.
async fn in_review_with(
    repository: Repository,
    adjust: impl FnOnce(&mut linewright::factory::FactoryConfig),
    commits: &[&str],
) -> (World, Number) {
    let mut world = World::with_repository(repository, adjust).await;
    world
        .store
        .merge_change("maintainer", "Share", "", &[(SHARED, "orig\n")])
        .await
        .unwrap();
    let (_, pr) = world.ticket_in_review().await;
    let branch = world.store.snapshot_pull(pr).await.head_ref;
    let work = tempfile::tempdir().unwrap();
    let clone = work.path().join("clone");
    let identity = [
        ("user.name", "maintainer"),
        ("user.email", "maintainer@example.com"),
    ];
    world
        .store
        .clone_repo(&clone, &branch, &identity)
        .await
        .unwrap();
    for contents in commits {
        commit_in(&clone, &[(SHARED, contents)], contents.trim());
    }
    let head = git::head(&clone).await.unwrap();
    world
        .store
        .push(&clone, &head, &branch, PushMode::Replace)
        .await
        .unwrap();
    (world, pr)
}

/// Moves the default branch on with [`SHARED`] holding `contents`.
async fn share(world: &World, contents: &str) {
    world
        .store
        .merge_change("maintainer", "Move on", "", &[(SHARED, contents)])
        .await
        .unwrap();
}

/// A pull request GitHub reports in conflict with the default branch, which
/// a rebase updates without one: the branch's first commit makes the same
/// change the default branch has since made, and the rebase drops it.
async fn conflicting_but_rebasing_cleanly() -> (World, Number) {
    let (world, pr) = in_review_with(Repository::default(), |_| {}, &["theirs\n", "ours\n"]).await;
    share(&world, "theirs\n").await;
    (world, pr)
}

/// A pull request that merges cleanly, as its branch's second commit
/// reverts its first, but whose update rebases, as the merge method merge
/// has it, replaying the first commit onto the default branch's change to
/// the same line.
async fn merging_cleanly_but_rebasing_in_conflict() -> (World, Number) {
    let (world, pr) = in_review_with(
        Repository::allowing([MergeMethod::Merge]),
        |_| {},
        &["mine\n", "orig\n"],
    )
    .await;
    share(&world, "theirs\n").await;
    (world, pr)
}

/// Has the maintainer ask for rework, which each rework does with a commit.
fn rework_asked_tidying(world: &World, pr: Number) {
    world
        .store
        .comment_as("maintainer", pr, "Tidy up.")
        .unwrap();
    world.harness.on(WorkKind::Rework, |call| {
        commit_in(&call.spec.dir, &[("src/tidy.txt", "tidy\n")], "Tidy up");
        Ok(json!({"outcome": "done", "comment": "Tidied."}).to_string())
    });
}

/// Has the maintainer ask for rework, whose session first commits a token
/// GitHub's push protection refuses after letting the update through, and
/// then takes it out and answers `followup`.
fn rework_asked_committing_a_token(world: &World, pr: Number, followup: serde_json::Value) {
    world
        .store
        .comment_as("maintainer", pr, "Configure it.")
        .unwrap();
    let store = world.store.clone();
    world.harness.on(WorkKind::Rework, move |call| {
        let dir = &call.spec.dir;
        if call.followup.is_none() {
            commit_token(dir);
            store.refuse_pushes_after(1, REFUSAL);
            Ok(json!({"outcome": "done", "comment": "Configured."}).to_string())
        } else {
            take_token_out(dir);
            store.accept_pushes();
            Ok(followup.to_string())
        }
    });
}

/// The commits pushed to `pr`'s branch after the first `count`.
async fn pushed_since(world: &World, pr: Number, count: usize) -> Vec<String> {
    let branch = world.store.snapshot_pull(pr).await.head_ref;
    world.store.pushed_to(&branch)[count..].to_vec()
}

async fn push_count(world: &World, pr: Number) -> usize {
    pushed_since(world, pr, 0).await.len()
}

/// The branch of `pr` as the store has it: the commit before its head, then
/// its head.
async fn update_then_head(world: &World, pr: Number) -> [String; 2] {
    let head = world.store.snapshot_pull(pr).await.head_sha;
    [store_git(world, &["rev-parse", &format!("{head}~1")]), head]
}

fn store_git(world: &World, args: &[&str]) -> String {
    git_in(&world.store.repository(), args).trim().to_string()
}

/// Whether the store's `commit` contains its default branch.
fn up_to_date(world: &World, commit: &str) -> bool {
    git_output(
        &world.store.repository(),
        &["merge-base", "--is-ancestor", "main", commit],
    )
    .status
    .success()
}

fn git(dir: &Path, args: &[&str]) -> String {
    git_in(dir, args).trim().to_string()
}

#[test_log::test(tokio::test)]
async fn a_rework_that_updates_without_a_conflict_then_commits_pushes_the_update_then_its_work() {
    let (mut world, pr) = conflicting_but_rebasing_cleanly().await;
    let before = push_count(&world, pr).await;
    rework_asked_tidying(&world, pr);

    world.poll().await;

    let pushes = pushed_since(&world, pr, before).await;
    assert_eq!(world.harness.stopped(WorkKind::Rework), []);
    assert_eq!(pushes, update_then_head(&world, pr).await);
    assert!(up_to_date(&world, &pushes[0]));
    assert_eq!(
        store_git(&world, &["show", &format!("{}:{SHARED}", pushes[0])]),
        "ours"
    );
    assert_eq!(last_pull_comment(&world, pr).await, "Tidied.");
}

#[test_log::test(tokio::test)]
async fn a_fix_ci_that_resolves_its_updates_conflict_then_rewrites_the_rebased_commits_pushes_the_updates_tip_first()
 {
    let (mut world, pr) = merging_cleanly_but_rebasing_in_conflict().await;
    let before = push_count(&world, pr).await;
    world
        .store
        .set_checks(pr, vec![check("test", CheckState::Failed, "boom")])
        .await
        .unwrap();
    let tip = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let resolved = tip.clone();
    world.harness.on(WorkKind::FixCi, move |call| {
        let dir = &call.spec.dir;
        resolve_in(dir, &[(SHARED, "mine\n")]);
        *resolved.lock().unwrap() = git(dir, &["rev-parse", "HEAD"]);
        std::fs::write(dir.join("src/fixed.txt"), "fixed\n").unwrap();
        git(dir, &["add", "--all"]);
        git(dir, &["commit", "--quiet", "--amend", "--no-edit"]);
        Ok(json!({"outcome": "done", "comment": "Fixed."}).to_string())
    });

    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    let tip = tip.lock().unwrap().clone();
    assert_eq!(world.harness.stopped(WorkKind::FixCi), [Stopped::Rebase]);
    assert_eq!(pushed_since(&world, pr, before).await, [tip, view.head_sha]);
}

#[test_log::test(tokio::test)]
async fn a_secret_in_the_second_push_sends_the_result_back_and_the_retry_pushes_only_the_runs_work()
{
    let (mut world, pr) = conflicting_but_rebasing_cleanly().await;
    let before = push_count(&world, pr).await;
    rework_asked_committing_a_token(
        &world,
        pr,
        json!({"outcome": "done", "comment": "Configured."}),
    );

    world.poll().await;

    let pushes = pushed_since(&world, pr, before).await;
    assert_eq!(world.harness.prompts(WorkKind::Rework).len(), 2);
    assert_eq!(pushes, update_then_head(&world, pr).await);
    assert!(up_to_date(&world, &pushes[0]));
    assert_eq!(last_pull_comment(&world, pr).await, "Configured.");
}

#[test_log::test(tokio::test)]
async fn a_run_that_stops_after_pushing_its_update_keeps_its_work_on_top_of_it() {
    let (mut world, pr) = conflicting_but_rebasing_cleanly().await;
    let before = push_count(&world, pr).await;
    rework_asked_committing_a_token(
        &world,
        pr,
        json!({"outcome": "gave-up", "reason": "Not sure."}),
    );

    world.poll().await;

    assert_eq!(
        pushed_since(&world, pr, before).await,
        update_then_head(&world, pr).await,
        "the update, then the work kept"
    );
}

#[test_log::test(tokio::test)]
async fn a_run_whose_environment_turns_reflogs_off_still_pushes_the_update_on_its_own() {
    let (mut world, pr) = conflicting_but_rebasing_cleanly().await;
    set_up_with(&world, "git config core.logAllRefUpdates false").await;
    world.factory.prepare_environment().await.unwrap();
    let before = push_count(&world, pr).await;
    rework_asked_tidying(&world, pr);

    world.poll().await;

    assert_eq!(
        pushed_since(&world, pr, before).await,
        update_then_head(&world, pr).await
    );
}
