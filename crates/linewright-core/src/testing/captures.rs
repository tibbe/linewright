use std::path::{Path, PathBuf};

use crate::benchmarks::{BenchmarkId, Benchmarks, KeptTask, MakeError};
use crate::name::Name;
use crate::runs::{RunId, Runs};
use crate::task::{self, BenchmarkChoice, CaptureError, Criteria, TaskStage};
use crate::testing::{World, back_door, implement_gadgets, open_designed_gadgets, runs_in};

pub async fn runs(world: &World) -> Runs {
    runs_in(&world.dir.path().join("work")).await
}

pub fn tasks(world: &World) -> PathBuf {
    world.dir.path().join("work").join("tasks")
}

pub fn id(id: &str) -> RunId {
    id.parse().unwrap()
}

/// The benchmark of implement tasks the tests capture into.
pub const IMPLEMENTS: &str = "implements";

pub async fn benchmarks(world: &World) -> Benchmarks {
    Benchmarks::new(runs(world).await.database().clone())
}

/// The ID of the benchmark `name` of `world` of implement tasks, made if
/// missing, also by another capture at once.
pub async fn made(world: &World, name: &str) -> BenchmarkId {
    let name = benchmark(name);
    let benchmarks = benchmarks(world).await;
    if let Some(found) = benchmarks
        .named(&name)
        .await
        .unwrap_or_else(|e| panic!("finding the benchmark: {e:#}"))
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
                .unwrap_or_else(|| panic!("finding the benchmark made"))
                .id
        }
        Err(e) => panic!("making the benchmark: {e:#}"),
    }
}

/// Captures run `run` of `world` as the task `named`, with `criteria`, into
/// the benchmark [`IMPLEMENTS`], made if missing.
pub async fn capture(
    world: &World,
    run: &str,
    named: &str,
    criteria: &str,
) -> Result<KeptTask, CaptureError> {
    let implements = made(world, IMPLEMENTS).await;
    capture_into(
        world,
        run,
        named,
        criteria,
        BenchmarkChoice::Existing(implements),
    )
    .await
}

/// Captures run `run` of `world` as the task `named`, with `criteria`, into
/// the benchmark `benchmark`.
pub async fn capture_into(
    world: &World,
    run: &str,
    named: &str,
    criteria: &str,
    benchmark: BenchmarkChoice,
) -> Result<KeptTask, CaptureError> {
    let named: Name = named.parse().unwrap();
    let criteria = Criteria::parse(criteria).unwrap();
    let run = id(run);
    let benchmarks = benchmarks(world).await;
    let capturable =
        task::capturable(&runs(world).await, &benchmarks, &run, &named, &benchmark).await?;
    task::capture(&*world.store, capturable, &criteria, &benchmarks).await
}

/// Takes the properties out of every input recorded in the repository's work
/// directory `work`, as Linewright recorded inputs before it recorded the
/// properties.
pub async fn record_as_before_the_properties(work: &Path) {
    let pool = back_door(work).await;
    sqlx::query("UPDATE run_inputs SET input = json_remove(input, '$.properties')")
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("taking out the properties: {e}"));
    pool.close().await;
}

/// An implement run on the ticket "Gadgets", and its ID.
pub async fn implemented(world: &mut World) -> String {
    implement_gadgets(world);
    let ticket = open_designed_gadgets(world);
    world.poll().await;
    format!("{}/implement-1", ticket.0)
}

pub fn benchmark(name: &str) -> Name {
    name.parse().unwrap()
}
