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
async fn the_repositorys_stage_table_sets_its_stages_effort_and_its_top_level_ranks_below_a_flag_and_above_the_operators_file()
 {
    for (flag, triage_effort) in [(None, Effort::Medium), (Some(Effort::Xhigh), Effort::Xhigh)] {
        let mut world = World::with(|config| {
            config.work.file.effort = Some(Effort::Low);
            config.work.flags_and_env.effort = flag;
        })
        .await;
        commit_on(
            &world,
            "main",
            &[(
                CONFIG,
                "effort = \"medium\"\n\n[stage.design]\neffort = \"high\"\n",
            )],
        )
        .await;
        let efforts = Arc::new(Mutex::new(vec![]));
        world.harness.on(WorkKind::Triage, {
            let efforts = efforts.clone();
            move |call| {
                efforts
                    .lock()
                    .unwrap()
                    .push((WorkKind::Triage, call.spec.effort));
                Ok(ready_for_agent())
            }
        });
        world.harness.on(WorkKind::Design, {
            let efforts = efforts.clone();
            move |call| {
                efforts
                    .lock()
                    .unwrap()
                    .push((WorkKind::Design, call.spec.effort));
                Ok(json!({"outcome": "designed", "design": "Built as asked."}).to_string())
            }
        });
        open_for_triage(&world);
        world
            .store
            .open_issue("maintainer", "Gadgets", "Please.", &["ready-for-agent"]);

        world.poll().await;

        let mut efforts = efforts.lock().unwrap().clone();
        efforts.sort_by_key(|(kind, _)| kind.name());
        assert_eq!(
            efforts,
            [
                (WorkKind::Design, Some(Effort::High)),
                (WorkKind::Triage, Some(triage_effort)),
            ],
            "with the flag {flag:?}"
        );
    }
}

#[tokio::test]
async fn the_repositorys_triage_prompt_is_sent_to_triage_runs() {
    let mut world = World::new().await;
    triage_ready(&world);
    open_for_triage(&world);
    let prompt = format!("The repository's triage.\n\n{}", shipped("triage.md"));
    commit_on(
        &world,
        "main",
        &[(".linewright/briefs/prompts/triage.md", &prompt)],
    )
    .await;

    world.poll().await;

    let prompts = world.harness.prompts(WorkKind::Triage);
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(
        prompts[0].starts_with("The repository's triage."),
        "{}",
        prompts[0]
    );
}

#[tokio::test]
async fn a_run_on_another_branch_is_sent_the_prompts_of_the_default_branch() {
    let mut world = World::new().await;
    let prompt = |whose: &str| format!("{whose} rework.\n\n{}", shipped("rework.md"));
    commit_on(
        &world,
        "main",
        &[(".linewright/briefs/prompts/rework.md", &prompt("Main's"))],
    )
    .await;
    let (_, pr) = world.ticket_in_review().await;
    let branch = world.store.pull(pr).await.unwrap().head_ref;
    commit_on(
        &world,
        &branch,
        &[(
            ".linewright/briefs/prompts/rework.md",
            &prompt("The branch's"),
        )],
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
            (CONFIG, "session-env = [\"GH_TOKEN\"]\nmax-runs = 9\n"),
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
async fn a_repositorys_prompt_with_a_wrong_placeholder_holds_only_the_runs_it_is_sent_to() {
    let mut world = World::new().await;
    implement(&world);
    triage_ready(&world);
    commit_on(
        &world,
        "main",
        &[(
            ".linewright/briefs/prompts/triage.md",
            "Triage {{the_issue}}.\n\n{{output}}\n",
        )],
    )
    .await;
    let ticket = open_ticket(&world, "Gadgets");
    let reported = open_for_triage(&world);

    world.poll().await;

    assert_eq!(
        world.harness.prompts(WorkKind::Triage),
        [] as [std::string::String; 0]
    );
    assert!(!is_claimed(&world, reported));
    assert_eq!(world.harness.prompts(WorkKind::Implement).len(), 1);
    assert!(is_claimed(&world, ticket));
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
                ".linewright/briefs/prompts/automation.md",
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
        error.contains(".linewright/briefs/prompts/automation.md"),
        "{error}"
    );
    assert_eq!(
        world.harness.automation_prompts(),
        [] as [std::string::String; 0]
    );
}
