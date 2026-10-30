//! Each back-door state is what its stage leaves, checked by running the
//! stage from the same start.

use linewright::domain::{CheckState, Requirement};
use serde_json::json;

use crate::common::*;

#[tokio::test]
async fn a_ticket_in_review_is_what_the_implement_stage_leaves() {
    let mut front = World::new().await;
    front.implement_a_ticket().await;
    let mut back = World::new().await;

    back.ticket_in_review().await;

    assert_same_state(&front.store, &back.store).await;
}

#[tokio::test]
async fn a_ticket_in_review_closing_another_issue_is_what_the_implement_stage_leaves() {
    let mut front = World::new().await;
    implement_closing(&front, open_old_invoices(&front));
    open_ticket(&front, "Do the thing");
    front.poll().await;
    let mut back = World::new().await;
    let other = open_old_invoices(&back);

    back.ticket_in_review_closing(other).await;

    assert_same_state(&front.store, &back.store).await;
}

/// Opens an issue a pull request may close besides its ticket.
fn open_old_invoices(world: &World) -> linewright::domain::Number {
    world
        .store
        .open_issue("maintainer", "Old invoices", "Please.", &[])
}

#[tokio::test]
async fn an_issue_triaged_for_the_maintainer_is_what_triage_leaves() {
    let mut front = World::new().await;
    triage_for_the_maintainer(&front);
    front
        .store
        .open_issue("reporter", "Resize widgets", "", &["needs-triage"]);
    front.poll().await;
    let back = World::new().await;

    back.issue_triaged_for_the_maintainer().await;

    assert_same_state(&front.store, &back.store).await;
}

#[tokio::test]
async fn an_issue_asking_its_reporter_is_what_triage_leaves() {
    let mut front = World::new().await;
    triage_asking(&front, json!(["reporter"]), "Which version, @reporter?");
    front
        .store
        .open_issue("reporter", "Crash", "It crashes.", &["needs-triage"]);
    front.poll().await;
    let back = World::new().await;

    back.issue_asking_its_reporter().await;

    assert_same_state(&front.store, &back.store).await;
}

/// A world whose spec change of the spec and of code a poll designed.
async fn spec_change_designed() -> World {
    let mut world = World::new().await;
    tickets_script(&world);
    implement_gadgets(&world);
    merge_spec_and_code_change(&world).await;
    world.poll().await;
    world
}

#[tokio::test]
async fn a_spec_change_cut_into_tickets_is_what_design_leaves() {
    let front = spec_change_designed().await;
    let back = World::new().await;

    back.spec_change_cut_into_tickets().await;

    assert_same_state(&front.store, &back.store).await;
}

#[tokio::test]
async fn a_spec_change_ticket_in_review_is_what_design_then_the_implement_stage_leave() {
    let mut front = spec_change_designed().await;
    front.poll().await;
    let back = World::new().await;

    back.spec_change_ticket_in_review().await;

    assert_same_state(&front.store, &back.store).await;
}

#[tokio::test]
async fn an_issue_cut_into_tickets_is_what_design_leaves() {
    let mut front = World::new().await;
    tickets_script(&front);
    implement_gadgets(&front);
    open_undesigned_gadgets(&front);
    front.poll().await;
    let back = World::new().await;

    back.issue_cut_into_tickets().await;

    assert_same_state(&front.store, &back.store).await;
}

#[tokio::test]
async fn an_issue_designed_on_the_issue_is_what_design_leaves() {
    let mut front = World::new().await;
    design_issues(&front);
    open_undesigned_gadgets(&front);
    front.poll().await;
    let back = World::new().await;
    let issue = open_undesigned_gadgets(&back);

    back.designed_on_the_issue(issue).await;

    assert_same_state(&front.store, &back.store).await;
}

/// Relabels a ticket handed back stalled, after its session timed out,
/// `ready-for-agent`.
async fn stalled_ticket_relabeled(world: &World) -> linewright::domain::Number {
    let (ticket, _) = world.ticket_timed_out_with_edits().await;
    relabel_ready_for_agent(world, ticket);
    ticket
}

#[tokio::test]
async fn a_stalled_ticket_designed_on_the_issue_is_what_design_leaves() {
    let mut front = World::new().await;
    stalled_ticket_relabeled(&front).await;
    design_issues(&front);
    front.poll().await;
    let back = World::new().await;
    let ticket = stalled_ticket_relabeled(&back).await;

    back.designed_on_the_issue(ticket).await;

    assert_same_state(&front.store, &back.store).await;
}

/// A world that ran the implement stage on a ticket opened by
/// [`open_designed_gadgets`], with each implement run scripted by `script`.
async fn gadgets_implemented(script: fn(&World)) -> World {
    let mut world = World::new().await;
    script(&world);
    open_designed_gadgets(&world);
    world.poll().await;
    world
}

#[tokio::test]
async fn a_ticket_handed_back_with_its_branch_is_what_the_implement_stage_leaves() {
    let front = gadgets_implemented(implement_giving_up_with_earlier_work).await;
    let back = World::new().await;

    back.ticket_handed_back_with_its_branch().await;

    assert_same_state(&front.store, &back.store).await;
}

#[tokio::test]
async fn a_ticket_timed_out_with_edits_is_what_the_implement_stage_leaves() {
    let front = gadgets_implemented(implement_timing_out_with_edits).await;
    let back = World::new().await;

    back.ticket_timed_out_with_edits().await;

    assert_same_state(&front.store, &back.store).await;
}

#[tokio::test]
async fn a_ticket_blocked_filing_a_spec_gap_is_what_the_implement_stage_leaves() {
    let front = gadgets_implemented(implement_blocked_filing_a_spec_gap).await;
    let back = World::new().await;

    back.ticket_blocked_filing_a_spec_gap().await;

    assert_same_state(&front.store, &back.store).await;
}

/// A world that designed a spec change of the spec, with each design
/// scripted by `script`.
async fn spec_change_of_the_spec_designed(script: fn(&World)) -> World {
    let mut world = World::new().await;
    script(&world);
    merge_spec_change(&world).await;
    world.poll().await;
    world
}

#[tokio::test]
async fn a_spec_change_handed_back_is_what_design_leaves() {
    let front = spec_change_of_the_spec_designed(design_giving_up).await;
    let back = World::new().await;

    back.spec_change_handed_back().await;

    assert_same_state(&front.store, &back.store).await;
}

#[tokio::test]
async fn a_spec_change_blocked_filing_a_spec_gap_is_what_design_leaves() {
    let front = spec_change_of_the_spec_designed(design_blocked_filing_a_spec_gap).await;
    let back = World::new().await;

    back.spec_change_blocked_filing_a_spec_gap().await;

    assert_same_state(&front.store, &back.store).await;
}

#[tokio::test]
async fn a_pull_request_ready_for_review_is_what_a_poll_leaves_once_cis_grace_has_passed() {
    let mut front = World::new().await;
    front.ticket_in_review().await;
    front.clock.advance(chrono::TimeDelta::minutes(6));
    front.poll().await;
    let mut back = World::new().await;
    let (_, pr) = back.ticket_in_review().await;

    back.ready_for_review(pr).await;

    assert_same_state(&front.store, &back.store).await;
}

#[tokio::test]
async fn a_pull_request_held_for_a_code_owners_review_is_what_auto_merge_leaves() {
    let mut front = World::with(|config| config.auto_merge = AUTO_MERGE).await;
    let (_, front_pr) = front.ticket_in_review().await;
    front.store.block_merge(
        front_pr,
        vec![Requirement::CodeOwnerReview {
            owners: vec!["alice".into()],
        }],
    );
    front.clock.advance(chrono::TimeDelta::minutes(6));
    front.poll().await;
    let mut back = World::with(|config| config.auto_merge = AUTO_MERGE).await;
    let (_, pr) = back.ticket_in_review().await;

    back.held_for_a_code_owners_review(pr).await;

    assert_same_state(&front.store, &back.store).await;
}

#[tokio::test]
async fn a_rework_timed_out_with_edits_is_what_the_rework_stage_leaves() {
    let mut front = World::new().await;
    let (_, pr) = front.ticket_in_review().await;
    front.rework_asked(pr);
    rework_timing_out_with_edits(&front);
    front.poll().await;
    let mut back = World::new().await;

    back.rework_timed_out_with_edits().await;

    assert_same_state(&front.store, &back.store).await;
}

#[tokio::test]
async fn a_pull_request_ready_for_review_on_green_checks_is_what_a_poll_leaves() {
    let mut front = World::new().await;
    let (_, front_pr) = front.ticket_in_review().await;
    front
        .store
        .set_checks(front_pr, vec![check("test", CheckState::Passed, "")])
        .await
        .unwrap();
    front.poll().await;
    let mut back = World::new().await;
    let (_, pr) = back.ticket_in_review().await;

    back.ready_for_review_on_green_checks(pr).await;

    assert_same_state(&front.store, &back.store).await;
}

#[tokio::test]
async fn a_pull_request_reworked_is_what_the_rework_stage_leaves() {
    let mut front = World::new().await;
    let (_, front_pr) = front.ticket_in_review().await;
    front.rework_asked(front_pr);
    front.poll().await;
    let mut back = World::new().await;
    let (_, pr) = back.ticket_in_review().await;

    back.reworked(pr).await;

    assert_same_state(&front.store, &back.store).await;
}
