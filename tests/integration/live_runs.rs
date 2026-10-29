//! A poll made while a run is part way through writing its result leaves
//! the items the run writes to it, and the next poll after the run ends
//! decides from what it wrote.

use crate::common;

use chrono::TimeDelta;
use common::*;
use linewright::domain::{Issue, Login, Number};
use linewright::reconcile::{Action, Subject, Work, WorkKind};
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

fn let_go_comments(issue: &Issue) -> usize {
    issue
        .comments
        .iter()
        .filter(|c| c.body.contains("has let go of it"))
        .count()
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
async fn a_triage_run_that_hands_an_issue_to_a_person_lets_go_of_it_once_through_a_poll_part_way() {
    let mut world = World::new().await;
    triaged_to(&world, json!({"outcome": "ready-for-human"}));
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);
    let paused = world.store.pause_after(removing(issue, "needs-triage"));
    world.factory.cycle().await.unwrap();

    poll_while_paused(&mut world, paused, issue).await;
    world.poll().await;

    let triaged = world.issue(issue);
    assert_eq!(labels(&triaged), ["bug", "ready-for-human"]);
    assert_eq!(assignees(&triaged), Vec::<&str>::new());
    assert_eq!(let_go_comments(&triaged), 0);
}

#[tokio::test]
async fn a_triage_run_that_asks_for_information_keeps_the_people_asked_through_a_poll_part_way() {
    let mut world = World::new().await;
    triaged_to(&world, json!({"outcome": "needs-info", "ask": ["tester"]}));
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);
    let paused = world
        .store
        .pause_after(Write::AssignPeople { number: issue });
    world.factory.cycle().await.unwrap();

    poll_while_paused(&mut world, paused, issue).await;
    world.poll().await;

    let triaged = world.issue(issue);
    assert_eq!(labels(&triaged), ["bug", "needs-info"]);
    assert_eq!(assignees(&triaged), ["tester"]);
    assert_eq!(let_go_comments(&triaged), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_needs_triage_while_its_triage_runs_cancels_the_run_and_lets_go_once_it_ends() {
    let mut world = World::new().await;
    let mut gate = gated(&world, WorkKind::Triage);
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);
    world.factory.cycle().await.unwrap();
    gate.begun().await;

    world.store.unlabel_as(issue, "needs-triage").unwrap();
    let actions = world.factory.cycle().await.unwrap();

    assert_eq!(
        actions,
        [Action::Cancel {
            subject: Subject::Issue(issue),
            reason: format!("`needs-triage` was removed from {issue}"),
        }]
    );
    assert_eq!(assignees(&world.issue(issue)), [LINEWRIGHT]);
    gate.release();
    world.factory.settle().await;

    world.poll().await;

    assert_eq!(assignees(&world.issue(issue)), Vec::<&str>::new());
}

/// An issue a maintainer assigns Linewright to while it holds no status
/// label, then labels `labels`.
fn assigned_by_hand(world: &World, labels: &[&str]) -> Number {
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    world
        .store
        .assign_as("maintainer", issue, LINEWRIGHT)
        .unwrap();
    world.clock.advance(TimeDelta::minutes(1));
    for label in labels {
        world.store.label_as("maintainer", issue, label).unwrap();
    }
    issue
}

#[tokio::test]
async fn an_issue_assigned_by_hand_and_handed_back_through_a_poll_part_way_is_not_let_go_of_besides()
 {
    let mut world = World::new().await;
    world
        .harness
        .on(WorkKind::Implement, |_| failed_session("it broke"));
    let issue = assigned_by_hand(&world, &["ready-for-agent", "linewright:designed"]);
    // The last status label it removes before adding `ready-for-human`.
    let paused = world.store.pause_after(removing(issue, "ready-for-agent"));
    world.factory.cycle().await.unwrap();

    poll_while_paused(&mut world, paused, issue).await;
    world.poll().await;

    let handed_back = world.issue(issue);
    assert!(labels(&handed_back).contains(&"ready-for-human"));
    assert_eq!(assignees(&handed_back), Vec::<&str>::new());
    assert!(last_comment(&handed_back).contains("The session ended unexpectedly: it broke"));
    assert_eq!(let_go_comments(&handed_back), 0);
}

#[tokio::test]
async fn an_issue_assigned_by_hand_and_cut_through_a_poll_part_way_ends_a_tracking_issue_let_go_silently()
 {
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
    let issue = assigned_by_hand(&world, &["ready-for-agent"]);
    // The last status label the cut removes before letting go.
    let paused = world.store.pause_after(removing(issue, "ready-for-human"));
    world.factory.cycle().await.unwrap();

    poll_while_paused(&mut world, paused, issue).await;
    world.poll().await;

    let cut = world.issue(issue);
    assert_eq!(labels(&cut), ["linewright:designed"]);
    assert_eq!(cut.sub_issues.len(), 1);
    assert_eq!(assignees(&cut), Vec::<&str>::new());
    assert_eq!(let_go_comments(&cut), 0);
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
