//! Between polls the factory keeps its snapshot, and a refresh reads the
//! issues and pull requests a delivery names into it again and reconciles,
//! end to end against the local store and a scripted runtime
//! (`docs/specs/architecture.md`, "The store is an interface" and "Webhook
//! deliveries are hints, not state").

mod common;

use std::sync::Mutex;
use std::sync::mpsc;

use common::*;
use linewright::domain::{CheckState, Item, Number, ReviewState};
use linewright::factory::Factory;
use linewright::reconcile::{Action, Subject, Work, WorkKind};
use linewright::webhook;

#[tokio::test]
async fn an_issue_labeled_ready_for_agent_and_refreshed_after_a_poll_is_claimed_and_implemented() {
    let mut world = World::new().await;
    implement(&world);
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    world.poll().await;
    let polled = world.store.snapshots_taken();

    world.store.label_as(issue, "ready-for-agent").unwrap();
    let actions = world.factory.refresh([Item::Issue(issue)]).await.unwrap();

    assert_eq!(
        actions,
        [
            Action::Claim { issue },
            Action::Start(Work::Implement { issue })
        ]
    );
    assert_eq!(world.store.snapshots_taken(), polled);
}

#[tokio::test(flavor = "multi_thread")]
async fn refreshing_an_issue_closed_since_the_poll_cancels_its_run() {
    let mut world = World::new().await;
    let release = gated(&world, WorkKind::Implement);
    let issue = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    world.factory.cycle().await.unwrap();

    world.store.close_issue_as(issue).unwrap();
    let actions = world.factory.refresh([Item::Issue(issue)]).await.unwrap();

    assert_eq!(
        actions,
        [Action::Cancel {
            subject: Subject::Issue(issue),
            reason: format!("{issue} was closed"),
        }]
    );
    release.send(()).unwrap();
    world.factory.settle().await;
}

#[tokio::test]
async fn a_ticket_waiting_for_a_persons_pull_request_is_implemented_once_a_refresh_reads_it_closed()
{
    let mut world = World::new().await;
    implement(&world);
    let issue = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    let theirs = world.persons_pull_closing(issue).await;
    world.poll().await;

    world.store.close_pull_as(theirs).unwrap();
    let actions = world.factory.refresh([Item::Pull(theirs)]).await.unwrap();

    assert_eq!(
        actions,
        [
            Action::Claim { issue },
            Action::Start(Work::Implement { issue })
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_a_persons_pull_request_opened_since_the_poll_closes_waits_for_it_once_refreshed()
{
    let mut world = World::with(|config| config.max_runs = 1).await;
    implement(&world);
    let release = gated(&world, WorkKind::Triage);
    world
        .store
        .open_issue("reporter", "It broke", "It crashes.", &["needs-triage"]);
    let issue = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    // The triage run takes the only free run, so the ticket waits for one.
    world.factory.cycle().await.unwrap();
    let theirs = world.persons_pull_closing(issue).await;
    release.send(()).unwrap();
    world.factory.settle().await;

    let actions = world.factory.refresh([Item::Pull(theirs)]).await.unwrap();

    assert!(
        !actions.contains(&Action::Start(Work::Implement { issue })),
        "{actions:?}"
    );
}

#[tokio::test]
async fn refreshing_linewrights_own_pull_request_reads_not_the_issue_it_closes() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.poll().await;
    let read = world.store.items_read();

    world.factory.refresh([Item::Pull(pr)]).await.unwrap();

    // The pull request, as Linewright's and as a merged one.
    assert_eq!(world.store.items_read(), read + 2);
}

#[tokio::test]
async fn a_spec_change_merged_since_the_poll_and_refreshed_is_cut_into_tickets() {
    let mut world = World::new().await;
    world.poll().await;

    let pr = spec_change(&world).await;
    let actions = world.factory.refresh([Item::Pull(pr)]).await.unwrap();

    assert_eq!(actions, [Action::Start(Work::Tickets { pr })]);
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

    world.store.label_as(issue, "ready-for-agent").unwrap();
    let spec = spec_change(&world).await;
    world
        .store
        .review_as("maintainer", pr, ReviewState::Commented, "Rename it.", &[])
        .unwrap();
    let mut poller = Factory::start(
        world.store.clone(),
        world.runtime.clone(),
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
            Action::Start(Work::Tickets { pr: spec }),
            Action::Claim { issue },
            Action::Start(Work::Implement { issue }),
        ]
    );
    assert_eq!(refreshed, polled);
}

#[tokio::test]
async fn refreshing_an_item_again_or_one_that_did_not_change_takes_no_actions() {
    let mut world = World::new().await;
    implement(&world);
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    let unchanged =
        world
            .store
            .open_issue("maintainer", "Leave it", "Later.", &["ready-for-human"]);
    world.poll().await;

    assert_eq!(refresh(&mut world, Item::Issue(unchanged)).await, []);
    world.store.label_as(issue, "ready-for-agent").unwrap();
    refresh(&mut world, Item::Issue(issue)).await;
    world.factory.settle().await;

    assert_eq!(refresh(&mut world, Item::Issue(issue)).await, []);
    assert_eq!(refresh(&mut world, Item::Issue(unchanged)).await, []);
}

#[tokio::test]
async fn a_refresh_after_a_poll_that_handed_an_issue_back_does_not_hand_it_back_again() {
    let mut world = World::new().await;
    let (issue, pr) = world.ticket_in_review().await;
    let unchanged =
        world
            .store
            .open_issue("maintainer", "Leave it", "Later.", &["ready-for-human"]);
    world.store.close_pull_as(pr).unwrap();
    let polled = world.factory.cycle().await.unwrap();
    assert!(matches!(polled[..], [Action::HandBack { .. }]));
    let comments = world.issue(issue).comments.len();

    assert_eq!(refresh(&mut world, Item::Issue(unchanged)).await, []);
    assert_eq!(world.issue(issue).comments.len(), comments);
}

#[tokio::test]
async fn a_poll_reads_what_changed_since_whether_refreshed_or_not() {
    let mut world = World::new().await;
    implement(&world);
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    let unchanged =
        world
            .store
            .open_issue("maintainer", "Leave it", "Later.", &["ready-for-human"]);
    world.poll().await;

    world.store.label_as(issue, "ready-for-agent").unwrap();
    assert_eq!(refresh(&mut world, Item::Issue(unchanged)).await, []);
    let actions = world.factory.cycle().await.unwrap();
    world.factory.settle().await;

    assert_eq!(
        actions,
        [
            Action::Claim { issue },
            Action::Start(Work::Implement { issue })
        ]
    );
}

#[tokio::test]
async fn a_refresh_before_the_first_poll_polls() {
    let mut world = World::new().await;
    implement(&world);
    let issue = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    let other = world
        .store
        .open_issue("maintainer", "Another", "Please.", &["ready-for-agent"]);

    let actions = refresh(&mut world, Item::Issue(issue)).await;

    assert_eq!(world.store.snapshots_taken(), 1);
    assert_eq!(
        actions,
        [
            Action::Claim { issue },
            Action::Start(Work::Implement { issue }),
            Action::Claim { issue: other },
            Action::Start(Work::Implement { issue: other }),
        ]
    );
    world.factory.settle().await;
}

#[tokio::test]
async fn a_spec_change_cut_since_the_poll_is_not_cut_again_by_a_refresh() {
    let mut world = World::new().await;
    world.runtime.on(WorkKind::Tickets, |_| {
        json(serde_json::json!({
            "tickets": [{"id": "store", "title": "Store gadgets", "body": "Gadgets persist.", "blocked_by": []}],
            "comment": "One slice."
        }))
    });
    let unchanged =
        world
            .store
            .open_issue("maintainer", "Leave it", "Later.", &["ready-for-human"]);
    world.poll().await;
    let pr = spec_change(&world).await;
    refresh(&mut world, Item::Pull(pr)).await;
    world.factory.settle().await;

    assert_eq!(refresh(&mut world, Item::Issue(unchanged)).await, []);
}

#[tokio::test]
async fn a_rework_started_by_a_refresh_takes_the_feedback_it_read_as_already_shown() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.poll().await;
    world.runtime.on(WorkKind::Rework, |call| {
        commit_in(
            &call.spec.dir,
            &[("CHANGELOG.md", "- thing\n")],
            "Changelog",
        );
        json(serde_json::json!({"outcome": "done", "comment": "Added the entry."}))
    });

    world
        .store
        .comment_as("maintainer", pr, "Add a changelog entry.")
        .unwrap();
    refresh(&mut world, Item::Pull(pr)).await;
    world.factory.settle().await;

    assert_eq!(
        world.runtime.prompts(WorkKind::Rework).len(),
        1,
        "one session, not resumed"
    );
}

#[tokio::test]
async fn a_refresh_after_a_rework_run_ends_does_not_start_it_again() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    let unchanged =
        world
            .store
            .open_issue("maintainer", "Leave it", "Later.", &["ready-for-human"]);
    world.runtime.on(WorkKind::Rework, |call| {
        commit_in(
            &call.spec.dir,
            &[("CHANGELOG.md", "- thing\n")],
            "Changelog",
        );
        json(serde_json::json!({"outcome": "done", "comment": "Added the entry."}))
    });
    world
        .store
        .comment_as("maintainer", pr, "Add a changelog entry.")
        .unwrap();
    world.poll().await;

    assert_eq!(refresh(&mut world, Item::Issue(unchanged)).await, []);
}

#[tokio::test]
async fn a_refresh_naming_items_the_store_does_not_have_reads_the_others() {
    let mut world = World::new().await;
    implement(&world);
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    world.poll().await;

    world.store.label_as(issue, "ready-for-agent").unwrap();
    let actions = world
        .factory
        .refresh([
            Item::Issue(Number(99)),
            Item::Pull(Number(98)),
            Item::Issue(issue),
        ])
        .await
        .unwrap();
    world.factory.settle().await;

    assert_eq!(
        actions,
        [
            Action::Claim { issue },
            Action::Start(Work::Implement { issue })
        ]
    );
}

async fn refresh(world: &mut World, item: Item) -> Vec<Action> {
    world.factory.refresh([item]).await.unwrap()
}

/// Sessions of `kind` wait until the returned sender sends, then fail.
fn gated(world: &World, kind: WorkKind) -> mpsc::Sender<()> {
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    world.runtime.on(kind, move |_| {
        released.lock().unwrap().recv().unwrap();
        Err(linewright::runtime::SessionError::Failed {
            reason: "released".into(),
            log_tail: String::new(),
        })
    });
    release
}

/// A person's pull request changing the spec, merged.
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

#[tokio::test]
async fn another_commit_or_a_push_to_another_branch_reads_nothing_and_decides_nothing() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.poll().await;
    world
        .store
        .set_checks(
            pr,
            vec![check("test", CheckState::Failed, "assertion failed")],
        )
        .await
        .unwrap();
    let read = world.store.items_read();
    // The kept snapshot has no checks on it, and a reconcile past the grace
    // period would mark it ready.
    world.clock.advance(chrono::TimeDelta::minutes(6));

    let actions = world
        .factory
        .refresh([
            webhook::Subject::Commit("0123456789abcdef0123456789abcdef01234567".into()),
            webhook::Subject::Push(world.store.snapshot_pull(pr).await.head_ref),
        ])
        .await
        .unwrap();

    assert_eq!(actions, []);
    assert_eq!(world.store.items_read(), read);
}

#[tokio::test]
async fn a_push_to_the_default_branch_names_every_open_pull_request_of_linewright() {
    let mut world = World::new().await;
    let (first_issue, first) = world.ticket_in_review().await;
    let (second_issue, second) = world.ticket_in_review().await;
    world.poll().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[("src/other.txt", "other\n")],
            "Other",
        )
        .await
        .unwrap();

    let actions = world
        .factory
        .refresh([webhook::Subject::Push("main".into())])
        .await
        .unwrap();
    world.factory.settle().await;

    assert_eq!(
        actions,
        [
            Action::Start(Work::Update {
                issue: first_issue,
                pr: first
            }),
            Action::Start(Work::Update {
                issue: second_issue,
                pr: second
            }),
        ]
    );
}
