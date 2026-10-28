use crate::common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::TimeDelta;

use common::*;
use linewright::domain::{
    CheckState, Issue, Login, Mergeability, Number, PullState, ReviewState, Snapshot,
};
use linewright::factory::Factory;
use linewright::harness::{Harness, SessionError, SessionSpec, Turn};
use linewright::reconcile::WorkKind;
use linewright::store::Store;
use linewright::store::local::LocalStore;
use linewright::webhook::{Listener, Pending};
use serde_json::json;

/// A poll interval no test waits out: what such a test sees, deliveries did.
const NEVER: Duration = Duration::from_hours(1);

struct Served {
    store: Arc<LocalStore>,
    harness: Arc<ScriptedHarness>,
    listener: Listener,
    serving: Serving,
    _dir: tempfile::TempDir,
}

/// Serves `world`, polling every `poll_interval`, once its first poll took
/// its snapshot and `serve` published it.
async fn serve(world: World, poll_interval: Duration) -> Served {
    let World {
        store,
        harness,
        factory,
        dir,
        ..
    } = world;
    let pending = Pending::default();
    let listener = Listener::bind(
        &webhooks("127.0.0.1:0"),
        pending.clone(),
        std::sync::Arc::new(linewright::clock::SystemClock),
    )
    .await
    .unwrap();
    let polled = store.snapshots_taken();
    let serving = Serving::start(factory, poll_interval, pending);
    eventually(|| store.snapshots_taken() > polled).await;
    // The store counts the snapshot before `serve` publishes it.
    eventually(|| serving.served.borrow().is_some()).await;
    Served {
        store,
        harness,
        listener,
        serving,
        _dir: dir,
    }
}

impl Served {
    async fn deliver(&self, event: &str, body: serde_json::Value) {
        let status = signed(&self.listener, event, &body.to_string()).await;
        assert!(status.is_success(), "{event}: {status}");
    }

    async fn deliver_form_encoded(&self, event: &str, body: serde_json::Value) {
        let status = signed_form(&self.listener, event, &body.to_string()).await;
        assert!(status.is_success(), "{event}: {status}");
    }

    fn issue(&self, number: Number) -> Issue {
        self.store.issue(number).unwrap()
    }

    /// The snapshot `serve` holds now.
    fn snapshot(&self) -> Arc<Snapshot> {
        let served = self.serving.served.borrow();
        served.as_ref().expect("served").view().snapshot
    }

    async fn stop(self) {
        self.serving.stop().await;
        self.listener.stop().await;
    }
}

fn claimed(store: &LocalStore, issue: Number) -> bool {
    let issue = store.issue(issue).unwrap();
    issue.assignees.contains(&Login::new(LINEWRIGHT))
}

#[tokio::test]
async fn an_issue_labeled_ready_for_agent_is_claimed_on_the_delivery() {
    let world = World::new().await;
    implement(&world);
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    let served = serve(world, NEVER).await;

    served.store.label_as(issue, "ready-for-agent").unwrap();
    served
        .deliver(
            "issues",
            json!({"action": "labeled", "issue": {"number": issue.0}}),
        )
        .await;

    eventually(|| claimed(&served.store, issue)).await;
    assert_eq!(served.store.snapshots_taken(), 1);
    served.stop().await;
}

#[tokio::test]
async fn an_issue_labeled_ready_for_agent_is_claimed_on_a_form_encoded_delivery() {
    let world = World::new().await;
    implement(&world);
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    let served = serve(world, NEVER).await;

    served.store.label_as(issue, "ready-for-agent").unwrap();
    served
        .deliver_form_encoded(
            "issues",
            json!({"action": "labeled", "issue": {"number": issue.0}}),
        )
        .await;

    eventually(|| claimed(&served.store, issue)).await;
    assert_eq!(served.store.snapshots_taken(), 1);
    served.stop().await;
}

fn about_issue(number: Number) -> serde_json::Value {
    json!({"issue": {"number": number.0}})
}

fn about_pull_as_issue(number: Number) -> serde_json::Value {
    json!({"issue": {"number": number.0, "pull_request": {"url": "https://api.github.com/x"}}})
}

fn about_pull(number: Number) -> serde_json::Value {
    json!({"number": number.0, "pull_request": {"number": number.0}})
}

#[tokio::test]
async fn an_issue_labeled_needs_triage_is_triaged_on_the_delivery() {
    let world = World::new().await;
    world.harness.on(WorkKind::Triage, |_| {
        Ok(json!({
            "outcome": "ready-for-human",
            "category": "bug",
            "comment": "Which widget?"
        })
        .to_string())
    });
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &[]);
    let served = serve(world, NEVER).await;

    served.store.label_as(issue, "needs-triage").unwrap();
    served.deliver("issues", about_issue(issue)).await;

    eventually(|| labels(&served.issue(issue)).contains(&"ready-for-human")).await;
    assert!(last_comment(&served.issue(issue)).contains("Which widget?"));
    assert_eq!(served.store.snapshots_taken(), 1);
    served.stop().await;
}

#[tokio::test]
async fn an_issue_closed_is_held_among_the_closed_items_on_the_delivery_about_it() {
    let world = World::new().await;
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-info"]);
    let served = serve(world, NEVER).await;

    served.store.close_issue_as_completed(issue).unwrap();
    served.deliver("issues", about_issue(issue)).await;

    let closed = || {
        let snapshot = served.snapshot();
        snapshot.closed.iter().any(|item| item.number == issue)
    };
    eventually(closed).await;
    assert!(served.snapshot().issue(issue).is_none());
    assert_eq!(served.store.snapshots_taken(), 1);
    served.stop().await;
}

#[tokio::test]
async fn a_refresh_holds_the_closed_issues_a_fresh_snapshot_would() {
    let world = World::new().await;
    let Tracking { closed: ticket, .. } = world.tracking_with_a_closed_ticket().await;
    let alone = world
        .store
        .open_issue(LINEWRIGHT, "Old idea", "Maybe.", &["ready-for-human"]);
    world.store.close_issue_as_completed(alone).unwrap();
    world.clock.advance(TimeDelta::days(8));
    let marker = world
        .store
        .open_issue(LINEWRIGHT, "Marker", "Now.", &["ready-for-human"]);
    let served = serve(world, NEVER).await;

    served.store.close_issue_as_completed(marker).unwrap();
    for issue in [ticket, alone, marker] {
        served.deliver("issues", about_issue(issue)).await;
    }

    let held = || -> Vec<Number> {
        let snapshot = served.snapshot();
        snapshot.closed.iter().map(|item| item.number).collect()
    };
    eventually(|| held().contains(&marker)).await;
    assert_eq!(held(), [ticket, marker]);
    served.stop().await;
}

#[tokio::test]
async fn a_refreshed_tracking_issue_that_gained_a_ticket_closed_before_the_done_window_holds_it() {
    let world = World::new().await;
    let Tracking {
        tracking,
        closed: ticket,
    } = world.tracking_with_a_closed_ticket().await;
    let old = world
        .store
        .open_issue(LINEWRIGHT, "Part three", "Of it.", &["ready-for-human"]);
    world.store.close_issue_as_completed(old).unwrap();
    world.clock.advance(TimeDelta::days(8));
    let marker = world
        .store
        .open_issue(LINEWRIGHT, "Marker", "Now.", &["ready-for-human"]);
    let served = serve(world, NEVER).await;

    served.store.add_sub_issue(tracking, old).await.unwrap();
    served.store.close_issue_as_completed(marker).unwrap();
    for issue in [tracking, marker] {
        served.deliver("issues", about_issue(issue)).await;
    }

    let held = || -> Vec<Number> {
        let snapshot = served.snapshot();
        snapshot.closed.iter().map(|item| item.number).collect()
    };
    eventually(|| held().contains(&marker)).await;
    assert_eq!(held(), [ticket, old, marker]);
    served.stop().await;
}

#[tokio::test]
async fn a_refresh_drops_an_issue_closed_before_the_done_window_since_the_poll() {
    let world = World::new().await;
    let aged = world
        .store
        .open_issue(LINEWRIGHT, "Old idea", "Maybe.", &["ready-for-human"]);
    world.store.close_issue_as_completed(aged).unwrap();
    world
        .clock
        .advance(TimeDelta::days(7) - TimeDelta::hours(1));
    let marker = world
        .store
        .open_issue(LINEWRIGHT, "Marker", "Now.", &["ready-for-human"]);
    let clock = world.clock.clone();
    let served = serve(world, NEVER).await;
    let held = || -> Vec<Number> {
        let snapshot = served.snapshot();
        snapshot.closed.iter().map(|item| item.number).collect()
    };
    assert_eq!(held(), [aged]);

    clock.advance(TimeDelta::hours(2));
    served.store.close_issue_as_completed(marker).unwrap();
    served.deliver("issues", about_issue(marker)).await;

    eventually(|| held().contains(&marker)).await;
    assert_eq!(held(), [marker]);
    served.stop().await;
}

#[tokio::test]
async fn an_answer_to_a_triage_question_is_triaged_again_on_the_delivery() {
    let world = World::new().await;
    world.harness.on(WorkKind::Triage, |call| {
        let outcome = if call.spec.prompt.contains("The round one.") {
            json!({
                "outcome": "ready-for-agent",
                "category": "bug",
                "comment": "## Agent Brief\n\n**Summary:** Round widgets crash."
            })
        } else {
            json!({
                "outcome": "needs-info",
                "category": "bug",
                "comment": "Which widget?"
            })
        };
        Ok(outcome.to_string())
    });
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);
    let served = serve(world, NEVER).await;
    eventually(|| labels(&served.issue(issue)).contains(&"needs-info")).await;

    served
        .store
        .comment_as("reporter", issue, "The round one.")
        .unwrap();
    served.deliver("issue_comment", about_issue(issue)).await;

    eventually(|| labels(&served.issue(issue)).contains(&"ready-for-agent")).await;
    assert_eq!(served.store.snapshots_taken(), 1);
    served.stop().await;
}

#[tokio::test]
async fn a_spec_change_merged_is_cut_into_tickets_on_the_delivery() {
    let world = World::new().await;
    world.harness.on(WorkKind::Design, |_| {
        Ok(json!({
            "outcome": "designed", "design": "A gadget store.", "tickets": [{"id": "store", "title": "Store gadgets", "body": "Gadgets persist.", "blocked_by": []}],
            "comment": "One slice."
        }).to_string())
    });
    let served = serve(world, NEVER).await;

    let pr = served
        .store
        .merge_change(
            "maintainer",
            "Gadgets",
            "Adds gadgets.",
            &[("docs/spec.md", "# Spec\n\nGadgets exist.\n")],
        )
        .await
        .unwrap();
    served.deliver("pull_request", about_pull(pr)).await;

    eventually(|| {
        served
            .store
            .issues()
            .iter()
            .any(|issue| issue.title == "Store gadgets")
    })
    .await;
    assert_eq!(served.store.snapshots_taken(), 1);
    served.stop().await;
}

type Feedback = fn(&LocalStore, Number) -> (&'static str, serde_json::Value);

#[tokio::test]
async fn feedback_on_linewrights_pull_request_is_addressed_on_the_delivery() {
    let feedback: [Feedback; 3] = [
        |store, pr| {
            store
                .review_as(
                    "maintainer",
                    pr,
                    ReviewState::ChangesRequested,
                    "Add a changelog entry.",
                    &[],
                )
                .unwrap();
            ("pull_request_review", about_pull(pr))
        },
        |store, pr| {
            store
                .review_as(
                    "maintainer",
                    pr,
                    ReviewState::Commented,
                    "",
                    &[("src/x.txt", "Add a changelog entry.")],
                )
                .unwrap();
            ("pull_request_review_comment", about_pull(pr))
        },
        |store, pr| {
            store
                .comment_as("maintainer", pr, "Add a changelog entry.")
                .unwrap();
            ("issue_comment", about_pull_as_issue(pr))
        },
    ];
    for give in feedback {
        let mut world = World::new().await;
        let (_, pr) = world.ticket_in_review().await;
        world.harness.on(WorkKind::Rework, |call| {
            assert!(call.spec.prompt.contains("Add a changelog entry."));
            commit_in(
                &call.spec.dir,
                &[("CHANGELOG.md", "- thing\n")],
                "Changelog",
            );
            Ok(json!({"outcome": "done", "comment": "Added the entry."}).to_string())
        });
        let served = serve(world, NEVER).await;

        let (event, body) = give(&served.store, pr);
        served.deliver(event, body).await;

        eventually(|| served.harness.prompts(WorkKind::Rework).len() == 1).await;
        assert_eq!(served.store.snapshots_taken(), 2, "{event}");
        served.stop().await;
    }
}

#[derive(Default)]
struct Endless {
    started: AtomicUsize,
    stopped: Arc<AtomicUsize>,
}

/// Counts a session stopped when dropped.
struct Stops(Arc<AtomicUsize>);

impl Drop for Stops {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl Harness for Endless {
    async fn start(&self, _: &SessionSpec) -> Result<Turn, SessionError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        let _stops = Stops(self.stopped.clone());
        std::future::pending().await
    }

    async fn resume(&self, spec: &SessionSpec, _: &str, _: &str) -> Result<Turn, SessionError> {
        self.start(spec).await
    }
}

#[tokio::test]
async fn closing_an_issue_stops_its_run_on_the_delivery() {
    let mut world = World::new().await;
    let harness = Arc::new(Endless::default());
    world.factory = Factory::start(
        world.store.clone(),
        harness.clone(),
        world.clock.clone(),
        config(&world.dir.path().join("endless")),
    )
    .await
    .unwrap();
    let issue = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    let served = serve(world, NEVER).await;
    eventually(|| harness.started.load(Ordering::SeqCst) == 1).await;

    served.store.close_issue_as_completed(issue).unwrap();
    served.deliver("issues", about_issue(issue)).await;

    eventually(|| harness.stopped.load(Ordering::SeqCst) == 1).await;
    assert_eq!(served.store.snapshots_taken(), 1);
    served.stop().await;
}

#[tokio::test]
async fn reopening_an_issue_ready_for_agent_claims_it_on_the_delivery() {
    let world = World::new().await;
    implement(&world);
    let issue = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.close_issue_as_completed(issue).unwrap();
    let served = serve(world, NEVER).await;

    served.store.reopen_issue_as(issue).unwrap();
    served.deliver("issues", about_issue(issue)).await;

    eventually(|| claimed(&served.store, issue)).await;
    assert_eq!(served.store.snapshots_taken(), 1);
    served.stop().await;
}

#[tokio::test]
async fn assigning_linewrights_pull_request_reads_it_again_on_the_delivery() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    let served = serve(world, NEVER).await;

    let before_unassigned = served.store.items_read();
    served
        .store
        .unassign_as("maintainer", pr, "factory")
        .unwrap();
    served.deliver("pull_request", about_pull(pr)).await;
    eventually(|| served.store.items_read() > before_unassigned).await;

    let before_assigned = served.store.items_read();
    served.store.assign_as("maintainer", pr, "factory").unwrap();
    served.deliver("pull_request", about_pull(pr)).await;
    eventually(|| served.store.items_read() > before_assigned).await;

    assert_eq!(served.store.snapshots_taken(), 2);
    served.stop().await;
}

#[tokio::test]
async fn closing_linewrights_pull_request_hands_its_ticket_back_on_the_delivery() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    let served = serve(world, NEVER).await;

    served.store.close_pull_as(pr).unwrap();
    served.deliver("pull_request", about_pull(pr)).await;

    eventually(|| labels(&served.issue(ticket)).contains(&"ready-for-human")).await;
    assert_eq!(
        last_comment(&served.issue(ticket)),
        format!("{pr} was closed without merging.")
    );
    assert_eq!(served.store.snapshots_taken(), 2);
    served.stop().await;
}

#[tokio::test]
async fn merging_linewrights_pull_request_works_the_ticket_it_unblocked_on_the_deliveries() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    let blocked = world.store.open_issue(
        "maintainer",
        "Then the next thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.add_blocked_by(blocked, ticket).await.unwrap();
    let served = serve(world, NEVER).await;
    assert!(!claimed(&served.store, blocked));

    served.store.merge_pull(pr).await.unwrap();
    served.deliver("pull_request", about_pull(pr)).await;
    served.deliver("issues", about_issue(ticket)).await;

    eventually(|| claimed(&served.store, blocked)).await;
    assert_eq!(served.store.snapshots_taken(), 2);
    served.stop().await;
}

#[tokio::test]
async fn a_delivery_replayed_late_and_twice_after_a_newer_change_takes_no_step_back() {
    let world = World::new().await;
    world.harness.on(WorkKind::Triage, |_| {
        Ok(json!({
            "outcome": "ready-for-human",
            "category": "bug",
            "comment": "Which widget?"
        })
        .to_string())
    });
    implement(&world);
    let other = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &[]);
    let served = serve(world, NEVER).await;
    // As GitHub sends it when the label is applied: its account of the issue
    // is of then.
    let labeled = json!({
        "action": "labeled",
        "label": {"name": "needs-triage"},
        "issue": {"number": issue.0, "state": "open", "labels": [{"name": "needs-triage"}]}
    });
    served.store.label_as(issue, "needs-triage").unwrap();
    served.deliver("issues", labeled.clone()).await;
    eventually(|| labels(&served.issue(issue)).contains(&"ready-for-human")).await;
    let comments = served.issue(issue).comments.len();

    served.deliver("issues", labeled.clone()).await;
    served.deliver("issues", labeled).await;
    // A delivery after them shows when they were acted on.
    served.store.label_as(other, "ready-for-agent").unwrap();
    served.deliver("issues", about_issue(other)).await;
    eventually(|| claimed(&served.store, other)).await;

    assert_eq!(served.harness.prompts(WorkKind::Triage).len(), 1);
    let issue = served.issue(issue);
    assert_eq!(labels(&issue), ["bug", "ready-for-human"]);
    assert_eq!(issue.comments.len(), comments);
    served.stop().await;
}

#[tokio::test]
async fn polls_go_on_at_the_poll_interval_while_deliveries_arrive() {
    let world = World::new().await;
    let issue = world
        .store
        .open_issue("maintainer", "Leave it", "Later.", &["ready-for-human"]);
    let served = serve(world, Duration::from_millis(100)).await;

    let polling = tokio::time::timeout(Duration::from_secs(10), async {
        while served.store.snapshots_taken() < 4 {
            served.deliver("issues", about_issue(issue)).await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;

    assert!(polling.is_ok(), "polls stopped while deliveries arrived");
    served.stop().await;
}

#[tokio::test]
async fn a_failed_read_for_a_delivery_is_logged_and_the_next_poll_catches_up() {
    let (log, _guard) = Log::capture();
    let world = World::new().await;
    implement(&world);
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    let served = serve(world, Duration::from_millis(200)).await;

    served.store.fail_reads("GitHub is down");
    served.store.label_as(issue, "ready-for-agent").unwrap();
    served.deliver("issues", about_issue(issue)).await;
    let failed = || {
        log.lines("ERROR")
            .iter()
            .any(|line| line.contains("deliveries failed") && line.contains("GitHub is down"))
    };
    eventually(failed).await;
    assert!(!claimed(&served.store, issue));
    served.store.restore_reads();

    eventually(|| claimed(&served.store, issue)).await;
    served.stop().await;
}

#[tokio::test]
async fn a_failed_read_for_a_delivery_is_no_failed_poll() {
    let (log, _guard) = Log::capture();
    let world = World::new().await;
    let issue = world
        .store
        .open_issue("maintainer", "Do the thing", "Please.", &[]);
    let served = serve(world, Duration::from_hours(1)).await;

    served.store.fail_reads("GitHub is down");
    served.deliver("issues", about_issue(issue)).await;
    eventually(|| {
        log.lines("ERROR")
            .iter()
            .any(|line| line.contains("deliveries failed"))
    })
    .await;

    assert_eq!(served.serving.failed_polls(), None);
    served.stop().await;
}

const CI_EVENTS: [&str; 3] = ["check_run", "check_suite", "status"];

/// The pull requests a check names are left empty, as GitHub leaves them
/// for one from a fork.
fn about_commit(event: &str, sha: &str) -> serde_json::Value {
    match event {
        "check_run" => json!({"check_run": {"head_sha": sha, "pull_requests": []}}),
        "check_suite" => json!({"check_suite": {"head_sha": sha, "pull_requests": []}}),
        "status" => json!({"sha": sha, "branches": []}),
        _ => unreachable!("{event} is not a CI event"),
    }
}

impl Served {
    async fn report_check(&self, pr: Number, state: CheckState, event: &str) {
        let check = check("test", state, "assertion failed: widgets == 2");
        self.store.set_checks(pr, vec![check]).await.unwrap();
        let head = self.store.snapshot_pull(pr).await.head_sha;
        self.deliver(event, about_commit(event, &head)).await;
    }
}

#[tokio::test]
async fn a_failing_check_on_linewrights_pull_request_starts_the_ci_fix_on_the_delivery() {
    for event in CI_EVENTS {
        let mut world = World::new().await;
        let (_, pr) = world.ticket_in_review().await;
        world.harness.on(WorkKind::FixCi, |_| {
            Ok(json!({"outcome": "done", "comment": "Fixed the widget count."}).to_string())
        });
        let served = serve(world, NEVER).await;

        served.report_check(pr, CheckState::Failed, event).await;

        eventually(|| served.harness.prompts(WorkKind::FixCi).len() == 1).await;
        let prompt = &served.harness.prompts(WorkKind::FixCi)[0];
        assert!(prompt.contains("widgets == 2"), "{event}: {prompt}");
        assert_eq!(served.store.snapshots_taken(), 2, "{event}");
        served.stop().await;
    }
}

#[tokio::test]
async fn green_checks_on_linewrights_pull_request_mark_it_ready_on_the_delivery() {
    for event in CI_EVENTS {
        let mut world = World::new().await;
        let (_, pr) = world.ticket_in_review().await;
        let served = serve(world, NEVER).await;

        served.report_check(pr, CheckState::Passed, event).await;

        eventually_async(|| async {
            served.store.snapshot_pull(pr).await.state == PullState::Open { draft: false }
        })
        .await;
        assert_eq!(served.store.snapshots_taken(), 2, "{event}");
        served.stop().await;
    }
}

#[tokio::test]
async fn with_auto_merge_green_checks_on_linewrights_pull_request_merge_it_on_the_delivery() {
    for event in CI_EVENTS {
        let mut world = World::with(|config| config.auto_merge = AUTO_MERGE).await;
        let (ticket, pr) = world.ticket_in_review().await;
        let served = serve(world, NEVER).await;

        served.report_check(pr, CheckState::Passed, event).await;

        eventually(|| !served.issue(ticket).open).await;
        let state = served.store.snapshot_pull(pr).await.state;
        assert!(
            matches!(state, PullState::Merged { .. }),
            "{event}: {state:?}"
        );
        assert_eq!(served.store.snapshots_taken(), 2, "{event}");
        served.stop().await;
    }
}

#[tokio::test]
async fn a_push_to_the_default_branch_brings_linewrights_pull_request_up_to_date_on_the_delivery() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    let served = serve(world, NEVER).await;
    let before = served.store.snapshot_pull(pr).await;

    served
        .store
        .commit(
            "maintainer",
            "main",
            &[("src/other.txt", "other\n")],
            "Other",
        )
        .await
        .unwrap();
    assert_eq!(
        served.store.snapshot_pull(pr).await.mergeability,
        Mergeability::Behind
    );
    served
        .deliver("push", json!({"ref": "refs/heads/main"}))
        .await;

    eventually_async(|| async {
        let after = served.store.snapshot_pull(pr).await;
        after.mergeability == Mergeability::Clean && after.head_sha != before.head_sha
    })
    .await;
    assert_eq!(served.store.snapshots_taken(), 2);
    served.stop().await;
}
