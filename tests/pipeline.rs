//! The system, end to end at the user's level: labeled issues and merged pull
//! requests in, labels, comments and pull requests out, against fakes of
//! GitHub, the agent runtime and the clock (`CODING_STANDARDS.md`, "Where
//! tests go").

mod common;

use common::*;
use linewright::reconcile::WorkKind;
use serde_json::json;

#[tokio::test]
async fn a_new_issue_is_triaged_into_a_category_a_state_and_a_brief() {
    let mut world = World::new().await;
    world.runtime.on(WorkKind::Triage, |_| {
        json(json!({
            "outcome": "ready-for-agent",
            "category": "bug",
            "comment": "## Agent Brief\n\n**Summary:** Empty widgets crash."
        }))
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
        "> *Linewright, triage*\n\n## Agent Brief\n\n**Summary:** Empty widgets crash."
    );
    assert!(issue.open);
}

fn triage_for_the_maintainer(world: &World) {
    world.runtime.on(WorkKind::Triage, |call| {
        let outcome = if call.spec.prompt.contains("Yes, make them resizable.") {
            json!({
                "outcome": "ready-for-agent",
                "category": "enhancement",
                "comment": "## Agent Brief\n\n**Summary:** Make widgets resizable."
            })
        } else {
            json!({
                "outcome": "ready-for-human",
                "category": "enhancement",
                "comment": "Should widgets be resizable?"
            })
        };
        json(outcome)
    });
}

#[tokio::test]
async fn an_issue_triaged_for_the_maintainer_says_an_answer_or_a_label_sends_it_back() {
    let mut world = World::new().await;
    triage_for_the_maintainer(&world);
    let issue = world
        .store
        .open_issue("reporter", "Resize widgets", "", &["needs-triage"]);

    world.poll().await;

    let issue = world.issue(issue);
    assert_eq!(labels(&issue), ["enhancement", "ready-for-human"]);
    assert_eq!(
        last_comment(&issue),
        "> *Linewright, triage*\n\nShould widgets be resizable?\n\n\
         Answer in this thread to have it triaged again with your answer, or \
         apply `ready-for-agent` to implement it as decided or `needs-triage` \
         to triage it again."
    );
}

#[tokio::test]
async fn the_maintainers_answer_to_a_triage_for_them_is_triaged_again() {
    let mut world = World::new().await;
    triage_for_the_maintainer(&world);
    let issue = world
        .store
        .open_issue("reporter", "Resize widgets", "", &["needs-triage"]);
    world.poll().await;

    world
        .store
        .comment_as("maintainer", issue, "Yes, make them resizable.")
        .unwrap();
    world.poll().await;

    let issue = world.issue(issue);
    assert_eq!(labels(&issue), ["enhancement", "ready-for-agent"]);
    assert_eq!(
        last_comment(&issue),
        "> *Linewright, triage*\n\n## Agent Brief\n\n**Summary:** Make widgets resizable."
    );
}

fn tickets_script(world: &World) {
    world.runtime.on(WorkKind::Tickets, |_| {
        json(json!({
            "tickets": [
                {"id": "store", "title": "Store gadgets", "body": "## What to build\n\nGadgets persist.", "blocked_by": []},
                {"id": "list", "title": "List gadgets", "body": "## What to build\n\nList them.", "blocked_by": ["store"]}
            ],
            "comment": "Two slices: storing, then listing."
        }))
    });
}

fn implement_script(world: &World) {
    world.runtime.on(WorkKind::Implement, |call| {
        let file = format!("src/{}.txt", subject_file(call));
        commit_in(&call.spec.dir, &[(&file, "gadget\n")], "Add gadgets");
        json(json!({"outcome": "done", "pr_title": "Add gadgets", "pr_body": "## Summary\n\nGadgets."}))
    });
}

#[tokio::test]
async fn a_merged_spec_change_becomes_tickets_worked_in_dependency_order_into_ready_pull_requests()
{
    let mut world = World::new().await;
    tickets_script(&world);
    implement_script(&world);
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
            .contains(&"linewright:spec-processed".to_string())
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
            .map(|l| l.number)
            .collect::<Vec<_>>(),
        [store_ticket.number, list_ticket.number]
    );
    assert_eq!(list_ticket.blocked_by[0].number, store_ticket.number);
    assert_eq!(labels(store_ticket), ["ready-for-agent"]);
    assert!(last_comment(tracking).starts_with("> *Linewright, tickets*"));

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
            .is_assigned(&linewright::domain::Login::new(MACHINE))
    );
    assert!(
        !world
            .issue(list_ticket.number)
            .is_assigned(&linewright::domain::Login::new(MACHINE))
    );

    world.clock.advance(chrono::TimeDelta::minutes(6));
    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(
        view.state,
        linewright::domain::PullState::Open { draft: false }
    );
    assert_eq!(
        view.comments.last().unwrap().body,
        "> *Linewright, review*\n\nNo CI reported: ready for review."
    );

    world.store.merge_pull(pr).await.unwrap();
    world.poll().await;

    assert!(!world.issue(store_ticket.number).open);
    assert!(
        world
            .issue(list_ticket.number)
            .is_assigned(&linewright::domain::Login::new(MACHINE))
    );
}

use linewright::domain::{CheckState, Login, ReviewState};
use linewright::store::local::CheckRecord;

fn check(name: &str, state: CheckState, log: &str) -> CheckRecord {
    CheckRecord {
        name: name.into(),
        state,
        log: log.into(),
    }
}

#[tokio::test]
async fn review_comments_are_addressed_answered_in_their_threads_and_review_is_requested_again() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world
        .store
        .set_checks(pr, vec![check("test", CheckState::Passed, "")])
        .await
        .unwrap();
    world.poll().await;
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
    world.runtime.on(WorkKind::Rework, |call| {
        assert!(call.spec.prompt.contains("Rename this."));
        commit_in(
            &call.spec.dir,
            &[("src/renamed.txt", "renamed\n")],
            "Rename",
        );
        let thread = format!("T{}-1", 2);
        json(json!({
            "outcome": "done",
            "comment": "Renamed as asked.",
            "replies": [{"thread": thread, "body": "Renamed.", "resolve": true}]
        }))
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
    assert_eq!(
        thread.comments.last().unwrap().body,
        "> *Linewright, rework*\n\nRenamed."
    );
    assert!(!thread.resolved, "people resolve their own threads");
    assert_eq!(
        view.comments.last().unwrap().body,
        "> *Linewright, rework*\n\nRenamed as asked."
    );
    assert_eq!(
        world.store.requested_reviewers(pr),
        [Login::new("maintainer")]
    );
    assert_eq!(world.runtime.prompts(WorkKind::Rework).len(), 1);
    let _ = ticket;

    world.poll().await;

    assert_eq!(
        world.runtime.prompts(WorkKind::Rework).len(),
        1,
        "answered feedback starts nothing"
    );
}

#[tokio::test]
async fn a_rework_that_makes_the_description_out_of_date_rewrites_it() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world
        .store
        .comment_as("maintainer", pr, "Rename it.")
        .unwrap();
    world.runtime.on(WorkKind::Rework, |call| {
        assert!(call.spec.prompt.contains("Does it."));
        commit_in(
            &call.spec.dir,
            &[("src/renamed.txt", "renamed\n")],
            "Rename",
        );
        json(json!({
            "outcome": "done",
            "comment": "Renamed as asked.",
            "pr_body": "Does it, renamed."
        }))
    });

    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(view.body, format!("Does it, renamed.\n\nCloses {ticket}"));
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
    world.runtime.on(WorkKind::Rework, move |call| {
        commit_in(
            &call.spec.dir,
            &[("src/renamed.txt", "renamed\n")],
            "Rename",
        );
        store.close_pull_as(pr).unwrap();
        json(json!({"outcome": "done", "comment": "Renamed as asked."}))
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
    assert!(
        !view
            .comments
            .iter()
            .any(|c| c.body.starts_with("> *Linewright, rework*"))
    );
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
    world.runtime.on(WorkKind::Rework, move |call| {
        if call.followup.is_none() {
            commit_in(
                &call.spec.dir,
                &[("CHANGELOG.md", "- thing\n")],
                "Changelog",
            );
            store
                .comment_as("maintainer", pr, "Also update the README.")
                .unwrap();
            return json(json!({"outcome": "done", "comment": "Added the entry."}));
        }
        assert!(call.followup.unwrap().contains("Also update the README."));
        commit_in(
            &call.spec.dir,
            &[("README.md", "# Widgets, updated\n")],
            "README",
        );
        json(json!({"outcome": "done", "comment": "Added the entry and updated the README."}))
    });

    world.poll().await;
    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(
        world.store.file(&view.head_ref, "README.md").await.unwrap(),
        "# Widgets, updated\n"
    );
    assert_eq!(
        world.runtime.prompts(WorkKind::Rework).len(),
        2,
        "one session, resumed once, and no second rework"
    );
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
    world.runtime.on(WorkKind::FixCi, |call| {
        assert!(call.spec.prompt.contains("assertion failed: widgets == 2"));
        let file = format!(
            "src/fix-{}.txt",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        commit_in(&call.spec.dir, &[(&file, "fix\n")], "Fix the test");
        json(json!({"outcome": "done", "comment": "Fixed the widget count."}))
    });

    for _ in 0..2 {
        world.poll().await;
        let view = world.store.snapshot_pull(pr).await;
        assert_eq!(
            view.comments.last().unwrap().body,
            "> *Linewright, CI*\n\nFixed the widget count."
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

    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["ready-for-human"]);
    assert!(!issue.is_assigned(&Login::new(MACHINE)));
    assert_eq!(
        last_comment(&issue),
        "> *Linewright, CI*\n\n`test` still failing after 2 fix attempts."
    );
}

#[tokio::test]
async fn a_branch_that_changes_the_spec_is_pushed_and_its_pull_request_opened() {
    let mut world = World::new().await;
    world.runtime.on(WorkKind::Implement, |call| {
        commit_in(
            &call.spec.dir,
            &[("docs/spec.md", "# Spec\n\nRewritten.\n")],
            "Edit spec",
        );
        json(json!({"outcome": "done", "pr_title": "Edit the spec", "pr_body": "Edits the spec."}))
    });
    let ticket = world
        .store
        .open_issue("maintainer", "Do it", "", &["ready-for-agent"]);

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
    assert!(world.issue(ticket).is_assigned(&Login::new(MACHINE)));
}

#[tokio::test]
async fn an_unparseable_result_is_sent_back_once_then_the_issue_is_handed_back() {
    let mut world = World::new().await;
    world
        .runtime
        .on(WorkKind::Triage, |_| Ok("I think it's a bug.".into()));
    let issue = world
        .store
        .open_issue("reporter", "Crash", "", &["needs-triage"]);

    world.poll().await;

    let prompts = world.runtime.prompts(WorkKind::Triage);
    assert_eq!(prompts.len(), 2);
    assert!(prompts[1].contains("holds no JSON object"));
    let issue = world.issue(issue);
    assert_eq!(labels(&issue), ["ready-for-human"]);
    assert!(last_comment(&issue).contains("still could not be used after 1 follow-up(s)"));
}

#[tokio::test]
async fn the_pull_request_is_titled_as_the_session_wrote_and_a_blank_title_is_sent_back() {
    let mut world = World::new().await;
    world.runtime.on(WorkKind::Implement, |call| {
        if call.followup.is_none() {
            commit_in(&call.spec.dir, &[("src/gadget.txt", "gadget\n")], "Add");
        }
        let title = if call.followup.is_none() {
            " "
        } else {
            " Report gadgets "
        };
        json(json!({"outcome": "done", "pr_title": title, "pr_body": "Gadgets."}))
    });
    world.store.open_issue(
        "maintainer",
        "Gadgets are missing",
        "",
        &["ready-for-agent"],
    );

    world.poll().await;

    let prompts = world.runtime.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 2);
    assert!(prompts[1].contains("`pr_title` is empty"));
    let pr = world.store.snapshot_pull(world.store.pulls()[0]).await;
    assert_eq!(pr.title, "Report gadgets");
}

#[tokio::test]
async fn a_blocking_spec_gap_hands_the_ticket_back_keeping_the_branch_and_a_non_blocking_one_becomes_an_issue()
 {
    let mut world = World::new().await;
    world.runtime.on(WorkKind::Implement, |call| {
        commit_in(&call.spec.dir, &[("src/partial.txt", "half\n")], "Start");
        json(json!({
            "outcome": "blocked",
            "question": "Should gadgets expire?",
            "spec_gaps": [{"title": "Spec: gadget colors", "body": "The spec doesn't say."}]
        }))
    });
    let ticket = world
        .store
        .open_issue("maintainer", "Gadgets", "", &["ready-for-agent"]);

    world.poll().await;

    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["ready-for-human"]);
    assert!(last_comment(&issue).contains("Should gadgets expire?"));
    assert!(last_comment(&issue).contains(&format!("`{}-gadgets`", ticket.0)));
    assert_eq!(
        world
            .store
            .file(&format!("{}-gadgets", ticket.0), "src/partial.txt")
            .await
            .unwrap(),
        "half\n"
    );
    let gap = world
        .store
        .issues()
        .into_iter()
        .find(|i| i.title == "Spec: gadget colors")
        .unwrap();
    assert_eq!(labels(&gap), ["ready-for-human"]);
    assert!(
        gap.body
            .ends_with(&format!("Found while working on {ticket}."))
    );
}

#[tokio::test]
async fn a_spec_gap_already_filed_for_a_ticket_is_shown_to_later_sessions_and_not_filed_again() {
    let mut world = World::new().await;
    world.runtime.on(WorkKind::Implement, |_| {
        json(json!({
            "outcome": "blocked",
            "question": "Should gadgets expire?",
            "spec_gaps": [{"title": "Spec: gadget colors", "body": "The spec doesn't say."}]
        }))
    });
    let ticket = world
        .store
        .open_issue("maintainer", "Gadgets", "", &["ready-for-agent"]);
    world.poll().await;
    world.store.unlabel_as(ticket, "ready-for-human").unwrap();
    world.store.label_as(ticket, "ready-for-agent").unwrap();

    world.poll().await;

    let prompts = world.runtime.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 2);
    assert!(!prompts[0].contains("Spec gaps already filed"));
    assert!(prompts[1].contains("Spec gaps already filed"));
    assert!(prompts[1].contains("Spec: gadget colors"));
    let gaps = world
        .store
        .issues()
        .into_iter()
        .filter(|i| i.title == "Spec: gadget colors")
        .count();
    assert_eq!(gaps, 1);
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
        format!("> *Linewright, review*\n\n{pr} was closed without merging.")
    );
    assert_eq!(
        world.store.pulls().len(),
        1,
        "no new pull request replaces a rejected one"
    );
}

#[tokio::test]
async fn a_pull_request_behind_the_default_branch_is_brought_up_to_date() {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
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
        config.auto_merge = true;
        config.merge_method = Some(MergeMethod::Squash);
    })
    .await;
    implement_script(&world);
    let first = world.store.open_issue(
        "maintainer",
        "Store gadgets",
        "Gadgets persist.",
        &["ready-for-agent"],
    );
    world.store.assign_as(first, "maintainer").unwrap();
    let second = world.store.open_issue(
        "maintainer",
        "List gadgets",
        "List them.",
        &["ready-for-agent"],
    );
    Store::add_blocked_by(world.store.as_ref(), second, first)
        .await
        .unwrap();
    world.poll().await;
    let pr = *world.store.pulls().last().unwrap();

    world.clock.advance(chrono::TimeDelta::minutes(6));
    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert!(matches!(view.state, PullState::Merged { .. }));
    assert_eq!(
        history(&world)[0],
        (format!("Add gadgets ({pr})"), 1),
        "squashed onto the default branch"
    );
    assert!(!world.issue(first).open);
    assert_eq!(world.store.requested_reviewers(pr), []);
    assert_eq!(view.comments, []);

    world.poll().await;

    assert!(world.issue(second).is_assigned(&Login::new(MACHINE)));
    let next = *world.store.pulls().last().unwrap();
    assert_ne!(next, pr);
    assert_eq!(world.store.snapshot_pull(next).await.closes, [second]);
}

/// Runs a ticket to a green pull request with auto-merge on and the merge
/// method left to the repository.
async fn auto_merged_by_default(linear_history: bool) -> World {
    let mut world = World::with_repository(linear_history, |config| config.auto_merge = true).await;
    world.ticket_in_review().await;
    world.clock.advance(chrono::TimeDelta::minutes(6));
    world.poll().await;
    world
}

#[tokio::test]
async fn with_auto_merge_and_no_merge_method_set_the_repository_decides_it() {
    let rebased = auto_merged_by_default(true).await;
    let merged = auto_merged_by_default(false).await;

    assert_eq!(history(&rebased)[0], ("Do the ticket".to_string(), 1));
    assert_eq!(history(&merged)[0].1, 2, "a merge commit");
}

#[tokio::test]
async fn with_auto_merge_a_pull_request_a_person_commented_on_merges_once_they_approve() {
    use linewright::domain::PullState;

    let mut world = World::with(|config| config.auto_merge = true).await;
    let (ticket, pr) = world.ticket_in_review().await;
    world
        .store
        .comment_as("maintainer", pr, "Rename it.")
        .unwrap();
    world.runtime.on(WorkKind::Rework, |call| {
        commit_in(
            &call.spec.dir,
            &[("src/renamed.txt", "renamed\n")],
            "Rename",
        );
        json(json!({"outcome": "done", "comment": "Renamed as asked."}))
    });

    world.poll().await;
    world.clock.advance(chrono::TimeDelta::minutes(6));
    world.poll().await;

    assert_eq!(world.runtime.prompts(WorkKind::Rework).len(), 1);
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
