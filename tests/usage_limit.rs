//! Under `serve` a session refused for the usage limit waits for the reset
//! and carries on, and no session starts until then, end to end at the
//! operator's level: a runtime that refuses sessions and a fake clock in,
//! the issues, sessions, log and the limit in `serve`'s view out, against the
//! local store (`docs/specs/operation.md`, "Usage limit").

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::TimeDelta;
use common::*;
use linewright::clock::Clock;
use linewright::domain::{Item, Login, Number, Timestamp};
use linewright::factory::Factory;
use linewright::marker::{Stage, marked};
use linewright::reconcile::{Action, Subject, WorkKind};
use linewright::runtime::{SessionError, UsageLimit};
use linewright::store::Store;
use linewright::webhook::Pending;
use serde_json::json;

/// Advances the fake clock to `minutes` past [`usage_limit_resets_at`].
fn advance_past_the_reset(world: &World, minutes: i64) {
    advance_to(world, usage_limit_resets_at() + TimeDelta::minutes(minutes));
}

/// Advances the fake clock to `at`.
fn advance_to(world: &World, at: Timestamp) {
    let now = world.clock.now();
    world.clock.advance(at - now);
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

fn open_ticket(world: &World, title: &str) -> Number {
    world
        .store
        .open_issue("maintainer", title, "Please.", &["ready-for-agent"])
}

/// The subjects of the runs `actions` start.
fn started(actions: &[Action]) -> Vec<Subject> {
    actions
        .iter()
        .filter_map(|action| match action {
            Action::Start(work) => Some(work.subject()),
            _ => None,
        })
        .collect()
}

/// One turn of a session, as its script saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    session: String,
    dir: PathBuf,
    followup: Option<String>,
    /// The time the session had left as the turn began.
    left: Duration,
}

type Turns = Arc<Mutex<Vec<Seen>>>;

/// Records the turn `call` in `turns`, returning how many came before it.
fn see(turns: &Turns, call: &Call) -> usize {
    let mut turns = turns.lock().unwrap();
    turns.push(Seen {
        session: call.session.to_string(),
        dir: call.spec.dir.clone(),
        followup: call.followup.map(str::to_string),
        left: call.spec.deadline - tokio::time::Instant::now(),
    });
    turns.len() - 1
}

/// A scripted turn's end.
type Answer = Result<String, SessionError>;

fn done() -> Answer {
    json(json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."}))
}

/// Has implementing sessions answer with `turns[n]` on their `n`th turn,
/// counting turns across sessions, and finish once those run out. Each
/// commits a file named for its turn first. Returns the turns they took.
fn implement_turns(world: &World, turns: Vec<fn(&Call) -> Answer>) -> Turns {
    let seen = Turns::default();
    world.runtime.on(WorkKind::Implement, {
        let seen = seen.clone();
        move |call| {
            let n = see(&seen, call);
            let file = format!("src/turn-{n}.txt");
            commit_in(&call.spec.dir, &[(&file, "work\n")], "Work");
            turns.get(n).map_or_else(done, |turn| turn(call))
        }
    });
    seen
}

/// A serving world with a ticket (returned) whose implementing session is
/// refused for the usage limit that resets at [`usage_limit_resets_at`]
/// before its first turn began, and finishes after. Its run is waiting.
async fn refused() -> (World, Number) {
    let mut world = World::serving().await;
    implement_turns(&world, vec![|_| refused_for_the_usage_limit()]);
    let ticket = open_ticket(&world, "Do the thing");
    world.factory.cycle().await.unwrap();
    assert_eq!(world.clock.next_sleep().await, usage_limit_resets_at());
    (world, ticket)
}

// No session starts until the limit resets.

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
    assert!(world.issue(fresh).assignees.is_empty());
    assert_eq!(labels(&world.issue(new)), ["needs-triage"]);
    assert!(world.issue(ticket).is_assigned(&Login::new(LINEWRIGHT)));
}

#[tokio::test]
async fn before_the_reset_a_refresh_starts_no_run_and_claims_no_issue() {
    let (mut world, _) = refused().await;
    let fresh = open_ticket(&world, "Do another thing");

    let actions = world.factory.refresh([Item::Issue(fresh)]).await.unwrap();

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
        .comment_as(LINEWRIGHT, asked, &marked(Stage::Triage, "Which version?"))
        .unwrap();
    world
        .store
        .comment_as("reporter", asked, "Version 3.")
        .unwrap();

    world.factory.cycle().await.unwrap();

    assert_eq!(labels(&world.issue(asked)), ["needs-triage"]);
    assert!(world.runtime.prompts(WorkKind::Triage).is_empty());
}

#[tokio::test]
async fn the_first_poll_after_the_reset_starts_work_again() {
    let (log, _guard) = Log::capture();
    let (mut world, _) = refused().await;
    let fresh = open_ticket(&world, "Do another thing");
    world.factory.cycle().await.unwrap();

    advance_past_the_reset(&world, 1);
    world.factory.settle().await;
    let actions = world.factory.cycle().await.unwrap();
    world.factory.settle().await;

    assert_eq!(started(&actions), [Subject::Issue(fresh)]);
    assert_eq!(world.store.pulls().len(), 2);
    let in_force = info_naming_the_reset(&log, "usage limit in force");
    assert_eq!(in_force.len(), 1, "{in_force:?}");
    let lifted = info_naming_the_reset(&log, "usage limit reset; sessions start again");
    assert_eq!(lifted.len(), 1, "{lifted:?}");
}

#[tokio::test]
async fn when_several_runs_are_refused_the_latest_reset_holds() {
    let mut world = World::serving().await;
    // The second session's limit resets an hour after the first's.
    implement_turns(
        &world,
        vec![|_| refused_for_the_usage_limit(), |_| {
            refused_until(usage_limit_resets_at() + TimeDelta::hours(1), None)
        }],
    );
    open_ticket(&world, "Do one thing");
    open_ticket(&world, "Do another thing");
    world.factory.cycle().await.unwrap();
    world.clock.next_sleep().await;
    world.clock.next_sleep().await;
    let third = open_ticket(&world, "Do a third thing");

    advance_past_the_reset(&world, 30);
    world.factory.next_finished().await;
    let held = world.factory.cycle().await.unwrap();
    world.clock.advance(TimeDelta::hours(1));
    world.factory.settle().await;
    let lifted = world.factory.cycle().await.unwrap();

    assert_eq!(started(&held), []);
    assert_eq!(started(&lifted), [Subject::Issue(third)]);
}

/// Ticks on the hour; the fake clock starts at 12:00.
const HOURLY: &str = "---\non:\n  schedule: \"0 * * * *\"\n---\nLook around.\n";

async fn with_hourly_workflow(world: &World) {
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
}

#[tokio::test]
async fn a_workflow_due_under_the_limit_waits_and_runs_at_the_first_poll_after_the_reset() {
    let mut world = World::serving().await;
    with_hourly_workflow(&world).await;
    world
        .runtime
        .on_workflow(|_| json(json!({"summary": "Nothing needs doing.", "issues": []})));
    world.poll().await;
    implement_turns(&world, vec![|_| refused_for_the_usage_limit()]);
    open_ticket(&world, "Do the thing");
    world.factory.cycle().await.unwrap();
    world.clock.next_sleep().await;
    let path = world.dir.path().join("work/workflow-runs.json");
    let last_runs = || std::fs::read_to_string(&path);
    let before = last_runs().unwrap();

    world.clock.advance(TimeDelta::hours(2));
    world.factory.cycle().await.unwrap();
    assert!(world.runtime.workflow_prompts().is_empty());
    assert_eq!(last_runs().unwrap(), before, "not recorded as run");

    advance_past_the_reset(&world, 1);
    world.factory.settle().await;
    world.poll().await;

    assert_eq!(world.runtime.workflow_prompts().len(), 1);
}

#[tokio::test]
async fn a_restart_forgets_the_limit_and_drops_the_waiting_run() {
    let (mut world, _) = refused().await;

    world.factory = Factory::start(
        world.store.clone(),
        world.runtime.clone(),
        world.clock.clone(),
        linewright::factory::FactoryConfig {
            waits_out_usage_limit: true,
            ..config(&world.dir.path().join("work"))
        },
    )
    .await
    .unwrap();
    world.poll().await;

    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 2);
    assert_eq!(world.store.pulls().len(), 1);
}

// The refused session waits out the limit and carries on.

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
    assert!(world.issue(ticket).comments.is_empty());
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
async fn a_session_refused_on_its_first_turn_takes_it_again_in_the_same_clone() {
    let (log, _guard) = Log::capture();
    let mut world = World::serving().await;
    let turns = implement_turns(&world, vec![|_| refused_for_the_usage_limit()]);
    open_ticket(&world, "Do the thing");
    world.factory.cycle().await.unwrap();
    world.clock.next_sleep().await;

    advance_past_the_reset(&world, 0);
    world.factory.settle().await;

    let turns = turns.lock().unwrap().clone();
    assert_eq!(turns.len(), 2, "{turns:?}");
    assert_eq!(turns[1].dir, turns[0].dir);
    assert_eq!(turns[1].followup, None, "started, not resumed");
    let prompts = world.runtime.prompts(WorkKind::Implement);
    assert_eq!(prompts[1], prompts[0], "the brief again");
    assert_eq!(world.store.pulls().len(), 1);
    let again = info_naming_the_reset(&log, "taking the refused turn again");
    assert_eq!(again.len(), 1, "{again:?}");
}

#[tokio::test]
async fn a_session_refused_again_at_the_reset_waits_until_the_new_reset() {
    let mut world = World::serving().await;
    implement_turns(
        &world,
        vec![refused_part_way, |call| {
            refused_until(usage_limit_resets_at() + TimeDelta::hours(5), Some(call))
        }],
    );
    open_ticket(&world, "Do the thing");
    world.factory.cycle().await.unwrap();
    world.clock.next_sleep().await;

    advance_past_the_reset(&world, 0);
    let next = world.clock.next_sleep().await;
    advance_to(&world, next);
    world.factory.settle().await;

    assert_eq!(next, usage_limit_resets_at() + TimeDelta::hours(5));
    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 3);
    assert_eq!(world.store.pulls().len(), 1);
}

#[tokio::test]
async fn a_session_refused_with_a_reset_that_has_passed_waits_a_short_time() {
    let mut world = World::serving().await;
    // An hour before the fake clock starts.
    implement_turns(
        &world,
        vec![|call| refused_until("2026-10-01T11:00:00Z".parse().unwrap(), Some(call))],
    );
    open_ticket(&world, "Do the thing");
    let before = world.clock.now();
    world.factory.cycle().await.unwrap();

    let until = world.clock.next_sleep().await;
    advance_to(&world, until);
    world.factory.settle().await;

    let wait = until - before;
    assert!(
        wait > TimeDelta::minutes(1) && wait < TimeDelta::minutes(2),
        "{wait}"
    );
    assert_eq!(world.store.pulls().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_resumed_session_has_the_time_it_had_left_when_refused() {
    let mut world = World::serving().await;
    let turns = implement_turns(&world, vec![refused_part_way]);
    open_ticket(&world, "Do the thing");
    // Of the 30 minutes the test factory gives a session, the first turn
    // runs 20.
    world.runtime.takes(Duration::from_secs(20 * 60));
    world.factory.cycle().await.unwrap();
    let reset = world.clock.next_sleep().await;

    tokio::time::advance(Duration::from_secs(60 * 60)).await;
    advance_to(&world, reset);
    world.factory.settle().await;

    let turns = turns.lock().unwrap().clone();
    assert_eq!(turns[1].left, Duration::from_secs(10 * 60));
}

#[tokio::test(start_paused = true)]
async fn a_scheduled_workflow_run_resumes_with_the_time_left_of_its_own_timeout() {
    let mut world = World::serving().await;
    // The workflow's own timeout is an hour, of which the first turn runs
    // 20 minutes.
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(
                ".linewright/workflows/hourly.md",
                "---\non:\n  schedule: \"0 * * * *\"\ntimeout: 60m\n---\nLook around.\n",
            )],
            "Add a workflow",
        )
        .await
        .unwrap();
    let seen = Turns::default();
    world.runtime.on_workflow({
        let seen = seen.clone();
        move |call| {
            if see(&seen, call) == 0 {
                return refused_part_way(call);
            }
            json(json!({"summary": "Nothing needs doing.", "issues": []}))
        }
    });
    // The first poll sees the workflow; it is due an hour later.
    world.poll().await;
    world.clock.advance(TimeDelta::hours(1));
    world.runtime.takes(Duration::from_secs(20 * 60));
    world.factory.cycle().await.unwrap();
    let reset = world.clock.next_sleep().await;

    tokio::time::advance(Duration::from_secs(2 * 60 * 60)).await;
    advance_to(&world, reset);
    world.factory.settle().await;

    let turns = seen.lock().unwrap().clone();
    assert_eq!(turns.len(), 2, "{turns:?}");
    assert_eq!(turns[1].session, turns[0].session);
    assert_eq!(turns[1].left, Duration::from_secs(40 * 60));
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

    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 1);
    let branch = format!("{}-do-the-thing", ticket.0);
    let read = world
        .store
        .read_file(&branch, "src/turn-0.txt")
        .await
        .unwrap();
    assert_eq!(read.as_deref(), Some("work\n"));
    assert!(world.issue(ticket).comments.is_empty());
    assert!(world.store.pulls().is_empty());
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
    assert!(store.pulls().is_empty());
}

#[tokio::test]
async fn a_waiting_run_counts_against_max_workers_and_is_not_started_again() {
    let mut world = World::with(|config| {
        config.waits_out_usage_limit = true;
        config.max_workers = 2;
    })
    .await;
    // A reset that has passed holds no starts back, so only the ceiling
    // does.
    implement_turns(
        &world,
        vec![|call| refused_until("2026-10-01T11:00:00Z".parse().unwrap(), Some(call))],
    );
    let waiting = open_ticket(&world, "Do the thing");
    world.factory.cycle().await.unwrap();
    world.clock.next_sleep().await;
    open_ticket(&world, "Do another thing");
    open_ticket(&world, "Do a third thing");

    let actions = world.factory.cycle().await.unwrap();

    let started = started(&actions);
    assert_eq!(started.len(), 1, "{started:?}");
    assert_ne!(started[0], Subject::Issue(waiting));
}

// `once` and `workflow run` do not wait.

#[tokio::test]
async fn once_ends_the_refused_run_without_waiting() {
    let mut world = World::new().await;
    implement_turns(&world, vec![refused_part_way]);
    let ticket = open_ticket(&world, "Do the thing");

    world.poll().await;

    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 1);
    assert!(world.issue(ticket).comments.is_empty());
    assert!(world.issue(ticket).is_assigned(&Login::new(LINEWRIGHT)));
}

// What `status` shows of the usage limit: `serve`'s own.

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

    /// The usage limit in `serve`'s view.
    fn usage_limit(&self) -> Option<UsageLimit> {
        self.serving.view.borrow().as_ref()?.memory.usage_limit
    }

    async fn stop(self) -> World {
        self.serving.stop().await;
        self.world
    }
}

fn the_usage_limit() -> Option<UsageLimit> {
    Some(UsageLimit {
        resets_at: usage_limit_resets_at(),
    })
}

#[tokio::test]
async fn while_serve_waits_out_the_limit_its_view_holds_it_until_the_reset() {
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
