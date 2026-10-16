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

/// Triages two issues in one poll with sessions that each draft a comment in
/// their scratch directory, and returns each session's scratch directory
/// with whether it was empty when the session started.
async fn triage_two_issues_drafting_in_scratch() -> Vec<(std::path::PathBuf, bool)> {
    let mut world = World::new().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sessions = seen.clone();
    world.runtime.on(WorkKind::Triage, move |call| {
        let scratch = call.spec.scratch.clone();
        let empty = std::fs::read_dir(&scratch).unwrap().next().is_none();
        std::fs::write(scratch.join("comment.md"), "A draft.").unwrap();
        sessions.lock().unwrap().push((scratch, empty));
        json(json!({
            "outcome": "ready-for-agent",
            "category": "bug",
            "comment": "## Agent Brief\n\n**Summary:** It crashes."
        }))
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

#[tokio::test]
async fn triage_and_implement_tasks_say_to_keep_scratch_files_in_the_temporary_directory() {
    let mut world = World::new().await;
    world.runtime.on(WorkKind::Triage, |_| {
        json(json!({
            "outcome": "ready-for-agent",
            "category": "bug",
            "comment": "## Agent Brief\n\n**Summary:** It crashes."
        }))
    });
    world.store.open_issue(
        "reporter",
        "Crash on empty widget",
        "It crashes.",
        &["needs-triage"],
    );
    world.poll().await;
    world.ticket_in_review().await;

    let prompts = [WorkKind::Triage, WorkKind::Implement]
        .into_iter()
        .flat_map(|kind| world.runtime.prompts(kind));

    for prompt in prompts {
        assert!(prompt.contains("`$TMPDIR`"), "{prompt}");
    }
}

#[tokio::test]
async fn an_implementing_session_is_given_linewrights_skills_as_its_plugin() {
    let mut world = World::new().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sessions = seen.clone();
    world.runtime.on(WorkKind::Implement, move |call| {
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
        json(json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."}))
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    world.store.assign_as(ticket, "maintainer").unwrap();

    world.poll().await;

    assert_eq!(
        seen.lock().unwrap().clone(),
        Some((
            "linewright".to_string(),
            include_str!("../briefs/skills/code-review/SKILL.md").to_string()
        ))
    );
}

#[tokio::test]
async fn each_stages_session_starts_by_invoking_its_linewright_skill() {
    let mut world = World::new().await;
    world.runtime.on(WorkKind::Triage, |_| {
        json(json!({
            "outcome": "ready-for-human",
            "category": "bug",
            "comment": "Does it crash on every widget?"
        }))
    });
    tickets_script(&world);
    world.store.open_issue(
        "reporter",
        "Crash on empty widget",
        "It crashes.",
        &["needs-triage"],
    );
    world
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
    world.ticket_in_review().await;

    let first_lines = |kind| {
        let mut lines: Vec<String> = world
            .runtime
            .prompts(kind)
            .iter()
            .map(|prompt| prompt.lines().next().unwrap_or_default().to_string())
            .collect();
        lines.dedup();
        lines
    };
    assert_eq!(
        [WorkKind::Triage, WorkKind::Tickets, WorkKind::Implement].map(first_lines),
        [
            ["/linewright:triage"],
            ["/linewright:to-tickets"],
            ["/linewright:implement"]
        ]
    );
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
async fn an_issue_triaged_for_the_maintainer_says_a_label_answers_it() {
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
         Answer with a label: `ready-for-agent` to implement it as decided or \
         `needs-triage` to triage it again. A comment alone starts nothing."
    );
}

#[tokio::test]
async fn the_maintainers_answer_to_a_triage_for_them_is_triaged_again_once_they_label_it() {
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

    assert_eq!(
        labels(&world.issue(issue)),
        ["enhancement", "ready-for-human"]
    );

    world.store.label_as(issue, "needs-triage").unwrap();
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
        "> *Linewright, review*\n\nNo CI reported: ready for review."
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

#[tokio::test]
async fn a_ticket_cut_from_a_spec_change_is_briefed_with_its_tracking_issue_and_the_spec_change() {
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
    world.poll().await;

    let issues = world.store.issues();
    let tracking = &issues[0];
    let prompts = world.runtime.prompts(WorkKind::Implement);
    let brief = &prompts[0];
    assert!(brief.contains("Gadgets persist."), "{brief}");
    assert!(
        brief.contains(&format!(
            "### Parent issue\n\n{}, the tracking issue of the spec change {spec_pr}.\n",
            tracking.number
        )),
        "{brief}"
    );
    assert!(!brief.contains("Tickets for the spec change"), "{brief}");
    assert!(!brief.contains("List gadgets"), "{brief}");
}

#[tokio::test]
async fn a_ticket_a_person_parented_is_briefed_with_its_parent_and_no_spec_change() {
    use linewright::store::Store;
    let mut world = World::new().await;
    implement(&world);
    let parent = world
        .store
        .open_issue("maintainer", "Gadgets, all of them", "", &[]);
    let ticket = world
        .store
        .open_issue("maintainer", "Gadgets", "", &["ready-for-agent"]);
    world.store.add_sub_issue(parent, ticket).await.unwrap();

    world.poll().await;

    let prompts = world.runtime.prompts(WorkKind::Implement);
    assert!(
        prompts[0].contains(&format!("### Parent issue\n\n{parent}.\n")),
        "{}",
        prompts[0]
    );
    assert!(!prompts[0].contains("spec change #"), "{}", prompts[0]);
}

#[tokio::test]
async fn a_ticket_with_no_parent_is_briefed_with_no_parent_issue() {
    let mut world = World::new().await;
    world.ticket_in_review().await;

    let prompts = world.runtime.prompts(WorkKind::Implement);
    assert_eq!(prompts.len(), 1);
    assert!(!prompts[0].contains("Parent issue"), "{}", prompts[0]);
}

#[tokio::test]
async fn sessions_on_a_tickets_pull_request_are_briefed_with_its_parent_issue() {
    use linewright::store::Store;
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    let parent = world
        .store
        .open_issue("maintainer", "Gadgets, all of them", "", &[]);
    world.store.add_sub_issue(parent, ticket).await.unwrap();
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
        json(json!({"outcome": "done", "comment": "Renamed."}))
    });

    world.poll().await;

    let prompts = world.runtime.prompts(WorkKind::Rework);
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains(&format!("### Parent issue\n\n{parent}.\n")),
        "{}",
        prompts[0]
    );
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

/// Runs a rework of the comments `authors` post, one each, on a ticket the
/// maintainer is assigned, and returns who was asked for review.
async fn reviewers_after_rework_of_comments_by(authors: &[&str]) -> Vec<Login> {
    let mut world = World::new().await;
    let (_, pr) = world.ticket_in_review().await;
    for author in authors {
        world.store.comment_as(author, pr, "Rename it.").unwrap();
    }
    world.runtime.on(WorkKind::Rework, |call| {
        commit_in(
            &call.spec.dir,
            &[("src/renamed.txt", "renamed\n")],
            "Rename",
        );
        json(json!({"outcome": "done", "comment": "Renamed as asked."}))
    });

    world.poll().await;

    assert_eq!(world.runtime.prompts(WorkKind::Rework).len(), 1);
    world.store.requested_reviewers(pr)
}

#[tokio::test]
async fn a_rework_requests_review_from_the_commenter_and_not_from_an_assignee_who_did_not_comment()
{
    assert_eq!(
        reviewers_after_rework_of_comments_by(&["reviewer"]).await,
        [Login::new("reviewer")]
    );
}

#[tokio::test]
async fn a_rework_of_only_bot_comments_requests_review_from_no_one() {
    assert_eq!(
        reviewers_after_rework_of_comments_by(&["linter[bot]"]).await,
        []
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
    world.runtime.on(WorkKind::Rework, move |call| {
        if call.followup.is_none() {
            commit_in(
                &call.spec.dir,
                &[("CHANGELOG.md", "- thing\n")],
                "Changelog",
            );
            store
                .comment_as("designer", pr, "Also update the README.")
                .unwrap();
            return json(json!({"outcome": "done", "comment": "Added the entry."}));
        }
        commit_in(
            &call.spec.dir,
            &[("README.md", "# Widgets, updated\n")],
            "README",
        );
        json(json!({"outcome": "done", "comment": "Added the entry and updated the README."}))
    });

    world.poll().await;

    assert_eq!(
        world.store.requested_reviewers(pr),
        [Login::new("designer"), Login::new("reviewer")]
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

/// A ticket for the factory, `claimed` or not, and a person's open pull
/// request that closes it.
async fn ticket_a_persons_pull_request_closes(world: &World, claimed: bool) -> (Number, Number) {
    implement(world);
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    if claimed {
        world.store.assign_as(ticket, LINEWRIGHT).unwrap();
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
    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 0);
}

#[tokio::test]
async fn a_claimed_ticket_a_persons_open_pull_request_closes_is_not_implemented() {
    let mut world = World::new().await;
    ticket_a_persons_pull_request_closes(&world, true).await;

    world.poll().await;

    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 0);
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
    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 1);
}

/// A ticket the factory implemented in a pull request that also closes
/// another issue, which the maintainer then labels `ready-for-agent`, with
/// the pull request's CI failing: the ticket, the other issue and the pull
/// request.
async fn pull_request_closing_another_issue(world: &mut World) -> (Number, Number, Number) {
    let other = world
        .store
        .open_issue("maintainer", "Old invoices", "Please.", &[]);
    world.runtime.on(WorkKind::Implement, move |call| {
        let file = format!("src/{}.txt", subject_file(call));
        commit_in(&call.spec.dir, &[(&file, "done\n")], "Do the ticket");
        let body = format!("Does it.\n\nCloses {other}");
        json(json!({"outcome": "done", "pr_title": "Do it", "pr_body": body}))
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );
    world.poll().await;
    let pr = *world
        .store
        .pulls()
        .last()
        .expect("a pull request was opened");
    world.store.label_as(other, "ready-for-agent").unwrap();
    world
        .store
        .set_checks(pr, vec![check("test", CheckState::Failed, "boom")])
        .await
        .unwrap();
    world.runtime.on(WorkKind::FixCi, |_| {
        json(json!({"outcome": "done", "comment": "Fixed."}))
    });
    (ticket, other, pr)
}

#[tokio::test]
async fn a_pull_request_handed_back_with_its_ticket_is_not_worked_for_another_issue_it_closes() {
    let mut world = World::new().await;
    let (ticket, other, _) = pull_request_closing_another_issue(&mut world).await;
    world.store.unlabel_as(ticket, "ready-for-agent").unwrap();
    world.store.label_as(ticket, "ready-for-human").unwrap();

    world.poll().await;

    assert_eq!(world.runtime.prompts(WorkKind::FixCi).len(), 0);
    assert_eq!(world.issue(other).assignees, []);
    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 1);
}

#[tokio::test]
async fn a_pull_request_is_worked_for_its_ticket_while_another_issue_it_closes_waits() {
    let mut world = World::new().await;
    let (ticket, other, _) = pull_request_closing_another_issue(&mut world).await;

    world.poll().await;

    let fixes = world.runtime.prompts(WorkKind::FixCi);
    assert_eq!(fixes.len(), 1);
    assert!(fixes[0].contains(&ticket.to_string()), "{}", fixes[0]);
    assert_eq!(world.issue(other).assignees, []);
    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 1);
}

#[tokio::test]
async fn a_pull_request_closed_unmerged_hands_back_its_ticket_and_another_issue_it_closed_goes_on()
{
    let mut world = World::new().await;
    let (ticket, other, pr) = pull_request_closing_another_issue(&mut world).await;
    world.poll().await;
    world.store.close_pull_as(pr).unwrap();

    world.poll().await;

    assert!(labels(&world.issue(ticket)).contains(&"ready-for-human"));
    assert_eq!(labels(&world.issue(other)), ["ready-for-agent"]);
    assert_eq!(world.issue(other).assignees, [Login::new(LINEWRIGHT)]);
    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 2);
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
    assert!(!issue.is_assigned(&Login::new(LINEWRIGHT)));
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
    assert!(world.issue(ticket).is_assigned(&Login::new(LINEWRIGHT)));
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

/// A ticket whose implementer committed part of the work and gave up, so
/// that the work so far is on the ticket's branch, returned with the branch.
async fn ticket_handed_back_with_its_branch(world: &mut World) -> (Number, String) {
    world.runtime.on(WorkKind::Implement, |call| {
        commit_in(&call.spec.dir, &[("src/partial.txt", "half\n")], "Start");
        json(json!({"outcome": "gave-up", "reason": "Too hard."}))
    });
    let ticket = world
        .store
        .open_issue("maintainer", "Gadgets", "", &["ready-for-agent"]);
    world.poll().await;
    let branch = format!("{}-gadgets", ticket.0);
    assert!(has_branch(world, &branch), "the work so far is kept");
    (ticket, branch)
}

#[tokio::test]
async fn closing_a_ticket_with_no_open_pull_request_deletes_its_branch() {
    let mut world = World::new().await;
    let (ticket, branch) = ticket_handed_back_with_its_branch(&mut world).await;

    world.store.close_issue_as(ticket).unwrap();
    world.poll().await;

    assert!(!has_branch(&world, &branch));
}

#[tokio::test]
async fn closing_a_ticket_whose_pull_request_is_open_leaves_its_branch() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    let branch = world.store.snapshot_pull(pr).await.head_ref;

    world.store.close_issue_as(ticket).unwrap();
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

    world.store.close_issue_as(issue).unwrap();
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

    world.store.close_issue_as(issue).unwrap();
    world.poll().await;

    assert!(!has_branch(&world, &named));
    for branch in &others {
        assert!(has_branch(&world, branch), "{branch} is left");
    }
}

#[tokio::test]
async fn the_branch_of_a_ticket_closed_while_linewright_was_stopped_is_deleted_after_a_restart() {
    let mut world = World::new().await;
    let (ticket, branch) = ticket_handed_back_with_its_branch(&mut world).await;

    world.store.close_issue_as(ticket).unwrap();
    world.factory = linewright::factory::Factory::start(
        world.store.clone(),
        world.runtime.clone(),
        world.clock.clone(),
        config(&world.dir.path().join("work")),
    )
    .await
    .unwrap();
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

/// A world on `repository` with the `update-branch` setting, if any, and a
/// ticket in review whose pull request the default branch has moved past, in
/// the file the pull request changes if `conflicting`. Its sessions on the
/// pull request resolve conflicts as an agent would.
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

/// [`moved_past`], with the settings `adjust` makes.
async fn moved_past_with(
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
        world.runtime.on(kind, move |call| {
            resolve_in(&call.spec.dir, &[(&file, "done\nother\n")]);
            json(json!({"outcome": "done", "comment": "Resolved the conflict."}))
        });
    }
    (world, pr)
}

/// Writes `files` in the session's clone and finishes the rebase or merge
/// in progress, as an agent resolving a conflict would.
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

/// Runs `git` in the store's repository.
fn store_git(world: &World, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .args(args)
        .current_dir(world.store.repository())
        .output()
        .unwrap()
}

/// Whether the store's repository has the branch `branch`.
fn has_branch(world: &World, branch: &str) -> bool {
    let branch = format!("refs/heads/{branch}");
    store_git(world, &["rev-parse", "--verify", "--quiet", &branch])
        .status
        .success()
}

/// How many merge commits `branch` has that the default branch doesn't.
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

/// Whether `branch` still contains `commit`: its history wasn't rewritten.
fn contains(world: &World, branch: &str, commit: &str) -> bool {
    store_git(world, &["merge-base", "--is-ancestor", commit, branch])
        .status
        .success()
}

/// The conflict instructions the sessions of `kind` were given.
fn told_to_finish(world: &World, kind: WorkKind) -> Vec<&'static str> {
    world
        .runtime
        .prompts(kind)
        .iter()
        .filter_map(|prompt| {
            if prompt.contains("A rebase onto `origin/main` stopped on conflicts in")
                && prompt.contains("finish with `GIT_EDITOR=true git rebase --continue`")
            {
                Some("rebase")
            } else if prompt.contains("A merge of `origin/main` stopped on conflicts in")
                && prompt.contains("finish with `git commit`")
            {
                Some("merge")
            } else {
                None
            }
        })
        .collect()
}

#[tokio::test]
async fn by_default_a_conflict_is_resolved_in_a_rebase_where_the_repository_allows_rebase_merging()
{
    let (mut world, pr) = moved_past(Repository::default(), None, true).await;

    world.poll().await;

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(told_to_finish(&world, WorkKind::Update), ["rebase"]);
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
    assert_eq!(told_to_finish(&world, WorkKind::Update), ["merge"]);
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
    assert_eq!(told_to_finish(&conflicting, WorkKind::Update), ["rebase"]);
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

    assert_eq!(told_to_finish(&merging, WorkKind::Update), ["merge"]);
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
    assert_eq!(told_to_finish(&world, WorkKind::Rework), ["rebase"]);
    assert_eq!(
        world.runtime.prompts(WorkKind::Update),
        Vec::<String>::new()
    );
    assert_eq!(merges(&world, &view.head_ref), 0);
}

/// A person resolves the conflict between `branch` and the default branch in
/// a merge commit, with `files` as the resolution, and pushes it: how #16's
/// branch came to be impossible to rebase.
async fn merge_default_branch_in(world: &World, branch: &str, files: &[(&str, &str)]) {
    use linewright::git;

    let work = tempfile::tempdir().unwrap();
    let dir = work.path().join("clone");
    let url = world.store.repository();
    git::clone(url.to_str().unwrap(), &dir, branch, &[])
        .await
        .unwrap();
    git::set_identity(&dir, "maintainer", "maintainer@example.com")
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

/// A world with the settings `adjust` makes and a ticket in review whose
/// branch the default branch moved past in a conflict, which a person
/// resolved in a merge commit, and the file resolved. The pull request is up
/// to date but not rebaseable.
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
    assert_eq!(told_to_finish(&world, WorkKind::Update), ["rebase"]);
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
    assert_eq!(told_to_finish(&world, WorkKind::Rework), ["rebase"]);
    assert_eq!(merges(&world, &view.head_ref), 0);
}

#[tokio::test]
async fn a_rebase_that_leaves_the_branch_as_it_was_pushes_nothing_and_is_not_tried_again() {
    let (mut world, pr, _) = merged_in_conflict(|_| {}).await;
    let before = world.store.snapshot_pull(pr).await;
    world.runtime.on(WorkKind::Update, |call| {
        let aborted = std::process::Command::new("git")
            .args(["rebase", "--abort"])
            .current_dir(&call.spec.dir)
            .output()
            .unwrap();
        assert!(aborted.status.success());
        json(json!({"outcome": "done", "comment": "The branch is fine as it is."}))
    });

    world.poll().await;
    world.poll().await;
    world.poll().await;

    let after = world.store.snapshot_pull(pr).await;
    assert_eq!(world.runtime.prompts(WorkKind::Update).len(), 1);
    assert_eq!(
        (after.head_sha, after.pushes.len()),
        (before.head_sha, before.pushes.len())
    );
}

#[tokio::test]
async fn with_auto_merge_by_rebase_a_pull_request_not_rebaseable_is_rebased_before_it_is_merged() {
    use linewright::domain::{MergeMethod, PullState};

    let (mut world, pr, _) = merged_in_conflict(|config| {
        config.auto_merge = true;
        config.merge_method = Some(MergeMethod::Rebase);
    })
    .await;
    world.clock.advance(chrono::TimeDelta::minutes(6));

    world.poll().await;

    assert_eq!(told_to_finish(&world, WorkKind::Update), ["rebase"]);
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

    assert!(world.issue(second).is_assigned(&Login::new(LINEWRIGHT)));
    let next = *world.store.pulls().last().unwrap();
    assert_ne!(next, pr);
    assert_eq!(world.store.snapshot_pull(next).await.closes, [second]);
}

/// Runs a ticket to a green pull request with auto-merge on and the
/// `merge-method` setting, if any.
async fn auto_merged(
    repository: Repository,
    merge_method: Option<linewright::domain::MergeMethod>,
) -> World {
    let mut world = World::with_repository(repository, |config| {
        config.auto_merge = true;
        config.merge_method = merge_method;
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
    assert_eq!(history(&squashed)[0], (format!("Do it ({pr})"), 1));
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
async fn with_auto_merge_a_pull_request_github_blocks_waits_for_a_person_then_merges() {
    use linewright::domain::{PullState, Requirement};

    let mut world = World::with(|config| config.auto_merge = true).await;
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

    let view = world.store.snapshot_pull(pr).await;
    assert_eq!(view.state, PullState::Open { draft: false });
    assert_eq!(world.store.requested_reviewers(pr), []);
    let comments: Vec<&str> = view.comments.iter().map(|c| c.body.as_str()).collect();
    assert_eq!(
        comments,
        [
            "> *Linewright, review*\n\nReady for review: GitHub requires a code owner's review \
             (@alice) before merging. Linewright merges it once GitHub allows it."
        ]
    );

    world.store.allow_merge(pr);
    world.poll().await;

    let merged = world.store.snapshot_pull(pr).await;
    assert!(matches!(merged.state, PullState::Merged { .. }));
    assert!(!world.issue(ticket).open);
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

#[tokio::test]
async fn with_auto_merge_a_merge_that_fails_twice_hands_the_ticket_back_and_relabeling_merges_it() {
    use linewright::domain::PullState;

    let (log, _guard) = Log::capture();
    let mut world = World::with(|config| config.auto_merge = true).await;
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

    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["ready-for-human"]);
    assert!(!issue.is_assigned(&Login::new(LINEWRIGHT)));
    assert_eq!(
        last_comment(&issue),
        format!(
            "> *Linewright, review*\n\nMerging {pr} failed twice. GitHub's reason: \
             Head branch was modified. Review and try the merge again."
        )
    );
    assert_eq!(
        world.store.snapshot_pull(pr).await.state,
        PullState::Open { draft: false }
    );

    world.store.unlabel_as(ticket, "ready-for-human").unwrap();
    world.store.label_as(ticket, "ready-for-agent").unwrap();
    world.poll().await;

    let reclaimed = world.issue(ticket);
    assert!(reclaimed.is_assigned(&Login::new(LINEWRIGHT)), "reclaimed");
    assert_eq!(
        labels(&reclaimed),
        ["ready-for-agent"],
        "a failure after the relabel is retried, not handed back"
    );

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

    let mut world = World::with(|config| config.auto_merge = true).await;
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

/// Checks that the implementing run on `ticket`, refused for the usage
/// limit, ended at once, logging the limit, and left the ticket as it was.
fn assert_run_ended_keeping_its_claim(
    world: &World,
    ticket: linewright::domain::Number,
    log: &Log,
) {
    use linewright::domain::Login;

    let issue = world.issue(ticket);
    assert_eq!(labels(&issue), ["ready-for-agent"]);
    assert!(issue.is_assigned(&Login::new(LINEWRIGHT)));
    assert!(issue.comments.is_empty(), "{:#?}", issue.comments);
    assert_eq!(
        world.runtime.prompts(WorkKind::Implement).len(),
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
    world.runtime.on(WorkKind::Implement, |call| {
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
        &["ready-for-agent"],
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
    world.runtime.on(WorkKind::Implement, |call| {
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
        &["ready-for-agent"],
    );

    world.poll().await;

    assert_run_ended_keeping_its_claim(&world, ticket, &log);
}

#[tokio::test]
async fn a_session_that_fails_on_its_infrastructure_is_retried_once_then_handed_back() {
    use linewright::runtime::SessionError;

    let mut world = World::new().await;
    world.runtime.on(WorkKind::Implement, |_| {
        Err(SessionError::Infrastructure(
            "the agent runtime's API failed".into(),
        ))
    });
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
    );

    world.poll().await;

    let issue = world.issue(ticket);
    assert_eq!(world.runtime.prompts(WorkKind::Implement).len(), 2);
    assert_eq!(labels(&issue), ["ready-for-human"]);
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
        world.runtime.prompts(WorkKind::Implement).len(),
        1,
        "not retried"
    );
    assert_eq!(labels(&issue), ["ready-for-human"]);
    assert!(!issue.is_assigned(&Login::new(LINEWRIGHT)));
    assert_eq!(issue.comments.len(), 1, "{:#?}", issue.comments);
    assert_eq!(
        last_comment(&issue),
        format!(
            "> *Linewright, implementation*\n\nGitHub refused to push the branch \
             `{}-do-the-thing` because Linewright's token lacks a permission. GitHub's \
             reason: {WORKFLOW_REFUSAL}\n\nThe work was not kept. Grant the permission and \
             apply `ready-for-agent` again, or make the change yourself.",
            ticket.0
        )
    );
    assert!(world.store.pulls().is_empty());
}

/// A ticket implemented while every push fails, `git push` reporting
/// `output`, after the poll that works it.
async fn implemented_with_pushes_refused(output: &str) -> (World, linewright::domain::Number) {
    let mut world = World::new().await;
    implement(&world);
    world.store.refuse_pushes(output);
    let ticket = world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent"],
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
            world.runtime.prompts(WorkKind::Implement).len(),
            1,
            "not retried"
        );
        assert_eq!(labels(&issue), ["ready-for-human"]);
        assert!(
            last_comment(&issue).contains(&format!(
                "GitHub's reason: {reason}\n\nThe work was not kept."
            )),
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
            world.runtime.prompts(WorkKind::Implement).len(),
            2,
            "{output}"
        );
        assert_eq!(labels(&issue), ["ready-for-human"]);
        let comment = last_comment(&issue);
        assert!(
            comment.contains("The run failed twice on its infrastructure")
                && !comment.contains("The work was not kept"),
            "{comment}"
        );
    }
}

#[tokio::test]
async fn a_blocked_run_whose_work_is_refused_for_a_missing_permission_asks_its_question_and_says_the_work_was_not_kept()
 {
    let mut world = World::new().await;
    world.runtime.on(WorkKind::Implement, |call| {
        commit_in(&call.spec.dir, &[("src/partial.txt", "half\n")], "Start");
        json(json!({"outcome": "blocked", "question": "Should gadgets expire?"}))
    });
    world.store.refuse_pushes(&rejected(WORKFLOW_REFUSAL));
    let ticket = world
        .store
        .open_issue("maintainer", "Gadgets", "", &["ready-for-agent"]);

    world.poll().await;

    let issue = world.issue(ticket);
    assert_eq!(
        world.runtime.prompts(WorkKind::Implement).len(),
        1,
        "not retried"
    );
    assert_eq!(labels(&issue), ["ready-for-human"]);
    assert_eq!(
        last_comment(&issue),
        format!(
            "> *Linewright, implementation*\n\nBlocked on a question the spec cannot decide:\n\n\
             Should gadgets expire?\n\nGitHub refused to push the branch `{}-gadgets` because \
             Linewright's token lacks a permission. GitHub's reason: {WORKFLOW_REFUSAL}\n\nThe \
             work was not kept. Grant the permission and apply `ready-for-agent` again, or make \
             the change yourself.",
            ticket.0
        )
    );
}

#[tokio::test]
async fn a_rework_push_refused_for_a_missing_permission_hands_the_ticket_back_at_once() {
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
    world.runtime.on(WorkKind::Rework, |call| {
        commit_in(
            &call.spec.dir,
            &[(".github/workflows/ci.yml", "on: push\n")],
            "Add CI",
        );
        json(json!({"outcome": "done", "comment": "Added CI."}))
    });
    world.store.refuse_pushes(&rejected(WORKFLOW_REFUSAL));

    world.poll().await;

    let issue = world.issue(ticket);
    assert_eq!(
        world.runtime.prompts(WorkKind::Rework).len(),
        1,
        "not retried"
    );
    assert_eq!(labels(&issue), ["ready-for-human"]);
    assert!(!issue.is_assigned(&Login::new(LINEWRIGHT)));
    assert_eq!(
        last_comment(&issue),
        format!(
            "> *Linewright, rework*\n\nGitHub refused to push the branch `{head}` because \
             Linewright's token lacks a permission. GitHub's reason: {WORKFLOW_REFUSAL}\n\nThe \
             work was not kept. Grant the permission and apply `ready-for-agent` again, or make \
             the change yourself."
        )
    );
    assert_eq!(
        world.store.snapshot_pull(pr).await.comments.len(),
        0,
        "nothing is posted on the pull request"
    );
}
