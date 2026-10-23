//! A finished session whose commits hold a secret: docs/specs/runs.md,
//! "A result the manager can't accept".

use crate::common;

use common::*;
use linewright::reconcile::WorkKind;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const SECRET: &str = "npm-s3cr3t";

async fn world_with_secret() -> World {
    World::with(|config| {
        config.secrets = secrets_of(&[("NPM_TOKEN", SECRET)]);
    })
    .await
}

/// Writes the secret to `.npmrc`, leaving it uncommitted.
fn write_secret(dir: &Path) {
    std::fs::write(
        dir.join(".npmrc"),
        format!("//registry.npmjs.org/:_authToken={SECRET}\n"),
    )
    .unwrap();
}

/// Commits the secret to `.npmrc`, returning the commit's abbreviated
/// name.
fn commit_secret(dir: &Path) -> String {
    write_secret(dir);
    git_in(dir, &["add", ".npmrc"]);
    git_in(dir, &["commit", "--quiet", "-m", "Publish to npm"]);
    git_out(dir, &["rev-parse", "--short", "HEAD"])
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn branches(store: &Path) -> Vec<String> {
    git_out(store, &["branch", "--format=%(refname:short)"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = vec![];
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found
}

/// Neither a prompt, a comment nor a kept record holds the secret.
async fn assert_secret_kept_out(world: &World) {
    for kind in [WorkKind::Implement, WorkKind::Rework] {
        for prompt in world.harness.prompts(kind) {
            assert!(!prompt.contains(SECRET), "{prompt}");
        }
    }
    for issue in world.store.issues() {
        for comment in &issue.comments {
            assert!(!comment.body.contains(SECRET), "{}", comment.body);
        }
    }
    for pr in world.store.pulls() {
        for comment in world.store.snapshot_pull(pr).await.comments {
            assert!(!comment.body.contains(SECRET), "{}", comment.body);
        }
    }
    for path in files_under(&world.dir.path().join("work/sessions")) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(!text.contains(SECRET), "{}: {text}", path.display());
    }
}

fn reason(commit: &str) -> String {
    format!("the commit `{commit}` adds the value of `NPM_TOKEN`, a credential, to `.npmrc`")
}

#[tokio::test]
async fn an_implement_run_whose_commits_hold_a_secret_is_sent_back_before_it_pushes_and_opens_its_pull_request_once_they_do_not()
 {
    let mut world = world_with_secret().await;
    let store = world.store.repository();
    let committed = Arc::new(Mutex::new(String::new()));
    let pushed_before_followup = Arc::new(Mutex::new(None));
    world.harness.on(WorkKind::Implement, {
        let committed = committed.clone();
        let pushed_before_followup = pushed_before_followup.clone();
        move |call| {
            let dir = &call.spec.dir;
            if call.followup.is_none() {
                *committed.lock().unwrap() = commit_secret(dir);
            } else {
                *pushed_before_followup.lock().unwrap() = Some(branches(&store));
                git_in(dir, &["reset", "--quiet", "--hard", "origin/main"]);
                commit_in(
                    dir,
                    &[(".npmrc", "//registry.npmjs.org/:_authToken=${NPM_TOKEN}\n")],
                    "Publish to npm",
                );
            }
            Ok(
                json!({"outcome": "done", "pr_title": "Publish", "pr_body": "Publishes."})
                    .to_string(),
            )
        }
    });
    world.store.open_issue(
        "maintainer",
        "Publish",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 2);
    let commit = committed.lock().unwrap().clone();
    assert!(prompts[1].contains(&reason(&commit)), "{}", prompts[1]);
    assert_eq!(
        pushed_before_followup.lock().unwrap().clone(),
        Some(vec!["main".to_string()]),
        "nothing is pushed before the session answers again"
    );
    let pr = world.store.snapshot_pull(world.store.pulls()[0]).await;
    assert_eq!(
        world.store.file(&pr.head_ref, ".npmrc").await.unwrap(),
        "//registry.npmjs.org/:_authToken=${NPM_TOKEN}\n"
    );
    assert_secret_kept_out(&world).await;
}

#[tokio::test]
async fn an_implement_run_whose_commits_still_hold_a_secret_after_its_follow_ups_hands_its_ticket_back_and_pushes_nothing()
 {
    let (log, _guard) = Log::capture();
    let mut world = world_with_secret().await;
    let committed = Arc::new(Mutex::new(String::new()));
    world.harness.on(WorkKind::Implement, {
        let committed = committed.clone();
        move |call| {
            if call.followup.is_none() {
                *committed.lock().unwrap() = commit_secret(&call.spec.dir);
            }
            Ok(
                json!({"outcome": "done", "pr_title": "Publish", "pr_body": "Publishes."})
                    .to_string(),
            )
        }
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Publish",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 2);
    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["linewright:stalled", "ready-for-human"]);
    let comment = last_comment(&issue);
    let commit = committed.lock().unwrap().clone();
    assert!(
        comment.contains("still could not be used after 1 follow-up(s)")
            && comment.contains(&reason(&commit)),
        "{comment}"
    );
    assert_eq!(world.store.pulls(), []);
    assert_eq!(branches(&world.store.repository()), ["main"]);
    assert_secret_kept_out(&world).await;
    let logged = log.text();
    assert!(
        logged.contains("NPM_TOKEN") && !logged.contains(SECRET),
        "{logged}"
    );
}

#[tokio::test]
async fn a_rework_whose_commits_hold_a_secret_pushes_nothing_to_its_pull_request_and_is_sent_back_then_handed_back()
 {
    let mut world = world_with_secret().await;
    let (_, pr) = world.ticket_in_review().await;
    let head = world.store.snapshot_pull(pr).await.head_sha;
    world
        .store
        .comment_as("maintainer", pr, "Publish it too.")
        .unwrap();
    let committed = Arc::new(Mutex::new(String::new()));
    world.harness.on(WorkKind::Rework, {
        let committed = committed.clone();
        move |call| {
            if call.followup.is_none() {
                *committed.lock().unwrap() = commit_secret(&call.spec.dir);
            }
            Ok(json!({"outcome": "done", "comment": "Published."}).to_string())
        }
    });

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Rework);
    assert_eq!(prompts.len(), 2);
    let commit = committed.lock().unwrap().clone();
    assert!(prompts[1].contains(&reason(&commit)), "{}", prompts[1]);
    assert_eq!(world.store.snapshot_pull(pr).await.head_sha, head);
    let comment = last_pull_comment(&world, pr).await;
    assert!(
        comment.contains("still could not be used after 1 follow-up(s)")
            && comment.contains(&reason(&commit)),
        "{comment}"
    );
    assert_secret_kept_out(&world).await;
}

#[tokio::test]
async fn a_rework_that_takes_a_secret_out_of_its_commits_when_sent_back_pushes_them() {
    let mut world = world_with_secret().await;
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .comment_as("maintainer", pr, "Publish it too.")
        .unwrap();
    world.harness.on(WorkKind::Rework, |call| {
        let dir = &call.spec.dir;
        if call.followup.is_none() {
            commit_secret(dir);
        } else {
            git_in(dir, &["reset", "--quiet", "--hard", "HEAD~1"]);
            commit_in(
                dir,
                &[(".npmrc", "//registry.npmjs.org/:_authToken=${NPM_TOKEN}\n")],
                "Publish to npm",
            );
        }
        Ok(json!({"outcome": "done", "comment": "Published."}).to_string())
    });

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Rework).len(), 2);
    let head_ref = world.store.snapshot_pull(pr).await.head_ref;
    assert_eq!(
        world.store.file(&head_ref, ".npmrc").await.unwrap(),
        "//registry.npmjs.org/:_authToken=${NPM_TOKEN}\n"
    );
    assert_eq!(last_pull_comment(&world, pr).await, "Published.");
    assert_secret_kept_out(&world).await;
}

#[tokio::test]
async fn a_rework_is_not_sent_back_for_a_secret_its_pull_request_already_held() {
    let mut world = world_with_secret().await;
    let (_, pr) = world.ticket_in_review().await;
    let head_ref = world.store.snapshot_pull(pr).await.head_ref;
    world
        .store
        .commit(
            "alice",
            &head_ref,
            &[(".npmrc", &format!("_authToken={SECRET}\n"))],
            "Publish",
        )
        .await
        .unwrap();
    world.rework_asked(pr);

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Rework).len(), 1);
    assert_eq!(
        world
            .store
            .file(&head_ref, "src/renamed.txt")
            .await
            .unwrap(),
        "renamed\n"
    );
}

#[tokio::test]
async fn a_secret_whose_value_spans_lines_is_found_whole() {
    let key = "-----BEGIN KEY-----\nc2VjcmV0\n-----END KEY-----";
    let mut world = World::with(|config| {
        config.secrets = secrets_of(&[("DEPLOY_KEY", key)]);
    })
    .await;
    world.harness.on(WorkKind::Implement, move |call| {
        commit_in(
            &call.spec.dir,
            &[("deploy/key.pem", &format!("{key}\n"))],
            "Add the deploy key",
        );
        Ok(json!({"outcome": "done", "pr_title": "Deploy", "pr_body": "Deploys."}).to_string())
    });
    world.store.open_issue(
        "maintainer",
        "Deploy",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[1].contains("adds the value of `DEPLOY_KEY`, a credential, to `deploy/key.pem`"),
        "{}",
        prompts[1]
    );
    assert_eq!(world.store.pulls(), []);
}

/// Asserts that `comment` says the work so far was not kept for the
/// secret in `.npmrc`, naming the commit that holds it, which is
/// `commit` when given.
fn assert_says_not_kept(comment: &str, commit: Option<&str>) {
    let (_, rest) = comment
        .split_once("\n\nThe work so far was not kept: the commit `")
        .unwrap_or_else(|| panic!("{comment}"));
    let (named, rest) = rest.split_once('`').unwrap();
    assert!(
        named.len() >= 7 && named.chars().all(|c| c.is_ascii_hexdigit()),
        "{comment}"
    );
    if let Some(commit) = commit {
        assert_eq!(named, commit);
    }
    assert!(
        rest.starts_with(" adds the value of `NPM_TOKEN`, a credential, to `.npmrc`."),
        "{comment}"
    );
}

#[tokio::test]
async fn an_implement_run_that_stops_with_a_secret_in_its_commits_keeps_nothing_and_says_why() {
    let mut world = world_with_secret().await;
    let committed = Arc::new(Mutex::new(String::new()));
    world.harness.on(WorkKind::Implement, {
        let committed = committed.clone();
        move |call| {
            *committed.lock().unwrap() = commit_secret(&call.spec.dir);
            Err(linewright::harness::SessionError::TimedOut {
                log_tail: String::new(),
            })
        }
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Publish",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["linewright:stalled", "ready-for-human"]);
    let commit = committed.lock().unwrap().clone();
    assert_says_not_kept(last_comment(&issue), Some(&commit));
    assert_eq!(branches(&world.store.repository()), ["main"]);
    assert_secret_kept_out(&world).await;
}

#[tokio::test]
async fn an_implement_run_that_blocks_with_a_secret_left_uncommitted_keeps_nothing_and_says_why() {
    let mut world = world_with_secret().await;
    world.harness.on(WorkKind::Implement, |call| {
        let dir = &call.spec.dir;
        commit_in(dir, &[("README.md", "Published to npm.\n")], "Start");
        write_secret(dir);
        Ok(json!({"outcome": "blocked", "question": "Which registry?"}).to_string())
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Publish",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let issue = world.issue(ticket);
    let comment = last_comment(&issue);
    assert!(comment.contains("Which registry?"), "{comment}");
    assert_says_not_kept(comment, None);
    assert_eq!(branches(&world.store.repository()), ["main"]);
    assert_secret_kept_out(&world).await;
}

#[tokio::test]
async fn a_rework_that_gives_up_with_a_secret_in_its_work_pushes_nothing_to_its_pull_request_and_says_why()
 {
    let mut world = world_with_secret().await;
    let (_, pr) = world.ticket_in_review().await;
    let head = world.store.snapshot_pull(pr).await.head_sha;
    world
        .store
        .comment_as("maintainer", pr, "Publish it too.")
        .unwrap();
    world.harness.on(WorkKind::Rework, |call| {
        write_secret(&call.spec.dir);
        Ok(json!({"outcome": "gave-up", "reason": "No registry."}).to_string())
    });

    world.poll().await;

    assert_eq!(world.store.snapshot_pull(pr).await.head_sha, head);
    let comment = last_pull_comment(&world, pr).await;
    assert!(comment.contains("No registry."), "{comment}");
    assert_says_not_kept(&comment, None);
    assert_secret_kept_out(&world).await;
}

#[tokio::test]
async fn a_run_that_stops_with_work_that_holds_no_secret_still_keeps_it() {
    let mut world = world_with_secret().await;
    world.harness.on(WorkKind::Implement, |call| {
        std::fs::write(
            call.spec.dir.join(".npmrc"),
            "//registry.npmjs.org/:_authToken=${NPM_TOKEN}\n",
        )
        .unwrap();
        Err(linewright::harness::SessionError::TimedOut {
            log_tail: String::new(),
        })
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Publish",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let branch = format!("{}-publish", ticket.0);
    assert_eq!(
        world.store.file(&branch, ".npmrc").await.unwrap(),
        "//registry.npmjs.org/:_authToken=${NPM_TOKEN}\n"
    );
    let comment = last_comment(&world.issue(ticket)).to_string();
    assert!(
        comment.ends_with(&format!("The work so far is on the branch `{branch}`.")),
        "{comment}"
    );
}

#[tokio::test]
async fn a_rework_that_stops_keeps_its_work_though_the_default_branch_it_took_in_holds_a_secret() {
    let mut world = world_with_secret().await;
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .commit(
            "alice",
            "main",
            &[(".npmrc", &format!("_authToken={SECRET}\n"))],
            "Publish",
        )
        .await
        .unwrap();
    world
        .store
        .comment_as("maintainer", pr, "Publish it too.")
        .unwrap();
    world.harness.on(WorkKind::Rework, |call| {
        std::fs::write(call.spec.dir.join("notes.txt"), "half done\n").unwrap();
        Ok(json!({"outcome": "gave-up", "reason": "No registry."}).to_string())
    });

    world.poll().await;

    let head_ref = world.store.snapshot_pull(pr).await.head_ref;
    assert_eq!(
        world.store.file(&head_ref, "notes.txt").await.unwrap(),
        "half done\n"
    );
    let comment = last_pull_comment(&world, pr).await;
    assert!(
        comment.contains(&format!("The work so far is on the branch `{head_ref}`.")),
        "{comment}"
    );
}
