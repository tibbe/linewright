use crate::common;

use std::sync::{Arc, Mutex};

use chrono::TimeDelta;
use common::*;
use linewright::domain::Login;
use linewright::reconcile::WorkKind;
use linewright::store::Store;
use linewright::work_settings::Effort;
use serde_json::json;

const CONFIG: &str = ".linewright/config.toml";

/// The prompt Linewright ships at `path` under `briefs/prompts/`.
fn shipped(path: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/briefs/prompts/{path}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

async fn commit_on(world: &World, branch: &str, files: &[(&str, &str)]) {
    world
        .store
        .commit("maintainer", branch, files, "Tell Linewright")
        .await
        .unwrap();
}

/// Triage answers that the issue is ready for an agent.
fn triage_ready(world: &World) {
    world
        .harness
        .on(WorkKind::Triage, |_| Ok(ready_for_agent()));
}

fn ready_for_agent() -> String {
    json!({
        "outcome": "ready-for-agent",
        "category": "bug",
        "comment": "## Agent Brief\n\n**Summary:** It crashes."
    })
    .to_string()
}

fn open_for_triage(world: &World) -> linewright::domain::Number {
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"])
}

fn is_claimed(world: &World, number: linewright::domain::Number) -> bool {
    world
        .issue(number)
        .assignees
        .contains(&Login::new(LINEWRIGHT))
}

#[tokio::test]
async fn the_repositorys_file_on_the_default_branch_sets_its_sessions_settings() {
    let mut world = World::new().await;
    commit_on(
        &world,
        "main",
        &[(CONFIG, "[stage.triage]\neffort = \"high\"\n")],
    )
    .await;
    let effort = Arc::new(Mutex::new(None));
    world.harness.on(WorkKind::Triage, {
        let effort = effort.clone();
        move |call| {
            *effort.lock().unwrap() = Some(call.spec.effort);
            Ok(ready_for_agent())
        }
    });
    open_for_triage(&world);

    world.poll().await;

    assert_eq!(*effort.lock().unwrap(), Some(Some(Effort::High)));
}

#[tokio::test]
async fn a_run_on_another_branch_is_sent_the_prompts_of_the_default_branch() {
    let mut world = World::new().await;
    let prompt = |whose: &str| format!("{whose} rework.\n\n{}", shipped("rework.md"));
    commit_on(
        &world,
        "main",
        &[(".linewright/prompts/rework.md", &prompt("Main's"))],
    )
    .await;
    let (_, pr) = world.ticket_in_review().await;
    let branch = world.store.pull(pr).await.unwrap().head_ref;
    commit_on(
        &world,
        &branch,
        &[(".linewright/prompts/rework.md", &prompt("The branch's"))],
    )
    .await;
    world.rework_asked(pr);

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Rework);
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(prompts[0].starts_with("Main's rework."), "{}", prompts[0]);
}

#[tokio::test]
async fn a_config_with_a_key_of_the_operators_holds_every_run_until_the_default_branch_fixes_it() {
    let mut world = World::new().await;
    implement(&world);
    triage_ready(&world);
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    commit_on(
        &world,
        "main",
        &[
            (CONFIG, "session-pass-env = [\"GH_TOKEN\"]\nmax-runs = 9\n"),
            (".linewright/automations/hourly.md", HOURLY),
        ],
    )
    .await;
    let ticket = open_ticket(&world, "Gadgets");
    let reported = open_for_triage(&world);
    world.poll().await;
    world.clock.advance(TimeDelta::hours(1));

    world.poll().await;

    assert_eq!(
        world.harness.prompts(WorkKind::Implement),
        [] as [std::string::String; 0]
    );
    assert_eq!(
        world.harness.prompts(WorkKind::Triage),
        [] as [std::string::String; 0]
    );
    assert_eq!(
        world.harness.automation_prompts(),
        [] as [std::string::String; 0]
    );
    assert!(!is_claimed(&world, ticket));
    assert!(!is_claimed(&world, reported));

    commit_on(&world, "main", &[(CONFIG, "effort = \"high\"\n")]).await;
    world.poll().await;

    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
    assert_eq!(world.harness.prompts(WorkKind::Triage).len(), 1);
    assert_eq!(world.harness.automation_prompts().len(), 1);
}

#[tokio::test]
async fn an_automation_run_by_hand_while_a_file_holds_it_fails_naming_the_file() {
    let world = World::new().await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    commit_on(
        &world,
        "main",
        &[
            (".linewright/automations/hourly.md", HOURLY),
            (
                ".linewright/prompts/automation.md",
                "Do {{instructions}} {{nope}}\n",
            ),
        ],
    )
    .await;

    let error = world
        .factory
        .run_automation(&"hourly".parse().unwrap())
        .await
        .unwrap_err();

    let error = format!("{error:#}");
    assert!(
        error.contains(".linewright/prompts/automation.md"),
        "{error}"
    );
    assert_eq!(
        world.harness.automation_prompts(),
        [] as [std::string::String; 0]
    );
}
