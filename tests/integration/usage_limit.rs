use crate::common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::TimeDelta;
use common::*;
use linewright::clock::Clock;
use linewright::domain::{Login, Number, Timestamp};
use linewright::factory::Factory;
use linewright::harness::{SessionError, UsageLimit};
use linewright::reconcile::{Action, Subject, WorkKind};
use linewright::store::Store;
use linewright::webhook::Pending;
use serde_json::json;

fn advance_past_the_reset(world: &World, minutes: i64) {
    advance_to(world, usage_limit_resets_at() + TimeDelta::minutes(minutes));
}

fn advance_to(world: &World, at: Timestamp) {
    let now = world.clock.now();
    world.clock.advance(at - now);
}

fn info_naming_the_reset(log: &Log, phrase: &str) -> Vec<String> {
    log.lines("INFO")
        .into_iter()
        .filter(|l| l.contains(phrase) && l.contains("2026-10-13 07:50:00"))
        .collect()
}

fn triage(world: &World) {
    world.harness.on(WorkKind::Triage, |_| {
        Ok(json!({"outcome": "ticket", "summary": "Do it.", "type": "feature"}).to_string())
    });
}

fn started(actions: &[Action]) -> Vec<Subject> {
    actions
        .iter()
        .filter_map(|action| {
            if let Action::Start(work) = action {
                Some(work.subject())
            } else {
                None
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    session: String,
    dir: PathBuf,
    followup: Option<String>,
}

type Turns = Arc<Mutex<Vec<Seen>>>;

/// Records the turn `call` in `turns`, returning how many came before it.
fn see(turns: &Turns, call: &Call) -> usize {
    let mut turns = turns.lock().unwrap();
    turns.push(Seen {
        session: call.session.to_string(),
        dir: call.spec.dir.clone(),
        followup: call.followup.map(str::to_string),
    });
    turns.len() - 1
}

type Answer = Result<String, SessionError>;

fn done() -> String {
    json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."}).to_string()
}

/// Has implementing sessions answer with `turns[n]` on their `n`th turn,
/// counting turns across sessions, and finish once those run out. Each
/// commits a file named for its turn first. Returns the turns they took.
fn implement_turns(world: &World, turns: Vec<fn(&Call) -> Answer>) -> Turns {
    let seen = Turns::default();
    world.harness.on(WorkKind::Implement, {
        let seen = seen.clone();
        move |call| {
            let n = see(&seen, call);
            let file = format!("src/turn-{n}.txt");
            commit_in(&call.spec.dir, &[(&file, "work\n")], "Work");
            turns.get(n).map_or_else(|| Ok(done()), |turn| turn(call))
        }
    });
    seen
}

async fn refused() -> (World, Number) {
    let mut world = World::serving().await;
    implement_turns(&world, vec![|_| refused_for_the_usage_limit()]);
    let ticket = open_ticket(&world, "Do the thing");
    world.factory.cycle().await.unwrap();
    assert_eq!(world.clock.next_sleep().await, usage_limit_resets_at());
    (world, ticket)
}

#[tokio::test]
async fn before_the_reset_a_poll_starts_no_run_and_claims_no_issue() {
    let (mut world, ticket) = refused().await;
    triage(&world);
    let fresh = open_ticket(&world, "Do another thing");
    let new = world
        .store
        .open_issue("reporter", "It broke", "How?", &["needs-triage"]);

    let actions = world.factory.cycle().await.unwrap();

    assert_eq!(started(&actions), []);
    assert_eq!(world.issue(fresh).assignees, []);
    assert_eq!(labels(&world.issue(new)), ["needs-triage"]);
    assert!(world.issue(ticket).is_assigned(&Login::new(LINEWRIGHT)));
}

#[tokio::test]
async fn a_restart_forgets_the_limit_and_drops_the_waiting_run() {
    let (mut world, _) = refused().await;

    world.factory = Factory::start(
        world.store.clone(),
        world.harness.clone(),
        world.clock.clone(),
        linewright::factory::FactoryConfig {
            waits_out_usage_limit: true,
            ..config(&world.dir.path().join("work"))
        },
    )
    .await
    .unwrap();
    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 2);
    assert_eq!(world.store.pulls().len(), 1);
}

#[tokio::test]
async fn a_session_refused_part_way_resumes_after_the_reset_in_the_same_clone() {
    let (log, _guard) = Log::capture();
    let mut world = World::serving().await;
    let turns = implement_turns(&world, vec![refused_part_way]);
    let ticket = open_ticket(&world, "Do the thing");
    world.factory.cycle().await.unwrap();

    assert_eq!(world.clock.next_sleep().await, usage_limit_resets_at());
    advance_past_the_reset(&world, 0);
    world.factory.settle().await;

    let turns = turns.lock().unwrap().clone();
    assert_eq!(turns.len(), 2, "{turns:?}");
    assert_eq!(turns[1].session, turns[0].session);
    assert_eq!(turns[1].dir, turns[0].dir);
    assert!(turns[1].followup.is_some(), "resumed, not started");
    assert_eq!(world.store.pulls().len(), 1);
    let branch = format!("{}-do-the-thing", ticket.0);
    for file in ["src/turn-0.txt", "src/turn-1.txt"] {
        let read = world.store.read_file(&branch, file).await.unwrap();
        assert_eq!(read.as_deref(), Some("work\n"), "{file}");
    }
    assert_eq!(world.issue(ticket).comments, []);
    let refused = log.usage_limit_warnings();
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(
        refused[0].contains("issue-"),
        "names its subject: {refused:?}"
    );
    let waits = info_naming_the_reset(&log, "waiting out the usage limit");
    assert_eq!(waits.len(), 1, "{waits:?}");
    assert!(waits[0].contains("issue-"), "names its subject: {waits:?}");
    let resumes = info_naming_the_reset(&log, "resuming the session");
    assert_eq!(resumes.len(), 1, "{resumes:?}");
    assert!(
        resumes[0].contains("issue-"),
        "names its subject: {resumes:?}"
    );
}

#[tokio::test]
async fn cancelling_the_issue_during_the_wait_cancels_the_run_and_keeps_its_branch() {
    let (log, _guard) = Log::capture();
    let mut world = World::serving().await;
    implement_turns(&world, vec![refused_part_way]);
    let ticket = open_ticket(&world, "Do the thing");
    world.factory.cycle().await.unwrap();
    world.clock.next_sleep().await;

    world.store.unlabel_as(ticket, "ready-for-agent").unwrap();
    world.factory.cycle().await.unwrap();
    world.factory.settle().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
    let branch = format!("{}-do-the-thing", ticket.0);
    let read = world
        .store
        .read_file(&branch, "src/turn-0.txt")
        .await
        .unwrap();
    assert_eq!(read.as_deref(), Some("work\n"));
    assert_eq!(world.issue(ticket).comments, []);
    assert_eq!(world.store.pulls(), []);
    let cancelled: Vec<_> = log
        .lines("INFO")
        .into_iter()
        .filter(|l| l.contains("run cancelled"))
        .collect();
    assert_eq!(cancelled.len(), 1, "{cancelled:?}");
}

#[tokio::test]
async fn a_stop_during_the_wait_drops_the_run() {
    let mut world = World::serving().await;
    implement_turns(&world, vec![refused_part_way]);
    let ticket = open_ticket(&world, "Do the thing");
    world.factory.cycle().await.unwrap();
    world.clock.next_sleep().await;

    let World { factory, store, .. } = world;
    factory.stop().await;

    let branch = format!("{}-do-the-thing", ticket.0);
    let read = store.read_file(&branch, "src/turn-0.txt").await.unwrap();
    assert_eq!(read.as_deref(), Some("work\n"), "its branch is kept");
    assert_eq!(store.pulls(), []);
}

#[tokio::test]
async fn once_ends_the_refused_run_without_waiting() {
    let mut world = World::new().await;
    implement_turns(&world, vec![refused_part_way]);
    let ticket = open_ticket(&world, "Do the thing");

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
    assert_eq!(world.issue(ticket).comments, []);
    assert!(world.issue(ticket).is_assigned(&Login::new(LINEWRIGHT)));
}

struct Served {
    world: World,
    serving: Serving,
}

impl Served {
    async fn start(world: World) -> Served {
        let factory = Factory::start(
            world.store.clone(),
            world.harness.clone(),
            world.clock.clone(),
            linewright::factory::FactoryConfig {
                waits_out_usage_limit: true,
                ..config(&world.dir.path().join("work"))
            },
        )
        .await
        .unwrap();
        let serving = Serving::start(factory, Duration::from_millis(10), Pending::default());
        Served { world, serving }
    }

    fn usage_limit(&self) -> Option<UsageLimit> {
        self.serving
            .served
            .borrow()
            .as_ref()?
            .view()
            .memory
            .usage_limit
    }

    async fn stop(self) -> World {
        self.serving.stop().await;
        self.world
    }
}

fn the_usage_limit() -> UsageLimit {
    UsageLimit {
        resets_at: usage_limit_resets_at(),
    }
}

#[tokio::test]
async fn while_serve_waits_out_the_limit_its_view_holds_it_until_the_reset() {
    let world = World::new().await;
    world
        .harness
        .on(WorkKind::Implement, |_| refused_for_the_usage_limit());
    world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    let served = Served::start(world).await;
    eventually(|| served.usage_limit() == Some(the_usage_limit())).await;

    implement(&served.world);
    advance_past_the_reset(&served.world, 1);

    eventually(|| served.usage_limit().is_none()).await;
    served.stop().await;
}
