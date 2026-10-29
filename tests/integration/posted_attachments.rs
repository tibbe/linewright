//! The attachments a session's result refers to, uploaded and posted in
//! place of their paths: docs/specs/runs.md, "Attachments".

use crate::common;

use common::*;
use linewright::reconcile::WorkKind;
use linewright::runs::Ending;
use linewright::store::local::{AttachRefusal, Write};
use serde_json::json;

/// What a session saves as its screenshot.
const SHOT: &str = "PNG bytes";

/// Where the local store keeps [`SHOT`], from `sha256sum` of it.
const KEPT_SHOT: &str =
    "../attachments/9698103cd397cda6f9807a2e2bf51935727f47a62d9b6d4f82e54ef60c6ba831.png";

/// Saves [`SHOT`] in the session's attachments directory, returning its
/// path there.
fn save_shot(call: &Call) -> String {
    let path = call.spec.attachments.join("shot.png");
    std::fs::write(&path, SHOT).unwrap();
    path.display().to_string()
}

/// Has each implement session do the ticket in its first turn and answer
/// `done` with a `pr_body` made by `body` from the session's call.
fn implement_with_body(world: &World, body: impl Fn(&Call) -> String + Send + Sync + 'static) {
    world.harness.on(WorkKind::Implement, move |call| {
        if call.followup.is_none() {
            commit_in(&call.spec.dir, &[("src/gadgets.txt", "done\n")], "Do it");
        }
        Ok(
            json!({"outcome": "done", "pr_title": "Show gadgets", "pr_body": body(call)})
                .to_string(),
        )
    });
}

#[tokio::test]
async fn an_implement_run_opens_its_pull_request_showing_the_screenshot_its_body_refers_to() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Implement, |call| {
        commit_in(&call.spec.dir, &[("src/gadgets.txt", "done\n")], "Do it");
        let shot = save_shot(call);
        Ok(json!({
            "outcome": "done",
            "pr_title": "Show gadgets",
            "pr_body": format!("Shows them.\n\n![The gadgets]({shot})"),
        })
        .to_string())
    });
    let ticket = open_ticket(&world, "Gadgets");

    world.poll().await;

    let pr = world.store.snapshot_pull(world.store.pulls()[0]).await;
    assert_eq!(
        pr.body,
        format!("Shows them.\n\n![The gadgets]({KEPT_SHOT})\n\nCloses {ticket}")
    );
}

#[tokio::test]
async fn an_implement_run_uploads_its_attachments_before_it_pushes() {
    let mut world = World::new().await;
    implement_with_body(&world, |call| {
        format!("![The gadgets]({})", save_shot(call))
    });
    let ticket = open_ticket(&world, "Gadgets");
    let mut paused = world.store.pause_after(Write::Attach {
        name: "shot.png".into(),
    });

    world.factory.cycle().await.unwrap();
    paused.reached().await;
    let branch = format!("{}-gadgets", ticket.0);
    assert!(world.store.file(&branch, "src/gadgets.txt").await.is_err());
    paused.release();
    world.factory.settle().await;

    assert_eq!(
        world.store.file(&branch, "src/gadgets.txt").await.unwrap(),
        "done\n"
    );
    assert_eq!(world.store.pulls().len(), 1);
}

#[tokio::test]
async fn a_reference_to_a_missing_attachment_is_sent_back_and_the_corrected_answer_posted() {
    let mut world = World::new().await;
    implement_with_body(&world, |call| {
        if call.followup.is_none() {
            let missing = call.spec.attachments.join("shot.png");
            format!("![The gadgets]({})", missing.display())
        } else {
            format!("![The gadgets]({})", save_shot(call))
        }
    });
    let ticket = open_ticket(&world, "Gadgets");

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[1].starts_with("Your final message could not be used: `")
            && prompts[1].contains(
                "shot.png` is in the attachments directory but is not a regular file there"
            ),
        "{}",
        prompts[1]
    );
    let pr = world.store.snapshot_pull(world.store.pulls()[0]).await;
    assert_eq!(
        pr.body,
        format!("![The gadgets]({KEPT_SHOT})\n\nCloses {ticket}")
    );
}

#[tokio::test]
async fn an_attachment_holding_a_secret_is_sent_back_naming_the_variable_and_the_file() {
    let mut world = World::with(|config| {
        config.secrets = secrets_of(&[("NPM_TOKEN", "npm-s3cr3t")]);
    })
    .await;
    implement_with_body(&world, |call| {
        let log = call.spec.attachments.join("log.txt");
        std::fs::write(&log, "token: npm-s3cr3t\n").unwrap();
        format!("[The log]({})", log.display())
    });
    open_ticket(&world, "Gadgets");

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 2);
    let attachments =
        std::fs::canonicalize(world.dir.path().join("work").join("sessions")).unwrap();
    assert!(
        prompts[1].starts_with("Your work is not finished: the attachment `")
            && prompts[1].contains(&attachments.display().to_string())
            && prompts[1]
                .contains("/attachments/log.txt` holds the value of `NPM_TOKEN`, a secret."),
        "{}",
        prompts[1]
    );
    assert!(!prompts[1].contains("npm-s3cr3t"), "{}", prompts[1]);
    assert_eq!(world.store.pulls(), []);
}

#[tokio::test]
async fn a_triage_comment_shows_the_attachment_it_refers_to() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Triage, |call| {
        Ok(json!({
            "outcome": "ready-for-agent",
            "category": "bug",
            "comment": format!("It crashes here:\n\n![The crash]({})", save_shot(call)),
        })
        .to_string())
    });
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);

    world.poll().await;

    assert_eq!(
        last_comment(&world.issue(issue)),
        format!("It crashes here:\n\n![The crash]({KEPT_SHOT})")
    );
}

#[tokio::test]
async fn a_design_shows_the_attachment_it_refers_to() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Design, |call| {
        Ok(json!({
            "outcome": "designed",
            "design": format!("Like this:\n\n![The sketch]({})", save_shot(call)),
        })
        .to_string())
    });
    let issue = world
        .store
        .open_issue("maintainer", "Gadgets", "Please.", &["ready-for-agent"]);

    world.poll().await;

    assert_eq!(
        last_comment(&world.issue(issue)),
        format!("Like this:\n\n![The sketch]({KEPT_SHOT})")
    );
}

#[tokio::test]
async fn a_rework_comment_shows_the_attachment_it_refers_to() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.rework_asked(pr);
    world.harness.on(WorkKind::Rework, |call| {
        commit_in(
            &call.spec.dir,
            &[("src/renamed.txt", "renamed\n")],
            "Rename",
        );
        Ok(json!({
            "outcome": "done",
            "comment": format!("Renamed:\n\n![Renamed]({})", save_shot(call)),
        })
        .to_string())
    });

    world.poll().await;

    assert_eq!(
        last_pull_comment(&world, pr).await,
        format!("Renamed:\n\n![Renamed]({KEPT_SHOT})")
    );
}

#[tokio::test]
async fn a_cancel_during_the_uploads_followed_by_a_refusal_cancels_the_run_and_posts_nothing() {
    for refusal in [
        AttachRefusal::File("File is too large".into()),
        AttachRefusal::Token("Not Found".into()),
    ] {
        cancelled_during_the_uploads(refusal).await;
    }
}

/// Has a cancel come while an upload is under way, which GitHub then
/// refuses as `refusal` says.
async fn cancelled_during_the_uploads(refusal: AttachRefusal) {
    let mut world = World::new().await;
    implement_with_body(&world, |call| {
        format!("![The gadgets]({})", save_shot(call))
    });
    let ticket = open_ticket(&world, "Gadgets");
    world.store.refuse_attachments(refusal.clone());
    let mut paused = world.store.pause_after(Write::Attach {
        name: "shot.png".into(),
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
    assert_eq!(world.issue(ticket).comments, [], "{refusal:?}");
    assert_eq!(ended_runs(&world), [Ending::Cancelled], "{refusal:?}");
}

#[tokio::test]
async fn an_attachment_github_refuses_is_sent_back_with_its_reason() {
    let mut world = World::new().await;
    let store = world.store.clone();
    implement_with_body(&world, move |call| {
        if call.followup.is_none() {
            store.refuse_attachments(AttachRefusal::File("File is too large".into()));
            format!("![The gadgets]({})", save_shot(call))
        } else {
            store.accept_attachments();
            "Shows them.".into()
        }
    });
    open_ticket(&world, "Gadgets");

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[1]
            .starts_with("Your final message could not be used: GitHub refused the attachment `")
            && prompts[1].contains("shot.png`: File is too large."),
        "{}",
        prompts[1]
    );
    assert_eq!(world.store.pulls().len(), 1);
}

#[tokio::test]
async fn an_attachment_github_refuses_over_linewrights_token_hands_the_ticket_back_at_once_keeping_the_work()
 {
    let mut world = World::new().await;
    implement_with_body(&world, |call| {
        format!("![The gadgets]({})", save_shot(call))
    });
    let ticket = open_ticket(&world, "Gadgets");
    world
        .store
        .refuse_attachments(AttachRefusal::Token("Not Found".into()));

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["linewright:stalled", "ready-for-human"]);
    let comment = last_comment(&issue);
    assert!(
        comment.starts_with(
            "GitHub refused Linewright's token the upload of the attachment `shot.png`. \
             GitHub's reason: Not Found\n\nLinewright's token needs write access to the \
             repository, in a classic or fine-grained personal access token"
        ),
        "{comment}"
    );
    assert_eq!(
        world
            .store
            .file(&format!("{}-gadgets", ticket.0), "src/gadgets.txt")
            .await
            .unwrap(),
        "done\n"
    );
    assert_eq!(world.store.pulls(), []);
}

#[tokio::test]
async fn a_spec_gap_already_filed_is_not_uploaded_for() {
    let mut world = World::new().await;
    let ticket = open_ticket(&world, "Gadgets");
    world.store.open_issue(
        LINEWRIGHT,
        "Spec: gadget colors",
        &format!("The spec doesn't say.\n\nFound while working on {ticket}."),
        &["ready-for-human"],
    );
    world.harness.on(WorkKind::Implement, |call| {
        commit_in(&call.spec.dir, &[("src/gadgets.txt", "done\n")], "Do it");
        let missing = call.spec.attachments.join("colors.png");
        Ok(json!({
            "outcome": "done",
            "pr_title": "Show gadgets",
            "pr_body": "Shows them.",
            "spec_gaps": [{
                "title": "Spec: gadget colors",
                "body": format!("![The colors]({})", missing.display()),
            }],
        })
        .to_string())
    });

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
    assert_eq!(world.store.pulls().len(), 1);
}

#[tokio::test]
async fn every_prompt_names_the_sessions_attachments_directory() {
    let mut world = World::new().await;
    let named = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let seen = named.clone();
    world.harness.on(WorkKind::Triage, move |call| {
        let directory = call.spec.attachments.display().to_string();
        seen.lock().unwrap().push((
            call.spec.prompt.contains(&format!("`{directory}`")),
            call.spec.attachments.is_dir(),
        ));
        Ok(
            json!({"outcome": "ready-for-agent", "category": "bug", "comment": "Brief."})
                .to_string(),
        )
    });
    world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);

    world.poll().await;

    assert_eq!(*named.lock().unwrap(), [(true, true)]);
}
