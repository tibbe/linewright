//! A run's writes that change what its item asks for are its last, and a
//! new status label goes on before the old one comes off
//! (`docs/specs/runs.md`, "Recording the outcome"): paused part way through
//! them, the item still asks for the run's work.

use crate::common;

use common::*;
use linewright::domain::{Login, Number};
use linewright::harness::SessionError;
use linewright::reconcile::WorkKind;
use linewright::store::local::Write;
use serde_json::json;

fn triaged_to(world: &World, outcome: &str) {
    let outcome = outcome.to_string();
    world.harness.on(WorkKind::Triage, move |_| {
        Ok(json!({"outcome": outcome, "category": "bug", "comment": "Triaged."}).to_string())
    });
}

fn adding(number: Number, label: &str) -> Write {
    Write::AddLabel {
        number,
        label: label.into(),
    }
}

fn linewright_assigned(world: &World, issue: Number) -> bool {
    world.issue(issue).is_assigned(&Login::new(LINEWRIGHT))
}

fn timed_out() -> Result<String, SessionError> {
    Err(SessionError::TimedOut {
        log_tail: String::new(),
    })
}

#[tokio::test]
async fn a_triage_run_readying_an_issue_adds_its_label_after_the_comment_and_category_and_before_removing_needs_triage()
 {
    let mut world = World::new().await;
    triaged_to(&world, "ready-for-agent");
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);
    let mut paused = world.store.pause_after(adding(issue, "ready-for-agent"));
    world.factory.cycle().await.unwrap();
    paused.reached().await;

    let part_way = world.issue(issue);
    assert_eq!(
        labels(&part_way),
        ["bug", "needs-triage", "ready-for-agent"]
    );
    assert!(linewright_assigned(&world, issue));
    assert_eq!(last_comment(&part_way), "Triaged.");
    paused.release();
    world.factory.settle().await;

    assert_eq!(labels(&world.issue(issue)), ["bug", "ready-for-agent"]);
}

#[tokio::test]
async fn a_triage_run_handing_an_issue_to_a_person_unassigns_linewright_last() {
    let mut world = World::new().await;
    triaged_to(&world, "ready-for-human");
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);
    let mut paused = world.store.pause_after(Write::RemoveLabel {
        number: issue,
        label: "needs-triage".into(),
    });
    world.factory.cycle().await.unwrap();
    paused.reached().await;

    assert_eq!(labels(&world.issue(issue)), ["bug", "ready-for-human"]);
    assert!(linewright_assigned(&world, issue));
    paused.release();
    world.factory.settle().await;

    assert!(!linewright_assigned(&world, issue));
}

#[tokio::test]
async fn an_issue_stalled_and_handed_back_is_labeled_stalled_and_undesigned_before_ready_for_human_and_unassigned_after()
 {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Implement, |_| timed_out());
    let ticket = open_ticket(&world, "Gadgets");
    let mut paused = world.store.pause_after(adding(ticket, "ready-for-human"));
    world.factory.cycle().await.unwrap();
    paused.reached().await;

    assert_eq!(
        labels(&world.issue(ticket)),
        ["linewright:stalled", "ready-for-agent", "ready-for-human"]
    );
    assert!(linewright_assigned(&world, ticket));
    paused.release();
    world.factory.settle().await;

    let handed_back = world.issue(ticket);
    assert_eq!(
        labels(&handed_back),
        ["linewright:stalled", "ready-for-human"]
    );
    assert!(!linewright_assigned(&world, ticket));
}

#[tokio::test]
async fn a_pull_request_stalled_and_handed_back_is_labeled_stalled_before_linewright_unassigns() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.rework_asked(pr);
    world.harness.on(WorkKind::Rework, |_| timed_out());
    let mut paused = world.store.pause_after(adding(pr, "linewright:stalled"));
    world.factory.cycle().await.unwrap();
    paused.reached().await;

    assert_eq!(world.store.pull_assignees(pr), [Login::new(LINEWRIGHT)]);
    paused.release();
    world.factory.settle().await;

    assert_eq!(world.store.pull_assignees(pr), []);
    assert_eq!(world.store.pull_labels(pr), ["linewright:stalled"]);
}

#[tokio::test]
async fn a_reply_to_needs_info_adds_needs_triage_before_removing_needs_info() {
    let mut world = World::new().await;
    triaged_to(&world, "needs-info");
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);
    world.poll().await;
    assert_eq!(labels(&world.issue(issue)), ["bug", "needs-info"]);
    world
        .store
        .comment_as("reporter", issue, "Version 3.")
        .unwrap();
    triaged_to(&world, "ready-for-agent");
    let mut paused = world.store.pause_after(adding(issue, "needs-triage"));
    let store = world.store.clone();

    let (cycled, ()) = tokio::join!(world.factory.cycle(), async {
        paused.reached().await;
        assert_eq!(
            labels(&store.issue(issue).unwrap()),
            ["bug", "needs-info", "needs-triage"]
        );
        paused.release();
    });
    cycled.unwrap();
    world.factory.settle().await;

    assert_eq!(labels(&world.issue(issue)), ["bug", "ready-for-agent"]);
}

#[tokio::test]
async fn a_cut_labels_its_issue_designed_before_removing_its_status_label_and_letting_go() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Design, |_| {
        Ok(json!({
            "outcome": "designed",
            "design": "A gadget store.",
            "tickets": [{"id": "store", "title": "Store gadgets", "body": "Gadgets persist.", "blocked_by": []}],
            "comment": "One slice."
        })
        .to_string())
    });
    implement(&world);
    let issue = world
        .store
        .open_issue("maintainer", "Gadgets", "Please.", &["ready-for-agent"]);
    let mut paused = world
        .store
        .pause_after(adding(issue, "linewright:designed"));
    world.factory.cycle().await.unwrap();
    paused.reached().await;

    assert_eq!(
        labels(&world.issue(issue)),
        ["linewright:designed", "ready-for-agent"]
    );
    assert!(linewright_assigned(&world, issue));
    paused.release();
    world.factory.settle().await;

    assert_eq!(labels(&world.issue(issue)), ["linewright:designed"]);
    assert!(!linewright_assigned(&world, issue));
}
