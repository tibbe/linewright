//! A push GitHub's push protection refuses over a secret in the work:
//! docs/specs/runs.md, "A result the manager can't accept" and "Unfinished
//! work".

use crate::common;

use common::*;
use linewright::domain::CheckState;
use linewright::reconcile::WorkKind;
use linewright::runs::Ending;
use linewright::store::local::Write;
use serde_json::json;
use std::path::Path;

/// What `git push` reported when GitHub's push protection refused a push,
/// recorded.
const REFUSAL: &str = include_str!("../../src/store/fixtures/push-protection.txt");

/// What GitHub's push protection reported finding, in [`REFUSAL`].
const FOUND: &str = "GitHub Personal Access Token in the commit \
                     `8479b4565e1a22d92de1bb6edb317cdc4dc64212`, at \
                     `tests/integration/push_protection.rs:22`";

/// Commits a fake token to `src/config.rs`. The token is put together here,
/// so that this file holds no text GitHub's push protection takes for one.
fn commit_token(dir: &Path) {
    let token = ["ghp_", "1wR8sX3kQ9vT2mZ7pL4nB6yC0dF5gH8jK2aE"].concat();
    commit_in(
        dir,
        &[(
            "src/config.rs",
            &format!("const TOKEN: &str = \"{token}\";\n"),
        )],
        "Configure",
    );
}

/// Rewrites the clone's last commit to read the token from the environment.
fn take_token_out(dir: &Path) {
    git_in(dir, &["reset", "--quiet", "--hard", "HEAD~1"]);
    commit_in(dir, &[("src/config.rs", FIXED)], "Configure");
}

const FIXED: &str = "const TOKEN: &str = env!(\"TOKEN\");\n";

#[tokio::test]
async fn an_implement_run_whose_push_github_refuses_over_a_secret_is_sent_back_and_opens_its_pull_request_once_it_is_taken_out()
 {
    let mut world = World::new().await;
    let store = world.store.clone();
    world.harness.on(WorkKind::Implement, move |call| {
        let dir = &call.spec.dir;
        if call.followup.is_none() {
            commit_token(dir);
            store.refuse_pushes(REFUSAL);
        } else {
            take_token_out(dir);
            store.accept_pushes();
        }
        Ok(
            json!({"outcome": "done", "pr_title": "Configure", "pr_body": "Configures."})
                .to_string(),
        )
    });
    world.store.open_issue(
        "maintainer",
        "Configure",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 2);
    assert!(prompts[1].contains(FOUND), "{}", prompts[1]);
    let pr = world.store.snapshot_pull(world.store.pulls()[0]).await;
    assert_eq!(
        world
            .store
            .file(&pr.head_ref, "src/config.rs")
            .await
            .unwrap(),
        FIXED
    );
}

#[tokio::test]
async fn an_implement_run_whose_push_github_still_refuses_after_its_follow_ups_hands_its_ticket_back_with_the_reason()
 {
    let mut world = World::new().await;
    let store = world.store.clone();
    world.harness.on(WorkKind::Implement, move |call| {
        if call.followup.is_none() {
            commit_token(&call.spec.dir);
            store.refuse_pushes(REFUSAL);
        }
        Ok(
            json!({"outcome": "done", "pr_title": "Configure", "pr_body": "Configures."})
                .to_string(),
        )
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Configure",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 2);
    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["linewright:stalled", "ready-for-human"]);
    let comment = last_comment(&issue);
    assert!(
        comment.contains("still could not be used after 1 follow-up(s)")
            && comment.contains(&format!(
                "GitHub's push protection refused the push over a secret: {FOUND}."
            ))
            && comment.contains(&format!(
                "\n\nThe work so far was not kept, as it holds a secret: GitHub's push protection \
                 found {FOUND}."
            ))
            && !comment.contains("infrastructure"),
        "{comment}"
    );
    assert_eq!(world.store.pulls(), []);
}

#[tokio::test]
async fn a_ticket_handed_back_as_github_still_refuses_its_push_says_once_that_the_work_was_not_kept()
 {
    let mut world = World::new().await;
    let store = world.store.clone();
    world.harness.on(WorkKind::Implement, move |call| {
        if call.followup.is_none() {
            commit_token(&call.spec.dir);
            store.refuse_pushes(REFUSAL);
        }
        Ok(
            json!({"outcome": "done", "pr_title": "Configure", "pr_body": "Configures."})
                .to_string(),
        )
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Configure",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let issue = world.issue(ticket);
    let comment = last_comment(&issue);
    assert_eq!(
        comment.matches("The work so far was not kept").count(),
        1,
        "{comment}"
    );
}

#[tokio::test]
async fn a_rework_whose_push_github_refuses_over_a_secret_is_sent_back_and_pushes_once_it_is_taken_out()
 {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.rework_asked(pr);
    let store = world.store.clone();
    world.harness.on(WorkKind::Rework, move |call| {
        let dir = &call.spec.dir;
        if call.followup.is_none() {
            commit_token(dir);
            store.refuse_pushes(REFUSAL);
        } else {
            take_token_out(dir);
            store.accept_pushes();
        }
        Ok(json!({"outcome": "done", "comment": "Configured."}).to_string())
    });

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Rework);
    assert_eq!(prompts.len(), 2);
    let head_ref = world.store.snapshot_pull(pr).await.head_ref;
    assert_eq!(
        world.store.file(&head_ref, "src/config.rs").await.unwrap(),
        FIXED
    );
    assert_eq!(last_pull_comment(&world, pr).await, "Configured.");
}

#[tokio::test]
async fn a_fix_ci_run_whose_push_github_still_refuses_after_its_follow_ups_hands_its_pull_request_back_with_the_reason()
 {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    let before = world.store.snapshot_pull(pr).await.head_sha;
    world
        .store
        .set_checks(pr, vec![check("test", CheckState::Failed, "boom")])
        .await
        .unwrap();
    let store = world.store.clone();
    world.harness.on(WorkKind::FixCi, move |call| {
        if call.followup.is_none() {
            commit_token(&call.spec.dir);
            store.refuse_pushes(REFUSAL);
        }
        Ok(json!({"outcome": "done", "comment": "Fixed."}).to_string())
    });

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::FixCi).len(), 2);
    assert_eq!(world.store.snapshot_pull(pr).await.head_sha, before);
    let comment = last_pull_comment(&world, pr).await;
    assert!(
        comment.contains("still could not be used after 1 follow-up(s)")
            && comment.contains(&format!(
                "GitHub's push protection refused the push over a secret: {FOUND}."
            )),
        "{comment}"
    );
}

#[tokio::test]
async fn a_blocked_run_whose_work_github_refuses_over_a_secret_says_the_work_was_not_kept_for_it() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Implement, |call| {
        commit_token(&call.spec.dir);
        Ok(json!({"outcome": "blocked", "question": "Which token?"}).to_string())
    });
    world.store.refuse_pushes(REFUSAL);
    let ticket = world.store.open_issue(
        "maintainer",
        "Configure",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let issue = world.issue(ticket);
    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
    assert_eq!(labels(&issue), ["ready-for-human"]);
    assert_eq!(
        last_comment(&issue),
        format!(
            "Blocked on a question the spec cannot decide:\n\nWhich token?\n\nThe work so far \
             was not kept, as it holds a secret: GitHub's push protection found {FOUND}."
        )
    );
}

#[tokio::test]
async fn what_github_reports_of_a_secret_is_redacted() {
    let mut world = World::with(|config| {
        config.secrets = secrets_of(&[("DEPLOY_KEY", "push_protection")]);
    })
    .await;
    let store = world.store.clone();
    world.harness.on(WorkKind::Implement, move |call| {
        if call.followup.is_none() {
            commit_token(&call.spec.dir);
            store.refuse_pushes(REFUSAL);
        }
        Ok(
            json!({"outcome": "done", "pr_title": "Configure", "pr_body": "Configures."})
                .to_string(),
        )
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Configure",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    assert!(
        prompts[1].contains("`tests/integration/[redacted].rs:22`")
            && !prompts[1].contains("push_protection"),
        "{}",
        prompts[1]
    );
    let comment = last_comment(&world.issue(ticket)).to_string();
    assert!(
        comment.contains("`tests/integration/[redacted].rs:22`")
            && !comment.contains("push_protection"),
        "{comment}"
    );
}

#[tokio::test]
async fn a_result_sent_back_for_another_reason_first_has_no_follow_up_left_when_github_refuses_its_push()
 {
    let mut world = World::new().await;
    let store = world.store.clone();
    world.harness.on(WorkKind::Implement, move |call| {
        if call.followup.is_none() {
            return Ok("Done, I think.".to_string());
        }
        commit_token(&call.spec.dir);
        store.refuse_pushes(REFUSAL);
        Ok(
            json!({"outcome": "done", "pr_title": "Configure", "pr_body": "Configures."})
                .to_string(),
        )
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Configure",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 2);
    let comment = last_comment(&world.issue(ticket)).to_string();
    assert!(
        comment.contains("still could not be used after 1 follow-up(s)")
            && comment.contains(&format!(
                "GitHub's push protection refused the push over a secret: {FOUND}."
            )),
        "{comment}"
    );
    assert_eq!(world.store.pulls(), []);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_implement_run_whose_push_github_refuses_over_a_secret_can_be_cancelled_while_its_session_runs_again()
 {
    let mut world = World::new().await;
    let store = world.store.clone();
    let (mut gate, post) = gate();
    world.harness.on(WorkKind::Implement, move |call| {
        let dir = &call.spec.dir;
        if call.followup.is_none() {
            commit_token(dir);
            store.refuse_pushes(REFUSAL);
        } else {
            post.wait();
            take_token_out(dir);
            store.accept_pushes();
        }
        Ok(
            json!({"outcome": "done", "pr_title": "Configure", "pr_body": "Configures."})
                .to_string(),
        )
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Configure",
        "",
        &["ready-for-agent", "linewright:designed"],
    );
    world.factory.cycle().await.unwrap();
    gate.begun().await;

    world
        .store
        .unassign_as("maintainer", ticket, LINEWRIGHT)
        .unwrap();
    world.factory.cycle().await.unwrap();
    gate.release();
    world.factory.settle().await;

    assert_eq!(world.store.pulls(), []);
    assert_eq!(world.issue(ticket).comments, []);
    assert_eq!(ended_runs(&world), [Ending::Cancelled]);
}

#[tokio::test]
async fn a_cancel_during_a_push_github_then_refuses_over_a_secret_ends_the_run_without_resuming_its_session()
 {
    let mut world = World::new().await;
    let store = world.store.clone();
    world.harness.on(WorkKind::Implement, move |call| {
        commit_token(&call.spec.dir);
        store.refuse_pushes(REFUSAL);
        Ok(
            json!({"outcome": "done", "pr_title": "Configure", "pr_body": "Configures."})
                .to_string(),
        )
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Configure",
        "",
        &["ready-for-agent", "linewright:designed"],
    );
    let mut paused = world.store.pause_after(Write::Push {
        branch: format!("{}-configure", ticket.0),
    });
    world.factory.cycle().await.unwrap();
    paused.reached().await;

    world
        .store
        .unassign_as("maintainer", ticket, LINEWRIGHT)
        .unwrap();
    world.factory.cycle().await.unwrap();
    paused.release();
    world.factory.settle().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
    assert_eq!(world.store.pulls(), []);
    assert_eq!(world.issue(ticket).comments, []);
    assert_eq!(ended_runs(&world), [Ending::Cancelled]);
}
