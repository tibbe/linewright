//! The actions that show the benchmarks, a benchmark and a task
//! (`docs/specs/benchmarks.md`, Benchmarks, Benchmark and Task), and what
//! they show.

use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use super::benchmark_run::shown_of;
use super::benchmarks::{benchmark_runs, benchmark_shown, benchmarks, task_shown};
use crate::access::{Permit, Read};
use crate::api::Sources;
use crate::benchmarks::results::{self, Total};
use crate::benchmarks::{
    Benchmark, BenchmarkId, BenchmarkListed, BenchmarkRunId, BenchmarkRunRecord, BenchmarkRunState,
    CriteriaAnswer, Shown, TaskAnswer, TaskId, TaskShown,
};
use crate::domain::{Number, Timestamp};
use crate::task::TaskStage;

/// The benchmarks as their screen shows them, narrowed by its filters, in
/// the order of their names, with the names `benchmarks --json` gives the
/// same facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct BenchmarksScreen {
    pub benchmarks: Vec<BenchmarkRow>,
}

/// A row of the benchmarks (`docs/specs/benchmarks.md`, Benchmarks, The
/// table).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct BenchmarkRow {
    /// Its ID, name and stage, and how many tasks it has.
    #[serde(flatten)]
    pub listed: BenchmarkListed,
    /// Its last benchmark run, where it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_benchmark_run: Option<LastBenchmarkRun>,
}

/// A benchmark's last benchmark run, by the names `benchmark show --json`
/// gives its facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct LastBenchmarkRun {
    pub id: BenchmarkRunId,
    pub launched_at: Timestamp,
}

/// The filters of the benchmarks (`docs/specs/benchmarks.md`, Benchmarks,
/// Filters): `None` leaves the list unnarrowed by it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BenchmarksFilters {
    /// Text that the name holds, whatever its letter case: trimmed, and
    /// never empty.
    name: Option<String>,
    stage: Option<TaskStage>,
}

impl BenchmarksFilters {
    /// The filters for the name `name`, as typed, and the stage `stage`: a
    /// name of white space alone leaves the list unnarrowed by it.
    pub fn new(name: Option<&str>, stage: Option<TaskStage>) -> BenchmarksFilters {
        BenchmarksFilters {
            name: name
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string),
            stage,
        }
    }

    /// Whether they keep `listed`: its name holds the text, whatever its
    /// letter case, and it is of the stage.
    pub fn keeps(&self, listed: &BenchmarkListed) -> bool {
        self.name.as_ref().is_none_or(|name| {
            listed
                .benchmark
                .name
                .as_str()
                .to_lowercase()
                .contains(&name.to_lowercase())
        }) && self
            .stage
            .is_none_or(|stage| listed.benchmark.stage == stage)
    }
}

/// The benchmarks that `filters` keep, as their screen shows them now.
pub async fn list_benchmarks(
    _: &Permit<Read>,
    sources: &Sources,
    filters: &BenchmarksFilters,
) -> Result<BenchmarksScreen> {
    let listed = benchmarks(sources).list().await?;
    let mut latest = benchmark_runs(sources).latest().await?;
    Ok(BenchmarksScreen {
        benchmarks: listed
            .into_iter()
            .filter(|listed| filters.keeps(listed))
            .map(|listed| BenchmarkRow {
                last_benchmark_run: latest
                    .remove(&listed.benchmark.id)
                    .map(|(id, launched_at)| LastBenchmarkRun { id, launched_at }),
                listed,
            })
            .collect(),
    })
}

/// A benchmark as its screen shows it, with the names `benchmark show
/// --json` gives the same facts.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct BenchmarkScreen {
    #[serde(flatten)]
    pub benchmark: Benchmark,
    /// Its benchmark runs, latest first.
    pub benchmark_runs: Vec<BenchmarkRunRow>,
    /// Its tasks, in its order.
    pub tasks: Vec<TaskAnswer>,
}

/// A row of a benchmark's benchmark runs (`docs/specs/benchmarks.md`,
/// Benchmark, Runs).
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct BenchmarkRunRow {
    pub id: BenchmarkRunId,
    pub launched_at: Timestamp,
    /// Its state, with what it waits for while it waits.
    #[serde(flatten)]
    pub shown: Shown,
    /// Whether it is kept as open, neither ended nor cancelled, whatever
    /// `serve` shows of it.
    #[serde(skip)]
    pub open: bool,
    /// How many tasks it makes trials of.
    pub tasks: usize,
    /// How many configurations it has.
    pub configurations: usize,
    /// From its launch to its end, or up to now while it is open; `None`
    /// where its end was not kept.
    pub duration_seconds: Option<u64>,
    /// The cost of its trials and its scorers, in US dollars, and how many
    /// of its trials that have ended have none.
    pub cost: Total,
}

impl BenchmarkRunRow {
    /// Of `record`, shown as `shown`, as of `now`.
    fn of(record: &BenchmarkRunRecord, shown: Shown, now: Timestamp) -> BenchmarkRunRow {
        BenchmarkRunRow {
            id: record.id,
            launched_at: record.launched_at,
            shown,
            open: record.state == BenchmarkRunState::Open,
            tasks: record.tasks.len(),
            configurations: record.configurations.len(),
            duration_seconds: record.duration(now).map(|duration| duration.as_secs()),
            cost: results::results(record).cost,
        }
    }
}

/// Benchmark `id` as its screen shows it now, `None` where there is no such
/// benchmark.
pub async fn get_benchmark(
    permit: &Permit<Read>,
    sources: &Sources,
    id: BenchmarkId,
) -> Result<Option<BenchmarkScreen>> {
    let Some(shown) = benchmark_shown(permit, sources, id).await? else {
        return Ok(None);
    };
    let now = sources.clock.now();
    Ok(Some(BenchmarkScreen {
        benchmark_runs: shown
            .benchmark_runs
            .iter()
            .map(|record| BenchmarkRunRow::of(record, shown_of(sources, record), now))
            .collect(),
        tasks: shown.tasks.iter().map(TaskAnswer::from).collect(),
        benchmark: shown.benchmark,
    }))
}

/// A task as its screen shows it, with the names `benchmark show --json`
/// gives the same facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct TaskScreen {
    /// Its ID, name, the run it was captured from, and its criteria, or why
    /// they do not read.
    #[serde(flatten)]
    pub task: TaskAnswer,
    pub benchmark: Benchmark,
    /// Its item and start, or why they do not read.
    #[serde(flatten)]
    pub facts: TaskFactsScreen,
}

/// A task's item and start (`docs/specs/benchmarks.md`, Task, Facts), or
/// why they do not read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum TaskFactsScreen {
    Read {
        item: Number,
        /// The item's title as at the start.
        title: String,
        /// When Linewright decided to start the run.
        started_at: Timestamp,
    },
    Unreadable {
        /// Why the item and the start do not read.
        facts_unreadable: String,
    },
}

impl TaskScreen {
    /// Of `task`.
    fn of(task: TaskShown) -> TaskScreen {
        TaskScreen {
            task: TaskAnswer {
                id: task.id,
                name: task.name,
                run: task.run,
                criteria: CriteriaAnswer::of(&task.criteria),
            },
            benchmark: task.benchmark,
            facts: match task.facts {
                Ok(facts) => TaskFactsScreen::Read {
                    item: facts.item,
                    title: facts.title,
                    started_at: facts.start,
                },
                Err(refused) => TaskFactsScreen::Unreadable {
                    facts_unreadable: refused.to_string(),
                },
            },
        }
    }
}

/// Task `id` as its screen shows it now, `None` where there is no such
/// task.
pub async fn get_task(
    permit: &Permit<Read>,
    sources: &Sources,
    id: TaskId,
) -> Result<Option<TaskScreen>> {
    Ok(task_shown(permit, sources, id).await?.map(TaskScreen::of))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn listed(name: &str, stage: TaskStage) -> BenchmarkListed {
        BenchmarkListed {
            benchmark: Benchmark {
                id: "1".parse().unwrap(),
                name: name.parse().unwrap(),
                stage,
            },
            tasks: 0,
        }
    }

    /// The names of the benchmarks the filters `name` and `stage` keep, of
    /// `board` (implement), `triage-boards` (triage) and `lanes` (triage).
    #[track_caller]
    fn kept(name: Option<&str>, stage: Option<TaskStage>) -> Vec<&'static str> {
        let filters = BenchmarksFilters::new(name, stage);
        [
            ("board", TaskStage::Implement),
            ("triage-boards", TaskStage::Triage),
            ("lanes", TaskStage::Triage),
        ]
        .into_iter()
        .filter(|(benchmark, of)| filters.keeps(&listed(benchmark, *of)))
        .map(|(benchmark, _)| benchmark)
        .collect()
    }

    /// `docs/specs/benchmarks.md`, Benchmarks, Filters.
    #[test_log::test]
    fn the_name_filter_keeps_each_benchmark_whose_name_holds_the_text_whatever_its_case() {
        assert_eq!(kept(Some("boa"), None), ["board", "triage-boards"]);
        assert_eq!(kept(Some("BOA"), None), ["board", "triage-boards"]);
        assert_eq!(kept(Some(" Ane "), None), ["lanes"]);
        assert_eq!(kept(None, None), ["board", "triage-boards", "lanes"]);
        assert_eq!(kept(Some("  "), None), ["board", "triage-boards", "lanes"]);
        assert_eq!(kept(Some("nothing"), None), Vec::<&str>::new());
    }

    /// `docs/specs/benchmarks.md`, Benchmarks, Filters.
    #[test_log::test]
    fn the_stage_filter_keeps_the_benchmarks_of_the_stage_and_narrows_with_the_name() {
        let triage = Some(TaskStage::Triage);
        assert_eq!(kept(None, triage), ["triage-boards", "lanes"]);
        assert_eq!(kept(Some("boa"), triage), ["triage-boards"]);
        assert_eq!(kept(None, Some(TaskStage::Design)), Vec::<&str>::new());
    }
}
