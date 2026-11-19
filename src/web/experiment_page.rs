//! The experiment page (`docs/specs/benchmarks.md`, Experiment): experiment
//! ID's heading, then its results as a chart, a grid and a table whose replays
//! open to the transcripts of their runs and their judges; and the cancelling
//! of an open experiment.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::num::NonZeroU32;
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

use super::confirmation::{Confirmation, TurnDown};
use super::html::{self, Html};
use super::launch::counted;
use super::names::Names;
use super::run_page::{RunQuery, Transcribed, transcript_events, transcript_page};
use super::shell::{Area, Frame, Trail, Within, frame_regions};
use super::table::{Align, Column};
use super::write::{Refused, Write, alert};
use super::{
    ErrorPage, Missing, Page, events, experiment_path, experiments, nothing_there, render,
    suite_path,
};
use crate::api::Sources;
use crate::benchmarks::results::{self, ConfigurationResults, Figures, Passes, Results};
use crate::benchmarks::{
    CancelRefusal, ConfigurationName, ExperimentId, ExperimentRecord, ExperimentState, NoJudgement,
    ReplayFigures, Shown, TrialEnd, TrialKey, TrialState, dollars, duration, percent, without,
};
use crate::case::CaseName;
use crate::domain::Timestamp;
use crate::logs::{KeptRuns, Which};
use crate::phrase::Status;
use crate::replay::{self, judge};
use crate::results::{Leak, Verdict};
use crate::runs::RunId;
use crate::runs::files::RunFiles;

/// The status and the word an experiment's state shows with: that of the status
/// with the same meaning (`docs/specs/benchmarks.md`, Experiment, State).
pub(super) fn state_status(shown: &Shown) -> (Status, &'static str) {
    match shown {
        Shown::Running => (Status::InFlight, "running"),
        Shown::Waiting(_) => (Status::Paused, "waiting"),
        Shown::Ended => (Status::Done, "ended"),
        Shown::Cancelled => (Status::Cancelled, "cancelled"),
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
    status: Status,
    state: &'static str,
    /// Its cancelling, while it is open (`docs/specs/benchmarks.md`,
    /// Experiment, Heading).
    cancel: Option<Confirmation>,
    path: String,
    facts: Vec<Fact>,
    chart: [Plot; 2],
    /// The configurations' rows, ranked, each followed by the rows it
    /// opens to.
    rows: Vec<Row>,
    grid: Grid,
}

/// A fact (`docs/specs/benchmarks.md`, Experiment, Facts): its label and
/// its value, "–" where the value is absent.
struct Fact {
    label: &'static str,
    value: Html,
}

/// What the page shows of a figure that is absent
/// (`docs/specs/design_system.md`, Screen states, Absent).
const ABSENT: &str = "–";

/// The horizontal axis of the chart.
#[derive(Clone, Copy)]
enum Axis {
    Cost,
    Time,
}

impl Axis {
    /// Its name in the page, which the `select`'s option gives.
    fn name(self) -> &'static str {
        match self {
            Axis::Cost => "cost",
            Axis::Time => "time",
        }
    }

    /// Its option's label (`docs/specs/benchmarks.md`, Experiment, The
    /// chart).
    fn label(self) -> &'static str {
        match self {
            Axis::Cost => "Mean cost",
            Axis::Time => "Mean time",
        }
    }

    /// The mean of `figures` along it, if it has one.
    fn mean(self, figures: &Figures) -> Option<f64> {
        match self {
            Axis::Cost => figures.cost.mean,
            Axis::Time => figures.time.mean,
        }
    }

    /// `value` along it, in words.
    fn words(self, value: f64) -> String {
        match self {
            Axis::Cost => dollars(value),
            Axis::Time => duration(value),
        }
    }
}

/// The chart along an axis: its ticks and its points, or none with no
/// point to show.
struct Plot {
    axis: Axis,
    drawn: Option<Drawn>,
}

/// What a plot with points draws: its SVG's `viewBox`, the lines across it
/// at a pass rate of 100% and of 50%, its axes, its ticks and its points.
struct Drawn {
    view_box: String,
    rules: String,
    axes: String,
    ticks: Vec<Tick>,
    points: Vec<Point>,
}

/// A tick's label, at `x`, `y` in the chart's SVG.
struct Tick {
    x: f64,
    y: f64,
    anchor: &'static str,
    label: String,
}

/// A configuration's point (`docs/specs/benchmarks.md`, Experiment, A
/// point), drawn in the `foreignObject` at `x`, `y` in the chart's SVG,
/// `POINT_WIDTH` by `POINT_HEIGHT`, whose dot sits at the point's place.
struct Point {
    id: String,
    name: ConfigurationName,
    tip: String,
    x: f64,
    y: f64,
    /// Whether its name is at its dot's left, so that it stays inside the
    /// chart.
    named_left: bool,
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

/// A row of the configurations table (`docs/specs/benchmarks.md`,
/// Experiment, The table).
enum Row {
    Figures(FiguresRow),
    /// What a replay's row opens to.
    Replay(ReplayDetails),
}

/// A configuration's, a case's or a replay's row. Its id joins the names
/// of what it is of with `_`, which no name holds, so that no two rows
/// share one.
struct FiguresRow {
    id: String,
    level: Level,
    /// The rows it opens to, if any.
    opens: Vec<String>,
    name: String,
    pass_rate: PassRate,
    /// Its cells under `ConfigurationsColumn::FIGURES`.
    figures: [String; 6],
}

/// What a figures row is of, and the id of the row a case's or a
/// replay's is under.
enum Level {
    Configuration,
    Case { of: String },
    Replay { of: String },
}

impl Level {
    /// The row it is under, or "" for a configuration's.
    fn of(&self) -> &str {
        match self {
            Level::Configuration => "",
            Level::Case { of } | Level::Replay { of } => of,
        }
    }

    /// The classes of its first cell, which set it in under the row it is
    /// under.
    fn name_class(&self) -> &'static str {
        match self {
            Level::Configuration => "",
            Level::Case { .. } => "pl-8",
            Level::Replay { .. } => "pl-12",
        }
    }
}

/// What a row's Pass rate shows.
enum PassRate {
    Text(String),
    /// A replay's, as a status (`docs/specs/benchmarks.md`, Experiment, A
    /// replay's status).
    Status {
        status: Status,
        words: String,
    },
}

/// What a replay's row opens to: its judge's reasoning, and the links to
/// the transcripts that have started.
struct ReplayDetails {
    id: String,
    of: String,
    reasoning: Option<Arc<Reasoning>>,
    run_href: Option<String>,
    judge_href: Option<String>,
}

impl ReplayDetails {
    /// Whether it links to a transcript.
    fn links(&self) -> bool {
        self.run_href.is_some() || self.judge_href.is_some()
    }

    /// Whether it holds nothing to open to.
    fn is_empty(&self) -> bool {
        self.reasoning.is_none() && !self.links()
    }
}

/// A column of the configurations table, the first naming each row.
#[derive(Clone, Copy)]
enum ConfigurationsColumn {
    Name,
    PassRate,
    MeanCost,
    TotalCost,
    MeanTime,
    TotalTime,
    Leaks,
    Unjudged,
}

impl ConfigurationsColumn {
    const ALL: [ConfigurationsColumn; 8] = [
        ConfigurationsColumn::Name,
        ConfigurationsColumn::PassRate,
        ConfigurationsColumn::MeanCost,
        ConfigurationsColumn::TotalCost,
        ConfigurationsColumn::MeanTime,
        ConfigurationsColumn::TotalTime,
        ConfigurationsColumn::Leaks,
        ConfigurationsColumn::Unjudged,
    ];

    /// The columns of a row's figures.
    const FIGURES: [ConfigurationsColumn; 6] = [
        ConfigurationsColumn::MeanCost,
        ConfigurationsColumn::TotalCost,
        ConfigurationsColumn::MeanTime,
        ConfigurationsColumn::TotalTime,
        ConfigurationsColumn::Leaks,
        ConfigurationsColumn::Unjudged,
    ];

    fn head(self) -> &'static str {
        match self {
            ConfigurationsColumn::Name => "",
            ConfigurationsColumn::PassRate => "Pass rate",
            ConfigurationsColumn::MeanCost => "Mean cost",
            ConfigurationsColumn::TotalCost => "Total cost",
            ConfigurationsColumn::MeanTime => "Mean time",
            ConfigurationsColumn::TotalTime => "Total time",
            ConfigurationsColumn::Leaks => "Leaks",
            ConfigurationsColumn::Unjudged => "Unjudged",
        }
    }

    fn align(self) -> &'static str {
        Align::of(match self {
            ConfigurationsColumn::MeanCost | ConfigurationsColumn::TotalCost => Column::Money,
            ConfigurationsColumn::MeanTime | ConfigurationsColumn::TotalTime => Column::Duration,
            ConfigurationsColumn::Name
            | ConfigurationsColumn::PassRate
            | ConfigurationsColumn::Leaks
            | ConfigurationsColumn::Unjudged => Column::Other,
        })
        .class()
    }
}

/// The path of the page of the transcript `of` of the replay of `case`
/// under `configuration` in `repetition` of experiment `id`.
fn transcript_path(
    id: ExperimentId,
    configuration: &ConfigurationName,
    case: &CaseName,
    repetition: NonZeroU32,
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
        let results = results::results(record);
        let ranked = ranked(&results.configurations);
        let (status, state) = state_status(shown);
        let path = experiment_path(record.id);
        ExperimentView {
            id: record.id,
            status,
            state,
            cancel: (record.state == ExperimentState::Open).then(|| Confirmation {
                id: "cancel-experiment".into(),
                label: "Cancel",
                question: format!(
                    "Cancel experiment {}? This stops its running replays and judges.",
                    record.id
                ),
                turn_down: TurnDown::KeepRunning,
                action: format!("{path}/cancel"),
                fields: vec![],
                said: "experiment-said",
            }),
            path,
            facts: facts(
                record,
                shown,
                &results,
                sources.clock.now(),
                &Names::of(&sources.work_dir.repo, None),
            ),
            chart: [Axis::Cost, Axis::Time].map(|axis| plot(axis, &ranked)),
            rows: rows(sources, record, &ranked, outputs),
            grid: grid(record, &ranked),
        }
    }
}

/// The facts of `record` (`docs/specs/benchmarks.md`, Experiment, Facts) as
/// of `now`, each where the spec's table shows it.
fn facts(
    record: &ExperimentRecord,
    shown: &Shown,
    results: &Results,
    now: Timestamp,
    names: &Names,
) -> Vec<Fact> {
    let absent = || html::escape(ABSENT);
    let mut facts = vec![Fact {
        label: "Launched",
        value: html::time(record.launched_at),
    }];
    if record.state != ExperimentState::Open {
        facts.push(Fact {
            label: "Ended",
            value: record.state.ended_at().map_or_else(absent, html::time),
        });
    }
    let running = (record.state == ExperimentState::Open).then_some(record.launched_at);
    facts.push(Fact {
        label: "Duration",
        value: record.duration(now).map_or_else(absent, |duration| {
            html::duration(None, duration.as_secs(), running)
        }),
    });
    facts.push(Fact {
        label: "Replays",
        value: html::escape(&format!("{} of {}", results.replays_ended, results.all)),
    });
    let mut cost = dollars(results.cost.total);
    without(&mut cost, results.cost.none, "a cost");
    facts.push(Fact {
        label: "Cost",
        value: html::escape(&cost),
    });
    facts.push(Fact {
        label: "Judges",
        value: html::escape(&dollars(results.judges.total)),
    });
    if let Shown::Waiting(waiting_for) = shown {
        facts.push(Fact {
            label: "Waiting for",
            value: html::phrase(&waiting_for.phrase(), names),
        });
    }
    facts
}

/// `configurations` with their places in the experiment, highest pass rate
/// first, those with the same pass rate in the experiment's order, and
/// those with none last (`docs/specs/benchmarks.md`, Experiment, The
/// table).
fn ranked(configurations: &[ConfigurationResults]) -> Vec<(usize, &ConfigurationResults)> {
    let mut ranked: Vec<_> = configurations.iter().enumerate().collect();
    ranked.sort_by(|(_, a), (_, b)| match (a.pass_rate, b.pass_rate) {
        (Some(a), Some(b)) => b.total_cmp(&a),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    ranked
}

const CHART_WIDTH: f64 = 480.0;
const CHART_HEIGHT: f64 = 260.0;
const CHART_LEFT: f64 = 48.0;
const CHART_RIGHT: f64 = 24.0;
const CHART_TOP: f64 = 16.0;
const CHART_BOTTOM: f64 = 32.0;
const POINT_WIDTH: f64 = 160.0;
const POINT_HEIGHT: f64 = 16.0;

/// The chart of the `ranked` configurations along `axis`: a point for each
/// that has a pass rate and a mean along it.
fn plot(axis: Axis, ranked: &[(usize, &ConfigurationResults)]) -> Plot {
    let at: Vec<(&ConfigurationResults, f64, f64)> = ranked
        .iter()
        .filter_map(|(_, configuration)| {
            Some((
                *configuration,
                axis.mean(&configuration.figures)?,
                configuration.pass_rate?,
            ))
        })
        .collect();
    if at.is_empty() {
        return Plot { axis, drawn: None };
    }
    let most = at.iter().map(|(_, mean, _)| *mean).fold(0.0_f64, f64::max);
    let scale = if most > 0.0 { most * 1.1 } else { 1.0 };
    let (left, right) = (CHART_LEFT, CHART_WIDTH - CHART_RIGHT);
    let (top, bottom) = (CHART_TOP, CHART_HEIGHT - CHART_BOTTOM);
    let x = |mean: f64| left + mean / scale * (right - left);
    let y = |rate: f64| top + (1.0 - rate) * (bottom - top);
    let mut ticks: Vec<Tick> = [0.0, 0.5, 1.0]
        .into_iter()
        .map(|rate| Tick {
            x: left - 6.0,
            y: y(rate) + 4.0,
            anchor: "end",
            label: percent(Some(rate)),
        })
        .collect();
    ticks.extend(
        [(0.0, "middle"), (0.5, "middle"), (1.0, "end")]
            .into_iter()
            .map(|(share, anchor)| Tick {
                x: x(share * scale),
                y: bottom + 18.0,
                anchor,
                label: axis.words(share * scale),
            }),
    );
    let points = at
        .into_iter()
        .map(|(configuration, mean, rate)| {
            let (at_x, at_y) = (x(mean), y(rate));
            let named_left = at_x > left + (right - left) * 0.6;
            Point {
                id: format!("point-{}-{}", axis.name(), configuration.name),
                name: configuration.name.clone(),
                tip: format!(
                    "{}: pass rate {}, {} {}",
                    configuration.name,
                    percent(Some(rate)),
                    axis.label().to_lowercase(),
                    axis.words(mean)
                ),
                x: if named_left {
                    at_x - POINT_WIDTH + POINT_HEIGHT / 2.0
                } else {
                    at_x - POINT_HEIGHT / 2.0
                },
                y: at_y - POINT_HEIGHT / 2.0,
                named_left,
            }
        })
        .collect();
    let half = y(0.5);
    Plot {
        axis,
        drawn: Some(Drawn {
            view_box: format!("0 0 {CHART_WIDTH} {CHART_HEIGHT}"),
            rules: format!("M{left} {top}H{right}M{left} {half}H{right}"),
            axes: format!("M{left} {top}V{bottom}H{right}"),
            ticks,
            points,
        }),
    }
}

/// The grid of `record`: a row for each case, a cell for each of the
/// `ranked` configurations.
fn grid(record: &ExperimentRecord, ranked: &[(usize, &ConfigurationResults)]) -> Grid {
    Grid {
        configurations: ranked
            .iter()
            .map(|(_, configuration)| configuration.name.clone())
            .collect(),
        rows: record
            .cases
            .iter()
            .enumerate()
            .map(|(k, case)| GridRow {
                case: case.clone(),
                cells: ranked
                    .iter()
                    .map(|(_, configuration)| {
                        configuration
                            .cases
                            .get(k)
                            .map_or_else(String::new, |of_case| {
                                let Figures {
                                    leaks, unjudged, ..
                                } = of_case.figures;
                                let mut cell = passes_of(of_case.passes);
                                if leaks > 0 {
                                    let _ = write!(cell, ", {}", counted(leaks, "leak", "leaks"));
                                }
                                if unjudged > 0 {
                                    let _ = write!(cell, ", {unjudged} unjudged");
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
/// leak, as "2 of 3", and absent with none.
fn passes_of(passes: Option<Passes>) -> String {
    passes.map_or_else(|| ABSENT.into(), |passes| passes.to_string())
}

/// The cells of `figures` under `ConfigurationsColumn::FIGURES`.
fn figure_cells(figures: &Figures) -> [String; 6] {
    let Figures {
        cost,
        time,
        leaks,
        unjudged,
    } = figures;
    let mean =
        |mean: Option<f64>, unit: fn(f64) -> String| mean.map_or_else(|| ABSENT.into(), unit);
    let mut total_cost = dollars(cost.total);
    without(&mut total_cost, cost.none, "a cost");
    [
        mean(cost.mean, dollars),
        total_cost,
        mean(time.mean, duration),
        duration(time.total),
        leaks.to_string(),
        unjudged.to_string(),
    ]
}

/// The configurations table's rows of `record`: each of the `ranked`
/// configurations, each followed by its cases, each followed by its
/// replays.
fn rows(
    sources: &Sources,
    record: &ExperimentRecord,
    ranked: &[(usize, &ConfigurationResults)],
    outputs: &Outputs,
) -> Vec<Row> {
    let mut rows = vec![];
    for &(c, configuration) in ranked {
        let configuration_id = format!("configuration-{}", configuration.name);
        let case_id = |case: &CaseName| format!("{configuration_id}_case-{case}");
        rows.push(Row::Figures(FiguresRow {
            id: configuration_id.clone(),
            level: Level::Configuration,
            opens: record.cases.iter().map(case_id).collect(),
            name: configuration.name.to_string(),
            pass_rate: PassRate::Text(
                configuration
                    .pass_rate
                    .map_or_else(|| ABSENT.into(), |rate| percent(Some(rate))),
            ),
            figures: figure_cells(&configuration.figures),
        }));
        for (k, case) in record.cases.iter().enumerate() {
            let id = case_id(case);
            let trials: Vec<_> = record
                .trials
                .iter()
                .filter(|(key, _)| key.case == k && key.configuration == c)
                .collect();
            let replay_id = |key: &TrialKey| format!("{id}_replay-{}", key.repetition);
            let of_case = configuration.cases.get(k);
            rows.push(Row::Figures(FiguresRow {
                id: id.clone(),
                level: Level::Case {
                    of: configuration_id.clone(),
                },
                opens: trials.iter().map(|(key, _)| replay_id(key)).collect(),
                name: case.to_string(),
                pass_rate: PassRate::Text(passes_of(of_case.and_then(|of_case| of_case.passes))),
                figures: of_case
                    .map_or_else(Default::default, |of_case| figure_cells(&of_case.figures)),
            }));
            for (key, state) in trials {
                let replay = ReplayAt {
                    id: record.id,
                    configuration: configuration.name.clone(),
                    case: case.clone(),
                    repetition: key.repetition,
                    of: Of::Run,
                };
                rows.extend(replay_rows(
                    sources,
                    &replay,
                    replay_id(key),
                    id.clone(),
                    state,
                    outputs,
                ));
            }
        }
    }
    rows
}

/// The row of the replay `at`, `id` under the row `of`, and the row it
/// opens to, if it has anything to open to.
fn replay_rows(
    sources: &Sources,
    at: &ReplayAt,
    id: String,
    of: String,
    state: &TrialState,
    outputs: &Outputs,
) -> Vec<Row> {
    let ReplayShown {
        status,
        words,
        replay,
    } = replay_shown(state);
    let output = experiments(sources).output_dir(at.id, &at.configuration, &at.case, at.repetition);
    let began = outputs.began(&output);
    let reasoning = matches!(state, TrialState::Ended(TrialEnd::Judged { .. }))
        .then(|| outputs.reasoning(&output))
        .flatten();
    let judge_began = matches!(
        state,
        TrialState::Judging { .. }
            | TrialState::Ended(
                TrialEnd::Judged { .. }
                    | TrialEnd::Unjudged {
                        why: NoJudgement::Judge { .. },
                        ..
                    }
            )
    );
    let details = ReplayDetails {
        id: format!("{id}_details"),
        of: id.clone(),
        reasoning,
        run_href: began.then(|| at.path(Of::Run)),
        judge_href: (began && judge_began).then(|| at.path(Of::Judge)),
    };
    let opens = !details.is_empty();
    let absent = || String::from(ABSENT);
    let mut rows = vec![Row::Figures(FiguresRow {
        id,
        level: Level::Replay { of },
        opens: if opens {
            vec![details.id.clone()]
        } else {
            vec![]
        },
        name: format!("Replay {}", at.repetition),
        pass_rate: PassRate::Status { status, words },
        figures: [
            replay
                .and_then(|replay| replay.money)
                .map_or_else(absent, |money| dollars(money.into())),
            String::new(),
            replay.map_or_else(absent, |replay| duration(replay.time.as_secs_f64())),
            String::new(),
            String::new(),
            String::new(),
        ],
    })];
    if opens {
        rows.push(Row::Replay(details));
    }
    rows
}

/// What a replay's row says of its state: its Pass rate's status and words
/// (`docs/specs/benchmarks.md`, Experiment, The table and A replay's
/// status), and its replay's figures, once it has them.
struct ReplayShown<'a> {
    status: Status,
    words: String,
    replay: Option<&'a ReplayFigures>,
}

fn replay_shown(state: &TrialState) -> ReplayShown<'_> {
    let (status, words, replay) = match state {
        TrialState::Pending => (Status::Paused, "waiting".into(), None),
        TrialState::Replaying => (Status::InFlight, "replaying".into(), None),
        TrialState::Replayed { replay } => (Status::Paused, "waiting".into(), Some(replay)),
        TrialState::Judging { replay } => (Status::InFlight, "judging".into(), Some(replay)),
        TrialState::Ended(TrialEnd::Judged { replay, judgement }) => {
            let leak = if judgement.leak { ", with a leak" } else { "" };
            let status = if judgement.correctness == Verdict::Pass && !judgement.leak {
                Status::Done
            } else {
                Status::Failed
            };
            (
                status,
                format!("{}{leak}", judgement.correctness.name()),
                Some(replay),
            )
        }
        TrialState::Ended(TrialEnd::Unjudged { replay, why }) => {
            let (status, why) = match why {
                NoJudgement::Outcome => (
                    replay.outcome.status(),
                    "its run is not done or handed back".to_string(),
                ),
                NoJudgement::Judge { reason, .. } => {
                    (Status::Failed, format!("the judge made none: {reason}"))
                }
                NoJudgement::Cancelled => (Status::Cancelled, "cancelled".into()),
            };
            (status, format!("no judgement: {why}"), Some(replay))
        }
        TrialState::Ended(TrialEnd::Cancelled) => {
            (Status::Cancelled, "no judgement: cancelled".into(), None)
        }
    };
    ReplayShown {
        status,
        words,
        replay,
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "experiment.html")]
struct ExperimentPage {
    page: Page,
    frame: Frame,
    experiment: ExperimentView,
}

/// The heading and the facts.
#[derive(Template)]
#[template(path = "experiment_heading.html")]
struct HeadingRegion<'a> {
    experiment: &'a ExperimentView,
}

#[derive(Template)]
#[template(path = "experiment_chart.html")]
struct ChartRegion<'a> {
    experiment: &'a ExperimentView,
}

#[derive(Template)]
#[template(path = "experiment_configurations.html")]
struct ConfigurationsRegion<'a> {
    experiment: &'a ExperimentView,
}

#[derive(Template)]
#[template(path = "experiment_grid.html")]
struct GridRegion<'a> {
    experiment: &'a ExperimentView,
}

impl ExperimentView {
    /// Its regions of the page's stream.
    fn regions(&self) -> [(&'static str, String); 4] {
        [
            ("experiment", render(&HeadingRegion { experiment: self })),
            ("chart", render(&ChartRegion { experiment: self })),
            (
                "configurations",
                render(&ConfigurationsRegion { experiment: self }),
            ),
            ("grid", render(&GridRegion { experiment: self })),
        ]
    }
}

/// Why there is no experiment page: no such experiment, or one that does
/// not read.
enum Unshown {
    NoSuch,
    Failed(anyhow::Error),
}

/// What is said of an experiment `id` that there is none of.
fn no_such(id: &str) -> String {
    format!("there is no experiment {id}")
}

/// Experiment `id`, by its ID as a path gives it.
async fn record_of(sources: &Sources, id: &str) -> Result<ExperimentRecord, Unshown> {
    let parsed: ExperimentId = id.parse().map_err(|_| Unshown::NoSuch)?;
    match experiments(sources).show(parsed).await {
        Ok(Some(record)) => Ok(record),
        Ok(None) => Err(Unshown::NoSuch),
        Err(e) => Err(Unshown::Failed(e)),
    }
}

impl Unshown {
    /// The status the page answers with and what it says, logging a
    /// failure to read experiment `id`.
    fn answer(self, id: &str) -> (StatusCode, String) {
        match self {
            Unshown::NoSuch => (StatusCode::NOT_FOUND, no_such(id)),
            Unshown::Failed(e) => {
                let error = format!("{e:#}");
                warn!(experiment = id, error, "reading an experiment failed");
                (StatusCode::INTERNAL_SERVER_ERROR, error)
            }
        }
    }
}

/// The name of experiment `id` in its heading.
fn experiment_name(id: impl std::fmt::Display) -> String {
    format!("Experiment {id}")
}

/// The crumbs of the screens of experiment `record`: Suites, then its suite
/// (`docs/specs/shell.md`, Breadcrumbs).
fn suite_crumbs(record: &ExperimentRecord) -> Within {
    Trail::within(Area::Suites).under(
        record.suite.name.to_string(),
        suite_path(&record.suite.name),
    )
}

pub(super) async fn experiment_page(
    State(sources): State<Sources>,
    Path(id): Path<String>,
) -> Response {
    let frame = Frame::of(&sources).await;
    match record_of(&sources, &id).await {
        Ok(record) => {
            let shown = shown_of(&sources, &record);
            let experiment = ExperimentView::of(&sources, &record, &shown, &Outputs::default());
            ExperimentPage {
                page: Page::new(
                    suite_crumbs(&record).here(experiment_name(record.id)),
                    format!("/events{}", experiment.path),
                ),
                frame,
                experiment,
            }
            .into_response()
        }
        Err(unshown) => {
            let page = Page::new(
                Trail::within(Area::Suites).here(experiment_name(&id)),
                format!("/events/experiments/{id}"),
            );
            if let Unshown::NoSuch = unshown {
                return nothing_there(page, frame, "experiment", Missing::Experiment(id));
            }
            let (status, error) = unshown.answer(&id);
            let page = ErrorPage {
                page,
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
                regions.extend(experiment.regions());
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
    match experiments(&sources)
        .cancel(parsed, sources.clock.now())
        .await
    {
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
    repetition: NonZeroU32,
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

/// The replay `path` names, of its experiment `record`, and where its
/// transcript is read from; or else the status and what the page says of
/// there being none.
fn transcribed(
    sources: &Sources,
    path: &ReplayPath,
    record: &ExperimentRecord,
) -> Result<(ReplayAt, Transcribed), (StatusCode, String)> {
    let replay = format!("{}/{}/{}", path.configuration, path.case, path.repetition);
    let none = || {
        (
            StatusCode::NOT_FOUND,
            format!("experiment {} has no replay {replay}", path.id),
        )
    };
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
        .filter(|repetition: &NonZeroU32| *repetition <= record.repetitions)
        .ok_or_else(none)?;
    let of = Of::ALL
        .into_iter()
        .find(|of| of.name() == path.of)
        .ok_or_else(none)?;
    let at = ReplayAt {
        id: record.id,
        configuration: record.configurations[configuration].clone(),
        case: record.cases[case].clone(),
        repetition,
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
            missing: None,
        },
        Of::Judge => Transcribed {
            kept: KeptRuns::Judge(output),
            which: Which::Run {
                id: case_run,
                item: None,
            },
            name: Some("judge".into()),
            of: html::escape(&format!("the judge of replay {}", at.name())),
            missing: None,
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
    let name = format!(
        "Replay {}/{}/{}'s {}",
        path.configuration, path.case, path.repetition, path.of
    );
    let record = match record_of(&sources, &path.id).await {
        Err(Unshown::NoSuch) => {
            let trail = Trail::within(Area::Suites).here(name);
            let page = Page::new(trail, format!("/events{}", uri.path()));
            let frame = Frame::of(&sources).await;
            return nothing_there(page, frame, "transcript", Missing::Experiment(path.id));
        }
        record => record.map_err(|unshown| unshown.answer(&path.id)),
    };
    let trail = match &record {
        Ok(record) => {
            suite_crumbs(record).under(experiment_name(record.id), experiment_path(record.id))
        }
        Err(_) => Trail::within(Area::Suites),
    }
    .here(name);
    let (context, transcribed) =
        match record.and_then(|record| transcribed(&sources, &path, &record)) {
            Ok((at, transcribed)) => (context(&at), Ok(transcribed)),
            Err(e) => (Html::default(), Err(e)),
        };
    transcript_page(
        &sources,
        uri.path(),
        trail,
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
    let transcribed = match record_of(&sources, &path.id).await {
        Ok(record) => transcribed(&sources, &path, &record).ok().map(|(_, t)| t),
        Err(_) => None,
    };
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::benchmarks::JudgementFigures;

    #[track_caller]
    fn ended(outcome: &str) -> ReplayFigures {
        ReplayFigures {
            outcome: serde_json::from_value(outcome.into()).unwrap(),
            money: None,
            time: Duration::from_secs(60),
        }
    }

    fn judged(correctness: Verdict, leak: bool) -> TrialState {
        TrialState::Ended(TrialEnd::Judged {
            replay: ended("done"),
            judgement: JudgementFigures {
                correctness,
                leak,
                judge_money: None,
            },
        })
    }

    #[track_caller]
    fn unjudged(outcome: &str, why: NoJudgement) -> TrialState {
        TrialState::Ended(TrialEnd::Unjudged {
            replay: ended(outcome),
            why,
        })
    }

    /// `docs/specs/benchmarks.md`, Experiment, A replay's status, and the
    /// Pass rate column's word.
    #[test_log::test]
    fn each_replay_shows_as_the_status_its_state_gives() {
        let states = [
            TrialState::Pending,
            TrialState::Replayed {
                replay: ended("done"),
            },
            TrialState::Replaying,
            TrialState::Judging {
                replay: ended("done"),
            },
            judged(Verdict::Pass, false),
            judged(Verdict::Pass, true),
            judged(Verdict::Fail, false),
            unjudged(
                "done",
                NoJudgement::Judge {
                    reason: "it gave up".into(),
                    judge_money: None,
                },
            ),
        ];

        let shown: Vec<(Status, String)> = states
            .iter()
            .map(|state| {
                let shown = replay_shown(state);
                (shown.status, shown.words)
            })
            .collect();

        assert_eq!(
            shown,
            [
                (Status::Paused, "waiting".into()),
                (Status::Paused, "waiting".into()),
                (Status::InFlight, "replaying".into()),
                (Status::InFlight, "judging".into()),
                (Status::Done, "pass".into()),
                (Status::Failed, "pass, with a leak".into()),
                (Status::Failed, "fail".into()),
                (
                    Status::Failed,
                    "no judgement: the judge made none: it gave up".into()
                ),
            ]
        );
    }

    /// `docs/specs/benchmarks.md`, Experiment, A replay's status: as its
    /// run's outcome does (`docs/specs/runs.md`, Ending).
    #[test_log::test]
    fn a_replay_whose_run_is_not_done_or_handed_back_shows_as_its_runs_outcome() {
        let outcomes = [
            "failed",
            "cancelled",
            "usage-limit",
            "interrupted",
            "handed-back",
        ];

        let shown: Vec<Status> = outcomes
            .map(|outcome| unjudged(outcome, NoJudgement::Outcome))
            .iter()
            .map(|state| replay_shown(state).status)
            .collect();

        assert_eq!(
            shown,
            [
                Status::Failed,
                Status::Cancelled,
                Status::Cancelled,
                Status::Cancelled,
                Status::Waiting,
            ]
        );
    }

    /// A replay of a cancelled experiment shows as a cancelled run does
    /// (`docs/specs/runs.md`, Ending).
    #[test_log::test]
    fn a_replay_its_experiments_cancel_left_without_a_judgement_shows_cancelled() {
        let states = [
            unjudged("done", NoJudgement::Cancelled),
            TrialState::Ended(TrialEnd::Cancelled),
        ];

        let shown = states.map(|state| replay_shown(&state).status);

        assert_eq!(shown, [Status::Cancelled; 2]);
    }
}
