use std::panic::Location;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::files::copy_dir;
use super::records::runs_in;
use super::world::World;
use crate::benchmarks::Benchmarks;
use crate::domain::Number;
use crate::name::Name;
use crate::reconcile::WorkKind;
use crate::task::{self, BenchmarkChoice, Criteria, Task};

/// What each triage answers: a bug, ready for an agent.
pub fn triaged() -> String {
    json!({
        "outcome": "ready-for-agent",
        "category": "bug",
        "comment": "## Agent Brief\n\n**Summary:** It crashes."
    })
    .to_string()
}

/// A triage run of `world` on an issue, captured as a task: the issue and
/// the task's directory.
#[track_caller]
pub fn triage_task(world: &mut World) -> impl Future<Output = (Number, PathBuf)> + '_ {
    let caller = Location::caller();
    async move {
        world.harness.on(WorkKind::Triage, |_| Ok(triaged()));
        let issue =
            world
                .store
                .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
        world.poll().await;
        let run = format!("{}/triage-1", issue.0);
        let task = task_at(caller, world, &run).await;
        (issue, task)
    }
}

/// The run `run` of `world`, captured as a task, failing at `caller`: the
/// task's directory.
pub async fn task_at(caller: &Location<'_>, world: &World, run: &str) -> PathBuf {
    let work = world.dir.path().join("work");
    let id = run
        .parse()
        .unwrap_or_else(|e| panic!("the run {run} at {caller}: {e}"));
    let runs = runs_in(&work).await;
    let benchmarks = Benchmarks::new(runs.database().clone());
    let name = Name::parse(run).unwrap_or_else(|e| panic!("naming {run} at {caller}: {e}"));
    let benchmark = BenchmarkChoice::New(name.clone());
    let capturable = task::capturable(&runs, &benchmarks, &id, &name, &benchmark)
        .await
        .unwrap_or_else(|e| panic!("capturing {run} at {caller}: {e}"));
    let criteria = Criteria::parse("It triages the issue.\n")
        .unwrap_or_else(|| panic!("blank criteria at {caller}"));
    task::capture(&*world.store, capturable, &criteria, &benchmarks)
        .await
        .unwrap_or_else(|e| panic!("capturing {run} at {caller}: {e}"))
        .dir
}

/// A triage task of a new world, kept in `under/case`, and the output of a
/// trial of it whose run ended `outcome`, written in `under/out` as a
/// trial leaves it: the ID of the task's run and the outcome.
#[track_caller]
pub fn triage_output<'a>(
    under: &'a Path,
    outcome: &'a str,
) -> impl Future<Output = (PathBuf, PathBuf)> + 'a {
    let caller = Location::caller();
    async move {
        let mut world = World::new().await;
        let (_, captured) = triage_task(&mut world).await;
        let task = under.join("case");
        copy_dir(&captured, &task);
        let run = Task::open(&task)
            .await
            .unwrap_or_else(|e| panic!("opening the case at {caller}: {e}"))
            .run()
            .to_string();
        let out = under.join("out");
        std::fs::create_dir(&out).unwrap();
        std::fs::write(out.join("task"), format!("{run}\n")).unwrap();
        std::fs::write(out.join("outcome"), format!("{outcome}\n")).unwrap();
        (task, out)
    }
}
