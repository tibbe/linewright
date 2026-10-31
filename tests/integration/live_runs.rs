//! A poll made while a run is part way through writing its result leaves
//! the items the run writes to it, and the next poll after the run ends
//! decides from what it wrote.

use crate::common;

use chrono::TimeDelta;
use common::*;
use linewright::domain::{Issue, Login, Number};
use linewright::reconcile::{Action, Work, WorkKind};
use linewright::store::local::{Paused, Write};
use serde_json::json;

fn triaged_to(world: &World, outcome: serde_json::Value) {
    world.harness.on(WorkKind::Triage, move |_| {
        let mut result = json!({"category": "bug", "comment": "Triaged."});
        result
            .as_object_mut()
            .unwrap()
            .extend(outcome.as_object().unwrap().clone());
        Ok(result.to_string())
    });
}

fn assignees(issue: &Issue) -> Vec<&str> {
    let mut assignees: Vec<&str> = issue.assignees.iter().map(Login::as_str).collect();
    assignees.sort_unstable();
    assignees
}

/// Polls while `paused` holds a run after one of its writes, asserting that
/// the poll writes nothing to `issue`, then lets the run end.
async fn poll_while_paused(world: &mut World, mut paused: Paused, issue: Number) -> Vec<Action> {
    paused.reached().await;
    let before = world.issue(issue);
    let actions = world.factory.cycle().await.unwrap();
    assert_eq!(world.issue(issue), before, "{actions:?}");
    paused.release();
    world.factory.settle().await;
    actions
}

fn removing(issue: Number, label: &str) -> Write {
    Write::RemoveLabel {
        number: issue,
        label: label.into(),
    }
}

#[tokio::test]
async fn a_triage_run_that_readies_an_issue_keeps_it_through_a_poll_part_way_and_its_design_starts_next()
 {
    let mut world = World::new().await;
    triaged_to(&world, json!({"outcome": "ready-for-agent"}));
    design_issues(&world);
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);
    let paused = world.store.pause_after(removing(issue, "needs-triage"));
    world.factory.cycle().await.unwrap();

    poll_while_paused(&mut world, paused, issue).await;

    let triaged = world.issue(issue);
    assert_eq!(labels(&triaged), ["bug", "ready-for-agent"]);
    assert_eq!(assignees(&triaged), [LINEWRIGHT]);
    assert_eq!(
        world.factory.cycle().await.unwrap(),
        [Action::Start(Work::DesignIssue { issue })]
    );
    world.factory.settle().await;
}

#[tokio::test]
async fn an_implement_runs_pull_request_is_left_to_it_through_a_poll_part_way() {
    let mut world = World::with(|config| config.require_label = Some(REQUIRED.into())).await;
    world.store.create_label(REQUIRED);
    implement(&world);
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed", REQUIRED],
    );
    let mut paused = world.store.pause_after(Write::OpenPull {
        head: world.issue(ticket).branch(),
    });
    world.factory.cycle().await.unwrap();
    paused.reached().await;
    let pr = *world
        .store
        .pulls()
        .last()
        .expect("the run opened its pull request");
    // Past the grace for CI to report, so that with no CI the pull request
    // is ready to be marked ready.
    world.clock.advance(TimeDelta::minutes(6));

    let actions = world.factory.cycle().await.unwrap();

    assert_eq!(actions, [], "{actions:?}");
    assert_eq!(world.store.pull_assignees(pr), []);
    paused.release();
    world.factory.settle().await;
    assert_eq!(world.store.pull_assignees(pr), [Login::new(LINEWRIGHT)]);
    assert!(world.store.pull_labels(pr).contains(&REQUIRED.to_string()));
    // The poll after the run finds the pull request claimed.
    assert_eq!(
        world.factory.cycle().await.unwrap(),
        [Action::MarkReady { pr }]
    );
}
