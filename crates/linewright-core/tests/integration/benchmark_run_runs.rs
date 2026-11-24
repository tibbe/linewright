//! Benchmark runs run under `serve`, through the system: their trials and
//! scorers, a restart and a drain (`docs/specs/benchmarks.md`, Benchmark runs:
//! Under `serve`, After a restart; `docs/specs/operation.md`, Stopping).

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use linewright_core::api::Sources;
use linewright_core::benchmarks::{
    BenchmarkRunState, Cancelled, ConfigurationId, NoScore, TrialEnd, TrialKey, TrialState,
};
use linewright_core::clock::Clock as _;
use linewright_core::cost::Usd;
use linewright_core::ending::Ends;
use linewright_core::factory::{Factory, FactoryConfig, ServeEnd};
use linewright_core::reconcile::WorkKind;
use linewright_core::runs::{Ending, OutcomeName, RepoWorkDir};
use linewright_core::store::local::LocalStore;
use linewright_core::testing::benchmark_runs::*;
use linewright_core::testing::*;
use linewright_core::trial::{self};
use linewright_core::work_settings::{Setup, SetupCommand};
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// The factory of a world of `store`, `harness` and `clock` started again
/// over the work directory `repo`, as `serve` starts after a restart.
async fn restarted(
    store: Arc<LocalStore>,
    harness: Arc<ScriptedHarness>,
    clock: Arc<FakeClock>,
    repo: &Path,
) -> Factory {
    restarted_with(store, harness, clock, repo, |_| {}).await
}

/// [`restarted`], its config changed by `adjust`.
async fn restarted_with<F: FnOnce(&mut FactoryConfig)>(
    store: Arc<LocalStore>,
    harness: Arc<ScriptedHarness>,
    clock: Arc<FakeClock>,
    repo: &Path,
    adjust: F,
) -> Factory {
    let mut config = config(repo).await;
    config.max_trials = 1;
    adjust(&mut config);
    Factory::start(store, harness, clock, config)
        .await
        .unwrap_or_else(|e| panic!("starting the factory again: {e:#}"))
}

fn key(task: usize, configuration: usize) -> TrialKey {
    TrialKey {
        repetition: NonZeroU32::MIN,
        task,
        configuration,
    }
}

#[test_log::test(tokio::test)]
async fn a_trial_whose_scorer_makes_no_score_keeps_why_as_lw_score_says_it() {
    let (world, work_dir) = with_benchmark(1).await;
    world.harness.on_scorer(|_| {
        Ok(json!({"outcome": "gave-up", "reason": "The output is empty."}).to_string())
    });
    let repo = work_dir.path().join(REPO);
    let (benchmark_runs, id) = launch(&repo, vec![], 1).await;

    let serving = serve(world.factory);
    let record = rests(&benchmark_runs, id, |record| {
        matches!(record.state, BenchmarkRunState::Ended { at: Some(_) })
    })
    .await;
    serving.stop().await;

    let whys: Vec<_> = record
        .trials
        .values()
        .map(|trial| {
            let TrialState::Ended(TrialEnd::Unscored(NoScore::Scorer { why, .. })) = trial else {
                panic!("expected no score from the scorer, got {trial:?}");
            };
            why.as_str()
        })
        .collect();
    assert_eq!(whys.len(), 2, "{whys:?}");
    assert!(
        whys.iter().all(|why| why.contains("The output is empty.")),
        "{whys:?}"
    );
}

/// The factory of `world`, its work directory `repo`, started again with
/// `command` as its setup command, which builds the trials' environment.
async fn with_setup(world: World, repo: &Path, command: &str) -> Factory {
    drop(world.factory);
    restarted_with(world.store, world.harness, world.clock, repo, |config| {
        edit_work(config, |work| {
            work.pipeline_flags_and_env.setup.command = Some(Setup::Command(
                SetupCommand::new(command.to_string()).unwrap(),
            ));
        });
    })
    .await
}

/// `docs/specs/benchmarks.md`, Benchmark runs, A trial without a score: its
/// build failed, which is kept as its reason and not as a run that failed.
#[test_log::test(tokio::test)]
async fn a_trial_whose_build_fails_ends_with_no_score_for_its_build() {
    let (world, work_dir) = with_benchmark(1).await;
    let repo = work_dir.path().join(REPO);
    let factory = with_setup(world, &repo, "echo building; exit 3").await;
    let (benchmark_runs, id) = launch(&repo, vec![], 1).await;

    let serving = serve(factory);
    let record = rests(&benchmark_runs, id, |record| {
        matches!(record.state, BenchmarkRunState::Ended { at: Some(_) })
    })
    .await;
    serving.stop().await;

    assert!(
        record.trials.values().all(|trial| matches!(
            trial,
            TrialState::Ended(TrialEnd::Unscored(NoScore::BuildFailed { .. }))
        )),
        "{:#?}",
        record.trials
    );
}

/// A trial whose environment cannot be had, its build never run, is a run
/// that failed, as any other failure to run.
#[test_log::test(tokio::test)]
async fn a_trial_whose_environment_cannot_be_had_ends_as_a_run_that_failed() {
    let (world, work_dir) = with_benchmark(1).await;
    let repo = work_dir.path().join(REPO);
    let factory = with_setup(world, &repo, "echo building").await;
    std::fs::create_dir_all(repo.join("trials")).unwrap();
    std::fs::write(repo.join("trials/environments"), "not a directory").unwrap();
    let (benchmark_runs, id) = launch(&repo, vec![], 1).await;

    let serving = serve(factory);
    let record = rests(&benchmark_runs, id, |record| {
        matches!(record.state, BenchmarkRunState::Ended { at: Some(_) })
    })
    .await;
    serving.stop().await;

    assert!(
        record.trials.values().all(|trial| matches!(
            trial,
            TrialState::Ended(TrialEnd::Unscored(NoScore::Outcome(run)))
                if run.outcome == OutcomeName::Ended(Ending::Failed)
        )),
        "{:#?}",
        record.trials
    );
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
async fn a_serve_killed_mid_trial_runs_it_again_into_a_new_output_when_restarted() {
    let (world, work_dir) = with_benchmark(1).await;
    let repo = work_dir.path().join(REPO);
    let (benchmark_runs, id) = launch(&repo, vec![], 1).await;
    let mut held = first_triage_held(&world);
    let killed = serve(world.factory);
    held.begun().await;

    killed.kill().await;
    let out = repo.join("benchmark-runs/1/1/1/1");
    std::fs::write(out.join("left-by-the-kill"), "").unwrap();
    let restarted = restarted(world.store, world.harness, world.clock, &repo);
    let serving = serve(restarted.await);
    let record = rests(&benchmark_runs, id, |record| {
        matches!(record.state, BenchmarkRunState::Ended { .. })
    })
    .await;
    serving.stop().await;

    assert!(
        matches!(
            record.trials[&key(0, 0)],
            TrialState::Ended(TrialEnd::Scored { .. })
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
async fn a_serve_killed_mid_scorer_scores_again_keeping_only_the_new_scorers_record() {
    let (world, work_dir) = with_benchmark(1).await;
    let repo = work_dir.path().join(REPO);
    let (benchmark_runs, id) = launch(&repo, vec![], 1).await;
    let (mut held, post) = gate();
    let calls = AtomicUsize::new(0);
    world.harness.on_scorer(move |_| {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            post.wait();
        }
        Ok(passes())
    });
    let killed = serve(world.factory);
    held.begun().await;

    killed.kill().await;
    let scorers = repo.join("benchmark-runs/1/1/1/1/scorer");
    std::fs::create_dir_all(scorers.join("left-by-the-kill")).unwrap();
    let restarted = restarted(world.store, world.harness, world.clock, &repo);
    let serving = serve(restarted.await);
    let record = rests(&benchmark_runs, id, |record| {
        matches!(record.state, BenchmarkRunState::Ended { .. })
    })
    .await;
    serving.stop().await;

    assert!(
        matches!(
            record.trials[&key(0, 0)],
            TrialState::Ended(TrialEnd::Scored { .. })
        ),
        "{:#?}",
        record.trials
    );
    let records: Vec<_> = std::fs::read_dir(&scorers).unwrap().collect();
    assert_eq!(records.len(), 1, "{records:?}");
    assert!(!scorers.join("left-by-the-kill").exists());
    held.release();
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn a_drain_waits_for_the_trial_in_flight_and_starts_no_other() {
    let (world, work_dir) = with_benchmark(1).await;
    let repo = work_dir.path().join(REPO);
    let (benchmark_runs, id) = launch(&repo, vec![], 1).await;
    let mut held = first_triage_held(&world);
    let serving = serve(world.factory);
    held.begun().await;

    serving.drain();
    held.release();
    let end = serving.ended().await;

    assert_eq!(end, ServeEnd::Drained);
    let record = benchmark_runs.show(id).await.unwrap().unwrap();
    assert!(
        matches!(record.trials[&key(0, 0)], TrialState::Ran { .. }),
        "{:#?}",
        record.trials
    );
    assert_eq!(record.trials[&key(1, 0)], TrialState::Pending);
}

/// The local API of `serving`, over the work directory `work_dir`.
async fn api_of(serving: &Serving, world_clock: Arc<FakeClock>, work_dir: &Path) -> axum::Router {
    let work_dir = RepoWorkDir {
        base: work_dir.to_path_buf(),
        repo: REPO.parse().unwrap(),
    };
    let runs = runs_in(&work_dir.resolve()).await;
    let (served, polling, benchmark_runs) = (
        serving.served.clone(),
        serving.polling.clone(),
        serving.benchmark_runs.clone(),
    );
    let sources = Sources {
        serving: linewright_core::status::Serving {
            since: world_clock.now(),
            max_runs: 4,
        },
        served,
        polling,
        deliveries: None,
        clock: world_clock,
        work_dir,
        runs,
        ends: Ends::new(),
        environment: linewright_core::api::EnvironmentLink::default(),
        benchmark_runs,
        exiting: linewright_core::winding_up::WindingUp::default().exiting(),
    };
    linewright_core::api::router(sources, CancellationToken::new())
}

#[test_log::test(tokio::test)]
async fn cancelling_a_benchmark_run_stops_its_trial_in_flight_and_starts_no_other() {
    let (world, work_dir) = with_benchmark(1).await;
    let repo = work_dir.path().join(REPO);
    let (benchmark_runs, id) = launch(&repo, vec![], 1).await;
    let agent = world.harness.hold();
    let serving = serve(world.factory);
    let api = api_of(&serving, world.clock.clone(), work_dir.path()).await;
    let shown = || async {
        let response = routed(
            &api,
            hyper::Method::GET,
            Some("localhost"),
            "/api/benchmark-runs/1",
        );
        status_and_body(response.await).await
    };
    eventually_async(|| async { shown().await == (200, r#"{"state":"running"}"#.to_string()) })
        .await;

    benchmark_runs.cancel(id, world.clock.now()).await.unwrap();
    let nudged = routed(
        &api,
        hyper::Method::POST,
        Some("localhost"),
        "/api/benchmark-runs/changed",
    )
    .await;
    let record = rests(&benchmark_runs, id, |record| {
        matches!(record.trials[&key(0, 0)], TrialState::Ended(_))
    })
    .await;
    let cancelled = shown().await;
    serving.stop().await;
    drop(agent);

    assert_eq!(nudged.status(), 200);
    assert_eq!(cancelled, (200, r#"{"state":"cancelled"}"#.to_string()));
    assert!(
        matches!(record.state, BenchmarkRunState::Cancelled { at: Some(_) }),
        "{:?}",
        record.state
    );
    let TrialState::Ended(TrialEnd::Cancelled(Cancelled::RunStopped(run))) =
        &record.trials[&key(0, 0)]
    else {
        panic!(
            "the cancel stopped the trial's run: {:?}",
            record.trials[&key(0, 0)]
        );
    };
    let kept = trial::kept_spent(&benchmark_runs.output_dir(
        id,
        ConfigurationId::AS_IS,
        record.tasks[0].id,
        NonZeroU32::MIN,
    ))
    .expect("the stopped trial's output keeps its cost");
    assert_eq!(run.outcome, OutcomeName::Ended(Ending::Cancelled));
    assert_eq!(
        run.money,
        Some(Usd::ZERO),
        "the run keeps what it cost: nothing, its one turn held open"
    );
    assert_eq!(run.money, kept.money, "as its output kept it");
    assert!(run.time > Duration::ZERO, "the run keeps its time: {run:?}");
    assert_eq!(record.trials[&key(1, 0)], TrialState::Pending);
}
