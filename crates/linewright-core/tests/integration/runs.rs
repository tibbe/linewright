use std::collections::BTreeMap;

use chrono::TimeDelta;
use linewright_core::domain::Number;
use linewright_core::layer::Layer;
use linewright_core::reconcile::{EndedRun, WorkKind};
use linewright_core::runs::{
    self, Ending, Layers, Listing, NoRuns, Query, Ran, Recorded, RunOf, SessionEntry, SessionKept,
    Started, StartedBy, State,
};
use linewright_core::testing::runs::*;
use linewright_core::testing::*;
use linewright_core::work_settings::{Effort, Setting};
use serde_json::json;

#[test_log::test(tokio::test)]
async fn a_run_that_starts_no_session_is_recorded_with_none_linked() {
    let work_dir = tempfile::tempdir().unwrap();
    let mut world = World::with_repository(Repository::requiring_up_to_date(), |config| {
        config.work_dir = work_dir.path().join(REPO);
    })
    .await;
    let (_, pr) = world.implement_a_ticket().await;
    world.approve_behind(pr).await;
    world.poll().await;

    let recorded = runs_in(&work_dir.path().join(REPO))
        .await
        .read()
        .await
        .unwrap();

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

#[test_log::test(tokio::test)]
async fn an_automations_run_by_hand_is_marked_as_one_and_a_scheduled_one_is_not() {
    let (_world, work_dir) = hourly_by_schedule_then_by_hand().await;

    let recorded = runs_in(&work_dir.path().join(REPO))
        .await
        .read()
        .await
        .unwrap();

    let mut started: Vec<(String, Option<StartedBy>)> = recorded
        .iter()
        .map(|run| (run.id.to_string(), run.started().by()))
        .collect();
    started.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        started,
        [
            ("hourly.md/1".to_string(), Some(StartedBy::Schedule)),
            ("hourly.md/2".to_string(), Some(StartedBy::Hand)),
        ]
    );
}

fn weekly(n: u32, start: &str) -> Recorded {
    let of = RunOf::Automation {
        automation: "weekly.md".parse().unwrap(),
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

#[test_log::test]
fn every_run_is_listed_newest_first() {
    let listing = runs::list(morning(), &Query::default(), at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(
        ids(&listing),
        [
            "1/rework-2",
            "1/rework-1",
            "2/triage-1",
            "1/implement-1",
            "weekly.md/1",
            "1/triage-1"
        ]
    );
    assert_eq!(listing.left_out, 0);
}

#[test_log::test]
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

#[test_log::test]
fn a_pull_requests_runs_are_its_own() {
    let query = Query {
        item: Some(Number(3)),
        ..Query::default()
    };

    let listing = runs::list(ticket_and_pull(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["3/rework-2", "3/fix-ci-1", "3/rework-1"]);
}

#[test_log::test]
fn a_tickets_runs_are_its_own_and_none_of_its_pull_requests() {
    let query = Query {
        item: Some(Number(1)),
        ..Query::default()
    };

    let listing = runs::list(ticket_and_pull(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["1/implement-1", "1/triage-1"]);
}

#[test_log::test]
fn an_item_with_no_runs_has_none_to_list() {
    let query = Query {
        item: Some(Number(3)),
        ..Query::default()
    };

    let listed = runs::list(morning(), &query, at("2026-10-14T13:00:00Z"));

    assert_eq!(listed, Err(NoRuns(Number(3))));
}

#[test_log::test]
fn stage_lists_only_that_stages_runs() {
    let query = Query {
        stage: Some(WorkKind::Triage),
        ..Query::default()
    };

    let listing = runs::list(morning(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["2/triage-1", "1/triage-1"]);
}

#[test_log::test]
fn since_lists_only_the_runs_started_then_or_after() {
    let query = Query {
        since: Some(at("2026-10-14T10:30:00Z")),
        ..Query::default()
    };

    let listing = runs::list(morning(), &query, at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(ids(&listing), ["1/rework-2", "1/rework-1", "2/triage-1"]);
}

#[test_log::test]
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

/// `docs/specs/ui_patterns.md`, Time, Durations.
#[test_log::test]
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

#[test_log::test]
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

#[test_log::test]
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

fn of_item(item: u64) -> Query {
    Query {
        item: Some(Number(item)),
        ..Query::default()
    }
}

#[test_log::test]
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
         session 2: model not recorded, effort not recorded\n  \
         session 3: model and effort not recorded\n"
    );
}

#[test_log::test]
fn an_items_runs_as_json_give_what_each_session_ran_with_and_where_its_settings_and_brief_files_came_from()
 {
    let mut run = work(1, WorkKind::Triage, 1, "2026-10-14T12:00:00Z");
    run.sessions = vec![
        session(
            1,
            Some(Ran {
                layers: Layers {
                    settings: BTreeMap::from([
                        (Setting::MODEL, Layer::Operator),
                        (Setting::EFFORT, Layer::Linewright),
                        (Setting::SESSION_TIMEOUT, Layer::Repository),
                    ]),
                    briefs: BTreeMap::from([
                        ("prompts/triage.md".to_string(), Layer::Linewright),
                        (
                            "prompts/follow-ups/result-error.md".to_string(),
                            Layer::Operator,
                        ),
                    ]),
                },
                ..ran(Some("opus"), Some("claude-opus-5-5"), Some(Effort::Medium))
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
                        "effort": "medium",
                        "layers": {
                            "settings": {
                                "model": "operator",
                                "effort": "linewright",
                                "session-timeout": "repository",
                            },
                            "briefs": {
                                "prompts/follow-ups/result-error.md": "operator",
                                "prompts/triage.md": "linewright",
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
        automation: "weekly.md".parse().unwrap(),
    };
    let mut run = ended(of, n, Some(StartedBy::Hand), start);
    run.state = State::Ended {
        at: at(start) + TimeDelta::minutes(1),
        ending,
    };
    run
}

#[test_log::test]
fn an_automations_run_says_whether_it_was_by_hand_and_how_it_ended() {
    let recorded = vec![
        weekly(1, "2026-10-14T09:00:00Z"),
        weekly_by_hand(2, "2026-10-14T10:00:00Z", Ending::Failed),
    ];

    let listing = runs::list(recorded, &Query::default(), at("2026-10-14T13:00:00Z")).unwrap();

    assert_eq!(
        runs::render(&listing, false),
        "weekly.md/2: failed — automation weekly.md by hand, started 2026-10-14 10:00:00 UTC, ran 1m\n\
         weekly.md/1: done — automation weekly.md, started 2026-10-14 09:00:00 UTC, ran 1m\n"
    );
}

#[test_log::test]
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
                    "id": "weekly.md/2",
                    "automation": "weekly.md",
                    "started_by": "hand",
                    "started_at": "2026-10-14T10:00:00Z",
                    "duration_seconds": 60,
                    "outcome": "done",
                },
                {
                    "id": "weekly.md/1",
                    "automation": "weekly.md",
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

#[test_log::test]
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

/// What `Runs` reads from a work directory whose database keeps each of
/// `records`.
async fn read(records: &[&str]) -> (tempfile::TempDir, Vec<Recorded>) {
    let (base, work_dir) = work_dir_keeping(records, &[]).await;
    let recorded = runs_in(&work_dir.resolve())
        .await
        .read()
        .await
        .unwrap_or_else(|e| panic!("reading the runs: {e:#}"));
    (base, recorded)
}

/// What `Runs` reads of #5's first rework, done in a minute, whose record
/// gives its sessions as `sessions`.
async fn rework_of_5_read(sessions: &serde_json::Value) -> (tempfile::TempDir, Vec<Recorded>) {
    let record = item_run_record(5, "rework", 1, Some("done"), sessions);
    read(&[&record]).await
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

#[test_log::test(tokio::test)]
async fn a_record_from_before_runs_kept_their_pull_request_reads_as_its_run() {
    let (_base, recorded) = rework_of_5_read(&json!([])).await;

    assert_eq!(recorded, [rework_of_5(vec![])]);
}

#[test_log::test(tokio::test)]
async fn a_record_from_before_sessions_kept_what_they_ran_with_reads_none_of_it() {
    let (base, recorded) = rework_of_5_read(&json!(["sessions/5/rework-1"])).await;

    assert_eq!(
        recorded,
        [rework_of_5(vec![SessionEntry {
            dir: base.path().join(REPO).join("sessions/5/rework-1"),
            kept: None,
        }])]
    );
}

/// What the one session of #5's first rework ran with, as read from a record
/// giving its sessions as `sessions`.
async fn ran_as_read(sessions: &serde_json::Value) -> Ran {
    let (_base, recorded) = rework_of_5_read(sessions).await;
    let [run] = &recorded[..] else {
        panic!("one run: {recorded:#?}");
    };
    let [
        SessionEntry {
            kept: Some(SessionKept { ran, .. }),
            ..
        },
    ] = &run.sessions[..]
    else {
        panic!("one session with what it ran with: {run:#?}");
    };
    ran.clone()
}

#[test_log::test(tokio::test)]
async fn a_record_from_before_the_harness_was_a_source_reads_a_model_and_effort_none_set_as_not_recorded()
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
    }]))
    .await;

    assert_eq!(
        read,
        Ran {
            layers: Layers {
                settings: BTreeMap::from([
                    (Setting::SESSION_TIMEOUT, Layer::Linewright),
                    (Setting::SESSION_ENV, Layer::Linewright),
                ]),
                briefs: BTreeMap::from([("prompts/rework.md".to_string(), Layer::Linewright)]),
            },
            ..ran(None, None, None)
        }
    );
}

#[test_log::test(tokio::test)]
async fn a_record_from_when_the_harness_was_a_source_reads_what_it_left_to_the_harness_as_not_recorded()
 {
    let read = ran_as_read(&json!([{
        "dir": "sessions/5/rework-1",
        "model": "opus",
        "reported_model": "claude-opus-5-5",
        "effort": null,
        "layers": {
            "settings": {
                "model": "operator",
                "effort": "harness",
                "session-timeout": "linewright"
            },
            "briefs": {"prompts/rework.md": "linewright"}
        }
    }]))
    .await;

    assert_eq!(
        read.layers.settings,
        BTreeMap::from([
            (Setting::MODEL, Layer::Operator),
            (Setting::SESSION_TIMEOUT, Layer::Linewright),
        ])
    );
    let mut run = work(5, WorkKind::Rework, 1, "2026-10-14T12:00:00Z");
    run.sessions = vec![session(1, Some(read))];
    let listing = runs::list(vec![run], &of_item(5), at("2026-10-14T13:00:00Z")).unwrap();
    let shown = runs::render(&listing, false);
    assert!(
        shown.contains("session 1: model claude-opus-5-5, effort not recorded"),
        "{shown}"
    );
}

#[test_log::test(tokio::test)]
async fn a_record_from_when_only_the_prompts_came_in_layers_reads_them_by_their_paths_under_briefs()
{
    let ran = ran_as_read(&json!([{
        "dir": "sessions/5/rework-1",
        "model": "opus",
        "effort": null,
        "layers": {
            "settings": {"model": "operator", "effort": "linewright"},
            "prompts": {
                "rework.md": "operator",
                "follow-ups/result-error.md": "repository"
            }
        }
    }]))
    .await;

    assert_eq!(
        ran.layers.briefs,
        BTreeMap::from([
            ("prompts/rework.md".to_string(), Layer::Operator),
            (
                "prompts/follow-ups/result-error.md".to_string(),
                Layer::Repository
            ),
        ])
    );
}

#[test_log::test(tokio::test)]
async fn a_record_from_when_skills_came_in_layers_reads_its_layers_as_kept() {
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
    }]))
    .await;

    assert_eq!(
        ran.layers.briefs,
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

#[test_log::test]
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

#[test_log::test(tokio::test)]
async fn a_run_with_no_end_is_in_flight_while_its_lock_is_held_and_interrupted_once_it_is_not() {
    let record = item_run_record(1, "triage", 1, None, &json!([]));
    let (base, work_dir) = work_dir_keeping(&[&record], &[]).await;
    let runs = runs_in(&work_dir.resolve()).await;
    let lock = lock_of(&base.path().join(REPO).join("runs/items/1/triage-1.json"));

    let held = runs_read(&runs.read().await.unwrap());
    drop(lock);
    let released = runs_read(&runs.read().await.unwrap());

    assert_eq!(
        [held, released],
        [
            [("1/triage-1".to_string(), None, State::InFlight)],
            [("1/triage-1".to_string(), None, State::Interrupted)],
        ]
    );
}

#[test_log::test(tokio::test)]
async fn runs_whose_records_cannot_be_read_are_none_to_an_answer_that_does_without_them() {
    let (_base, work_dir) = work_dir_holding(&[]);
    let runs = runs_in(&work_dir.resolve()).await;
    let unreadable_session = json!([{"dir": "sessions/1/triage-1", "model": 5}]);
    keep_record(
        &work_dir.resolve(),
        &serde_json::from_str(&item_run_record(1, "triage", 1, None, &unreadable_session)).unwrap(),
    )
    .await;

    assert!(runs.read().await.is_err());
    assert_eq!(runs.read_or_none().await, []);
}

#[test_log::test(tokio::test)]
async fn the_runs_ended_on_a_pull_request_are_those_whose_record_has_an_end_however_it_ended() {
    let ended = item_run_record(20, "fix-ci", 1, Some("handed-back"), &json!([]));
    let interrupted = item_run_record(20, "fix-ci", 2, None, &json!([]));
    let on_an_issue = item_run_record(2, "implement", 1, Some("done"), &json!([]));
    let (_base, work_dir) = work_dir_keeping(&[&ended, &interrupted, &on_an_issue], &[]).await;
    let runs = runs_in(&work_dir.resolve()).await;

    let ended_on_pulls: Vec<EndedRun> = runs
        .read()
        .await
        .unwrap()
        .iter()
        .filter_map(Recorded::ended_on_pull)
        .collect();

    assert_eq!(
        ended_on_pulls,
        [EndedRun {
            pr: Number(20),
            kind: WorkKind::FixCi,
            ending: Ending::HandedBack,
            at: at("2026-10-14T12:01:00Z"),
            answered_until: None,
        }]
    );
}
