//! The experiment page (`docs/specs/design_system.md`, The experiment
//! page): experiment ID's heading, then its results as a chart, a grid and
//! a table whose replays open to the transcripts of their runs and their
//! judges; and the cancelling of an open experiment.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path as FilePath, PathBuf};
use std::sync::{Arc, Mutex};

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::html::{self, Html};
use super::launch::counted;
use super::names::Names;
use super::run_page::{RunQuery, Transcribed, transcript_events, transcript_page};
use super::write::{Refused, Write, alert};
use super::{
    ErrorPage, Frame, Page, events, experiment_path, experiments, frame_regions, render, suite_path,
};
use crate::api::Sources;
use crate::benchmarks::results::{self, CasePasses, ConfigurationResults, Results};
use crate::benchmarks::{
    CancelRefusal, ConfigurationName, ExperimentId, ExperimentRecord, ExperimentState, NoJudgement,
    ReplayFigures, Shown, TrialEnd, TrialKey, TrialState, dollars, duration, mean, percent,
    without,
};
use crate::case::CaseName;
use crate::logs::{KeptRuns, Which};
use crate::phrase::Tone;
use crate::replay::{self, judge};
use crate::results::{Leak, Verdict};
use crate::runs::RunId;
use crate::runs::files::RunFiles;

/// The tone and the word an experiment's state shows with: that of the
/// status with the same meaning (`docs/specs/design_system.md`, The
/// experiment page, State).
pub(super) fn state_status(shown: &Shown) -> (Tone, &'static str) {
    match shown {
        Shown::Running => (Tone::InProgress, "running"),
        Shown::Waiting(_) => (Tone::Paused, "waiting"),
        Shown::Ended => (Tone::Success, "ended"),
        Shown::Cancelled => (Tone::Cancelled, "cancelled"),
    }
}

/// How experiment `record` shows: an open one as `serve` last showed it.
pub(super) fn shown_of(sources: &Sources, record: &ExperimentRecord) -> Shown {
    let served = sources
        .experiments
        .shown()
        .experiments
        .get(&record.id)
        .cloned();
    Shown::of(record.state, served)
}

/// What the replays' outputs hold that the page shows, by output, as read
/// once it holds it: that an output has begun, and the judges' reasoning
/// of a judged replay, whose judgement is never replaced.
#[derive(Clone, Default)]
struct Outputs(Arc<Mutex<HashMap<PathBuf, OutputRead>>>);

#[derive(Clone, Default)]
struct OutputRead {
    began: bool,
    judgement: Judgement,
}

/// What an output's judgement gave the page.
#[derive(Clone, Default)]
enum Judgement {
    #[default]
    Unread,
    /// The judges' reasoning, or none where the output holds no judgement
    /// that reads.
    Read(Option<Arc<Reasoning>>),
}

/// What a judge said of a replay.
struct Reasoning {
    correctness: String,
    leak: String,
}

impl Outputs {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, OutputRead>> {
        self.0.lock().expect("the outputs' lock is not poisoned")
    }

    /// Whether the output in `dir` has begun.
    fn began(&self, dir: &FilePath) -> bool {
        let mut read = self.lock();
        let output = read.entry(dir.to_path_buf()).or_default();
        if !output.began {
            output.began = replay::began(dir);
        }
        output.began
    }

    /// The reasoning of the judgement the output in `dir` of a judged
    /// replay holds, if it holds one that reads; one that does not read is
    /// logged.
    fn reasoning(&self, dir: &FilePath) -> Option<Arc<Reasoning>> {
        let mut read = self.lock();
        let output = read.entry(dir.to_path_buf()).or_default();
        if let Judgement::Read(reasoning) = &output.judgement {
            return reasoning.clone();
        }
        let reasoning = match judge::read(dir) {
            Ok(kept) => kept.map(|kept| {
                Arc::new(Reasoning {
                    correctness: kept.correctness.reasoning,
                    leak: match kept.leak {
                        Leak::None { reasoning } | Leak::Found { reasoning, .. } => reasoning,
                    },
                })
            }),
            Err(e) => {
                warn!(output = %dir.display(), error = format!("{e:#}"), "reading a judgement failed");
                None
            }
        };
        output.judgement = Judgement::Read(reasoning.clone());
        reasoning
    }
}

/// The experiment as the page shows it.
struct ExperimentView {
    id: ExperimentId,
    suite: String,
    suite_href: String,
    launched: Html,
    tone: Tone,
    state: &'static str,
    /// What a waiting experiment waits for.
    waiting: Option<Html>,
    ended: usize,
    all: usize,
    /// Whether it is open, and so offers to be cancelled.
    open: bool,
    path: String,
    chart: Chart,
    grid: Grid,
    configurations: Vec<ConfigurationView>,
    /// What the judges cost.
    judges: String,
}

/// The chart: a point for each configuration at its mean cost and its pass
/// rate (`docs/specs/design_system.md`, The experiment page, The chart).
struct Chart {
    svg: Html,
    /// The configurations it has no point for, lacking either figure.
    unplotted: Vec<ConfigurationName>,
}

struct Grid {
    configurations: Vec<ConfigurationName>,
    rows: Vec<GridRow>,
}

struct GridRow {
    case: CaseName,
    /// For each configuration, its passes, then its leaks and its replays
    /// without a judgement, if it has any.
    cells: Vec<String>,
}

struct ConfigurationView {
    name: ConfigurationName,
    pass_rate: String,
    cost: String,
    time: String,
    leaks: usize,
    unjudged: usize,
    cases: Vec<CaseView>,
}

struct CaseView {
    name: CaseName,
    passes: String,
    replays: Vec<ReplayView>,
}

struct ReplayView {
    repetition: u32,
    tone: Tone,
    state: &'static str,
    /// Its judgement and its figures, as they stand.
    said: String,
    reasoning: Option<Arc<Reasoning>>,
    run_href: Option<String>,
    judge_href: Option<String>,
}

/// The path of the page of the transcript `of` of the replay of `case`
/// under `configuration` in `repetition` of experiment `id`.
fn transcript_path(
    id: ExperimentId,
    configuration: &ConfigurationName,
    case: &CaseName,
    repetition: u32,
    of: Of,
) -> String {
    format!(
        "{}/{configuration}/{case}/{repetition}/{}",
        experiment_path(id),
        of.name()
    )
}

impl ExperimentView {
    fn of(
        sources: &Sources,
        record: &ExperimentRecord,
        shown: &Shown,
        outputs: &Outputs,
    ) -> ExperimentView {
        let names = Names::of(&sources.work_dir.repo, None);
        let results = results::results(record);
        let (tone, state) = state_status(shown);
        let path = experiment_path(record.id);
        ExperimentView {
            id: record.id,
            suite: record.suite.name.to_string(),
            suite_href: suite_path(&record.suite.name),
            launched: html::time(record.launched_at),
            tone,
            state,
            waiting: matches!(shown, Shown::Waiting(_))
                .then(|| html::phrase(&shown.phrase(), &names)),
            ended: results.ended,
            all: results.all,
            open: record.state == ExperimentState::Open,
            path,
            chart: chart(&results),
            grid: grid(record, &results),
            configurations: configurations(sources, record, &results, outputs),
            judges: {
                let mut judges = format!("The judges cost {}", dollars(results.judges.total));
                without(&mut judges, results.judges.none, "a cost");
                judges.push('.');
                judges
            },
        }
    }
}

const CHART_WIDTH: f64 = 480.0;
const CHART_HEIGHT: f64 = 260.0;
const CHART_LEFT: f64 = 56.0;
const CHART_RIGHT: f64 = 32.0;
const CHART_TOP: f64 = 20.0;
const CHART_BOTTOM: f64 = 44.0;

/// The chart of `results`, drawn as inline SVG.
fn chart(results: &Results) -> Chart {
    let (plotted, unplotted): (Vec<_>, Vec<_>) = results
        .configurations
        .iter()
        .map(|configuration| {
            (
                configuration,
                configuration.cost.mean.zip(configuration.pass_rate),
            )
        })
        .partition(|(_, at)| at.is_some());
    let most = plotted
        .iter()
        .filter_map(|(_, at)| at.map(|(cost, _)| cost))
        .fold(0.0_f64, f64::max);
    let scale = if most > 0.0 { most * 1.1 } else { 1.0 };
    let width = CHART_WIDTH - CHART_LEFT - CHART_RIGHT;
    let height = CHART_HEIGHT - CHART_TOP - CHART_BOTTOM;
    let (left, right) = (CHART_LEFT, CHART_LEFT + width);
    let (top, bottom) = (CHART_TOP, CHART_TOP + height);
    let mut svg = format!(
        r#"<svg class="chart w-full max-w-xl" viewBox="0 0 {CHART_WIDTH} {CHART_HEIGHT}" role="img" aria-label="mean cost of a replay against pass rate">"#
    );
    let _ = write!(
        svg,
        r#"<g class="axes stroke-current text-muted" fill="none"><path d="M{left} {top}V{bottom}H{right}"/></g><g class="ticks fill-current text-muted text-xs"><text x="{}" y="{}" text-anchor="end">100%</text><text x="{}" y="{}" text-anchor="end">0%</text><text x="{left}" y="{}" text-anchor="middle">$0.00</text><text x="{right}" y="{}" text-anchor="middle">{}</text><text x="{}" y="{}" text-anchor="middle">mean cost of a replay</text><text x="{}" y="{}" text-anchor="middle" transform="rotate(-90 {} {})">pass rate</text></g>"#,
        left - 6.0,
        top + 4.0,
        left - 6.0,
        bottom,
        bottom + 16.0,
        bottom + 16.0,
        html::escape(&dollars(scale)),
        left + width / 2.0,
        bottom + 34.0,
        left - 40.0,
        top + height / 2.0,
        left - 40.0,
        top + height / 2.0,
    );
    for (configuration, at) in &plotted {
        let Some((cost, rate)) = at else { continue };
        let x = left + cost / scale * width;
        let y = top + (1.0 - rate) * height;
        let name = html::escape(configuration.name.as_str());
        let _ = write!(
            svg,
            r#"<g class="point text-info"><circle cx="{x:.1}" cy="{y:.1}" r="5" class="fill-current"><title>{name}: pass rate {}, mean cost {}</title></circle><text x="{:.1}" y="{:.1}" class="fill-base-content text-xs">{name}</text></g>"#,
            percent(Some(*rate)),
            html::escape(&dollars(*cost)),
            x + 8.0,
            y - 6.0,
        );
    }
    svg.push_str("</svg>");
    Chart {
        svg: html::built(svg),
        unplotted: unplotted
            .into_iter()
            .map(|(configuration, _)| configuration.name.clone())
            .collect(),
    }
}

/// The grid of `record`: a row for each case, a cell for each
/// configuration.
fn grid(record: &ExperimentRecord, results: &Results) -> Grid {
    Grid {
        configurations: record.configurations.clone(),
        rows: record
            .cases
            .iter()
            .enumerate()
            .map(|(i, case)| GridRow {
                case: case.clone(),
                cells: results
                    .configurations
                    .iter()
                    .map(|configuration| {
                        configuration
                            .passes
                            .get(i)
                            .map_or_else(String::new, |passes| {
                                let mut cell = passes_of(passes);
                                if passes.leaks > 0 {
                                    let _ = write!(
                                        cell,
                                        ", {}",
                                        counted(passes.leaks, "leak", "leaks")
                                    );
                                }
                                if passes.unjudged > 0 {
                                    let _ = write!(cell, ", {} unjudged", passes.unjudged);
                                }
                                cell
                            })
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// How many of a case's replays pass, of those with a judgement and no
/// leak, as "2 of 3".
fn passes_of(passes: &CasePasses) -> String {
    format!("{} of {}", passes.pass, passes.of)
}

/// The table's rows of `record`: each configuration's figures, its cases
/// and their replays.
fn configurations(
    sources: &Sources,
    record: &ExperimentRecord,
    results: &Results,
    outputs: &Outputs,
) -> Vec<ConfigurationView> {
    results
        .configurations
        .iter()
        .enumerate()
        .map(|(c, figures)| {
            let ConfigurationResults {
                name,
                pass_rate,
                cost,
                time,
                leaks,
                unjudged,
                passes,
            } = figures;
            ConfigurationView {
                name: name.clone(),
                pass_rate: percent(*pass_rate),
                cost: mean(cost, dollars, "a cost"),
                time: mean(time, duration, "a time"),
                leaks: *leaks,
                unjudged: *unjudged,
                cases: record
                    .cases
                    .iter()
                    .enumerate()
                    .map(|(k, case)| CaseView {
                        name: case.clone(),
                        passes: passes.get(k).map_or_else(String::new, passes_of),
                        replays: record
                            .trials
                            .iter()
                            .filter(|(key, _)| key.case == k && key.configuration == c)
                            .map(|(key, state)| {
                                replay_view(sources, record, name, case, key, state, outputs)
                            })
                            .collect(),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// A replay as the table shows it.
fn replay_view(
    sources: &Sources,
    record: &ExperimentRecord,
    configuration: &ConfigurationName,
    case: &CaseName,
    key: &TrialKey,
    state: &TrialState,
    outputs: &Outputs,
) -> ReplayView {
    let repetition = key.repetition.get();
    let path = |of| transcript_path(record.id, configuration, case, repetition, of);
    let (tone, word, judgement, replay) = match state {
        TrialState::Pending => (Tone::Pending, "pending", None, None),
        TrialState::Replaying => (Tone::InProgress, "replaying", None, None),
        TrialState::Replayed { replay } => (Tone::Paused, "replayed", None, Some(replay)),
        TrialState::Judging { replay } => (Tone::InProgress, "judging", None, Some(replay)),
        TrialState::Ended(TrialEnd::Judged { replay, judgement }) => {
            let leak = if judgement.leak { ", with a leak" } else { "" };
            let tone = if judgement.correctness == Verdict::Pass && !judgement.leak {
                Tone::Success
            } else {
                Tone::Failure
            };
            (
                tone,
                "judged",
                Some(format!("{}{leak}", judgement.correctness.name())),
                Some(replay),
            )
        }
        TrialState::Ended(TrialEnd::Unjudged { replay, why }) => {
            let why = match why {
                NoJudgement::Outcome => "its run is not done or handed back".to_string(),
                NoJudgement::Judge { reason, .. } => format!("the judge made none: {reason}"),
                NoJudgement::Cancelled => "cancelled".into(),
            };
            (
                Tone::Cancelled,
                "unjudged",
                Some(format!("no judgement: {why}")),
                Some(replay),
            )
        }
        TrialState::Ended(TrialEnd::Cancelled) => (
            Tone::Cancelled,
            "unjudged",
            Some("no judgement: cancelled".into()),
            None,
        ),
    };
    let output = experiments(sources).output_dir(record.id, configuration, case, key.repetition);
    let began = outputs.began(&output);
    let reasoning = matches!(state, TrialState::Ended(TrialEnd::Judged { .. }))
        .then(|| outputs.reasoning(&output))
        .flatten();
    let judged = reasoning.is_some() || matches!(state, TrialState::Judging { .. });
    ReplayView {
        repetition,
        tone,
        state: word,
        said: said(judgement, replay),
        reasoning,
        run_href: began.then(|| path(Of::Run)),
        judge_href: judged.then(|| path(Of::Judge)),
    }
}

/// A replay's judgement, if it has one or the reason it has none, then
/// its cost and its time, if it has them.
fn said(judgement: Option<String>, replay: Option<&ReplayFigures>) -> String {
    let mut said: Vec<String> = judgement.into_iter().collect();
    if let Some(replay) = replay {
        said.push(replay.money.map_or_else(
            || "no cost".into(),
            |money| format!("cost {}", dollars(money.into())),
        ));
        said.push(format!("took {}", duration(replay.time.as_secs_f64())));
    }
    said.join(", ")
}

#[derive(Template, WebTemplate)]
#[template(path = "experiment.html")]
struct ExperimentPage {
    page: Page,
    frame: Frame,
    experiment: ExperimentView,
}

#[derive(Template)]
#[template(path = "experiment.html", block = "experiment")]
struct ExperimentRegion<'a> {
    experiment: &'a ExperimentView,
}

/// Why there is no experiment page: no such experiment, or one that does
/// not read.
enum Unshown {
    NoSuch(String),
    Failed(anyhow::Error),
}

/// What is said of an experiment `id` that there is none of.
fn no_such(id: &str) -> String {
    format!("there is no experiment {id}")
}

/// Experiment `id`, by its ID as a path gives it.
async fn record_of(sources: &Sources, id: &str) -> Result<ExperimentRecord, Unshown> {
    let parsed: ExperimentId = id.parse().map_err(|_| Unshown::NoSuch(no_such(id)))?;
    match experiments(sources).show(parsed).await {
        Ok(Some(record)) => Ok(record),
        Ok(None) => Err(Unshown::NoSuch(no_such(id))),
        Err(e) => Err(Unshown::Failed(e)),
    }
}

impl Unshown {
    /// The status the page answers with and what it says, logging a
    /// failure to read experiment `id`.
    fn answer(self, id: &str) -> (StatusCode, String) {
        match self {
            Unshown::NoSuch(error) => (StatusCode::NOT_FOUND, error),
            Unshown::Failed(e) => {
                let error = format!("{e:#}");
                warn!(experiment = id, error, "reading an experiment failed");
                (StatusCode::INTERNAL_SERVER_ERROR, error)
            }
        }
    }
}

pub(super) async fn experiment_page(
    State(sources): State<Sources>,
    Path(id): Path<String>,
) -> Response {
    let title = format!("Experiment {id} · {}", sources.work_dir.repo);
    let frame = Frame::of(&sources).await;
    match record_of(&sources, &id).await {
        Ok(record) => {
            let shown = shown_of(&sources, &record);
            let experiment = ExperimentView::of(&sources, &record, &shown, &Outputs::default());
            ExperimentPage {
                page: Page::new(title, format!("/events{}", experiment.path)),
                frame,
                experiment,
            }
            .into_response()
        }
        Err(unshown) => {
            let (status, error) = unshown.answer(&id);
            let page = ErrorPage {
                page: Page::new(title, format!("/events/experiments/{id}")),
                frame,
                region: "experiment",
                error,
            };
            (status, page).into_response()
        }
    }
}

/// The experiment page's regions: the frame's, and the experiment once it
/// reads.
pub(super) async fn experiment_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Path(id): Path<String>,
) -> Response {
    let outputs = Outputs::default();
    let regions = move || {
        let (sources, id, outputs) = (sources.clone(), id.clone(), outputs.clone());
        async move {
            let mut regions = frame_regions(&Frame::of(&sources).await);
            if let Ok(record) = record_of(&sources, &id).await {
                let shown = shown_of(&sources, &record);
                let experiment = ExperimentView::of(&sources, &record, &shown, &outputs);
                regions.push((
                    "experiment",
                    render(&ExperimentRegion {
                        experiment: &experiment,
                    }),
                ));
            }
            regions
        }
    };
    events(regions, stopping)
}

/// Cancels experiment `id`, tells `serve` so, and answers with what the
/// page says of it. Refused, cancelling nothing, unless the browser shows
/// the request came from the web UI's own pages.
pub(super) async fn cancel(
    State(sources): State<Sources>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, Refused> {
    const WRITE: Write = Write::Cancel;
    WRITE.same_origin(&headers, &uri)?;
    let parsed: ExperimentId = id
        .parse()
        .map_err(|_| WRITE.refused(StatusCode::NOT_FOUND, &no_such(&id)))?;
    match experiments(&sources).cancel(parsed).await {
        Ok(()) => {
            sources.experiments.changed();
            info!(experiment = %parsed, "cancelled an experiment");
            let said = format!("Cancelled experiment {parsed}");
            Ok((StatusCode::OK, alert("alert-success", &said)).into_response())
        }
        Err(e @ CancelRefusal::NoSuchExperiment(_)) => {
            Err(WRITE.refused(StatusCode::NOT_FOUND, &e.to_string()))
        }
        Err(e @ CancelRefusal::NotOpen { .. }) => {
            Err(WRITE.refused(StatusCode::UNPROCESSABLE_ENTITY, &e.to_string()))
        }
        Err(CancelRefusal::Failed(e)) => {
            let why = format!("{e:#}");
            warn!(experiment = %parsed, error = why, "cancelling an experiment failed");
            Err(WRITE.refused(StatusCode::INTERNAL_SERVER_ERROR, &why))
        }
    }
}

/// A replay's transcript, as its path names it.
#[derive(serde::Deserialize)]
pub(super) struct ReplayPath {
    id: String,
    configuration: String,
    case: String,
    repetition: String,
    of: String,
}

/// Which of a replay's transcripts a page shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Of {
    Run,
    Judge,
}

impl Of {
    const ALL: [Of; 2] = [Of::Run, Of::Judge];

    /// Its name in a path.
    fn name(self) -> &'static str {
        match self {
            Of::Run => "run",
            Of::Judge => "judge",
        }
    }
}

/// A replay of an experiment whose transcript a page shows.
struct ReplayAt {
    id: ExperimentId,
    configuration: ConfigurationName,
    case: CaseName,
    repetition: u32,
    of: Of,
}

impl ReplayAt {
    /// The replay `path` names: its name in the experiment.
    fn name(&self) -> String {
        format!("{}/{}/{}", self.configuration, self.case, self.repetition)
    }

    fn path(&self, of: Of) -> String {
        transcript_path(
            self.id,
            &self.configuration,
            &self.case,
            self.repetition,
            of,
        )
    }
}

/// The replay `path` names, and where its transcript is read from; or
/// else the status and what the page says of there being none.
async fn transcribed(
    sources: &Sources,
    path: &ReplayPath,
) -> Result<(ReplayAt, Transcribed), (StatusCode, String)> {
    let replay = format!("{}/{}/{}", path.configuration, path.case, path.repetition);
    let none = || {
        (
            StatusCode::NOT_FOUND,
            format!("experiment {} has no replay {replay}", path.id),
        )
    };
    let record = record_of(sources, &path.id)
        .await
        .map_err(|unshown| unshown.answer(&path.id))?;
    let configuration = record
        .configurations
        .iter()
        .position(|name| name.as_str() == path.configuration)
        .ok_or_else(none)?;
    let case = record
        .cases
        .iter()
        .position(|name| name.as_str() == path.case)
        .ok_or_else(none)?;
    let repetition = path
        .repetition
        .parse()
        .ok()
        .filter(|repetition: &std::num::NonZeroU32| *repetition <= record.repetitions)
        .ok_or_else(none)?;
    let of = Of::ALL
        .into_iter()
        .find(|of| of.name() == path.of)
        .ok_or_else(none)?;
    let at = ReplayAt {
        id: record.id,
        configuration: record.configurations[configuration].clone(),
        case: record.cases[case].clone(),
        repetition: repetition.get(),
        of,
    };
    let output = experiments(sources).output_dir(at.id, &at.configuration, &at.case, repetition);
    let unread = |e: anyhow::Error| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
    let case_run: RunId = replay::case_run(&output).map_err(unread)?.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("the replay {replay} of experiment {} has not begun", at.id),
        )
    })?;
    let item = match &case_run.of {
        crate::runs::RunOf::Work { item, .. } => *item,
        crate::runs::RunOf::Automation { .. } => return Err(none()),
    };
    let transcribed = match of {
        Of::Run => Transcribed {
            kept: KeptRuns::Files(RunFiles::new(&output)),
            which: Which::Item(item),
            name: None,
            of: html::escape(&format!("the run of replay {}", at.name())),
        },
        Of::Judge => Transcribed {
            kept: KeptRuns::Judge(output),
            which: Which::Run {
                id: case_run,
                item: None,
            },
            name: Some("judge".into()),
            of: html::escape(&format!("the judge of replay {}", at.name())),
        },
    };
    Ok((at, transcribed))
}

/// What the page of a replay's transcript says above it: which replay of
/// which experiment, linking to the experiment's page and to its other
/// transcript.
fn context(at: &ReplayAt) -> Html {
    let (this, other, other_name) = match at.of {
        Of::Run => ("run's", Of::Judge, "judge's"),
        Of::Judge => ("judge's", Of::Run, "run's"),
    };
    html::built(format!(
        r#"<p id="replay" class="mb-3">Replay {} of <a class="link" href="{}">experiment {}</a>: its {this} transcript, or its <a class="link" href="{}">{other_name}</a></p>"#,
        html::escape(&at.name()),
        html::escape(&experiment_path(at.id)),
        at.id,
        html::escape(&at.path(other)),
    ))
}

/// The page of a replay's run's or judge's transcript, shown as the run
/// page shows a run's.
pub(super) async fn replay_page(
    State(sources): State<Sources>,
    Path(path): Path<ReplayPath>,
    Query(query): Query<RunQuery>,
    uri: Uri,
) -> Response {
    let title = format!(
        "Replay {}/{}/{} of experiment {} · {}",
        path.configuration, path.case, path.repetition, path.id, sources.work_dir.repo
    );
    let (context, transcribed) = match transcribed(&sources, &path).await {
        Ok((at, transcribed)) => (context(&at), Ok(transcribed)),
        Err(e) => (Html::default(), Err(e)),
    };
    transcript_page(
        &sources,
        uri.path(),
        title,
        context,
        transcribed,
        query.verbose(),
        Html::default(),
    )
    .await
}

/// The regions of the page of a replay's transcript.
pub(super) async fn replay_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Path(path): Path<ReplayPath>,
    Query(query): Query<RunQuery>,
    headers: HeaderMap,
) -> Response {
    let transcribed = transcribed(&sources, &path).await.ok().map(|(_, t)| t);
    transcript_events(
        sources,
        stopping,
        transcribed.as_ref(),
        &query,
        &headers,
        false,
    )
    .await
}
