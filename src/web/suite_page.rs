//! The suite page (`docs/specs/benchmarks.md`, Suite and New case): suite
//! NAME's heading, then its experiments and its cases, each in a tab of its
//! own, the New case dialog that captures a run of the suite's stage into it,
//! and the form that launches an experiment on it.

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::capture::{CaseFields, NOT_POLLED, capture_run, polled};
use super::confirmation::{Confirmation, TurnDown};
use super::experiment_page::{shown_of, state_status};
use super::html::{self, Html};
use super::launch;
use super::making::{Answer, Making, case_not_made};
use super::shell::{Area, Frame, Trail, frame_regions};
use super::table::{Align, Column};
use super::write::{Refused, Write, alert};
use super::{
    ErrorPage, Missing, Page, events, experiment_path, nothing_there, render, run_path, suite_path,
    suites,
};
use crate::api::Sources;
use crate::benchmarks::results;
use crate::benchmarks::{
    CaseEntry, ExperimentRecord, ExperimentState, RemoveError, Suite, SuiteName, SuiteShown,
    dollars, without,
};
use crate::case::{self, CaseName, Refusal, ReplayStage, SuiteChoice};
use crate::domain::Timestamp;
use crate::phrase::Status;
use crate::runs::{Recorded, RunId};

/// A case's row.
struct CaseRow {
    name: CaseName,
    run: RunId,
    run_href: String,
    criteria: CriteriaShown,
    remove: Confirmation,
}

/// A case's criteria, as its row shows them.
enum CriteriaShown {
    Read {
        /// Their first line that holds more than white space.
        first_line: String,
        text: String,
    },
    /// Why they do not read.
    Unreadable(String),
}

impl CaseRow {
    /// Its row's id, keyed by the case's name.
    fn row_id(&self) -> String {
        format!("case-{}", self.name)
    }

    /// The id of the row of its whole criteria, which its row opens to.
    fn criteria_id(&self) -> String {
        format!("{}_criteria", self.row_id())
    }

    /// The row of `case`, of the suite at `path`, removed after a
    /// confirmation (`docs/specs/benchmarks.md`, Remove case).
    fn of(case: CaseEntry, path: &str) -> CaseRow {
        CaseRow {
            remove: Confirmation {
                id: format!("remove-{}", case.name),
                label: "Remove",
                question: format!("Remove the case {} and its directory?", case.name),
                turn_down: TurnDown::Cancel,
                action: format!("{path}/remove-case"),
                fields: vec![("case", case.name.to_string())],
                said: "suite-said",
            },
            run_href: run_path(&case.run),
            name: case.name,
            run: case.run,
            criteria: match case.criteria {
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

/// An experiment's row.
struct ExperimentRow {
    href: String,
    launched: Html,
    status: Status,
    state: &'static str,
    /// How many cases it replays.
    cases: usize,
    configurations: usize,
    /// Kept up by the page's script while it is open; `None` where its end
    /// was not kept.
    duration: Option<Html>,
    /// Of its replays and its judges, and how many of its replays have none.
    cost: String,
}

impl ExperimentRow {
    fn of(sources: &Sources, record: &ExperimentRecord, now: Timestamp) -> ExperimentRow {
        let (status, state) = state_status(&shown_of(sources, record));
        let running = (record.state == ExperimentState::Open).then_some(record.launched_at);
        let cost = results::results(record).cost;
        let mut cost_text = dollars(cost.total);
        without(&mut cost_text, cost.none, "a cost");
        ExperimentRow {
            href: experiment_path(record.id),
            launched: html::time(record.launched_at),
            status,
            state,
            cases: record.cases.len(),
            configurations: record.configurations.len(),
            duration: record
                .duration(now)
                .map(|duration| html::duration(None, duration.as_secs(), running)),
            cost: cost_text,
        }
    }
}

/// A column of the table of experiments, the first naming each row.
#[derive(Clone, Copy)]
enum ExperimentsColumn {
    Launched,
    State,
    Cases,
    Configurations,
    Duration,
    Cost,
}

impl ExperimentsColumn {
    const ALL: [ExperimentsColumn; 6] = [
        ExperimentsColumn::Launched,
        ExperimentsColumn::State,
        ExperimentsColumn::Cases,
        ExperimentsColumn::Configurations,
        ExperimentsColumn::Duration,
        ExperimentsColumn::Cost,
    ];

    /// Empty for the column that names each row, which has no head.
    fn head(self) -> &'static str {
        match self {
            ExperimentsColumn::Launched => "",
            ExperimentsColumn::State => "State",
            ExperimentsColumn::Cases => "Cases",
            ExperimentsColumn::Configurations => "Configurations",
            ExperimentsColumn::Duration => "Duration",
            ExperimentsColumn::Cost => "Cost",
        }
    }

    /// The class that aligns the column's head and cells as its kind does.
    fn align(self) -> &'static str {
        let column = match self {
            ExperimentsColumn::Duration => Column::Duration,
            ExperimentsColumn::Cost => Column::Money,
            ExperimentsColumn::Launched
            | ExperimentsColumn::State
            | ExperimentsColumn::Cases
            | ExperimentsColumn::Configurations => Column::Other,
        };
        Align::of(column).class()
    }
}

/// A column of the table of cases, the first naming each row.
#[derive(Clone, Copy)]
enum CasesColumn {
    Name,
    CapturedFrom,
    Remove,
}

impl CasesColumn {
    const ALL: [CasesColumn; 3] = [
        CasesColumn::Name,
        CasesColumn::CapturedFrom,
        CasesColumn::Remove,
    ];

    /// Empty for the column that names each row and for "Remove"'s, which
    /// have no head.
    fn head(self) -> &'static str {
        match self {
            CasesColumn::Name | CasesColumn::Remove => "",
            CasesColumn::CapturedFrom => "Captured from",
        }
    }

    /// The class that aligns the column's head and cells as its kind does.
    fn align(self) -> &'static str {
        let column = match self {
            CasesColumn::Name | CasesColumn::CapturedFrom | CasesColumn::Remove => Column::Other,
        };
        Align::of(column).class()
    }
}

/// The form that launches an experiment on the suite.
struct LaunchOffer {
    /// How many replays the form makes as it starts.
    count: String,
    /// The paths under `briefs/` of the files a configuration may give.
    paths: Vec<String>,
    /// Their texts as the form starts them, for its script.
    texts: Html,
    efforts: [&'static str; 5],
}

impl LaunchOffer {
    /// The form for a suite with `cases` cases.
    fn of(sources: &Sources, cases: usize) -> LaunchOffer {
        let texts = launch::starting_texts(sources);
        LaunchOffer {
            count: launch::replay_count(cases, 1, 1),
            paths: texts.keys().cloned().collect(),
            texts: launch::texts_json(&texts),
            efforts: launch::efforts(),
        }
    }
}

/// A suite as its page shows it.
struct Shown {
    suite: Suite,
    cases: Vec<CaseRow>,
    /// Latest first.
    experiments: Vec<ExperimentRow>,
}

/// A run New case offers to capture.
struct RunChoice {
    id: RunId,
    /// The name the dialog suggests for its case.
    name: CaseName,
}

#[derive(Template, WebTemplate)]
#[template(path = "suite.html")]
struct SuitePage {
    page: Page,
    frame: Frame,
    suite: Suite,
    path: String,
    cases: Vec<CaseRow>,
    experiments: Vec<ExperimentRow>,
    /// "New" on the experiments, which opens `launch`.
    new_experiment: Html,
    launch: LaunchOffer,
    /// The New case dialog's form.
    new_case: Html,
}

impl SuitePage {
    /// Whether the page opens on the tab of its cases, as it does while the
    /// suite has no experiments.
    fn opens_on_cases(&self) -> bool {
        self.experiments.is_empty()
    }
}

#[derive(Template)]
#[template(path = "suite_cases.html")]
struct CasesRegion<'a> {
    cases: &'a [CaseRow],
}

#[derive(Template)]
#[template(path = "suite_experiments.html")]
struct ExperimentsRegion<'a> {
    experiments: &'a [ExperimentRow],
}

/// "New" on the experiments of a suite with `cases` cases, if it has any,
/// and how many it has, for the count of the launch's replays.
#[derive(Template)]
#[template(path = "suite_new_experiment.html")]
struct NewExperimentRegion {
    cases: usize,
}

/// The New case dialog's fields (`docs/specs/benchmarks.md`, New case).
#[derive(Default)]
pub(super) struct NewCaseFields {
    /// `None` for the first run offered, as for a run that does not read.
    run: Option<RunId>,
    name: String,
    criteria: String,
}

impl NewCaseFields {
    /// The fields as they start: the latest run of `runs`, and the name
    /// suggested for it.
    fn starting(runs: &[RunChoice]) -> NewCaseFields {
        NewCaseFields {
            name: runs
                .first()
                .map(|run| run.name.to_string())
                .unwrap_or_default(),
            ..NewCaseFields::default()
        }
    }
}

/// The region of the New case dialog that holds the button "Add case".
const NEW_CASE_READY: &str = "new-case-ready";

/// What the New case dialog's form holds.
#[derive(Template)]
#[template(path = "new_case.html")]
pub(super) struct NewCaseForm {
    /// The suite's name, as its path gives it.
    suite: String,
    making: Making<NewCaseFields>,
    /// The runs of the suite's stage that can be captured, latest first.
    runs: Vec<RunChoice>,
    /// Whether `serve` has polled, so that a capture can run.
    polled: bool,
}

impl NewCaseForm {
    /// The form as it starts, for suite `suite` of `stage`.
    async fn blank(sources: &Sources, suite: &SuiteName, stage: ReplayStage) -> NewCaseForm {
        let runs = runs_of(sources, stage).await;
        NewCaseForm {
            suite: suite.to_string(),
            making: Making {
                fields: NewCaseFields::starting(&runs),
                refusal: None,
            },
            runs,
            polled: polled(sources),
        }
    }

    fn title(&self) -> String {
        format!("New case in {}", self.suite)
    }

    /// The choice of a run, the region of the page's stream that keeps it.
    fn runs(&self) -> Html {
        html::rendered(&NewCaseRuns {
            runs: &self.runs,
            chosen: self.making.fields.run.as_ref(),
        })
    }

    fn ready(&self) -> NewCaseReady {
        NewCaseReady::of(&self.runs, self.polled)
    }

    /// It refused for `why`, answered with `status`.
    fn refused(mut self, (status, why): (StatusCode, String)) -> Answer<NewCaseForm> {
        self.making.refusal = Some(why);
        Answer::Refused { status, form: self }
    }
}

/// The New case dialog's choice of a run, `chosen` chosen, or else the
/// first; or "No runs".
#[derive(Template)]
#[template(path = "new_case_runs.html")]
struct NewCaseRuns<'a> {
    runs: &'a [RunChoice],
    chosen: Option<&'a RunId>,
}

impl NewCaseRuns<'_> {
    fn chosen(&self, id: &RunId) -> bool {
        self.chosen == Some(id)
    }
}

/// The region of the New case dialog that holds "Add case": disabled with
/// no runs to capture or before `serve` has polled, after `note`, why it
/// cannot run yet.
#[derive(Template)]
#[template(
    source = r#"{%- import "macros.html" as ui -%}{% call ui::new_submit("Add case", NEW_CASE_READY, disabled, note) %}{% endcall %}"#,
    ext = "html"
)]
struct NewCaseReady {
    disabled: bool,
    note: &'static str,
}

impl NewCaseReady {
    fn of(runs: &[RunChoice], polled: bool) -> NewCaseReady {
        NewCaseReady {
            disabled: runs.is_empty() || !polled,
            note: if polled { "" } else { NOT_POLLED },
        }
    }
}

/// Why there is no suite page: no such suite, or one that does not read.
enum Unshown {
    NoSuch,
    Failed(anyhow::Error),
}

/// What is said of a suite named `name` that there is none of.
fn no_such(name: &str) -> String {
    format!("there is no suite {name}")
}

/// Suite `name` with its cases, in its order, and its experiments, latest
/// first.
async fn shown(sources: &Sources, name: &str) -> Result<Shown, Unshown> {
    let parsed: SuiteName = name.parse().map_err(|_| Unshown::NoSuch)?;
    match suites(sources).show(&parsed).await {
        Ok(Some(SuiteShown {
            suite,
            cases,
            experiments,
        })) => Ok(Shown {
            cases: {
                let path = suite_path(&suite.name);
                cases
                    .into_iter()
                    .map(|case| CaseRow::of(case, &path))
                    .collect()
            },
            suite,
            experiments: {
                let now = sources.clock.now();
                experiments
                    .iter()
                    .map(|record| ExperimentRow::of(sources, record, now))
                    .collect()
            },
        }),
        Ok(None) => Err(Unshown::NoSuch),
        Err(e) => Err(Unshown::Failed(e)),
    }
}

/// Of `recorded`, the runs of `stage`, latest first.
fn of_stage(recorded: Vec<Recorded>, stage: ReplayStage) -> Vec<RunId> {
    let mut runs: Vec<Recorded> = recorded
        .into_iter()
        .filter(|run| case::capturable_stage(&run.id.of).is_some_and(|(of, _)| of == stage))
        .collect();
    runs.sort_by_key(|run| std::cmp::Reverse(run.started_at));
    runs.into_iter().map(|run| run.id).collect()
}

/// The runs of `stage` that can be captured, latest first. Runs that do not
/// read are logged and left out.
async fn runs_of(sources: &Sources, stage: ReplayStage) -> Vec<RunChoice> {
    let recorded = match sources.runs.read().await {
        Ok(recorded) => recorded,
        Err(e) => {
            warn!(error = format!("{e:#}"), "reading the runs failed");
            return vec![];
        }
    };
    let mut runs = vec![];
    for id in of_stage(recorded, stage) {
        match case::capturable_run(&sources.runs, &id).await {
            Ok(_) => runs.push(RunChoice {
                name: CaseName::suggested(&id),
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

pub(super) async fn suite_page(
    State(sources): State<Sources>,
    Path(name): Path<String>,
) -> Response {
    let trail = Trail::within(Area::Suites).here(name.as_str());
    let frame = Frame::of(&sources).await;
    match shown(&sources, &name).await {
        Ok(Shown {
            suite,
            cases,
            experiments,
        }) => {
            let path = suite_path(&suite.name);
            let new_case = NewCaseForm::blank(&sources, &suite.name, suite.stage).await;
            SuitePage {
                page: Page::new(trail, format!("/events{path}")),
                frame,
                launch: LaunchOffer::of(&sources, cases.len()),
                new_experiment: html::rendered(&NewExperimentRegion { cases: cases.len() }),
                new_case: html::rendered(&new_case),
                path,
                suite,
                cases,
                experiments,
            }
            .into_response()
        }
        Err(unshown) => {
            let page = Page::new(trail, format!("/events/suites/{name}"));
            match unshown {
                Unshown::NoSuch => nothing_there(page, frame, "cases", Missing::Suite(name)),
                Unshown::Failed(e) => {
                    let error = format!("{e:#}");
                    warn!(suite = name, error, "reading a suite failed");
                    let page = ErrorPage {
                        page,
                        frame,
                        region: "cases",
                        error,
                    };
                    (StatusCode::INTERNAL_SERVER_ERROR, page).into_response()
                }
            }
        }
    }
}

/// The suite page's regions: the frame's, and once the suite reads, its
/// experiments, whether it offers to launch one, its cases, and the runs
/// New case offers and whether it can add one.
pub(super) async fn suite_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Path(name): Path<String>,
) -> Response {
    let regions = move || {
        let sources = sources.clone();
        let name = name.clone();
        async move {
            let mut regions = frame_regions(&Frame::of(&sources).await);
            if let Ok(Shown {
                suite,
                cases,
                experiments,
            }) = shown(&sources, &name).await
            {
                regions.push((
                    "experiments",
                    render(&ExperimentsRegion {
                        experiments: &experiments,
                    }),
                ));
                regions.push((
                    "new-experiment-offer",
                    render(&NewExperimentRegion { cases: cases.len() }),
                ));
                regions.push(("cases", render(&CasesRegion { cases: &cases })));
                let runs = runs_of(&sources, suite.stage).await;
                regions.push((
                    "new-case-run",
                    render(&NewCaseRuns {
                        runs: &runs,
                        chosen: None,
                    }),
                ));
                let ready = NewCaseReady::of(&runs, polled(&sources));
                regions.push((NEW_CASE_READY, render(&ready)));
            }
            regions
        }
    };
    events(regions, stopping)
}

/// Suite `name` of a write's path, or else what the page says of there
/// being none.
fn suite_of_path(write: Write, name: &str) -> Result<SuiteName, Refused> {
    name.parse()
        .map_err(|_| write.refused(StatusCode::NOT_FOUND, &no_such(name)))
}

/// What the New case dialog's form sends, a field it lacks being empty.
#[derive(Deserialize)]
struct AddForm {
    #[serde(default)]
    run: String,
    #[serde(flatten)]
    case: CaseFields,
}

/// The stage of suite `name`, or else the status and why there is no
/// suite to add a case to.
async fn stage_of(
    sources: &Sources,
    name: &str,
) -> Result<(SuiteName, ReplayStage), (StatusCode, String)> {
    let suite: SuiteName = name
        .parse()
        .map_err(|_| (StatusCode::NOT_FOUND, no_such(name)))?;
    match suites(sources).show(&suite).await {
        Ok(Some(shown)) => Ok((suite, shown.suite.stage)),
        Ok(None) => Err((StatusCode::NOT_FOUND, no_such(name))),
        Err(e) => {
            let why = format!("{e:#}");
            warn!(suite = %suite, error = why, "reading a suite failed");
            Err((StatusCode::INTERNAL_SERVER_ERROR, why))
        }
    }
}

/// Captures the run the New case dialog's form in `body` names into suite
/// `name`, and answers with the dialog's form: as it starts once made, or as
/// sent with why it was refused. Refused, writing nothing, unless the browser
/// shows the request came from the web UI's own pages.
pub(super) async fn add_case(
    State(sources): State<Sources>,
    Path(name): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Answer<NewCaseForm> {
    const WRITE: Write = Write::Capture;
    let sent = WRITE.read_form::<AddForm>(&body);
    let fields = sent
        .as_ref()
        .map(|sent| NewCaseFields {
            run: sent.run.parse().ok(),
            name: sent.case.name.clone(),
            criteria: sent.case.criteria.clone(),
        })
        .unwrap_or_default();
    let mut form = NewCaseForm {
        suite: name,
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
    let (suite, stage) = match stage_of(&sources, &form.suite).await {
        Ok(of) => of,
        Err(refused) => return form.refused(refused),
    };
    form.runs = runs_of(&sources, stage).await;
    let run: RunId = match sent.run.parse() {
        Ok(run) => run,
        Err(why) => return form.refused((StatusCode::UNPROCESSABLE_ENTITY, why)),
    };
    match capture_run(&sources, &run, &SuiteChoice::Named(suite), &sent.case).await {
        Ok(_) => {
            form.making = Making {
                fields: NewCaseFields::starting(&form.runs),
                refusal: None,
            };
            Answer::Made { blank: form }
        }
        Err(not) => form.refused((not.status(), case_not_made(&not))),
    }
}

/// What the form that removes a case sends.
#[derive(Deserialize)]
struct RemoveForm {
    case: String,
}

/// Removes the case the form in `body` names from suite `name`, and answers
/// with what the page says of it. Refused, removing nothing, unless the
/// browser shows the request came from the web UI's own pages.
pub(super) async fn remove_case(
    State(sources): State<Sources>,
    Path(name): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Refused> {
    const WRITE: Write = Write::RemoveCase;
    WRITE.same_origin(&headers, &uri)?;
    let suite = suite_of_path(WRITE, &name)?;
    let form: RemoveForm = WRITE.read_form(&body)?;
    let case: CaseName = form
        .case
        .parse()
        .map_err(|error: String| WRITE.refused(StatusCode::UNPROCESSABLE_ENTITY, &error))?;
    match suites(&sources).remove_case(&suite, &case).await {
        Ok(()) => {
            info!(suite = %suite, case = %case, "removed a case");
            let said = format!("Removed the case {}", html::escape(case.as_str()));
            Ok((StatusCode::OK, alert("alert-success", &said)).into_response())
        }
        Err(e @ (RemoveError::NoSuchSuite(_) | RemoveError::NoSuchCase { .. })) => {
            Err(WRITE.refused(StatusCode::NOT_FOUND, &e.to_string()))
        }
        Err(RemoveError::Failed(e)) => {
            let why = format!("{e:#}");
            warn!(suite = %suite, case = %case, error = why, "removing a case failed");
            Err(WRITE.refused(StatusCode::INTERNAL_SERVER_ERROR, &why))
        }
    }
}
