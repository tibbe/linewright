use crate::common;

use common::*;
use linewright::domain::{CheckState, Item, Number, ReviewState};
use linewright::factory::Factory;
use linewright::reconcile::{Action, Subject, Work, WorkKind};
use linewright::webhook;

#[tokio::test(flavor = "multi_thread")]
async fn refreshing_an_issue_closed_since_the_poll_cancels_its_run() {
    let mut world = World::new().await;
    let gate = gated(&world, WorkKind::Implement);
    let issue = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.factory.cycle().await.unwrap();

    world.store.close_issue_as_completed(issue).unwrap();
    let actions = world.factory.refresh([Item::Issue(issue)]).await.unwrap();

    assert_eq!(
        actions,
        [Action::Cancel {
            subject: Subject::Issue(issue),
            reason: format!("{issue} was closed"),
        }]
    );
    gate.release();
    world.factory.settle().await;
}

#[tokio::test]
async fn a_review_on_linewrights_pull_request_refreshed_after_its_run_is_addressed() {
    let mut world = World::new().await;
    let (issue, pr) = world.ticket_in_review().await;

    world
        .store
        .review_as(
            "maintainer",
            pr,
            ReviewState::ChangesRequested,
            "Not like this.",
            &[],
        )
        .unwrap();
    let actions = world.factory.refresh([Item::Pull(pr)]).await.unwrap();

    assert_eq!(actions, [Action::Start(Work::Rework { issue, pr })]);
}

#[tokio::test]
async fn a_refresh_decides_on_the_items_it_reads_as_a_poll_of_the_same_store_does() {
    let mut world = World::new().await;
    let issue = world
        .store
        .open_issue("maintainer", "Do another thing", "Please.", &[]);
    let (ticket, pr) = world.ticket_in_review().await;

    world
        .store
        .label_as("maintainer", issue, "ready-for-agent")
        .unwrap();
    let spec = spec_change(&world).await;
    world
        .store
        .review_as("maintainer", pr, ReviewState::Commented, "Rename it.", &[])
        .unwrap();
    let mut poller = Factory::start(
        world.store.clone(),
        world.harness.clone(),
        world.clock.clone(),
        config(&world.dir.path().join("poller")),
    )
    .await
    .unwrap();
    // Stopped, it decides without acting, leaving the store as it found it.
    poller.stopping().cancel();
    let polled = poller.cycle().await.unwrap();
    let refreshed = world
        .factory
        .refresh([Item::Issue(issue), Item::Pull(spec), Item::Pull(pr)])
        .await
        .unwrap();

    assert_eq!(
        polled,
        [
            Action::Start(Work::Rework { issue: ticket, pr }),
            Action::Claim {
                item: Item::Issue(issue)
            },
            Action::Start(Work::DesignIssue { issue }),
            Action::Claim {
                item: Item::Pull(spec)
            },
            Action::Start(Work::DesignSpecChange { pr: spec }),
        ]
    );
    assert_eq!(refreshed, polled);
}

#[tokio::test]
async fn a_refresh_before_the_first_poll_polls() {
    let mut world = World::new().await;
    implement(&world);
    let issue = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    let other = world.store.open_issue(
        "maintainer",
        "Another",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );

    let actions = refresh(&mut world, Item::Issue(issue)).await;

    assert_eq!(world.store.snapshots_taken(), 1);
    assert_eq!(
        actions,
        [
            Action::Claim {
                item: Item::Issue(issue)
            },
            Action::Start(Work::Implement { issue }),
            Action::Claim {
                item: Item::Issue(other)
            },
            Action::Start(Work::Implement { issue: other }),
        ]
    );
    world.factory.settle().await;
}

#[tokio::test]
async fn a_rework_started_by_a_refresh_takes_the_feedback_it_read_as_already_shown() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.poll().await;
    world.harness.on(WorkKind::Rework, |call| {
        commit_in(
            &call.spec.dir,
            &[("CHANGELOG.md", "- thing\n")],
            "Changelog",
        );
        Ok(serde_json::json!({"outcome": "done", "comment": "Added the entry."}).to_string())
    });

    world
        .store
        .comment_as("maintainer", pr, "Add a changelog entry.")
        .unwrap();
    refresh(&mut world, Item::Pull(pr)).await;
    world.factory.settle().await;

    assert_eq!(
        world.harness.prompts(WorkKind::Rework).len(),
        1,
        "one session, not resumed"
    );
}

async fn refresh(world: &mut World, item: Item) -> Vec<Action> {
    world.factory.refresh([item]).await.unwrap()
}

async fn spec_change(world: &World) -> Number {
    world
        .store
        .merge_change(
            "maintainer",
            "Gadgets",
            "Adds gadgets.",
            &[("docs/spec.md", "# Spec\n\nGadgets exist.\n")],
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn a_commit_names_the_open_pull_request_a_run_opened_since_the_poll_whose_head_it_is() {
    let mut world = World::new().await;
    // The poll that started the run that opened it took its snapshot first.
    let (issue, pr) = world.ticket_in_review().await;
    world
        .store
        .set_checks(
            pr,
            vec![check("test", CheckState::Failed, "assertion failed")],
        )
        .await
        .unwrap();
    let head = world.store.snapshot_pull(pr).await.head_sha;

    let actions = world
        .factory
        .refresh([webhook::Subject::Commit(head)])
        .await
        .unwrap();

    assert_eq!(
        actions,
        [Action::Start(Work::FixCi {
            issue,
            pr,
            failing: vec!["test".into()]
        })]
    );
}
