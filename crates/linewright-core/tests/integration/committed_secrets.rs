//! A finished session whose commits hold a secret: docs/specs/runs.md,
//! "A result Linewright can't accept".

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use linewright_core::reconcile::WorkKind;
use linewright_core::testing::*;
use serde_json::json;

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

/// What a session is sent back with when its commit `commit` adds the value
/// of `variable` to `file`.
fn sent_back_for_secret(commit: &str, variable: &str, file: &str) -> String {
    not_finished(&marker(
        "parts/held-credential.md",
        &[("commit", commit), ("variable", variable), ("file", file)],
    ))
}

#[test_log::test(tokio::test)]
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
    assert_eq!(
        prompts[1],
        sent_back_for_secret(&commit, "NPM_TOKEN", ".npmrc")
    );
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

#[test_log::test(tokio::test)]
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
        comment.contains("its result was not accepted: ")
            && comment.contains(&sent_back_for_secret(&commit, "NPM_TOKEN", ".npmrc")),
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

#[test_log::test(tokio::test)]
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
    assert_eq!(
        prompts[1],
        sent_back_for_secret(&commit, "NPM_TOKEN", ".npmrc")
    );
    assert_eq!(world.store.snapshot_pull(pr).await.head_sha, head);
    let comment = last_pull_comment(&world, pr).await;
    assert!(
        comment.contains("its result was not accepted: ")
            && comment.contains(&sent_back_for_secret(&commit, "NPM_TOKEN", ".npmrc")),
        "{comment}"
    );
    assert_secret_kept_out(&world).await;
}

#[test_log::test(tokio::test)]
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

#[test_log::test(tokio::test)]
async fn a_commit_holding_a_harness_credential_no_one_named_is_sent_back_naming_its_variable() {
    let token = "sk-ant-oat01-s3cr3t";
    let mut world = World::with(|config| {
        config.secrets = secrets_unnamed(&[("CLAUDE_SCRIPTED_TOKEN", token)]);
    })
    .await;
    let committed = Arc::new(Mutex::new(String::new()));
    world.harness.on(WorkKind::Implement, {
        let committed = committed.clone();
        move |call| {
            if call.followup.is_none() {
                commit_in(
                    &call.spec.dir,
                    &[("auth.txt", &format!("{token}\n"))],
                    "Add the token",
                );
                *committed.lock().unwrap() =
                    git_out(&call.spec.dir, &["rev-parse", "--short", "HEAD"]);
            }
            Ok(json!({"outcome": "done", "pr_title": "Token", "pr_body": "Adds it."}).to_string())
        }
    });
    world.store.open_issue(
        "maintainer",
        "Token",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 2);
    assert_eq!(
        prompts[1],
        sent_back_for_secret(
            &committed.lock().unwrap(),
            "CLAUDE_SCRIPTED_TOKEN",
            "auth.txt"
        )
    );
    assert_eq!(world.store.pulls(), []);
}

/// Asserts that `comment` names, in turn, the commit that holds the secret
/// in `.npmrc`, which is `commit` when given, its variable and the file.
fn assert_names_the_secret(comment: &str, commit: Option<&str>) {
    let quoted: Vec<&str> = comment.split('`').skip(1).step_by(2).collect();
    let named = quoted.windows(3).any(|names| {
        let is_commit = match commit {
            Some(commit) => names[0] == commit,
            None => names[0].len() >= 7 && names[0].chars().all(|c| c.is_ascii_hexdigit()),
        };
        is_commit && names[1] == "NPM_TOKEN" && names[2] == ".npmrc"
    });
    assert!(named, "{comment}");
}

#[test_log::test(tokio::test)]
async fn an_implement_run_that_stops_with_a_secret_in_its_commits_keeps_nothing_and_says_why() {
    let mut world = world_with_secret().await;
    let committed = Arc::new(Mutex::new(String::new()));
    world.harness.on(WorkKind::Implement, {
        let committed = committed.clone();
        move |call| {
            *committed.lock().unwrap() = commit_secret(&call.spec.dir);
            Err(linewright_core::harness::SessionError::TimedOut {
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
    assert_names_the_secret(last_comment(&issue), Some(&commit));
    assert_eq!(branches(&world.store.repository()), ["main"]);
    assert_secret_kept_out(&world).await;
}

#[test_log::test(tokio::test)]
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
    assert_names_the_secret(comment, None);
    assert_eq!(branches(&world.store.repository()), ["main"]);
    assert_secret_kept_out(&world).await;
}

#[test_log::test(tokio::test)]
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
    assert_names_the_secret(&comment, None);
    assert_secret_kept_out(&world).await;
}

#[test_log::test(tokio::test)]
async fn a_run_that_stops_with_work_that_holds_no_secret_still_keeps_it() {
    let mut world = world_with_secret().await;
    world.harness.on(WorkKind::Implement, |call| {
        std::fs::write(
            call.spec.dir.join(".npmrc"),
            "//registry.npmjs.org/:_authToken=${NPM_TOKEN}\n",
        )
        .unwrap();
        Err(linewright_core::harness::SessionError::TimedOut {
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
