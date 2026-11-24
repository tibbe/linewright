//! Running a task's stage on the task, in place of GitHub
//! (`docs/specs/benchmarks.md`, Trials).

mod configuration;
mod gh_gate;
mod journal;
pub mod scorer;

pub use configuration::{Configuration, ConfigurationRefusal};
pub use gh_gate::{Gate, GhConfigDir, Upstream};
pub use journal::Journal;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use chrono::TimeDelta;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info, warn};

use crate::clock::{Clock, RunningFrom, SystemClock};
use crate::clones::Clones;
use crate::cost::{RunCost, Spend, Usd};
use crate::domain::{Item, Scope};
use crate::environment::trials::{self, BuildLog};
use crate::environment::{self, Source};
use crate::factory::FactoryConfig;
use crate::harness::{GhReach, Harness, UsageLimit};
use crate::repo_files::{RepoLayer, holding};
use crate::run::{self, Shell, ShellParts};
use crate::runs::files::RunFiles;
use crate::runs::{Begin, Ending, OutcomeName, RunId, Unended};
use crate::session::{Cancellation, Sessions};
use crate::store::local::LocalStore;
use crate::store::{RepoSettings, Store};
use crate::task::{Task, TaskItemKind};
use crate::winding_up::WindingUp;
use crate::work_settings::RunKind;
use crate::{COMMAND, version_and_commit};

const OUTCOME: &str = "outcome";
/// The directory of the output that keeps the sessions' records, as the
/// runs of the trial keep them in the output.
const SESSIONS: &str = crate::runs::SESSIONS;
/// The directory of the output that keeps the writes.
const WRITES: &str = "writes";
/// The directory of the output that keeps the attachments.
const ATTACHMENTS: &str = "attachments";
const VERSION: &str = "version";
/// The directory, under the work directory, of the clones and the
/// environments of trials.
pub const TRIALS: &str = "trials";
/// The part of an output that names the run whose task it runs.
pub const TASK: &str = "task";
/// The directory of the output that keeps a copy of the configuration.
const CONFIGURATION: &str = "configuration";
const BUILD_LOG: &str = "build.log";
/// Beside [`BUILD_LOG`]: how long the build took, in seconds.
const BUILD_SECONDS: &str = "build-seconds";
/// The time of the trial and what its run cost.
const COST: &str = "cost.json";
const COMMITS: &str = "commits.git";

/// What a trial or a scorer runs under: what cancels its run, and where a
/// session refused for the usage limit reports the limit before it waits
/// until the limit resets and goes on.
pub struct Under {
    pub cancel: Cancellation,
    pub report_refusal: UnboundedSender<UsageLimit>,
}

/// The link to the attachment `name` from the writes, its path from them.
fn attachment_link(name: &str) -> String {
    format!("../{ATTACHMENTS}/{name}")
}

/// The directory of a trial's output: one that held nothing.
#[derive(Clone, Debug)]
pub struct Output {
    dir: PathBuf,
    /// When the trial started.
    started: Instant,
    written: Arc<Mutex<Parts>>,
}

/// What the output holds that later writes depend on.
#[derive(Debug, Default)]
struct Parts {
    /// The ending written as the outcome, once it is: no outcome replaces
    /// it.
    ended: Option<Ending>,
    /// The trial's run, once it began.
    run: Option<RunId>,
    /// Whether the cost is written: it is written once, as the trial ends.
    cost_written: bool,
    /// What the trial spent, once it ended.
    spent: Option<Spent>,
}

/// What a trial spent: its time, and what its run cost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spent {
    pub time: std::time::Duration,
    /// `None` when its run has no cost (`docs/specs/runs.md`, Cost), or did
    /// not begin.
    pub money: Option<Usd>,
}

/// The cost of a trial, as its output keeps it (`docs/specs/benchmarks.md`,
/// The output of a trial).
#[derive(Serialize)]
struct TrialCost {
    seconds: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<RunCost>,
}

/// What a trial spent, as its output in `dir` kept it once the trial ended;
/// `None`, which it logs, when that does not read, such as after a hard kill.
pub fn kept_spent(dir: &Path) -> Option<Spent> {
    /// The parts of [`TrialCost`] that [`Spent`] holds.
    #[derive(Deserialize)]
    struct KeptCost {
        seconds: f64,
        run: Option<KeptRunCost>,
    }
    /// The part of [`RunCost`] that [`Spent`] holds.
    #[derive(Deserialize)]
    struct KeptRunCost {
        all: Option<Spend>,
    }
    let path = dir.join(COST);
    let kept = std::fs::read_to_string(&path)
        .map_err(anyhow::Error::from)
        .and_then(|text| Ok(serde_json::from_str::<KeptCost>(&text)?))
        .and_then(|kept| {
            Ok(Spent {
                time: std::time::Duration::try_from_secs_f64(kept.seconds)?,
                money: kept.run.and_then(|run| run.all).and_then(|all| all.usd),
            })
        });
    match kept {
        Ok(spent) => Some(spent),
        Err(e) => {
            warn!(
                error = format!("{e:#}"),
                path = %path.display(),
                "reading what a trial spent failed"
            );
            None
        }
    }
}

/// Why [`run()`] gives no ending, or [`Setting::prepare`] no setting.
#[derive(Debug, thiserror::Error)]
pub enum Unrun {
    /// The build of its environment failed (`docs/specs/benchmarks.md`,
    /// Benchmark runs, A trial without a score).
    #[error(transparent)]
    BuildFailed(#[from] trials::BuildFailed),
    /// Anything else, such as an environment that cannot be had, or an ending
    /// that could not be written.
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// Why a directory cannot take a trial's output.
#[derive(Debug, thiserror::Error)]
pub enum OutputRefusal {
    #[error("the output's directory {0} holds something: give an empty or missing one")]
    NotEmpty(PathBuf),
    #[error("the output's directory {dir} cannot be used")]
    Unusable {
        dir: PathBuf,
        #[source]
        error: std::io::Error,
    },
}

impl Output {
    /// The output in `dir`, which it makes when it is missing.
    ///
    /// # Errors
    ///
    /// When `dir` holds anything, or cannot be made or read.
    pub fn make(dir: &Path) -> Result<Output, OutputRefusal> {
        Ok(Output {
            dir: make_empty_dir(dir)?,
            started: Instant::now(),
            written: Arc::default(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Writes the cost, as the trial ends now, unless it is written: the
    /// time since the output was made, and what the trial's run cost, if
    /// it began. It logs what it cannot write: the cost without the run's
    /// when the run's record cannot be read, and nothing when the file
    /// cannot be written.
    pub fn finished(&self) {
        let mut written = self.lock_written();
        if written.cost_written {
            return;
        }
        let time = self.started.elapsed();
        let seconds = time.as_secs_f64();
        let run = written
            .run
            .as_ref()
            .and_then(|id| match RunFiles::new(&self.dir).cost(id) {
                Ok(cost) => Some(cost),
                Err(e) => {
                    error!(error = format!("{e:#}"), "reading what the run cost failed");
                    None
                }
            });
        written.spent = Some(Spent {
            time,
            money: run
                .as_ref()
                .and_then(|run| run.spend.as_ref())
                .and_then(|spend| spend.all.usd),
        });
        let text = serde_json::to_string_pretty(&TrialCost { seconds, run })
            .map_err(anyhow::Error::from)
            .and_then(|text| self.write(COST, &format!("{text}\n")));
        match text {
            Ok(()) => written.cost_written = true,
            Err(e) => error!(error = format!("{e:#}"), "writing the cost failed"),
        }
    }

    /// What the trial spent, once [`Output::finished`] says it ended.
    pub fn spent(&self) -> Option<Spent> {
        self.lock_written().spent
    }

    /// Writes `in-flight` as the outcome of `run`, which has begun.
    fn in_flight(&self, run: &RunId) -> Result<()> {
        let mut written = self.lock_written();
        written.run = Some(run.clone());
        write_outcome(&self.dir, OutcomeName::Unended(Unended::InFlight))
    }

    fn ended(&self, ending: Ending) -> Result<()> {
        let mut written = self.lock_written();
        write_outcome(&self.dir, OutcomeName::Ended(ending))?;
        written.ended = Some(ending);
        Ok(())
    }

    fn lock_written(&self) -> MutexGuard<'_, Parts> {
        self.written
            .lock()
            .expect("the output's lock is not poisoned")
    }

    /// Replaces the file `name` with one holding `text`, so that a reader
    /// sees the one or the other whole.
    fn write(&self, name: &str, text: &str) -> Result<()> {
        write_part(&self.dir, name, text)
    }
}

/// Writes the output of `build` and its time into `dir`, as the parts
/// [`BUILD_LOG`] and [`BUILD_SECONDS`].
fn write_build(dir: &Path, build: &BuildLog) -> Result<()> {
    write_part(dir, BUILD_LOG, &build.output)?;
    write_part(
        dir,
        BUILD_SECONDS,
        &format!("{}\n", build.time.as_secs_f64()),
    )
}

/// The directory `dir`, made when it is missing, as an absolute path.
///
/// # Errors
///
/// When `dir` holds anything, or cannot be made or read.
pub(crate) fn make_empty_dir(dir: &Path) -> Result<PathBuf, OutputRefusal> {
    let unusable = |error| OutputRefusal::Unusable {
        dir: dir.to_path_buf(),
        error,
    };
    std::fs::create_dir_all(dir).map_err(unusable)?;
    let dir = std::path::absolute(dir).map_err(unusable)?;
    if std::fs::read_dir(&dir).map_err(unusable)?.next().is_some() {
        return Err(OutputRefusal::NotEmpty(dir));
    }
    Ok(dir)
}

/// Replaces the outcome part in the output's directory `dir` with
/// `outcome`, so that a reader sees the one or the other whole.
pub(crate) fn write_outcome(dir: &Path, outcome: OutcomeName) -> Result<()> {
    let name = serde_json::to_value(outcome)?;
    let name = name.as_str().context("an outcome's name is a string")?;
    write_part(dir, OUTCOME, &format!("{name}\n"))
}

/// Whether the output in `dir` has begun: its `task` part names the run
/// whose task it runs.
pub fn began(dir: &Path) -> bool {
    dir.join(TASK).is_file()
}

/// The run whose task the output in `dir` runs, as its `task` part
/// names it; `None` before the trial wrote it.
///
/// # Errors
///
/// When the part cannot be read, or names no run.
pub fn task_run(dir: &Path) -> Result<Option<RunId>> {
    let path = dir.join(TASK);
    match std::fs::read_to_string(&path) {
        Ok(text) => text
            .trim_end()
            .parse()
            .map(Some)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("reading {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Replaces the part `name` in the output's directory `dir` with one
/// holding `text`, so that a reader sees the one or the other whole.
fn write_part(dir: &Path, name: &str, text: &str) -> Result<()> {
    write_whole(&dir.join(name), &dir.join(format!(".{name}")), text)
}

/// Replaces the file at `path` with one holding `text`, written first at
/// `partial` in the same directory, so that a reader sees the one or the
/// other whole. A failed write leaves no file at `partial`.
fn write_whole(path: &Path, partial: &Path, text: impl AsRef<[u8]>) -> Result<()> {
    let written = std::fs::write(partial, text).and_then(|()| std::fs::rename(partial, path));
    if written.is_err() {
        let _ = std::fs::remove_file(partial);
    }
    written.with_context(|| format!("writing {}", path.display()))
}

/// Writes a file at `path` holding `text`, written first at `partial` in the
/// same directory, so that a reader sees it whole or not at all, and never
/// over a file already at `path`. No file is left at `partial`.
///
/// # Errors
///
/// When the file cannot be written, of kind
/// [`std::io::ErrorKind::AlreadyExists`] when one is already at `path`.
fn write_whole_new(path: &Path, partial: &Path, text: impl AsRef<[u8]>) -> std::io::Result<()> {
    let written = std::fs::write(partial, text).and_then(|()| std::fs::hard_link(partial, path));
    if let Err(e) = std::fs::remove_file(partial) {
        warn!(partial = %partial.display(), error = %e, "removing a partial write failed");
    }
    written
}

/// Runs the stage of `task` on its item with `config` and `harness`, its
/// sessions with `configuration` over every other layer, with the task in place
/// of GitHub, and keeps what it did in `out`: the version, the ID of the task's
/// run and a copy of `configuration` first, `in-flight` as the outcome once the
/// run begins, its ending once it ends, and the cost once it returns, however
/// it does. Its sessions' `gh` cannot read the task's repository on GitHub
/// (`docs/specs/benchmarks.md`, The repository on GitHub). `config`'s work
/// directory is that of the task's repository. The run's clones are kept in a
/// directory of this trial's own under it, removed through `config`'s winding
/// up once the run ends, or by a later trial when a signal ends this one first.
/// `under` cancels the run, and says what its sessions do at the usage limit.
///
/// # Errors
///
/// [`Unrun::BuildFailed`] when the build of the environment fails, and
/// [`Unrun::Failed`] when the output cannot be written, the task's model of
/// GitHub cannot be seeded, a file on the task's default branch would hold the
/// run, the environment cannot be had, the gate of the sessions' `gh` cannot be
/// started, or the run cannot be begun: then the run does not run. Once the
/// run ended, only when its ending cannot be written.
pub async fn run(
    task: &Task,
    out: &Output,
    harness: Arc<dyn Harness>,
    config: FactoryConfig,
    configuration: Option<&Configuration>,
    under: &Under,
) -> Result<Ending, Unrun> {
    let ran = run_unfinished(task, out, harness, config, configuration, under).await;
    out.finished();
    ran
}

/// [`run`], but for writing the cost.
async fn run_unfinished(
    task: &Task,
    out: &Output,
    harness: Arc<dyn Harness>,
    config: FactoryConfig,
    configuration: Option<&Configuration>,
    under: &Under,
) -> Result<Ending, Unrun> {
    out.write(
        VERSION,
        &format!("{COMMAND} {}\n", version_and_commit(config.commit)),
    )?;
    out.write(TASK, &format!("{}\n", task.run()))?;
    if let Some(configuration) = configuration {
        configuration
            .keep(&out.path().join(CONFIGURATION))
            .context("keeping a copy of the configuration")?;
    }
    let (work_dir, winding_up) = (config.work_dir.clone(), config.winding_up.clone());
    in_own_dir(&work_dir, winding_up, "trial", async |own| {
        let ending = run_in(task, out, harness, config, configuration, under, own).await?;
        out.ended(ending)?;
        Ok(ending)
    })
    .await?
}

/// A directory of a trial's or a scorer's own, among the trials', and what
/// winds up the clones made in it.
#[derive(Clone, Copy)]
struct OwnDir<'a> {
    path: &'a Path,
    clones_winding_up: &'a WindingUp,
}

impl OwnDir<'_> {
    /// The clones made in it.
    fn clones(self) -> Clones {
        Clones::new(self.path, self.clones_winding_up.clone())
    }
}

/// Runs `work` in a new directory of its own, named `prefix`, among the
/// trials' under `work_dir`, and removes the directory through `winding_up`
/// once the clones made in it are wound up. The directories of earlier ones that a signal ended are removed
/// meanwhile.
///
/// # Errors
///
/// When the directory cannot be made: `work` does not run then.
async fn in_own_dir<T>(
    work_dir: &Path,
    winding_up: WindingUp,
    prefix: &str,
    work: impl AsyncFnOnce(OwnDir<'_>) -> T,
) -> Result<T> {
    let trials = Clones::new(&work_dir.join(TRIALS), winding_up);
    trials.sweep();
    let own = trials
        .make(prefix)
        .await
        .with_context(|| format!("creating the {prefix}'s own directory"))?;
    let clones_winding_up = WindingUp::default();
    let done = work(OwnDir {
        path: own.path(),
        clones_winding_up: &clones_winding_up,
    })
    .await;
    clones_winding_up.wait().await;
    drop(own);
    Ok(done)
}

async fn run_in(
    task: &Task,
    out: &Output,
    harness: Arc<dyn Harness>,
    config: FactoryConfig,
    configuration: Option<&Configuration>,
    under: &Under,
    own: OwnDir<'_>,
) -> Result<Ending, Unrun> {
    let kind = task.stage().kind();
    let setting = Setting::prepare(
        task,
        &RunKind::Stage(kind),
        &config,
        harness.clone(),
        &own.path.join("store"),
        &out.path().join(COMMITS),
        |build| write_build(out.path(), build),
    )
    .await?;
    let store: Arc<dyn Store> = Arc::new(Journal::new(setting.store.clone(), out.path()));
    let clock = setting.clock.clone();
    let settings = setting.settings.clone();
    let shell = Shell::new(ShellParts {
        sessions: setting.sessions(
            task,
            store,
            harness,
            &config,
            own.clones(),
            &under.report_refusal,
        ),
        spec_glob: config.spec_glob,
        spec_lookback: config.spec_lookback,
        done_window: config.done_window,
        max_runs: config.max_runs,
        ci_grace: config.ci_grace,
        ci_timeout: config.ci_timeout,
        ci_attempts: config.ci_attempts,
        auto_merge: config.auto_merge,
        require_label: config.require_label,
        work: config.work,
        environments: config.environments,
        operator_briefs: config.briefs,
        settings,
        work_dir: config.work_dir,
        pushed_branches: None,
    });
    let now = clock.now();
    let issue = task.item();
    let scope = Scope {
        items: task
            .items()
            .iter()
            .map(|item| match item.kind {
                TaskItemKind::Issue(_) => Item::Issue(item.number),
                TaskItemKind::Pull(_) => Item::Pull(item.number),
            })
            .collect(),
        branches: BTreeSet::new(),
    };
    let snapshot = shell
        .sessions
        .store
        .snapshot(shell.policy.windows(now), &scope)
        .await
        .context("taking a snapshot of the task")?;
    let work = task.stage().work(issue);
    let running = RunFiles::new(out.path())
        .begin(
            Begin::Work {
                item: issue,
                stage: kind,
            },
            now,
        )
        .await?;
    out.in_flight(running.id())?;
    info!(run = %running.id(), task = %task.run(), "running a trial");
    let with = shell.run_with(&setting.repo, RunKind::Stage(kind));
    let with = match configuration {
        Some(configuration) => configuration.over(with, &setting.repo, &shell.operator_briefs),
        None => with,
    };
    let ctx = run::Ctx {
        with,
        as_of: now,
        reworked: None,
        cancel: under.cancel.clone(),
        run: running,
    };
    let ended = run::run(Arc::new(shell), work, Arc::new(snapshot), ctx).await;
    drop(setting);
    Ok(ended)
}

/// What a task gives the sessions run on it in place of GitHub
/// (`docs/specs/benchmarks.md`, Trials): a model of GitHub seeded from
/// the task, Linewright's files on its default branch, its environment and
/// a `gh` kept from the task's repository. That `gh` reaches GitHub only
/// while the setting lives.
pub struct Setting {
    /// The model of GitHub, as the task's run found it.
    pub store: Arc<dyn Store>,
    /// The clock of the model, running from the task's start.
    pub clock: Arc<dyn Clock>,
    /// Linewright's files on the task's default branch.
    pub repo: RepoLayer,
    /// The repository's settings, as the task's properties and the config
    /// give them.
    pub settings: RepoSettings,
    /// What sessions copy the environment from.
    pub environment: Source,
    gate: Gate,
}

impl Setting {
    /// Prepares the setting of a `kind` of run on `task` with `config` and
    /// `harness`: seeds the model of GitHub at `root`, its repository at
    /// `repository`, refuses a file on the task's default branch that would
    /// hold `kind`, prepares the environment, handing the output of its
    /// build and its time to `build_log` when this built it, and starts the
    /// gate of the sessions' `gh`.
    ///
    /// # Errors
    ///
    /// [`Unrun::BuildFailed`] when the environment's build fails, and
    /// [`Unrun::Failed`] when the model of GitHub cannot be seeded, a file on
    /// the task's default branch would hold `kind`, the environment cannot be
    /// had, `build_log` fails, or the gate cannot be started.
    pub async fn prepare(
        task: &Task,
        kind: &RunKind<'_>,
        config: &FactoryConfig,
        harness: Arc<dyn Harness>,
        root: &Path,
        repository: &Path,
        build_log: impl Fn(&BuildLog) -> Result<()>,
    ) -> Result<Setting, Unrun> {
        let clock: Arc<dyn Clock> = Arc::new(RunningFrom::new(task.start()));
        let seeded = LocalStore::from_task(root, repository, task, clock.clone())
            .await
            .context("seeding the model of GitHub from the task")?;
        let store: Arc<dyn Store> = Arc::new(seeded);
        let properties = task.properties();
        let mut settings = properties.settings.clone();
        if let Some(method) = config.merge_method {
            settings.merge_method = method;
        }
        let repo = RepoLayer::read(store.as_ref(), &settings.default_branch, &config.briefs)
            .await
            .context("reading Linewright's files in the task")?;
        if let Some(file) = holding(&repo.unusable, kind).first() {
            let (refused, held) = match kind {
                RunKind::Scorer => ("scored", "the scorer"),
                RunKind::Stage(_) | RunKind::Automation(_) => ("run", "the run"),
            };
            return Err(anyhow!(
                "the task cannot be {refused}: {} on its default branch would hold {held}: {}",
                file.path,
                file.error
            )
            .into());
        }
        let head = task
            .heads()
            .get(&settings.default_branch)
            .context("the task lacks the head of its default branch")?;
        let builder = environment::Builder {
            store: store.clone(),
            harness,
            bash: config.bash.clone(),
            clock: Arc::new(SystemClock),
            redaction: config.secrets.redaction().clone(),
            given: config.environments.pipeline.clone(),
            default_branch: settings.default_branch.clone(),
            winding_up: config.winding_up.clone(),
            database: config.database.clone(),
        };
        let max_age = TimeDelta::from_std(config.work.setup_max_age(&repo.settings))
            .context("`setup.max-age` is too long")?;
        let prepared = trials::prepare(
            &trials::dir(config.database.work_dir()),
            head,
            builder,
            config.work.setup(&repo.settings),
            SystemClock.now(),
            max_age,
        )
        .await;
        let environment = match prepared {
            Ok(in_use) => {
                if let Some(build) = in_use.built() {
                    build_log(build)?;
                }
                in_use.source()
            }
            Err(trials::Unprepared::Failed(failed)) => {
                if let Some(build) = &failed.output {
                    build_log(build)?;
                }
                return Err(failed.into());
            }
            Err(e @ trials::Unprepared::Unavailable(_)) => return Err(Unrun::Failed(e.into())),
        };
        let gate = Gate::start(&properties.settings.repo, Upstream::GitHub)
            .context("starting the gate of the sessions' gh")?;
        Ok(Setting {
            store,
            clock,
            repo,
            settings,
            environment,
            gate,
        })
    }

    /// How far the sessions' `gh` reaches: not to the task's repository.
    pub fn gh(&self) -> GhReach {
        self.gate.gh()
    }

    /// The parts the sessions run on `task` in this setting share, with
    /// `config` and `harness`: they fetch from and clone `store`, make their
    /// clones in `clones`, and report a refusal for the usage limit to
    /// `report_refusal`.
    pub(crate) fn sessions(
        &self,
        task: &Task,
        store: Arc<dyn Store>,
        harness: Arc<dyn Harness>,
        config: &FactoryConfig,
        clones: Clones,
        report_refusal: &UnboundedSender<UsageLimit>,
    ) -> Sessions {
        Sessions {
            store,
            harness,
            bash: config.bash.clone(),
            clock: self.clock.clone(),
            identity: task.properties().identity.clone(),
            default_branch: self.settings.default_branch.clone(),
            secrets: config.secrets.clone(),
            clones,
            environment: self.environment.clone(),
            gh: self.gh(),
            result_retries: config.result_retries,
            report_refusal: report_refusal.clone(),
        }
    }
}
