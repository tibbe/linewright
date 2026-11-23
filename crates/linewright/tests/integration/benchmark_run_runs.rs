//! Benchmark runs run under `serve`, through the system: their trials and
//! scorers, a restart and a drain (`docs/specs/benchmarks.md`, Benchmark runs:
//! Under `serve`, After a restart; `docs/specs/operation.md`, Stopping).

use linewright_core::benchmarks::{BenchmarkRunState, TrialEnd, TrialState};
use linewright_core::testing::benchmark_runs::*;
use linewright_core::testing::*;
use linewright_core::trial::Configuration;

use crate::programs::*;

#[test_log::test(tokio::test)]
async fn serve_runs_and_scores_every_trial_and_the_cli_shows_the_results() {
    let (world, work_dir) = with_benchmark(2).await;
    let repo = work_dir.path().join(REPO);
    let short = tempfile::tempdir().unwrap();
    std::fs::write(short.path().join("config.toml"), "model = \"haiku\"\n").unwrap();
    let configuration = Configuration::read(short.path()).unwrap();
    let (benchmark_runs, id) =
        launch(&repo, vec![("short".parse().unwrap(), configuration)], 2).await;

    let serving = serve(world.factory);
    let record = rests(&benchmark_runs, id, |record| {
        matches!(record.state, BenchmarkRunState::Ended { at: Some(_) })
    })
    .await;
    serving.stop().await;

    assert!(
        record
            .trials
            .values()
            .all(|trial| matches!(trial, TrialState::Ended(TrialEnd::Scored { .. }))),
        "{:#?}",
        record.trials
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("benchmark-runs/1/2/2/2/outcome")).unwrap(),
        "done\n"
    );
    let shown = lw_in(work_dir.path(), &["benchmark-run", "show", "1"]).await;
    assert_eq!(shown.code, Some(0), "{}", shown.stderr);
    for line in [
        "state: ended\n",
        "trials ended: 8 of 8\n",
        "configuration 1 (as-is):\n  pass rate: 100%\n",
        "configuration 2 (short):\n  pass rate: 100%\n",
        "  task 1 (export):\n    passes: 2 of 2\n",
        "  task 2 (import):\n    passes: 2 of 2\n",
        "trial 2 of import under short (2/2/2): scored pass",
    ] {
        assert!(
            shown.stdout.contains(line),
            "{line:?} in:\n{}",
            shown.stdout
        );
    }
}
