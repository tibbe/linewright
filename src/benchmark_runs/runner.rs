//! The trials and scorers of benchmark runs in flight under `serve`
//! (`docs/specs/benchmarks.md`, Benchmark runs): each step starts as the plan
//! says, and each of its trial's changes is recorded.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context as _, Result, anyhow};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::{Id, JoinError, JoinSet};
use tracing::{error, info, warn};

use super::plan::{Gate, Step, plan};
use crate::benchmarks::{
    BenchmarkRunId, BenchmarkRunRecord, BenchmarkRunState, BenchmarkRuns, ConfigurationName,
    ScoreFigures, Shown, TrialChange, TrialFigures, TrialRef,
};
use crate::clock::Clock;
use crate::factory::FactoryConfig;
use crate::harness::{Harness, UsageLimit};
use crate::runs::{Ending, OutcomeName};
use crate::session::Cancellation;
use crate::task::{Task, TaskName};
use crate::trial::scorer::{self, ScoreRefusal, ScoreableTrial, ScorerRecord, Unscored};
use crate::trial::{self, Configuration, OnUsageLimit, Output, Under};

/// How the benchmark runs stand, as `serve` shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BenchmarkRunsShown {
    /// Each open benchmark run, as the last plan showed it.
    pub benchmark_runs: BTreeMap<BenchmarkRunId, Shown>,
    /// The trials and scorers in flight.
    pub in_flight: usize,
    pub max_trials: usize,
}

/// A step in flight.
struct InFlight {
    step: Step,
    cancel: Cancellation,
    /// Whether its benchmark run was cancelled, which ends its trial.
    benchmark_run_cancelled: bool,
}

/// A step that ended: the task that ran it, and the change of its trial it
/// makes, or how the task failed.
pub type Finished = Result<(Id, TrialChange), JoinError>;

/// What every step runs with.
#[derive(Clone)]
struct StepContext {
    benchmark_runs: BenchmarkRuns,
    clock: Arc<dyn Clock>,
    harness: Arc<dyn Harness>,
    config: FactoryConfig,
    refusals: UnboundedSender<UsageLimit>,
}

/// The trials and scorers of benchmark runs in flight, at most `max-trials`.
pub struct BenchmarkRunsRunner {
    context: StepContext,
    max_trials: usize,
    steps: JoinSet<TrialChange>,
    /// Each step in flight, by the task that runs it.
    in_flight: HashMap<Id, InFlight>,
    /// The changes of trials that ended but could not be recorded, recorded
    /// again at each tick.
    unrecorded: Vec<(TrialRef, TrialChange)>,
    /// Whether the trials a stop left in flight were taken back
    /// ([`BenchmarkRuns::interrupted`]): nothing starts before.
    taken_back: bool,
    /// Set once a stop cancelled every step: the trials of those that end
    /// are left as they are, for [`BenchmarkRuns::interrupted`] at the next
    /// start.
    stopping: bool,
    shown: BTreeMap<BenchmarkRunId, Shown>,
}

impl BenchmarkRunsRunner {
    /// Runs the steps of `benchmark_runs` with `harness` and `config`, a
    /// trial's, at most `max_trials` at once, their sessions waiting out
    /// the usage limit and reporting it to `refusals`, and records when
    /// each change happens by `clock`.
    pub fn new(
        benchmark_runs: BenchmarkRuns,
        clock: Arc<dyn Clock>,
        harness: Arc<dyn Harness>,
        config: FactoryConfig,
        max_trials: usize,
        refusals: UnboundedSender<UsageLimit>,
    ) -> BenchmarkRunsRunner {
        BenchmarkRunsRunner {
            context: StepContext {
                benchmark_runs,
                clock,
                harness,
                config,
                refusals,
            },
            max_trials,
            steps: JoinSet::new(),
            in_flight: HashMap::new(),
            unrecorded: vec![],
            taken_back: false,
            stopping: false,
            shown: BTreeMap::new(),
        }
    }

    /// Starts what the plan says of the open benchmark runs once `gate` is
    /// open, and cancels the steps of the benchmark runs
    /// cancelled. The first tick first takes back the trials a stop left in
    /// flight, and each records the changes earlier ones could not. What
    /// fails is logged, for the next tick to try again.
    pub async fn tick(&mut self, gate: Gate) {
        if self.stopping {
            return;
        }
        if !self.taken_back {
            if let Err(e) = self.context.benchmark_runs.interrupted().await {
                error!(
                    error = format!("{e:#}"),
                    "taking back the trials a stop interrupted failed"
                );
                return;
            }
            self.taken_back = true;
        }
        for (trial, change) in std::mem::take(&mut self.unrecorded) {
            self.record(trial, change).await;
        }
        let open = match self.context.benchmark_runs.open().await {
            Ok(open) => open,
            Err(e) => {
                error!(
                    error = format!("{e:#}"),
                    "reading the open benchmark runs failed"
                );
                return;
            }
        };
        for step in self.in_flight.values_mut() {
            let benchmark_run = step.step.trial().benchmark_run;
            if !step.benchmark_run_cancelled
                && !open.iter().any(|record| record.id == benchmark_run)
            {
                info!(benchmark_run = %benchmark_run, ?step.step, "its benchmark run was cancelled: stopping a trial or scorer");
                step.benchmark_run_cancelled = true;
                step.cancel.cancel();
            }
        }
        let planned = plan(&open, self.in_flight.len(), self.max_trials, gate);
        for (id, shown) in &planned.shown {
            if *shown == Shown::Ended
                && let Err(e) = self
                    .context
                    .benchmark_runs
                    .end(*id, self.context.clock.now())
                    .await
            {
                error!(
                    error = format!("{e:#}"),
                    "recording a benchmark run's end failed"
                );
            }
        }
        self.shown = planned.shown;
        for step in planned.starts {
            if let Some(record) = open
                .iter()
                .find(|record| record.id == step.trial().benchmark_run)
            {
                self.start(step, record).await;
            }
        }
    }

    /// Records that `step` of `record` starts, and starts it.
    async fn start(&mut self, step: Step, record: &BenchmarkRunRecord) {
        let trial = step.trial();
        let Some(at) = StepAt::of(record, trial) else {
            error!(
                ?trial,
                "a trial is of a task or a configuration its benchmark run lacks"
            );
            return;
        };
        let starts = match step {
            Step::Trial(_) => TrialChange::RunStarts,
            Step::Scorer(_) => TrialChange::ScorerStarts,
        };
        if let Err(e) = self
            .context
            .benchmark_runs
            .step(&trial, starts, self.context.clock.now())
            .await
        {
            error!(
                error = format!("{e:#}"),
                "starting a trial or scorer failed"
            );
            return;
        }
        let cancel = Cancellation::default();
        let context = self.context.clone();
        let under = Under {
            cancel: cancel.clone(),
            usage_limit: OnUsageLimit::WaitOut(context.refusals.clone()),
        };
        info!(benchmark_run = %trial.benchmark_run, task = %at.task, configuration = %at.configuration, repetition = trial.key.repetition, ?step, "a step of a benchmark run starts");
        let task = self.steps.spawn(async move {
            match step {
                Step::Trial(_) => context.trial(&at, &under).await,
                Step::Scorer(_) => context.score(&at, &under).await,
            }
        });
        self.in_flight.insert(
            task.id(),
            InFlight {
                step,
                cancel,
                benchmark_run_cancelled: false,
            },
        );
    }

    /// The next step to end, `None` with none in flight. Taking it is
    /// cancel-safe: [`BenchmarkRunsRunner::finished`] records it.
    pub async fn join_next(&mut self) -> Option<Finished> {
        self.steps.join_next_with_id().await
    }

    /// Records how the step `ended` changed its trial: cancelled once its
    /// benchmark run is, and nothing once a stop began, unless its benchmark
    /// run was cancelled. A step that panicked ends its trial without a score.
    pub async fn finished(&mut self, ended: Finished) {
        let (id, change) = match ended {
            Ok((id, change)) => (id, Some(change)),
            Err(e) => {
                error!(error = %e, "a trial or scorer of a benchmark run panicked");
                (e.id(), None)
            }
        };
        let Some(InFlight {
            step,
            benchmark_run_cancelled,
            ..
        }) = self.in_flight.remove(&id)
        else {
            return;
        };
        if self.stopping && !benchmark_run_cancelled {
            info!(
                ?step,
                "a trial or scorer stopped: it starts again with serve"
            );
            return;
        }
        let trial = step.trial();
        let cancelled = benchmark_run_cancelled
            || match self.context.benchmark_runs.state(trial.benchmark_run).await {
                Ok(state) => state != Some(BenchmarkRunState::Open),
                Err(e) => {
                    warn!(
                        error = format!("{e:#}"),
                        "reading whether a benchmark run is open failed"
                    );
                    false
                }
            };
        let change = match (cancelled, change, step) {
            (true, Some(TrialChange::RunEnds(run)), _) => {
                TrialChange::Cancelled { stopped: Some(run) }
            }
            (true, _, _) => TrialChange::Cancelled { stopped: None },
            (false, Some(change), _) => change,
            (false, None, Step::Trial(_)) => TrialChange::RunEnds(TrialFigures {
                outcome: OutcomeName::Ended(Ending::Failed),
                money: None,
                time: std::time::Duration::ZERO,
            }),
            (false, None, Step::Scorer(_)) => scorer_makes_none(anyhow!("the scorer panicked")),
        };
        self.record(trial, change).await;
    }

    /// Records `change` of `trial`, or keeps it to record at the next tick.
    async fn record(&mut self, trial: TrialRef, change: TrialChange) {
        if let Err(e) = self
            .context
            .benchmark_runs
            .step(&trial, change.clone(), self.context.clock.now())
            .await
        {
            error!(
                error = format!("{e:#}"),
                "recording how a trial or scorer ended failed; the next tick tries again"
            );
            self.unrecorded.push((trial, change));
        }
    }

    /// Waits for the next step to end, and records it.
    pub async fn next_finished(&mut self) {
        if let Some(ended) = self.join_next().await {
            self.finished(ended).await;
        }
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// Cancels every step in flight, leaving their trials to start again.
    pub fn cancel_all(&mut self) {
        self.stopping = true;
        for step in self.in_flight.values() {
            step.cancel.cancel();
        }
    }

    pub fn shown(&self) -> BenchmarkRunsShown {
        BenchmarkRunsShown {
            benchmark_runs: self.shown.clone(),
            in_flight: self.in_flight.len(),
            max_trials: self.max_trials,
        }
    }
}

/// Where a step's trial is in its benchmark run.
struct StepAt {
    trial: TrialRef,
    task: TaskName,
    configuration: ConfigurationName,
}

impl StepAt {
    /// Where `trial` of `record` is, `None` when `record` lacks its task or
    /// configuration.
    fn of(record: &BenchmarkRunRecord, trial: TrialRef) -> Option<StepAt> {
        Some(StepAt {
            trial,
            task: record.tasks.get(trial.key.task)?.clone(),
            configuration: record.configurations.get(trial.key.configuration)?.clone(),
        })
    }
}

impl StepContext {
    /// The output of `at`'s trial.
    fn output_dir(&self, at: &StepAt) -> PathBuf {
        self.benchmark_runs.output_dir(
            at.trial.benchmark_run,
            &at.configuration,
            &at.task,
            at.trial.key.repetition,
        )
    }

    /// The benchmark run's copy of `at`'s task.
    async fn task(&self, at: &StepAt) -> Result<Task> {
        Task::open(
            &self
                .benchmark_runs
                .task_dir(at.trial.benchmark_run, &at.task),
        )
        .await
        .context("opening the benchmark run's copy of the task")
    }

    /// Runs `at`'s task under its configuration into a new output, one a
    /// stop left in its place removed first, and gives how it ended: a run
    /// that did not run or end failed.
    async fn trial(&self, at: &StepAt, under: &Under) -> TrialChange {
        let started = Instant::now();
        let dir = self.output_dir(at);
        remove_left_behind(dir.clone()).await;
        let (outcome, spent) = match Output::make(&dir) {
            Ok(out) => {
                let ended = self.trial_into(at, &out, under).await;
                out.finished();
                (ended, out.spent())
            }
            Err(e) => (Err(e.into()), None),
        };
        let outcome = outcome.unwrap_or_else(|e| {
            error!(error = format!("{e:#}"), dir = %dir.display(), "a trial of a benchmark run failed");
            Ending::Failed
        });
        TrialChange::RunEnds(TrialFigures {
            outcome: OutcomeName::Ended(outcome),
            money: spent.and_then(|spent| spent.money),
            time: spent.map_or_else(|| started.elapsed(), |spent| spent.time),
        })
    }

    async fn trial_into(&self, at: &StepAt, out: &Output, under: &Under) -> Result<Ending> {
        let task = self.task(at).await?;
        let configuration = if at.configuration == ConfigurationName::as_is() {
            None
        } else {
            let dir = self
                .benchmark_runs
                .configuration_dir(at.trial.benchmark_run, &at.configuration);
            Some(
                Configuration::read(&dir)
                    .context("reading the benchmark run's copy of the configuration")?,
            )
        };
        Box::pin(trial::run(
            &task,
            out,
            self.harness.clone(),
            self.config.clone(),
            configuration.as_ref(),
            under,
        ))
        .await
    }

    /// Scores the output of `at`'s trial, the records of scorers a stop
    /// left there removed first, and keeps the score. An output that
    /// holds one, kept by a scorer a stop interrupted before its score
    /// was recorded, gives that one.
    async fn score(&self, at: &StepAt, under: &Under) -> TrialChange {
        let dir = self.output_dir(at);
        let task = match self.task(at).await {
            Ok(task) => task,
            Err(e) => {
                return scorer_makes_none(e.context("the task cannot be scored"));
            }
        };
        let scoreable = match ScoreableTrial::open(&dir, &task).await {
            Ok(scoreable) => scoreable,
            Err(ScoreRefusal::Scored) => {
                return match scorer::kept(&dir) {
                    Ok(Some(score)) => TrialChange::Scored(score),
                    Ok(None) => scorer_makes_none(anyhow!("the output's score went missing")),
                    Err(e) => scorer_makes_none(e),
                };
            }
            Err(e) => {
                return scorer_makes_none(anyhow!("the output cannot be scored: {e}"));
            }
        };
        let discarding = scoreable.clone();
        if let Err(e) =
            tokio::task::spawn_blocking(move || scorer::discard_unkept(&discarding)).await
        {
            warn!(error = %e, "removing the scorers' records a stop left failed");
        }
        let record = match ScorerRecord::make(&scoreable) {
            Ok(record) => record,
            Err(e) => return scorer_makes_none(e),
        };
        let spent = record.spent();
        match scorer::run(
            &task,
            &scoreable,
            record,
            self.harness.clone(),
            self.config.clone(),
            under,
        )
        .await
        {
            Ok(score) => {
                let figures: ScoreFigures = score.figures();
                match scorer::keep(&scoreable, score) {
                    Ok(()) => TrialChange::Scored(figures),
                    Err(e) => TrialChange::ScorerMakesNone {
                        why: Unscored::Failed(e.context("keeping the score")).to_string(),
                        scorer_money: figures.scorer_money,
                    },
                }
            }
            Err(unscored) => TrialChange::ScorerMakesNone {
                why: unscored.to_string(),
                scorer_money: spent.money(),
            },
        }
    }
}

/// A scorer that made no score for `error`, a cause outside the spec's
/// table, with no cost of its own.
fn scorer_makes_none(error: anyhow::Error) -> TrialChange {
    TrialChange::ScorerMakesNone {
        why: Unscored::Failed(error).to_string(),
        scorer_money: None,
    }
}

/// Removes the output in `dir` that a stop left, if any, off the runtime's
/// threads.
async fn remove_left_behind(dir: PathBuf) {
    let removing = tokio::task::spawn_blocking(move || remove_dir(&dir)).await;
    if let Err(e) = removing {
        warn!(error = %e, "removing the output a stop left failed");
    }
}

fn remove_dir(dir: &Path) {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            warn!(dir = %dir.display(), error = %e, "removing the output a stop left failed");
        }
        _ => {}
    }
}
