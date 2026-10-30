use crate::common;

use chrono::TimeDelta;
use common::*;
use linewright::domain::{Assignment, AutoMerge, Issue, IssueEvent, PullState};
use linewright::reconcile::WorkKind;
use linewright::store::Store;
use linewright::store::local::{LocalStore, Paused, Write};
use linewright::work_settings::{Effort, Partial, StageTable};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn a_new_issue_is_triaged_into_a_category_a_state_and_a_brief() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Triage, |_| {
        Ok(json!({
            "outcome": "ready-for-agent",
            "category": "bug",
            "comment": "## Agent Brief\n\n**Summary:** Empty widgets crash."
        })
        .to_string())
    });
    let issue = world.store.open_issue(
        "reporter",
        "Crash on empty widget",
        "It crashes.",
        &["needs-triage"],
    );

    world.poll().await;

    let issue = world.issue(issue);
    assert_eq!(labels(&issue), ["bug", "ready-for-agent"]);
    assert_eq!(
        last_comment(&issue),
        "## Agent Brief\n\n**Summary:** Empty widgets crash."
    );
    assert!(issue.open);
    let claims = issue
        .events
        .iter()
        .filter(|e| {
            matches!(e, IssueEvent::Assignment(Assignment::Assigned { assignee, .. })
                if assignee.as_str() == LINEWRIGHT)
        })
        .count();
    assert_eq!(claims, 1, "claimed once before triage");
}

#[tokio::test]
async fn a_triage_run_paused_after_removing_needs_triage_ends_labeled_once_released() {
    let mut world = World::new().await;
    let (issue, mut paused) = triage_paused_after_removing_needs_triage(&world);

    world.factory.cycle().await.unwrap();
    paused.reached().await;
    assert_eq!(labels(&world.issue(issue)), ["bug", "ready-for-agent"]);
    paused.release();
    world.factory.settle().await;

    assert_eq!(labels(&world.issue(issue)), ["bug", "ready-for-agent"]);
}

#[tokio::test]
async fn a_cancel_after_an_implement_run_begins_pushing_its_result_does_not_stop_it() {
    let mut world = World::new().await;
    implement(&world);
    let ticket = open_ticket(&world, "Gadgets");
    let mut paused = world.store.pause_after(Write::Push {
        branch: format!("{}-gadgets", ticket.0),
    });
    world.factory.cycle().await.unwrap();
    paused.reached().await;

    world
        .store
        .unassign_as("maintainer", ticket, LINEWRIGHT)
        .unwrap();
    world.factory.cycle().await.unwrap();
    paused.release();
    world.factory.settle().await;

    assert_eq!(world.store.pulls().len(), 1);
    assert_eq!(ended_runs(&world), [linewright::runs::Ending::Done]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_implement_run_cancelled_as_its_session_answers_keeps_its_work_and_posts_nothing() {
    let mut world = World::new().await;
    let (mut gate, post) = gate();
    world.harness.on(WorkKind::Implement, move |call| {
        commit_in(&call.spec.dir, &[("src/gadgets.txt", "done\n")], "Do it");
        post.wait();
        Ok(json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."}).to_string())
    });
    let ticket = open_ticket(&world, "Gadgets");
    world.factory.cycle().await.unwrap();
    gate.begun().await;

    world
        .store
        .unassign_as("maintainer", ticket, LINEWRIGHT)
        .unwrap();
    world.factory.cycle().await.unwrap();
    gate.release();
    world.factory.settle().await;

    assert_eq!(world.store.pulls(), []);
    assert_eq!(world.issue(ticket).comments, []);
    assert_eq!(
        world
            .store
            .file(&format!("{}-gadgets", ticket.0), "src/gadgets.txt")
            .await
            .unwrap(),
        "done\n"
    );
    assert_eq!(ended_runs(&world), [linewright::runs::Ending::Cancelled]);
}

#[tokio::test]
async fn a_cancel_after_a_rework_begins_pushing_its_result_does_not_stop_it() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.rework_asked(pr);
    let head_ref = world.store.snapshot_pull(pr).await.head_ref;
    let mut paused = world.store.pause_after(Write::Push {
        branch: head_ref.clone(),
    });
    world.factory.cycle().await.unwrap();
    paused.reached().await;

    world
        .store
        .unassign_as("maintainer", pr, LINEWRIGHT)
        .unwrap();
    world.factory.cycle().await.unwrap();
    paused.release();
    world.factory.settle().await;

    assert_eq!(last_pull_comment(&world, pr).await, "Renamed.");
    assert_eq!(
        world
            .store
            .file(&head_ref, "src/renamed.txt")
            .await
            .unwrap(),
        "renamed\n"
    );
    assert_eq!(ended_runs(&world), [linewright::runs::Ending::Done]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rework_cancelled_before_its_push_by_its_pull_request_closing_pushes_and_posts_nothing() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .comment_as("maintainer", pr, "Rename it.")
        .unwrap();
    let (mut gate, post) = gate();
    world.harness.on(WorkKind::Rework, move |call| {
        commit_in(
            &call.spec.dir,
            &[("src/renamed.txt", "renamed\n")],
            "Rename",
        );
        post.wait();
        Ok(json!({"outcome": "done", "comment": "Renamed."}).to_string())
    });
    let head_ref = world.store.snapshot_pull(pr).await.head_ref;
    world.factory.cycle().await.unwrap();
    gate.begun().await;

    world.store.close_pull_as(pr).unwrap();
    world.factory.cycle().await.unwrap();
    gate.release();
    world.factory.settle().await;

    assert!(
        world
            .store
            .file(&head_ref, "src/renamed.txt")
            .await
            .is_err()
    );
    assert_ne!(last_pull_comment(&world, pr).await, "Renamed.");
    assert_eq!(ended_runs(&world), [linewright::runs::Ending::Cancelled]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_attempt_retried_after_an_infrastructure_failure_can_be_cancelled_during_its_new_session()
 {
    let mut world = World::new().await;
    let (mut gate, post) = gate();
    let sessions = std::sync::atomic::AtomicUsize::new(0);
    world.harness.on(WorkKind::Implement, move |call| {
        if sessions.fetch_add(1, std::sync::atomic::Ordering::Relaxed) > 0 {
            post.wait();
        }
        let file = call.spec.dir.join("src/gadgets.txt");
        let done = std::fs::read_to_string(&file).unwrap_or_default();
        commit_in(
            &call.spec.dir,
            &[("src/gadgets.txt", &format!("{done}done\n"))],
            "Do it",
        );
        Ok(json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."}).to_string())
    });
    let ticket = open_ticket(&world, "Gadgets");
    let mut paused = world.store.pause_after(Write::Push {
        branch: format!("{}-gadgets", ticket.0),
    });
    world.factory.cycle().await.unwrap();
    paused.reached().await;
    paused.fail();
    gate.begun().await;

    world
        .store
        .unassign_as("maintainer", ticket, LINEWRIGHT)
        .unwrap();
    world.factory.cycle().await.unwrap();
    gate.release();
    world.factory.settle().await;

    assert_eq!(world.store.pulls(), []);
    assert_eq!(world.issue(ticket).comments, []);
    assert_eq!(ended_runs(&world), [linewright::runs::Ending::Cancelled]);
}

/// Starts triaging an issue, pauses the run once it removes `needs-triage`,
/// and has a poll cancel the run for it. Returns the issue and the paused
/// write.
async fn triage_cancelled_while_recording(world: &mut World) -> (Number, Paused) {
    let (issue, mut paused) = triage_paused_after_removing_needs_triage(world);
    world.factory.cycle().await.unwrap();
    paused.reached().await;
    world.factory.cycle().await.unwrap();
    (issue, paused)
}

#[tokio::test]
async fn a_triage_run_cancelled_while_it_records_its_result_records_all_of_it() {
    let (log, _guard) = Log::capture();
    let mut world = World::new().await;
    let (issue, paused) = triage_cancelled_while_recording(&mut world).await;

    paused.release();
    world.factory.settle().await;

    let issue = world.issue(issue);
    assert_eq!(labels(&issue), ["bug", "ready-for-agent"]);
    assert_eq!(last_comment(&issue), TRIAGE_BRIEF);
    assert_eq!(ended_runs(&world), [linewright::runs::Ending::Done]);
    assert!(
        log.text()
            .contains("the run is applying its result; left to finish"),
        "{}",
        log.text()
    );
}

#[tokio::test]
async fn a_run_whose_recording_fails_after_a_cancel_arrived_is_handed_back() {
    let mut world = World::new().await;
    let (issue, paused) = triage_cancelled_while_recording(&mut world).await;

    paused.fail();
    world.factory.settle().await;

    let issue = world.issue(issue);
    assert!(issue.has_label("ready-for-human"), "{:?}", issue.labels);
    assert_eq!(ended_runs(&world), [linewright::runs::Ending::HandedBack]);
}

/// Triages two issues in one poll with sessions that each draft a comment in
/// their scratch directory, and returns each session's scratch directory
/// with whether it was empty when the session started.
async fn triage_two_issues_drafting_in_scratch() -> Vec<(std::path::PathBuf, bool)> {
    let mut world = World::new().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sessions = seen.clone();
    world.harness.on(WorkKind::Triage, move |call| {
        let scratch = call.spec.scratch.clone();
        let empty = std::fs::read_dir(&scratch).unwrap().next().is_none();
        std::fs::write(scratch.join("comment.md"), "A draft.").unwrap();
        sessions.lock().unwrap().push((scratch, empty));
        Ok(json!({
            "outcome": "ready-for-agent",
            "category": "bug",
            "comment": "## Agent Brief\n\n**Summary:** It crashes."
        })
        .to_string())
    });
    for title in ["Crash on empty widget", "Crash on full widget"] {
        world
            .store
            .open_issue("reporter", title, "It crashes.", &["needs-triage"]);
    }

    world.poll().await;

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    seen
}

#[tokio::test]
async fn sessions_running_at_once_get_different_scratch_directories() {
    let seen = triage_two_issues_drafting_in_scratch().await;

    assert_ne!(seen[0].0, seen[1].0);
}

#[tokio::test]
async fn a_sessions_scratch_directory_is_empty_when_it_starts() {
    let seen = triage_two_issues_drafting_in_scratch().await;

    assert_eq!(
        seen.iter().map(|(_, empty)| *empty).collect::<Vec<_>>(),
        [true, true]
    );
}

#[tokio::test]
async fn a_sessions_scratch_directory_is_removed_when_its_run_ends() {
    let seen = triage_two_issues_drafting_in_scratch().await;

    assert_eq!(
        seen.iter()
            .map(|(scratch, _)| scratch.exists())
            .collect::<Vec<_>>(),
        [false, false]
    );
}

/// The first-message prompts of every work kind, the design of an issue and
/// of a spec change, and an automation's.
async fn every_kind_of_first_message_prompt() -> Vec<(&'static str, Vec<String>)> {
    let mut issue = with_issue_for_triage().await;
    design_issues(&issue);
    implement(&issue);
    for _ in 0..3 {
        issue.poll().await;
    }
    let pr = *issue
        .store
        .pulls()
        .last()
        .expect("a pull request was opened");
    issue.rework_asked(pr);
    issue.poll().await;

    let mut spec_change = World::new().await;
    tickets_script(&spec_change);
    merge_spec_change(&spec_change).await;
    spec_change.poll().await;

    let mut failing = World::new().await;
    let (_, failing_pr) = failing.ticket_in_review().await;
    failing
        .store
        .set_checks(
            failing_pr,
            vec![check("test", CheckState::Failed, "failed")],
        )
        .await
        .unwrap();
    failing.harness.on(WorkKind::FixCi, |call| {
        commit_in(&call.spec.dir, &[("src/fix.txt", "fix\n")], "Fix the test");
        Ok(json!({"outcome": "done", "comment": "Fixed."}).to_string())
    });
    failing.poll().await;

    let (mut moved, _) = moved_past(Repository::default(), None, true).await;
    moved.poll().await;

    let mut automated = World::new().await;
    automated.hourly_by_schedule_then_by_hand().await;

    vec![
        ("triage", issue.harness.prompts(WorkKind::Triage)),
        (
            "design of an issue",
            issue.harness.prompts(WorkKind::Design),
        ),
        ("implement", issue.harness.prompts(WorkKind::Implement)),
        ("rework", issue.harness.prompts(WorkKind::Rework)),
        (
            "design of a spec change",
            spec_change.harness.prompts(WorkKind::Design),
        ),
        ("fix-ci", failing.harness.prompts(WorkKind::FixCi)),
        ("update", moved.harness.prompts(WorkKind::Update)),
        ("automation", automated.harness.automation_prompts()),
    ]
}

#[tokio::test]
async fn every_first_message_prompt_names_the_repository() {
    for (kind, prompts) in every_kind_of_first_message_prompt().await {
        assert_ne!(prompts.len(), 0, "no {kind} prompt");
        let first = &prompts[0];
        assert!(first.contains(REPO), "{kind}: {first}");
    }
}

#[tokio::test]
async fn an_implementing_session_is_given_linewrights_skills_as_its_plugin() {
    let mut world = World::new().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sessions = seen.clone();
    world.harness.on(WorkKind::Implement, move |call| {
        let read = |path: &str| std::fs::read_to_string(call.spec.plugin.join(path)).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&read(".claude-plugin/plugin.json")).unwrap();
        *sessions.lock().unwrap() = Some((
            manifest["name"].as_str().unwrap().to_string(),
            read("skills/code-review/SKILL.md"),
        ));
        commit_in(
            &call.spec.dir,
            &[("src/thing.txt", "done\n")],
            "Do the ticket",
        );
        Ok(json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."}).to_string())
    });
    world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    assert_eq!(
        seen.lock().unwrap().clone(),
        Some((
            "linewright".to_string(),
            include_str!("../../briefs/plugin/skills/code-review/SKILL.md").to_string()
        ))
    );
}

#[tokio::test]
async fn the_maintainers_answer_to_a_triage_for_them_is_triaged_again_once_they_label_it() {
    let mut world = World::new().await;
    let issue = world.issue_triaged_for_the_maintainer().await;

    world
        .store
        .comment_as("maintainer", issue, "Yes, make them resizable.")
        .unwrap();
    world.poll().await;

    assert_eq!(
        labels(&world.issue(issue)),
        ["enhancement", "ready-for-human"]
    );

    world
        .store
        .label_as("maintainer", issue, "needs-triage")
        .unwrap();
    world.poll().await;

    let issue = world.issue(issue);
    assert_eq!(labels(&issue), ["enhancement", "ready-for-agent"]);
    assert_eq!(
        last_comment(&issue),
        "## Agent Brief\n\n**Summary:** Make widgets resizable."
    );
}

#[tokio::test]
async fn an_issue_given_a_second_status_label_keeps_only_the_newer() {
    let mut world = World::new().await;
    let issue = world.store.open_issue(
        "maintainer",
        "Resize widgets",
        "",
        &["bug", "ready-for-human"],
    );
    world.clock.advance(TimeDelta::minutes(1));
    world
        .store
        .label_as("maintainer", issue, "needs-info")
        .unwrap();

    world.poll().await;

    assert_eq!(labels(&world.issue(issue)), ["bug", "needs-info"]);
}

fn triaged_to(world: &World, outcome: &'static str) {
    world.harness.on(WorkKind::Triage, move |_| {
        Ok(json!({
            "outcome": outcome,
            "category": "bug",
            "comment": "Triaged."
        })
        .to_string())
    });
}

#[tokio::test]
async fn an_issue_linewright_filed_with_needs_triage_is_triaged() {
    let mut world = World::new().await;
    triaged_to(&world, "ready-for-agent");
    let issue = world
        .store
        .open_issue(LINEWRIGHT, "Untested fix", "", &["needs-triage"]);

    world.poll().await;

    let issue = world.issue(issue);
    assert_eq!(labels(&issue), ["bug", "ready-for-agent"]);
    assert_eq!(last_comment(&issue), "Triaged.");
}

fn assignees(issue: &Issue) -> Vec<&str> {
    issue.assignees.iter().map(Login::as_str).collect()
}

#[tokio::test]
async fn needs_info_assigns_the_people_asked_and_names_them_all_even_one_github_refuses() {
    let mut world = World::new().await;
    triage_asking(
        &world,
        json!(["tester", "@outsider"]),
        "Which version, @tester?",
    );
    world.store.refuse_to_assign("outsider");
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);

    world.poll().await;

    let issue = world.issue(issue);
    assert_eq!(labels(&issue), ["bug", "needs-info"]);
    assert_eq!(assignees(&issue), ["tester"]);
    assert_eq!(last_comment(&issue), "@outsider\n\nWhich version, @tester?");
}

#[tokio::test]
async fn a_result_asking_something_that_is_not_a_login_is_sent_back() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Triage, |call| {
        let ask = if call.followup.is_none() {
            "the reporter"
        } else {
            "reporter"
        };
        Ok(json!({"outcome": "needs-info", "category": "bug", "ask": [ask], "comment": "Which version?"}).to_string())
    });
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Triage);
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[1].contains("`/ask/0`: \"the reporter\" does not match"),
        "{}",
        prompts[1]
    );
    assert_eq!(assignees(&world.issue(issue)), ["reporter"]);
}

#[tokio::test]
async fn a_reply_unassigns_the_people_asked_and_linewright_claims_the_issue_to_triage_it_again() {
    let mut world = World::new().await;
    let issue = world.issue_asking_its_reporter().await;

    world
        .store
        .comment_as("reporter", issue, "Version 3.")
        .unwrap();
    world.poll().await;

    let issue = world.issue(issue);
    assert_eq!(labels(&issue), ["bug", "ready-for-agent"]);
    assert_eq!(last_comment(&issue), "Triaged again.");
    assert_eq!(assignees(&issue), [LINEWRIGHT]);
}

#[tokio::test]
async fn a_reply_leaves_a_person_someone_else_assigned_and_linewright_does_not_claim_over_them() {
    let mut world = World::new().await;
    let issue = world.issue_asking_its_reporter().await;
    world
        .store
        .assign_as("maintainer", issue, "maintainer")
        .unwrap();

    world
        .store
        .comment_as("reporter", issue, "Version 3.")
        .unwrap();
    world.poll().await;

    let issue = world.issue(issue);
    assert_eq!(labels(&issue), ["bug", "needs-triage"]);
    assert_eq!(assignees(&issue), ["maintainer"]);
    assert_eq!(last_comment(&issue), "Version 3.");
}

#[tokio::test(flavor = "multi_thread")]
async fn unassigning_linewright_during_triage_cancels_it_and_it_posts_nothing() {
    let mut world = World::new().await;
    let (release, released) = std::sync::mpsc::channel::<()>();
    let released = Mutex::new(released);
    let (begin, mut begun) = tokio::sync::mpsc::unbounded_channel();
    world.harness.on(WorkKind::Triage, move |_| {
        begin.send(()).unwrap();
        released.lock().unwrap().recv().unwrap();
        Ok(json!({
            "outcome": "ready-for-human",
            "category": "bug",
            "comment": "Triaged."
        })
        .to_string())
    });
    let issue = world
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);
    world.factory.cycle().await.unwrap();
    begun.recv().await.unwrap();
    assert!(world.issue(issue).is_assigned(&Login::new(LINEWRIGHT)));

    world
        .store
        .unassign_as("maintainer", issue, LINEWRIGHT)
        .unwrap();
    world.factory.cycle().await.unwrap();
    release.send(()).unwrap();
    world.factory.settle().await;

    let issue = world.issue(issue);
    assert_eq!(labels(&issue), ["needs-triage"]);
    assert_eq!(issue.comments, []);
    assert_eq!(issue.assignees, []);
}

#[tokio::test]
async fn a_merged_spec_change_becomes_tickets_worked_in_dependency_order_into_ready_pull_requests()
{
    let mut world = World::new().await;
    tickets_script(&world);
    implement_gadgets(&world);
    let spec_pr = world
        .store
        .merge_change(
            "maintainer",
            "Gadgets",
            "Adds gadgets.",
            &[("docs/spec.md", "# Spec\n\nGadgets exist.\n")],
        )
        .await
        .unwrap();

    world.poll().await;

    assert!(
        world
            .store
            .pull_labels(spec_pr)
            .contains(&"linewright:designed".to_string())
    );
    let issues = world.store.issues();
    let [tracking, store_ticket, list_ticket] = &issues[..] else {
        panic!("expected a tracking issue and two tickets, got {issues:#?}");
    };
    assert_eq!(tracking.title, "Spec change: Gadgets");
    assert!(tracking.body.contains(&format!("{spec_pr}")));
    assert_eq!(
        tracking
            .sub_issues
            .iter()
            .map(|s| s.number)
            .collect::<Vec<_>>(),
        [store_ticket.number, list_ticket.number]
    );
    assert_eq!(list_ticket.blocked_by[0].number, store_ticket.number);
    assert_eq!(
        labels(store_ticket),
        ["linewright:designed", "ready-for-agent"]
    );
    assert_eq!(
        labels(list_ticket),
        ["linewright:designed", "ready-for-agent"]
    );
    assert_eq!(
        last_comment(tracking),
        format!(
            "A gadget store, and a listing that reads it.\n\n## Tickets\n\n\
             1. {} Store gadgets\n2. {} List gadgets\n\n\
             Two slices: storing, then listing.",
            store_ticket.number, list_ticket.number
        )
    );
    assert_eq!(
        last_pull_comment(&world, spec_pr).await,
        format!(
            "Two slices: storing, then listing.\n\nTracked in {}:\n\n\
             1. {} Store gadgets\n2. {} List gadgets\n",
            tracking.number, store_ticket.number, list_ticket.number
        )
    );

    world.poll().await;

    let pulls = world.store.pulls();
    let pr = *pulls.last().unwrap();
    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(view.title, "Add gadgets");
    assert!(
        view.body
            .ends_with(&format!("Closes {}", store_ticket.number))
    );
    assert_eq!(
        view.state,
        linewright::domain::PullState::Open { draft: true }
    );
    assert_eq!(
        world
            .store
            .file(
                &view.head_ref,
                &format!("src/issue-{}.txt", store_ticket.number.0)
            )
            .await
            .unwrap(),
        "gadget\n"
    );
    assert!(
        world
            .issue(store_ticket.number)
            .is_assigned(&linewright::domain::Login::new(LINEWRIGHT))
    );
    assert!(
        !world
            .issue(list_ticket.number)
            .is_assigned(&linewright::domain::Login::new(LINEWRIGHT))
    );

    world.clock.advance(chrono::TimeDelta::minutes(6));
    world.poll().await;

    let ready = world.store.snapshot_pull(pr).await;
    assert_eq!(
        ready.state,
        linewright::domain::PullState::Open { draft: false }
    );
    assert_eq!(
        ready.comments.last().unwrap().body,
        "No CI reported: ready for review."
    );

    world.store.merge_pull(pr).await.unwrap();
    world.poll().await;

    assert!(!world.issue(store_ticket.number).open);
    assert!(
        world
            .issue(list_ticket.number)
            .is_assigned(&linewright::domain::Login::new(LINEWRIGHT))
    );
}

/// A world whose spec change, to the spec and to code, was cut into tickets,
/// the first of which a poll implemented; and that cut.
async fn implemented_a_ticket_cut_from_a_spec_change() -> (World, SpecCut) {
    let mut world = World::new().await;
    let cut = world.spec_change_cut_into_tickets().await;
    world.poll().await;
    (world, cut)
}

#[tokio::test]
async fn a_spec_change_whose_design_label_was_removed_has_its_cut_and_pull_requests_closed_but_not_a_persons_sub_issue()
 {
    let mut world = World::new().await;
    let (cut, pr) = world.spec_change_ticket_in_review().await;
    let spec_pr = cut.spec_change;
    let persons = world
        .store
        .open_issue("maintainer", "Paint gadgets", "", &[]);
    world
        .store
        .add_sub_issue(cut.tracking, persons)
        .await
        .unwrap();
    world
        .store
        .remove_label(spec_pr, "linewright:designed")
        .await
        .unwrap();

    world.poll().await;

    for ticket in cut.tickets {
        let discarded = world.issue(ticket);
        assert!(!discarded.open, "{discarded:#?}");
        assert_eq!(
            last_comment(&discarded),
            format!(
                "{spec_pr} is being designed again, so this ticket from its earlier cut is \
                 discarded."
            )
        );
    }
    let pull = world.store.snapshot_pull(pr).await;
    assert!(matches!(pull.state, PullState::Closed { .. }), "{pull:#?}");
    let tracking = world.issue(cut.tracking);
    assert!(!tracking.open);
    assert_eq!(
        last_comment(&tracking),
        format!(
            "{spec_pr} is being designed again, so this tracking issue and its tickets are \
             discarded."
        )
    );
    assert!(world.issue(persons).open);
}

/// That `brief` holds the spec diff of the spec change from
/// [`World::spec_change_cut_into_tickets`], after the pointer to it, and none
/// of its changes to code.
fn assert_briefed_with_the_spec_diff(brief: &str) {
    let parent = brief.find("### Parent issue").expect(brief);
    let diff = brief.find("```diff\n").expect(brief);
    assert!(parent < diff, "{brief}");
    assert!(brief[diff..].contains("+Gadgets exist."), "{brief}");
    assert!(!brief.contains("Gadgets live here."), "{brief}");
}

#[tokio::test]
async fn a_ticket_cut_from_a_spec_change_is_briefed_with_its_tracking_issue_and_the_spec_change() {
    let (world, cut) = implemented_a_ticket_cut_from_a_spec_change().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    let brief = &prompts[0];
    assert!(brief.contains("Gadgets persist."), "{brief}");
    assert!(
        brief.contains(&format!(
            "### Parent issue\n\n{}, the tracking issue of the spec change {}.\n",
            cut.tracking, cut.spec_change
        )),
        "{brief}"
    );
    assert!(!brief.contains("Tickets for the spec change"), "{brief}");
    assert!(!brief.contains("List them."), "{brief}");
}

#[tokio::test]
async fn a_ticket_cut_from_a_spec_change_is_briefed_with_its_tracking_issues_comments() {
    let (world, _) = implemented_a_ticket_cut_from_a_spec_change().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    let brief = &prompts[0];
    let parent = brief.find("### Parent issue").expect(brief);
    let design = brief
        .find("Two slices: storing, then listing.")
        .expect(brief);
    assert!(parent < design, "{brief}");
    assert!(
        brief[parent..design].contains(&format!("**@{LINEWRIGHT}**")),
        "{brief}"
    );
}

#[tokio::test]
async fn a_ticket_cut_from_a_spec_change_is_briefed_with_the_spec_changes_spec_diff() {
    let (world, _) = implemented_a_ticket_cut_from_a_spec_change().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    assert_briefed_with_the_spec_diff(&prompts[0]);
}

/// A parent issue a person opened, whose comment holds a design, made the
/// parent of `ticket`.
async fn person_parented(world: &World, ticket: Number) -> Number {
    use linewright::store::Store;
    let parent = world
        .store
        .open_issue("maintainer", "Gadgets, all of them", "", &[]);
    world
        .store
        .comment_as("designer", parent, "Store them in one table.")
        .unwrap();
    world.store.add_sub_issue(parent, ticket).await.unwrap();
    parent
}

/// That `brief` holds the comment of the parent from [`person_parented`],
/// with its author, after the pointer to it.
fn assert_briefed_with_the_parents_comment(brief: &str) {
    let heading = brief.find("### Parent issue").expect(brief);
    let comment = brief.find("**@designer**").expect(brief);
    assert!(heading < comment, "{brief}");
    assert!(
        brief[comment..].contains("Store them in one table."),
        "{brief}"
    );
}

#[tokio::test]
async fn a_ticket_a_person_parented_is_briefed_with_its_parent_its_comments_and_no_spec_change() {
    let mut world = World::new().await;
    implement(&world);
    let ticket = world.store.open_issue(
        "maintainer",
        "Gadgets",
        "",
        &["ready-for-agent", "linewright:designed"],
    );
    let parent = person_parented(&world, ticket).await;

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    assert!(
        prompts[0].contains(&format!("### Parent issue\n\n{parent}.\n")),
        "{}",
        prompts[0]
    );
    assert_briefed_with_the_parents_comment(&prompts[0]);
    assert!(!prompts[0].contains("spec change #"), "{}", prompts[0]);
    assert!(!prompts[0].contains("```diff"), "{}", prompts[0]);
}

#[tokio::test]
async fn a_ticket_with_no_parent_is_briefed_with_no_parent_issue() {
    let mut world = World::new().await;

    world.implement_a_ticket().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 1);
    assert!(!prompts[0].contains("Parent issue"), "{}", prompts[0]);
    assert!(
        !prompts[0].contains("parent issue's comments"),
        "{}",
        prompts[0]
    );
    assert!(!prompts[0].contains("```diff"), "{}", prompts[0]);
}

#[tokio::test]
async fn sessions_on_a_tickets_pull_request_are_briefed_with_its_parent_issue_and_its_comments() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    let parent = person_parented(&world, ticket).await;
    world
        .store
        .comment_as("maintainer", pr, "Rename it.")
        .unwrap();
    world.harness.on(WorkKind::Rework, |call| {
        commit_in(
            &call.spec.dir,
            &[("src/renamed.txt", "renamed\n")],
            "Rename",
        );
        Ok(json!({"outcome": "done", "comment": "Renamed."}).to_string())
    });

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Rework);
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains(&format!("### Parent issue\n\n{parent}.\n")),
        "{}",
        prompts[0]
    );
    assert_briefed_with_the_parents_comment(&prompts[0]);
    assert!(!prompts[0].contains("```diff"), "{}", prompts[0]);
}

#[tokio::test]
async fn sessions_on_the_pull_request_of_a_ticket_cut_from_a_spec_change_are_briefed_with_its_spec_diff()
 {
    let mut world = World::new().await;
    let (_, pr) = world.spec_change_ticket_in_review().await;
    world.rework_asked(pr);

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Rework);
    assert_eq!(prompts.len(), 1);
    assert_briefed_with_the_spec_diff(&prompts[0]);
}

use linewright::domain::{CheckState, Login, Number, ReviewState};
#[tokio::test]
async fn a_green_pull_request_is_marked_ready_for_review_with_no_one_asked_to_review_it() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .set_checks(pr, vec![check("test", CheckState::Passed, "")])
        .await
        .unwrap();

    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(
        view.state,
        linewright::domain::PullState::Open { draft: false }
    );
    assert_eq!(world.store.requested_reviewers(pr), []);
}

#[tokio::test]
async fn review_comments_are_addressed_answered_in_their_threads_and_review_is_requested_again() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world.ready_for_review(pr).await;
    world
        .store
        .review_as(
            "maintainer",
            pr,
            ReviewState::ChangesRequested,
            "Close.",
            &[("src/x.txt", "Rename this.")],
        )
        .unwrap();
    world.harness.on(WorkKind::Rework, |call| {
        assert!(call.spec.prompt.contains("Rename this."));
        commit_in(
            &call.spec.dir,
            &[("src/renamed.txt", "renamed\n")],
            "Rename",
        );
        let thread = format!("T{}-1", 2);
        Ok(json!({
            "outcome": "done",
            "comment": "Renamed as asked.",
            "replies": [{"thread": thread, "body": "Renamed.", "resolve": true}]
        })
        .to_string())
    });

    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(
        world
            .store
            .file(&view.head_ref, "src/renamed.txt")
            .await
            .unwrap(),
        "renamed\n"
    );
    let thread = &view.threads[0];
    assert_eq!(thread.comments.last().unwrap().body, "Renamed.");
    assert_eq!(view.comments.last().unwrap().body, "Renamed as asked.");
    assert_eq!(
        world.store.requested_reviewers(pr),
        [Login::new("maintainer")]
    );
    assert_eq!(world.harness.prompts(WorkKind::Rework).len(), 1);
    let _ = ticket;

    world.poll().await;

    assert_eq!(
        world.harness.prompts(WorkKind::Rework).len(),
        1,
        "answered feedback starts nothing"
    );
}

#[tokio::test]
async fn a_rework_requests_review_from_a_person_whose_feedback_arrived_while_it_worked() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .comment_as("reviewer", pr, "Add a changelog entry.")
        .unwrap();
    let store = world.store.clone();
    world.harness.on(WorkKind::Rework, move |call| {
        if call.followup.is_none() {
            commit_in(
                &call.spec.dir,
                &[("CHANGELOG.md", "- thing\n")],
                "Changelog",
            );
            store
                .comment_as("designer", pr, "Also update the README.")
                .unwrap();
            return Ok(json!({"outcome": "done", "comment": "Added the entry."}).to_string());
        }
        commit_in(
            &call.spec.dir,
            &[("README.md", "# Widgets, updated\n")],
            "README",
        );
        Ok(
            json!({"outcome": "done", "comment": "Added the entry and updated the README."})
                .to_string(),
        )
    });

    world.poll().await;

    assert_eq!(
        world.store.requested_reviewers(pr),
        [Login::new("designer"), Login::new("reviewer")]
    );
}

async fn ticket_a_persons_pull_request_closes(world: &World, claimed: bool) -> (Number, Number) {
    implement(world);
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    if claimed {
        world
            .store
            .assign_as("maintainer", ticket, LINEWRIGHT)
            .unwrap();
    }
    let theirs = world.persons_pull_closing(ticket).await;
    (ticket, theirs)
}

#[tokio::test]
async fn a_ticket_a_persons_open_pull_request_closes_is_neither_claimed_nor_implemented() {
    let mut world = World::new().await;
    let (ticket, _) = ticket_a_persons_pull_request_closes(&world, false).await;

    world.poll().await;

    assert_eq!(world.issue(ticket).assignees, []);
    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 0);
}

#[tokio::test]
async fn a_claimed_ticket_a_persons_open_pull_request_closes_is_not_implemented() {
    let mut world = World::new().await;
    ticket_a_persons_pull_request_closes(&world, true).await;

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 0);
}

#[tokio::test]
async fn a_ticket_is_claimed_and_implemented_once_a_persons_pull_request_closing_it_is_closed_unmerged()
 {
    let mut world = World::new().await;
    let (ticket, theirs) = ticket_a_persons_pull_request_closes(&world, false).await;
    world.poll().await;

    world.store.close_pull_as(theirs).unwrap();
    world.poll().await;

    assert_eq!(world.issue(ticket).assignees, [Login::new(LINEWRIGHT)]);
    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
}

/// A ticket the factory implemented in a pull request that also closes
/// another issue, which the maintainer then labels `ready-for-agent`, with
/// the pull request's CI failing: the ticket, the other issue and the pull
/// request.
async fn pull_request_closing_another_issue(world: &mut World) -> (Number, Number, Number) {
    let other = world
        .store
        .open_issue("maintainer", "Old invoices", "Please.", &[]);
    let (ticket, pr) = world.ticket_in_review_closing(other).await;
    world
        .store
        .label_as("maintainer", other, "ready-for-agent")
        .unwrap();
    world
        .store
        .label_as("maintainer", other, "linewright:designed")
        .unwrap();
    world
        .store
        .set_checks(pr, vec![check("test", CheckState::Failed, "boom")])
        .await
        .unwrap();
    world.harness.on(WorkKind::FixCi, |_| {
        Ok(json!({"outcome": "done", "comment": "Fixed."}).to_string())
    });
    (ticket, other, pr)
}

#[tokio::test]
async fn a_pull_request_whose_ticket_is_unlabeled_and_unassigned_is_still_worked() {
    let mut world = World::new().await;
    let (ticket, other, _) = pull_request_closing_another_issue(&mut world).await;
    world.store.unlabel_as(ticket, "ready-for-agent").unwrap();
    world
        .store
        .unassign_as("maintainer", ticket, LINEWRIGHT)
        .unwrap();

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::FixCi).len(), 1);
    assert_eq!(world.issue(other).assignees, []);
    assert_eq!(world.issue(ticket).assignees, []);
    assert_eq!(labels(&world.issue(ticket)), ["linewright:designed"]);
    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 0);
}

#[tokio::test]
async fn a_pull_request_is_worked_while_each_issue_it_closes_waits_for_it() {
    let mut world = World::new().await;
    let (_, other, _) = pull_request_closing_another_issue(&mut world).await;

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::FixCi).len(), 1);
    assert_eq!(world.issue(other).assignees, []);
    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 0);
}

#[tokio::test]
async fn a_pull_request_closed_unmerged_hands_back_its_claimed_ticket_and_an_unclaimed_issue_it_closed_goes_on()
 {
    let mut world = World::new().await;
    let (ticket, other, pr) = pull_request_closing_another_issue(&mut world).await;
    world.store.close_pull_as(pr).unwrap();

    world.poll().await;

    assert!(labels(&world.issue(ticket)).contains(&"ready-for-human"));
    assert_eq!(
        labels(&world.issue(other)),
        ["linewright:designed", "ready-for-agent"]
    );
    assert_eq!(world.issue(other).assignees, [Login::new(LINEWRIGHT)]);
    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
}

fn refusals(world: &World, issue: Number, pr: Number) -> usize {
    let said = format!("{pr} was closed without merging");
    world
        .issue(issue)
        .comments
        .iter()
        .filter(|c| c.body.contains(&said))
        .count()
}

#[tokio::test]
async fn a_pull_request_closed_unmerged_hands_back_each_claimed_ticket_it_closes_once_then_relabeling_starts_afresh()
 {
    let mut world = World::new().await;
    let (ticket, other, pr) = pull_request_closing_another_issue(&mut world).await;
    world
        .store
        .assign_as("maintainer", other, LINEWRIGHT)
        .unwrap();
    world.clock.advance(TimeDelta::minutes(1));
    world.store.close_pull_as(pr).unwrap();
    world.clock.advance(TimeDelta::minutes(1));

    world.poll().await;
    world.poll().await;

    for issue in [ticket, other] {
        assert_eq!(labels(&world.issue(issue)), ["ready-for-human"]);
        assert_eq!(world.issue(issue).assignees, []);
        assert_eq!(refusals(&world, issue, pr), 1);
    }

    implement(&world);
    design_issues(&world);
    world.clock.advance(TimeDelta::minutes(1));
    for issue in [ticket, other] {
        world
            .store
            .label_as("maintainer", issue, "ready-for-agent")
            .unwrap();
    }
    world.poll().await;
    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Design).len(), 2);
    for issue in [ticket, other] {
        assert_eq!(refusals(&world, issue, pr), 1);
        assert_eq!(world.issue(issue).assignees, [Login::new(LINEWRIGHT)]);
    }
    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 2);
    assert_eq!(world.store.pulls().len(), 3);
}

#[tokio::test]
async fn a_ticket_whose_pull_request_is_edited_not_to_close_it_is_worked_again_beside_it() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world
        .store
        .edit_body(pr, "Does it, but not that.")
        .await
        .unwrap();

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
    let pulls = world.store.pulls();
    assert_eq!(pulls.len(), 2);
    let (old, new) = (
        world.store.snapshot_pull(pulls[0]).await,
        world.store.snapshot_pull(pulls[1]).await,
    );
    assert!(old.is_open() && new.is_open());
    assert_ne!(old.head_ref, new.head_ref);
    assert_eq!(new.closes, [ticket]);
    let prompt = &world.harness.prompts(WorkKind::Implement)[0];
    assert!(prompt.contains(&new.head_ref), "{prompt}");
}

#[tokio::test]
async fn a_run_whose_pull_request_closes_while_it_works_pushes_and_records_nothing() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .comment_as("maintainer", pr, "Rename it.")
        .unwrap();
    let store = world.store.clone();
    world.harness.on(WorkKind::Rework, move |call| {
        commit_in(
            &call.spec.dir,
            &[("src/renamed.txt", "renamed\n")],
            "Rename",
        );
        store.close_pull_as(pr).unwrap();
        Ok(json!({"outcome": "done", "comment": "Renamed as asked."}).to_string())
    });

    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert!(
        world
            .store
            .file(&view.head_ref, "src/renamed.txt")
            .await
            .is_err()
    );
    assert!(!view.comments.iter().any(|c| c.body == "Renamed as asked."));
}

#[tokio::test]
async fn feedback_posted_while_the_session_works_is_addressed_before_the_push() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .comment_as("maintainer", pr, "Add a changelog entry.")
        .unwrap();
    let store = world.store.clone();
    world.harness.on(WorkKind::Rework, move |call| {
        if call.followup.is_none() {
            commit_in(
                &call.spec.dir,
                &[("CHANGELOG.md", "- thing\n")],
                "Changelog",
            );
            store
                .comment_as("maintainer", pr, "Also update the README.")
                .unwrap();
            return Ok(json!({"outcome": "done", "comment": "Added the entry."}).to_string());
        }
        assert!(call.followup.unwrap().contains("Also update the README."));
        commit_in(
            &call.spec.dir,
            &[("README.md", "# Widgets, updated\n")],
            "README",
        );
        Ok(
            json!({"outcome": "done", "comment": "Added the entry and updated the README."})
                .to_string(),
        )
    });

    world.poll().await;
    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(
        world.store.file(&view.head_ref, "README.md").await.unwrap(),
        "# Widgets, updated\n"
    );
    assert_eq!(
        world.harness.prompts(WorkKind::Rework).len(),
        2,
        "one session, resumed once, and no second rework"
    );
}

#[tokio::test]
async fn the_answer_to_feedback_posted_while_the_session_works_has_its_own_follow_ups() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .comment_as("maintainer", pr, "Add a changelog entry.")
        .unwrap();
    let store = world.store.clone();
    let answers = Arc::new(Mutex::new(0));
    world.harness.on(WorkKind::Rework, move |call| {
        let answer = {
            let mut answers = answers.lock().unwrap();
            *answers += 1;
            *answers
        };
        match answer {
            // Each a result that does not parse, the test factory's one
            // follow-up spent on it.
            1 | 3 => Ok("Done, I think.".into()),
            2 => {
                commit_in(
                    &call.spec.dir,
                    &[("CHANGELOG.md", "- thing\n")],
                    "Changelog",
                );
                store
                    .comment_as("maintainer", pr, "Also update the README.")
                    .unwrap();
                Ok(json!({"outcome": "done", "comment": "Added the entry."}).to_string())
            }
            _ => {
                commit_in(
                    &call.spec.dir,
                    &[("README.md", "# Widgets, updated\n")],
                    "README",
                );
                Ok(
                    json!({"outcome": "done", "comment": "Added the entry and updated the README."})
                        .to_string(),
                )
            }
        }
    });

    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(
        world.store.file(&view.head_ref, "README.md").await.unwrap(),
        "# Widgets, updated\n"
    );
    assert!(
        view.comments
            .iter()
            .any(|c| c.body == "Added the entry and updated the README.")
    );
}

#[tokio::test]
async fn a_rework_that_needed_no_change_answers_its_feedback_also_after_a_restart() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .comment_as("coverage[bot]", pr, "Coverage dropped 0.1%.")
        .unwrap();
    world.harness.on(WorkKind::Rework, |_| {
        Ok(json!({"outcome": "done", "comment": "No change needed: noise."}).to_string())
    });

    world.poll().await;
    world.poll().await;
    world.restart(|_| {}).await;
    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Rework).len(), 1);
}

/// A slot for what `read` sees in the store, and the hook that fills it.
fn seen<T: Send + 'static>(
    world: &World,
    read: impl FnOnce(&LocalStore) -> T + Send + 'static,
) -> (Arc<Mutex<Option<T>>>, impl FnOnce() + Send + 'static) {
    let slot = Arc::new(Mutex::new(None));
    let (store, fill) = (world.store.clone(), slot.clone());
    (slot, move || *fill.lock().unwrap() = Some(read(&store)))
}

fn taken<T>(slot: &Mutex<Option<T>>) -> T {
    slot.lock().unwrap().take().expect("the hook ran")
}

/// Checks that the tickets of the cut of [`tickets_script`] were linked but
/// not ready when its design comment landed on their tracking issue, the
/// first in `at_comment`, and ready when the item they were cut from was
/// labeled designed. Returns the tracking issue as it was at the comment.
fn readied_after_their_design<'a>(at_comment: &'a [Issue], at_designed: &[Issue]) -> &'a Issue {
    let [tracking, store_ticket, list_ticket] = at_comment else {
        panic!("expected a tracking issue and two tickets, got {at_comment:#?}");
    };
    assert!(
        last_comment(tracking).starts_with("A gadget store, and a listing that reads it."),
        "{tracking:#?}"
    );
    assert_eq!(list_ticket.blocked_by[0].number, store_ticket.number);
    assert_eq!(labels(store_ticket), Vec::<&str>::new());
    assert_eq!(labels(list_ticket), Vec::<&str>::new());
    let [_, tickets @ ..] = at_designed else {
        panic!("expected a tracking issue and its tickets, got {at_designed:#?}");
    };
    assert_eq!(tickets.len(), 2, "{tickets:#?}");
    for ticket in tickets {
        assert_eq!(labels(ticket), ["linewright:designed", "ready-for-agent"]);
    }
    tracking
}

#[tokio::test]
async fn the_tickets_of_a_spec_change_are_readied_only_once_their_design_is_on_the_tracking_issue()
{
    let mut world = World::new().await;
    tickets_script(&world);
    let spec_pr = world
        .store
        .merge_change(
            "maintainer",
            "Gadgets",
            "Adds gadgets.",
            &[("docs/spec.md", "# Spec\n\nGadgets exist.\n")],
        )
        .await
        .unwrap();
    let (at_comment, on_comment) = seen(&world, move |store| {
        (store.issues(), store.pull_labels(spec_pr))
    });
    world.store.after_linewrights_next_comment(on_comment);
    let (at_designed, on_designed) = seen(&world, LocalStore::issues);
    world
        .store
        .after_labeling(spec_pr, "linewright:designed", on_designed);

    world.poll().await;

    let (at_comment, spec_labels) = taken(&at_comment);
    readied_after_their_design(&at_comment, &taken(&at_designed));
    assert!(!spec_labels.contains(&"linewright:designed".to_string()));
}

#[tokio::test]
async fn feedback_posted_while_a_rework_records_its_outcome_starts_another() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .comment_as("coverage[bot]", pr, "Coverage dropped 0.1%.")
        .unwrap();
    world.harness.on(WorkKind::Rework, |_| {
        Ok(json!({"outcome": "done", "comment": "No change needed: noise."}).to_string())
    });
    let (store, clock) = (world.store.clone(), world.clock.clone());
    world.store.after_linewrights_next_comment(move || {
        clock.advance(chrono::TimeDelta::minutes(1));
        store
            .comment_as("maintainer", pr, "Please rename `foo` too.")
            .unwrap();
    });

    world.poll().await;
    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Rework).len(), 2);
}

#[tokio::test]
async fn failing_ci_is_fixed_by_the_agent_and_after_the_last_attempt_handed_back_naming_the_check()
{
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world
        .store
        .set_checks(
            pr,
            vec![check(
                "test",
                CheckState::Failed,
                "assertion failed: widgets == 2",
            )],
        )
        .await
        .unwrap();
    world.harness.on(WorkKind::FixCi, |call| {
        assert!(call.spec.prompt.contains("assertion failed: widgets == 2"));
        let file = format!(
            "src/fix-{}.txt",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        commit_in(&call.spec.dir, &[(&file, "fix\n")], "Fix the test");
        Ok(json!({"outcome": "done", "comment": "Fixed the widget count."}).to_string())
    });

    for _ in 0..2 {
        world.poll().await;
        let view = world.store.snapshot_pull(pr).await;
        assert_eq!(
            view.comments.last().unwrap().body,
            "Fixed the widget count."
        );
        world
            .store
            .set_checks(
                pr,
                vec![check(
                    "test",
                    CheckState::Failed,
                    "assertion failed: widgets == 2",
                )],
            )
            .await
            .unwrap();
    }
    world.poll().await;

    assert_stalled(
        &world,
        ticket,
        pr,
        "`test` still failing after 2 fix attempts.",
    )
    .await;

    world.store.assign_as("maintainer", pr, LINEWRIGHT).unwrap();
    world.poll().await;

    assert_eq!(
        world.harness.prompts(WorkKind::FixCi).len(),
        3,
        "reassigning Linewright gives fresh fix attempts"
    );
}

async fn linewright_assigned(world: &World, pr: Number) -> bool {
    world
        .store
        .snapshot_pull(pr)
        .await
        .is_assigned(&Login::new(LINEWRIGHT))
}

/// A stall of `pr` reconcile decided for `reason`, its `ticket` left as it
/// was.
async fn assert_stalled(world: &World, ticket: Number, pr: Number, reason: &str) {
    assert_eq!(world.store.pull_labels(pr), ["linewright:stalled"]);
    let comment = handed_back_comment(world, ticket, pr).await;
    assert!(comment.starts_with(reason), "{comment}");
}

/// The comment `pr` was handed back with, its `ticket` left as it was.
async fn handed_back_comment(world: &World, ticket: Number, pr: Number) -> String {
    assert!(!linewright_assigned(world, pr).await);
    assert_ticket_left_as_it_was(world, ticket);
    last_pull_comment(world, pr).await
}

/// Nothing a run on its pull request handed back is written on `ticket`.
fn assert_ticket_left_as_it_was(world: &World, ticket: Number) {
    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["linewright:designed", "ready-for-agent"]);
    assert!(issue.is_assigned(&Login::new(LINEWRIGHT)));
    assert!(
        !issue
            .comments
            .iter()
            .any(|c| c.author == Login::new(LINEWRIGHT)),
        "nothing is posted on the ticket"
    );
}

#[tokio::test]
async fn ci_that_does_not_complete_in_time_hands_the_pull_request_back_and_reassigning_waits_again()
{
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world
        .store
        .set_checks(pr, vec![check("e2e", CheckState::Pending, "")])
        .await
        .unwrap();

    world
        .clock
        .advance(chrono::TimeDelta::hours(6) + chrono::TimeDelta::minutes(1));
    world.poll().await;

    assert_stalled(
        &world,
        ticket,
        pr,
        "CI did not complete within 6h: `e2e` still running.",
    )
    .await;

    world.clock.advance(chrono::TimeDelta::hours(1));
    world.store.assign_as("maintainer", pr, LINEWRIGHT).unwrap();
    world.clock.advance(chrono::TimeDelta::hours(5));
    world.poll().await;

    assert!(linewright_assigned(&world, pr).await);
    world
        .clock
        .advance(chrono::TimeDelta::hours(1) + chrono::TimeDelta::minutes(1));
    world.poll().await;
    assert!(!linewright_assigned(&world, pr).await);
}

#[tokio::test]
async fn a_pull_request_run_that_ends_unexpectedly_hands_the_pull_request_back() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world.rework_asked(pr);
    world.harness.on(WorkKind::Rework, |_| {
        Err(linewright::harness::SessionError::Failed {
            reason: "it crashed".into(),
            log_tail: String::new(),
        })
    });

    world.poll().await;

    let comment = handed_back_comment(&world, ticket, pr).await;
    assert!(comment.contains("it crashed"), "{comment}");
    world.factory.settle().await;
    assert_eq!(
        world.factory.cycle().await.unwrap(),
        [],
        "a replay ends once its pull request is handed back"
    );
    assert!(world.factory.is_idle());
}

#[tokio::test]
async fn a_stages_table_gives_its_sessions_its_settings_and_a_timeout_from_it_is_named_in_the_hand_back()
 {
    let mut world = World::with(|config| {
        config.work.flags_and_env.model = Some("sonnet".into());
        config.work.stages.insert(
            WorkKind::Implement,
            StageTable {
                settings: Partial {
                    model: Some("opus".into()),
                    effort: Some(Effort::Xhigh),
                    session_timeout: Some(std::time::Duration::from_mins(90)),
                    ..Partial::default()
                },
                ..StageTable::default()
            },
        );
    })
    .await;
    let given = Arc::new(Mutex::new(None));
    world.harness.on(WorkKind::Implement, {
        let given = given.clone();
        move |call| {
            *given.lock().unwrap() = Some((
                call.spec.model.clone(),
                call.spec.effort,
                call.spec.deadline - tokio::time::Instant::now(),
            ));
            Err(linewright::harness::SessionError::TimedOut {
                log_tail: String::new(),
            })
        }
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Gadgets",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let (model, effort, left) = given.lock().unwrap().clone().expect("implement ran");
    assert_eq!(
        (model.as_deref(), effort),
        (Some("opus"), Some(Effort::Xhigh))
    );
    assert!(
        left > std::time::Duration::from_mins(89) && left <= std::time::Duration::from_mins(90),
        "{left:?}"
    );
    let comment = last_comment(&world.issue(ticket)).to_string();
    assert!(
        comment.starts_with(
            "The session ended unexpectedly: it timed out. The work may need cutting smaller, \
             or `session-timeout` in `[stage.implement]` may need to be longer."
        ),
        "{comment}"
    );
}

#[tokio::test]
async fn a_ticket_whose_session_times_out_is_marked_stalled_until_relabeling_it_has_it_claimed() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Implement, |_| {
        Err(linewright::harness::SessionError::TimedOut {
            log_tail: String::new(),
        })
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Gadgets",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    assert_eq!(
        labels(&world.issue(ticket)),
        ["linewright:stalled", "ready-for-human"]
    );

    relabel_ready_for_agent(&world, ticket);
    design_issues(&world);
    world.poll().await;

    let issue = world.issue(ticket);
    assert!(issue.is_assigned(&Login::new(LINEWRIGHT)));
    assert!(!issue.has_label("linewright:stalled"), "{:?}", issue.labels);
}

/// A design that cuts one ticket.
fn one_ticket() -> serde_json::Value {
    json!({
        "outcome": "designed",
        "design": "A gadget store.",
        "tickets": [{"id": "store", "title": "Store gadgets", "body": "Gadgets persist.", "blocked_by": []}],
        "comment": "One slice."
    })
}

/// Has each design answer `result`, noting who was assigned to the spec
/// change while it ran.
fn design_noting_assignees(world: &World, result: serde_json::Value) -> Arc<Mutex<Vec<Login>>> {
    let seen = Arc::new(Mutex::new(vec![]));
    let (store, noted) = (world.store.clone(), seen.clone());
    world.harness.on(WorkKind::Design, move |_| {
        let spec_change = *store.pulls().last().expect("a spec change");
        *noted.lock().unwrap() = store.pull_assignees(spec_change);
        Ok(result.to_string())
    });
    seen
}

#[tokio::test]
async fn linewright_is_assigned_to_a_spec_change_while_designing_it_and_unassigned_after() {
    for result in [
        one_ticket(),
        json!({"outcome": "nothing-to-build", "reason": "Gadgets exist already."}),
    ] {
        let mut world = World::new().await;
        let seen = design_noting_assignees(&world, result);
        let spec_pr = merge_spec_change(&world).await;

        world.poll().await;

        assert_eq!(*seen.lock().unwrap(), [Login::new(LINEWRIGHT)]);
        assert_eq!(spec_change_assignees(&world, spec_pr).await, []);
        assert!(
            world
                .store
                .pull_labels(spec_pr)
                .contains(&"linewright:designed".to_string())
        );
    }
}

#[tokio::test]
async fn a_spec_change_with_nothing_to_build_says_why_and_files_no_issues() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Design, |_| {
        Ok(json!({"outcome": "nothing-to-build", "reason": "Gadgets exist already."}).to_string())
    });
    let spec_pr = merge_spec_change(&world).await;

    world.poll().await;

    assert_eq!(
        last_pull_comment(&world, spec_pr).await,
        "Gadgets exist already."
    );
    assert_eq!(world.store.issues(), []);
}

#[tokio::test]
async fn a_design_with_no_tickets_is_sent_back_to_its_session() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Design, |call| {
        let result = if call.followup.is_some_and(|f| f.contains("`/tickets`: [] has less than 1 item")) {
            one_ticket()
        } else {
            json!({"outcome": "designed", "design": "Nothing new.", "tickets": [], "comment": "None."})
        };
        Ok(result.to_string())
    });
    let spec_pr = merge_spec_change(&world).await;

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Design).len(), 2);
    assert_eq!(world.store.pull_labels(spec_pr), ["linewright:designed"]);
    let titles: Vec<String> = world.store.issues().into_iter().map(|i| i.title).collect();
    assert_eq!(titles, ["Spec change: Gadgets", "Store gadgets"]);
}

#[tokio::test]
async fn a_handed_back_spec_change_loses_its_design_label() {
    let mut world = World::new().await;
    let store = world.store.clone();
    world.harness.on(WorkKind::Design, move |_| {
        let spec_change = *store.pulls().last().expect("a spec change");
        store
            .label_as("maintainer", spec_change, "linewright:designed")
            .unwrap();
        Ok(json!({"outcome": "gave-up", "reason": "Not now."}).to_string())
    });
    let spec_pr = merge_spec_change(&world).await;

    world.poll().await;

    assert_eq!(world.store.pull_labels(spec_pr), Vec::<String>::new());
    assert!(
        last_pull_comment(&world, spec_pr)
            .await
            .contains("Not now.")
    );
}

#[tokio::test]
async fn a_design_files_its_spec_gaps_and_its_tickets() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Design, |_| {
        let mut result = one_ticket();
        result["spec_gaps"] =
            json!([{"title": "Spec: gadget colors", "body": "The spec doesn't say."}]);
        Ok(result.to_string())
    });
    let spec_pr = merge_spec_change(&world).await;

    world.poll().await;

    assert_eq!(world.store.pull_labels(spec_pr), ["linewright:designed"]);
    let titles: Vec<String> = world.store.issues().into_iter().map(|i| i.title).collect();
    assert_eq!(
        titles,
        [
            "Spec change: Gadgets",
            "Store gadgets",
            "Spec: gadget colors"
        ]
    );
}

fn design_answering(world: &World, result: Value) {
    world
        .harness
        .on(WorkKind::Design, move |_| Ok(result.to_string()));
}

#[tokio::test]
async fn a_readied_issue_is_designed_on_the_issue_and_the_next_poll_implements_it() {
    let mut world = World::new().await;
    design_answering(
        &world,
        json!({"outcome": "designed", "design": "A gadget store."}),
    );
    implement(&world);
    let issue = open_undesigned_gadgets(&world);

    world.poll().await;

    let designed = world.issue(issue);
    assert_eq!(last_comment(&designed), "A gadget store.");
    assert_eq!(
        labels(&designed),
        ["linewright:designed", "ready-for-agent"]
    );
    assert_eq!(designed.assignees, [Login::new(LINEWRIGHT)]);
    let designs = world.harness.prompts(WorkKind::Design);
    let [design] = &designs[..] else {
        panic!("expected one design, got {designs:#?}");
    };
    assert!(design.contains("Add gadgets."), "{design}");

    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Design).len(), 1);
    let implementations = world.harness.prompts(WorkKind::Implement);
    let [implementation] = &implementations[..] else {
        panic!("expected one implementation, got {implementations:#?}");
    };
    assert!(
        implementation.contains("A gadget store."),
        "{implementation}"
    );
}

/// A world holding an issue cut into the two tickets of [`tickets_script`]:
/// the issue, and its tickets in order.
async fn issue_cut_into_tickets() -> (World, Number, [Number; 2]) {
    let world = World::new().await;
    let (issue, tickets) = world.issue_cut_into_tickets().await;
    (world, issue, tickets)
}

#[tokio::test]
async fn the_tickets_of_an_issue_are_readied_only_once_their_design_is_on_it() {
    let mut world = World::new().await;
    tickets_script(&world);
    let issue = open_undesigned_gadgets(&world);
    let (at_comment, on_comment) = seen(&world, LocalStore::issues);
    world.store.after_linewrights_next_comment(on_comment);
    let (at_designed, on_designed) = seen(&world, LocalStore::issues);
    world
        .store
        .after_labeling(issue, "linewright:designed", on_designed);

    world.poll().await;

    let at_comment = taken(&at_comment);
    let tracking = readied_after_their_design(&at_comment, &taken(&at_designed));
    assert_eq!(tracking.number, issue);
    assert!(!labels(tracking).contains(&"linewright:designed"));
}

#[tokio::test]
async fn the_tickets_of_an_issue_are_implemented_in_order_briefed_with_its_design_and_it_is_not() {
    let (mut world, issue, [store_ticket, _]) = issue_cut_into_tickets().await;

    world.poll().await;

    let implementations = world.harness.prompts(WorkKind::Implement);
    let [implementation] = &implementations[..] else {
        panic!("expected one implementation, got {implementations:#?}");
    };
    assert!(
        implementation.contains(&format!("### The ticket: {store_ticket} Store gadgets")),
        "{implementation}"
    );
    assert!(
        implementation.contains(&format!("### Parent issue\n\n{issue}.\n")),
        "{implementation}"
    );
    assert!(
        implementation.contains("A gadget store, and a listing that reads it."),
        "{implementation}"
    );
    assert_eq!(world.harness.prompts(WorkKind::Design).len(), 0);
}

#[tokio::test]
async fn an_issue_cut_into_tickets_is_closed_once_all_of_them_are() {
    let (mut world, issue, [store_ticket, list_ticket]) = issue_cut_into_tickets().await;

    world.store.close_issue_as_completed(store_ticket).unwrap();
    world.poll().await;

    assert!(world.issue(issue).open);

    world.store.close_issue_as_completed(list_ticket).unwrap();
    world.poll().await;

    let closed = world.issue(issue);
    assert!(!closed.open);
    assert_eq!(last_comment(&closed), "All tickets are closed.");
}

#[tokio::test]
async fn a_cut_of_an_issue_stopped_after_its_label_is_let_go_of_as_the_cut_would_have() {
    let mut world = World::new().await;
    tickets_script(&world);
    implement_gadgets(&world);
    let issue = world.store.open_issue(
        "maintainer",
        "Gadgets",
        "Add gadgets.",
        &["ready-for-agent", "linewright:designed"],
    );
    world
        .store
        .assign_as(LINEWRIGHT, issue, LINEWRIGHT)
        .unwrap();
    let ticket = world
        .store
        .open_issue(LINEWRIGHT, "Store gadgets", "Gadgets persist.", &[]);
    world.store.add_sub_issue(issue, ticket).await.unwrap();

    world.poll().await;

    let tracking = world.issue(issue);
    assert_eq!(labels(&tracking), ["linewright:designed"]);
    assert_eq!(tracking.assignees, []);
    assert_eq!(tracking.comments, []);
    assert_eq!(world.harness.prompts(WorkKind::Design).len(), 0);
    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 0);
    assert!(world.issue(ticket).open);
}

#[tokio::test]
async fn a_cut_of_an_issue_stopped_part_way_closes_linewrights_tickets_and_their_pull_requests_and_designs_the_issue_again()
 {
    let mut world = World::new().await;
    design_answering(
        &world,
        json!({"outcome": "designed", "design": "A gadget store."}),
    );
    let issue = open_undesigned_gadgets(&world);
    world
        .store
        .assign_as(LINEWRIGHT, issue, LINEWRIGHT)
        .unwrap();
    let left = world
        .store
        .open_issue(LINEWRIGHT, "Store gadgets", "Gadgets persist.", &[]);
    let persons = world
        .store
        .open_issue("maintainer", "Paint gadgets", "", &[]);
    for sub_issue in [left, persons] {
        world.store.add_sub_issue(issue, sub_issue).await.unwrap();
    }
    let linewrights = world
        .store
        .open_pull_as(
            LINEWRIGHT,
            &format!("{}-store-gadgets", left.0),
            "Store gadgets",
            &format!("Closes {left}"),
            &[("src/gadgets.txt", "gadgets\n")],
        )
        .await
        .unwrap();
    let theirs = world.persons_pull_closing(left).await;

    world.poll().await;

    let discarded = world.issue(left);
    assert!(!discarded.open);
    assert_eq!(
        last_comment(&discarded),
        format!(
            "{issue} is being designed again, so this ticket from its earlier cut is discarded."
        )
    );
    for pr in [linewrights, theirs] {
        let pull = world.store.snapshot_pull(pr).await;
        assert!(matches!(pull.state, PullState::Closed { .. }), "{pull:#?}");
        assert_eq!(
            last_pull_comment(&world, pr).await,
            format!(
                "{issue} is being designed again, so this pull request for a ticket of its \
                 earlier cut is discarded."
            )
        );
    }
    assert_eq!(
        last_comment(&world.issue(issue)),
        format!(
            "This issue is being designed again, so the ticket {left} and pull requests \
             {linewrights} and {theirs} of its earlier cut are discarded."
        )
    );
    assert!(world.issue(persons).open);
    assert_eq!(world.harness.prompts(WorkKind::Design).len(), 0);

    world.poll().await;

    let designs = world.harness.prompts(WorkKind::Design);
    let [design] = &designs[..] else {
        panic!("expected one design, got {designs:#?}");
    };
    assert!(design.contains("is being designed again"), "{design}");
    assert!(world.issue(persons).open);
    assert_eq!(
        labels(&world.issue(issue)),
        ["linewright:designed", "ready-for-agent"]
    );
}

#[tokio::test]
async fn removing_the_design_label_from_an_issue_with_no_pull_request_designs_it_again() {
    let mut world = World::new().await;
    implement(&world);
    let issue = open_undesigned_gadgets(&world);
    world.designed_on_the_issue(issue).await;

    world
        .store
        .unlabel_as(issue, "linewright:designed")
        .unwrap();
    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Design).len(), 1);
    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 0);
    assert_eq!(
        labels(&world.issue(issue)),
        ["linewright:designed", "ready-for-agent"]
    );
}

#[tokio::test]
async fn a_handed_back_spec_change_is_designed_again_once_linewright_is_assigned_again() {
    let mut world = World::new().await;
    let spec_pr = world.spec_change_handed_back().await;
    let seen = design_noting_assignees(&world, one_ticket());

    world
        .store
        .assign_as("maintainer", spec_pr, LINEWRIGHT)
        .unwrap();
    world.poll().await;

    assert_eq!(*seen.lock().unwrap(), [Login::new(LINEWRIGHT)]);
    assert!(
        world
            .store
            .pull_labels(spec_pr)
            .contains(&"linewright:designed".to_string())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unassigning_linewright_from_a_spec_change_while_it_is_cut_cancels_the_cut() {
    let mut world = World::new().await;
    let (release, released) = std::sync::mpsc::channel::<()>();
    let released = Mutex::new(released);
    let (begin, mut begun) = tokio::sync::mpsc::unbounded_channel();
    world.harness.on(WorkKind::Design, move |_| {
        begin.send(()).unwrap();
        released.lock().unwrap().recv().unwrap();
        Err(linewright::harness::SessionError::TimedOut {
            log_tail: String::new(),
        })
    });
    let spec_pr = merge_spec_change(&world).await;
    world.factory.cycle().await.unwrap();
    begun.recv().await.unwrap();

    world
        .store
        .unassign_as("maintainer", spec_pr, LINEWRIGHT)
        .unwrap();
    world.factory.cycle().await.unwrap();
    release.send(()).unwrap();
    world.factory.settle().await;

    assert_eq!(world.store.pull_labels(spec_pr), Vec::<String>::new());
    assert_eq!(world.store.snapshot_pull(spec_pr).await.comments, []);
    world.poll().await;
    assert_eq!(world.harness.prompts(WorkKind::Design).len(), 1);
}

#[tokio::test]
async fn a_pull_request_closed_while_its_run_works_gets_no_hand_back() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world.rework_asked(pr);
    let store = world.store.clone();
    world.harness.on(WorkKind::Rework, move |_| {
        store.close_pull_as(pr).unwrap();
        Ok(json!({"outcome": "gave-up", "reason": "Cannot rename it."}).to_string())
    });

    world.poll().await;

    assert!(linewright_assigned(&world, pr).await);
    assert!(
        !world
            .store
            .snapshot_pull(pr)
            .await
            .comments
            .iter()
            .any(|c| c.author == Login::new(LINEWRIGHT))
    );
    assert!(
        !world
            .issue(ticket)
            .comments
            .iter()
            .any(|c| c.body.contains("Cannot rename it."))
    );
}

#[tokio::test]
async fn a_branch_that_changes_the_spec_is_pushed_and_its_pull_request_opened() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Implement, |call| {
        commit_in(
            &call.spec.dir,
            &[("docs/spec.md", "# Spec\n\nRewritten.\n")],
            "Edit spec",
        );
        Ok(
            json!({"outcome": "done", "pr_title": "Edit the spec", "pr_body": "Edits the spec."})
                .to_string(),
        )
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Do it",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let pr = world.store.snapshot_pull(world.store.pulls()[0]).await;
    assert_eq!(pr.title, "Edit the spec");
    assert_eq!(
        pr.state,
        linewright::domain::PullState::Open { draft: true }
    );
    assert_eq!(
        world
            .store
            .file(&pr.head_ref, "docs/spec.md")
            .await
            .unwrap(),
        "# Spec\n\nRewritten.\n"
    );
    assert!(world.issue(ticket).is_assigned(&Login::new(LINEWRIGHT)));
}

#[tokio::test]
async fn a_pull_request_linewright_opens_is_assigned_to_it() {
    let mut world = World::new().await;

    let (_, pr) = world.ticket_in_review().await;

    assert_eq!(
        world.store.snapshot_pull(pr).await.assignees,
        [Login::new(LINEWRIGHT)]
    );
}

#[tokio::test]
async fn the_pull_request_is_titled_as_the_session_wrote_and_a_blank_title_is_sent_back() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Implement, |call| {
        if call.followup.is_none() {
            commit_in(&call.spec.dir, &[("src/gadget.txt", "gadget\n")], "Add");
        }
        let title = if call.followup.is_none() {
            " "
        } else {
            " Report gadgets "
        };
        Ok(json!({"outcome": "done", "pr_title": title, "pr_body": "Gadgets."}).to_string())
    });
    world.store.open_issue(
        "maintainer",
        "Gadgets are missing",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 2);
    assert!(prompts[1].contains("`/pr_title`: \" \" does not match"));
    let pr = world.store.snapshot_pull(world.store.pulls()[0]).await;
    assert_eq!(pr.title, "Report gadgets");
}

#[tokio::test]
async fn a_spec_gap_already_filed_for_a_ticket_is_shown_to_later_sessions() {
    let mut world = World::new().await;
    let ticket = world.ticket_blocked_filing_a_spec_gap().await;
    relabeled_and_designed_again(&world, ticket).await;

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("Spec: gadget colors"));
}

#[tokio::test]
async fn a_pull_request_closed_without_merging_hands_its_ticket_back() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;

    world.store.close_pull_as(pr).unwrap();
    world.poll().await;
    world.poll().await;

    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["ready-for-human"]);
    assert_eq!(
        last_comment(&issue),
        format!("{pr} was closed without merging.")
    );
    assert_eq!(
        world.store.pulls().len(),
        1,
        "no new pull request replaces a rejected one"
    );
}

#[tokio::test]
async fn a_ticket_reopened_after_its_pull_request_merged_is_implemented_again_unrelabeled() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world.store.merge_pull(pr).await.unwrap();
    world.poll().await;
    assert!(!world.issue(ticket).open);

    world.store.reopen_issue_as(ticket).unwrap();
    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
    let next = *world.store.pulls().last().unwrap();
    assert_ne!(next, pr);
}

#[tokio::test]
async fn relabeling_a_claimed_ticket_ready_for_human_lets_go_of_it_but_not_of_its_pull_request() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;

    world
        .store
        .label_as("maintainer", ticket, "ready-for-human")
        .unwrap();
    world.poll().await;

    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["linewright:designed", "ready-for-human"]);
    assert_eq!(issue.assignees, []);
    assert_eq!(issue.comments, []);
    assert!(linewright_assigned(&world, pr).await);
}

#[tokio::test]
async fn a_ticket_without_the_required_label_assigned_to_linewright_by_hand_is_implemented() {
    let mut world = World::with(|c| c.require_label = Some("linewright".into())).await;
    implement(&world);
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.poll().await;
    assert_eq!(
        world.harness.prompts(WorkKind::Implement),
        Vec::<String>::new()
    );

    world
        .store
        .assign_as("maintainer", ticket, LINEWRIGHT)
        .unwrap();
    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
    assert!(world.store.pulls().last().is_some());
}

#[tokio::test]
async fn an_issue_assigned_to_linewright_by_hand_beside_someone_else_is_triaged() {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Triage, |_| {
        Ok(json!({
            "outcome": "ready-for-human",
            "category": "bug",
            "comment": "Is this a bug?"
        })
        .to_string())
    });
    let issue = world
        .store
        .open_issue("reporter", "It broke", "It broke.", &["needs-triage"]);
    world.store.assign_as("maintainer", issue, "alice").unwrap();
    world.poll().await;
    assert_eq!(
        world.harness.prompts(WorkKind::Triage),
        Vec::<String>::new()
    );

    world
        .store
        .assign_as("maintainer", issue, LINEWRIGHT)
        .unwrap();
    world.poll().await;
    // Letting go of the triaged issue, at the next poll, posts nothing more.
    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Triage).len(), 1);
    let issue = world.issue(issue);
    assert_eq!(issue.assignees, [Login::new("alice")]);
    assert_eq!(issue.comments.len(), 1);
    assert!(last_comment(&issue).starts_with("Is this a bug?"));
}

#[tokio::test]
async fn a_blocked_ticket_assigned_to_linewright_by_hand_is_held_assigned_until_its_blocker_closes()
{
    let mut world = World::new().await;
    implement(&world);
    let blocker = world
        .store
        .open_issue("maintainer", "First", "First.", &["ready-for-human"]);
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.add_blocked_by(ticket, blocker).await.unwrap();
    world
        .store
        .assign_as("maintainer", ticket, LINEWRIGHT)
        .unwrap();

    world.poll().await;

    assert_eq!(
        world.harness.prompts(WorkKind::Implement),
        Vec::<String>::new()
    );
    assert!(world.issue(ticket).is_assigned(&Login::new(LINEWRIGHT)));
    assert_eq!(world.issue(ticket).comments, []);

    world.store.close_issue_as_completed(blocker).unwrap();
    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
}

#[tokio::test]
async fn removing_the_label_of_a_ticket_assigned_to_linewright_by_hand_lets_go_of_it_silently() {
    let mut world = World::new().await;
    let blocker = world
        .store
        .open_issue("maintainer", "First", "First.", &["ready-for-human"]);
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.add_blocked_by(ticket, blocker).await.unwrap();
    world
        .store
        .assign_as("maintainer", ticket, LINEWRIGHT)
        .unwrap();
    world.poll().await;

    world.store.unlabel_as(ticket, "ready-for-agent").unwrap();
    world.poll().await;

    let issue = world.issue(ticket);
    assert_eq!(issue.assignees, []);
    assert_eq!(issue.comments, []);
}

#[tokio::test]
async fn relabeling_a_ticket_assigned_to_linewright_by_hand_lets_go_of_it_silently_though_its_label_was_applied_again()
 {
    let mut world = World::new().await;
    let blocker = world
        .store
        .open_issue("maintainer", "First", "First.", &["ready-for-human"]);
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.add_blocked_by(ticket, blocker).await.unwrap();
    world
        .store
        .assign_as("maintainer", ticket, LINEWRIGHT)
        .unwrap();
    world.poll().await;

    world
        .store
        .label_as("maintainer", ticket, "ready-for-agent")
        .unwrap();
    world
        .store
        .label_as("maintainer", ticket, "ready-for-human")
        .unwrap();
    world.poll().await;

    let issue = world.issue(ticket);
    assert_eq!(issue.assignees, []);
    assert_eq!(issue.comments, []);
}

#[tokio::test]
async fn an_issue_whose_label_asks_nothing_assigned_to_linewright_by_hand_is_let_go_with_one_comment()
 {
    let mut world = World::new().await;
    let issues = [
        world
            .store
            .open_issue("maintainer", "Decide", "Decide.", &["ready-for-human"]),
        world
            .store
            .open_issue("maintainer", "Ask", "Ask.", &["needs-info"]),
        world
            .store
            .open_issue("maintainer", "Unlabeled", "Hm.", &[]),
    ];
    for issue in issues {
        world
            .store
            .assign_as("maintainer", issue, LINEWRIGHT)
            .unwrap();
    }

    world.poll().await;
    world.poll().await;

    for issue in issues {
        let issue = world.issue(issue);
        assert!(issue.assignees.is_empty(), "{issue:?}");
        assert_eq!(issue.comments.len(), 1, "{issue:?}");
        assert!(
            last_comment(&issue).contains(
                "Label it `ready-for-agent` to have it implemented or `needs-triage` to have \
                 it assessed, and assign @factory again."
            ),
            "{issue:?}"
        );
    }
    assert_eq!(
        world.harness.prompts(WorkKind::Triage),
        Vec::<String>::new()
    );
    assert_eq!(
        world.harness.prompts(WorkKind::Implement),
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn reassigning_linewright_after_unassigning_it_implements_the_ticket_again_unrelabeled() {
    let mut world = World::new().await;
    implement(&world);
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world
        .store
        .assign_as("maintainer", ticket, LINEWRIGHT)
        .unwrap();
    world
        .store
        .unassign_as("maintainer", ticket, LINEWRIGHT)
        .unwrap();
    world.poll().await;
    assert_eq!(
        world.harness.prompts(WorkKind::Implement),
        Vec::<String>::new()
    );

    world
        .store
        .assign_as("maintainer", ticket, LINEWRIGHT)
        .unwrap();
    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
}

/// Relabels the handed-back ticket for a poll to start its next run, which
/// does `work` and answers `done`.
async fn worked_again(
    world: &mut World,
    ticket: Number,
    work: impl Fn(&std::path::Path) + Send + Sync + 'static,
) -> NextRun {
    let next = script_next_run(world, work);
    relabeled_and_designed_again(world, ticket).await;
    world.poll().await;
    next
}

/// What the next implement run's clone held when it started.
struct NextRun(Arc<Mutex<Option<bool>>>);

impl NextRun {
    fn held_earlier_work(&self) -> bool {
        self.0.lock().unwrap().expect("the ticket was worked again")
    }
}

fn script_next_run(
    world: &World,
    work: impl Fn(&std::path::Path) + Send + Sync + 'static,
) -> NextRun {
    let held_earlier_work = Arc::new(Mutex::new(None));
    let next = NextRun(held_earlier_work.clone());
    world.harness.on(WorkKind::Implement, move |call| {
        *held_earlier_work.lock().unwrap() = Some(call.spec.dir.join(EARLIER_WORK).exists());
        work(&call.spec.dir);
        Ok(json!({"outcome": "done", "pr_title": "Add gadgets", "pr_body": "## Summary\n\nGadgets."}).to_string())
    });
    next
}

/// Relabels the handed-back ticket `ready-for-agent`, and designs it again:
/// the hand-back took its design with it.
async fn relabeled_and_designed_again(world: &World, ticket: Number) {
    relabel_ready_for_agent(world, ticket);
    world.designed_on_the_issue(ticket).await;
}

fn finish_gadgets(dir: &std::path::Path) {
    commit_in(dir, &[("src/gadgets.txt", "gadgets\n")], "Finish gadgets");
}

#[tokio::test]
async fn a_handed_back_tickets_next_run_starts_from_its_branch_and_is_told_it_holds_earlier_work() {
    let mut world = World::new().await;
    let (ticket, branch) = world.ticket_handed_back_with_its_branch().await;

    let next = worked_again(&mut world, ticket, finish_gadgets).await;

    assert!(next.held_earlier_work());
    let prompt = &world.harness.prompts(WorkKind::Implement)[0];
    assert!(prompt.contains(&branch), "{prompt}");
    let pr = world.store.snapshot_pull(world.store.pulls()[0]).await;
    assert_eq!(pr.head_ref, branch);
    assert_eq!(
        world.store.file(&branch, EARLIER_WORK).await.unwrap(),
        "half\n"
    );
    assert_eq!(
        world.store.file(&branch, "src/gadgets.txt").await.unwrap(),
        "gadgets\n"
    );
}

#[tokio::test]
async fn a_run_that_resets_its_tickets_branch_to_the_default_branch_replaces_the_earlier_work() {
    let mut world = World::new().await;
    let (ticket, branch) = world.ticket_handed_back_with_its_branch().await;

    worked_again(&mut world, ticket, |dir| {
        git_in(dir, &["reset", "--quiet", "--hard", "origin/main"]);
        finish_gadgets(dir);
    })
    .await;

    assert!(world.store.file(&branch, EARLIER_WORK).await.is_err());
    assert_eq!(
        world.store.file(&branch, "src/gadgets.txt").await.unwrap(),
        "gadgets\n"
    );
}

/// The subjects of the commits on `branch` past `main`, oldest first.
fn subjects(world: &World, branch: &str) -> Vec<String> {
    let output = store_git(
        world,
        &[
            "log",
            "--reverse",
            "--format=%s",
            &format!("main..{branch}"),
        ],
    );
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn a_timed_out_session_leaves_its_uncommitted_edits_on_the_tickets_branch_as_an_unfinished_commit()
 {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Implement, |call| {
        start_and_leave_edits(&call.spec.dir);
        Err(linewright::harness::SessionError::TimedOut {
            log_tail: String::new(),
        })
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Gadgets",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let branch = format!("{}-gadgets", ticket.0);
    assert_eq!(subjects(&world, &branch), ["Start", UNFINISHED]);
    let issue = world.issue(ticket);
    assert!(issue.has_label("ready-for-human"), "{:?}", issue.labels);
    let comment = last_comment(&issue);
    assert!(comment.contains(&kept_on(&branch)), "{comment}");
}

#[tokio::test]
async fn a_session_that_crashes_answers_no_json_blocks_or_gives_up_leaves_its_uncommitted_edits_as_an_unfinished_commit()
 {
    use linewright::harness::SessionError;

    type Ending = fn() -> Result<String, SessionError>;
    let endings: [(Ending, &str); 4] = [
        (
            || {
                Err(SessionError::Failed {
                    reason: "it crashed".into(),
                    log_tail: String::new(),
                })
            },
            "it crashed",
        ),
        (|| Ok("Out of ideas.".into()), "Out of ideas."),
        (
            || Ok(json!({"outcome": "blocked", "question": "Should gadgets expire?"}).to_string()),
            "Should gadgets expire?",
        ),
        (
            || Ok(json!({"outcome": "gave-up", "reason": "Too hard."}).to_string()),
            "Too hard.",
        ),
    ];
    for (ending, says) in endings {
        let mut world = World::new().await;
        world.harness.on(WorkKind::Implement, move |call| {
            if call.followup.is_none() {
                start_and_leave_edits(&call.spec.dir);
            }
            ending()
        });
        let ticket = world.store.open_issue(
            "maintainer",
            "Gadgets",
            "",
            &["ready-for-agent", "linewright:designed"],
        );

        world.poll().await;

        let branch = format!("{}-gadgets", ticket.0);
        assert_eq!(subjects(&world, &branch), ["Start", UNFINISHED]);
        let issue = world.issue(ticket);
        assert!(issue.has_label("ready-for-human"), "{:?}", issue.labels);
        assert!(!issue.is_assigned(&Login::new(LINEWRIGHT)));
        let comment = last_comment(&issue);
        assert!(comment.contains(says), "{comment}");
        assert!(comment.contains(&kept_on(&branch)), "{comment}");
    }
}

/// Starts a ticket's run, which leaves edits uncommitted, and has `cancel`
/// cancel it while its session runs.
async fn cancelled_with_edits(world: &mut World, cancel: impl Fn(&World, Number)) -> Number {
    let (release, released) = std::sync::mpsc::channel::<()>();
    let released = Mutex::new(released);
    let (begin, mut begun) = tokio::sync::mpsc::unbounded_channel();
    world.harness.on(WorkKind::Implement, move |call| {
        start_and_leave_edits(&call.spec.dir);
        begin.send(()).unwrap();
        released.lock().unwrap().recv().unwrap();
        Ok(json!({"outcome": "gave-up", "reason": "Released."}).to_string())
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Gadgets",
        "",
        &["ready-for-agent", "linewright:designed"],
    );
    world.factory.cycle().await.unwrap();
    begun.recv().await.unwrap();

    cancel(world, ticket);
    world.factory.cycle().await.unwrap();
    release.send(()).unwrap();
    world.factory.settle().await;
    ticket
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_cancelled_by_closing_its_ticket_pushes_its_work_and_the_branch_is_deleted_after() {
    let mut world = World::new().await;

    let ticket = cancelled_with_edits(&mut world, |world, ticket| {
        world.store.close_issue_as_completed(ticket).unwrap();
    })
    .await;

    let branch = format!("{}-gadgets", ticket.0);
    assert_eq!(subjects(&world, &branch), ["Start", UNFINISHED]);
    world.poll().await;
    assert!(!has_branch(&world, &branch));
}

#[tokio::test]
async fn a_run_that_ends_with_nothing_committed_or_uncommitted_pushes_nothing_and_names_no_branch()
{
    let mut world = World::new().await;
    world.harness.on(WorkKind::Implement, |call| {
        std::fs::write(call.spec.dir.join("build.log"), "noise\n").unwrap();
        std::fs::write(call.spec.dir.join(".git/info/exclude"), "*.log\n").unwrap();
        Err(linewright::harness::SessionError::TimedOut {
            log_tail: String::new(),
        })
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Gadgets",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let branch = format!("{}-gadgets", ticket.0);
    assert!(!has_branch(&world, &branch));
    let issue = world.issue(ticket);
    let comment = last_comment(&issue);
    assert!(!comment.contains(&branch), "{comment}");
}

#[tokio::test]
async fn a_done_answer_that_never_folds_in_the_unfinished_commit_opens_no_pull_request() {
    let mut world = World::new().await;
    let (ticket, branch) = world.ticket_timed_out_with_edits().await;
    world.harness.on(WorkKind::Implement, |_| {
        Ok(
            json!({"outcome": "done", "pr_title": "Add gadgets", "pr_body": "Gadgets."})
                .to_string(),
        )
    });

    relabeled_and_designed_again(&world, ticket).await;
    world.poll().await;

    assert_eq!(world.store.pulls(), []);
    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["linewright:stalled", "ready-for-human"]);
    let comment = last_comment(&issue);
    assert!(comment.contains(&kept_on(&branch)), "{comment}");
    assert_eq!(subjects(&world, &branch), ["Start", UNFINISHED]);
}

#[tokio::test]
async fn a_run_that_fails_on_its_infrastructure_is_retried_from_its_unfinished_commit_and_names_the_branch()
 {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Implement, |call| {
        if call.spec.dir.join("src/partial.txt").exists() {
            let unfinished = std::process::Command::new("git")
                .arg("-C")
                .arg(&call.spec.dir)
                .args(["log", "-1", "--format=%h"])
                .output()
                .unwrap()
                .stdout;
            let unfinished = String::from_utf8(unfinished).unwrap();
            assert!(call.spec.prompt.contains(unfinished.trim()));
        } else {
            start_and_leave_edits(&call.spec.dir);
        }
        Err(linewright::harness::SessionError::Infrastructure(
            "the harness's API failed".into(),
        ))
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Gadgets",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let branch = format!("{}-gadgets", ticket.0);
    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 2);
    assert_eq!(subjects(&world, &branch), ["Start", UNFINISHED]);
    let comment = last_comment(&world.issue(ticket)).to_string();
    assert!(
        comment.starts_with("The run failed twice on its infrastructure")
            && comment.ends_with(&kept_on(&branch)),
        "{comment}"
    );
}

#[tokio::test]
async fn a_timed_out_rework_leaves_its_uncommitted_edits_on_the_pull_requests_branch_as_an_unfinished_commit()
 {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world.rework_asked(pr);
    world.harness.on(WorkKind::Rework, |call| {
        rename_and_leave_edits(&call.spec.dir);
        timed_out()
    });

    world.poll().await;

    let branch = world.store.snapshot_pull(pr).await.head_ref;
    assert_eq!(
        subjects(&world, &branch),
        ["Do the ticket", "Rename", UNFINISHED]
    );
    let comment = handed_back_comment(&world, ticket, pr).await;
    assert!(comment.contains(&kept_on(&branch)), "{comment}");
}

#[tokio::test]
async fn a_timed_out_ci_fix_leaves_its_uncommitted_edits_on_the_pull_requests_branch_as_an_unfinished_commit()
 {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world
        .store
        .set_checks(pr, vec![check("test", CheckState::Failed, "it failed")])
        .await
        .unwrap();
    world.harness.on(WorkKind::FixCi, |call| {
        rename_and_leave_edits(&call.spec.dir);
        timed_out()
    });

    world.poll().await;

    let branch = world.store.snapshot_pull(pr).await.head_ref;
    assert_eq!(
        subjects(&world, &branch),
        ["Do the ticket", "Rename", UNFINISHED]
    );
    let comment = handed_back_comment(&world, ticket, pr).await;
    assert!(comment.contains(&kept_on(&branch)), "{comment}");
}

#[tokio::test]
async fn a_rework_that_crashes_answers_no_json_blocks_or_gives_up_leaves_its_uncommitted_edits_as_an_unfinished_commit()
 {
    use linewright::harness::SessionError;

    type Ending = fn() -> Result<String, SessionError>;
    let endings: [(Ending, &str); 4] = [
        (
            || {
                Err(SessionError::Failed {
                    reason: "it crashed".into(),
                    log_tail: String::new(),
                })
            },
            "The session ended unexpectedly: it crashed",
        ),
        (
            || Ok("Out of ideas.".into()),
            "The session's result still could not be used",
        ),
        (
            || {
                Ok(
                    json!({"outcome": "blocked", "question": "Should it keep its old name?"})
                        .to_string(),
                )
            },
            "Should it keep its old name?",
        ),
        (
            || Ok(json!({"outcome": "gave-up", "reason": "Too hard."}).to_string()),
            "Too hard.",
        ),
    ];
    for (ending, says) in endings {
        let mut world = World::new().await;
        let (_, pr) = world.ticket_in_review().await;
        world.rework_asked(pr);
        world.harness.on(WorkKind::Rework, move |call| {
            if call.followup.is_none() {
                rename_and_leave_edits(&call.spec.dir);
            }
            ending()
        });

        world.poll().await;

        let branch = world.store.snapshot_pull(pr).await.head_ref;
        assert_eq!(
            subjects(&world, &branch),
            ["Do the ticket", "Rename", UNFINISHED]
        );
        assert!(!linewright_assigned(&world, pr).await);
        let comment = last_pull_comment(&world, pr).await;
        assert!(comment.contains(says), "{comment}");
        assert!(comment.contains(&kept_on(&branch)), "{comment}");
    }
}

/// Starts a rework, which leaves edits uncommitted, and has `cancel` cancel
/// it while its session runs.
async fn rework_cancelled_with_edits(world: &mut World, cancel: impl Fn(&World, Number)) -> Number {
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .comment_as("maintainer", pr, "Rename it.")
        .unwrap();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let released = Mutex::new(released);
    let (begin, mut begun) = tokio::sync::mpsc::unbounded_channel();
    world.harness.on(WorkKind::Rework, move |call| {
        rename_and_leave_edits(&call.spec.dir);
        begin.send(()).unwrap();
        released.lock().unwrap().recv().unwrap();
        Ok(json!({"outcome": "gave-up", "reason": "Released."}).to_string())
    });
    world.factory.cycle().await.unwrap();
    begun.recv().await.unwrap();

    cancel(world, pr);
    world.factory.cycle().await.unwrap();
    release.send(()).unwrap();
    world.factory.settle().await;
    pr
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rework_cancelled_by_unassigning_linewright_keeps_its_edits_as_an_unfinished_commit_and_posts_and_starts_nothing()
 {
    let mut world = World::new().await;

    let pr = rework_cancelled_with_edits(&mut world, |world, pr| {
        world
            .store
            .unassign_as("maintainer", pr, LINEWRIGHT)
            .unwrap();
    })
    .await;
    world
        .store
        .comment_as("maintainer", pr, "I'll take it from here.")
        .unwrap();
    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(
        subjects(&world, &view.head_ref),
        ["Do the ticket", "Rename", UNFINISHED]
    );
    assert!(
        !view
            .comments
            .iter()
            .any(|c| c.author == Login::new(LINEWRIGHT)),
        "a cancelled run posts nothing"
    );
    assert_eq!(world.harness.prompts(WorkKind::Rework).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rework_cancelled_by_closing_its_pull_request_pushes_nothing() {
    let mut world = World::new().await;

    let pr = rework_cancelled_with_edits(&mut world, |world, pr| {
        world.store.close_pull_as(pr).unwrap();
    })
    .await;

    let branch = world.store.snapshot_pull(pr).await.head_ref;
    assert_eq!(subjects(&world, &branch), ["Do the ticket"]);
}

#[tokio::test]
async fn a_rework_whose_pull_request_closes_before_it_times_out_pushes_and_posts_nothing() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.rework_asked(pr);
    let store = world.store.clone();
    world.harness.on(WorkKind::Rework, move |call| {
        rename_and_leave_edits(&call.spec.dir);
        store.close_pull_as(pr).unwrap();
        timed_out()
    });

    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(subjects(&world, &view.head_ref), ["Do the ticket"]);
    assert!(
        !view
            .comments
            .iter()
            .any(|c| c.author == Login::new(LINEWRIGHT))
    );
}

#[tokio::test]
async fn a_stopped_rework_leaves_commits_a_person_pushed_while_it_worked_and_says_its_push_was_rejected()
 {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.rework_asked(pr);
    let branch = world.store.snapshot_pull(pr).await.head_ref;
    let store = world.store.clone();
    let pushed_to = branch.clone();
    world.harness.on(WorkKind::Rework, move |call| {
        rename_and_leave_edits(&call.spec.dir);
        let (store, pushed_to) = (store.clone(), pushed_to.clone());
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                store
                    .commit(
                        "alice",
                        &pushed_to,
                        &[("src/alice.txt", "alice\n")],
                        "Alice's fix",
                    )
                    .await
                    .unwrap();
            });
        })
        .join()
        .unwrap();
        timed_out()
    });

    world.poll().await;

    assert_eq!(subjects(&world, &branch), ["Do the ticket", "Alice's fix"]);
    let comment = last_pull_comment(&world, pr).await;
    assert!(
        comment.contains(&format!(
            "Keeping the work so far on the branch `{branch}` failed"
        )),
        "{comment}"
    );
}

#[tokio::test]
async fn the_next_rework_is_told_to_fold_in_the_unfinished_commit_and_its_done_is_sent_back_until_it_does()
 {
    let mut world = World::new().await;
    let (_, pr, branch) = world.rework_timed_out_with_edits().await;
    let unfinished = store_git(&world, &["log", "-1", "--format=%h", &branch]).stdout;
    let unfinished = String::from_utf8(unfinished).unwrap();
    let unfinished = unfinished.trim().to_string();
    world.harness.on(WorkKind::Rework, |call| {
        if call.followup.is_some() {
            git_in(&call.spec.dir, &["reset", "--quiet", "--soft", "HEAD~1"]);
            git_in(&call.spec.dir, &["commit", "--quiet", "-m", "Rename more"]);
        }
        Ok(json!({"outcome": "done", "comment": "Renamed."}).to_string())
    });

    world.store.assign_as("maintainer", pr, LINEWRIGHT).unwrap();
    world
        .store
        .comment_as("maintainer", pr, "Carry on.")
        .unwrap();
    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Rework);
    assert!(prompts[0].contains(&unfinished), "{}", prompts[0]);
    assert!(prompts[1].contains(&unfinished), "{}", prompts[1]);
    assert_eq!(prompts.len(), 2);
    assert_eq!(
        subjects(&world, &branch),
        ["Do the ticket", "Rename", "Rename more"]
    );
    assert_eq!(last_pull_comment(&world, pr).await, "Renamed.");
}

#[tokio::test]
async fn a_stopped_rework_with_nothing_new_but_its_update_pushes_nothing() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    let before = world.store.snapshot_pull(pr).await.head_sha;
    world
        .store
        .merge_change(
            "maintainer",
            "Other work",
            "",
            &[("src/other.txt", "other\n")],
        )
        .await
        .unwrap();
    world.rework_asked(pr);
    world.harness.on(WorkKind::Rework, |_| timed_out());
    let branch = world.store.snapshot_pull(pr).await.head_ref;

    world.poll().await;

    assert_eq!(
        String::from_utf8(store_git(&world, &["rev-parse", &branch]).stdout)
            .unwrap()
            .trim(),
        before
    );
    assert_eq!(world.harness.prompts(WorkKind::Rework).len(), 1);
    let comment = handed_back_comment(&world, ticket, pr).await;
    assert!(!comment.contains(&kept_on(&branch)), "{comment}");
}

#[tokio::test]
async fn closing_a_ticket_with_no_open_pull_request_deletes_its_branch() {
    let mut world = World::new().await;
    let (ticket, branch) = world.ticket_handed_back_with_its_branch().await;

    world.store.close_issue_as_completed(ticket).unwrap();
    world.poll().await;

    assert!(!has_branch(&world, &branch));
}

#[tokio::test]
async fn closing_a_ticket_whose_pull_request_is_open_leaves_its_branch() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    let branch = world.store.snapshot_pull(pr).await.head_ref;

    world.store.close_issue_as_completed(ticket).unwrap();
    world.poll().await;

    assert!(has_branch(&world, &branch));
}

#[tokio::test]
async fn a_branch_named_for_a_closed_issue_whose_head_a_person_made_is_left() {
    let mut world = World::new().await;
    let issue = world.store.open_issue("maintainer", "Gadgets", "", &[]);
    let branch = format!("{}-gadgets", issue.0);
    world
        .store
        .commit(
            "alice",
            &branch,
            &[("src/gadgets.txt", "gadgets\n")],
            "Gadgets",
        )
        .await
        .unwrap();

    world.store.close_issue_as_completed(issue).unwrap();
    world.poll().await;

    assert!(has_branch(&world, &branch));
}

#[tokio::test]
async fn only_a_branch_named_as_linewright_names_a_closed_tickets_branch_is_deleted() {
    let mut world = World::new().await;
    let issue = world.store.open_issue("maintainer", "Gadgets", "", &[]);
    let named = format!("{}-gadgets", issue.0);
    let others = [
        format!("0{}-gadgets", issue.0),
        format!("{}--gadgets", issue.0),
        format!("{}-Gadgets", issue.0),
        "gadgets".to_string(),
    ];
    for branch in others.iter().chain([&named]) {
        world
            .store
            .commit(
                LINEWRIGHT,
                branch,
                &[("src/gadgets.txt", "gadgets\n")],
                "Gadgets",
            )
            .await
            .unwrap();
    }

    world.store.close_issue_as_completed(issue).unwrap();
    world.poll().await;

    assert!(!has_branch(&world, &named));
    for branch in &others {
        assert!(has_branch(&world, branch), "{branch} is left");
    }
}

#[tokio::test]
async fn the_branch_of_a_ticket_closed_while_linewright_was_stopped_is_deleted_after_a_restart() {
    let mut world = World::new().await;
    let (ticket, branch) = world.ticket_handed_back_with_its_branch().await;

    world.store.close_issue_as_completed(ticket).unwrap();
    world.restart(|_| {}).await;
    world.poll().await;

    assert!(!has_branch(&world, &branch));
}

#[tokio::test]
async fn a_pull_request_behind_the_default_branch_is_brought_up_to_date() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    let before = world.store.snapshot_pull(pr).await.head_sha;
    world
        .store
        .merge_change(
            "maintainer",
            "Other work",
            "",
            &[("src/other.txt", "other\n")],
        )
        .await
        .unwrap();

    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(
        world
            .store
            .file(&view.head_ref, "src/other.txt")
            .await
            .unwrap(),
        "other\n"
    );
    assert_eq!(view.mergeability, linewright::domain::Mergeability::Clean);
    assert_eq!(
        (
            contains(&world, &view.head_ref, &before),
            merges(&world, &view.head_ref)
        ),
        (true, 1),
        "merged in, the branch's commits kept"
    );
}

async fn moved_past(
    repository: Repository,
    update_branch: Option<linewright::domain::UpdateMethod>,
    conflicting: bool,
) -> (World, linewright::domain::Number) {
    moved_past_with(
        repository,
        |config| config.update_method = update_branch,
        conflicting,
    )
    .await
}

pub(crate) async fn moved_past_with(
    repository: Repository,
    adjust: impl FnOnce(&mut linewright::factory::FactoryConfig),
    conflicting: bool,
) -> (World, linewright::domain::Number) {
    let mut world = World::with_repository(repository, adjust).await;
    let (ticket, pr) = world.ticket_in_review().await;
    let file = if conflicting {
        format!("src/issue-{}.txt", ticket.0)
    } else {
        "src/other.txt".into()
    };
    world
        .store
        .merge_change("maintainer", "Other work", "", &[(&file, "other\n")])
        .await
        .unwrap();
    for kind in [WorkKind::Update, WorkKind::Rework] {
        let file = file.clone();
        world.harness.on(kind, move |call| {
            resolve_in(&call.spec.dir, &[(&file, "done\nother\n")]);
            Ok(json!({"outcome": "done", "comment": "Resolved the conflict."}).to_string())
        });
    }
    (world, pr)
}

fn resolve_in(dir: &std::path::Path, files: &[(&str, &str)]) {
    for (path, contents) in files {
        std::fs::write(dir.join(path), contents).unwrap();
    }
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_EDITOR", "true")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    git(&["add", "--all"]);
    let rebasing = dir.join(git(&["rev-parse", "--git-path", "rebase-merge"]).trim());
    if rebasing.exists() {
        git(&["rebase", "--continue"]);
    } else {
        git(&["commit", "--quiet", "--no-edit"]);
    }
}

fn store_git(world: &World, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .args(args)
        .current_dir(world.store.repository())
        .output()
        .unwrap()
}

fn has_branch(world: &World, branch: &str) -> bool {
    let branch = format!("refs/heads/{branch}");
    store_git(world, &["rev-parse", "--verify", "--quiet", &branch])
        .status
        .success()
}

fn merges(world: &World, branch: &str) -> usize {
    let output = store_git(
        world,
        &[
            "rev-list",
            "--merges",
            "--count",
            &format!("main..{branch}"),
        ],
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn contains(world: &World, branch: &str, commit: &str) -> bool {
    store_git(world, &["merge-base", "--is-ancestor", commit, branch])
        .status
        .success()
}

#[tokio::test]
async fn an_operators_prompt_is_the_one_the_session_gets() {
    let operators = prompts_with("update.md", |text| {
        format!("Use the repository's own skill.\n\n{text}")
    });
    let (mut world, _) = moved_past_with(
        Repository::default(),
        |config| {
            config.prompts =
                linewright::briefs::OperatorPrompts::read(Some(operators.path())).unwrap();
        },
        true,
    )
    .await;

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Update);
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].starts_with("Use the repository's own skill.\n\n"),
        "{}",
        prompts[0]
    );
    assert_eq!(world.harness.stopped(WorkKind::Update), [Stopped::Rebase]);
}

#[tokio::test]
async fn by_default_a_conflict_is_resolved_in_a_rebase_where_the_repository_allows_rebase_merging()
{
    let (mut world, pr) = moved_past(Repository::default(), None, true).await;

    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(world.harness.stopped(WorkKind::Update), [Stopped::Rebase]);
    assert_eq!(merges(&world, &view.head_ref), 0);
    assert_eq!(view.mergeability, linewright::domain::Mergeability::Clean);
}

#[tokio::test]
async fn by_default_a_conflict_is_resolved_in_a_merge_where_the_repository_disallows_rebase_merging()
 {
    use linewright::domain::MergeMethod;

    let (mut world, pr) = moved_past(
        Repository::allowing([MergeMethod::Merge, MergeMethod::Squash]),
        None,
        true,
    )
    .await;

    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(world.harness.stopped(WorkKind::Update), [Stopped::Merge]);
    assert_eq!(merges(&world, &view.head_ref), 1);
    assert_eq!(view.mergeability, linewright::domain::Mergeability::Clean);
}

#[tokio::test]
async fn by_default_a_branch_is_always_rebased_where_the_repository_requires_linear_history() {
    let linear = || Repository {
        linear_history: true,
        ..Repository::default()
    };
    let (mut clean, clean_pr) = moved_past(linear(), None, false).await;
    let before = clean.store.snapshot_pull(clean_pr).await.head_sha;
    let (mut conflicting, conflicting_pr) = moved_past(linear(), None, true).await;

    clean.poll().await;
    conflicting.poll().await;

    let clean_ref = clean.store.snapshot_pull(clean_pr).await.head_ref;
    assert!(!contains(&clean, &clean_ref, &before), "rebased");
    assert_eq!(merges(&clean, &clean_ref), 0);
    assert_eq!(
        conflicting.harness.stopped(WorkKind::Update),
        [Stopped::Rebase]
    );
    let conflicting_ref = conflicting
        .store
        .snapshot_pull(conflicting_pr)
        .await
        .head_ref;
    assert_eq!(merges(&conflicting, &conflicting_ref), 0);
}

#[tokio::test]
async fn update_branch_merge_merges_even_on_a_conflict_and_rebase_always_rebases() {
    use linewright::domain::UpdateMethod;

    let (mut merging, merging_pr) =
        moved_past(Repository::default(), Some(UpdateMethod::Merge), true).await;
    let (mut rebasing, rebasing_pr) =
        moved_past(Repository::default(), Some(UpdateMethod::Rebase), false).await;
    let before = rebasing.store.snapshot_pull(rebasing_pr).await.head_sha;

    merging.poll().await;
    rebasing.poll().await;

    assert_eq!(merging.harness.stopped(WorkKind::Update), [Stopped::Merge]);
    let merging_ref = merging.store.snapshot_pull(merging_pr).await.head_ref;
    assert_eq!(merges(&merging, &merging_ref), 1);
    let rebasing_ref = rebasing.store.snapshot_pull(rebasing_pr).await.head_ref;
    assert!(!contains(&rebasing, &rebasing_ref, &before), "rebased");
    assert_eq!(merges(&rebasing, &rebasing_ref), 0);
}

#[tokio::test]
async fn a_rework_run_first_updates_the_branch_by_the_same_rules() {
    let (mut world, pr) = moved_past(Repository::default(), None, true).await;
    world
        .store
        .comment_as("maintainer", pr, "Tidy up.")
        .unwrap();

    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(world.harness.stopped(WorkKind::Rework), [Stopped::Rebase]);
    assert_eq!(
        world.harness.prompts(WorkKind::Update),
        Vec::<String>::new()
    );
    assert_eq!(merges(&world, &view.head_ref), 0);
}

async fn merge_default_branch_in(world: &World, branch: &str, files: &[(&str, &str)]) {
    use linewright::git;

    let work = tempfile::tempdir().unwrap();
    let dir = work.path().join("clone");
    let url = world.store.repository();
    let identity = [
        ("user.name", "maintainer"),
        ("user.email", "maintainer@example.com"),
    ];
    git::clone(url.to_str().unwrap(), &dir, branch, &identity, &[])
        .await
        .unwrap();
    assert!(
        git::git(&dir, &["merge", "--quiet", "origin/main"])
            .await
            .is_err(),
        "the merge conflicts"
    );
    resolve_in(&dir, files);
    git::git(
        &dir,
        &["push", "--quiet", "origin", &format!("HEAD:{branch}")],
    )
    .await
    .unwrap();
}

async fn merged_in_conflict(
    adjust: impl FnOnce(&mut linewright::factory::FactoryConfig),
) -> (World, linewright::domain::Number, String) {
    use linewright::domain::{Mergeability, Rebaseability};

    let (world, pr) = moved_past_with(Repository::default(), adjust, true).await;
    let view = world.store.snapshot_pull(pr).await;
    let file = format!("src/issue-{}.txt", view.closes[0].0);
    merge_default_branch_in(&world, &view.head_ref, &[(&file, "done\nother\n")]).await;
    let updated = world.store.snapshot_pull(pr).await;
    assert_eq!(
        (updated.mergeability, updated.rebaseability),
        (Mergeability::Clean, Rebaseability::NotRebaseable),
        "up to date, but not rebaseable"
    );
    (world, pr, file)
}

#[tokio::test]
async fn a_branch_with_a_merge_commit_github_cannot_rebase_is_rebased_by_an_update_run() {
    use linewright::domain::{Mergeability, Rebaseability};

    let (mut world, pr, file) = merged_in_conflict(|_| {}).await;

    world.poll().await;
    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(world.harness.stopped(WorkKind::Update), [Stopped::Rebase]);
    assert_eq!(merges(&world, &view.head_ref), 0);
    assert_eq!(
        (view.mergeability, view.rebaseability),
        (Mergeability::Clean, Rebaseability::Rebaseable)
    );
    assert_eq!(
        world.store.file(&view.head_ref, &file).await.unwrap(),
        "done\nother\n"
    );
}

#[tokio::test]
async fn a_rework_run_on_a_pull_request_github_cannot_rebase_rebases_it_first() {
    let (mut world, pr, _) = merged_in_conflict(|_| {}).await;
    world
        .store
        .comment_as("maintainer", pr, "Tidy up.")
        .unwrap();

    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(world.harness.stopped(WorkKind::Rework), [Stopped::Rebase]);
    assert_eq!(merges(&world, &view.head_ref), 0);
}

#[tokio::test]
async fn a_rebase_that_leaves_the_branch_as_it_was_pushes_nothing_and_is_not_tried_again() {
    let (mut world, pr, _) = merged_in_conflict(|_| {}).await;
    let before = world.store.snapshot_pull(pr).await;
    world.harness.on(WorkKind::Update, |call| {
        let aborted = std::process::Command::new("git")
            .args(["rebase", "--abort"])
            .current_dir(&call.spec.dir)
            .output()
            .unwrap();
        assert!(aborted.status.success());
        Ok(json!({"outcome": "done", "comment": "The branch is fine as it is."}).to_string())
    });

    world.poll().await;
    world.poll().await;
    world.poll().await;

    let after = world.store.snapshot_pull(pr).await;
    assert_eq!(world.harness.prompts(WorkKind::Update).len(), 1);
    assert_eq!(
        (after.head_sha, after.pushes.len()),
        (before.head_sha, before.pushes.len())
    );
}

#[tokio::test]
async fn with_auto_merge_by_rebase_a_pull_request_not_rebaseable_is_rebased_before_it_is_merged() {
    use linewright::domain::{MergeMethod, PullState};

    let (mut world, pr, _) = merged_in_conflict(|config| {
        config.auto_merge = Some(AutoMerge {
            merge_method: Some(MergeMethod::Rebase),
        });
    })
    .await;
    world.clock.advance(chrono::TimeDelta::minutes(6));

    world.poll().await;

    assert_eq!(world.harness.stopped(WorkKind::Update), [Stopped::Rebase]);
    let view = world.store.snapshot_pull(pr).await;
    assert!(matches!(view.state, PullState::Open { .. }));

    world.clock.advance(chrono::TimeDelta::minutes(6));
    world.poll().await;

    let merged = world.store.snapshot_pull(pr).await;
    assert!(matches!(merged.state, PullState::Merged { .. }));
    assert_eq!(history(&world)[0], ("Do the ticket".to_string(), 1));
}

/// The subject and parent count of each commit on the store's default
/// branch, newest first.
fn history(world: &World) -> Vec<(String, usize)> {
    let output = std::process::Command::new("git")
        .args(["log", "--format=%s%x09%P", "main"])
        .current_dir(world.store.repository())
        .output()
        .unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            let (subject, parents) = line.split_once('\t').unwrap();
            (subject.to_string(), parents.split_whitespace().count())
        })
        .collect()
}

#[tokio::test]
async fn with_auto_merge_a_green_pull_request_is_merged_and_the_ticket_it_blocked_is_worked() {
    use linewright::domain::{MergeMethod, PullState};
    use linewright::store::Store;

    let mut world = World::with(|config| {
        config.auto_merge = Some(AutoMerge {
            merge_method: Some(MergeMethod::Squash),
        });
    })
    .await;
    let (first, pr) = world.ticket_in_review().await;
    let second = open_ticket(&world, "List gadgets");
    Store::add_blocked_by(world.store.as_ref(), second, first)
        .await
        .unwrap();
    world
        .store
        .assign_as("maintainer", first, "maintainer")
        .unwrap();

    world.clock.advance(chrono::TimeDelta::minutes(6));
    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert!(matches!(view.state, PullState::Merged { .. }));
    assert_eq!(
        history(&world)[0],
        (format!("Do the ticket ({pr})"), 1),
        "squashed onto the default branch"
    );
    assert!(!world.issue(first).open);
    assert_eq!(world.store.requested_reviewers(pr), []);
    assert_eq!(view.comments, []);

    world.poll().await;

    assert!(world.issue(second).is_assigned(&Login::new(LINEWRIGHT)));
    let next = *world.store.pulls().last().unwrap();
    assert_ne!(next, pr);
    assert_eq!(world.store.snapshot_pull(next).await.closes, [second]);
}

async fn auto_merged(
    repository: Repository,
    merge_method: Option<linewright::domain::MergeMethod>,
) -> World {
    let mut world = World::with_repository(repository, |config| {
        config.auto_merge = Some(AutoMerge { merge_method });
    })
    .await;
    world.ticket_in_review().await;
    world.clock.advance(chrono::TimeDelta::minutes(6));
    world.poll().await;
    world
}

#[tokio::test]
async fn with_auto_merge_and_no_merge_method_set_the_repository_decides_it() {
    let rebased = auto_merged(
        Repository {
            linear_history: true,
            ..Repository::default()
        },
        None,
    )
    .await;
    let merged = auto_merged(Repository::default(), None).await;

    assert_eq!(history(&rebased)[0], ("Do the ticket".to_string(), 1));
    assert_eq!(history(&merged)[0].1, 2, "a merge commit");
}

#[tokio::test]
async fn with_auto_merge_and_no_merge_method_set_the_first_method_the_repository_allows_is_used() {
    use linewright::domain::MergeMethod;

    let rebased = auto_merged(Repository::allowing([MergeMethod::Rebase]), None).await;
    let squashed = auto_merged(
        Repository::allowing([MergeMethod::Squash, MergeMethod::Rebase]),
        None,
    )
    .await;

    assert_eq!(history(&rebased)[0], ("Do the ticket".to_string(), 1));
    let pr = *squashed.store.pulls().last().unwrap();
    // A squash of one commit is titled after it.
    assert_eq!(history(&squashed)[0], (format!("Do the ticket ({pr})"), 1));
}

#[tokio::test]
async fn with_auto_merge_a_merge_method_set_wins_over_the_one_the_repository_allows_first() {
    use linewright::domain::MergeMethod;

    let rebased = auto_merged(
        Repository::allowing([MergeMethod::Squash, MergeMethod::Rebase]),
        Some(MergeMethod::Rebase),
    )
    .await;

    assert_eq!(history(&rebased)[0], ("Do the ticket".to_string(), 1));
}

#[tokio::test]
async fn with_auto_merge_a_pull_request_github_blocks_waits_for_a_person_also_across_a_restart_then_merges()
 {
    use linewright::domain::{PullState, Requirement};

    let mut world = World::with(|config| config.auto_merge = AUTO_MERGE).await;
    let (ticket, pr) = world.ticket_in_review().await;
    world.store.block_merge(
        pr,
        vec![Requirement::CodeOwnerReview {
            owners: vec!["alice".into()],
        }],
    );

    world.clock.advance(chrono::TimeDelta::minutes(6));
    world.poll().await;
    world.poll().await;
    world.restart(|config| config.auto_merge = AUTO_MERGE).await;
    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(view.state, PullState::Open { draft: false });
    assert_eq!(world.store.requested_reviewers(pr), []);
    let comments: Vec<&str> = view.comments.iter().map(|c| c.body.as_str()).collect();
    assert_eq!(
        comments,
        ["Ready for review: GitHub requires a code owner's review \
             (@alice) before merging. Linewright merges it once GitHub allows it."]
    );

    world.store.allow_merge(pr);
    world.poll().await;

    let merged = world.store.snapshot_pull(pr).await;
    assert!(matches!(merged.state, PullState::Merged { .. }));
    assert!(!world.issue(ticket).open);
}

#[tokio::test]
async fn with_auto_merge_a_pull_request_held_before_linewright_recorded_its_holds_is_not_held_again()
 {
    let mut world = World::with(|config| config.auto_merge = AUTO_MERGE).await;
    let (_, pr) = world.ticket_in_review().await;
    world.held_for_a_code_owners_review(pr).await;

    world.poll().await;
    world.poll().await;

    assert_eq!(world.store.snapshot_pull(pr).await.comments.len(), 1);
}

#[tokio::test]
async fn with_auto_merge_a_pull_request_held_before_linewright_recorded_its_holds_is_not_held_again_once_github_works_out_its_merge_state()
 {
    let mut world = World::with(|config| config.auto_merge = AUTO_MERGE).await;
    let (_, pr) = world.ticket_in_review().await;
    world.held_for_a_code_owners_review(pr).await;
    world.store.pend_merge_state(pr);

    world.poll().await;
    world.store.work_out_merge_state(pr);
    world.poll().await;

    assert_eq!(world.store.snapshot_pull(pr).await.comments.len(), 1);
}

#[tokio::test]
async fn with_auto_merge_a_pull_request_github_blocks_while_linewright_is_stopped_is_held_for_review()
 {
    use linewright::domain::Requirement;

    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    world.ready_for_review(pr).await;
    // Linewright runs, keeping its record of what it holds, before it stops.
    world.poll().await;
    world.store.block_merge(
        pr,
        vec![Requirement::CodeOwnerReview {
            owners: vec!["alice".into()],
        }],
    );

    world.restart(|config| config.auto_merge = AUTO_MERGE).await;
    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    let comments: Vec<&str> = view.comments.iter().map(|c| c.body.as_str()).collect();
    assert_eq!(
        comments,
        [
            "No CI reported: ready for review.",
            "Ready for review: GitHub requires a code owner's review (@alice) before merging. \
             Linewright merges it once GitHub allows it."
        ]
    );
}

#[tokio::test]
async fn with_auto_merge_a_pull_request_a_person_commented_on_merges_once_they_approve() {
    use linewright::domain::PullState;

    let mut world = World::with(|config| config.auto_merge = AUTO_MERGE).await;
    let (ticket, pr) = world.ticket_in_review().await;
    world
        .store
        .comment_as("maintainer", pr, "Rename it.")
        .unwrap();
    world.harness.on(WorkKind::Rework, |call| {
        commit_in(
            &call.spec.dir,
            &[("src/renamed.txt", "renamed\n")],
            "Rename",
        );
        Ok(json!({"outcome": "done", "comment": "Renamed as asked."}).to_string())
    });

    world.poll().await;
    world.clock.advance(chrono::TimeDelta::minutes(6));
    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Rework).len(), 1);
    assert_eq!(
        world.store.snapshot_pull(pr).await.state,
        PullState::Open { draft: false },
        "held, and ready for the maintainer to approve"
    );

    world
        .store
        .review_as("maintainer", pr, ReviewState::Approved, "", &[])
        .unwrap();
    world.poll().await;

    assert!(matches!(
        world.store.snapshot_pull(pr).await.state,
        PullState::Merged { .. }
    ));
    assert!(!world.issue(ticket).open);
}

#[tokio::test]
async fn with_auto_merge_a_merge_that_fails_twice_hands_the_pull_request_back_and_reassigning_merges_it()
 {
    use linewright::domain::PullState;

    let (log, _guard) = Log::capture();
    let mut world = World::with(|config| config.auto_merge = AUTO_MERGE).await;
    let (ticket, pr) = world.ticket_in_review().await;
    world.store.refuse_merges(
        pr,
        "Head branch was modified. Review and try the merge again.",
    );

    world.clock.advance(chrono::TimeDelta::minutes(6));
    world.poll().await;

    let failed: Vec<String> = log
        .lines("WARN")
        .into_iter()
        .filter(|line| line.contains("merge failed"))
        .collect();
    assert_eq!(failed.len(), 1, "the first failure is logged: {failed:?}");
    assert!(world.issue(ticket).has_label("ready-for-agent"));

    world.poll().await;
    world.poll().await;

    assert_stalled(
        &world,
        ticket,
        pr,
        &format!(
            "Merging {pr} failed twice. GitHub's reason: \
             Head branch was modified. Review and try the merge again."
        ),
    )
    .await;
    assert_eq!(
        world.store.snapshot_pull(pr).await.state,
        PullState::Open { draft: false }
    );

    world.clock.advance(chrono::TimeDelta::minutes(1));
    world.store.assign_as("maintainer", pr, LINEWRIGHT).unwrap();
    world.clock.advance(chrono::TimeDelta::minutes(1));
    world.poll().await;

    assert!(
        linewright_assigned(&world, pr).await,
        "a failure after Linewright is reassigned is retried, not handed back"
    );
    assert_eq!(world.store.pull_labels(pr), Vec::<String>::new());

    world.store.accept_merges(pr);
    world.poll().await;

    assert!(matches!(
        world.store.snapshot_pull(pr).await.state,
        PullState::Merged { .. }
    ));
    assert!(!world.issue(ticket).open);
}

#[tokio::test]
async fn with_auto_merge_a_merge_that_succeeds_on_the_retry_leaves_no_handback_or_comment() {
    use linewright::domain::PullState;

    let mut world = World::with(|config| config.auto_merge = AUTO_MERGE).await;
    let (ticket, pr) = world.ticket_in_review().await;
    world
        .store
        .refuse_merges(pr, "Pull Request is not mergeable");
    let comments_before = world.issue(ticket).comments.len();

    world.clock.advance(chrono::TimeDelta::minutes(6));
    world.poll().await;
    world.store.accept_merges(pr);
    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert!(matches!(view.state, PullState::Merged { .. }));
    assert_eq!(view.comments, []);
    let issue = world.issue(ticket);
    assert_eq!(issue.comments.len(), comments_before);
    assert!(!issue.has_label("ready-for-human"));
}

fn assert_run_ended_keeping_its_claim(
    world: &World,
    ticket: linewright::domain::Number,
    log: &Log,
) {
    use linewright::domain::Login;

    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["linewright:designed", "ready-for-agent"]);
    assert!(issue.is_assigned(&Login::new(LINEWRIGHT)));
    assert!(issue.comments.is_empty(), "{:#?}", issue.comments);
    assert_eq!(
        world.harness.prompts(WorkKind::Implement).len(),
        1,
        "not retried"
    );
    let refused = log.usage_limit_warnings();
    assert_eq!(refused.len(), 1, "{refused:?}");
}

#[tokio::test]
async fn a_run_refused_for_the_usage_limit_ends_keeping_its_claim_and_its_work() {
    use linewright::store::Store;

    let (log, _guard) = Log::capture();
    let mut world = World::new().await;
    world.harness.on(WorkKind::Implement, |call| {
        commit_in(
            &call.spec.dir,
            &[("src/half.txt", "half\n")],
            "Start the ticket",
        );
        refused_for_the_usage_limit()
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    assert_run_ended_keeping_its_claim(&world, ticket, &log);
    assert_eq!(
        world
            .store
            .read_file(&format!("{}-do-the-thing", ticket.0), "src/half.txt")
            .await
            .unwrap()
            .as_deref(),
        Some("half\n")
    );
}

#[tokio::test]
async fn a_run_refused_for_the_usage_limit_whose_work_cannot_be_pushed_still_keeps_its_claim() {
    let (log, _guard) = Log::capture();
    let mut world = World::new().await;
    world.harness.on(WorkKind::Implement, |call| {
        commit_in(
            &call.spec.dir,
            &[("src/half.txt", "half\n")],
            "Start the ticket",
        );
        let unreachable = call.spec.dir.join("no-such-remote");
        let moved = std::process::Command::new("git")
            .args(["remote", "set-url", "origin"])
            .arg(&unreachable)
            .current_dir(&call.spec.dir)
            .status()
            .unwrap();
        assert!(moved.success());
        refused_for_the_usage_limit()
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    assert_run_ended_keeping_its_claim(&world, ticket, &log);
}

#[tokio::test]
async fn a_session_that_fails_on_its_infrastructure_is_retried_once_then_handed_back() {
    use linewright::harness::SessionError;

    let mut world = World::new().await;
    world.harness.on(WorkKind::Implement, |_| {
        Err(SessionError::Infrastructure(
            "the harness's API failed".into(),
        ))
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let issue = world.issue(ticket);
    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 2);
    assert_eq!(labels(&issue), ["linewright:stalled", "ready-for-human"]);
    assert!(
        last_comment(&issue).contains("The run failed twice on its infrastructure"),
        "{}",
        last_comment(&issue)
    );
}

/// GitHub's reason for refusing a push that changes a workflow file, when
/// Linewright's token lacks the workflow permission.
const WORKFLOW_REFUSAL: &str = "refusing to allow a Personal Access Token to create or update \
     workflow `.github/workflows/ci.yml` without `workflow` scope";

#[tokio::test]
async fn a_push_refused_for_a_missing_permission_hands_the_ticket_back_at_once_with_the_remedies() {
    let (world, ticket) = implemented_with_pushes_refused(&rejected(WORKFLOW_REFUSAL)).await;

    let issue = world.issue(ticket);
    assert_eq!(
        world.harness.prompts(WorkKind::Implement).len(),
        1,
        "not retried"
    );
    assert!(issue.has_label("ready-for-human"), "{:?}", issue.labels);
    assert!(!issue.is_assigned(&Login::new(LINEWRIGHT)));
    assert_eq!(issue.comments.len(), 1, "{:#?}", issue.comments);
    let comment = last_comment(&issue);
    assert!(
        comment.contains(WORKFLOW_REFUSAL) && comment.contains("apply `ready-for-agent` again"),
        "{comment}"
    );
    assert_eq!(world.store.pulls(), []);
}

async fn implemented_with_pushes_refused(output: &str) -> (World, linewright::domain::Number) {
    let mut world = World::new().await;
    implement(&world);
    world.store.refuse_pushes(output);
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.poll().await;
    (world, ticket)
}

#[tokio::test]
async fn other_pushes_refused_for_a_missing_permission_are_handed_back_at_once_with_githubs_reason()
{
    for (output, reason) in [
        (
            rejected(
                "refusing to allow a GitHub App to create or update workflow \
                 `.github/workflows/ci.yml` without `workflows` permission",
            ),
            "refusing to allow a GitHub App to create or update workflow \
             `.github/workflows/ci.yml` without `workflows` permission",
        ),
        (
            "remote: Permission to acme/widgets.git denied to linewright-bot.\n\
             fatal: unable to access 'https://github.com/acme/widgets.git/': The requested \
             URL returned error: 403"
                .to_string(),
            "Permission to acme/widgets.git denied to linewright-bot.",
        ),
    ] {
        let (world, ticket) = implemented_with_pushes_refused(&output).await;

        let issue = world.issue(ticket);
        assert_eq!(
            world.harness.prompts(WorkKind::Implement).len(),
            1,
            "not retried"
        );
        assert!(issue.has_label("ready-for-human"), "{:?}", issue.labels);
        assert!(
            last_comment(&issue).contains(reason),
            "{}",
            last_comment(&issue)
        );
    }
}

#[tokio::test]
async fn a_push_refused_for_a_reason_not_recognized_is_retried_once_then_handed_back() {
    for output in [
        rejected("pre-receive hook declined"),
        "To https://github.com/acme/widgets.git\n ! [rejected] HEAD -> branch (stale info)\n\
         error: failed to push some refs to 'https://github.com/acme/widgets.git'"
            .to_string(),
        "fatal: unable to access 'https://github.com/acme/widgets.git/': Could not resolve \
         host: github.com"
            .to_string(),
    ] {
        let (world, ticket) = implemented_with_pushes_refused(&output).await;

        let issue = world.issue(ticket);
        assert_eq!(
            world.harness.prompts(WorkKind::Implement).len(),
            2,
            "{output}"
        );
        assert!(issue.has_label("ready-for-human"), "{:?}", issue.labels);
        let comment = last_comment(&issue);
        assert!(
            comment.contains("The run failed twice on its infrastructure")
                && !comment.contains("The work was not kept"),
            "{comment}"
        );
    }
}

#[tokio::test]
async fn a_hand_back_says_nothing_of_a_secret_the_sessions_work_printed() {
    // A hook the session left in its clone can print one as Linewright
    // pushes.
    let mut world = World::with(|config| {
        config.secrets = secrets_of(&[("GH_TOKEN", "ghp_s3cr3t")]);
    })
    .await;
    implement(&world);
    world
        .store
        .refuse_pushes("pre-push: GH_TOKEN=ghp_s3cr3t\nerror: failed to push some refs");
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let issue = world.issue(ticket);
    let comment = last_comment(&issue);
    assert!(
        comment.contains("pre-push: GH_TOKEN=[redacted]") && !comment.contains("ghp_s3cr3t"),
        "{comment}"
    );
}

#[tokio::test]
async fn a_blocked_run_whose_work_is_refused_for_a_missing_permission_asks_its_question_and_says_the_work_was_not_kept()
 {
    let mut world = World::new().await;
    world.harness.on(WorkKind::Implement, |call| {
        commit_in(&call.spec.dir, &[("src/partial.txt", "half\n")], "Start");
        Ok(json!({"outcome": "blocked", "question": "Should gadgets expire?"}).to_string())
    });
    world.store.refuse_pushes(&rejected(WORKFLOW_REFUSAL));
    let ticket = world.store.open_issue(
        "maintainer",
        "Gadgets",
        "",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let issue = world.issue(ticket);
    assert_eq!(
        world.harness.prompts(WorkKind::Implement).len(),
        1,
        "not retried"
    );
    assert!(issue.has_label("ready-for-human"), "{:?}", issue.labels);
    let comment = last_comment(&issue);
    assert!(
        comment.contains("Should gadgets expire?") && comment.contains(WORKFLOW_REFUSAL),
        "{comment}"
    );
}

#[tokio::test]
async fn a_rework_push_refused_for_a_missing_permission_hands_the_pull_request_back_at_once() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    let head = world.store.snapshot_pull(pr).await.head_ref;
    world
        .store
        .review_as(
            "maintainer",
            pr,
            ReviewState::ChangesRequested,
            "Add CI.",
            &[],
        )
        .unwrap();
    world.harness.on(WorkKind::Rework, |call| {
        commit_in(
            &call.spec.dir,
            &[(".github/workflows/ci.yml", "on: push\n")],
            "Add CI",
        );
        Ok(json!({"outcome": "done", "comment": "Added CI."}).to_string())
    });
    world.store.refuse_pushes(&rejected(WORKFLOW_REFUSAL));

    world.poll().await;

    assert_eq!(
        world.harness.prompts(WorkKind::Rework).len(),
        1,
        "not retried"
    );
    let comment = handed_back_comment(&world, ticket, pr).await;
    assert!(
        comment.contains(&format!("`{head}`")) && comment.contains(WORKFLOW_REFUSAL),
        "{comment}"
    );
}

#[tokio::test]
async fn a_stopped_runs_work_that_cannot_be_kept_logs_nothing_of_a_secret_its_push_printed() {
    let (log, _guard) = Log::capture();
    let mut world = World::with(|config| {
        config.secrets = secrets_of(&[("GH_TOKEN", "ghp_s3cr3t")]);
    })
    .await;
    world.harness.on(WorkKind::Implement, |call| {
        commit_in(&call.spec.dir, &[("src/half.txt", "half\n")], "Start");
        refused_for_the_usage_limit()
    });
    world
        .store
        .refuse_pushes("pre-push: GH_TOKEN=ghp_s3cr3t\nerror: failed to push some refs");
    world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );

    world.poll().await;

    let warnings = log.lines("WARN");
    let kept = warnings
        .iter()
        .find(|line| line.contains("keeping the stopped run's work failed"))
        .expect("the failed keep is logged");
    assert!(
        kept.contains("GH_TOKEN=[redacted]") && !kept.contains("ghp_s3cr3t"),
        "{kept}"
    );
}
