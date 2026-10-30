use crate::common;

use std::collections::BTreeMap;

use chrono::TimeDelta;
use common::*;
use linewright::domain::Number;
use linewright::layer::{Layer, Source};
use linewright::reconcile::WorkKind;
use linewright::runs::{
    self, Ending, Layers, Listing, NoRuns, Query, Ran, Recorded, RunId, RunOf, Runs, SessionEntry,
    Started, StartedBy, State,
};
use linewright::session_env::SetEnv;
use linewright::work_settings::{Effort, Partial, Setting, StageTable};
use serde_json::json;

fn triage_ready(world: &World) {
    world.harness.on(WorkKind::Triage, |_| {
        Ok(json!({
            "outcome": "ready-for-agent",
            "category": "bug",
            "comment": "## Agent Brief\n\n**Summary:** It crashes."
        })
        .to_string())
    });
}

#[tokio::test]
async fn a_triage_run_is_listed_under_its_run_id_with_its_outcome() {
    let (mut world, work_dir) = world_in_work_dir().await;
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 1, "{}", output.stdout);
    assert!(
        lines[0].starts_with("1/triage-1: done — #1 triage, started 2026-10-01 12:00:"),
        "{}",
        output.stdout
    );
}

fn triage_again(world: &World, issue: linewright::domain::Number) {
    world.store.unlabel_as(issue, "ready-for-agent").unwrap();
    world
        .store
        .label_as("maintainer", issue, "needs-triage")
        .unwrap();
}

/// A world whose triage sessions run with the operator's `opus` and a
/// value its table sets, leaving effort to the harness.
async fn triage_on_opus() -> (World, tempfile::TempDir) {
    let work_dir = tempfile::tempdir().unwrap();
    let mut world = World::with(|config| {
        config.work_dir = work_dir.path().join(REPO);
        config.work.stages.insert(
            WorkKind::Triage,
            StageTable {
                settings: Partial {
                    model: Some("opus".into()),
                    ..Partial::default()
                },
                session_env: Some(SetEnv::new([("CI".into(), "true".into())]).unwrap()),
                ..StageTable::default()
            },
        );
    })
    .await;
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;
    (world, work_dir)
}

#[tokio::test]
async fn an_items_runs_show_each_sessions_model_and_effort() {
    let (_world, work_dir) = triage_on_opus().await;

    let output = lw_in(work_dir.path(), &["runs", "1"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}", output.stdout);
    assert!(
        lines[0].starts_with("1/triage-1: done"),
        "{}",
        output.stdout
    );
    assert_eq!(
        lines[1],
        "  session 1: model opus, effort the harness's choice"
    );
}

/// A world whose triage sessions run with no model set, on a harness that
/// reports running `claude-opus-5-5`.
async fn triage_reporting_opus() -> (World, tempfile::TempDir) {
    let (mut world, work_dir) = world_in_work_dir().await;
    world.harness.reports_model("claude-opus-5-5");
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;
    (world, work_dir)
}

#[tokio::test]
async fn a_session_with_no_model_set_records_the_model_the_harness_reported_and_the_harness_as_its_source()
 {
    let (_world, work_dir) = triage_reporting_opus().await;

    let json = lw_in(work_dir.path(), &["runs", "1", "--json"]).await;

    assert_eq!(json.code, Some(0), "{}", json.stderr);
    let answer: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    let session = &answer["runs"][0]["sessions"][0];
    assert_eq!(session["model"], serde_json::Value::Null);
    assert_eq!(session["reported_model"], "claude-opus-5-5");
    assert_eq!(session["layers"]["settings"]["model"], "harness");
}

#[tokio::test]
async fn a_run_that_starts_no_session_is_recorded_with_none_linked() {
    let (mut world, work_dir) = world_in_work_dir().await;
    world.implement_a_ticket().await;
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
    // Brought up to date without a session: nothing conflicts.
    world.poll().await;

    let recorded = Runs::new(&work_dir.path().join(REPO)).read().unwrap();

    let sessions = |id: &str| {
        let run = recorded
            .iter()
            .find(|run| run.id.to_string() == id)
            .unwrap();
        run.sessions.len()
    };
    assert_eq!((sessions("2/update-1"), sessions("1/implement-1")), (0, 1));
}

async fn hourly_by_schedule_then_by_hand() -> (World, tempfile::TempDir) {
    let (mut world, work_dir) = world_in_work_dir().await;
    world.hourly_by_schedule_then_by_hand().await;
    (world, work_dir)
}

#[tokio::test]
async fn an_automations_run_by_hand_is_marked_as_one_and_a_scheduled_one_is_not() {
    let (_world, work_dir) = hourly_by_schedule_then_by_hand().await;

    let recorded = Runs::new(&work_dir.path().join(REPO)).read().unwrap();

    let mut started: Vec<(String, Option<StartedBy>)> = recorded
        .iter()
        .map(|run| (run.id.to_string(), run.started().by()))
        .collect();
    started.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        started,
        [
            ("hourly/1".to_string(), Some(StartedBy::Schedule)),
            ("hourly/2".to_string(), Some(StartedBy::Hand)),
        ]
    );
}

#[tokio::test]
async fn a_run_handed_back_says_so() {
    let (mut world, work_dir) = world_in_work_dir().await;
    world.harness.on(WorkKind::Implement, |_| {
        Ok(json!({"outcome": "gave-up", "reason": "It cannot be done."}).to_string())
    });
    world.store.open_issue(
        "maintainer",
        "Do it",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs", "1"]).await;

    assert!(
        output
            .stdout
            .starts_with("1/implement-1: handed back — #1 implement, "),
        "{}",
        output.stdout
    );
}

#[tokio::test]
async fn a_run_whose_push_is_refused_for_a_missing_permission_is_handed_back() {
    let (mut world, work_dir) = world_in_work_dir().await;
    implement(&world);
    world.store.refuse_pushes(&rejected(
        "refusing to allow a Personal Access Token to create or update workflow \
         `.github/workflows/ci.yml` without `workflow` scope",
    ));
    world.store.open_issue(
        "maintainer",
        "Do it",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs", "1"]).await;

    assert!(
        output
            .stdout
            .starts_with("1/implement-1: handed back — #1 implement, "),
        "{}",
        output.stdout
    );
}

#[tokio::test]
async fn runs_of_an_item_with_none_exits_4_saying_so() {
    let (mut world, work_dir) = world_in_work_dir().await;
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs", "7"]).await;

    assert_eq!(output.code, Some(4), "{}", output.stderr);
    assert_eq!(output.stdout, "");
    assert!(
        output.stderr.contains("#7 has no runs"),
        "{}",
        output.stderr
    );
}

#[tokio::test]
async fn runs_json_prints_the_answer_as_json() {
    let (mut world, work_dir) = world_in_work_dir().await;
    triage_ready(&world);
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs", "--json"]).await;

    assert_eq!(output.code, Some(0), "{}", output.stderr);
    let answer: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(answer["runs"][0]["id"], "1/triage-1");
}

fn ended(of: RunOf, n: u32, by: Option<StartedBy>, start: &str) -> Recorded {
    Recorded::new(
        Started::new(RunId { of, n }, by).unwrap(),
        at(start),
        State::Ended {
            at: at(start) + TimeDelta::minutes(1),
            ending: Ending::Done,
        },
        vec![],
        None,
    )
}

fn work(item: u64, stage: WorkKind, n: u32, start: &str) -> Recorded {
    let of = RunOf::Work {
        item: Number(item),
        stage,
    };
    ended(of, n, None, start)
}

fn weekly(n: u32, start: &str) -> Recorded {
    let of = RunOf::Automation {
        automation: "weekly".parse().unwrap(),
    };
    ended(of, n, Some(StartedBy::Schedule), start)
}

fn morning() -> Vec<Recorded> {
    vec![
        work(1, WorkKind::Triage, 1, "2026-10-14T08:00:00Z"),
        weekly(1, "2026-10-14T09:00:00Z"),
        work(1, WorkKind::Implement, 1, "2026-10-14T10:00:00Z"),
        work(2, WorkKind::Triage, 1, "2026-10-14T10:30:00Z"),
        work(1, WorkKind::Rework, 1, "2026-10-14T11:00:00Z"),
        work(1, WorkKind::Rework, 2, "2026-10-14T12:00:00Z"),
    ]
}

fn ids(listing: &Listing) -> Vec<String> {
    listing.runs.iter().map(|run| run.id.to_string()).collect()
}

#[test]
fn every_run_is_listed_newest_first() {
    let listing = runs::list(morning(), &Query::default(), at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(
        ids(&listing),
        [
            "1/rework-2",
            "1/rework-1",
            "2/triage-1",
            "1/implement-1",
            "weekly/1",
            "1/triage-1"
        ]
    );
    assert_eq!(listing.left_out, 0);
}

#[test]
fn an_items_runs_are_only_its_own() {
    let query = Query {
        item: Some(Number(1)),
        ..Query::default()
    };

    let listing = runs::list(morning(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(
        ids(&listing),
        ["1/rework-2", "1/rework-1", "1/implement-1", "1/triage-1"]
    );
}

fn ticket_and_pull() -> Vec<Recorded> {
    vec![
        work(1, WorkKind::Triage, 1, "2026-10-14T08:00:00Z"),
        work(1, WorkKind::Implement, 1, "2026-10-14T09:00:00Z"),
        work(3, WorkKind::Rework, 1, "2026-10-14T09:30:00Z"),
        work(2, WorkKind::Triage, 1, "2026-10-14T10:30:00Z"),
        work(3, WorkKind::FixCi, 1, "2026-10-14T11:00:00Z"),
        work(3, WorkKind::Rework, 2, "2026-10-14T12:00:00Z"),
    ]
}

#[test]
fn a_pull_requests_runs_are_its_own() {
    let query = Query {
        item: Some(Number(3)),
        ..Query::default()
    };

    let listing = runs::list(ticket_and_pull(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["3/rework-2", "3/fix-ci-1", "3/rework-1"]);
}

#[test]
fn a_tickets_runs_are_its_own_and_none_of_its_pull_requests() {
    let query = Query {
        item: Some(Number(1)),
        ..Query::default()
    };

    let listing = runs::list(ticket_and_pull(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["1/implement-1", "1/triage-1"]);
}

#[test]
fn an_item_with_no_runs_has_none_to_list() {
    let query = Query {
        item: Some(Number(3)),
        ..Query::default()
    };

    let listed = runs::list(morning(), &query, at("2026-10-14T13:00:00Z"));

    assert_eq!(listed, Err(NoRuns(Number(3))));
}

#[test]
fn stage_lists_only_that_stages_runs() {
    let query = Query {
        stage: Some(WorkKind::Triage),
        ..Query::default()
    };

    let listing = runs::list(morning(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["2/triage-1", "1/triage-1"]);
}

#[test]
fn since_lists_only_the_runs_started_then_or_after() {
    let query = Query {
        since: Some(at("2026-10-14T10:30:00Z")),
        ..Query::default()
    };

    let listing = runs::list(morning(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["1/rework-2", "1/rework-1", "2/triage-1"]);
}

#[test]
fn a_list_cut_to_its_limit_ends_saying_how_many_it_left_out_and_how_to_show_them() {
    let query = Query {
        item: Some(Number(1)),
        limit: Some(1),
        ..Query::default()
    };

    let listing = runs::list(morning(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["1/rework-2"]);
    assert_eq!(listing.left_out, 3);
    assert_eq!(
        runs::render(&listing, false),
        "1/rework-2: done — #1 rework, started 2026-10-14 12:00:00 UTC, ran 1m\n\
         left out: 3 earlier runs; show them with --limit 4\n"
    );
}

/// `docs/specs/design_system.md`, Time, Durations.
#[test]
fn a_run_shows_how_long_it_ran_to_its_two_largest_units() {
    let mut run = work(1, WorkKind::Implement, 1, "2026-10-14T09:00:00Z");
    run.state = State::Ended {
        at: at("2026-10-14T12:02:09Z"),
        ending: Ending::Done,
    };

    let listing = runs::list(vec![run], &Query::default(), at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(
        runs::render(&listing, false),
        "1/implement-1: done — #1 implement, started 2026-10-14 09:00:00 UTC, ran 3h 2m\n"
    );
}

#[test]
fn a_run_in_flight_has_run_until_now_and_one_interrupted_for_no_known_time() {
    let mut in_flight = work(1, WorkKind::Rework, 2, "2026-10-14T12:00:00Z");
    in_flight.state = State::InFlight;
    let mut interrupted = work(1, WorkKind::Rework, 1, "2026-10-14T11:00:00Z");
    interrupted.state = State::Interrupted;

    let listing = runs::list(
        vec![in_flight, interrupted],
        &Query::default(),
        at("2026-10-14T12:30:00Z"),
    )
    .unwrap();

    assert_eq!(
        runs::render(&listing, false),
        "1/rework-2: in flight — #1 rework, started 2026-10-14 12:00:00 UTC, ran 30m\n\
         1/rework-1: interrupted — #1 rework, started 2026-10-14 11:00:00 UTC\n"
    );
}

#[tokio::test]
async fn the_filters_narrow_the_list_from_the_command_line() {
    let (mut world, work_dir) = world_in_work_dir().await;
    triage_ready(&world);
    let issue =
        world
            .store
            .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;
    triage_again(&world, issue);
    world.poll().await;
    let dir = work_dir.path();

    let limited = lw_in(
        dir,
        &[
            "runs",
            "1",
            "--stage",
            "triage",
            "--since",
            "2026-10-01",
            "--limit",
            "1",
        ],
    )
    .await;
    let later = lw_in(dir, &["runs", "--since", "2026-10-02T00:00:00Z"]).await;
    let other_stage = lw_in(dir, &["runs", "--stage", "fix-ci"]).await;
    let no_stage = lw_in(dir, &["runs", "--stage", "review"]).await;

    let lines: Vec<&str> = limited.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{}{}", limited.stdout, limited.stderr);
    assert!(
        lines[0].starts_with("1/triage-2: done"),
        "{}",
        limited.stdout
    );
    assert_eq!(
        lines[2],
        "left out: 1 earlier run; show them with --limit 2"
    );
    assert_eq!((later.code, later.stdout.as_str()), (Some(0), ""));
    assert_eq!(
        (other_stage.code, other_stage.stdout.as_str()),
        (Some(0), "")
    );
    assert_eq!(no_stage.code, Some(2), "{}", no_stage.stderr);
}

#[tokio::test]
async fn an_operators_prompt_is_sent_and_recorded_as_the_operators() {
    let work_dir = tempfile::tempdir().unwrap();
    let operators = prompts_with("follow-ups/result-error.md", |text| {
        format!("The operator's follow-up.\n\n{text}")
    });
    let mut world = World::with(|config| {
        config.work_dir = work_dir.path().join(REPO);
        config.prompts = linewright::briefs::OperatorPrompts::read(Some(operators.path())).unwrap();
    })
    .await;
    world
        .harness
        .on(WorkKind::Triage, |_| Ok("I think it's a bug.".into()));
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world.poll().await;

    let output = lw_in(work_dir.path(), &["runs", "1", "--json"]).await;

    let sent = world.harness.prompts(WorkKind::Triage);
    assert_eq!(sent.len(), 2);
    assert!(
        sent[1].starts_with("The operator's follow-up.\n\n"),
        "{}",
        sent[1]
    );
    let answer: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    let prompts = &answer["runs"][0]["sessions"][0]["layers"]["prompts"];
    assert_eq!(prompts["follow-ups/result-error.md"], "operator");
}

fn ran(model: Option<&str>, reported_model: Option<&str>, effort: Option<Effort>) -> Ran {
    Ran {
        model: model.map(str::to_string),
        reported_model: reported_model.map(str::to_string),
        effort,
        layers: Layers {
            settings: BTreeMap::new(),
            prompts: BTreeMap::new(),
        },
    }
}

#[test]
fn a_session_ran_the_model_the_harness_reported_or_else_the_one_set() {
    let sessions = [
        ran(Some("opus"), Some("claude-opus-5-5"), None),
        ran(Some("opus"), None, None),
        ran(None, Some("claude-opus-5-5"), None),
        ran(None, None, None),
    ];

    assert_eq!(
        sessions.iter().map(Ran::model_ran).collect::<Vec<_>>(),
        [
            Some("claude-opus-5-5"),
            Some("opus"),
            Some("claude-opus-5-5"),
            None
        ]
    );
}

fn session(n: usize, ran: Option<Ran>) -> SessionEntry {
    SessionEntry {
        dir: format!("sessions/1/triage-1/{n}").into(),
        ran,
    }
}

fn of_item(item: u64) -> Query {
    Query {
        item: Some(Number(item)),
        ..Query::default()
    }
}

#[test]
fn an_items_runs_show_what_each_session_ran_with_or_that_it_was_not_recorded() {
    let mut run = work(1, WorkKind::Triage, 1, "2026-10-14T12:00:00Z");
    run.sessions = vec![
        session(
            1,
            Some(ran(
                Some("opus"),
                Some("claude-opus-5-5"),
                Some(Effort::High),
            )),
        ),
        session(2, Some(ran(None, None, None))),
        session(3, None),
    ];

    let listing = runs::list(vec![run], &of_item(1), at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(
        runs::render(&listing, false),
        "1/triage-1: done — #1 triage, started 2026-10-14 12:00:00 UTC, ran 1m\n  \
         session 1: model claude-opus-5-5, effort high\n  \
         session 2: model the harness's choice, effort the harness's choice\n  \
         session 3: model and effort not recorded\n"
    );
}

#[test]
fn an_items_runs_as_json_give_what_each_session_ran_with_and_where_its_settings_and_prompts_came_from()
 {
    let mut run = work(1, WorkKind::Triage, 1, "2026-10-14T12:00:00Z");
    run.sessions = vec![
        session(
            1,
            Some(Ran {
                layers: Layers {
                    settings: BTreeMap::from([
                        (Setting::MODEL, Source::Layer(Layer::Operator)),
                        (Setting::EFFORT, Source::Harness),
                        (Setting::SESSION_TIMEOUT, Source::Layer(Layer::Repository)),
                    ]),
                    prompts: BTreeMap::from([
                        ("triage.md".to_string(), Layer::Linewright),
                        ("follow-ups/result-error.md".to_string(), Layer::Operator),
                    ]),
                },
                ..ran(Some("opus"), Some("claude-opus-5-5"), None)
            }),
        ),
        session(2, None),
    ];

    let listing = runs::list(vec![run], &of_item(1), at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(
        serde_json::to_value(&listing).unwrap(),
        json!({
            "runs": [{
                "id": "1/triage-1",
                "item": 1,
                "stage": "triage",
                "started_at": "2026-10-14T12:00:00Z",
                "duration_seconds": 60,
                "outcome": "done",
                "sessions": [
                    {
                        "model": "opus",
                        "reported_model": "claude-opus-5-5",
                        "effort": null,
                        "layers": {
                            "settings": {
                                "model": "operator",
                                "effort": "harness",
                                "session-timeout": "repository",
                            },
                            "prompts": {
                                "follow-ups/result-error.md": "operator",
                                "triage.md": "linewright",
                            },
                        },
                    },
                    null,
                ],
            }],
            "left_out": 0,
        })
    );
}

fn weekly_by_hand(n: u32, start: &str, ending: Ending) -> Recorded {
    let of = RunOf::Automation {
        automation: "weekly".parse().unwrap(),
    };
    let mut run = ended(of, n, Some(StartedBy::Hand), start);
    run.state = State::Ended {
        at: at(start) + TimeDelta::minutes(1),
        ending,
    };
    run
}

#[test]
fn an_automations_run_says_whether_it_was_by_hand_and_how_it_ended() {
    let recorded = vec![
        weekly(1, "2026-10-14T09:00:00Z"),
        weekly_by_hand(2, "2026-10-14T10:00:00Z", Ending::Failed),
    ];

    let listing = runs::list(recorded, &Query::default(), at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(
        runs::render(&listing, false),
        "weekly/2: failed — automation weekly by hand, started 2026-10-14 10:00:00 UTC, ran 1m\n\
         weekly/1: done — automation weekly, started 2026-10-14 09:00:00 UTC, ran 1m\n"
    );
}

#[test]
fn the_whole_list_as_json_says_how_each_automations_run_started_and_leaves_the_sessions_out() {
    let mut triage = work(1, WorkKind::Triage, 1, "2026-10-14T08:00:00Z");
    triage.sessions = vec![session(1, Some(ran(Some("opus"), None, None)))];
    let recorded = vec![
        triage,
        weekly(1, "2026-10-14T09:00:00Z"),
        weekly_by_hand(2, "2026-10-14T10:00:00Z", Ending::Done),
    ];

    let listing = runs::list(recorded, &Query::default(), at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(
        serde_json::to_value(&listing).unwrap(),
        json!({
            "runs": [
                {
                    "id": "weekly/2",
                    "automation": "weekly",
                    "started_by": "hand",
                    "started_at": "2026-10-14T10:00:00Z",
                    "duration_seconds": 60,
                    "outcome": "done",
                },
                {
                    "id": "weekly/1",
                    "automation": "weekly",
                    "started_by": "schedule",
                    "started_at": "2026-10-14T09:00:00Z",
                    "duration_seconds": 60,
                    "outcome": "done",
                },
                {
                    "id": "1/triage-1",
                    "item": 1,
                    "stage": "triage",
                    "started_at": "2026-10-14T08:00:00Z",
                    "duration_seconds": 60,
                    "outcome": "done",
                },
            ],
            "left_out": 0,
        })
    );
}

#[test]
fn an_items_runs_narrowed_to_none_are_no_runs_to_list_not_none_at_all() {
    let query = Query {
        stage: Some(WorkKind::FixCi),
        ..of_item(1)
    };

    let listed = runs::list(morning(), &query, at("2026-10-14T13:00:00Z"));

    assert_eq!(
        listed,
        Ok(Listing {
            runs: vec![],
            left_out: 0,
        })
    );
}

/// What `Runs` reads from a work directory holding each of `files`.
fn read(files: &[(&str, &str)]) -> (tempfile::TempDir, Vec<Recorded>) {
    let (base, work_dir) = work_dir_holding(files);
    let recorded = Runs::new(&work_dir.resolve()).read().unwrap();
    (base, recorded)
}

/// What `Runs` reads of #5's first rework, done in a minute, whose record
/// gives its sessions as `sessions`.
fn rework_of_5_read(sessions: &serde_json::Value) -> (tempfile::TempDir, Vec<Recorded>) {
    let record = item_run_record(5, "rework", 1, Some("done"), sessions);
    read(&[("runs/items/5/rework-1.json", &record)])
}

fn rework_of_5(sessions: Vec<SessionEntry>) -> Recorded {
    Recorded::new(
        Started::new("5/rework-1".parse().unwrap(), None).unwrap(),
        at("2026-10-14T12:00:00Z"),
        State::Ended {
            at: at("2026-10-14T12:01:00Z"),
            ending: Ending::Done,
        },
        sessions,
        None,
    )
}

#[test]
fn a_record_from_before_runs_kept_their_pull_request_reads_as_its_run() {
    let (_base, recorded) = rework_of_5_read(&json!([]));

    assert_eq!(recorded, [rework_of_5(vec![])]);
}

#[test]
fn a_record_from_before_sessions_kept_what_they_ran_with_reads_none_of_it() {
    let (base, recorded) = rework_of_5_read(&json!(["sessions/5/rework-1"]));

    assert_eq!(
        recorded,
        [rework_of_5(vec![SessionEntry {
            dir: base.path().join(REPO).join("sessions/5/rework-1"),
            ran: None,
        }])]
    );
}

/// What the one session of #5's first rework ran with, as read from a record
/// giving its sessions as `sessions`.
fn ran_as_read(sessions: &serde_json::Value) -> Ran {
    let (_base, recorded) = rework_of_5_read(sessions);
    let [run] = &recorded[..] else {
        panic!("one run: {recorded:#?}");
    };
    let [SessionEntry { ran: Some(ran), .. }] = &run.sessions[..] else {
        panic!("one session with what it ran with: {run:#?}");
    };
    ran.clone()
}

#[test]
fn a_record_from_before_the_harness_was_a_source_reads_a_model_and_effort_none_set_as_the_harnesss()
{
    let read = ran_as_read(&json!([{
        "dir": "sessions/5/rework-1",
        "model": null,
        "effort": null,
        "layers": {
            "settings": {
                "model": "linewright",
                "effort": "linewright",
                "session-timeout": "linewright",
                "session-env": "linewright"
            },
            "prompts": {"rework.md": "linewright"}
        }
    }]));

    assert_eq!(
        read,
        Ran {
            layers: Layers {
                settings: BTreeMap::from([
                    (Setting::MODEL, Source::Harness),
                    (Setting::EFFORT, Source::Harness),
                    (Setting::SESSION_TIMEOUT, Source::Layer(Layer::Linewright)),
                    (Setting::SessionEnv, Source::Layer(Layer::Linewright)),
                ]),
                prompts: BTreeMap::from([("rework.md".to_string(), Layer::Linewright)]),
            },
            ..ran(None, None, None)
        }
    );
}

#[test]
fn a_record_from_when_skills_came_in_layers_reads_its_layers_as_kept() {
    let ran = ran_as_read(&json!([{
        "dir": "sessions/5/rework-1",
        "model": "opus",
        "effort": null,
        "layers": {
            "settings": {"model": "operator", "effort": "linewright"},
            "briefs": {
                "prompts/rework.md": "operator",
                "plugin/skills/tdd/SKILL.md": "linewright"
            }
        }
    }]));

    assert_eq!(
        ran.layers.prompts,
        BTreeMap::from([
            ("prompts/rework.md".to_string(), Layer::Operator),
            ("plugin/skills/tdd/SKILL.md".to_string(), Layer::Linewright),
        ])
    );
}

/// What run each record read is, how it started and how it ended, in the
/// order of their IDs.
fn runs_read(recorded: &[Recorded]) -> Vec<(String, Option<StartedBy>, State)> {
    let mut read: Vec<_> = recorded
        .iter()
        .map(|run| (run.id.to_string(), run.started().by(), run.state))
        .collect();
    read.sort_by(|a, b| a.0.cmp(&b.0));
    read
}

fn done_at(time: &str) -> State {
    State::Ended {
        at: at(time),
        ending: Ending::Done,
    }
}

#[test]
fn an_automations_runs_read_from_where_the_previous_version_kept_them_as_scheduled_and_from_where_they_are_kept()
 {
    let old = json!({
        "workflow": "weekly",
        "n": 1,
        "started_at": "2026-09-28T06:00:00Z",
        "end": {"at": "2026-09-28T06:05:00Z", "ending": "done"},
        "sessions": [],
    });
    let new = json!({
        "automation": "weekly",
        "n": 2,
        "started_by": "hand",
        "started_at": "2026-10-05T06:00:00Z",
        "end": {"at": "2026-10-05T06:05:00Z", "ending": "done"},
        "sessions": [],
    });

    let (_base, recorded) = read(&[
        ("runs/workflows/weekly/1.json", &old.to_string()),
        ("runs/automations/weekly/2.json", &new.to_string()),
    ]);

    assert_eq!(
        runs_read(&recorded),
        [
            (
                "weekly/1".to_string(),
                Some(StartedBy::Schedule),
                done_at("2026-09-28T06:05:00Z"),
            ),
            (
                "weekly/2".to_string(),
                Some(StartedBy::Hand),
                done_at("2026-10-05T06:05:00Z"),
            ),
        ]
    );
}

#[test]
fn a_spec_changes_run_the_previous_version_recorded_as_tickets_reads_as_a_design_run() {
    let record = item_run_record(4, "tickets", 1, Some("handed-back"), &json!([]));

    let (_base, recorded) = read(&[("runs/items/4/tickets-1.json", &record)]);

    assert_eq!(
        runs_read(&recorded),
        [(
            "4/design-1".to_string(),
            None,
            State::Ended {
                at: at("2026-10-14T12:01:00Z"),
                ending: Ending::HandedBack,
            },
        )]
    );
}

#[test]
fn a_stage_the_previous_version_named_tickets_is_none_to_ask_for() {
    assert_eq!(
        "tickets".parse::<WorkKind>(),
        Err(
            "\"tickets\" is not a stage: one of triage, design, implement, rework, \
             fix-ci, update"
                .to_string()
        )
    );
}

#[test]
fn a_run_with_no_end_is_in_flight_while_its_lock_is_held_and_interrupted_once_it_is_not() {
    let record = item_run_record(1, "triage", 1, None, &json!([]));
    let (base, work_dir) = work_dir_holding(&[("runs/items/1/triage-1.json", &record)]);
    let runs = Runs::new(&work_dir.resolve());
    let lock = lock_of(&base.path().join(REPO).join("runs/items/1/triage-1.json"));

    let held = runs_read(&runs.read().unwrap());
    drop(lock);
    let released = runs_read(&runs.read().unwrap());

    assert_eq!(
        [held, released],
        [
            [("1/triage-1".to_string(), None, State::InFlight)],
            [("1/triage-1".to_string(), None, State::Interrupted)],
        ]
    );
}

#[test]
fn runs_whose_records_cannot_be_read_are_none_to_an_answer_that_does_without_them() {
    let (_base, work_dir) = work_dir_holding(&[("runs/items", "not a directory")]);
    let runs = Runs::new(&work_dir.resolve());

    assert!(runs.read().is_err());
    assert_eq!(runs.read_or_none(), []);
}
