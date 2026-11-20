//! The benchmark page (`docs/specs/benchmarks.md`, Benchmark and New task):
//! benchmark NAME's heading, then its benchmark runs and its tasks, each in a
//! tab of its own, the New task dialog that captures a run of the benchmark's
//! stage into it, and the New benchmark run dialog that launches a benchmark
//! run on it.

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
use super::capture::{NOT_POLLED, TaskFields, capture_run, polled};
use super::confirmation::{Confirmation, TurnDown};
use super::html::{self, Html};
use super::launch::{BriefTexts, NewBenchmarkRunForm};
use super::making::{Answer, Making, task_not_made};
use super::pages::{PageHref, Pages, PagesShown, Requested};
use super::shell::{self, Area, Frame, Heading, Missing, Navigation, Place, Thing};
use super::table::{Align, Column, Fold, View};
use super::write::{Refused, Write, alert};
use super::{
    ErrorPage, Page, benchmark_path, benchmark_run_path, benchmarks, events, nothing_there, render,
    run_path, with_filters,
};
use crate::api::Sources;
use crate::benchmarks::results;
use crate::benchmarks::{
    Benchmark, BenchmarkName, BenchmarkRunRecord, BenchmarkRunState, BenchmarkShown, RemoveError,
    TaskEntry, dollars, without,
};
use crate::domain::Timestamp;
use crate::phrase::Status;
use crate::runs::{Recorded, RunId};
use crate::task::{self, BenchmarkChoice, Refusal, TaskName, TaskStage};

/// A task's row.
struct TaskRow {
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
    /// Its row's id, keyed by the task's name.
    fn row_id(&self) -> String {
        format!("task-{}", self.name)
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
                id: format!("remove-{}", task.name),
                verb: Verb::Remove,
                question: format!("Remove the task {} and its directory?", task.name),
                turn_down: TurnDown::Cancel,
                action: format!("{path}/remove-task"),
                fields: vec![("task", task.name.to_string())],
                said: "benchmark-said",
            },
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

    /// Where a link to these pages of benchmark `name` leads.
    fn href(self, name: &BenchmarkName) -> PageHref {
        PageHref::at(self.path(&benchmark_path(name)))
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
    /// The New task dialog's form.
    new_task: Html,
}

impl BenchmarkPage {
    /// Whether the page opens on the tab of its tasks, as it does while the
    /// benchmark has no benchmark runs.
    fn opens_on_tasks(&self) -> bool {
        self.benchmark_runs.is_empty()
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

/// The region of the New task dialog that holds the button "Add task".
const NEW_TASK_READY: &str = "new-task-ready";

/// What the New task dialog's form holds.
#[derive(Template)]
#[template(path = "new_task.html")]
pub(super) struct NewTaskForm {
    /// The benchmark's name, as its path gives it.
    benchmark: String,
    making: Making<NewTaskFields>,
    /// The runs of the benchmark's stage that can be captured, latest first.
    runs: Vec<RunChoice>,
    /// Whether `serve` has polled, so that a capture can run.
    polled: bool,
}

impl NewTaskForm {
    /// The form as it starts, for benchmark `benchmark` of `stage`.
    async fn blank(sources: &Sources, benchmark: &BenchmarkName, stage: TaskStage) -> NewTaskForm {
        let runs = runs_of(sources, stage).await;
        NewTaskForm {
            benchmark: benchmark.to_string(),
            making: Making {
                fields: NewTaskFields::starting(&runs),
                refusal: None,
            },
            runs,
            polled: polled(sources),
        }
    }

    fn title(&self) -> String {
        format!("New task in {}", self.benchmark)
    }

    /// The choice of a run, the region of the page's stream that keeps it.
    fn runs(&self) -> Html {
        html::rendered(&NewTaskRuns {
            runs: &self.runs,
            chosen: self.making.fields.run.as_ref(),
        })
    }

    fn ready(&self) -> NewTaskReady {
        NewTaskReady::of(&self.runs, self.polled)
    }

    /// It refused for `why`, answered with `status`.
    fn refused(mut self, (status, why): (StatusCode, String)) -> Answer<NewTaskForm> {
        self.making.refusal = Some(why);
        Answer::Refused { status, form: self }
    }
}

/// The New task dialog's choice of a run, `chosen` chosen, or else the
/// first; or "No runs".
#[derive(Template)]
#[template(path = "new_task_runs.html")]
struct NewTaskRuns<'a> {
    runs: &'a [RunChoice],
    chosen: Option<&'a RunId>,
}

impl NewTaskRuns<'_> {
    fn chosen(&self, id: &RunId) -> bool {
        self.chosen == Some(id)
    }
}

/// The region of the New task dialog that holds "Add task": disabled with
/// no runs to capture or before `serve` has polled, after `note`, why it
/// cannot run yet.
#[derive(Template)]
#[template(
    source = r#"{%- import "macros.html" as ui -%}{% call ui::new_submit(crate::web::buttons::Verb::AddTask, NEW_TASK_READY, disabled, note) %}{% endcall %}"#,
    ext = "html"
)]
struct NewTaskReady {
    disabled: bool,
    note: &'static str,
}

impl NewTaskReady {
    fn of(runs: &[RunChoice], polled: bool) -> NewTaskReady {
        NewTaskReady {
            disabled: runs.is_empty() || !polled,
            note: if polled { "" } else { NOT_POLLED },
        }
    }
}

/// Why there is no benchmark page: no such benchmark, or one that does not
/// read.
enum Unshown {
    NoSuch,
    Failed(anyhow::Error),
}

/// What is said of a benchmark named `name` that there is none of.
fn no_such(name: &str) -> String {
    format!("there is no benchmark {name}")
}

/// Benchmark `name` with the pages `paging` asks for of its tasks, in its
/// order, and of its benchmark runs, latest first.
async fn shown(sources: &Sources, name: &str, paging: Paging) -> Result<Shown, Unshown> {
    let parsed: BenchmarkName = name.parse().map_err(|_| Unshown::NoSuch)?;
    match benchmarks(sources).show(&parsed).await {
        Ok(Some(BenchmarkShown {
            benchmark,
            tasks,
            benchmark_runs,
        })) => {
            let tasks_page = Pages::of(tasks.len(), paging.tasks);
            let benchmark_runs_page = Pages::of(benchmark_runs.len(), paging.benchmark_runs);
            let path = benchmark_path(&benchmark.name);
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
                    .href(&benchmark.name)
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
                    .href(&benchmark.name)
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
        paging.href(&benchmark.name).stream,
        navigation,
    )
}

pub(super) async fn benchmark_page(
    State(sources): State<Sources>,
    Path(name): Path<String>,
    Query(paging): Query<Paging>,
    navigation: Navigation,
) -> Response {
    let frame = Frame::of(&sources).await;
    match shown(&sources, &name, paging).await {
        Ok(Shown {
            benchmark,
            task_count,
            tasks,
            tasks_nav,
            benchmark_runs,
            benchmark_runs_nav,
        }) => {
            let path = benchmark_path(&benchmark.name);
            let new_task = NewTaskForm::blank(&sources, &benchmark.name, benchmark.stage).await;
            let new_benchmark_run =
                NewBenchmarkRunForm::blank(&sources, &benchmark.name, task_count);
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
                new_task: html::rendered(&new_task),
                path,
                tasks,
                tasks_nav,
                benchmark_runs,
                benchmark_runs_nav,
            }
            .into_response()
        }
        Err(unshown) => {
            let events = paging.path(&format!("/events/benchmarks/{name}"));
            match unshown {
                Unshown::NoSuch => nothing_there(
                    |thing| Page::new(Place::Alone(Area::Benchmarks, thing), events, navigation),
                    Missing::Benchmark(name),
                    frame,
                    "tasks",
                ),
                Unshown::Failed(e) => {
                    let error = format!("{e:#}");
                    warn!(benchmark = name, error, "reading a benchmark failed");
                    let page = ErrorPage {
                        page: Page::new(
                            Place::Alone(Area::Benchmarks, Thing::Found(Heading::named(name))),
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
    Path(name): Path<String>,
    Query(paging): Query<Paging>,
    navigation: Navigation,
) -> Response {
    let regions = move || {
        let sources = sources.clone();
        let name = name.clone();
        async move {
            let frame = Frame::of(&sources).await;
            let Ok(Shown {
                benchmark,
                task_count,
                tasks,
                tasks_nav,
                benchmark_runs,
                benchmark_runs_nav,
            }) = shown(&sources, &name, paging).await
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
            let runs = runs_of(&sources, benchmark.stage).await;
            regions.push((
                "new-task-run",
                render(&NewTaskRuns {
                    runs: &runs,
                    chosen: None,
                }),
            ));
            let ready = NewTaskReady::of(&runs, polled(&sources));
            regions.push((NEW_TASK_READY, render(&ready)));
            regions
        }
    };
    events(regions, stopping)
}

/// Benchmark `name` of a write's path, or else what the page says of there
/// being none.
fn benchmark_of_path(write: Write, name: &str) -> Result<BenchmarkName, Refused> {
    name.parse()
        .map_err(|_| write.refused(StatusCode::NOT_FOUND, &no_such(name)))
}

/// What the New task dialog's form sends, a field it lacks being empty.
#[derive(Deserialize)]
struct AddForm {
    #[serde(default)]
    run: String,
    #[serde(flatten)]
    task: TaskFields,
}

/// Benchmark `name` with its tasks and benchmark runs, or else the status and
/// why there is no benchmark for a "New" dialog to make a thing in.
pub(super) async fn benchmark_for_making(
    sources: &Sources,
    name: &str,
) -> Result<BenchmarkShown, (StatusCode, String)> {
    let benchmark: BenchmarkName = name
        .parse()
        .map_err(|_| (StatusCode::NOT_FOUND, no_such(name)))?;
    match benchmarks(sources).show(&benchmark).await {
        Ok(Some(shown)) => Ok(shown),
        Ok(None) => Err((StatusCode::NOT_FOUND, no_such(name))),
        Err(e) => {
            let why = format!("{e:#}");
            warn!(benchmark = %benchmark, error = why, "reading a benchmark failed");
            Err((StatusCode::INTERNAL_SERVER_ERROR, why))
        }
    }
}

/// Captures the run the New task dialog's form in `body` names into benchmark
/// `name`, and answers with the dialog's form: as it starts once made, or as
/// sent with why it was refused. Refused, writing nothing, unless the browser
/// shows the request came from the web UI's own pages.
pub(super) async fn add_task(
    State(sources): State<Sources>,
    Path(name): Path<String>,
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
    let mut form = NewTaskForm {
        benchmark: name,
        making: Making {
            fields,
            refusal: None,
        },
        runs: vec![],
        polled: polled(&sources),
    };
    let sent = match WRITE.same_origin(&headers, &uri).and(sent) {
        Ok(sent) => sent,
        Err(refused) => return form.refused(refused.into_parts()),
    };
    let (benchmark, stage) = match benchmark_for_making(&sources, &form.benchmark).await {
        Ok(shown) => (shown.benchmark.name, shown.benchmark.stage),
        Err(refused) => return form.refused(refused),
    };
    form.runs = runs_of(&sources, stage).await;
    let run: RunId = match sent.run.parse() {
        Ok(run) => run,
        Err(why) => return form.refused((StatusCode::UNPROCESSABLE_ENTITY, why)),
    };
    match capture_run(
        &sources,
        &run,
        &BenchmarkChoice::Named(benchmark),
        &sent.task,
    )
    .await
    {
        Ok(_) => {
            form.making = Making {
                fields: NewTaskFields::starting(&form.runs),
                refusal: None,
            };
            Answer::Made { blank: form }
        }
        Err(not) => form.refused((not.status(), task_not_made(&not))),
    }
}

/// What the form that removes a task sends.
#[derive(Deserialize)]
struct RemoveForm {
    task: String,
}

/// Removes the task the form in `body` names from benchmark `name`, and answers
/// with what the page says of it. Refused, removing nothing, unless the
/// browser shows the request came from the web UI's own pages.
pub(super) async fn remove_task(
    State(sources): State<Sources>,
    Path(name): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Refused> {
    const WRITE: Write = Write::RemoveTask;
    WRITE.same_origin(&headers, &uri)?;
    let benchmark = benchmark_of_path(WRITE, &name)?;
    let form: RemoveForm = WRITE.read_form(&body)?;
    let task: TaskName = form
        .task
        .parse()
        .map_err(|error: String| WRITE.refused(StatusCode::UNPROCESSABLE_ENTITY, &error))?;
    match benchmarks(&sources).remove_task(&benchmark, &task).await {
        Ok(()) => {
            info!(benchmark = %benchmark, task = %task, "removed a task");
            let said = format!("Removed the task {}", html::escape(task.as_str()));
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
