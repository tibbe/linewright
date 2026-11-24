use linewright_core::reconcile::WorkKind;
use linewright_core::testing::*;
use serde_json::json;

/// A world requiring the label [`REQUIRED`], which the repository has if
/// `label_exists`.
async fn world(label_exists: bool) -> World {
    let world = World::with(|c| c.require_label = Some(REQUIRED.into())).await;
    if label_exists {
        world.store.create_label(REQUIRED);
    }
    world
}

async fn merged_spec_change(world: &World) {
    world.harness.on(WorkKind::Design, |_| {
        Ok(json!({
            "outcome": "designed", "design": "A gadget store.", "tickets": [
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
    world.store.label_as("maintainer", pr, REQUIRED).unwrap();
}

fn all_labels(world: &World) -> Vec<Vec<String>> {
    world.store.issues().into_iter().map(|i| i.labels).collect()
}

fn assert_each_carries_required(all: &[Vec<String>]) {
    assert!(
        all.iter().all(|l| l.iter().any(|label| label == REQUIRED)),
        "{all:?}"
    );
}

#[test_log::test(tokio::test)]
async fn the_issue_and_every_ticket_of_a_spec_change_carry_the_required_label() {
    let mut world = world(true).await;
    merged_spec_change(&world).await;

    world.poll().await;
    world.poll().await;

    let all = all_labels(&world);
    assert_eq!(all.len(), 3, "the issue and its two tickets: {all:?}");
    assert_each_carries_required(&all);
}

#[test_log::test(tokio::test)]
async fn tickets_get_ready_for_agent_only_after_every_blocker_edge_exists() {
    let mut world = world(true).await;
    merged_spec_change(&world).await;

    world.poll().await;
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

#[test_log::test(tokio::test)]
async fn a_missing_required_label_is_not_created_and_the_filing_completes() {
    let mut world = world(false).await;
    merged_spec_change(&world).await;
    world.poll().await;
    let issue = world.store.issues()[0].number;
    world
        .store
        .assign_as("maintainer", issue, LINEWRIGHT)
        .unwrap();

    world.poll().await;

    assert_eq!(
        all_labels(&world),
        [
            vec!["linewright:designed".to_string()],
            vec![
                "linewright:designed".to_string(),
                "ready-for-agent".to_string()
            ],
            vec![
                "linewright:designed".to_string(),
                "ready-for-agent".to_string()
            ],
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

#[test_log::test(tokio::test)]
async fn a_draft_pull_request_and_a_spec_gap_issue_carry_the_required_label() {
    let mut world = world(true).await;
    implement_with_gap(&world);
    let ticket = world.store.open_issue(
        "maintainer",
        "Gadgets",
        "",
        &["ready-for-agent", "linewright:designed", REQUIRED],
    );

    world.poll().await;

    let gap = world
        .store
        .issues()
        .into_iter()
        .find(|i| i.title == "Spec: gadget colors")
        .unwrap();
    assert!(labels(&gap).contains(&REQUIRED), "{:?}", gap.labels);
    let pulls = world.store.pulls();
    assert_eq!(pulls.len(), 1, "for {ticket}");
    assert!(
        world
            .store
            .pull_labels(pulls[0])
            .contains(&REQUIRED.to_string())
    );
}

#[test_log::test(tokio::test)]
async fn a_missing_required_label_leaves_the_draft_pull_request_and_spec_gap_without_it() {
    let mut world = world(false).await;
    implement_with_gap(&world);
    world.store.open_issue(
        "maintainer",
        "Gadgets",
        "",
        &["ready-for-agent", "linewright:designed", REQUIRED],
    );

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
session-timeout: 60m
max-issues: 3
---
Look at the bugs.
";

async fn filing_automation(label_exists: bool) -> (World, Vec<Vec<String>>) {
    let mut world = world(label_exists).await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(".linewright/automations/weekly.md", WEEKLY)],
            "Add an automation",
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
    world.poll().await;
    world.run_by_hand("weekly.md").await.unwrap();
    let all = all_labels(&world);
    (world, all)
}

#[test_log::test(tokio::test)]
async fn automation_filed_issues_carry_the_required_label() {
    let (_, all) = filing_automation(true).await;
    assert_eq!(all.len(), 2, "{all:?}");
    assert_each_carries_required(&all);
}

#[test_log::test(tokio::test)]
async fn automation_filed_issues_are_filed_without_a_missing_required_label() {
    let (world, all) = filing_automation(false).await;
    assert_eq!(
        all,
        [
            vec!["ready-for-agent".to_string()],
            vec!["ready-for-human".to_string()]
        ]
    );
    assert!(!world.store.has_label(REQUIRED));
}
