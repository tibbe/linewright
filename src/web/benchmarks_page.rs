//! The benchmarks page (`docs/specs/benchmarks.md`, Benchmarks): a row per
//! benchmark, narrowed by its filters, and the New benchmark dialog that makes
//! one.

use std::sync::Arc;

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::buttons::ScreenButton;
use super::html::{self, Html};
use super::making::{Answer, Making, NOT_A_NAME, benchmark_not_made};
use super::shell::{self, Area, Frame, Place};
use super::table::{Align, Column};
use super::write::Write;
use super::{
    Page, benchmark_path, benchmark_run_path, benchmark_runs, benchmarks, events, given, render,
    with_filters,
};
use crate::api::Sources;
use crate::benchmarks::{BenchmarkListed, BenchmarkName, BenchmarkRunId, MakeError};
use crate::domain::Timestamp;
use crate::task::TaskStage;

/// The filters as the page's address carries them, an empty one leaving the
/// list unnarrowed by it.
#[derive(Deserialize)]
pub(super) struct FilterQuery {
    #[serde(default)]
    name: String,
    #[serde(default)]
    stage: String,
}

/// The narrowing the filters ask for.
struct Narrowing {
    /// Trimmed, to fill the filters in again.
    fields: FilterQuery,
    narrowed: Result<Narrowed, String>,
}

struct Narrowed {
    name: Option<String>,
    stage: Option<TaskStage>,
}

impl Narrowing {
    fn of(fields: &FilterQuery) -> Narrowing {
        let fields = FilterQuery {
            name: fields.name.trim().to_string(),
            stage: fields.stage.trim().to_string(),
        };
        let narrowed = given(&fields.stage)
            .map(str::parse::<TaskStage>)
            .transpose()
            .map(|stage| Narrowed {
                name: given(&fields.name).map(str::to_string),
                stage,
            });
        Narrowing { fields, narrowed }
    }

    /// The fields given, as the query of `path`.
    fn path(&self, path: &str) -> String {
        let FilterQuery { name, stage } = &self.fields;
        with_filters(path, &[("name", name), ("stage", stage)])
    }
}

impl Narrowed {
    /// Whether it keeps `listed` (`docs/specs/benchmarks.md`, Benchmarks,
    /// Filters): its name holds the text, whatever its letter case, and it
    /// is of the stage.
    fn keeps(&self, listed: &BenchmarkListed) -> bool {
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

/// The filters, filled in as the reader left them.
struct Filters {
    name: String,
    stages: Vec<(&'static str, bool)>,
}

impl Filters {
    fn of(fields: &FilterQuery) -> Filters {
        Filters {
            name: fields.name.clone(),
            stages: TaskStage::ALL
                .into_iter()
                .map(|stage| (stage.name(), fields.stage == stage.name()))
                .collect(),
        }
    }
}

/// A benchmark's row.
struct Row {
    name: BenchmarkName,
    stage: TaskStage,
    href: String,
    /// Its latest benchmark run, if it has one.
    last: Option<LastBenchmarkRun>,
}

/// A benchmark's latest benchmark run.
struct LastBenchmarkRun {
    href: String,
    /// When it was launched.
    launched: Html,
}

impl Row {
    fn of(listed: BenchmarkListed, last: Option<(BenchmarkRunId, Timestamp)>) -> Row {
        Row {
            href: benchmark_path(&listed.benchmark.name),
            name: listed.benchmark.name,
            stage: listed.benchmark.stage,
            last: last.map(|(id, at)| LastBenchmarkRun {
                href: benchmark_run_path(id),
                launched: html::time(at),
            }),
        }
    }
}

/// A column of the table of benchmarks, the first naming each row.
#[derive(Clone, Copy)]
enum BenchmarksColumn {
    Name,
    Stage,
    LastBenchmarkRun,
}

impl BenchmarksColumn {
    const ALL: [BenchmarksColumn; 3] = [
        BenchmarksColumn::Name,
        BenchmarksColumn::Stage,
        BenchmarksColumn::LastBenchmarkRun,
    ];

    /// Empty for the column that names each row, which has no head.
    fn head(self) -> &'static str {
        match self {
            BenchmarksColumn::Name => "",
            BenchmarksColumn::Stage => "Stage",
            BenchmarksColumn::LastBenchmarkRun => "Last benchmark run",
        }
    }

    /// The class that aligns the column's head and cells as its kind does.
    fn align(self) -> &'static str {
        let column = match self {
            BenchmarksColumn::Name
            | BenchmarksColumn::Stage
            | BenchmarksColumn::LastBenchmarkRun => Column::Other,
        };
        Align::of(column).class()
    }
}

/// The benchmarks the filters keep, by name, or why they are not listed.
enum BenchmarksShown {
    Table(Vec<Row>),
    Unlisted(String),
}

/// The New benchmark dialog's fields (`docs/specs/benchmarks.md`, New
/// benchmark).
pub(super) struct NewBenchmarkFields {
    name: String,
    stage: TaskStage,
}

impl NewBenchmarkFields {
    /// The stage that "Stage" starts on: the first.
    const STARTING_STAGE: TaskStage = TaskStage::ALL[0];
}

impl Default for NewBenchmarkFields {
    fn default() -> NewBenchmarkFields {
        NewBenchmarkFields {
            name: String::new(),
            stage: NewBenchmarkFields::STARTING_STAGE,
        }
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "benchmarks.html")]
struct BenchmarksPage {
    page: Page,
    frame: Frame,
    filters: Filters,
    benchmarks: BenchmarksShown,
    new_benchmark: Making<NewBenchmarkFields>,
}

#[derive(Template)]
#[template(path = "benchmarks_table.html")]
struct BenchmarksRegion<'a> {
    benchmarks: &'a BenchmarksShown,
}

/// What the New benchmark dialog's form holds.
#[derive(Template)]
#[template(path = "new_benchmark.html")]
pub(super) struct NewBenchmarkForm {
    new_benchmark: Making<NewBenchmarkFields>,
}

/// The benchmarks the narrowing keeps, by name, or why they are not listed, and
/// the status the page answers with.
async fn listed(sources: &Sources, narrowing: &Narrowing) -> (StatusCode, BenchmarksShown) {
    let narrowed = match &narrowing.narrowed {
        Ok(narrowed) => narrowed,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                BenchmarksShown::Unlisted(e.clone()),
            );
        }
    };
    let read = async {
        let listed = benchmarks(sources).list().await?;
        let latest = benchmark_runs(sources).latest().await?;
        anyhow::Ok((listed, latest))
    };
    match read.await {
        Ok((listed, mut latest)) => {
            let rows = listed
                .into_iter()
                .filter(|listed| narrowed.keeps(listed))
                .map(|listed| {
                    let last = latest.remove(&listed.benchmark.name);
                    Row::of(listed, last)
                })
                .collect();
            (StatusCode::OK, BenchmarksShown::Table(rows))
        }
        Err(e) => {
            let error = format!("{e:#}");
            warn!(error, "reading the benchmarks failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                BenchmarksShown::Unlisted(error),
            )
        }
    }
}

/// The benchmarks page, its stream's narrowed as `narrowing` says, with
/// "New" among the screen's buttons (`docs/specs/design_system.md`, Making).
fn page(narrowing: &Narrowing) -> Page {
    Page::new(
        Place::Navigation(Area::Benchmarks),
        narrowing.path("/events/benchmarks"),
    )
    .buttons(vec![ScreenButton::New {
        dialog: NEW_BENCHMARK,
    }])
}

/// The id of the New benchmark dialog.
const NEW_BENCHMARK: &str = "new-benchmark";

pub(super) async fn benchmarks_page(
    State(sources): State<Sources>,
    Query(fields): Query<FilterQuery>,
) -> Response {
    let narrowing = Narrowing::of(&fields);
    let (status, benchmarks) = listed(&sources, &narrowing).await;
    let page = BenchmarksPage {
        page: page(&narrowing),
        frame: Frame::of(&sources).await,
        filters: Filters::of(&narrowing.fields),
        benchmarks,
        new_benchmark: Making::blank(),
    };
    (status, page).into_response()
}

/// The benchmarks page's regions: the shell's, and the benchmarks the filters
/// keep.
pub(super) async fn benchmarks_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Query(fields): Query<FilterQuery>,
) -> Response {
    let narrowing = Arc::new(Narrowing::of(&fields));
    let regions = move || {
        let sources = sources.clone();
        let narrowing = narrowing.clone();
        async move {
            let mut regions = shell::regions(&Frame::of(&sources).await, &page(&narrowing));
            let (_, benchmarks) = listed(&sources, &narrowing).await;
            regions.push((
                "benchmarks",
                render(&BenchmarksRegion {
                    benchmarks: &benchmarks,
                }),
            ));
            regions
        }
    };
    events(regions, stopping)
}

/// What the New benchmark dialog's form sends, a field it lacks being empty.
#[derive(Deserialize)]
struct MakeForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    stage: String,
}

/// Makes the benchmark the form in `body` names, and answers with the dialog's
/// form: as it starts once made, or as sent with why it was refused.
/// Refused, writing nothing, unless the browser shows the request came from
/// the web UI's own pages.
pub(super) async fn make_benchmark(
    State(sources): State<Sources>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Answer<NewBenchmarkForm> {
    const WRITE: Write = Write::MakeBenchmark;
    let sent = WRITE.read_form::<MakeForm>(&body);
    let fields = match &sent {
        Ok(form) => NewBenchmarkFields {
            name: form.name.clone(),
            stage: form
                .stage
                .parse()
                .unwrap_or(NewBenchmarkFields::STARTING_STAGE),
        },
        Err(_) => NewBenchmarkFields::default(),
    };
    let refuse = |status, why| Answer::Refused {
        status,
        form: NewBenchmarkForm {
            new_benchmark: Making::refused(fields, why),
        },
    };
    let form = match WRITE.same_origin(&headers, &uri).and(sent) {
        Ok(form) => form,
        Err(refused) => {
            let (status, why) = refused.into_parts();
            return refuse(status, why);
        }
    };
    let Ok(name) = form.name.parse::<BenchmarkName>() else {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, NOT_A_NAME.into());
    };
    let stage = match form.stage.parse::<TaskStage>() {
        Ok(stage) => stage,
        Err(why) => return refuse(StatusCode::UNPROCESSABLE_ENTITY, why),
    };
    match benchmarks(&sources).make(&name, stage).await {
        Ok(benchmark) => {
            info!(benchmark = %benchmark.name, stage = %benchmark.stage, "made a benchmark");
            Answer::Made {
                blank: NewBenchmarkForm {
                    new_benchmark: Making::blank(),
                },
            }
        }
        Err(e) => {
            let status = match &e {
                MakeError::Taken(_) => StatusCode::UNPROCESSABLE_ENTITY,
                MakeError::Failed(e) => {
                    warn!(benchmark = %name, error = format!("{e:#}"), "making a benchmark failed");
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            };
            refuse(status, benchmark_not_made(&e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmarks::Benchmark;

    #[track_caller]
    fn listed(name: &str, stage: TaskStage) -> BenchmarkListed {
        BenchmarkListed {
            benchmark: Benchmark {
                name: name.parse().unwrap(),
                stage,
            },
            tasks: 0,
        }
    }

    /// The names of the benchmarks the filters `name` and `stage` keep, of
    /// `board` (implement), `triage-boards` (triage) and `lanes` (triage).
    #[track_caller]
    fn kept(name: &str, stage: &str) -> Vec<&'static str> {
        let narrowing = Narrowing::of(&FilterQuery {
            name: name.into(),
            stage: stage.into(),
        });
        let narrowed = narrowing.narrowed.unwrap();
        [
            ("board", TaskStage::Implement),
            ("triage-boards", TaskStage::Triage),
            ("lanes", TaskStage::Triage),
        ]
        .into_iter()
        .filter(|(benchmark, of)| narrowed.keeps(&listed(benchmark, *of)))
        .map(|(benchmark, _)| benchmark)
        .collect()
    }

    /// `docs/specs/benchmarks.md`, Benchmarks, Filters.
    #[test_log::test]
    fn the_name_filter_keeps_each_benchmark_whose_name_holds_the_text_whatever_its_case() {
        assert_eq!(kept("boa", ""), ["board", "triage-boards"]);
        assert_eq!(kept("BOA", ""), ["board", "triage-boards"]);
        assert_eq!(kept(" Ane ", ""), ["lanes"]);
        assert_eq!(kept("", ""), ["board", "triage-boards", "lanes"]);
        assert_eq!(kept("nothing", ""), Vec::<&str>::new());
    }

    /// `docs/specs/benchmarks.md`, Benchmarks, Filters.
    #[test_log::test]
    fn the_stage_filter_keeps_the_benchmarks_of_the_stage_and_narrows_with_the_name() {
        assert_eq!(kept("", "triage"), ["triage-boards", "lanes"]);
        assert_eq!(kept("boa", "triage"), ["triage-boards"]);
        assert_eq!(kept("", "design"), Vec::<&str>::new());
    }
}
