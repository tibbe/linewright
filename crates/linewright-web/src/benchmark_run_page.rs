//! The benchmark run page (`docs/specs/benchmarks.md`, Benchmark run):
//! benchmark run ID's heading, then its results as a chart, a grid and a table
//! whose trials open to the transcripts of their runs and their scorers; and
//! the cancelling of an open benchmark run.

use std::fmt::Write as _;
use std::ops::Range;

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use tracing::warn;

use linewright_core::access::{Permit, Read};
use linewright_core::actions::{
    self, Action, BenchmarkRunScreen, NoTrial, Outputs, TrialAt, TrialSaid, TrialScreen,
    TrialTranscript,
};
use linewright_core::api::Sources;
use linewright_core::benchmarks::results::{ConfigurationResults, Figures, Passes};
use linewright_core::benchmarks::{
    BenchmarkOfRun, BenchmarkRunId, BenchmarkRunRecord, CancelRefusal, RunTask, Shown, TrialAnswer,
    TrialName, dollars, duration, percent, without,
};
use linewright_core::name::Name;
use linewright_core::phrase::Status;

use super::buttons::{ScreenButton, Verb};
use super::caller::{Asking, Reader};
use super::components::{CardTitle, HeadingLevel};
use super::confirmation::{Confirmation, TurnDown};
use super::editing::{self, EditDialog};
use super::html::{self, Html};
use super::icons::CardIcon;
use super::launch::counted;
use super::names::Naming;
use super::pages::{PageHref, Pages, PagesShown, Requested};
use super::run_page::{PageOf, RunQuery, Transcribed, transcript_events, transcript_page};
use super::shell::{self, Area, Frame, Heading, Missing, Navigation, Place, Thing, Within};
use super::stats::{ABSENT, Stat, absent};
use super::table::{Align, Column, Fold};
use super::write::{Refused, Write, alert};
use super::{
    ErrorPage, Page, benchmark_path, benchmark_run_path, events, nothing_there, render,
    with_filters, with_query,
};

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

/// The benchmark run as the page shows it.
struct BenchmarkRunView {
    id: BenchmarkRunId,
    status: Status,
    state: &'static str,
    /// Its cancelling, while it is open (`docs/specs/benchmarks.md`,
    /// Benchmark run, Heading).
    cancel: Option<Confirmation>,
    facts: Vec<Stat>,
    chart: [Plot; 2],
    /// A page of the configurations' rows, ranked, each followed by the rows
    /// it opens to.
    rows: Vec<Row>,
    configurations_nav: Option<PagesShown>,
    /// A page of the grid's rows.
    grid: Grid,
    grid_nav: Option<PagesShown>,
}

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

    /// Its scale where the largest mean is 0: $1 for cost and 1 second for
    /// time (`docs/specs/benchmarks.md`, Benchmark run, The plot, The scale).
    fn least_scale(self) -> f64 {
        match self {
            Axis::Cost | Axis::Time => 1.0,
        }
    }

    /// `value` along it, in words.
    fn words(self, value: f64) -> String {
        match self {
            Axis::Cost => dollars(value),
            Axis::Time => duration(value),
        }
    }

    /// `value` along it as a tick's label: in words, but for a time under 10
    /// seconds, which is to a tenth of a second so that no two ticks read
    /// alike.
    fn tick(self, value: f64) -> String {
        match self {
            Axis::Time if value < 10.0 => format!("{}s", (value * 10.0).round() / 10.0),
            Axis::Cost | Axis::Time => self.words(value),
        }
    }
}

/// The chart along an axis: its ticks and its points, or none with no
/// point to show.
struct Plot {
    axis: Axis,
    drawn: Option<Drawn>,
}

/// What a plot with points draws: the lines across it at a pass rate of 100%
/// and of 50%, its axes, its ticks along the pass rate and along the mean,
/// and its points.
///
/// Each place in it is a percentage of its frame's width and height, from
/// its axes' corner at 0% across and 100% down, and an offset from it in
/// pixels. Its ticks are in the margin around the frame.
struct Drawn {
    rules: Vec<Line>,
    axes: Vec<Line>,
    rate_ticks: Vec<Tick>,
    mean_ticks: Vec<Tick>,
    points: Vec<Point>,
}

/// A line of the chart, from `x1`, `y1` to `x2`, `y2`, in percent of the
/// frame.
struct Line {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

/// A tick's label, at `x`, `y` in percent of the frame, moved by `dx`, `dy`
/// pixels.
struct Tick {
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    anchor: &'static str,
    label: String,
}

/// A configuration's point (`docs/specs/benchmarks.md`, Benchmark run, A
/// point), at `x`, `y` in percent of the frame. It is drawn in a
/// `foreignObject` `POINT_WIDTH` by `POINT_HEIGHT` pixels, `dx`, `dy` pixels
/// from its place, so that its dot sits at that place.
struct Point {
    id: String,
    name: Name,
    tip: String,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    /// Whether its name is at its dot's left, so that it stays inside the
    /// chart.
    named_left: bool,
}

struct Grid {
    configurations: Vec<Name>,
    rows: Vec<GridRow>,
}

struct GridRow {
    task: Name,
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
    fold: Fold,
    name: String,
    pass_rate: PassRate,
    /// Its cells under `ConfigurationsColumn::FIGURES`.
    figures: [String; 6],
    /// The id of the dialog its "Edit" opens: a configuration's but `as-is`
    /// (`docs/specs/benchmarks.md`, Benchmark runs, The configurations).
    edit: Option<String>,
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
        words: &'static str,
    },
}

/// What a trial's row opens to (`docs/specs/benchmarks.md`, Benchmark run,
/// A trial's row): its score's lines or its reason for none, and the links
/// to the transcripts that have started, each under its heading.
struct TrialDetails {
    id: String,
    of: String,
    said: Option<Said>,
    run_href: Option<String>,
    scorer_href: Option<String>,
}

impl TrialDetails {
    /// Whether it holds nothing to open to.
    fn is_empty(&self) -> bool {
        self.said.is_none() && self.run_href.is_none() && self.scorer_href.is_none()
    }
}

/// What a trial's details say of its score: its lines (`docs/specs/benchmarks.md`,
/// Benchmark run, A trial's score), or its reason for none.
enum Said {
    Score {
        correctness: ScoreLine,
        leak: ScoreLine,
        /// The tool calls that show its leak, as far as the output holds
        /// them.
        calls: Vec<String>,
    },
    Reason(String),
}

/// A line of a trial's score: its label, its value, then the scorer's
/// reasoning, as far as the output holds it.
struct ScoreLine {
    label: &'static str,
    value: &'static str,
    reasoning: Option<String>,
}

impl Said {
    /// What `trial`'s details say of its score: its lines, with what the
    /// scorer said of it, or its reason for none.
    fn of(trial: &TrialScreen) -> Option<Said> {
        match (&trial.trial.state, &trial.said) {
            (
                TrialAnswer::Scored {
                    correctness, leak, ..
                },
                said,
            ) => {
                let (correctness_reasoning, leak_reasoning, calls) = match said {
                    Some(TrialSaid::Reasoning {
                        correctness_reasoning,
                        leak_reasoning,
                        leak_calls,
                    }) => (
                        Some(correctness_reasoning.clone()),
                        Some(leak_reasoning.clone()),
                        leak_calls.clone(),
                    ),
                    None | Some(TrialSaid::NoScore { .. }) => (None, None, vec![]),
                };
                Some(Said::Score {
                    correctness: ScoreLine {
                        label: "Correctness",
                        value: correctness.name(),
                        reasoning: correctness_reasoning,
                    },
                    leak: ScoreLine {
                        label: "Leak",
                        value: if *leak { "yes" } else { "no" },
                        reasoning: leak_reasoning,
                    },
                    calls,
                })
            }
            (_, Some(TrialSaid::NoScore { no_score })) => Some(Said::Reason(no_score.clone())),
            (_, None | Some(TrialSaid::Reasoning { .. })) => None,
        }
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
    /// "Edit", at the end of a configuration's row.
    Edit,
}

impl ConfigurationsColumn {
    const ALL: [ConfigurationsColumn; 9] = [
        ConfigurationsColumn::Name,
        ConfigurationsColumn::PassRate,
        ConfigurationsColumn::MeanCost,
        ConfigurationsColumn::TotalCost,
        ConfigurationsColumn::MeanTime,
        ConfigurationsColumn::TotalTime,
        ConfigurationsColumn::Leaks,
        ConfigurationsColumn::Unscored,
        ConfigurationsColumn::Edit,
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
            ConfigurationsColumn::Name => "Configuration",
            ConfigurationsColumn::PassRate => "Pass rate",
            ConfigurationsColumn::MeanCost => "Mean cost",
            ConfigurationsColumn::TotalCost => "Total cost",
            ConfigurationsColumn::MeanTime => "Mean time",
            ConfigurationsColumn::TotalTime => "Total time",
            ConfigurationsColumn::Leaks => "Leaks",
            ConfigurationsColumn::Unscored => "Unscored",
            ConfigurationsColumn::Edit => "",
        }
    }

    fn align(self) -> &'static str {
        Align::of(match self {
            ConfigurationsColumn::MeanCost | ConfigurationsColumn::TotalCost => Column::Money,
            ConfigurationsColumn::MeanTime | ConfigurationsColumn::TotalTime => Column::Duration,
            ConfigurationsColumn::Name
            | ConfigurationsColumn::PassRate
            | ConfigurationsColumn::Leaks
            | ConfigurationsColumn::Unscored
            | ConfigurationsColumn::Edit => Column::Other,
        })
        .class()
    }
}

/// The path of the page of the transcript `of` of the trial of `task`
/// under `configuration` in `repetition` of benchmark run `id`.
fn transcript_path(at: &TrialAt, of: TrialTranscript) -> String {
    format!(
        "{}/{}/{}/{}/{}",
        benchmark_run_path(at.benchmark_run),
        at.configuration,
        at.task,
        at.repetition,
        of.name()
    )
}

impl BenchmarkRunView {
    /// Of `benchmark_run`, on the pages `paging` asks for, its links to
    /// other pages leading to the stream of a page that draws the dialog
    /// "Cancel" opens where `cancels`.
    fn of(
        permit: &Permit<Read>,
        sources: &Sources,
        benchmark_run: &BenchmarkRunScreen,
        paging: Paging,
        cancels: bool,
    ) -> BenchmarkRunView {
        let ranked = ranked(&benchmark_run.results.configurations);
        let (status, state) = state_status(&benchmark_run.shown);
        let path = benchmark_run_path(benchmark_run.id);
        let configurations_page = Pages::of(ranked.len(), paging.configurations);
        let grid_page = Pages::of(benchmark_run.tasks.len(), paging.tasks);
        let href = |asked: Paging| asked.href(&path, cancels);
        BenchmarkRunView {
            id: benchmark_run.id,
            status,
            state,
            cancel: benchmark_run.open.then(|| Confirmation {
                id: "cancel-benchmark-run".into(),
                verb: Verb::CancelWork,
                question: format!(
                    "Cancel benchmark run {}? This stops its running trials and scorers.",
                    benchmark_run.id
                ),
                turn_down: TurnDown::KeepRunning,
                action: format!("{path}/cancel"),
                fields: vec![],
                said: "benchmark-run-said",
            }),
            configurations_nav: configurations_page.shown("configurations", |page| {
                href(Paging {
                    configurations: page,
                    ..paging
                })
            }),
            grid_nav: grid_page.shown("tasks", |page| {
                href(Paging {
                    tasks: page,
                    ..paging
                })
            }),
            facts: facts(
                benchmark_run,
                &Naming::Titled {
                    repo: &sources.work_dir.repo,
                    snapshot: None,
                },
            ),
            chart: [Axis::Cost, Axis::Time].map(|axis| plot(axis, &ranked)),
            rows: rows(permit, benchmark_run, &ranked[configurations_page.range()]),
            grid: grid(benchmark_run, &ranked, grid_page.range()),
        }
    }
}

/// The facts of `benchmark_run` (`docs/specs/benchmarks.md`, Benchmark
/// run, Facts), each where the spec's table shows it.
fn facts(benchmark_run: &BenchmarkRunScreen, naming: &Naming) -> Vec<Stat> {
    let results = &benchmark_run.results;
    let mut facts = vec![Stat {
        label: "Launched",
        value: html::time(benchmark_run.launched_at),
    }];
    let open = benchmark_run.open;
    if !open {
        facts.push(Stat {
            label: "Ended",
            value: benchmark_run.ended_at.map_or_else(absent, html::time),
        });
    }
    let running = open.then_some(benchmark_run.launched_at);
    facts.push(Stat {
        label: "Duration",
        value: benchmark_run
            .duration_seconds
            .map_or_else(absent, |seconds| html::duration(None, seconds, running)),
    });
    facts.push(Stat {
        label: "Trials",
        value: html::escape(&format!("{} of {}", results.trials_ended, results.all)),
    });
    let mut cost = dollars(results.cost.total);
    without(&mut cost, results.cost.none, "a cost");
    facts.push(Stat {
        label: "Cost",
        value: html::escape(&cost),
    });
    facts.push(Stat {
        label: "Scorers",
        value: html::escape(&dollars(results.scorers.total)),
    });
    if let Shown::Waiting(waiting_for) = &benchmark_run.shown {
        facts.push(Stat {
            label: "Waiting for",
            value: html::phrase(&waiting_for.phrase(), naming),
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
    let scale = if most > 0.0 {
        most * 1.1
    } else {
        axis.least_scale()
    };
    let x = |mean: f64| mean / scale * 100.0;
    let y = |rate: f64| (1.0 - rate) * 100.0;
    let rate_ticks = [0.0, 0.5, 1.0]
        .into_iter()
        .map(|rate| Tick {
            x: 0.0,
            y: y(rate),
            dx: -6.0,
            dy: 4.0,
            anchor: "end",
            label: percent(Some(rate)),
        })
        .collect();
    let mean_ticks = [(0.0, "middle"), (0.5, "middle"), (1.0, "end")]
        .into_iter()
        .map(|(share, anchor)| Tick {
            x: x(share * scale),
            y: 100.0,
            dx: 0.0,
            dy: 18.0,
            anchor,
            label: axis.tick(share * scale),
        })
        .collect();
    let points = at
        .into_iter()
        .map(|(configuration, mean, rate)| {
            let named_left = x(mean) > 60.0;
            Point {
                id: format!("point-{}-{}", axis.name(), configuration.id),
                name: configuration.name.clone(),
                tip: format!(
                    "{}: pass rate {}, {} {}",
                    configuration.name,
                    percent(Some(rate)),
                    axis.label().to_lowercase(),
                    axis.words(mean)
                ),
                x: x(mean),
                y: y(rate),
                dx: if named_left {
                    POINT_HEIGHT / 2.0 - POINT_WIDTH
                } else {
                    -POINT_HEIGHT / 2.0
                },
                dy: -POINT_HEIGHT / 2.0,
                named_left,
            }
        })
        .collect();
    let across = |height: f64| Line {
        x1: 0.0,
        y1: height,
        x2: 100.0,
        y2: height,
    };
    Plot {
        axis,
        drawn: Some(Drawn {
            rules: vec![across(0.0), across(50.0)],
            axes: vec![
                Line {
                    x1: 0.0,
                    y1: 0.0,
                    x2: 0.0,
                    y2: 100.0,
                },
                across(100.0),
            ],
            rate_ticks,
            mean_ticks,
            points,
        }),
    }
}

/// The grid of `benchmark_run`: a row for each of its tasks in `tasks`, by
/// index, a cell for each of the `ranked` configurations.
fn grid(
    benchmark_run: &BenchmarkRunScreen,
    ranked: &[(usize, &ConfigurationResults)],
    tasks: Range<usize>,
) -> Grid {
    Grid {
        configurations: ranked
            .iter()
            .map(|(_, configuration)| configuration.name.clone())
            .collect(),
        rows: benchmark_run.tasks[tasks.clone()]
            .iter()
            .zip(tasks)
            .map(|(task, k)| GridRow {
                task: task.name.clone(),
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

/// The configurations table's rows of `benchmark_run`: each of the
/// `ranked` configurations, each followed by its tasks, each followed by
/// its trials.
fn rows(
    permit: &Permit<Read>,
    benchmark_run: &BenchmarkRunScreen,
    ranked: &[(usize, &ConfigurationResults)],
) -> Vec<Row> {
    let mut rows = vec![];
    for &(_, configuration) in ranked {
        let configuration_id = format!("configuration-{}", configuration.id);
        let task_id = |task: &RunTask| format!("{configuration_id}_task-{}", task.id);
        rows.push(Row::Figures(FiguresRow {
            id: configuration_id.clone(),
            level: Level::Configuration,
            fold: Fold::of(
                benchmark_run.tasks.iter().map(task_id).collect(),
                format!("Show the tasks of {}", configuration.name),
            ),
            name: configuration.name.to_string(),
            pass_rate: PassRate::Text(
                configuration
                    .pass_rate
                    .map_or_else(|| ABSENT.into(), |rate| percent(Some(rate))),
            ),
            figures: figure_cells(&configuration.figures),
            edit: editing::edit_configuration_id(configuration.id)
                .filter(|_| permit.caller().can(Action::EditConfiguration.level())),
        }));
        for (k, task) in benchmark_run.tasks.iter().enumerate() {
            let id = task_id(task);
            let trials = benchmark_run.trials_of(task.id, configuration.id);
            let trial_id = |trial: &TrialScreen| format!("{id}_trial-{}", trial.trial.repetition);
            let of_task = configuration.tasks.get(k);
            rows.push(Row::Figures(FiguresRow {
                id: id.clone(),
                level: Level::Task {
                    of: configuration_id.clone(),
                },
                fold: Fold::of(
                    trials.iter().map(|trial| trial_id(trial)).collect(),
                    format!(
                        "Show the trials of {} under {}",
                        task.name, configuration.name
                    ),
                ),
                name: task.name.to_string(),
                pass_rate: PassRate::Text(passes_of(of_task.and_then(|of_task| of_task.passes))),
                figures: of_task
                    .map_or_else(Default::default, |of_task| figure_cells(&of_task.figures)),
                edit: None,
            }));
            for trial in trials {
                let at = TrialAt {
                    benchmark_run: benchmark_run.id,
                    configuration: configuration.id,
                    task: task.id,
                    repetition: trial.trial.repetition,
                };
                rows.extend(trial_rows(&at, trial_id(trial), id.clone(), trial));
            }
        }
    }
    rows
}

/// The row of `trial`, at `at`, `id` under the row `of`, and the row it
/// opens to, if it has anything to open to.
fn trial_rows(at: &TrialAt, id: String, of: String, trial: &TrialScreen) -> Vec<Row> {
    let details = TrialDetails {
        id: format!("{id}_details"),
        of: id.clone(),
        said: Said::of(trial),
        run_href: trial
            .run_began
            .then(|| transcript_path(at, TrialTranscript::Run)),
        scorer_href: trial
            .scorer_began
            .then(|| transcript_path(at, TrialTranscript::Scorer)),
    };
    let opens = !details.is_empty();
    let absent = || String::from(ABSENT);
    let state = &trial.trial.state;
    let mut rows = vec![Row::Figures(FiguresRow {
        id,
        level: Level::Trial { of },
        fold: Fold::of(
            opens.then(|| details.id.clone()).into_iter().collect(),
            format!("Show the details of {}", trial.trial.name),
        ),
        name: format!("Trial {}", at.repetition),
        pass_rate: PassRate::Status {
            status: trial.shows_as,
            words: trial.status,
        },
        figures: [
            state
                .run()
                .and_then(|run| run.cost)
                .map_or_else(absent, |cost| dollars(cost.into())),
            String::new(),
            state.seconds().map_or_else(absent, duration),
            String::new(),
            String::new(),
            String::new(),
        ],
        edit: None,
    })];
    if opens {
        rows.push(Row::Trial(details));
    }
    rows
}

#[derive(Template, WebTemplate)]
#[template(path = "benchmark_run.html")]
struct BenchmarkRunPage {
    page: Page,
    frame: Frame,
    benchmark_run: BenchmarkRunView,
    /// The Edit configuration dialog of each configuration but `as-is`.
    edits: Vec<EditDialog>,
    /// The values they start from.
    edit_values: Html,
}

/// The chart's card's title, the label of the choice of its horizontal axis
/// (`docs/specs/benchmarks.md`, Benchmark run, The chart's look).
pub(super) fn chart_title() -> CardTitle {
    CardTitle {
        level: HeadingLevel::H2,
        icon: Some(CardIcon::Chart),
        name: html::built(r#"<label for="chart-axis">Pass rate vs</label>"#.to_string()),
    }
}

#[derive(Template)]
#[template(path = "benchmark_run_facts.html")]
struct FactsRegion<'a> {
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

/// The query of the stream of a page that draws the dialog "Cancel" opens.
const CANCELS: &str = "cancels";

/// What the benchmark run page's stream is asked.
#[derive(serde::Deserialize)]
pub(super) struct StreamQuery {
    /// Set where the page draws the dialog "Cancel" opens.
    cancels: Option<String>,
    #[serde(flatten)]
    paging: Paging,
}

/// The page of each of the page's tables that its address asks for: of
/// Configurations and of the grid's tasks.
#[derive(Clone, Copy, serde::Deserialize)]
pub(super) struct Paging {
    #[serde(rename = "configurations-page", default)]
    configurations: Requested,
    #[serde(rename = "tasks-page", default)]
    tasks: Requested,
}

impl Paging {
    /// Where a link to these pages of the benchmark run at `path` leads, its
    /// stream that of a page that draws the dialog "Cancel" opens where
    /// `cancels`.
    fn href(self, path: &str, cancels: bool) -> PageHref {
        let query = with_filters(
            "",
            &[
                ("configurations-page", &self.configurations.field()),
                ("tasks-page", &self.tasks.field()),
            ],
        );
        let query = query.trim_start_matches('?');
        let stream_query = if cancels {
            [CANCELS, query]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("&")
        } else {
            query.to_string()
        };
        PageHref {
            page: with_query(path, query),
            stream: with_query(&format!("/events{path}"), &stream_query),
        }
    }
}

/// The name of Benchmark run's screen, its last crumb.
const BENCHMARK_RUN: &str = "Benchmark run";

impl BenchmarkRunView {
    /// Its page, of `record`: under its benchmark, its heading its name and
    /// its state, and "Cancel" among the screen's buttons while it is open
    /// (`docs/specs/benchmarks.md`, Benchmark run, Heading), where `cancels`,
    /// the page drawing the dialog "Cancel" opens; its stream's on the pages
    /// `paging` asks for.
    fn page(
        &self,
        benchmark: &BenchmarkOfRun,
        cancels: bool,
        paging: Paging,
        navigation: Navigation,
    ) -> Page {
        let heading = Heading {
            icon: Some(CardIcon::Beaker),
            state: Some((self.status, self.state)),
            ..Heading::named(benchmark_run_name(self.id))
        };
        let buttons = self
            .cancel
            .iter()
            .filter(|_| cancels)
            .map(|cancel| ScreenButton::Act {
                verb: Verb::CancelWork,
                dialog: cancel.id.clone(),
                action: Action::CancelBenchmarkRun,
            })
            .collect();
        Page::new(
            benchmark_crumbs(benchmark).screen(BENCHMARK_RUN, Thing::Found(heading)),
            paging.href(&benchmark_run_path(self.id), cancels).stream,
            navigation,
        )
        .buttons(buttons)
    }

    /// Its regions of the page's stream.
    fn regions(&self) -> [(&'static str, String); 4] {
        [
            (
                "benchmark-run",
                render(&FactsRegion {
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
async fn record_of(
    permit: &Permit<Read>,
    sources: &Sources,
    id: &str,
) -> Result<BenchmarkRunRecord, Unshown> {
    let parsed: BenchmarkRunId = id.parse().map_err(|_| Unshown::NoSuch)?;
    match actions::benchmark_run_record(permit, sources, parsed).await {
        Ok(Some(record)) => Ok(record),
        Ok(None) => Err(Unshown::NoSuch),
        Err(e) => Err(Unshown::Failed(e)),
    }
}

/// Benchmark run `id`, by its ID as a path gives it, as its screen shows
/// it, what the trials' outputs hold read through `outputs`.
async fn screen_of(
    permit: &Permit<Read>,
    sources: &Sources,
    id: &str,
    outputs: &Outputs,
) -> Result<BenchmarkRunScreen, Unshown> {
    let parsed: BenchmarkRunId = id.parse().map_err(|_| Unshown::NoSuch)?;
    match actions::get_benchmark_run(permit, sources, parsed, outputs).await {
        Ok(Some(benchmark_run)) => Ok(benchmark_run),
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

/// The crumbs of the screens of a benchmark run of `benchmark`:
/// Benchmarks, then its benchmark (`docs/specs/shell.md`, Breadcrumbs).
fn benchmark_crumbs(benchmark: &BenchmarkOfRun) -> Within {
    Place::within(Area::Benchmarks).under(benchmark.name.to_string(), benchmark_path(benchmark.id))
}

pub(super) async fn benchmark_run_page(
    Reader { permit, sources }: Reader,
    Path(id): Path<String>,
    Query(paging): Query<Paging>,
    navigation: Navigation,
) -> Response {
    let frame = Frame::of(&permit, &sources).await;
    match screen_of(&permit, &sources, &id, &Outputs::default()).await {
        Ok(screen) => {
            let cancels = screen.open;
            let benchmark_run = BenchmarkRunView::of(&permit, &sources, &screen, paging, cancels);
            BenchmarkRunPage {
                page: benchmark_run.page(&screen.benchmark, cancels, paging, navigation),
                frame,
                benchmark_run,
                edits: editing::edit_configurations(&screen),
                edit_values: html::built(editing::configuration_values(&screen)),
            }
            .into_response()
        }
        Err(unshown) => {
            let events = paging.href(&format!("/benchmark-runs/{id}"), false).stream;
            let page = |thing| {
                Page::new(
                    Place::within(Area::Benchmarks).screen(BENCHMARK_RUN, thing),
                    events,
                    navigation,
                )
            };
            if let Unshown::NoSuch = unshown {
                return nothing_there(page, Missing::BenchmarkRun(id), frame, "benchmark-run");
            }
            let page = page(Thing::Found(Heading {
                icon: Some(CardIcon::Beaker),
                ..Heading::named(benchmark_run_name(&id))
            }));
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

/// The benchmark run page's regions: the notices, and once the benchmark run
/// reads, the rest of the shell's, the benchmark run's and the values its
/// Edit configuration dialogs start from.
pub(super) async fn benchmark_run_events(
    Reader { permit, sources }: Reader,
    watch: super::caller::Watch,
    Path(id): Path<String>,
    Query(query): Query<StreamQuery>,
    navigation: Navigation,
) -> Response {
    let cancels = query.cancels.is_some();
    let paging = query.paging;
    let outputs = Outputs::default();
    let regions = move || {
        let (permit, sources) = (permit.clone(), sources.clone());
        let (id, outputs) = (id.clone(), outputs.clone());
        async move {
            let frame = Frame::of(&permit, &sources).await;
            let Ok(screen) = screen_of(&permit, &sources, &id, &outputs).await else {
                return vec![shell::holds(&frame)];
            };
            let benchmark_run = BenchmarkRunView::of(&permit, &sources, &screen, paging, cancels);
            let mut regions = shell::regions(
                &frame,
                &benchmark_run.page(&screen.benchmark, cancels, paging, navigation),
            );
            regions.extend(benchmark_run.regions());
            regions.push((
                editing::EDIT_CONFIGURATIONS_VALUES,
                editing::configuration_values(&screen),
            ));
            regions
        }
    };
    events(regions, watch)
}

/// Cancels benchmark run `id`, tells `serve` so, and answers with what the
/// page says of it. Refused, cancelling nothing, unless the browser shows
/// the request came from the web UI's own pages.
pub(super) async fn cancel(
    State(sources): State<Sources>,
    asking: Asking,
    Path(id): Path<String>,
    uri: Uri,
) -> Result<Response, Refused> {
    const WRITE: Write = Write::Cancel;
    let permit = WRITE.permit(Action::CancelBenchmarkRun, &asking, &uri)?;
    let parsed: BenchmarkRunId = id
        .parse()
        .map_err(|_| WRITE.refused(StatusCode::NOT_FOUND, &no_such(&id)))?;
    match actions::cancel_benchmark_run(&permit, &sources, parsed).await {
        Ok(()) => {
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
            Err(WRITE.refused(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")))
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

impl TrialPath {
    /// The trial and its transcript that it names of benchmark run `id`;
    /// `None` where a part names none.
    fn parsed(&self, id: BenchmarkRunId) -> Option<(TrialAt, TrialTranscript)> {
        let at = TrialAt {
            benchmark_run: id,
            configuration: self.configuration.parse().ok()?,
            task: self.task.parse().ok()?,
            repetition: self.repetition.parse().ok()?,
        };
        let of = TrialTranscript::ALL
            .into_iter()
            .find(|of| of.name() == self.of)?;
        Some((at, of))
    }

    /// The trial, as the path names it.
    fn trial(&self) -> String {
        format!("{}/{}/{}", self.configuration, self.task, self.repetition)
    }
}

/// The transcript of a trial that a page shows.
struct TrialShown {
    at: TrialAt,
    name: TrialName,
    of: TrialTranscript,
}

/// What the page of a trial's transcript says above it: which trial of
/// which benchmark run, linking to the benchmark run's page and to its other
/// transcript.
fn context(shown: &TrialShown) -> Html {
    let (this, other, other_name) = match shown.of {
        TrialTranscript::Run => ("run's", TrialTranscript::Scorer, "scorer's"),
        TrialTranscript::Scorer => ("scorer's", TrialTranscript::Run, "run's"),
    };
    html::built(format!(
        r#"<span id="trial">{} of <a class="link" href="{}">benchmark run {}</a>: its {this} transcript, or its <a class="link" href="{}">{other_name}</a></span>"#,
        html::escape(&shown.name.capitalized()),
        html::escape(&benchmark_run_path(shown.at.benchmark_run)),
        shown.at.benchmark_run,
        html::escape(&transcript_path(&shown.at, other)),
    ))
}

/// The name of the screen of a trial's transcript, its last crumb.
const TRIAL: &str = "Trial";

/// The crumbs of the screens of benchmark run `record`'s trials.
fn trial_crumbs(record: &BenchmarkRunRecord) -> Within {
    benchmark_crumbs(&(&record.benchmark).into())
        .under(benchmark_run_name(record.id), benchmark_run_path(record.id))
}

/// The trial's transcript that `path` names, if its benchmark run has it,
/// and where it is read from; or else the status and what the page says of
/// there being none.
type Lookup = Result<(TrialShown, Transcribed), (StatusCode, String)>;

/// The crumbs of the page of the trial's transcript that `path` names, of
/// its benchmark run `id`, and its [`Lookup`]; or else that there is no
/// such benchmark run.
async fn looked_up(
    permit: &Permit<Read>,
    sources: &Sources,
    path: &TrialPath,
    id: BenchmarkRunId,
) -> Result<(Within, Lookup), Missing> {
    let missing = || Missing::BenchmarkRun(path.id.clone());
    let found = match path.parsed(id) {
        Some((at, of)) => match actions::find_trial(permit, sources, &at).await {
            Ok(Ok(found)) => Some((at, of, found)),
            Ok(Err(NoTrial::BenchmarkRun)) => return Err(missing()),
            Ok(Err(NoTrial::Trial)) => None,
            Err(e) => {
                let error = format!("{e:#}");
                warn!(
                    benchmark_run = path.id,
                    trial = path.trial(),
                    error,
                    "reading a trial failed"
                );
                let failed = (StatusCode::INTERNAL_SERVER_ERROR, error);
                return Ok((Place::within(Area::Benchmarks), Err(failed)));
            }
        },
        None => None,
    };
    let Some((at, of, found)) = found else {
        let none = (
            StatusCode::NOT_FOUND,
            format!("benchmark run {} has no trial {}", path.id, path.trial()),
        );
        return match record_of(permit, sources, &path.id).await {
            Ok(record) => Ok((trial_crumbs(&record), Err(none))),
            Err(Unshown::NoSuch) => Err(missing()),
            Err(unshown) => Ok((
                Place::within(Area::Benchmarks),
                Err(unshown.answer(&path.id)),
            )),
        };
    };
    let within = trial_crumbs(&found.benchmark_run);
    let Some(kept) = found.transcript(of) else {
        let not_begun = (
            StatusCode::NOT_FOUND,
            format!(
                "the trial {} of benchmark run {id} has not begun",
                path.trial()
            ),
        );
        return Ok((within, Err(not_begun)));
    };
    let transcribed = Transcribed {
        kept: kept.kept,
        which: kept.which,
        missing: None,
    };
    let shown = TrialShown {
        at,
        name: found.name,
        of,
    };
    Ok((within, Ok((shown, transcribed))))
}

/// The place of the page of the trial's transcript that `path` names, and
/// where the transcript is read from or why it cannot be; or else the
/// benchmark run the path named that does not exist.
async fn trial_transcript(
    permit: &Permit<Read>,
    sources: &Sources,
    path: &TrialPath,
) -> Result<(Place, Result<Transcribed, (StatusCode, String)>), Missing> {
    let id: BenchmarkRunId = path
        .id
        .parse()
        .map_err(|_| Missing::BenchmarkRun(path.id.clone()))?;
    let (within, found) = looked_up(permit, sources, path, id).await?;
    let named = found.as_ref().ok().map_or_else(
        || format!("Trial {}", path.trial()),
        |(shown, _)| shown.name.capitalized(),
    );
    let heading = Heading {
        line: found.as_ref().ok().map(|(shown, _)| context(shown)),
        ..Heading::named(format!("{named}'s {}", path.of))
    };
    let place = within.screen(TRIAL, Thing::Found(heading));
    Ok((place, found.map(|(_, transcribed)| transcribed)))
}

/// The page of a trial's transcript whose benchmark run is missing, `thing`.
fn missing_trial_page(thing: Thing, events: String, navigation: Navigation) -> Page {
    Page::new(
        Place::within(Area::Benchmarks).screen(TRIAL, thing),
        events,
        navigation,
    )
}

/// The page of a trial's run's or scorer's transcript, shown as the run
/// page shows a run's.
pub(super) async fn trial_page(
    Reader { permit, sources }: Reader,
    Path(path): Path<TrialPath>,
    Query(query): Query<RunQuery>,
    uri: Uri,
    navigation: Navigation,
) -> Response {
    let (place, transcribed) = match trial_transcript(&permit, &sources, &path).await {
        Ok(shown) => shown,
        Err(missing) => {
            let events = format!("/events{}", uri.path());
            let frame = Frame::of(&permit, &sources).await;
            return nothing_there(
                |thing| missing_trial_page(thing, events, navigation),
                missing,
                frame,
                "transcript",
            );
        }
    };
    transcript_page(
        Frame::of(&permit, &sources).await,
        uri.path(),
        place,
        transcribed,
        query.verbose(),
        None,
        navigation,
    )
    .await
}

/// The regions of the page of a trial's transcript, whose place, read as
/// the stream starts, stays as its page shows it.
pub(super) async fn trial_events(
    Reader { permit, sources }: Reader,
    watch: super::caller::Watch,
    Path(path): Path<TrialPath>,
    Query(query): Query<RunQuery>,
    uri: Uri,
    headers: HeaderMap,
    navigation: Navigation,
) -> Response {
    let events = uri.path().to_string();
    let (place, transcribed) = match trial_transcript(&permit, &sources, &path).await {
        Ok((place, transcribed)) => (place, transcribed.ok()),
        Err(missing) => {
            let place = Place::within(Area::Benchmarks).screen(TRIAL, Thing::Missing(missing));
            (place, None)
        }
    };
    let page: PageOf = Box::new(move || {
        let page = Page::new(place.clone(), events.clone(), navigation);
        Box::pin(async move { Some(page) })
    });
    transcript_events(
        permit,
        sources,
        watch,
        transcribed.as_ref(),
        &query,
        &headers,
        page,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use linewright_core::benchmarks::{ConfigurationId, results};

    /// A configuration `name` at a pass rate of 50%, with `mean` both its
    /// mean cost and its mean time.
    fn configuration(name: &str, mean: f64) -> ConfigurationResults {
        let figure = results::Mean {
            mean: Some(mean),
            total: mean,
            none: 0,
        };
        ConfigurationResults {
            id: ConfigurationId::AS_IS,
            name: name.parse().unwrap(),
            pass_rate: Some(0.5),
            figures: Figures {
                cost: figure.clone(),
                time: figure,
                leaks: 0,
                unscored: 0,
            },
            tasks: Vec::new(),
        }
    }

    /// The labels of the horizontal axis's ticks of the chart along `axis`
    /// of configurations at `means`.
    fn horizontal_ticks(axis: Axis, means: &[f64]) -> Vec<String> {
        let configurations: Vec<ConfigurationResults> = means
            .iter()
            .enumerate()
            .map(|(at, mean)| configuration(&format!("c{at}"), *mean))
            .collect();
        let ranked: Vec<(usize, &ConfigurationResults)> =
            configurations.iter().enumerate().collect();
        let drawn = plot(axis, &ranked).drawn.expect("a plot with points");
        drawn
            .mean_ticks
            .iter()
            .map(|tick| tick.label.clone())
            .collect()
    }

    /// `docs/specs/benchmarks.md`, Benchmark run, The plot, The scale and
    /// Ticks: the horizontal axis runs from 0 to 110% of the largest mean, with
    /// ticks at 0, half the scale and the scale.
    #[test_log::test]
    fn the_horizontal_scale_runs_to_110_percent_of_the_largest_mean() {
        assert_eq!(
            horizontal_ticks(Axis::Cost, &[0.5, 2.0]),
            ["$0.00", "$1.10", "$2.20"]
        );
        assert_eq!(
            horizontal_ticks(Axis::Time, &[30.0, 200.0]),
            ["0s", "1m 50s", "3m 40s"]
        );
    }

    /// `docs/specs/benchmarks.md`, Benchmark run, The plot, The scale: where
    /// the largest mean is 0, the scale runs to $1 for cost and to 1 second for
    /// time.
    #[test_log::test]
    fn with_a_largest_mean_of_0_the_scale_runs_to_a_dollar_or_a_second() {
        assert_eq!(
            horizontal_ticks(Axis::Cost, &[0.0]),
            ["$0.00", "$0.50", "$1.00"]
        );
        assert_eq!(
            horizontal_ticks(Axis::Time, &[0.0, 0.0]),
            ["0s", "0.5s", "1s"]
        );
    }
}
