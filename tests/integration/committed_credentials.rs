//! A finished session whose commits hold a credential: docs/specs/runs.md,
//! "A result the manager can't accept".

use crate::common;

use common::*;
use linewright::reconcile::WorkKind;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const CREDENTIAL: &str = "npm-s3cr3t";

async fn world_with_credential() -> World {
    World::with(|config| {
        config.credentials = credentials_of(&[("NPM_TOKEN", CREDENTIAL)]);
    })
    .await
}

/// Commits the credential to `.npmrc`, returning the commit's abbreviated
/// name.
fn commit_credential(dir: &Path) -> String {
    commit_in(
        dir,
        &[(
            ".npmrc",
            &format!("//registry.npmjs.org/:_authToken={CREDENTIAL}\n"),
        )],
        "Publish to npm",
    );
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

/// Neither a prompt, a comment nor a kept record holds the credential.
async fn assert_credential_kept_out(world: &World) {
    for kind in [WorkKind::Implement, WorkKind::Rework] {
        for prompt in world.runtime.prompts(kind) {
            assert!(!prompt.contains(CREDENTIAL), "{prompt}");
        }
    }
    for issue in world.store.issues() {
        for comment in &issue.comments {
            assert!(!comment.body.contains(CREDENTIAL), "{}", comment.body);
        }
    }
    for pr in world.store.pulls() {
        for comment in world.store.snapshot_pull(pr).await.comments {
            assert!(!comment.body.contains(CREDENTIAL), "{}", comment.body);
        }
    }
    for path in files_under(&world.dir.path().join("work/sessions")) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(!text.contains(CREDENTIAL), "{}: {text}", path.display());
    }
}

fn reason(commit: &str) -> String {
    format!("the commit `{commit}` adds the value of `NPM_TOKEN`, a credential, to `.npmrc`")
}

#[tokio::test]
async fn an_implement_run_whose_commits_hold_a_credential_is_sent_back_before_it_pushes_and_opens_its_pull_request_once_they_do_not()
 {
    let mut world = world_with_credential().await;
    let store = world.store.repository();
    let committed = Arc::new(Mutex::new(String::new()));
    let pushed_before_followup = Arc::new(Mutex::new(None));
    world.runtime.on(WorkKind::Implement, {
        let committed = committed.clone();
        let pushed_before_followup = pushed_before_followup.clone();
        move |call| {
            let dir = &call.spec.dir;
            if call.followup.is_none() {
                *committed.lock().unwrap() = commit_credential(dir);
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
    world
        .store
        .open_issue("maintainer", "Publish", "", &["ready-for-agent"]);

    world.poll().await;

    let prompts = world.runtime.prompts(WorkKind::Implement);
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
    assert_credential_kept_out(&world).await;
}

#[tokio::test]
async fn an_implement_run_whose_commits_still_hold_a_credential_after_its_follow_ups_hands_its_ticket_back_and_pushes_nothing()
 {
    let (log, _guard) = Log::capture();
    let mut world = world_with_credential().await;
    let committed = Arc::new(Mutex::new(String::new()));
    world.runtime.on(WorkKind::Implement, {
        let committed = committed.clone();
        move |call| {
            if call.followup.is_none() {
                *committed.lock().unwrap() = commit_credential(&call.spec.dir);
            }
            Ok(
                json!({"outcome": "done", "pr_title": "Publish", "pr_body": "Publishes."})
                    .to_string(),
            )
        }
    });
    let ticket = world
        .store
        .open_issue("maintainer", "Publish", "", &["ready-for-agent"]);

    world.poll().await;

    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 2);
    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["ready-for-human"]);
    let comment = last_comment(&issue);
    let commit = committed.lock().unwrap().clone();
    assert!(
        comment.contains("still could not be used after 1 follow-up(s)")
            && comment.contains(&reason(&commit)),
        "{comment}"
    );
    assert_eq!(world.store.pulls(), []);
    assert_eq!(branches(&world.store.repository()), ["main"]);
    assert_credential_kept_out(&world).await;
    let logged = log.text();
    assert!(
        logged.contains("NPM_TOKEN") && !logged.contains(CREDENTIAL),
        "{logged}"
    );
}

#[tokio::test]
async fn a_rework_whose_commits_hold_a_credential_pushes_nothing_to_its_pull_request_and_is_sent_back_then_handed_back()
 {
    let mut world = world_with_credential().await;
    let (_, pr) = world.ticket_in_review().await;
    let head = world.store.snapshot_pull(pr).await.head_sha;
    world
        .store
        .comment_as("maintainer", pr, "Publish it too.")
        .unwrap();
    let committed = Arc::new(Mutex::new(String::new()));
    world.runtime.on(WorkKind::Rework, {
        let committed = committed.clone();
        move |call| {
            if call.followup.is_none() {
                *committed.lock().unwrap() = commit_credential(&call.spec.dir);
            }
            Ok(json!({"outcome": "done", "comment": "Published."}).to_string())
        }
    });

    world.poll().await;

    let prompts = world.runtime.prompts(WorkKind::Rework);
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
    assert_credential_kept_out(&world).await;
}

#[tokio::test]
async fn a_rework_that_takes_a_credential_out_of_its_commits_when_sent_back_pushes_them() {
    let mut world = world_with_credential().await;
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .comment_as("maintainer", pr, "Publish it too.")
        .unwrap();
    world.runtime.on(WorkKind::Rework, |call| {
        let dir = &call.spec.dir;
        if call.followup.is_none() {
            commit_credential(dir);
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

    assert_eq!(world.runtime.prompts(WorkKind::Rework).len(), 2);
    let head_ref = world.store.snapshot_pull(pr).await.head_ref;
    assert_eq!(
        world.store.file(&head_ref, ".npmrc").await.unwrap(),
        "//registry.npmjs.org/:_authToken=${NPM_TOKEN}\n"
    );
    assert_eq!(last_pull_comment(&world, pr).await, "Published.");
    assert_credential_kept_out(&world).await;
}

#[tokio::test]
async fn a_rework_is_not_sent_back_for_a_credential_its_pull_request_already_held() {
    let mut world = world_with_credential().await;
    let (_, pr) = world.ticket_in_review().await;
    let head_ref = world.store.snapshot_pull(pr).await.head_ref;
    world
        .store
        .commit(
            "alice",
            &head_ref,
            &[(".npmrc", &format!("_authToken={CREDENTIAL}\n"))],
            "Publish",
        )
        .await
        .unwrap();
    world.rework_asked(pr);

    world.poll().await;

    assert_eq!(world.runtime.prompts(WorkKind::Rework).len(), 1);
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
async fn a_credential_whose_value_spans_lines_is_found_whole() {
    let key = "-----BEGIN KEY-----\nc2VjcmV0\n-----END KEY-----";
    let mut world = World::with(|config| {
        config.credentials = credentials_of(&[("DEPLOY_KEY", key)]);
    })
    .await;
    world.runtime.on(WorkKind::Implement, move |call| {
        commit_in(
            &call.spec.dir,
            &[("deploy/key.pem", &format!("{key}\n"))],
            "Add the deploy key",
        );
        Ok(json!({"outcome": "done", "pr_title": "Deploy", "pr_body": "Deploys."}).to_string())
    });
    world
        .store
        .open_issue("maintainer", "Deploy", "", &["ready-for-agent"]);

    world.poll().await;

    let prompts = world.runtime.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[1].contains("adds the value of `DEPLOY_KEY`, a credential, to `deploy/key.pem`"),
        "{}",
        prompts[1]
    );
    assert_eq!(world.store.pulls(), []);
}

#[tokio::test]
async fn a_run_that_stops_with_a_credential_in_its_commits_keeps_nothing() {
    let mut world = world_with_credential().await;
    world.runtime.on(WorkKind::Implement, |call| {
        commit_credential(&call.spec.dir);
        Err(linewright::runtime::SessionError::TimedOut {
            log_tail: String::new(),
        })
    });
    let ticket = world
        .store
        .open_issue("maintainer", "Publish", "", &["ready-for-agent"]);

    world.poll().await;

    assert_eq!(labels(&world.issue(ticket)), ["ready-for-human"]);
    assert_eq!(branches(&world.store.repository()), ["main"]);
    assert_credential_kept_out(&world).await;
}
