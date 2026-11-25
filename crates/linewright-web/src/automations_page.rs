//! The automations page (`docs/specs/automations.md`, Automations):
//! each automation file that does not parse, then a row per automation.

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::Query;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use linewright_core::access::{Permit, Read};
use linewright_core::actions::{self, AutomationsScreen, LastRunScreen};
use linewright_core::api::{Sources, Unexplained};
use linewright_core::phrase::{Phrase, Status};
use linewright_core::runs;
use linewright_core::status;

use super::caller::Reader;
use super::html::{self, Html};
use super::names::Naming;
use super::pages::{PageHref, Pages, PagesShown, Requested};
use super::shell::{self, Area, Frame, Navigation, Place};
use super::table::{Align, Column};
use super::{ErrorPage, Page, events, render, run_path, with_filters};

/// The automations, ready for the page: a page of their rows, with the pages
/// under them.
struct AutomationsShown {
    unparsed: Vec<Html>,
    rows: Vec<RowShown>,
    pages: Option<PagesShown>,
}

struct RowShown {
    name: String,
    trigger: String,
    last_run: LastRunShown,
    /// `None` for an automation not on a schedule.
    next_run: Option<Html>,
}

/// A column of the table of automations, the first naming each row.
#[derive(Clone, Copy)]
enum AutomationsColumn {
    Name,
    Trigger,
    LastRun,
    NextRun,
}

impl AutomationsColumn {
    const ALL: [AutomationsColumn; 4] = [
        AutomationsColumn::Name,
        AutomationsColumn::Trigger,
        AutomationsColumn::LastRun,
        AutomationsColumn::NextRun,
    ];

    fn head(self) -> &'static str {
        match self {
            AutomationsColumn::Name => "Automation",
            AutomationsColumn::Trigger => "Trigger",
            AutomationsColumn::LastRun => "Last run",
            AutomationsColumn::NextRun => "Next run",
        }
    }

    /// The class that aligns the column's head and cells as its kind does.
    fn align(self) -> &'static str {
        let column = match self {
            AutomationsColumn::Name
            | AutomationsColumn::Trigger
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
    fn of(
        automations: &AutomationsScreen,
        naming: &Naming,
        requested: Requested,
    ) -> AutomationsShown {
        let html = |phrase: &Phrase| html::phrase(phrase, naming);
        let pages = Pages::of(automations.automations.len(), requested);
        AutomationsShown {
            unparsed: automations
                .unparsed_automations
                .iter()
                .map(|file| html(&status::does_not_parse(file)))
                .collect(),
            rows: automations.automations[pages.range()]
                .iter()
                .map(|row| RowShown {
                    name: row.name.clone(),
                    trigger: row.when.clone(),
                    last_run: match &row.last_run {
                        LastRunScreen::Run {
                            run,
                            last_run_outcome,
                        } => LastRunShown::Run {
                            id: html::escape(&run.id.to_string()),
                            href: run_path(&run.id),
                            about: html(&runs::describe_start(run.by_hand(), run.started_at)),
                            status: last_run_outcome.status(),
                            word: last_run_outcome.word(),
                        },
                        LastRunScreen::NoneYet { first_seen } => {
                            LastRunShown::NoneYet(html(&match first_seen {
                                Some(at) => Phrase::from("none yet, first seen ").time(*at),
                                None => "none yet".into(),
                            }))
                        }
                    },
                    next_run: status::describe_next_run(row.trigger).map(|next| html(&next)),
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
async fn shown(
    permit: &Permit<Read>,
    sources: &Sources,
    requested: Requested,
) -> (Frame, Option<AutomationsShown>) {
    let read = actions::overview(permit, sources).await;
    let automations = read.automations.as_ref().map(|automations| {
        AutomationsShown::of(
            automations,
            &shell::naming(&read, &sources.work_dir.repo),
            requested,
        )
    });
    (Frame::of_read(permit, sources, &read), automations)
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
    Reader { permit, sources }: Reader,
    Query(query): Query<AutomationsQuery>,
    navigation: Navigation,
) -> Response {
    let requested = query.page;
    let page = page(requested, navigation);
    match shown(&permit, &sources, requested).await {
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
    Reader { permit, sources }: Reader,
    watch: super::caller::Watch,
    Query(query): Query<AutomationsQuery>,
    navigation: Navigation,
) -> Response {
    let requested = query.page;
    let regions = move || {
        let (permit, sources) = (permit.clone(), sources.clone());
        async move {
            let (frame, automations) = shown(&permit, &sources, requested).await;
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
    events(regions, watch)
}
