//! Experiments run under `serve`, through the system: their replays and
//! judges, a restart and a drain (`docs/specs/benchmarks.md`, Experiments:
//! Under `serve`, After a restart; `docs/specs/operation.md`, Stopping).

use crate::common;
use crate::replay::triaged;

use std::num::NonZeroU32;
use std::panic::Location;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::*;
use linewright::api::Sources;
use linewright::benchmarks::{
    ConfigurationName, ExperimentId, ExperimentRecord, ExperimentState, Experiments, Suites,
    TrialEnd, TrialKey, TrialState,
};
use linewright::case::{self, CaseName, Criteria, SuiteChoice};
use linewright::clock::Clock as _;
use linewright::ending::Ends;
use linewright::factory::{Factory, ServeEnd};
use linewright::reconcile::WorkKind;
use linewright::replay::Configuration;
use linewright::runs::RepoWorkDir;
use linewright::store::local::LocalStore;
use linewright::webhook::Pending;
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// A judge's result: a pass, and no leak.
fn passes() -> String {
    json!({
        "outcome": "judged",
        "correctness": {"reasoning": "It triaged the issue.", "verdict": "pass"},
        "leak": {"reasoning": "I read every transcript.", "found": false, "calls": []},
    })
    .to_string()
}

/// A world whose factory runs `max_replays` replays and judges at once,
/// its work directory `REPO`'s in the directory given with it, holding the
/// triage suite `triage` of the cases `export` and `import`, captured from
/// the triage of two issues, then closed. Each judge passes.
#[track_caller]
fn with_suite(max_replays: usize) -> impl Future<Output = (World, tempfile::TempDir)> {
    let caller = Location::caller();
    async move {
        let work_dir = tempfile::tempdir().unwrap();
        let repo = work_dir.path().join(REPO);
        let mut world = World::with(|config| {
            config.work_dir = repo.clone();
            config.max_replays = max_replays;
        })
        .await;
        world.harness.on(WorkKind::Triage, |_| Ok(triaged()));
        world.harness.on_judge(|_| Ok(passes()));
        let issues =
            [("export", "Export fails"), ("import", "Import fails")].map(|(case, title)| {
                let issue =
                    world
                        .store
                        .open_issue("reporter", title, "It crashes.", &["needs-triage"]);
                (case, issue)
            });
        world.poll().await;
        let runs = runs_in(&repo).await;
        let suites = Suites::new(runs.database().clone());
        for (n, (case, issue)) in issues.into_iter().enumerate() {
            let suite = "triage".parse().unwrap();
            let choice = if n == 0 {
                SuiteChoice::New(suite)
            } else {
                SuiteChoice::Named(suite)
            };
            let run = format!("{}/triage-1", issue.0).parse().unwrap();
            let name: CaseName = case.parse().unwrap();
            let capturable = case::capturable(&runs, &suites, &run, &name, &choice)
                .await
                .unwrap_or_else(|e| panic!("capturing {run} at {caller}: {e}"));
            let criteria = Criteria::parse("It triages the issue.\n").unwrap();
            case::capture(&*world.store, capturable, &criteria, &suites)
                .await
                .unwrap_or_else(|e| panic!("capturing {run} at {caller}: {e}"));
            world.store.close_issue_as_completed(issue).unwrap();
        }
        (world, work_dir)
    }
}

/// Launches an experiment on the suite `triage` of the database in `repo`,
/// under `as-is` and `configurations`, `repetitions` times.
#[track_caller]
fn launch(
    repo: &Path,
    configurations: Vec<(ConfigurationName, Configuration)>,
    repetitions: u32,
) -> impl Future<Output = (Experiments, ExperimentId)> + use<> {
    let caller = Location::caller();
    let repo = repo.to_path_buf();
    async move {
        let experiments = Experiments::new(runs_in(&repo).await.database().clone());
        let launched = experiments
            .launch(
                &"triage".parse().unwrap(),
                configurations,
                NonZeroU32::new(repetitions).unwrap(),
                at("2026-11-17T12:00:00Z"),
            )
            .await
            .unwrap_or_else(|e| panic!("launching at {caller}: {e:#}"));
        (experiments, launched.id)
    }
}

/// Waits up to a minute for the experiment `id` to come to rest as `done`
/// says, and gives it.
#[track_caller]
fn rests<'a>(
    experiments: &'a Experiments,
    id: ExperimentId,
    done: impl Fn(&ExperimentRecord) -> bool + 'a,
) -> impl Future<Output = ExperimentRecord> + 'a {
    let caller = Location::caller();
    async move {
        let waited = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let record = experiments
                    .show(id)
                    .await
                    .unwrap_or_else(|e| panic!("reading experiment {id} at {caller}: {e:#}"))
                    .unwrap_or_else(|| panic!("no experiment {id} at {caller}"));
                if done(&record) {
                    return record;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        waited
            .await
            .unwrap_or_else(|_| panic!("experiment {id} did not come to rest, waited at {caller}"))
    }
}

/// The factory of a world of `store`, `harness` and `clock` started again
/// over the work directory `repo`, as `serve` starts after a restart.
#[track_caller]
fn restarted(
    store: Arc<LocalStore>,
    harness: Arc<ScriptedHarness>,
    clock: Arc<FakeClock>,
    repo: &Path,
) -> impl Future<Output = Factory> + use<> {
    let caller = Location::caller();
    let config = config(repo);
    async move {
        let mut config = config.await;
        config.max_replays = 1;
        Factory::start(store, harness, clock, config)
            .await
            .unwrap_or_else(|e| panic!("starting the factory again at {caller}: {e:#}"))
    }
}

fn serve(factory: Factory) -> Serving {
    Serving::start(factory, Duration::from_hours(1), Pending::default())
}

fn key(case: usize, configuration: usize) -> TrialKey {
    TrialKey {
        repetition: NonZeroU32::MIN,
        case,
        configuration,
    }
}

#[test_log::test(tokio::test)]
async fn serve_replays_and_judges_every_trial_and_the_cli_shows_the_results() {
    let (world, work_dir) = with_suite(2).await;
    let repo = work_dir.path().join(REPO);
    let short = tempfile::tempdir().unwrap();
    std::fs::write(short.path().join("config.toml"), "model = \"haiku\"\n").unwrap();
    let configuration = Configuration::read(short.path()).unwrap();
    let (experiments, id) = launch(&repo, vec![("short".parse().unwrap(), configuration)], 2).await;

    let serving = serve(world.factory);
    let record = rests(&experiments, id, |record| {
        record.state == ExperimentState::Ended
    })
    .await;
    serving.stop().await;

    assert!(
        record
            .trials
            .values()
            .all(|trial| matches!(trial, TrialState::Ended(TrialEnd::Judged { .. }))),
        "{:#?}",
        record.trials
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("experiments/1/short/import/2/outcome")).unwrap(),
        "done\n"
    );
    let shown = lw_in(work_dir.path(), &["experiment", "1"]).await;
    assert_eq!(shown.code, Some(0), "{}", shown.stderr);
    for line in [
        "state: ended\n",
        "replays ended: 8 of 8\n",
        "configuration as-is:\n  pass rate: 100%\n  passes: export 2 of 2, import 2 of 2\n",
        "configuration short:\n  pass rate: 100%\n  passes: export 2 of 2, import 2 of 2\n",
        "replay short/import/2: judged pass",
    ] {
        assert!(
            shown.stdout.contains(line),
            "{line:?} in:\n{}",
            shown.stdout
        );
    }
}

/// Holds the first triage session open until the gate releases it, then
/// answers it, as every later one, with a triage.
fn first_triage_held(world: &World) -> Gate {
    let (gate, post) = gate();
    let calls = AtomicUsize::new(0);
    world.harness.on(WorkKind::Triage, move |_| {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            post.wait();
        }
        Ok(triaged())
    });
    gate
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn a_serve_killed_mid_replay_replays_it_again_into_a_new_output_when_restarted() {
    let (world, work_dir) = with_suite(1).await;
    let repo = work_dir.path().join(REPO);
    let (experiments, id) = launch(&repo, vec![], 1).await;
    let mut held = first_triage_held(&world);
    let killed = serve(world.factory);
    held.begun().await;

    killed.kill().await;
    let out = repo.join("experiments/1/as-is/export/1");
    std::fs::write(out.join("left-by-the-kill"), "").unwrap();
    let restarted = restarted(world.store, world.harness, world.clock, &repo);
    let serving = serve(restarted.await);
    let record = rests(&experiments, id, |record| {
        record.state == ExperimentState::Ended
    })
    .await;
    serving.stop().await;

    assert!(
        matches!(
            record.trials[&key(0, 0)],
            TrialState::Ended(TrialEnd::Judged { .. })
        ),
        "{:#?}",
        record.trials
    );
    assert_eq!(
        std::fs::read_to_string(out.join("outcome")).unwrap(),
        "done\n"
    );
    assert!(!out.join("left-by-the-kill").exists());
    held.release();
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn a_serve_killed_mid_judge_judges_again_keeping_only_the_new_judges_record() {
    let (world, work_dir) = with_suite(1).await;
    let repo = work_dir.path().join(REPO);
    let (experiments, id) = launch(&repo, vec![], 1).await;
    let (mut held, post) = gate();
    let calls = AtomicUsize::new(0);
    world.harness.on_judge(move |_| {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            post.wait();
        }
        Ok(passes())
    });
    let killed = serve(world.factory);
    held.begun().await;

    killed.kill().await;
    let judges = repo.join("experiments/1/as-is/export/1/judge");
    std::fs::create_dir_all(judges.join("left-by-the-kill")).unwrap();
    let restarted = restarted(world.store, world.harness, world.clock, &repo);
    let serving = serve(restarted.await);
    let record = rests(&experiments, id, |record| {
        record.state == ExperimentState::Ended
    })
    .await;
    serving.stop().await;

    assert!(
        matches!(
            record.trials[&key(0, 0)],
            TrialState::Ended(TrialEnd::Judged { .. })
        ),
        "{:#?}",
        record.trials
    );
    let records: Vec<_> = std::fs::read_dir(&judges).unwrap().collect();
    assert_eq!(records.len(), 1, "{records:?}");
    assert!(!judges.join("left-by-the-kill").exists());
    held.release();
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn a_drain_waits_for_the_replay_in_flight_and_starts_no_other() {
    let (world, work_dir) = with_suite(1).await;
    let repo = work_dir.path().join(REPO);
    let (experiments, id) = launch(&repo, vec![], 1).await;
    let mut held = first_triage_held(&world);
    let serving = serve(world.factory);
    held.begun().await;

    serving.drain();
    held.release();
    let end = serving.ended().await;

    assert_eq!(end, ServeEnd::Drained);
    let record = experiments.show(id).await.unwrap().unwrap();
    assert!(
        matches!(record.trials[&key(0, 0)], TrialState::Replayed { .. }),
        "{:#?}",
        record.trials
    );
    assert_eq!(record.trials[&key(1, 0)], TrialState::Pending);
}

/// The local API of `serving`, over the work directory `work_dir`.
fn api_of(
    serving: &Serving,
    world_clock: Arc<FakeClock>,
    work_dir: &Path,
) -> impl Future<Output = axum::Router> + use<> {
    let work_dir = RepoWorkDir {
        base: work_dir.to_path_buf(),
        repo: REPO.parse().unwrap(),
    };
    let runs = runs_in(&work_dir.resolve());
    let (served, polling, experiments) = (
        serving.served.clone(),
        serving.polling.clone(),
        serving.experiments.clone(),
    );
    async move {
        let sources = Sources {
            serving: linewright::status::Serving {
                since: world_clock.now(),
                max_runs: 4,
            },
            served,
            polling,
            deliveries: None,
            clock: world_clock,
            work_dir,
            runs: runs.await,
            ends: Ends::new(),
            environment: linewright::api::EnvironmentLink::default(),
            experiments,
            exiting: linewright::winding_up::WindingUp::default().exiting(),
        };
        linewright::api::router(sources, CancellationToken::new())
    }
}

#[test_log::test(tokio::test)]
async fn cancelling_an_experiment_stops_its_replay_in_flight_and_starts_no_other() {
    let (world, work_dir) = with_suite(1).await;
    let repo = work_dir.path().join(REPO);
    let (experiments, id) = launch(&repo, vec![], 1).await;
    let agent = world.harness.hold();
    let serving = serve(world.factory);
    let api = api_of(&serving, world.clock.clone(), work_dir.path()).await;
    let shown = || async {
        let response = routed(
            &api,
            hyper::Method::GET,
            Some("localhost"),
            "/api/experiments/1",
        );
        status_and_body(response.await).await
    };
    eventually_async(|| async { shown().await == (200, r#"{"state":"running"}"#.to_string()) })
        .await;

    experiments.cancel(id).await.unwrap();
    let nudged = routed(
        &api,
        hyper::Method::POST,
        Some("localhost"),
        "/api/experiments/changed",
    )
    .await;
    let record = rests(&experiments, id, |record| {
        matches!(record.trials[&key(0, 0)], TrialState::Ended(_))
    })
    .await;
    let cancelled = shown().await;
    serving.stop().await;
    drop(agent);

    assert_eq!(nudged.status(), 200);
    assert_eq!(cancelled, (200, r#"{"state":"cancelled"}"#.to_string()));
    assert_eq!(record.state, ExperimentState::Cancelled);
    assert_eq!(
        record.trials[&key(0, 0)],
        TrialState::Ended(TrialEnd::Cancelled)
    );
    assert_eq!(record.trials[&key(1, 0)], TrialState::Pending);
}
