//! The suites page (`docs/specs/benchmarks.md`, Suites): a row per suite,
//! narrowed by its filters, and the New suite dialog that makes one.

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

use super::html::{self, Html};
use super::making::{Answer, Making, NOT_A_NAME, suite_not_made};
use super::shell::{Area, Frame, Trail, frame_regions};
use super::table::{Align, Column};
use super::write::Write;
use super::{
    Page, events, experiment_path, experiments, given, render, suite_path, suites, with_filters,
};
use crate::api::Sources;
use crate::benchmarks::{ExperimentId, MakeError, SuiteListed, SuiteName};
use crate::case::ReplayStage;
use crate::domain::Timestamp;

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
    stage: Option<ReplayStage>,
}

impl Narrowing {
    fn of(fields: &FilterQuery) -> Narrowing {
        let fields = FilterQuery {
            name: fields.name.trim().to_string(),
            stage: fields.stage.trim().to_string(),
        };
        let narrowed = given(&fields.stage)
            .map(str::parse::<ReplayStage>)
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
    /// Whether it keeps `listed` (`docs/specs/benchmarks.md`, Suites,
    /// Filters): its name holds the text, whatever its letter case, and it
    /// is of the stage.
    fn keeps(&self, listed: &SuiteListed) -> bool {
        self.name.as_ref().is_none_or(|name| {
            listed
                .suite
                .name
                .as_str()
                .to_lowercase()
                .contains(&name.to_lowercase())
        }) && self.stage.is_none_or(|stage| listed.suite.stage == stage)
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
            stages: ReplayStage::ALL
                .into_iter()
                .map(|stage| (stage.name(), fields.stage == stage.name()))
                .collect(),
        }
    }
}

/// A suite's row.
struct Row {
    name: SuiteName,
    stage: ReplayStage,
    href: String,
    /// Its latest experiment, if it has one.
    last: Option<LastExperiment>,
}

/// A suite's latest experiment.
struct LastExperiment {
    href: String,
    /// When it was launched.
    launched: Html,
}

impl Row {
    fn of(listed: SuiteListed, last: Option<(ExperimentId, Timestamp)>) -> Row {
        Row {
            href: suite_path(&listed.suite.name),
            name: listed.suite.name,
            stage: listed.suite.stage,
            last: last.map(|(id, at)| LastExperiment {
                href: experiment_path(id),
                launched: html::time(at),
            }),
        }
    }
}

/// A column of the table of suites, the first naming each row.
#[derive(Clone, Copy)]
enum SuitesColumn {
    Name,
    Stage,
    LastExperiment,
}

impl SuitesColumn {
    const ALL: [SuitesColumn; 3] = [
        SuitesColumn::Name,
        SuitesColumn::Stage,
        SuitesColumn::LastExperiment,
    ];

    /// Empty for the column that names each row, which has no head.
    fn head(self) -> &'static str {
        match self {
            SuitesColumn::Name => "",
            SuitesColumn::Stage => "Stage",
            SuitesColumn::LastExperiment => "Last experiment",
        }
    }

    /// The class that aligns the column's head and cells as its kind does.
    fn align(self) -> &'static str {
        let column = match self {
            SuitesColumn::Name | SuitesColumn::Stage | SuitesColumn::LastExperiment => {
                Column::Other
            }
        };
        Align::of(column).class()
    }
}

/// The suites the filters keep, by name, or why they are not listed.
enum SuitesShown {
    Table(Vec<Row>),
    Unlisted(String),
}

/// The New suite dialog's fields (`docs/specs/benchmarks.md`, New suite).
pub(super) struct NewSuiteFields {
    name: String,
    stage: ReplayStage,
}

impl NewSuiteFields {
    /// The stage that "Stage" starts on: the first.
    const STARTING_STAGE: ReplayStage = ReplayStage::ALL[0];
}

impl Default for NewSuiteFields {
    fn default() -> NewSuiteFields {
        NewSuiteFields {
            name: String::new(),
            stage: NewSuiteFields::STARTING_STAGE,
        }
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "suites.html")]
struct SuitesPage {
    page: Page,
    frame: Frame,
    filters: Filters,
    suites: SuitesShown,
    new_suite: Making<NewSuiteFields>,
}

#[derive(Template)]
#[template(path = "suites_table.html")]
struct SuitesRegion<'a> {
    suites: &'a SuitesShown,
}

/// What the New suite dialog's form holds.
#[derive(Template)]
#[template(path = "new_suite.html")]
pub(super) struct NewSuiteForm {
    new_suite: Making<NewSuiteFields>,
}

/// The suites the narrowing keeps, by name, or why they are not listed, and
/// the status the page answers with.
async fn listed(sources: &Sources, narrowing: &Narrowing) -> (StatusCode, SuitesShown) {
    let narrowed = match &narrowing.narrowed {
        Ok(narrowed) => narrowed,
        Err(e) => return (StatusCode::BAD_REQUEST, SuitesShown::Unlisted(e.clone())),
    };
    let read = async {
        let listed = suites(sources).list().await?;
        let latest = experiments(sources).latest().await?;
        anyhow::Ok((listed, latest))
    };
    match read.await {
        Ok((listed, mut latest)) => {
            let rows = listed
                .into_iter()
                .filter(|listed| narrowed.keeps(listed))
                .map(|listed| {
                    let last = latest.remove(&listed.suite.name);
                    Row::of(listed, last)
                })
                .collect();
            (StatusCode::OK, SuitesShown::Table(rows))
        }
        Err(e) => {
            let error = format!("{e:#}");
            warn!(error, "reading the suites failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                SuitesShown::Unlisted(error),
            )
        }
    }
}

pub(super) async fn suites_page(
    State(sources): State<Sources>,
    Query(fields): Query<FilterQuery>,
) -> Response {
    let narrowing = Narrowing::of(&fields);
    let (status, suites) = listed(&sources, &narrowing).await;
    let page = SuitesPage {
        page: Page::new(Trail::of(Area::Suites), narrowing.path("/events/suites")),
        frame: Frame::of(&sources).await,
        filters: Filters::of(&narrowing.fields),
        suites,
        new_suite: Making::blank(),
    };
    (status, page).into_response()
}

/// The suites page's regions: the frame's, and the suites the filters keep.
pub(super) async fn suites_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Query(fields): Query<FilterQuery>,
) -> Response {
    let narrowing = Arc::new(Narrowing::of(&fields));
    let regions = move || {
        let sources = sources.clone();
        let narrowing = narrowing.clone();
        async move {
            let mut regions = frame_regions(&Frame::of(&sources).await);
            let (_, suites) = listed(&sources, &narrowing).await;
            regions.push(("suites", render(&SuitesRegion { suites: &suites })));
            regions
        }
    };
    events(regions, stopping)
}

/// What the New suite dialog's form sends, a field it lacks being empty.
#[derive(Deserialize)]
struct MakeForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    stage: String,
}

/// Makes the suite the form in `body` names, and answers with the dialog's
/// form: as it starts once made, or as sent with why it was refused.
/// Refused, writing nothing, unless the browser shows the request came from
/// the web UI's own pages.
pub(super) async fn make_suite(
    State(sources): State<Sources>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Answer<NewSuiteForm> {
    const WRITE: Write = Write::MakeSuite;
    let sent = WRITE.read_form::<MakeForm>(&body);
    let fields = match &sent {
        Ok(form) => NewSuiteFields {
            name: form.name.clone(),
            stage: form.stage.parse().unwrap_or(NewSuiteFields::STARTING_STAGE),
        },
        Err(_) => NewSuiteFields::default(),
    };
    let refuse = |status, why| Answer::Refused {
        status,
        form: NewSuiteForm {
            new_suite: Making::refused(fields, why),
        },
    };
    let form = match WRITE.same_origin(&headers, &uri).and(sent) {
        Ok(form) => form,
        Err(refused) => {
            let (status, why) = refused.into_parts();
            return refuse(status, why);
        }
    };
    let Ok(name) = form.name.parse::<SuiteName>() else {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, NOT_A_NAME.into());
    };
    let stage = match form.stage.parse::<ReplayStage>() {
        Ok(stage) => stage,
        Err(why) => return refuse(StatusCode::UNPROCESSABLE_ENTITY, why),
    };
    match suites(&sources).make(&name, stage).await {
        Ok(suite) => {
            info!(suite = %suite.name, stage = %suite.stage, "made a suite");
            Answer::Made {
                blank: NewSuiteForm {
                    new_suite: Making::blank(),
                },
            }
        }
        Err(e) => {
            let status = match &e {
                MakeError::Taken(_) => StatusCode::UNPROCESSABLE_ENTITY,
                MakeError::Failed(e) => {
                    warn!(suite = %name, error = format!("{e:#}"), "making a suite failed");
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            };
            refuse(status, suite_not_made(&e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmarks::Suite;

    #[track_caller]
    fn listed(name: &str, stage: ReplayStage) -> SuiteListed {
        SuiteListed {
            suite: Suite {
                name: name.parse().unwrap(),
                stage,
            },
            cases: 0,
        }
    }

    /// The names of the suites the filters `name` and `stage` keep, of
    /// `board` (implement), `triage-boards` (triage) and `lanes` (triage).
    #[track_caller]
    fn kept(name: &str, stage: &str) -> Vec<&'static str> {
        let narrowing = Narrowing::of(&FilterQuery {
            name: name.into(),
            stage: stage.into(),
        });
        let narrowed = narrowing.narrowed.unwrap();
        [
            ("board", ReplayStage::Implement),
            ("triage-boards", ReplayStage::Triage),
            ("lanes", ReplayStage::Triage),
        ]
        .into_iter()
        .filter(|(suite, of)| narrowed.keeps(&listed(suite, *of)))
        .map(|(suite, _)| suite)
        .collect()
    }

    /// `docs/specs/benchmarks.md`, Suites, Filters.
    #[test_log::test]
    fn the_name_filter_keeps_each_suite_whose_name_holds_the_text_whatever_its_case() {
        assert_eq!(kept("boa", ""), ["board", "triage-boards"]);
        assert_eq!(kept("BOA", ""), ["board", "triage-boards"]);
        assert_eq!(kept(" Ane ", ""), ["lanes"]);
        assert_eq!(kept("", ""), ["board", "triage-boards", "lanes"]);
        assert_eq!(kept("nothing", ""), Vec::<&str>::new());
    }

    /// `docs/specs/benchmarks.md`, Suites, Filters.
    #[test_log::test]
    fn the_stage_filter_keeps_the_suites_of_the_stage_and_narrows_with_the_name() {
        assert_eq!(kept("", "triage"), ["triage-boards", "lanes"]);
        assert_eq!(kept("boa", "triage"), ["triage-boards"]);
        assert_eq!(kept("", "design"), Vec::<&str>::new());
    }
}
