//! The automations page (`docs/specs/automations.md`, Automations):
//! each automation file that does not parse, then a row per automation.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::html::{self, Html};
use super::names::Names;
use super::pages::{PageHref, Pages, PagesShown, Requested};
use super::shell::{self, Area, Frame, Navigation, Place, Read};
use super::table::{Align, Column};
use super::{ErrorPage, Page, events, render, run_path, with_filters};
use crate::api::{Sources, Unexplained};
use crate::domain::Timestamp;
use crate::phrase::{Phrase, Status};
use crate::runs::{self, Recorded, Run};
use crate::status::{self, Trigger, UnparsedAutomation, View};

/// The automations as `status` answers for them, with what the page shows
/// beside: each schedule in words, and each last run's outcome.
pub(super) struct Automations {
    /// By path.
    pub(super) unparsed: Vec<UnparsedAutomation>,
    /// By name, as `status` has them.
    pub(super) rows: Vec<Row>,
}

pub(super) struct Row {
    pub(super) name: String,
    /// When it runs, in words.
    schedule: String,
    pub(super) last_run: LastRun,
    next_run: Trigger,
}

pub(super) enum LastRun {
    Run(Run),
    /// Before its first run, with when Linewright first saw it, which is
    /// unknown until a poll has.
    NoneYet {
        first_seen: Option<Timestamp>,
    },
}

impl Automations {
    /// From `status`'s answer over `view` and `recorded`.
    pub(super) fn of(
        status: &status::Status,
        view: &View,
        recorded: &[Recorded],
        now: Timestamp,
    ) -> Automations {
        let rows = status
            .automations
            .iter()
            .filter_map(|answered| {
                let automation = view
                    .automations
                    .parsed
                    .iter()
                    .find(|automation| automation.name.to_string() == answered.name)?;
                let last_run = answered
                    .last_run
                    .as_ref()
                    .and_then(|last| recorded.iter().find(|run| run.id == last.id));
                let last_run = match last_run {
                    Some(run) => LastRun::Run(run.clone().at(now)),
                    None => LastRun::NoneYet {
                        first_seen: view.last_scheduled_runs.get(&automation.name),
                    },
                };
                Some(Row {
                    name: answered.name.clone(),
                    schedule: automation.trigger.describe(),
                    last_run,
                    next_run: answered.trigger,
                })
            })
            .collect();
        Automations {
            unparsed: status.unparsed_automations.clone(),
            rows,
        }
    }
}

/// The automations, ready for the page: a page of their rows, with the pages
/// under them.
struct AutomationsShown {
    unparsed: Vec<Html>,
    rows: Vec<RowShown>,
    pages: Option<PagesShown>,
}

struct RowShown {
    name: String,
    schedule: String,
    last_run: LastRunShown,
    next_run: Html,
}

/// A column of the table of automations, the first naming each row.
#[derive(Clone, Copy)]
enum AutomationsColumn {
    Name,
    Schedule,
    LastRun,
    NextRun,
}

impl AutomationsColumn {
    const ALL: [AutomationsColumn; 4] = [
        AutomationsColumn::Name,
        AutomationsColumn::Schedule,
        AutomationsColumn::LastRun,
        AutomationsColumn::NextRun,
    ];

    fn head(self) -> &'static str {
        match self {
            AutomationsColumn::Name => "Automation",
            AutomationsColumn::Schedule => "Schedule",
            AutomationsColumn::LastRun => "Last run",
            AutomationsColumn::NextRun => "Next run",
        }
    }

    /// The class that aligns the column's head and cells as its kind does.
    fn align(self) -> &'static str {
        let column = match self {
            AutomationsColumn::Name
            | AutomationsColumn::Schedule
            | AutomationsColumn::LastRun
            | AutomationsColumn::NextRun => Column::Other,
        };
        Align::of(column).class()
    }
}

enum LastRunShown {
    Run {
        id: Html,
        href: String,
        status: Status,
        word: &'static str,
        about: Html,
    },
    NoneYet(Html),
}

impl AutomationsShown {
    fn of(automations: &Automations, names: &Names, requested: Requested) -> AutomationsShown {
        let html = |phrase: &Phrase| html::phrase(phrase, names);
        let pages = Pages::of(automations.rows.len(), requested);
        AutomationsShown {
            unparsed: automations
                .unparsed
                .iter()
                .map(|file| html(&status::does_not_parse(file)))
                .collect(),
            rows: automations.rows[pages.range()]
                .iter()
                .map(|row| RowShown {
                    name: row.name.clone(),
                    schedule: row.schedule.clone(),
                    last_run: match &row.last_run {
                        LastRun::Run(run) => LastRunShown::Run {
                            id: html::escape(&run.id.to_string()),
                            href: run_path(&run.id),
                            about: html(&runs::describe_start(
                                run.started().by_hand(),
                                run.started_at,
                            )),
                            status: run.outcome.status(),
                            word: run.outcome.word(),
                        },
                        LastRun::NoneYet { first_seen } => {
                            LastRunShown::NoneYet(html(&match first_seen {
                                Some(at) => Phrase::from("none yet, first seen ").time(*at),
                                None => "none yet".into(),
                            }))
                        }
                    },
                    next_run: html(&status::describe_next_run(row.next_run)),
                })
                .collect(),
            pages: pages.shown("automations", |page| {
                PageHref::at(path("/automations", page))
            }),
        }
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "automations.html")]
struct AutomationsPage {
    page: Page,
    frame: Frame,
    automations: AutomationsShown,
}

#[derive(Template)]
#[template(path = "automations.html", block = "automations")]
struct AutomationsRegion<'a> {
    automations: &'a AutomationsShown,
}

/// What the page's address carries: the page of the table.
#[derive(Deserialize)]
pub(super) struct AutomationsQuery {
    #[serde(default)]
    page: Requested,
}

/// `path` with the page `page` as its query.
fn path(path: &str, page: Requested) -> String {
    with_filters(path, &[("page", &page.field())])
}

/// The frame and the `requested` page of the automations, from one read of
/// `serve`'s view and the runs; `None` for the automations before the first
/// poll.
async fn shown(sources: &Sources, requested: Requested) -> (Frame, Option<AutomationsShown>) {
    let read = Read::of(sources).await;
    let automations = read.automations.as_ref().map(|automations| {
        AutomationsShown::of(automations, &read.names(&sources.work_dir.repo), requested)
    });
    (Frame::of_read(sources, &read), automations)
}

/// The automations page, its stream's on the `requested` page.
fn page(requested: Requested, navigation: Navigation) -> Page {
    Page::new(
        Place::Navigation(Area::Automations),
        path("/events/automations", requested),
        navigation,
    )
    .filling_the_viewport()
}

pub(super) async fn automations_page(
    State(sources): State<Sources>,
    Query(query): Query<AutomationsQuery>,
    navigation: Navigation,
) -> Response {
    let requested = query.page;
    let page = page(requested, navigation);
    match shown(&sources, requested).await {
        (frame, Some(automations)) => AutomationsPage {
            page,
            frame,
            automations,
        }
        .into_response(),
        (frame, None) => {
            let page = ErrorPage {
                page,
                frame,
                region: "automations",
                error: Unexplained::NotPolledYet.to_string(),
            };
            (Unexplained::NotPolledYet.status(), page).into_response()
        }
    }
}

/// The automations page's regions: the shell's, and the automations once a
/// poll has seen them.
pub(super) async fn automations_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Query(query): Query<AutomationsQuery>,
    navigation: Navigation,
) -> Response {
    let requested = query.page;
    let regions = move || {
        let sources = sources.clone();
        async move {
            let (frame, automations) = shown(&sources, requested).await;
            let mut regions = shell::regions(&frame, &page(requested, navigation));
            if let Some(automations) = automations {
                regions.push((
                    "automations",
                    render(&AutomationsRegion {
                        automations: &automations,
                    }),
                ));
            }
            regions
        }
    };
    events(regions, stopping)
}
