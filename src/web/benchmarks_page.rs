//! The benchmarks page (`docs/specs/benchmarks.md`, Benchmarks): a row per
//! benchmark, narrowed by its filters, and the New benchmark dialog that makes
//! one.

use std::sync::Arc;

use askama::Template;
use askama_web::WebTemplate;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tracing::warn;

use super::buttons::ScreenButton;
use super::caller::{Asking, Reader};
use super::html::{self, Html};
use super::making::{self, Answer, Field, FieldRefusals, Form, MadeShown, Making, Refusal};
use super::pages::{PageHref, Pages, PagesShown, Requested};
use super::shell::{self, Area, Frame, Navigation, Place};
use super::table::{Align, Column, View};
use super::write::Write;
use super::{Page, benchmark_path, benchmark_run_path, events, given, render, with_filters};
use crate::access::{Permit, Read};
use crate::actions::{self, Action, benchmark_taken};
use crate::api::Sources;
use crate::benchmarks::{BenchmarkListed, BenchmarkRunId, MakeError};
use crate::domain::Timestamp;
use crate::name::Name;
use crate::task::TaskStage;

/// What the page's address carries: the filters and the page of the table.
#[derive(Deserialize)]
pub(super) struct BenchmarksQuery {
    #[serde(flatten)]
    filters: FilterQuery,
    #[serde(default)]
    page: Requested,
}

/// The filters as the page's address carries them, an empty one leaving the
/// list unnarrowed by it.
#[derive(Deserialize)]
struct FilterQuery {
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
    /// The page of the table the address asks for.
    page: Requested,
}

struct Narrowed {
    name: Option<String>,
    stage: Option<TaskStage>,
}

impl Narrowing {
    fn of(query: &BenchmarksQuery) -> Narrowing {
        let fields = &query.filters;
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
        Narrowing {
            fields,
            narrowed,
            page: query.page,
        }
    }

    /// The fields given and the page `page`, as the query of `path`.
    fn path(&self, path: &str, page: Requested) -> String {
        let FilterQuery { name, stage } = &self.fields;
        with_filters(
            path,
            &[("name", name), ("stage", stage), ("page", &page.field())],
        )
    }

    /// Where the link to page `page` of the table leads.
    fn href(&self, page: Requested) -> PageHref {
        PageHref::at(self.path("/benchmarks", page))
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
    name: Name,
    stage: TaskStage,
    view: View,
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
            view: View {
                href: benchmark_path(listed.benchmark.id),
                name: format!("benchmark {}", listed.benchmark.name),
            },
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
    /// "View", linking to the benchmark's Benchmark.
    View,
}

impl BenchmarksColumn {
    const ALL: [BenchmarksColumn; 4] = [
        BenchmarksColumn::Name,
        BenchmarksColumn::Stage,
        BenchmarksColumn::LastBenchmarkRun,
        BenchmarksColumn::View,
    ];

    fn head(self) -> &'static str {
        match self {
            BenchmarksColumn::Name => "Benchmark",
            BenchmarksColumn::Stage => "Stage",
            BenchmarksColumn::LastBenchmarkRun => "Last benchmark run",
            BenchmarksColumn::View => "",
        }
    }

    /// The class that aligns the column's head and cells as its kind does.
    fn align(self) -> &'static str {
        let column = match self {
            BenchmarksColumn::Name
            | BenchmarksColumn::Stage
            | BenchmarksColumn::LastBenchmarkRun
            | BenchmarksColumn::View => Column::Other,
        };
        Align::of(column).class()
    }
}

/// A page of the benchmarks the filters keep, by name, with the pages under
/// them, or why they are not listed.
enum BenchmarksShown {
    Table {
        rows: Vec<Row>,
        pages: Option<PagesShown>,
    },
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

impl Form for NewBenchmarkFields {
    type Field = NewBenchmarkField;
}

/// A field of the New benchmark dialog.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum NewBenchmarkField {
    Name,
    Stage,
}

impl Field for NewBenchmarkField {
    fn required(&self) -> bool {
        match self {
            NewBenchmarkField::Name | NewBenchmarkField::Stage => true,
        }
    }
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
async fn listed(
    permit: &Permit<Read>,
    sources: &Sources,
    narrowing: &Narrowing,
) -> (StatusCode, BenchmarksShown) {
    let narrowed = match &narrowing.narrowed {
        Ok(narrowed) => narrowed,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                BenchmarksShown::Unlisted(e.clone()),
            );
        }
    };
    match actions::list_benchmarks(permit, sources).await {
        Ok((listed, mut latest)) => {
            let kept: Vec<BenchmarkListed> = listed
                .into_iter()
                .filter(|listed| narrowed.keeps(listed))
                .collect();
            let pages = Pages::of(kept.len(), narrowing.page);
            let rows = pages
                .shows(kept)
                .into_iter()
                .map(|kept| {
                    let last = latest.remove(&kept.benchmark.id);
                    Row::of(kept, last)
                })
                .collect();
            let pages = pages.shown("benchmarks", |page| narrowing.href(page));
            (StatusCode::OK, BenchmarksShown::Table { rows, pages })
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
fn page(narrowing: &Narrowing, navigation: Navigation) -> Page {
    Page::new(
        Place::Navigation(Area::Benchmarks),
        narrowing.path("/events/benchmarks", narrowing.page),
        navigation,
    )
    .buttons(vec![ScreenButton::New {
        dialog: NEW_BENCHMARK,
        action: Action::CreateBenchmark,
    }])
}

/// The id of the New benchmark dialog.
const NEW_BENCHMARK: &str = "new-benchmark";

pub(super) async fn benchmarks_page(
    Reader { permit, sources }: Reader,
    Query(query): Query<BenchmarksQuery>,
    navigation: Navigation,
) -> Response {
    let narrowing = Narrowing::of(&query);
    let (status, benchmarks) = listed(&permit, &sources, &narrowing).await;
    let page = BenchmarksPage {
        page: page(&narrowing, navigation),
        frame: Frame::of(&permit, &sources).await,
        filters: Filters::of(&narrowing.fields),
        benchmarks,
        new_benchmark: Making::blank(),
    };
    (status, page).into_response()
}

/// The benchmarks page's regions: the shell's, and the benchmarks the filters
/// keep.
pub(super) async fn benchmarks_events(
    Reader { permit, sources }: Reader,
    watch: super::caller::Watch,
    Query(query): Query<BenchmarksQuery>,
    navigation: Navigation,
) -> Response {
    let narrowing = Arc::new(Narrowing::of(&query));
    let regions = move || {
        let (permit, sources) = (permit.clone(), sources.clone());
        let narrowing = narrowing.clone();
        async move {
            let frame = Frame::of(&permit, &sources).await;
            let mut regions = shell::regions(&frame, &page(&narrowing, navigation));
            let (_, benchmarks) = listed(&permit, &sources, &narrowing).await;
            regions.push((
                "benchmarks",
                render(&BenchmarksRegion {
                    benchmarks: &benchmarks,
                }),
            ));
            regions
        }
    };
    events(regions, watch)
}

/// What the New benchmark dialog's form sends, a field it lacks being empty.
#[derive(Deserialize)]
struct MakeForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    stage: String,
}

impl MakeForm {
    /// The name and stage it sends, or the refusal of each field that does
    /// not hold one (`docs/specs/benchmarks.md`, New benchmark, Refusals;
    /// `docs/specs/design_system.md`, Making, Required).
    fn checked(&self) -> Result<(Name, TaskStage), FieldRefusals<NewBenchmarkField>> {
        let name = making::parse_name(&self.name);
        let stage = self.stage.parse::<TaskStage>();
        match (name, stage) {
            (Ok(name), Ok(stage)) => Ok((name, stage)),
            (Err(name), Ok(_)) => Err(FieldRefusals::of(NewBenchmarkField::Name, name)),
            (Ok(_), Err(stage)) => Err(FieldRefusals::of(NewBenchmarkField::Stage, stage)),
            (Err(name), Err(stage)) => Err(FieldRefusals::of(NewBenchmarkField::Name, name)
                .and(NewBenchmarkField::Stage, stage)),
        }
    }
}

/// Makes the benchmark the form in `body` names, and answers with the dialog's
/// form: as it starts once made, or as sent with why it was refused.
/// Refused, writing nothing, unless the browser shows the request came from
/// the web UI's own pages.
pub(super) async fn make_benchmark(
    State(sources): State<Sources>,
    asking: Asking,
    uri: Uri,
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
    let refuse = |status, refusal| Answer::Refused {
        status,
        form: NewBenchmarkForm {
            new_benchmark: Making::refused(fields, refusal),
        },
    };
    let (permit, form) = match WRITE.permit_with(Action::CreateBenchmark, &asking, &uri, sent) {
        Ok(permitted) => permitted,
        Err(refused) => {
            let (status, why) = refused.into_parts();
            return refuse(status, Refusal::Whole(why));
        }
    };
    let (name, stage) = match form.checked() {
        Ok(checked) => checked,
        Err(refusals) => {
            return refuse(StatusCode::UNPROCESSABLE_ENTITY, Refusal::Fields(refusals));
        }
    };
    match actions::create_benchmark(&permit, &sources, &name, stage).await {
        Ok(_) => Answer::Made {
            blank: NewBenchmarkForm {
                new_benchmark: Making::blank(),
            },
            shown: MadeShown::OnScreen,
        },
        Err(e) => {
            let (status, refusal) = match &e {
                MakeError::Taken(taken) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Refusal::Fields(FieldRefusals::of(
                        NewBenchmarkField::Name,
                        benchmark_taken(taken),
                    )),
                ),
                MakeError::Failed(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Refusal::Whole(format!("{e:#}")),
                ),
            };
            refuse(status, refusal)
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
    fn kept(name: &str, stage: &str) -> Vec<&'static str> {
        let narrowing = Narrowing::of(&BenchmarksQuery {
            filters: FilterQuery {
                name: name.into(),
                stage: stage.into(),
            },
            page: Requested::default(),
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
