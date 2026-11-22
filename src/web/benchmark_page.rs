//! The benchmark page (`docs/specs/benchmarks.md`, Benchmark and New task):
//! benchmark ID's heading, then its benchmark runs and its tasks, each in a
//! tab of its own at its own address, the New task dialog that captures a run
//! of the benchmark's stage into it, and the New benchmark run dialog that
//! launches a benchmark run on it.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::benchmark_run_page::{shown_of, state_status};
use super::buttons::Verb;
use super::capture::{CapturingField, NotPolled, Task, TaskField, TaskFields, capture_run, polled};
use super::confirmation::{Confirmation, TurnDown};
use super::html::{self, Html};
use super::launch::{BriefTexts, NewBenchmarkRunForm, StageSettings};
use super::making::{
    self, Answer, Control, Field, FieldRefusals, FieldShown, Form, MadeShown, Making, REQUIRED,
    both,
};
use super::pages::{PageHref, Pages, PagesShown, Requested};
use super::shell::{self, Area, Frame, Heading, Missing, Navigation, Place, Thing};
use super::table::{Align, Column, Fold, View};
use super::write::{Refused, Write, alert};
use super::{
    ErrorPage, Page, benchmark_path, benchmark_run_path, benchmark_tab_path, benchmarks, events,
    no_such_page, nothing_there, render, run_path, with_filters,
};
use crate::api::Sources;
use crate::benchmarks::results;
use crate::benchmarks::{
    Benchmark, BenchmarkId, BenchmarkName, BenchmarkRunRecord, BenchmarkRunState, BenchmarkShown,
    RemoveError, TaskEntry, TaskId, dollars, without,
};
use crate::domain::Timestamp;
use crate::phrase::Status;
use crate::runs::{Recorded, RunId};
use crate::task::{self, BenchmarkChoice, Refusal, TaskName, TaskStage};

/// A task's row.
struct TaskRow {
    id: TaskId,
    name: TaskName,
    run: RunId,
    run_href: String,
    criteria: CriteriaShown,
    remove: Confirmation,
}

/// A task's criteria, as its row shows them.
enum CriteriaShown {
    Read {
        /// Their first line that holds more than white space.
        first_line: String,
        text: String,
    },
    /// Why they do not read.
    Unreadable(String),
}

impl TaskRow {
    /// Its row's id, keyed by the task's ID.
    fn row_id(&self) -> String {
        format!("task-{}", self.id)
    }

    /// The id of the row of its whole criteria, which its row opens to.
    fn criteria_id(&self) -> String {
        format!("{}_criteria", self.row_id())
    }

    /// What its row opens to: its whole criteria, where they read.
    fn fold(&self) -> Fold {
        let rows = match self.criteria {
            CriteriaShown::Read { .. } => vec![self.criteria_id()],
            CriteriaShown::Unreadable(_) => vec![],
        };
        Fold::of(rows, format!("Show the criteria of {}", self.name))
    }

    /// The row of `task`, of the benchmark at `path`, removed after a
    /// confirmation (`docs/specs/benchmarks.md`, Remove task).
    fn of(task: TaskEntry, path: &str) -> TaskRow {
        TaskRow {
            remove: Confirmation {
                id: format!("remove-{}", task.id),
                verb: Verb::Remove,
                question: format!("Remove the task {} and its directory?", task.name),
                turn_down: TurnDown::Cancel,
                action: format!("{path}/remove-task"),
                fields: vec![("task", task.id.to_string())],
                said: "benchmark-said",
            },
            id: task.id,
            run_href: run_path(&task.run),
            name: task.name,
            run: task.run,
            criteria: match task.criteria {
                Ok(criteria) => CriteriaShown::Read {
                    first_line: criteria
                        .as_str()
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or_default()
                        .to_string(),
                    text: criteria.as_str().to_string(),
                },
                Err(refused) => CriteriaShown::Unreadable(refused.why),
            },
        }
    }
}

/// A benchmark run's row.
struct BenchmarkRunRow {
    view: View,
    launched: Html,
    status: Status,
    state: &'static str,
    /// How many tasks it runs.
    tasks: usize,
    configurations: usize,
    /// Kept up by the page's script while it is open; `None` where its end
    /// was not kept.
    duration: Option<Html>,
    /// Of its trials and its scorers, and how many of its trials have none.
    cost: String,
}

impl BenchmarkRunRow {
    fn of(sources: &Sources, record: &BenchmarkRunRecord, now: Timestamp) -> BenchmarkRunRow {
        let (status, state) = state_status(&shown_of(sources, record));
        let running = (record.state == BenchmarkRunState::Open).then_some(record.launched_at);
        let cost = results::results(record).cost;
        let mut cost_text = dollars(cost.total);
        without(&mut cost_text, cost.none, "a cost");
        BenchmarkRunRow {
            view: View {
                href: benchmark_run_path(record.id),
                name: format!("benchmark run {}", record.id),
            },
            launched: html::time(record.launched_at),
            status,
            state,
            tasks: record.tasks.len(),
            configurations: record.configurations.len(),
            duration: record
                .duration(now)
                .map(|duration| html::duration(None, duration.as_secs(), running)),
            cost: cost_text,
        }
    }
}

/// A column of the table of benchmark runs, the first naming each row.
#[derive(Clone, Copy)]
enum BenchmarkRunsColumn {
    Launched,
    State,
    Tasks,
    Configurations,
    Duration,
    Cost,
    /// "View", linking to the benchmark run's Benchmark run.
    View,
}

impl BenchmarkRunsColumn {
    const ALL: [BenchmarkRunsColumn; 7] = [
        BenchmarkRunsColumn::Launched,
        BenchmarkRunsColumn::State,
        BenchmarkRunsColumn::Tasks,
        BenchmarkRunsColumn::Configurations,
        BenchmarkRunsColumn::Duration,
        BenchmarkRunsColumn::Cost,
        BenchmarkRunsColumn::View,
    ];

    fn head(self) -> &'static str {
        match self {
            BenchmarkRunsColumn::Launched => "Launched",
            BenchmarkRunsColumn::State => "State",
            BenchmarkRunsColumn::Tasks => "Tasks",
            BenchmarkRunsColumn::Configurations => "Configurations",
            BenchmarkRunsColumn::Duration => "Duration",
            BenchmarkRunsColumn::Cost => "Cost",
            BenchmarkRunsColumn::View => "",
        }
    }

    /// The class that aligns the column's head and cells as its kind does.
    fn align(self) -> &'static str {
        let column = match self {
            BenchmarkRunsColumn::Duration => Column::Duration,
            BenchmarkRunsColumn::Cost => Column::Money,
            BenchmarkRunsColumn::Launched
            | BenchmarkRunsColumn::State
            | BenchmarkRunsColumn::Tasks
            | BenchmarkRunsColumn::Configurations
            | BenchmarkRunsColumn::View => Column::Other,
        };
        Align::of(column).class()
    }
}

/// A column of the table of tasks, the first naming each row.
#[derive(Clone, Copy)]
enum TasksColumn {
    Name,
    CapturedFrom,
    Remove,
}

impl TasksColumn {
    const ALL: [TasksColumn; 3] = [
        TasksColumn::Name,
        TasksColumn::CapturedFrom,
        TasksColumn::Remove,
    ];

    /// Empty for "Remove"'s, which has no head.
    fn head(self) -> &'static str {
        match self {
            TasksColumn::Name => "Task",
            TasksColumn::Remove => "",
            TasksColumn::CapturedFrom => "Captured from",
        }
    }

    /// The class that aligns the column's head and cells as its kind does.
    fn align(self) -> &'static str {
        let column = match self {
            TasksColumn::Name | TasksColumn::CapturedFrom | TasksColumn::Remove => Column::Other,
        };
        Align::of(column).class()
    }
}

/// A benchmark as its page shows it: a page of each of its tables.
struct Shown {
    benchmark: Benchmark,
    /// How many tasks it has, on every page.
    task_count: usize,
    tasks: Vec<TaskRow>,
    tasks_nav: Option<PagesShown>,
    /// Latest first.
    benchmark_runs: Vec<BenchmarkRunRow>,
    benchmark_runs_nav: Option<PagesShown>,
}

/// A tab of the benchmark page (`docs/specs/benchmarks.md`, Benchmark, Tabs),
/// at its own address (`docs/specs/web/design_system.md`, A tab).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BenchmarkTab {
    /// The benchmark runs.
    Runs,
    Tasks,
}

impl BenchmarkTab {
    const ALL: [BenchmarkTab; 2] = [BenchmarkTab::Runs, BenchmarkTab::Tasks];

    /// The tab the page opens on: `asked`, the tab its address names, or
    /// without one, Runs, or Tasks while the benchmark has no benchmark runs
    /// (`docs/specs/benchmarks.md`, Benchmark, Tabs).
    fn opened(asked: Option<BenchmarkTab>, has_benchmark_runs: bool) -> BenchmarkTab {
        asked.unwrap_or(if has_benchmark_runs {
            BenchmarkTab::Runs
        } else {
            BenchmarkTab::Tasks
        })
    }

    /// Its segment of its address, after the page's.
    pub(super) fn segment(self) -> &'static str {
        match self {
            BenchmarkTab::Runs => "runs",
            BenchmarkTab::Tasks => "tasks",
        }
    }

    /// The id of its panel.
    fn panel(self) -> &'static str {
        match self {
            BenchmarkTab::Runs => "runs-panel",
            BenchmarkTab::Tasks => "tasks-panel",
        }
    }

    /// The id of its link.
    fn link_id(self) -> String {
        format!("{}-tab", self.panel())
    }

    fn label(self) -> &'static str {
        match self {
            BenchmarkTab::Runs => "Runs",
            BenchmarkTab::Tasks => "Tasks",
        }
    }
}

impl std::str::FromStr for BenchmarkTab {
    type Err = ();

    /// The tab whose segment is `segment`.
    fn from_str(segment: &str) -> Result<BenchmarkTab, ()> {
        BenchmarkTab::ALL
            .into_iter()
            .find(|tab| tab.segment() == segment)
            .ok_or(())
    }
}

/// A tab's link, which opens its panel.
struct TabLink {
    tab: BenchmarkTab,
    /// Its address, with the page's pages.
    href: String,
}

/// The page of each of the page's tables that its address asks for.
#[derive(Clone, Copy, Deserialize)]
pub(super) struct Paging {
    #[serde(rename = "runs-page", default)]
    benchmark_runs: Requested,
    #[serde(rename = "tasks-page", default)]
    tasks: Requested,
}

impl Paging {
    /// The pages, as the query of `path`.
    fn path(self, path: &str) -> String {
        with_filters(
            path,
            &[
                ("runs-page", &self.benchmark_runs.field()),
                ("tasks-page", &self.tasks.field()),
            ],
        )
    }

    /// Where a link to these pages of benchmark `id`, on its tab `tab`,
    /// leads. The stream is the same on every tab.
    fn href(self, id: BenchmarkId, tab: BenchmarkTab) -> PageHref {
        PageHref {
            page: self.tab_path(id, tab),
            stream: self.stream(id),
        }
    }

    /// The address of tab `tab` of benchmark `id` on these pages.
    fn tab_path(self, id: BenchmarkId, tab: BenchmarkTab) -> String {
        self.path(&benchmark_tab_path(id, tab))
    }

    /// The page's stream on these pages of benchmark `id`.
    fn stream(self, id: BenchmarkId) -> String {
        self.path(&format!("/events{}", benchmark_path(id)))
    }
}

/// A run New task offers to capture.
struct RunChoice {
    id: RunId,
    /// The name the dialog suggests for its task.
    name: TaskName,
}

#[derive(Template, WebTemplate)]
#[template(path = "benchmark.html")]
struct BenchmarkPage {
    page: Page,
    frame: Frame,
    path: String,
    /// The tab that shows.
    tab: BenchmarkTab,
    tab_links: Vec<TabLink>,
    tasks: Vec<TaskRow>,
    tasks_nav: Option<PagesShown>,
    benchmark_runs: Vec<BenchmarkRunRow>,
    benchmark_runs_nav: Option<PagesShown>,
    /// "New" on the benchmark runs, which opens the New benchmark run dialog.
    new_benchmark_run_offer: Html,
    /// The New benchmark run dialog's form.
    new_benchmark_run: Html,
    /// What "Add a configuration" adds to it.
    added_configuration: Html,
    /// What choosing a file of the brief adds to a configuration.
    added_file: Html,
    brief_texts: Html,
    stage_settings: Html,
    /// The New task dialog's form.
    new_task: Html,
}

impl BenchmarkPage {
    /// Whether tab `tab` is open, its panel shown.
    fn opens(&self, tab: BenchmarkTab) -> bool {
        self.tab == tab
    }

    fn launch_path(&self) -> String {
        format!("{}/launch", self.path)
    }
}

#[derive(Template)]
#[template(path = "benchmark_tasks.html")]
struct TasksRegion<'a> {
    tasks: &'a [TaskRow],
    tasks_nav: Option<&'a PagesShown>,
}

#[derive(Template)]
#[template(path = "benchmark_benchmark_runs.html")]
struct BenchmarkRunsRegion<'a> {
    benchmark_runs: &'a [BenchmarkRunRow],
    benchmark_runs_nav: Option<&'a PagesShown>,
}

/// "New" on the benchmark runs of a benchmark with `tasks` tasks, if it has
/// any, and how many it has, for the count of the launch's trials.
#[derive(Template)]
#[template(path = "benchmark_new_benchmark_run.html")]
struct NewBenchmarkRunRegion {
    tasks: usize,
}

/// The New task dialog's fields (`docs/specs/benchmarks.md`, New task).
#[derive(Default)]
pub(super) struct NewTaskFields {
    /// `None` for the first run offered, as for a run that does not read.
    run: Option<RunId>,
    name: String,
    criteria: String,
}

impl Form for NewTaskFields {
    type Field = NewTaskField;
}

/// A field of the New task dialog.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum NewTaskField {
    /// The run captured.
    Run,
    Task(TaskField),
}

impl Field for NewTaskField {
    fn required(&self) -> bool {
        match self {
            NewTaskField::Run => true,
            NewTaskField::Task(field) => field.required(),
        }
    }
}

impl From<TaskField> for NewTaskField {
    fn from(field: TaskField) -> NewTaskField {
        NewTaskField::Task(field)
    }
}

/// New task captures into its own benchmark, which no field chooses.
impl CapturingField for NewTaskField {
    fn naming(_: &BenchmarkChoice) -> Option<NewTaskField> {
        None
    }
}

impl NewTaskFields {
    /// The fields as they start: the latest run of `runs`, and the name
    /// suggested for it.
    fn starting(runs: &[RunChoice]) -> NewTaskFields {
        NewTaskFields {
            name: runs
                .first()
                .map(|run| run.name.to_string())
                .unwrap_or_default(),
            ..NewTaskFields::default()
        }
    }
}

/// The benchmark a making dialog makes a thing in, as its title names it.
pub(super) enum InBenchmark {
    Found(BenchmarkName),
    /// One that is not found, by the ID its path gives.
    Asked(String),
}

impl InBenchmark {
    /// The benchmark of `found`, which was looked for by the ID `asked`.
    pub(super) fn of<T>(found: &Result<BenchmarkShown, T>, asked: String) -> InBenchmark {
        match found {
            Ok(shown) => InBenchmark::Found(shown.benchmark.name.clone()),
            Err(_) => InBenchmark::Asked(asked),
        }
    }
}

impl std::fmt::Display for InBenchmark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InBenchmark::Found(name) => write!(f, "{name}"),
            InBenchmark::Asked(id) => f.write_str(id),
        }
    }
}

/// The region of the New task dialog that holds the button "Add task".
const NEW_TASK_READY: &str = "new-task-ready";

/// What the New task dialog's form holds.
#[derive(Template)]
#[template(path = "new_task.html")]
pub(super) struct NewTaskForm {
    benchmark: InBenchmark,
    making: Making<NewTaskFields>,
    /// The runs of the benchmark's stage that can be captured, latest first.
    runs: Vec<RunChoice>,
}

impl NewTaskForm {
    /// The form as it starts, for benchmark `benchmark` of `stage`.
    async fn blank(sources: &Sources, benchmark: &Benchmark) -> NewTaskForm {
        let runs = runs_of(sources, benchmark.stage).await;
        NewTaskForm {
            benchmark: InBenchmark::Found(benchmark.name.clone()),
            making: Making::of(NewTaskFields::starting(&runs)),
            runs,
        }
    }

    fn title(&self) -> String {
        format!("New task in {}", self.benchmark)
    }

    /// The choice of a run, the region of the page's stream that keeps it,
    /// with the `attributes` of its field.
    fn runs(&self, attributes: &Html) -> Html {
        html::rendered(&NewTaskRuns {
            runs: &self.runs,
            chosen: self.making.fields.run.as_ref(),
            attributes,
        })
    }

    fn ready(&self) -> NewTaskReady {
        NewTaskReady::of(&self.runs)
    }

    /// It refused for `refusal`, answered with `status`.
    fn refused(
        mut self,
        (status, refusal): (StatusCode, making::Refusal<NewTaskField>),
    ) -> Answer<NewTaskForm> {
        self.making.refusal = Some(refusal);
        Answer::Refused { status, form: self }
    }

    /// It refused for `why`, of no field, answered with `status`.
    fn refused_whole(self, (status, why): (StatusCode, String)) -> Answer<NewTaskForm> {
        self.refused((status, making::Refusal::Whole(why)))
    }
}

/// The New task dialog's choice of a run, `chosen` chosen, or else the
/// first; or "No runs". Its field's `attributes` are drawn by the `field`
/// macro.
#[derive(Template)]
#[template(path = "new_task_runs.html")]
struct NewTaskRuns<'a> {
    runs: &'a [RunChoice],
    chosen: Option<&'a RunId>,
    attributes: &'a Html,
}

/// The id of the New task dialog's choice of a run, which is the region of
/// the page's stream that keeps it.
const NEW_TASK_RUN: &str = "new-task-run";

impl NewTaskRuns<'_> {
    fn chosen(&self, id: &RunId) -> bool {
        self.chosen == Some(id)
    }
}

/// The region of the New task dialog that holds "Add task": disabled with
/// no runs to capture.
#[derive(Template)]
#[template(
    source = r#"{%- import "macros.html" as ui -%}{% call ui::new_submit(crate::web::buttons::Verb::AddTask, NEW_TASK_READY, disabled) %}{% endcall %}"#,
    ext = "html"
)]
struct NewTaskReady {
    disabled: bool,
}

impl NewTaskReady {
    fn of(runs: &[RunChoice]) -> NewTaskReady {
        NewTaskReady {
            disabled: runs.is_empty(),
        }
    }
}

/// Why there is no benchmark page: no such benchmark, or one that does not
/// read.
enum Unshown {
    NoSuch,
    Failed(anyhow::Error),
}

/// What is said of a benchmark whose ID is `id` that there is none of.
fn no_such(id: &str) -> String {
    format!("there is no benchmark {id}")
}

/// Benchmark `id` with the pages `paging` asks for of its tasks, in its
/// order, and of its benchmark runs, latest first.
async fn shown(sources: &Sources, id: &str, paging: Paging) -> Result<Shown, Unshown> {
    let parsed: BenchmarkId = id.parse().map_err(|_| Unshown::NoSuch)?;
    match benchmarks(sources).show(parsed).await {
        Ok(Some(BenchmarkShown {
            benchmark,
            tasks,
            benchmark_runs,
        })) => {
            let tasks_page = Pages::of(tasks.len(), paging.tasks);
            let benchmark_runs_page = Pages::of(benchmark_runs.len(), paging.benchmark_runs);
            let path = benchmark_path(benchmark.id);
            let now = sources.clock.now();
            Ok(Shown {
                task_count: tasks.len(),
                tasks: tasks_page
                    .shows(tasks)
                    .into_iter()
                    .map(|task| TaskRow::of(task, &path))
                    .collect(),
                tasks_nav: tasks_page.shown("tasks", |page| {
                    Paging {
                        tasks: page,
                        ..paging
                    }
                    .href(benchmark.id, BenchmarkTab::Tasks)
                }),
                benchmark_runs: benchmark_runs_page
                    .shows(benchmark_runs)
                    .iter()
                    .map(|record| BenchmarkRunRow::of(sources, record, now))
                    .collect(),
                benchmark_runs_nav: benchmark_runs_page.shown("benchmark runs", |page| {
                    Paging {
                        benchmark_runs: page,
                        ..paging
                    }
                    .href(benchmark.id, BenchmarkTab::Runs)
                }),
                benchmark,
            })
        }
        Ok(None) => Err(Unshown::NoSuch),
        Err(e) => Err(Unshown::Failed(e)),
    }
}

/// Of `recorded`, the runs of `stage`, latest first.
fn of_stage(recorded: Vec<Recorded>, stage: TaskStage) -> Vec<RunId> {
    let mut runs: Vec<Recorded> = recorded
        .into_iter()
        .filter(|run| task::capturable_stage(&run.id.of).is_some_and(|(of, _)| of == stage))
        .collect();
    runs.sort_by_key(|run| std::cmp::Reverse(run.started_at));
    runs.into_iter().map(|run| run.id).collect()
}

/// The runs of `stage` that can be captured, latest first. Runs that do not
/// read are logged and left out.
async fn runs_of(sources: &Sources, stage: TaskStage) -> Vec<RunChoice> {
    let recorded = match sources.runs.read().await {
        Ok(recorded) => recorded,
        Err(e) => {
            warn!(error = format!("{e:#}"), "reading the runs failed");
            return vec![];
        }
    };
    let mut runs = vec![];
    for id in of_stage(recorded, stage) {
        match task::capturable_run(&sources.runs, &id).await {
            Ok(_) => runs.push(RunChoice {
                name: TaskName::suggested(&id),
                id,
            }),
            Err(e) if e.refusal() == Refusal::Failed => {
                warn!(run = %id, error = e.why(), "reading whether a run can be captured failed");
            }
            Err(_) => {}
        }
    }
    runs
}

/// The page of `benchmark`: its heading, its name with its stage under it
/// (`docs/specs/benchmarks.md`, Benchmark, Heading), its stream's on the
/// pages `paging` asks for.
fn page(benchmark: &Benchmark, paging: Paging, navigation: Navigation) -> Page {
    let heading = Heading {
        line: Some(html::escape(&benchmark.stage.to_string())),
        ..Heading::named(benchmark.name.as_str())
    };
    Page::new(
        Place::Alone(Area::Benchmarks, Thing::Found(heading)),
        paging.stream(benchmark.id),
        navigation,
    )
}

pub(super) async fn benchmark_page(
    State(sources): State<Sources>,
    Path(id): Path<String>,
    Query(paging): Query<Paging>,
    navigation: Navigation,
) -> Response {
    benchmark_page_on(sources, id, None, paging, navigation).await
}

/// The benchmark page at tab `tab`'s address, or else no page.
pub(super) async fn benchmark_tab_page(
    State(sources): State<Sources>,
    Path((id, tab)): Path<(String, String)>,
    Query(paging): Query<Paging>,
    navigation: Navigation,
) -> Response {
    let Ok(tab) = tab.parse() else {
        return no_such_page().await;
    };
    benchmark_page_on(sources, id, Some(tab), paging, navigation).await
}

/// The benchmark page of benchmark `id` on tab `tab`, or without one, on the
/// tab it opens on: its benchmark runs, or its tasks while it has no benchmark
/// runs.
async fn benchmark_page_on(
    sources: Sources,
    id: String,
    tab: Option<BenchmarkTab>,
    paging: Paging,
    navigation: Navigation,
) -> Response {
    let frame = Frame::of(&sources).await;
    match shown(&sources, &id, paging).await {
        Ok(Shown {
            benchmark,
            task_count,
            tasks,
            tasks_nav,
            benchmark_runs,
            benchmark_runs_nav,
        }) => {
            let path = benchmark_path(benchmark.id);
            let tab = BenchmarkTab::opened(tab, !benchmark_runs.is_empty());
            let tab_links = BenchmarkTab::ALL
                .into_iter()
                .map(|link| TabLink {
                    tab: link,
                    href: paging.tab_path(benchmark.id, link),
                })
                .collect();
            let new_task = NewTaskForm::blank(&sources, &benchmark).await;
            let new_benchmark_run = NewBenchmarkRunForm::blank(&sources, &benchmark, task_count);
            BenchmarkPage {
                page: page(&benchmark, paging, navigation),
                frame,
                new_benchmark_run_offer: html::rendered(&NewBenchmarkRunRegion {
                    tasks: task_count,
                }),
                added_configuration: new_benchmark_run.added(),
                added_file: NewBenchmarkRunForm::added_file(),
                new_benchmark_run: html::rendered(&new_benchmark_run),
                brief_texts: html::rendered(&BriefTexts::of(&sources)),
                stage_settings: html::rendered(&StageSettings::of(&sources, benchmark.stage)),
                new_task: html::rendered(&new_task),
                path,
                tab,
                tab_links,
                tasks,
                tasks_nav,
                benchmark_runs,
                benchmark_runs_nav,
            }
            .into_response()
        }
        Err(unshown) => {
            let events = paging.path(&format!("/events/benchmarks/{id}"));
            match unshown {
                Unshown::NoSuch => nothing_there(
                    |thing| Page::new(Place::Alone(Area::Benchmarks, thing), events, navigation),
                    Missing::Benchmark(id),
                    frame,
                    "tasks",
                ),
                Unshown::Failed(e) => {
                    let error = format!("{e:#}");
                    warn!(benchmark = id, error, "reading a benchmark failed");
                    let page = ErrorPage {
                        page: Page::new(
                            Place::Alone(Area::Benchmarks, Thing::Found(Heading::named(id))),
                            events,
                            navigation,
                        ),
                        frame,
                        region: "tasks",
                        error,
                    };
                    (StatusCode::INTERNAL_SERVER_ERROR, page).into_response()
                }
            }
        }
    }
}

/// The benchmark page's regions: the notices, and once the benchmark reads,
/// the rest of the shell's, its benchmark runs, whether it offers to launch one, its tasks, and the runs
/// New task offers and whether it can add one.
pub(super) async fn benchmark_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Path(id): Path<String>,
    Query(paging): Query<Paging>,
    navigation: Navigation,
) -> Response {
    let regions = move || {
        let sources = sources.clone();
        let id = id.clone();
        async move {
            let frame = Frame::of(&sources).await;
            let Ok(Shown {
                benchmark,
                task_count,
                tasks,
                tasks_nav,
                benchmark_runs,
                benchmark_runs_nav,
            }) = shown(&sources, &id, paging).await
            else {
                return vec![shell::holds(&frame)];
            };
            let mut regions = shell::regions(&frame, &page(&benchmark, paging, navigation));
            regions.push((
                "benchmark-runs",
                render(&BenchmarkRunsRegion {
                    benchmark_runs: &benchmark_runs,
                    benchmark_runs_nav: benchmark_runs_nav.as_ref(),
                }),
            ));
            regions.push((
                "new-benchmark-run-offer",
                render(&NewBenchmarkRunRegion { tasks: task_count }),
            ));
            regions.push((
                "tasks",
                render(&TasksRegion {
                    tasks: &tasks,
                    tasks_nav: tasks_nav.as_ref(),
                }),
            ));
            regions.push(("brief-texts", render(&BriefTexts::of(&sources))));
            regions.push((
                "stage-settings",
                render(&StageSettings::of(&sources, benchmark.stage)),
            ));
            let runs = runs_of(&sources, benchmark.stage).await;
            let unrefused =
                FieldShown::unrefused(&NewTaskField::Run).attributes(NEW_TASK_RUN, Control::Select);
            regions.push((
                NEW_TASK_RUN,
                render(&NewTaskRuns {
                    runs: &runs,
                    chosen: None,
                    attributes: &unrefused,
                }),
            ));
            let ready = NewTaskReady::of(&runs);
            regions.push((NEW_TASK_READY, render(&ready)));
            regions
        }
    };
    events(regions, stopping)
}

/// The ID of the benchmark of a write's path, or else what the page says of
/// there being none.
fn benchmark_of_path(write: Write, id: &str) -> Result<BenchmarkId, Refused> {
    id.parse()
        .map_err(|_| write.refused(StatusCode::NOT_FOUND, &no_such(id)))
}

/// What the New task dialog's form sends, a field it lacks being empty.
#[derive(Deserialize)]
struct AddForm {
    #[serde(default)]
    run: String,
    #[serde(flatten)]
    task: TaskFields,
}

impl AddForm {
    /// The run it sends and the task, or else the refusal of each field
    /// that does not hold one (`docs/specs/benchmarks.md`, New task;
    /// `docs/specs/design_system.md`, Making, Required); or else, of a run
    /// that does not read, its refusal above the buttons, as a run's.
    fn checked(&self) -> Result<(RunId, Task), making::Refusal<NewTaskField>> {
        let run = if self.run.trim().is_empty() {
            Err(FieldRefusals::of(NewTaskField::Run, REQUIRED))
        } else {
            Ok(self.run.as_str())
        };
        let (run, task) = both(run, self.task.checked().map_err(FieldRefusals::into_fields))
            .map_err(making::Refusal::Fields)?;
        let run = run.parse().map_err(making::Refusal::Whole)?;
        Ok((run, task))
    }
}

/// Benchmark `id`, as its path gives it, with its tasks and benchmark runs,
/// or else the status and why there is no benchmark for a making dialog to
/// make a thing in.
pub(super) async fn benchmark_for_making(
    sources: &Sources,
    id: &str,
) -> Result<BenchmarkShown, (StatusCode, String)> {
    let benchmark: BenchmarkId = id
        .parse()
        .map_err(|_| (StatusCode::NOT_FOUND, no_such(id)))?;
    match benchmarks(sources).show(benchmark).await {
        Ok(Some(shown)) => Ok(shown),
        Ok(None) => Err((StatusCode::NOT_FOUND, no_such(id))),
        Err(e) => {
            let why = format!("{e:#}");
            warn!(benchmark = %benchmark, error = why, "reading a benchmark failed");
            Err((StatusCode::INTERNAL_SERVER_ERROR, why))
        }
    }
}

/// Captures the run the New task dialog's form in `body` names into benchmark
/// `id`, and answers with the dialog's form: as it starts once made, or as
/// sent with why it was refused. Refused, writing nothing, unless the browser
/// shows the request came from the web UI's own pages.
pub(super) async fn add_task(
    State(sources): State<Sources>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Answer<NewTaskForm> {
    const WRITE: Write = Write::Capture;
    let sent = WRITE.read_form::<AddForm>(&body);
    let fields = sent
        .as_ref()
        .map(|sent| NewTaskFields {
            run: sent.run.parse().ok(),
            name: sent.task.name.clone(),
            criteria: sent.task.criteria.clone(),
        })
        .unwrap_or_default();
    let found = benchmark_for_making(&sources, &id).await;
    let mut form = NewTaskForm {
        benchmark: InBenchmark::of(&found, id),
        making: Making::of(fields),
        runs: vec![],
    };
    let sent = match WRITE.same_origin(&headers, &uri).and(sent) {
        Ok(sent) => sent,
        Err(refused) => return form.refused_whole(refused.into_parts()),
    };
    let benchmark = match found {
        Ok(shown) => shown.benchmark,
        Err(refused) => return form.refused_whole(refused),
    };
    form.runs = runs_of(&sources, benchmark.stage).await;
    let (run, task) = match sent.checked() {
        Ok(checked) => checked,
        Err(refusal) => return form.refused((StatusCode::UNPROCESSABLE_ENTITY, refusal)),
    };
    let served = match polled(&sources) {
        Ok(served) => served,
        Err(NotPolled) => return form.refused(NotPolled::refusal()),
    };
    let choice = BenchmarkChoice::Existing(benchmark.id);
    match capture_run(&sources, &served, &run, &choice, task).await {
        Ok(_) => {
            form.making = Making::of(NewTaskFields::starting(&form.runs));
            Answer::Made {
                blank: form,
                shown: MadeShown::OnScreen,
            }
        }
        Err(not) => form.refused(not.refusal(&choice)),
    }
}

/// What the form that removes a task sends.
#[derive(Deserialize)]
struct RemoveForm {
    task: String,
}

/// Removes the task the form in `body` names by its ID from benchmark `id`, and
/// answers with what the page says of it. Refused, removing nothing, unless the
/// browser shows the request came from the web UI's own pages.
pub(super) async fn remove_task(
    State(sources): State<Sources>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Refused> {
    const WRITE: Write = Write::RemoveTask;
    WRITE.same_origin(&headers, &uri)?;
    let benchmark = benchmark_of_path(WRITE, &id)?;
    let form: RemoveForm = WRITE.read_form(&body)?;
    let task: TaskId = form
        .task
        .parse()
        .map_err(|error: String| WRITE.refused(StatusCode::UNPROCESSABLE_ENTITY, &error))?;
    match benchmarks(&sources).remove_task(benchmark, task).await {
        Ok(name) => {
            info!(benchmark = %benchmark, task = %task, name = %name, "removed a task");
            let said = format!("Removed the task {}", html::escape(name.as_str()));
            Ok((StatusCode::OK, alert("alert-success", &said)).into_response())
        }
        Err(e @ (RemoveError::NoSuchBenchmark(_) | RemoveError::NoSuchTask { .. })) => {
            Err(WRITE.refused(StatusCode::NOT_FOUND, &e.to_string()))
        }
        Err(RemoveError::Failed(e)) => {
            let why = format!("{e:#}");
            warn!(benchmark = %benchmark, task = %task, error = why, "removing a task failed");
            Err(WRITE.refused(StatusCode::INTERNAL_SERVER_ERROR, &why))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_log::test]
    fn the_page_opens_on_the_tab_asked_for_or_else_on_runs_or_on_tasks_while_there_are_no_benchmark_runs()
     {
        let opened: Vec<BenchmarkTab> = [
            (None, true),
            (None, false),
            (Some(BenchmarkTab::Tasks), true),
            (Some(BenchmarkTab::Runs), false),
        ]
        .into_iter()
        .map(|(asked, has_benchmark_runs)| BenchmarkTab::opened(asked, has_benchmark_runs))
        .collect();

        assert_eq!(
            opened,
            [
                BenchmarkTab::Runs,
                BenchmarkTab::Tasks,
                BenchmarkTab::Tasks,
                BenchmarkTab::Runs
            ]
        );
    }

    #[test_log::test]
    fn a_tab_is_named_in_its_address_by_its_segment_alone() {
        let parsed: Vec<Result<BenchmarkTab, ()>> = ["runs", "tasks", "Tasks", "nope", ""]
            .into_iter()
            .map(str::parse)
            .collect();

        assert_eq!(
            parsed,
            [
                Ok(BenchmarkTab::Runs),
                Ok(BenchmarkTab::Tasks),
                Err(()),
                Err(()),
                Err(())
            ]
        );
        assert_eq!(
            benchmark_tab_path("1".parse().unwrap(), BenchmarkTab::Tasks),
            "/benchmarks/1/tasks"
        );
    }
}
