use std::num::NonZeroU32;
use std::panic::Location;
use std::path::Path;
use std::time::Duration;

use serde_json::json;

use crate::benchmarks::{BenchmarkRunId, BenchmarkRunRecord, BenchmarkRuns, Benchmarks};
use crate::factory::Factory;
use crate::name::Name;
use crate::reconcile::WorkKind;
use crate::task::{self, BenchmarkChoice, Criteria};
use crate::testing::{REPO, Serving, World, at, runs_in, triaged};
use crate::trial::Configuration;
use crate::webhook::Pending;

/// A scorer's result: a pass, and no leak.
pub fn passes() -> String {
    json!({
        "outcome": "judged",
        "correctness": {"reasoning": "It triaged the issue.", "verdict": "pass"},
        "leak": {"reasoning": "I read every transcript.", "found": false, "calls": []},
    })
    .to_string()
}

/// A world whose factory runs `max_trials` trials and scorers at once,
/// its work directory `REPO`'s in the directory given with it, holding the
/// triage benchmark `triage`, 1, of the tasks `export`, 1, and `import`, 2,
/// captured from the triage of two issues, then closed. Each scorer passes.
#[track_caller]
pub fn with_benchmark(max_trials: usize) -> impl Future<Output = (World, tempfile::TempDir)> {
    let caller = Location::caller();
    async move {
        let work_dir = tempfile::tempdir().unwrap();
        let repo = work_dir.path().join(REPO);
        let mut world = World::with(|config| {
            config.work_dir.clone_from(&repo);
            config.max_trials = max_trials;
        })
        .await;
        world.harness.on(WorkKind::Triage, |_| Ok(triaged()));
        world.harness.on_scorer(|_| Ok(passes()));
        let issues =
            [("export", "Export fails"), ("import", "Import fails")].map(|(task, title)| {
                let issue =
                    world
                        .store
                        .open_issue("reporter", title, "It crashes.", &["needs-triage"]);
                (task, issue)
            });
        world.poll().await;
        let runs = runs_in(&repo).await;
        let benchmarks = Benchmarks::new(runs.database().clone());
        let mut choice = BenchmarkChoice::New("triage".parse().unwrap());
        for (task, issue) in issues {
            let run = format!("{}/triage-1", issue.0).parse().unwrap();
            let name: Name = task.parse().unwrap();
            let capturable = task::capturable(&runs, &benchmarks, &run, &name, &choice)
                .await
                .unwrap_or_else(|e| panic!("capturing {run} at {caller}: {e}"));
            let criteria = Criteria::parse("It triages the issue.\n").unwrap();
            let kept = task::capture(&*world.store, capturable, &criteria, &benchmarks)
                .await
                .unwrap_or_else(|e| panic!("capturing {run} at {caller}: {e}"));
            choice = BenchmarkChoice::Existing(kept.benchmark.id);
            world.store.close_issue_as_completed(issue).unwrap();
        }
        (world, work_dir)
    }
}

/// Launches a benchmark run on the benchmark `triage`, 1, of the database in
/// `repo`, under `as-is` and `configurations`, `repetitions` times.
#[track_caller]
pub fn launch(
    repo: &Path,
    configurations: Vec<(Name, Configuration)>,
    repetitions: u32,
) -> impl Future<Output = (BenchmarkRuns, BenchmarkRunId)> + use<> {
    let caller = Location::caller();
    let repo = repo.to_path_buf();
    async move {
        let benchmark_runs = BenchmarkRuns::new(runs_in(&repo).await.database().clone());
        let launched = benchmark_runs
            .launch(
                "1".parse().unwrap(),
                configurations,
                NonZeroU32::new(repetitions).unwrap(),
                at("2026-11-17T12:00:00Z"),
            )
            .await
            .unwrap_or_else(|e| panic!("launching at {caller}: {e:#}"));
        (benchmark_runs, launched.id)
    }
}

/// Waits up to a minute for the benchmark run `id` to come to rest as `done`
/// says, and gives it.
#[track_caller]
pub fn rests<'a>(
    benchmark_runs: &'a BenchmarkRuns,
    id: BenchmarkRunId,
    done: impl Fn(&BenchmarkRunRecord) -> bool + 'a,
) -> impl Future<Output = BenchmarkRunRecord> + 'a {
    let caller = Location::caller();
    async move {
        let waited = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let record = benchmark_runs
                    .show(id)
                    .await
                    .unwrap_or_else(|e| panic!("reading benchmark run {id} at {caller}: {e:#}"))
                    .unwrap_or_else(|| panic!("no benchmark run {id} at {caller}"));
                if done(&record) {
                    return record;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        waited.await.unwrap_or_else(|_| {
            panic!("benchmark run {id} did not come to rest, waited at {caller}")
        })
    }
}

pub fn serve(factory: Factory) -> Serving {
    Serving::start(factory, Duration::from_hours(1), Pending::default())
}
