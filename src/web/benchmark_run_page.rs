//! The benchmark run page (`docs/specs/benchmarks.md`, Benchmark run):
//! benchmark run ID's heading, then its results as a chart, a grid and a table
//! whose trials open to the transcripts of their runs and their scorers; and
//! the cancelling of an open benchmark run.

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
    ErrorPage, Missing, Page, benchmark_path, benchmark_run_path, benchmark_runs, events,
    nothing_there, render,
};
use crate::api::Sources;
use crate::benchmarks::results::{self, ConfigurationResults, Figures, Passes, Results};
use crate::benchmarks::{
    BenchmarkRunId, BenchmarkRunRecord, BenchmarkRunState, CancelRefusal, ConfigurationName,
    NoScore, Shown, TrialEnd, TrialFigures, TrialKey, TrialState, dollars, duration, percent,
    without,
};
use crate::domain::Timestamp;
use crate::logs::{KeptRuns, Which};
use crate::phrase::Status;
use crate::results::{Leak, Verdict};
use crate::runs::RunId;
use crate::runs::files::RunFiles;
use crate::task::TaskName;
use crate::trial::{self, scorer};

/// The status and the word a benchmark run's state shows with: that of the
/// status with the same meaning (`docs/specs/benchmarks.md`, Benchmark run,
/// State).
pub(super) fn state_status(shown: &Shown) -> (Status, &'static str) {
    match shown {
        Shown::Running => (Status::InFlight, "running"),
        Shown::Waiting(_) => (Status::Paused, "waiting"),
        Shown::Ended => (Status::Done, "ended"),
        Shown::Cancelled => (Status::Cancelled, "cancelled"),
    }
}

/// How benchmark run `record` shows: an open one as `serve` last showed it.
pub(super) fn shown_of(sources: &Sources, record: &BenchmarkRunRecord) -> Shown {
    let served = sources
        .benchmark_runs
        .shown()
        .benchmark_runs
        .get(&record.id)
        .cloned();
    Shown::of(record.state, served)
}

/// What the trials' outputs hold that the page shows, by output, as read
/// once it holds it: that an output has begun, and the scorers' reasoning
/// of a scored trial, whose score is never replaced.
#[derive(Clone, Default)]
struct Outputs(Arc<Mutex<HashMap<PathBuf, OutputRead>>>);

#[derive(Clone, Default)]
struct OutputRead {
    began: bool,
    score: Score,
}

/// What an output's score gave the page.
#[derive(Clone, Default)]
enum Score {
    #[default]
    Unread,
    /// The scorers' reasoning, or none where the output holds no score
    /// that reads.
    Read(Option<Arc<Reasoning>>),
}

/// What a scorer said of a trial.
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
            output.began = trial::began(dir);
        }
        output.began
    }

    /// The reasoning of the score the output in `dir` of a scored
    /// trial holds, if it holds one that reads; one that does not read is
    /// logged.
    fn reasoning(&self, dir: &FilePath) -> Option<Arc<Reasoning>> {
        let mut read = self.lock();
        let output = read.entry(dir.to_path_buf()).or_default();
        if let Score::Read(reasoning) = &output.score {
            return reasoning.clone();
        }
        let reasoning = match scorer::read(dir) {
            Ok(kept) => kept.map(|kept| {
                Arc::new(Reasoning {
                    correctness: kept.correctness.reasoning,
                    leak: match kept.leak {
                        Leak::None { reasoning } | Leak::Found { reasoning, .. } => reasoning,
                    },
                })
            }),
            Err(e) => {
                warn!(output = %dir.display(), error = format!("{e:#}"), "reading a score failed");
                None
            }
        };
        output.score = Score::Read(reasoning.clone());
        reasoning
    }
}

/// The benchmark run as the page shows it.
struct BenchmarkRunView {
    id: BenchmarkRunId,
    status: Status,
    state: &'static str,
    /// Its cancelling, while it is open (`docs/specs/benchmarks.md`,
    /// Benchmark run, Heading).
    cancel: Option<Confirmation>,
    path: String,
    facts: Vec<Fact>,
    chart: [Plot; 2],
    /// The configurations' rows, ranked, each followed by the rows it
    /// opens to.
    rows: Vec<Row>,
    grid: Grid,
}

/// A fact (`docs/specs/benchmarks.md`, Benchmark run, Facts): its label and
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

    /// Its option's label (`docs/specs/benchmarks.md`, Benchmark run, The
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

/// A configuration's point (`docs/specs/benchmarks.md`, Benchmark run, A
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
    task: TaskName,
    /// For each configuration, its passes, then its leaks and its trials
    /// without a score, if it has any.
    cells: Vec<String>,
}

/// A row of the configurations table (`docs/specs/benchmarks.md`,
/// Benchmark run, The table).
enum Row {
    Figures(FiguresRow),
    /// What a trial's row opens to.
    Trial(TrialDetails),
}

/// A configuration's, a task's or a trial's row. Its id joins the names
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

/// What a figures row is of, and the id of the row a task's or a
/// trial's is under.
enum Level {
    Configuration,
    Task { of: String },
    Trial { of: String },
}

impl Level {
    /// The row it is under, or "" for a configuration's.
    fn of(&self) -> &str {
        match self {
            Level::Configuration => "",
            Level::Task { of } | Level::Trial { of } => of,
        }
    }

    /// The classes of its first cell, which set it in under the row it is
    /// under.
    fn name_class(&self) -> &'static str {
        match self {
            Level::Configuration => "",
            Level::Task { .. } => "pl-8",
            Level::Trial { .. } => "pl-12",
        }
    }
}

/// What a row's Pass rate shows.
enum PassRate {
    Text(String),
    /// A trial's, as a status (`docs/specs/benchmarks.md`, Benchmark run, A
    /// trial's status).
    Status {
        status: Status,
        words: String,
    },
}

/// What a trial's row opens to: its scorer's reasoning, and the links to
/// the transcripts that have started.
struct TrialDetails {
    id: String,
    of: String,
    reasoning: Option<Arc<Reasoning>>,
    run_href: Option<String>,
    scorer_href: Option<String>,
}

impl TrialDetails {
    /// Whether it links to a transcript.
    fn links(&self) -> bool {
        self.run_href.is_some() || self.scorer_href.is_some()
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
    Unscored,
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
        ConfigurationsColumn::Unscored,
    ];

    /// The columns of a row's figures.
    const FIGURES: [ConfigurationsColumn; 6] = [
        ConfigurationsColumn::MeanCost,
        ConfigurationsColumn::TotalCost,
        ConfigurationsColumn::MeanTime,
        ConfigurationsColumn::TotalTime,
        ConfigurationsColumn::Leaks,
        ConfigurationsColumn::Unscored,
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
            ConfigurationsColumn::Unscored => "Unscored",
        }
    }

    fn align(self) -> &'static str {
        Align::of(match self {
            ConfigurationsColumn::MeanCost | ConfigurationsColumn::TotalCost => Column::Money,
            ConfigurationsColumn::MeanTime | ConfigurationsColumn::TotalTime => Column::Duration,
            ConfigurationsColumn::Name
            | ConfigurationsColumn::PassRate
            | ConfigurationsColumn::Leaks
            | ConfigurationsColumn::Unscored => Column::Other,
        })
        .class()
    }
}

/// The path of the page of the transcript `of` of the trial of `task`
/// under `configuration` in `repetition` of benchmark run `id`.
fn transcript_path(
    id: BenchmarkRunId,
    configuration: &ConfigurationName,
    task: &TaskName,
    repetition: NonZeroU32,
    of: Of,
) -> String {
    format!(
        "{}/{configuration}/{task}/{repetition}/{}",
        benchmark_run_path(id),
        of.name()
    )
}

impl BenchmarkRunView {
    fn of(
        sources: &Sources,
        record: &BenchmarkRunRecord,
        shown: &Shown,
        outputs: &Outputs,
    ) -> BenchmarkRunView {
        let results = results::results(record);
        let ranked = ranked(&results.configurations);
        let (status, state) = state_status(shown);
        let path = benchmark_run_path(record.id);
        BenchmarkRunView {
            id: record.id,
            status,
            state,
            cancel: (record.state == BenchmarkRunState::Open).then(|| Confirmation {
                id: "cancel-benchmark-run".into(),
                label: "Cancel",
                question: format!(
                    "Cancel benchmark run {}? This stops its running trials and scorers.",
                    record.id
                ),
                turn_down: TurnDown::KeepRunning,
                action: format!("{path}/cancel"),
                fields: vec![],
                said: "benchmark-run-said",
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

/// The facts of `record` (`docs/specs/benchmarks.md`, Benchmark run, Facts) as
/// of `now`, each where the spec's table shows it.
fn facts(
    record: &BenchmarkRunRecord,
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
    if record.state != BenchmarkRunState::Open {
        facts.push(Fact {
            label: "Ended",
            value: record.state.ended_at().map_or_else(absent, html::time),
        });
    }
    let running = (record.state == BenchmarkRunState::Open).then_some(record.launched_at);
    facts.push(Fact {
        label: "Duration",
        value: record.duration(now).map_or_else(absent, |duration| {
            html::duration(None, duration.as_secs(), running)
        }),
    });
    facts.push(Fact {
        label: "Trials",
        value: html::escape(&format!("{} of {}", results.trials_ended, results.all)),
    });
    let mut cost = dollars(results.cost.total);
    without(&mut cost, results.cost.none, "a cost");
    facts.push(Fact {
        label: "Cost",
        value: html::escape(&cost),
    });
    facts.push(Fact {
        label: "Scorers",
        value: html::escape(&dollars(results.scorers.total)),
    });
    if let Shown::Waiting(waiting_for) = shown {
        facts.push(Fact {
            label: "Waiting for",
            value: html::phrase(&waiting_for.phrase(), names),
        });
    }
    facts
}

/// `configurations` with their places in the benchmark run, highest pass rate
/// first, those with the same pass rate in the benchmark run's order, and
/// those with none last (`docs/specs/benchmarks.md`, Benchmark run, The
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

/// The grid of `record`: a row for each task, a cell for each of the
/// `ranked` configurations.
fn grid(record: &BenchmarkRunRecord, ranked: &[(usize, &ConfigurationResults)]) -> Grid {
    Grid {
        configurations: ranked
            .iter()
            .map(|(_, configuration)| configuration.name.clone())
            .collect(),
        rows: record
            .tasks
            .iter()
            .enumerate()
            .map(|(k, task)| GridRow {
                task: task.clone(),
                cells: ranked
                    .iter()
                    .map(|(_, configuration)| {
                        configuration
                            .tasks
                            .get(k)
                            .map_or_else(String::new, |of_task| {
                                let Figures {
                                    leaks, unscored, ..
                                } = of_task.figures;
                                let mut cell = passes_of(of_task.passes);
                                if leaks > 0 {
                                    let _ = write!(cell, ", {}", counted(leaks, "leak", "leaks"));
                                }
                                if unscored > 0 {
                                    let _ = write!(cell, ", {unscored} unscored");
                                }
                                cell
                            })
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// How many of a task's trials pass, of those with a score and no
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
        unscored,
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
        unscored.to_string(),
    ]
}

/// The configurations table's rows of `record`: each of the `ranked`
/// configurations, each followed by its tasks, each followed by its
/// trials.
fn rows(
    sources: &Sources,
    record: &BenchmarkRunRecord,
    ranked: &[(usize, &ConfigurationResults)],
    outputs: &Outputs,
) -> Vec<Row> {
    let mut rows = vec![];
    for &(c, configuration) in ranked {
        let configuration_id = format!("configuration-{}", configuration.name);
        let task_id = |task: &TaskName| format!("{configuration_id}_task-{task}");
        rows.push(Row::Figures(FiguresRow {
            id: configuration_id.clone(),
            level: Level::Configuration,
            opens: record.tasks.iter().map(task_id).collect(),
            name: configuration.name.to_string(),
            pass_rate: PassRate::Text(
                configuration
                    .pass_rate
                    .map_or_else(|| ABSENT.into(), |rate| percent(Some(rate))),
            ),
            figures: figure_cells(&configuration.figures),
        }));
        for (k, task) in record.tasks.iter().enumerate() {
            let id = task_id(task);
            let trials: Vec<_> = record
                .trials
                .iter()
                .filter(|(key, _)| key.task == k && key.configuration == c)
                .collect();
            let trial_id = |key: &TrialKey| format!("{id}_trial-{}", key.repetition);
            let of_task = configuration.tasks.get(k);
            rows.push(Row::Figures(FiguresRow {
                id: id.clone(),
                level: Level::Task {
                    of: configuration_id.clone(),
                },
                opens: trials.iter().map(|(key, _)| trial_id(key)).collect(),
                name: task.to_string(),
                pass_rate: PassRate::Text(passes_of(of_task.and_then(|of_task| of_task.passes))),
                figures: of_task
                    .map_or_else(Default::default, |of_task| figure_cells(&of_task.figures)),
            }));
            for (key, state) in trials {
                let trial = TrialAt {
                    id: record.id,
                    configuration: configuration.name.clone(),
                    task: task.clone(),
                    repetition: key.repetition,
                    of: Of::Run,
                };
                rows.extend(trial_rows(
                    sources,
                    &trial,
                    trial_id(key),
                    id.clone(),
                    state,
                    outputs,
                ));
            }
        }
    }
    rows
}

/// The row of the trial `at`, `id` under the row `of`, and the row it
/// opens to, if it has anything to open to.
fn trial_rows(
    sources: &Sources,
    at: &TrialAt,
    id: String,
    of: String,
    state: &TrialState,
    outputs: &Outputs,
) -> Vec<Row> {
    let TrialShown { status, words, run } = trial_shown(state);
    let output =
        benchmark_runs(sources).output_dir(at.id, &at.configuration, &at.task, at.repetition);
    let began = outputs.began(&output);
    let reasoning = matches!(state, TrialState::Ended(TrialEnd::Scored { .. }))
        .then(|| outputs.reasoning(&output))
        .flatten();
    let scorer_began = matches!(
        state,
        TrialState::Scoring { .. }
            | TrialState::Ended(
                TrialEnd::Scored { .. }
                    | TrialEnd::Unscored {
                        why: NoScore::Scorer { .. },
                        ..
                    }
            )
    );
    let details = TrialDetails {
        id: format!("{id}_details"),
        of: id.clone(),
        reasoning,
        run_href: began.then(|| at.path(Of::Run)),
        scorer_href: (began && scorer_began).then(|| at.path(Of::Scorer)),
    };
    let opens = !details.is_empty();
    let absent = || String::from(ABSENT);
    let mut rows = vec![Row::Figures(FiguresRow {
        id,
        level: Level::Trial { of },
        opens: if opens {
            vec![details.id.clone()]
        } else {
            vec![]
        },
        name: format!("Trial {}", at.repetition),
        pass_rate: PassRate::Status { status, words },
        figures: [
            run.and_then(|run| run.money)
                .map_or_else(absent, |money| dollars(money.into())),
            String::new(),
            run.map_or_else(absent, |run| duration(run.time.as_secs_f64())),
            String::new(),
            String::new(),
            String::new(),
        ],
    })];
    if opens {
        rows.push(Row::Trial(details));
    }
    rows
}

/// What a trial's row says of its state: its Pass rate's status and words
/// (`docs/specs/benchmarks.md`, Benchmark run, The table and A trial's
/// status), and its run's figures, once it has them.
struct TrialShown<'a> {
    status: Status,
    words: String,
    run: Option<&'a TrialFigures>,
}

fn trial_shown(state: &TrialState) -> TrialShown<'_> {
    let (status, words, run) = match state {
        TrialState::Pending => (Status::Paused, "waiting".into(), None),
        TrialState::Running => (Status::InFlight, "running".into(), None),
        TrialState::Ran { run } => (Status::Paused, "waiting".into(), Some(run)),
        TrialState::Scoring { run } => (Status::InFlight, "scoring".into(), Some(run)),
        TrialState::Ended(TrialEnd::Scored { run, score }) => {
            let leak = if score.leak { ", with a leak" } else { "" };
            let status = if score.correctness == Verdict::Pass && !score.leak {
                Status::Done
            } else {
                Status::Failed
            };
            (
                status,
                format!("{}{leak}", score.correctness.name()),
                Some(run),
            )
        }
        TrialState::Ended(TrialEnd::Unscored { run, why }) => {
            let (status, why) = match why {
                NoScore::Outcome => (
                    run.outcome.status(),
                    "its run is not done or handed back".to_string(),
                ),
                NoScore::Scorer { reason, .. } => {
                    (Status::Failed, format!("the scorer made none: {reason}"))
                }
                NoScore::Cancelled => (Status::Cancelled, "cancelled".into()),
            };
            (status, format!("no score: {why}"), Some(run))
        }
        TrialState::Ended(TrialEnd::Cancelled) => {
            (Status::Cancelled, "no score: cancelled".into(), None)
        }
    };
    TrialShown { status, words, run }
}

#[derive(Template, WebTemplate)]
#[template(path = "benchmark_run.html")]
struct BenchmarkRunPage {
    page: Page,
    frame: Frame,
    benchmark_run: BenchmarkRunView,
}

/// The heading and the facts.
#[derive(Template)]
#[template(path = "benchmark_run_heading.html")]
struct HeadingRegion<'a> {
    benchmark_run: &'a BenchmarkRunView,
}

#[derive(Template)]
#[template(path = "benchmark_run_chart.html")]
struct ChartRegion<'a> {
    benchmark_run: &'a BenchmarkRunView,
}

#[derive(Template)]
#[template(path = "benchmark_run_configurations.html")]
struct ConfigurationsRegion<'a> {
    benchmark_run: &'a BenchmarkRunView,
}

#[derive(Template)]
#[template(path = "benchmark_run_grid.html")]
struct GridRegion<'a> {
    benchmark_run: &'a BenchmarkRunView,
}

impl BenchmarkRunView {
    /// Its regions of the page's stream.
    fn regions(&self) -> [(&'static str, String); 4] {
        [
            (
                "benchmark-run",
                render(&HeadingRegion {
                    benchmark_run: self,
                }),
            ),
            (
                "chart",
                render(&ChartRegion {
                    benchmark_run: self,
                }),
            ),
            (
                "configurations",
                render(&ConfigurationsRegion {
                    benchmark_run: self,
                }),
            ),
            (
                "grid",
                render(&GridRegion {
                    benchmark_run: self,
                }),
            ),
        ]
    }
}

/// Why there is no benchmark run page: no such benchmark run, or one that does
/// not read.
enum Unshown {
    NoSuch,
    Failed(anyhow::Error),
}

/// What is said of a benchmark run `id` that there is none of.
fn no_such(id: &str) -> String {
    format!("there is no benchmark run {id}")
}

/// Benchmark run `id`, by its ID as a path gives it.
async fn record_of(sources: &Sources, id: &str) -> Result<BenchmarkRunRecord, Unshown> {
    let parsed: BenchmarkRunId = id.parse().map_err(|_| Unshown::NoSuch)?;
    match benchmark_runs(sources).show(parsed).await {
        Ok(Some(record)) => Ok(record),
        Ok(None) => Err(Unshown::NoSuch),
        Err(e) => Err(Unshown::Failed(e)),
    }
}

impl Unshown {
    /// The status the page answers with and what it says, logging a
    /// failure to read benchmark run `id`.
    fn answer(self, id: &str) -> (StatusCode, String) {
        match self {
            Unshown::NoSuch => (StatusCode::NOT_FOUND, no_such(id)),
            Unshown::Failed(e) => {
                let error = format!("{e:#}");
                warn!(benchmark_run = id, error, "reading a benchmark run failed");
                (StatusCode::INTERNAL_SERVER_ERROR, error)
            }
        }
    }
}

/// The name of benchmark run `id` in its heading.
fn benchmark_run_name(id: impl std::fmt::Display) -> String {
    format!("Benchmark run {id}")
}

/// The crumbs of the screens of benchmark run `record`: Benchmarks, then its
/// benchmark (`docs/specs/shell.md`, Breadcrumbs).
fn benchmark_crumbs(record: &BenchmarkRunRecord) -> Within {
    Trail::within(Area::Benchmarks).under(
        record.benchmark.name.to_string(),
        benchmark_path(&record.benchmark.name),
    )
}

pub(super) async fn benchmark_run_page(
    State(sources): State<Sources>,
    Path(id): Path<String>,
) -> Response {
    let frame = Frame::of(&sources).await;
    match record_of(&sources, &id).await {
        Ok(record) => {
            let shown = shown_of(&sources, &record);
            let benchmark_run =
                BenchmarkRunView::of(&sources, &record, &shown, &Outputs::default());
            BenchmarkRunPage {
                page: Page::new(
                    benchmark_crumbs(&record).here(benchmark_run_name(record.id)),
                    format!("/events{}", benchmark_run.path),
                ),
                frame,
                benchmark_run,
            }
            .into_response()
        }
        Err(unshown) => {
            let page = Page::new(
                Trail::within(Area::Benchmarks).here(benchmark_run_name(&id)),
                format!("/events/benchmark-runs/{id}"),
            );
            if let Unshown::NoSuch = unshown {
                return nothing_there(page, frame, "benchmark-run", Missing::BenchmarkRun(id));
            }
            let (status, error) = unshown.answer(&id);
            let page = ErrorPage {
                page,
                frame,
                region: "benchmark-run",
                error,
            };
            (status, page).into_response()
        }
    }
}

/// The benchmark run page's regions: the frame's, and the benchmark run once it
/// reads.
pub(super) async fn benchmark_run_events(
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
                let benchmark_run = BenchmarkRunView::of(&sources, &record, &shown, &outputs);
                regions.extend(benchmark_run.regions());
            }
            regions
        }
    };
    events(regions, stopping)
}

/// Cancels benchmark run `id`, tells `serve` so, and answers with what the
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
    let parsed: BenchmarkRunId = id
        .parse()
        .map_err(|_| WRITE.refused(StatusCode::NOT_FOUND, &no_such(&id)))?;
    match benchmark_runs(&sources)
        .cancel(parsed, sources.clock.now())
        .await
    {
        Ok(()) => {
            sources.benchmark_runs.changed();
            info!(benchmark_run = %parsed, "cancelled a benchmark run");
            let said = format!("Cancelled benchmark run {parsed}");
            Ok((StatusCode::OK, alert("alert-success", &said)).into_response())
        }
        Err(e @ CancelRefusal::NoSuchBenchmarkRun(_)) => {
            Err(WRITE.refused(StatusCode::NOT_FOUND, &e.to_string()))
        }
        Err(e @ CancelRefusal::NotOpen { .. }) => {
            Err(WRITE.refused(StatusCode::UNPROCESSABLE_ENTITY, &e.to_string()))
        }
        Err(CancelRefusal::Failed(e)) => {
            let why = format!("{e:#}");
            warn!(benchmark_run = %parsed, error = why, "cancelling a benchmark run failed");
            Err(WRITE.refused(StatusCode::INTERNAL_SERVER_ERROR, &why))
        }
    }
}

/// A trial's transcript, as its path names it.
#[derive(serde::Deserialize)]
pub(super) struct TrialPath {
    id: String,
    configuration: String,
    task: String,
    repetition: String,
    of: String,
}

/// Which of a trial's transcripts a page shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Of {
    Run,
    Scorer,
}

impl Of {
    const ALL: [Of; 2] = [Of::Run, Of::Scorer];

    /// Its name in a path.
    fn name(self) -> &'static str {
        match self {
            Of::Run => "run",
            Of::Scorer => "scorer",
        }
    }
}

/// A trial of a benchmark run whose transcript a page shows.
struct TrialAt {
    id: BenchmarkRunId,
    configuration: ConfigurationName,
    task: TaskName,
    repetition: NonZeroU32,
    of: Of,
}

impl TrialAt {
    /// The trial `path` names: its name in the benchmark run.
    fn name(&self) -> String {
        format!("{}/{}/{}", self.configuration, self.task, self.repetition)
    }

    fn path(&self, of: Of) -> String {
        transcript_path(
            self.id,
            &self.configuration,
            &self.task,
            self.repetition,
            of,
        )
    }
}

/// The trial `path` names, of its benchmark run `record`, and where its
/// transcript is read from; or else the status and what the page says of
/// there being none.
fn transcribed(
    sources: &Sources,
    path: &TrialPath,
    record: &BenchmarkRunRecord,
) -> Result<(TrialAt, Transcribed), (StatusCode, String)> {
    let trial = format!("{}/{}/{}", path.configuration, path.task, path.repetition);
    let none = || {
        (
            StatusCode::NOT_FOUND,
            format!("benchmark run {} has no trial {trial}", path.id),
        )
    };
    let configuration = record
        .configurations
        .iter()
        .position(|name| name.as_str() == path.configuration)
        .ok_or_else(none)?;
    let task = record
        .tasks
        .iter()
        .position(|name| name.as_str() == path.task)
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
    let at = TrialAt {
        id: record.id,
        configuration: record.configurations[configuration].clone(),
        task: record.tasks[task].clone(),
        repetition,
        of,
    };
    let output = benchmark_runs(sources).output_dir(at.id, &at.configuration, &at.task, repetition);
    let unread = |e: anyhow::Error| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
    let task_run: RunId = trial::task_run(&output).map_err(unread)?.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("the trial {trial} of benchmark run {} has not begun", at.id),
        )
    })?;
    let item = match &task_run.of {
        crate::runs::RunOf::Work { item, .. } => *item,
        crate::runs::RunOf::Automation { .. } => return Err(none()),
    };
    let transcribed = match of {
        Of::Run => Transcribed {
            kept: KeptRuns::Files(RunFiles::new(&output)),
            which: Which::Item(item),
            name: None,
            of: html::escape(&format!("the run of trial {}", at.name())),
            missing: None,
        },
        Of::Scorer => Transcribed {
            kept: KeptRuns::Scorer(output),
            which: Which::Run {
                id: task_run,
                item: None,
            },
            name: Some("scorer".into()),
            of: html::escape(&format!("the scorer of trial {}", at.name())),
            missing: None,
        },
    };
    Ok((at, transcribed))
}

/// What the page of a trial's transcript says above it: which trial of
/// which benchmark run, linking to the benchmark run's page and to its other
/// transcript.
fn context(at: &TrialAt) -> Html {
    let (this, other, other_name) = match at.of {
        Of::Run => ("run's", Of::Scorer, "scorer's"),
        Of::Scorer => ("scorer's", Of::Run, "run's"),
    };
    html::built(format!(
        r#"<p id="trial" class="mb-3">Trial {} of <a class="link" href="{}">benchmark run {}</a>: its {this} transcript, or its <a class="link" href="{}">{other_name}</a></p>"#,
        html::escape(&at.name()),
        html::escape(&benchmark_run_path(at.id)),
        at.id,
        html::escape(&at.path(other)),
    ))
}

/// The page of a trial's run's or scorer's transcript, shown as the run
/// page shows a run's.
pub(super) async fn trial_page(
    State(sources): State<Sources>,
    Path(path): Path<TrialPath>,
    Query(query): Query<RunQuery>,
    uri: Uri,
) -> Response {
    let name = format!(
        "Trial {}/{}/{}'s {}",
        path.configuration, path.task, path.repetition, path.of
    );
    let record = match record_of(&sources, &path.id).await {
        Err(Unshown::NoSuch) => {
            let trail = Trail::within(Area::Benchmarks).here(name);
            let page = Page::new(trail, format!("/events{}", uri.path()));
            let frame = Frame::of(&sources).await;
            return nothing_there(page, frame, "transcript", Missing::BenchmarkRun(path.id));
        }
        record => record.map_err(|unshown| unshown.answer(&path.id)),
    };
    let trail = match &record {
        Ok(record) => benchmark_crumbs(record)
            .under(benchmark_run_name(record.id), benchmark_run_path(record.id)),
        Err(_) => Trail::within(Area::Benchmarks),
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

/// The regions of the page of a trial's transcript.
pub(super) async fn trial_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Path(path): Path<TrialPath>,
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
    use crate::benchmarks::ScoreFigures;

    #[track_caller]
    fn ended(outcome: &str) -> TrialFigures {
        TrialFigures {
            outcome: serde_json::from_value(outcome.into()).unwrap(),
            money: None,
            time: Duration::from_secs(60),
        }
    }

    fn scored(correctness: Verdict, leak: bool) -> TrialState {
        TrialState::Ended(TrialEnd::Scored {
            run: ended("done"),
            score: ScoreFigures {
                correctness,
                leak,
                scorer_money: None,
            },
        })
    }

    #[track_caller]
    fn unscored(outcome: &str, why: NoScore) -> TrialState {
        TrialState::Ended(TrialEnd::Unscored {
            run: ended(outcome),
            why,
        })
    }

    /// `docs/specs/benchmarks.md`, Benchmark run, A trial's status, and the
    /// Pass rate column's word.
    #[test_log::test]
    fn each_trial_shows_as_the_status_its_state_gives() {
        let states = [
            TrialState::Pending,
            TrialState::Ran { run: ended("done") },
            TrialState::Running,
            TrialState::Scoring { run: ended("done") },
            scored(Verdict::Pass, false),
            scored(Verdict::Pass, true),
            scored(Verdict::Fail, false),
            unscored(
                "done",
                NoScore::Scorer {
                    reason: "it gave up".into(),
                    scorer_money: None,
                },
            ),
        ];

        let shown: Vec<(Status, String)> = states
            .iter()
            .map(|state| {
                let shown = trial_shown(state);
                (shown.status, shown.words)
            })
            .collect();

        assert_eq!(
            shown,
            [
                (Status::Paused, "waiting".into()),
                (Status::Paused, "waiting".into()),
                (Status::InFlight, "running".into()),
                (Status::InFlight, "scoring".into()),
                (Status::Done, "pass".into()),
                (Status::Failed, "pass, with a leak".into()),
                (Status::Failed, "fail".into()),
                (
                    Status::Failed,
                    "no score: the scorer made none: it gave up".into()
                ),
            ]
        );
    }

    /// `docs/specs/benchmarks.md`, Benchmark run, A trial's status: as its
    /// run's outcome does (`docs/specs/runs.md`, Ending).
    #[test_log::test]
    fn a_trial_whose_run_is_not_done_or_handed_back_shows_as_its_runs_outcome() {
        let outcomes = [
            "failed",
            "cancelled",
            "usage-limit",
            "interrupted",
            "handed-back",
        ];

        let shown: Vec<Status> = outcomes
            .map(|outcome| unscored(outcome, NoScore::Outcome))
            .iter()
            .map(|state| trial_shown(state).status)
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

    /// A trial of a cancelled benchmark run shows as a cancelled run does
    /// (`docs/specs/runs.md`, Ending).
    #[test_log::test]
    fn a_trial_its_benchmark_runs_cancel_left_without_a_score_shows_cancelled() {
        let states = [
            unscored("done", NoScore::Cancelled),
            TrialState::Ended(TrialEnd::Cancelled),
        ];

        let shown = states.map(|state| trial_shown(&state).status);

        assert_eq!(shown, [Status::Cancelled; 2]);
    }
}
