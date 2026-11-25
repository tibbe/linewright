//! Task (`docs/specs/benchmarks.md`, Task, Edit task and Remove task): task
//! ID's heading, its facts and its criteria, the dialog that changes it and
//! the confirmation that removes it.

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tracing::warn;

use linewright_core::access::{Permit, Read};
use linewright_core::actions;
use linewright_core::actions::{Action, TaskFactsScreen, TaskScreen};
use linewright_core::api::Sources;
use linewright_core::benchmarks::{CriteriaAnswer, TaskId};

use super::buttons::{ScreenButton, Verb};
use super::caller::Reader;
use super::confirmation::{Confirmation, TurnDown};
use super::editing::{self, EditDialog};
use super::html::{self, Html};
use super::icons::CardIcon;
use super::item_parts::InlineItem;
use super::shell::{self, Area, Frame, Heading, Navigation, Place};
use super::stats::{Stat, absent};
use super::{ErrorPage, Page, benchmark_path, events, nothing_there, render, run_path};

/// The name of Task's screen, its last crumb.
const TASK: &str = "Task";

/// The id of the dialog "Remove" opens.
const REMOVE: &str = "remove-task";

/// The id of its question, which the `confirm_dialog` macro gives it.
const REMOVE_QUESTION: &str = "remove-task_question";

/// A task's criteria, as Task shows them.
enum CriteriaShown {
    Read(String),
    /// Why they do not read.
    Unreadable(String),
}

/// A task's item and start, as Task's facts show them.
enum ItemAndStart {
    Read {
        item: Html,
        started: Html,
    },
    /// Why they do not read.
    Unreadable(String),
}

/// A task as Task shows it.
struct TaskView {
    /// The run it was captured from, linking to its Run.
    captured_from: Html,
    item_and_start: ItemAndStart,
    criteria: CriteriaShown,
    /// Its removal (`docs/specs/benchmarks.md`, Remove task).
    remove: Confirmation,
}

impl TaskView {
    /// Of `task`, of the repo `repo`.
    fn of(shown: &TaskScreen, repo: &str) -> TaskView {
        let task = &shown.task;
        TaskView {
            captured_from: html::built(format!(
                r#"<a class="link link-hover" href="{}">{}</a>"#,
                html::escape(&run_path(&task.run)),
                html::escape(&task.run.to_string()),
            )),
            item_and_start: match &shown.facts {
                TaskFactsScreen::Read {
                    item,
                    title,
                    started_at,
                } => ItemAndStart::Read {
                    item: html::rendered(&InlineItem::of(*item, title, repo)),
                    started: html::time(*started_at),
                },
                TaskFactsScreen::Unreadable { facts_unreadable } => {
                    ItemAndStart::Unreadable(facts_unreadable.clone())
                }
            },
            criteria: match &task.criteria {
                CriteriaAnswer::Criteria(criteria) => CriteriaShown::Read(criteria.clone()),
                CriteriaAnswer::Unreadable(why) => CriteriaShown::Unreadable(why.clone()),
            },
            remove: Confirmation {
                id: REMOVE.into(),
                verb: Verb::Remove,
                question: format!("Remove the task {} and its directory?", task.name),
                turn_down: TurnDown::Cancel,
                action: format!("{}/remove-task", benchmark_path(shown.benchmark.id)),
                fields: vec![("task", task.id.to_string())],
                said: "task-said",
            },
        }
    }

    /// Captured from, Item and Started (`docs/specs/benchmarks.md`, Task,
    /// Facts), Item and Started absent where they do not read.
    fn facts(&self) -> [Stat; 3] {
        let (item, started) = match &self.item_and_start {
            ItemAndStart::Read { item, started } => (item.clone(), started.clone()),
            ItemAndStart::Unreadable(_) => (absent(), absent()),
        };
        [
            Stat {
                label: "Captured from",
                value: self.captured_from.clone(),
            },
            Stat {
                label: "Item",
                value: item,
            },
            Stat {
                label: "Started",
                value: started,
            },
        ]
    }
}

/// The address of task `id`'s stream, as its address gives `id`.
fn stream(id: impl std::fmt::Display) -> String {
    format!("/events/tasks/{id}")
}

#[derive(Template, WebTemplate)]
#[template(path = "task.html")]
struct TaskPage {
    page: Page,
    frame: Frame,
    task: TaskView,
    edit: EditDialog,
    edit_values: Html,
}

#[derive(Template)]
#[template(path = "task_facts.html")]
struct FactsRegion<'a> {
    task: &'a TaskView,
}

#[derive(Template)]
#[template(path = "task_criteria.html")]
struct CriteriaRegion<'a> {
    task: &'a TaskView,
}

/// The question of the task's removal, a region of Task's stream, so that
/// it names the task as it is named now.
#[derive(Template)]
#[template(
    source = r#"<p id="{{ REMOVE_QUESTION }}">{{ task.remove.question }}</p>"#,
    ext = "html"
)]
struct QuestionRegion<'a> {
    task: &'a TaskView,
}

/// The page of `task`: under its benchmark, its heading its name with its
/// stage under it, and "Edit" then "Remove" among the screen's buttons
/// (`docs/specs/benchmarks.md`, Task, Heading).
fn page(task: &TaskScreen, navigation: Navigation) -> Page {
    let heading = Heading {
        icon: Some(CardIcon::Beaker),
        line: Some(html::escape(&task.benchmark.stage.to_string())),
        ..Heading::named(task.task.name.as_str())
    };
    Page::new(
        Place::within(Area::Benchmarks)
            .under(
                task.benchmark.name.to_string(),
                benchmark_path(task.benchmark.id),
            )
            .screen(TASK, heading),
        stream(task.task.id),
        navigation,
    )
    .buttons(vec![
        editing::edit_task_button(),
        ScreenButton::Act {
            verb: Verb::Remove,
            dialog: REMOVE.into(),
            action: Action::RemoveTask,
        },
    ])
}

/// Why there is no task page: no such task, or one that does not read.
enum Unshown {
    NoSuch,
    Failed(anyhow::Error),
}

/// Task `id`, by its ID as a path gives it.
async fn task_of(
    permit: &Permit<Read>,
    sources: &Sources,
    id: &str,
) -> Result<TaskScreen, Unshown> {
    let parsed: TaskId = id.parse().map_err(|_: String| Unshown::NoSuch)?;
    match actions::get_task(permit, sources, parsed).await {
        Ok(Some(task)) => Ok(task),
        Ok(None) => Err(Unshown::NoSuch),
        Err(e) => Err(Unshown::Failed(e)),
    }
}

pub(super) async fn task_page(
    Reader { permit, sources }: Reader,
    Path(id): Path<String>,
    navigation: Navigation,
) -> Response {
    let frame = Frame::of(&permit, &sources).await;
    match task_of(&permit, &sources, &id).await {
        Ok(task) => TaskPage {
            page: page(&task, navigation),
            frame,
            task: TaskView::of(&task, &sources.work_dir.repo),
            edit: editing::edit_task(&task.task),
            edit_values: html::built(editing::task_values(&task.task)),
        }
        .into_response(),
        Err(unshown) => {
            let events = stream(&id);
            match unshown {
                Unshown::NoSuch => nothing_there(events, navigation, frame, "task"),
                Unshown::Failed(e) => {
                    let error = format!("{e:#}");
                    warn!(task = id, error, "reading a task failed");
                    let heading = Heading {
                        icon: Some(CardIcon::Beaker),
                        ..Heading::named(id)
                    };
                    let page = ErrorPage {
                        page: Page::new(
                            Place::within(Area::Benchmarks).screen(TASK, heading),
                            events,
                            navigation,
                        ),
                        frame,
                        region: "task",
                        error,
                    };
                    (StatusCode::INTERNAL_SERVER_ERROR, page).into_response()
                }
            }
        }
    }
}

/// Task's regions: the notices, and while the task reads, the rest of the
/// shell's, its facts, its criteria, the question of its removal and the
/// values Edit task starts from.
pub(super) async fn task_events(
    Reader { permit, sources }: Reader,
    watch: super::caller::Watch,
    Path(id): Path<String>,
    navigation: Navigation,
) -> Response {
    let regions = move || {
        let (permit, sources) = (permit.clone(), sources.clone());
        let id = id.clone();
        async move {
            let frame = Frame::of(&permit, &sources).await;
            let Ok(task) = task_of(&permit, &sources, &id).await else {
                return vec![shell::holds(&frame)];
            };
            let view = TaskView::of(&task, &sources.work_dir.repo);
            let mut regions = shell::regions(&frame, &page(&task, navigation));
            regions.push(("task", render(&FactsRegion { task: &view })));
            regions.push(("criteria", render(&CriteriaRegion { task: &view })));
            regions.push((REMOVE_QUESTION, render(&QuestionRegion { task: &view })));
            regions.push((editing::EDIT_TASK_VALUES, editing::task_values(&task.task)));
            regions
        }
    };
    events(regions, watch)
}
