//! The replays and judges of experiments in flight under `serve`
//! (`docs/specs/benchmarks.md`, Experiments): each step starts as the plan
//! says, and each of its trial's changes is recorded.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context as _, Result};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::{Id, JoinError, JoinSet};
use tracing::{error, info, warn};

use super::plan::{Gate, Step, plan};
use crate::benchmarks::{
    ConfigurationName, ExperimentId, ExperimentRecord, ExperimentState, Experiments,
    JudgementFigures, ReplayFigures, Shown, TrialChange, TrialRef,
};
use crate::case::{Case, CaseName};
use crate::clock::Clock;
use crate::factory::FactoryConfig;
use crate::harness::{Harness, UsageLimit};
use crate::replay::judge::{self, JudgeRecord, JudgeRefusal, Replayed};
use crate::replay::{self, Configuration, OnUsageLimit, Output, Under};
use crate::runs::{Ending, OutcomeName};
use crate::session::Cancellation;

/// How the experiments stand, as `serve` shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExperimentsShown {
    /// Each open experiment, as the last plan showed it.
    pub experiments: BTreeMap<ExperimentId, Shown>,
    /// The replays and judges in flight.
    pub in_flight: usize,
    pub max_replays: usize,
}

/// A step in flight.
struct InFlight {
    step: Step,
    cancel: Cancellation,
    /// Whether its experiment was cancelled, which ends its trial.
    experiment_cancelled: bool,
}

/// A step that ended: the task that ran it, and the change of its trial it
/// makes, or how the task failed.
pub type Finished = Result<(Id, TrialChange), JoinError>;

/// What every step runs with.
#[derive(Clone)]
struct StepContext {
    experiments: Experiments,
    clock: Arc<dyn Clock>,
    harness: Arc<dyn Harness>,
    config: FactoryConfig,
    refusals: UnboundedSender<UsageLimit>,
}

/// The replays and judges of experiments in flight, at most `max-replays`.
pub struct ExperimentsRunner {
    context: StepContext,
    max_replays: usize,
    steps: JoinSet<TrialChange>,
    /// Each step in flight, by the task that runs it.
    in_flight: HashMap<Id, InFlight>,
    /// The changes of trials that ended but could not be recorded, recorded
    /// again at each tick.
    unrecorded: Vec<(TrialRef, TrialChange)>,
    /// Whether the trials a stop left in flight were taken back
    /// ([`Experiments::interrupted`]): nothing starts before.
    taken_back: bool,
    /// Set once a stop cancelled every step: the trials of those that end
    /// are left as they are, for [`Experiments::interrupted`] at the next
    /// start.
    stopping: bool,
    shown: BTreeMap<ExperimentId, Shown>,
}

impl ExperimentsRunner {
    /// Runs the steps of `experiments` with `harness` and `config`, a
    /// replay's, at most `max_replays` at once, their sessions waiting out
    /// the usage limit and reporting it to `refusals`, and records when
    /// each change happens by `clock`.
    pub fn new(
        experiments: Experiments,
        clock: Arc<dyn Clock>,
        harness: Arc<dyn Harness>,
        config: FactoryConfig,
        max_replays: usize,
        refusals: UnboundedSender<UsageLimit>,
    ) -> ExperimentsRunner {
        ExperimentsRunner {
            context: StepContext {
                experiments,
                clock,
                harness,
                config,
                refusals,
            },
            max_replays,
            steps: JoinSet::new(),
            in_flight: HashMap::new(),
            unrecorded: vec![],
            taken_back: false,
            stopping: false,
            shown: BTreeMap::new(),
        }
    }

    /// Starts what the plan says of the open experiments once `gate` is
    /// open, and cancels the steps of the experiments
    /// cancelled. The first tick first takes back the trials a stop left in
    /// flight, and each records the changes earlier ones could not. What
    /// fails is logged, for the next tick to try again.
    pub async fn tick(&mut self, gate: Gate) {
        if self.stopping {
            return;
        }
        if !self.taken_back {
            if let Err(e) = self.context.experiments.interrupted().await {
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
        let open = match self.context.experiments.open().await {
            Ok(open) => open,
            Err(e) => {
                error!(
                    error = format!("{e:#}"),
                    "reading the open experiments failed"
                );
                return;
            }
        };
        for step in self.in_flight.values_mut() {
            let experiment = step.step.trial().experiment;
            if !step.experiment_cancelled && !open.iter().any(|record| record.id == experiment) {
                info!(%experiment, ?step.step, "its experiment was cancelled: stopping a replay or judge");
                step.experiment_cancelled = true;
                step.cancel.cancel();
            }
        }
        let planned = plan(&open, self.in_flight.len(), self.max_replays, gate);
        for (id, shown) in &planned.shown {
            if *shown == Shown::Ended
                && let Err(e) = self
                    .context
                    .experiments
                    .end(*id, self.context.clock.now())
                    .await
            {
                error!(
                    error = format!("{e:#}"),
                    "recording an experiment's end failed"
                );
            }
        }
        self.shown = planned.shown;
        for step in planned.starts {
            if let Some(record) = open
                .iter()
                .find(|record| record.id == step.trial().experiment)
            {
                self.start(step, record).await;
            }
        }
    }

    /// Records that `step` of `record` starts, and starts it.
    async fn start(&mut self, step: Step, record: &ExperimentRecord) {
        let trial = step.trial();
        let Some(at) = StepAt::of(record, trial) else {
            error!(
                ?trial,
                "a trial is of a case or a configuration its experiment lacks"
            );
            return;
        };
        let starts = match step {
            Step::Replay(_) => TrialChange::ReplayStarts,
            Step::Judge(_) => TrialChange::JudgeStarts,
        };
        if let Err(e) = self
            .context
            .experiments
            .step(&trial, starts, self.context.clock.now())
            .await
        {
            error!(
                error = format!("{e:#}"),
                "starting a replay or judge failed"
            );
            return;
        }
        let cancel = Cancellation::default();
        let context = self.context.clone();
        let under = Under {
            cancel: cancel.clone(),
            usage_limit: OnUsageLimit::WaitOut(context.refusals.clone()),
        };
        info!(experiment = %trial.experiment, case = %at.case, configuration = %at.configuration, repetition = trial.key.repetition, ?step, "a step of an experiment starts");
        let task = self.steps.spawn(async move {
            match step {
                Step::Replay(_) => context.replay(&at, &under).await,
                Step::Judge(_) => context.judge(&at, &under).await,
            }
        });
        self.in_flight.insert(
            task.id(),
            InFlight {
                step,
                cancel,
                experiment_cancelled: false,
            },
        );
    }

    /// The next step to end, `None` with none in flight. Taking it is
    /// cancel-safe: [`ExperimentsRunner::finished`] records it.
    pub async fn join_next(&mut self) -> Option<Finished> {
        self.steps.join_next_with_id().await
    }

    /// Records how the step `ended` changed its trial: cancelled once its
    /// experiment is, and nothing once a stop began, unless its experiment
    /// was cancelled. A step that panicked ends its trial without a
    /// judgement.
    pub async fn finished(&mut self, ended: Finished) {
        let (id, change) = match ended {
            Ok((id, change)) => (id, Some(change)),
            Err(e) => {
                error!(error = %e, "a replay or judge of an experiment panicked");
                (e.id(), None)
            }
        };
        let Some(InFlight {
            step,
            experiment_cancelled,
            ..
        }) = self.in_flight.remove(&id)
        else {
            return;
        };
        if self.stopping && !experiment_cancelled {
            info!(
                ?step,
                "a replay or judge stopped: it starts again with serve"
            );
            return;
        }
        let trial = step.trial();
        let cancelled = experiment_cancelled
            || match self.context.experiments.state(trial.experiment).await {
                Ok(state) => state != Some(ExperimentState::Open),
                Err(e) => {
                    warn!(
                        error = format!("{e:#}"),
                        "reading whether an experiment is open failed"
                    );
                    false
                }
            };
        let change = match (cancelled, change, step) {
            (true, _, _) => TrialChange::Cancelled,
            (false, Some(change), _) => change,
            (false, None, Step::Replay(_)) => TrialChange::ReplayEnds(ReplayFigures {
                outcome: OutcomeName::Ended(Ending::Failed),
                money: None,
                time: std::time::Duration::ZERO,
            }),
            (false, None, Step::Judge(_)) => TrialChange::JudgeMakesNone {
                reason: "the judge panicked".into(),
                judge_money: None,
            },
        };
        self.record(trial, change).await;
    }

    /// Records `change` of `trial`, or keeps it to record at the next tick.
    async fn record(&mut self, trial: TrialRef, change: TrialChange) {
        if let Err(e) = self
            .context
            .experiments
            .step(&trial, change.clone(), self.context.clock.now())
            .await
        {
            error!(
                error = format!("{e:#}"),
                "recording how a replay or judge ended failed; the next tick tries again"
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

    pub fn shown(&self) -> ExperimentsShown {
        ExperimentsShown {
            experiments: self.shown.clone(),
            in_flight: self.in_flight.len(),
            max_replays: self.max_replays,
        }
    }
}

/// Where a step's trial is in its experiment.
struct StepAt {
    trial: TrialRef,
    case: CaseName,
    configuration: ConfigurationName,
}

impl StepAt {
    /// Where `trial` of `record` is, `None` when `record` lacks its case or
    /// configuration.
    fn of(record: &ExperimentRecord, trial: TrialRef) -> Option<StepAt> {
        Some(StepAt {
            trial,
            case: record.cases.get(trial.key.case)?.clone(),
            configuration: record.configurations.get(trial.key.configuration)?.clone(),
        })
    }
}

impl StepContext {
    /// The output of `at`'s replay.
    fn output_dir(&self, at: &StepAt) -> PathBuf {
        self.experiments.output_dir(
            at.trial.experiment,
            &at.configuration,
            &at.case,
            at.trial.key.repetition,
        )
    }

    /// The experiment's copy of `at`'s case.
    async fn case(&self, at: &StepAt) -> Result<Case> {
        Case::open(&self.experiments.case_dir(at.trial.experiment, &at.case))
            .await
            .context("opening the experiment's copy of the case")
    }

    /// Replays `at`'s case under its configuration into a new output, one a
    /// stop left in its place removed first, and gives how it ended: a run
    /// that did not run or end failed.
    async fn replay(&self, at: &StepAt, under: &Under) -> TrialChange {
        let started = Instant::now();
        let dir = self.output_dir(at);
        remove_left_behind(dir.clone()).await;
        let (outcome, spent) = match Output::make(&dir) {
            Ok(out) => {
                let ended = self.replay_into(at, &out, under).await;
                out.finished();
                (ended, out.spent())
            }
            Err(e) => (Err(e.into()), None),
        };
        let outcome = outcome.unwrap_or_else(|e| {
            error!(error = format!("{e:#}"), dir = %dir.display(), "a replay of an experiment failed");
            Ending::Failed
        });
        TrialChange::ReplayEnds(ReplayFigures {
            outcome: OutcomeName::Ended(outcome),
            money: spent.and_then(|spent| spent.money),
            time: spent.map_or_else(|| started.elapsed(), |spent| spent.time),
        })
    }

    async fn replay_into(&self, at: &StepAt, out: &Output, under: &Under) -> Result<Ending> {
        let case = self.case(at).await?;
        let configuration = if at.configuration == ConfigurationName::as_is() {
            None
        } else {
            let dir = self
                .experiments
                .configuration_dir(at.trial.experiment, &at.configuration);
            Some(
                Configuration::read(&dir)
                    .context("reading the experiment's copy of the configuration")?,
            )
        };
        Box::pin(replay::run(
            &case,
            out,
            self.harness.clone(),
            self.config.clone(),
            configuration.as_ref(),
            under,
        ))
        .await
    }

    /// Judges the output of `at`'s replay, the records of judges a stop
    /// left there removed first, and keeps the judgement. An output that
    /// holds one, kept by a judge a stop interrupted before its judgement
    /// was recorded, gives that one.
    async fn judge(&self, at: &StepAt, under: &Under) -> TrialChange {
        let dir = self.output_dir(at);
        let case = match self.case(at).await {
            Ok(case) => case,
            Err(e) => return judge_makes_none(format!("the case cannot be judged: {e:#}")),
        };
        let replayed = match Replayed::open(&dir, &case).await {
            Ok(replayed) => replayed,
            Err(JudgeRefusal::Judged) => {
                return match judge::kept(&dir) {
                    Ok(Some(judgement)) => TrialChange::Judged(judgement),
                    Ok(None) => judge_makes_none("the output's judgement went missing".into()),
                    Err(e) => judge_makes_none(format!("{e:#}")),
                };
            }
            Err(e) => return judge_makes_none(format!("the output cannot be judged: {e}")),
        };
        let discarding = replayed.clone();
        if let Err(e) =
            tokio::task::spawn_blocking(move || judge::discard_unkept(&discarding)).await
        {
            warn!(error = %e, "removing the judges' records a stop left failed");
        }
        let record = match JudgeRecord::make(&replayed) {
            Ok(record) => record,
            Err(e) => return judge_makes_none(format!("{e:#}")),
        };
        let spent = record.spent();
        match judge::run(
            &case,
            &replayed,
            record,
            self.harness.clone(),
            self.config.clone(),
            under,
        )
        .await
        {
            Ok(judgement) => {
                let figures: JudgementFigures = judgement.figures();
                match judge::keep(&replayed, judgement) {
                    Ok(()) => TrialChange::Judged(figures),
                    Err(e) => TrialChange::JudgeMakesNone {
                        reason: format!("keeping the judgement: {e:#}"),
                        judge_money: figures.judge_money,
                    },
                }
            }
            Err(unjudged) => TrialChange::JudgeMakesNone {
                reason: unjudged.to_string(),
                judge_money: spent.money(),
            },
        }
    }
}

/// A judge that made no judgement for `reason`, before its session opened.
fn judge_makes_none(reason: String) -> TrialChange {
    TrialChange::JudgeMakesNone {
        reason,
        judge_money: None,
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
