use std::panic::Location;
use std::path::{Path, PathBuf};

use crate::benchmarks::{BenchmarkId, Benchmarks, KeptTask, MakeError};
use crate::name::Name;
use crate::runs::{RunId, Runs};
use crate::task::{self, BenchmarkChoice, CaptureError, Criteria, TaskStage};
use crate::testing::{World, back_door, implement_gadgets, open_designed_gadgets, runs_in};

#[track_caller]
pub fn runs(world: &World) -> impl Future<Output = Runs> + use<> {
    runs_in(&world.dir.path().join("work"))
}

pub fn tasks(world: &World) -> PathBuf {
    world.dir.path().join("work").join("tasks")
}

#[track_caller]
pub fn id(id: &str) -> RunId {
    id.parse().unwrap()
}

/// The benchmark of implement tasks the tests capture into.
pub const IMPLEMENTS: &str = "implements";

#[track_caller]
pub fn benchmarks(world: &World) -> impl Future<Output = Benchmarks> + use<> {
    let runs = runs(world);
    async move { Benchmarks::new(runs.await.database().clone()) }
}

/// The ID of the benchmark `name` of `world` of implement tasks, made if
/// missing, also by another capture at once.
#[track_caller]
pub fn made<'a>(world: &'a World, name: &str) -> impl Future<Output = BenchmarkId> + 'a {
    let caller = Location::caller();
    let name = benchmark(name);
    async move {
        let benchmarks = benchmarks(world).await;
        if let Some(found) = benchmarks
            .named(&name)
            .await
            .unwrap_or_else(|e| panic!("finding the benchmark at {caller}: {e:#}"))
        {
            return found.id;
        }
        match benchmarks.make(&name, TaskStage::Implement).await {
            Ok(made) => made.id,
            Err(MakeError::Taken(_)) => {
                benchmarks
                    .named(&name)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| panic!("finding the benchmark made at {caller}"))
                    .id
            }
            Err(e) => panic!("making the benchmark at {caller}: {e:#}"),
        }
    }
}

/// Captures run `run` of `world` as the task `named`, with `criteria`, into
/// the benchmark [`IMPLEMENTS`], made if missing.
#[track_caller]
pub fn capture<'a>(
    world: &'a World,
    run: &'a str,
    named: &'a str,
    criteria: &'a str,
) -> impl Future<Output = Result<KeptTask, CaptureError>> + 'a {
    let made = made(world, IMPLEMENTS);
    Box::pin(async move {
        let implements = made.await;
        capture_into(
            world,
            run,
            named,
            criteria,
            BenchmarkChoice::Existing(implements),
        )
        .await
    })
}

/// Captures run `run` of `world` as the task `named`, with `criteria`, into
/// the benchmark `benchmark`.
#[track_caller]
pub fn capture_into<'a>(
    world: &'a World,
    run: &str,
    named: &str,
    criteria: &str,
    benchmark: BenchmarkChoice,
) -> impl Future<Output = Result<KeptTask, CaptureError>> + 'a {
    let named: Name = named.parse().unwrap();
    let criteria = Criteria::parse(criteria).unwrap();
    let run = id(run);
    let benchmarks = benchmarks(world);
    Box::pin(async move {
        let benchmarks = benchmarks.await;
        let capturable =
            task::capturable(&runs(world).await, &benchmarks, &run, &named, &benchmark).await?;
        task::capture(&*world.store, capturable, &criteria, &benchmarks).await
    })
}

/// Takes the properties out of every input recorded in the repository's work
/// directory `work`, as Linewright recorded inputs before it recorded the
/// properties.
#[track_caller]
pub fn record_as_before_the_properties(work: &Path) -> impl Future<Output = ()> + use<> {
    let caller = Location::caller();
    let pool = back_door(work);
    async move {
        let pool = pool.await;
        sqlx::query("UPDATE run_inputs SET input = json_remove(input, '$.properties')")
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("taking out the properties at {caller}: {e}"));
        pool.close().await;
    }
}

/// An implement run on the ticket "Gadgets", and its ID.
pub async fn implemented(world: &mut World) -> String {
    implement_gadgets(world);
    let ticket = open_designed_gadgets(world);
    world.poll().await;
    format!("{}/implement-1", ticket.0)
}

#[track_caller]
pub fn benchmark(name: &str) -> Name {
    name.parse().unwrap()
}
