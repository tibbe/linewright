use crate::common;

use common::*;
use linewright::reconcile::WorkKind;
use serde_json::json;

const REQUIRED: &str = "linewright";

async fn world(required: Option<&str>, label_exists: bool) -> World {
    let required = required.map(str::to_string);
    let world = World::with(|c| c.require_label = required).await;
    if label_exists {
        world.store.create_label(REQUIRED);
    }
    world
}

async fn merged_spec_change(world: &World) {
    world.harness.on(WorkKind::Tickets, |_| {
        Ok(json!({
            "tickets": [
                {"id": "store", "title": "Store gadgets", "body": "Gadgets persist.", "blocked_by": []},
                {"id": "list", "title": "List gadgets", "body": "List them.", "blocked_by": ["store"]}
            ],
            "comment": "Two slices."
        }).to_string())
    });
    let pr = world
        .store
        .merge_change(
            "maintainer",
            "Gadgets",
            "Adds gadgets.",
            &[("docs/spec.md", "# Spec\n\nGadgets exist.\n")],
        )
        .await
        .unwrap();
    world.store.label_as(pr, REQUIRED).unwrap();
}

fn all_labels(world: &World) -> Vec<Vec<String>> {
    world.store.issues().into_iter().map(|i| i.labels).collect()
}

#[tokio::test]
async fn the_tracking_issue_and_every_ticket_of_a_spec_change_carry_the_required_label() {
    let mut world = world(Some(REQUIRED), true).await;
    merged_spec_change(&world).await;

    world.poll().await;

    assert_eq!(
        all_labels(&world),
        [
            vec![REQUIRED.to_string()],
            vec![REQUIRED.to_string(), "ready-for-agent".to_string()],
            vec![REQUIRED.to_string(), "ready-for-agent".to_string()],
        ]
    );
}

#[tokio::test]
async fn tickets_get_ready_for_agent_only_after_every_blocker_edge_exists() {
    let mut world = world(Some(REQUIRED), true).await;
    merged_spec_change(&world).await;

    world.poll().await;

    let issues = world.store.issues();
    let ticket = &issues[2];
    assert_eq!(ticket.blocked_by.len(), 1);
    let ready = ticket
        .events
        .iter()
        .position(|e| format!("{e:?}").contains("ready-for-agent"))
        .unwrap();
    let required = ticket
        .events
        .iter()
        .position(|e| format!("{e:?}").contains(REQUIRED))
        .unwrap();
    assert!(required < ready, "{:?}", ticket.events);
}

#[tokio::test]
async fn unset_created_items_carry_exactly_the_labels_they_did_before() {
    let mut world = world(None, true).await;
    let pr = world
        .store
        .merge_change(
            "maintainer",
            "Gadgets",
            "Adds gadgets.",
            &[("docs/spec.md", "# Spec\n\nGadgets exist.\n")],
        )
        .await
        .unwrap();
    let _ = pr;
    world.harness.on(WorkKind::Tickets, |_| {
        Ok(json!({
            "tickets": [{"id": "a", "title": "A", "body": "a", "blocked_by": []}],
            "comment": "One."
        })
        .to_string())
    });

    world.poll().await;

    assert_eq!(
        all_labels(&world),
        [vec![], vec!["ready-for-agent".to_string()]]
    );
}

#[tokio::test]
async fn a_missing_required_label_is_not_created_and_the_filing_completes() {
    let mut world = world(Some(REQUIRED), false).await;
    merged_spec_change(&world).await;

    world.poll().await;

    assert_eq!(
        all_labels(&world),
        [
            vec![],
            vec!["ready-for-agent".to_string()],
            vec!["ready-for-agent".to_string()],
        ]
    );
    assert!(!world.store.has_label(REQUIRED));
}

fn implement_with_gap(world: &World) {
    world.harness.on(WorkKind::Implement, |call| {
        commit_in(&call.spec.dir, &[("src/g.txt", "g\n")], "Add gadgets");
        Ok(json!({
            "outcome": "done",
            "pr_title": "Add gadgets",
            "pr_body": "Gadgets.",
            "spec_gaps": [{"title": "Spec: gadget colors", "body": "Unsaid."}]
        })
        .to_string())
    });
}

#[tokio::test]
async fn a_draft_pull_request_and_a_spec_gap_issue_carry_the_required_label() {
    let mut world = world(Some(REQUIRED), true).await;
    implement_with_gap(&world);
    let ticket =
        world
            .store
            .open_issue("maintainer", "Gadgets", "", &["ready-for-agent", REQUIRED]);

    world.poll().await;

    let gap = world
        .store
        .issues()
        .into_iter()
        .find(|i| i.title == "Spec: gadget colors")
        .unwrap();
    assert_eq!(labels(&gap), [REQUIRED, "ready-for-human"]);
    let pulls = world.store.pulls();
    assert_eq!(pulls.len(), 1, "for {ticket}");
    assert!(
        world
            .store
            .pull_labels(pulls[0])
            .contains(&REQUIRED.to_string())
    );
}

#[tokio::test]
async fn a_missing_required_label_leaves_the_draft_pull_request_and_spec_gap_without_it() {
    let mut world = world(Some(REQUIRED), false).await;
    implement_with_gap(&world);
    // Claimed only with the label, which the repository then lacks.
    world
        .store
        .open_issue("maintainer", "Gadgets", "", &["ready-for-agent", REQUIRED]);

    world.poll().await;

    let pulls = world.store.pulls();
    assert_eq!(pulls.len(), 1);
    assert!(
        !world
            .store
            .pull_labels(pulls[0])
            .contains(&REQUIRED.to_string())
    );
    assert!(!world.store.has_label(REQUIRED));
    let gap = world
        .store
        .issues()
        .into_iter()
        .find(|i| i.title == "Spec: gadget colors")
        .unwrap();
    assert_eq!(labels(&gap), ["ready-for-human"]);
}

const WEEKLY: &str = "---
on:
  schedule: \"0 6 * * 1\"
timeout: 60m
max-issues: 3
---
Look at the bugs.
";

async fn filing_automation(
    required: Option<&str>,
    label_exists: bool,
) -> (World, Vec<Vec<String>>) {
    let world = world(required, label_exists).await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(".linewright/workflows/weekly.md", WEEKLY)],
            "Add a workflow",
        )
        .await
        .unwrap();
    world.harness.on_automation(|_| {
        Ok(json!({
            "summary": "Two.",
            "issues": [
                {"title": "Fix it", "body": "Do.", "label": "ready-for-agent"},
                {"title": "Decide", "body": "Hm.", "label": "ready-for-human"}
            ]
        })
        .to_string())
    });
    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();
    let all = all_labels(&world);
    (world, all)
}

#[tokio::test]
async fn automation_filed_issues_carry_the_required_label() {
    let (_, all) = filing_automation(Some(REQUIRED), true).await;
    assert_eq!(
        all,
        [
            vec!["ready-for-agent".to_string(), REQUIRED.to_string()],
            vec!["ready-for-human".to_string(), REQUIRED.to_string()],
        ]
    );
}

#[tokio::test]
async fn automation_filed_issues_are_filed_without_a_missing_required_label() {
    let (world, all) = filing_automation(Some(REQUIRED), false).await;
    assert_eq!(
        all,
        [
            vec!["ready-for-agent".to_string()],
            vec!["ready-for-human".to_string()]
        ]
    );
    assert!(!world.store.has_label(REQUIRED));
}

#[tokio::test]
async fn unset_automation_filed_issues_carry_only_their_state_label() {
    let (_, all) = filing_automation(None, true).await;
    assert_eq!(
        all,
        [
            vec!["ready-for-agent".to_string()],
            vec!["ready-for-human".to_string()]
        ]
    );
}
