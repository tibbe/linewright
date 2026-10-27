//! The automations page (`docs/specs/design_system.md`, The automations page):
//! each automation file that does not parse, then a row per automation.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use tokio_util::sync::CancellationToken;

use super::html::{self, Html};
use super::names::Names;
use super::{ErrorPage, Frame, Page, Read, events, frame_regions, render, run_path};
use crate::api::{Sources, Unexplained};
use crate::domain::Timestamp;
use crate::phrase::{Phrase, Tone};
use crate::runs::{self, Recorded, Run};
use crate::status::{self, Schedule, Status, UnparsedAutomation, View};

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
    next_run: Schedule,
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
        status: &Status,
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
                // With no run recorded, the last scheduled run is the time a
                // poll first saw the automation.
                let last_run = match last_run {
                    Some(run) => LastRun::Run(run.clone().at(now)),
                    None => LastRun::NoneYet {
                        first_seen: view.last_scheduled_runs.get(&automation.name),
                    },
                };
                Some(Row {
                    name: answered.name.clone(),
                    schedule: automation.schedule.describe(),
                    last_run,
                    next_run: answered.schedule,
                })
            })
            .collect();
        Automations {
            unparsed: status.unparsed_automations.clone(),
            rows,
        }
    }
}

/// The automations, ready for the page.
struct AutomationsShown {
    unparsed: Vec<Html>,
    rows: Vec<RowShown>,
}

struct RowShown {
    name: String,
    schedule: String,
    last_run: LastRunShown,
    next_run: Html,
}

enum LastRunShown {
    Run {
        id: Html,
        href: String,
        tone: Tone,
        word: &'static str,
        about: Html,
    },
    NoneYet(Html),
}

impl AutomationsShown {
    fn of(automations: &Automations, names: &Names) -> AutomationsShown {
        let html = |phrase: &Phrase| html::phrase(phrase, names);
        AutomationsShown {
            unparsed: automations
                .unparsed
                .iter()
                .map(|file| html(&status::does_not_parse(file)))
                .collect(),
            rows: automations
                .rows
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
                            tone: run.outcome.tone(),
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

/// The frame and the automations, from one read of `serve`'s view and the
/// runs; `None` for the automations before the first poll.
fn shown(sources: &Sources) -> (Frame, Option<AutomationsShown>) {
    let read = Read::of(sources);
    let automations = read
        .automations
        .as_ref()
        .map(|automations| AutomationsShown::of(automations, &read.names(&sources.work_dir.repo)));
    (Frame::of_read(sources, &read), automations)
}

pub(super) async fn automations_page(State(sources): State<Sources>) -> Response {
    let page = Page::new(
        format!("Automations · {}", sources.work_dir.repo),
        "/events/automations".into(),
    )
    .filling_the_viewport();
    match shown(&sources) {
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

/// The automations page's regions: the frame's, and the automations once a
/// poll has seen them.
pub(super) async fn automations_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
) -> Response {
    let regions = move || {
        let (frame, automations) = shown(&sources);
        let mut regions = frame_regions(&frame);
        if let Some(automations) = automations {
            regions.push((
                "automations",
                render(&AutomationsRegion {
                    automations: &automations,
                }),
            ));
        }
        regions
    };
    events(regions, stopping)
}
