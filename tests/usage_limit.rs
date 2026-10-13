//! Under the usage limit `serve` starts no session until the limit resets,
//! end to end at the operator's level: a runtime that refuses sessions and a
//! fake clock in, the issues, sessions, log and the limit `status` reads
//! out, against the local store (`docs/specs/operation.md`, "Usage limit").

mod common;

use std::path::PathBuf;
use std::time::Duration;

use chrono::TimeDelta;
use common::*;
use linewright::clock::Clock;
use linewright::domain::{Item, Login, Number};
use linewright::factory::Factory;
use linewright::marker::{Stage, marked};
use linewright::reconcile::WorkKind;
use linewright::runtime::{SessionError, UsageLimit};
use linewright::usage_limit;
use linewright::webhook::Pending;
use serde_json::json;

/// A world whose implementing sessions are refused for the usage limit
/// that resets at 7:50 UTC on 13 October, after one poll on a ticket
/// (returned) that its run was refused on.
async fn refused() -> (World, Number) {
    let mut world = World::new().await;
    world
        .runtime
        .on(WorkKind::Implement, |_| refused_for_the_usage_limit());
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    world.poll().await;
    (world, ticket)
}

/// Advances the fake clock to `minutes` past [`usage_limit_resets_at`].
fn advance_past_the_reset(world: &World, minutes: i64) {
    let now = world.clock.now();
    world
        .clock
        .advance(usage_limit_resets_at() - now + TimeDelta::minutes(minutes));
}

/// The lines logged at `INFO` that say `phrase` and name the reset in
/// [`usage_limit_resets_at`].
fn info_naming_the_reset(log: &Log, phrase: &str) -> Vec<String> {
    log.lines("INFO")
        .into_iter()
        .filter(|l| l.contains(phrase) && l.contains("2026-10-13 07:50:00"))
        .collect()
}

fn triage(world: &World) {
    world.runtime.on(WorkKind::Triage, |_| {
        json(json!({"outcome": "ticket", "summary": "Do it.", "type": "feature"}))
    });
}

#[tokio::test]
async fn before_the_reset_a_poll_starts_no_run_and_claims_no_issue() {
    let (mut world, ticket) = refused().await;
    implement(&world);
    triage(&world);
    let fresh = world.store.open_issue(
        "maintainer",
        "Do another thing",
        "Please.",
        &["ready-for-agent"],
    );
    let new = world
        .store
        .open_issue("reporter", "It broke", "How?", &["needs-triage"]);

    world.poll().await;

    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 1);
    assert!(world.runtime.prompts(WorkKind::Triage).is_empty());
    assert!(world.issue(fresh).assignees.is_empty());
    assert_eq!(labels(&world.issue(new)), ["needs-triage"]);
    assert!(world.issue(ticket).is_assigned(&Login::new(MACHINE)));
}

#[tokio::test]
async fn before_the_reset_a_refresh_starts_no_run_and_claims_no_issue() {
    let (mut world, _) = refused().await;
    implement(&world);
    let fresh = world.store.open_issue(
        "maintainer",
        "Do another thing",
        "Please.",
        &["ready-for-agent"],
    );

    let actions = world.factory.refresh([Item::Issue(fresh)]).await.unwrap();
    world.factory.settle().await;

    assert_eq!(actions, []);
    assert!(world.issue(fresh).assignees.is_empty());
}

#[tokio::test]
async fn before_the_reset_actions_that_start_no_session_are_still_taken() {
    let (mut world, _) = refused().await;
    let asked = world
        .store
        .open_issue("reporter", "It broke", "How?", &["needs-info"]);
    world
        .store
        .comment_as(MACHINE, asked, &marked(Stage::Triage, "Which version?"))
        .unwrap();
    world
        .store
        .comment_as("reporter", asked, "Version 3.")
        .unwrap();

    world.poll().await;

    assert_eq!(labels(&world.issue(asked)), ["needs-triage"]);
    assert!(world.runtime.prompts(WorkKind::Triage).is_empty());
}

#[tokio::test]
async fn the_first_poll_after_the_reset_starts_the_refused_ticket_again() {
    let (log, _guard) = Log::capture();
    let (mut world, _) = refused().await;
    implement(&world);
    world.clock.advance(TimeDelta::days(11));
    world.poll().await;
    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 1);

    advance_past_the_reset(&world, 1);
    world.poll().await;

    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 2);
    assert_eq!(world.store.pulls().len(), 1);
    let in_force = info_naming_the_reset(&log, "usage limit in force");
    assert_eq!(in_force.len(), 1, "{in_force:?}");
    let lifted = info_naming_the_reset(&log, "usage limit reset; sessions start again");
    assert_eq!(lifted.len(), 1, "{lifted:?}");
}

#[tokio::test]
async fn when_several_runs_are_refused_the_latest_reset_holds() {
    let mut world = World::new().await;
    world.store.open_issue(
        "maintainer",
        "Do one thing",
        "Please.",
        &["ready-for-agent"],
    );
    let later = world.store.open_issue(
        "maintainer",
        "Do another thing",
        "Please.",
        &["ready-for-agent"],
    );
    // The second ticket's limit resets an hour after the first's.
    let later_file = format!("issue-{}", later.0);
    world.runtime.on(WorkKind::Implement, move |call| {
        let hours = if subject_file(call) == later_file {
            1
        } else {
            0
        };
        Err(SessionError::UsageLimit(UsageLimit {
            resets_at: usage_limit_resets_at() + TimeDelta::hours(hours),
        }))
    });
    world.poll().await;
    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 2);
    implement(&world);

    advance_past_the_reset(&world, 30);
    world.poll().await;
    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 2);

    world.clock.advance(TimeDelta::hours(1));
    world.poll().await;
    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 4);
}

/// Ticks on the hour; the fake clock starts at 12:00.
const HOURLY: &str = "---\non:\n  schedule: \"0 * * * *\"\n---\nLook around.\n";

#[tokio::test]
async fn a_workflow_due_under_the_limit_waits_and_runs_at_the_first_poll_after_the_reset() {
    let mut world = World::new().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(".linewright/workflows/hourly.md", HOURLY)],
            "Add a workflow",
        )
        .await
        .unwrap();
    world
        .runtime
        .on_workflow(|_| json(json!({"summary": "Nothing needs doing.", "issues": []})));
    world.poll().await;
    world
        .runtime
        .on(WorkKind::Implement, |_| refused_for_the_usage_limit());
    world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    world.poll().await;
    let path = world.dir.path().join("work/workflow-runs.json");
    let last_runs = || std::fs::read_to_string(&path);
    let before = last_runs().unwrap();

    world.clock.advance(TimeDelta::hours(2));
    world.poll().await;
    assert!(world.runtime.workflow_prompts().is_empty());
    assert_eq!(last_runs().unwrap(), before, "not recorded as run");

    advance_past_the_reset(&world, 1);
    world.poll().await;

    assert_eq!(world.runtime.workflow_prompts().len(), 1);
}

#[tokio::test]
async fn a_restarted_factory_holds_no_limit() {
    let (mut world, _) = refused().await;
    implement(&world);

    world.factory = Factory::start(
        world.store.clone(),
        world.runtime.clone(),
        world.clock.clone(),
        config(&work_dir(&world)),
    )
    .await
    .unwrap();
    world.poll().await;

    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 2);
    assert_eq!(world.store.pulls().len(), 1);
}

/// A world's factory serving, polling every few milliseconds.
struct Served {
    world: World,
    serving: Serving,
}

impl Served {
    /// Serves a fresh world's factory, as `serve` does, in place of `world`'s.
    async fn start(world: World) -> Served {
        let factory = Factory::start(
            world.store.clone(),
            world.runtime.clone(),
            world.clock.clone(),
            config(&work_dir(&world)),
        )
        .await
        .unwrap();
        let serving = Serving::start(factory, Duration::from_millis(10), Pending::default());
        Served { world, serving }
    }

    /// The usage limit `status` reads.
    fn usage_limit(&self) -> Option<UsageLimit> {
        usage_limit::load(&work_dir(&self.world))
    }

    async fn stop(self) -> World {
        self.serving.stop().await;
        self.world
    }
}

fn work_dir(world: &World) -> PathBuf {
    world.dir.path().join("work")
}

fn the_usage_limit() -> Option<UsageLimit> {
    Some(UsageLimit {
        resets_at: usage_limit_resets_at(),
    })
}

#[tokio::test]
async fn while_serve_waits_out_the_limit_status_reads_it_until_the_reset() {
    let world = World::new().await;
    world
        .runtime
        .on(WorkKind::Implement, |_| refused_for_the_usage_limit());
    world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    let served = Served::start(world).await;
    eventually(|| served.usage_limit() == the_usage_limit()).await;

    implement(&served.world);
    advance_past_the_reset(&served.world, 1);

    eventually(|| served.usage_limit().is_none()).await;
    served.stop().await;
}

#[tokio::test]
async fn serve_starts_with_no_usage_limit_for_status_to_read() {
    let world = World::new().await;
    usage_limit::record(&work_dir(&world), the_usage_limit()).unwrap();

    let served = Served::start(world).await;
    let polled = served.world.store.snapshots_taken();
    eventually(|| served.world.store.snapshots_taken() > polled).await;

    assert_eq!(served.usage_limit(), None);
    served.stop().await;
}

#[tokio::test]
async fn once_leaves_no_usage_limit_for_status_to_read() {
    let (world, _) = refused().await;

    assert_eq!(usage_limit::load(&work_dir(&world)), None);
}

#[tokio::test]
async fn a_workflow_run_by_hand_leaves_no_usage_limit_for_status_to_read() {
    let world = World::new().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(".linewright/workflows/hourly.md", HOURLY)],
            "Add a workflow",
        )
        .await
        .unwrap();
    world.runtime.on_workflow(|_| refused_for_the_usage_limit());

    world
        .factory
        .run_workflow(&"hourly".parse().unwrap())
        .await
        .unwrap();

    assert_eq!(usage_limit::load(&work_dir(&world)), None);
}
