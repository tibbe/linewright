use crate::common;

use std::collections::BTreeSet;
use std::panic::Location;

use common::*;
use linewright::domain::Number;
use linewright::git::{self, CommitId};
use linewright::input::Input;
use linewright::runs::Runs;
use linewright::spec_gap;
use linewright::store::{API_VERSION, NewIssue, RecordedItem, Resource, Store};

fn runs(world: &World) -> Runs {
    Runs::new(&world.dir.path().join("work"))
}

/// The input the run `id` recorded, if any.
#[track_caller]
fn input_of(world: &World, id: &str) -> Option<Input> {
    runs(world).read_input(&id.parse().unwrap()).unwrap()
}

/// The head of `branch` in `world`'s store.
#[track_caller]
fn head_of<'a>(world: &'a World, branch: &str) -> impl Future<Output = CommitId> + 'a {
    let caller = Location::caller();
    let rev = format!("refs/heads/{branch}");
    async move {
        let head = git::git(&world.store.repository(), &["rev-parse", &rev])
            .await
            .unwrap_or_else(|e| panic!("reading {rev} at {caller}: {e:#}"));
        CommitId::from(head.trim())
    }
}

/// The bodies of the comments recorded for `item`.
#[track_caller]
fn comment_bodies(item: &RecordedItem) -> Vec<&str> {
    item.resource(Resource::Comments)
        .and_then(|comments| comments.as_array())
        .expect("the comments are recorded as a list")
        .iter()
        .map(|comment| comment["body"].as_str().expect("a comment has a body"))
        .collect()
}

#[test_log::test(tokio::test)]
async fn a_design_run_keeps_its_item_parent_spec_change_and_open_spec_gap_as_they_stood_at_its_start()
 {
    let mut world = World::new().await;
    let cut = world.spec_change_cut_into_tickets().await;
    let ticket = cut.tickets[0];
    let spec_change = world.issue(ticket).parent.unwrap().spec_change.unwrap();
    let gap = world
        .store
        .create_issue(NewIssue {
            title: "Should gadgets expire?".into(),
            body: spec_gap::body("The spec does not say.", ticket),
            labels: vec!["ready-for-human".into()],
        })
        .await
        .unwrap();
    world
        .store
        .comment_as("maintainer", ticket, "Written before.")
        .unwrap();
    world
        .store
        .remove_label(ticket, "linewright:designed")
        .await
        .unwrap();
    design_issues(&world);
    let store = world.store.clone();
    world.store.before_recording(move || {
        for item in [ticket, cut.tracking, spec_change, gap] {
            store
                .comment_as("maintainer", item, "Written after.")
                .unwrap();
        }
    });

    world.poll().await;

    let input = input_of(&world, &format!("{}/design-1", ticket.0)).unwrap();
    assert_eq!(input.api_version, API_VERSION);
    let numbers: BTreeSet<Number> = input.items.iter().map(|item| item.number).collect();
    assert_eq!(
        numbers,
        BTreeSet::from([ticket, cut.tracking, spec_change, gap])
    );
    for item in &input.items {
        let n = item.number.0;
        assert_eq!(item.resource(Resource::Issue).unwrap()["number"], n);
        assert!(
            !comment_bodies(item).contains(&"Written after."),
            "#{n}: {:?}",
            comment_bodies(item)
        );
    }
    let recorded_ticket = input.items.iter().find(|i| i.number == ticket).unwrap();
    assert_eq!(comment_bodies(recorded_ticket), ["Written before."]);
    let parent = recorded_ticket.resource(Resource::Parent).unwrap();
    assert_eq!(parent["number"], cut.tracking.0);
    let recorded_change = input
        .items
        .iter()
        .find(|i| i.number == spec_change)
        .unwrap();
    assert!(recorded_change.resource(Resource::Pull).is_some());
    assert_eq!(
        input.heads.keys().collect::<Vec<_>>(),
        ["main"],
        "{:?}",
        input.heads
    );
}

#[test_log::test(tokio::test)]
async fn an_implement_run_on_a_branch_with_earlier_work_records_both_heads() {
    let mut world = World::new().await;
    let (ticket, branch) = world.ticket_handed_back_with_its_branch().await;
    relabel_ready_for_agent(&world, ticket);
    world.designed_on_the_issue(ticket).await;
    let main = head_of(&world, "main").await;
    let earlier = head_of(&world, &branch).await;

    world.poll().await;

    let input = input_of(&world, &format!("{}/implement-1", ticket.0)).unwrap();
    assert_eq!(
        input.heads.into_iter().collect::<Vec<_>>(),
        [(branch, earlier), ("main".to_string(), main)]
    );
}

#[test_log::test(tokio::test)]
async fn an_automations_runs_record_no_input() {
    let mut world = World::new().await;

    world.hourly_by_schedule_then_by_hand().await;

    for id in ["hourly/1", "hourly/2"] {
        assert!(
            runs(&world).read_run(&id.parse().unwrap()).is_some(),
            "{id}"
        );
        assert_eq!(input_of(&world, id), None, "{id}");
    }
}

#[test_log::test(tokio::test)]
async fn a_run_whose_recording_fails_does_its_work_without_input() {
    let mut world = World::new().await;
    implement_gadgets(&world);
    let ticket = open_designed_gadgets(&world);
    world.store.fail_recording("GitHub is down");

    world.poll().await;

    assert_eq!(world.store.pulls().len(), 1);
    assert_eq!(input_of(&world, &format!("{}/implement-1", ticket.0)), None);
}
