//! Running a case's stage on the case, in place of GitHub
//! (`docs/specs/benchmarks.md`, Replaying).

mod gh_gate;
mod journal;
pub mod judge;

pub use gh_gate::{Gate, GhConfigDir, Upstream};
pub use journal::Journal;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result, bail};
use chrono::TimeDelta;
use tracing::{error, info};

use crate::case::Case;
use crate::clock::{Clock, RunningFrom, SystemClock};
use crate::clones::Clones;
use crate::ending::Signal;
use crate::environment::{self, Source, replays};
use crate::factory::FactoryConfig;
use crate::harness::{GhReach, Harness};
use crate::repo_files::{RepoLayer, holding};
use crate::run::{self, Shell, ShellParts};
use crate::runs::files::RunFiles;
use crate::runs::{Begin, Ending, OutcomeName, Unended};
use crate::serving::Signals;
use crate::session::{Cancellation, Sessions};
use crate::store::local::LocalStore;
use crate::store::{RepoSettings, Store};
use crate::winding_up::WindingUp;
use crate::work_settings::RunKind;
use crate::{COMMAND, VERSION_AND_COMMIT};

const OUTCOME: &str = "outcome";
/// The directory of the output that keeps the sessions' records, as the
/// runs of the replay keep them in the output.
const SESSIONS: &str = crate::runs::SESSIONS;
/// The directory of the output that keeps the writes.
const WRITES: &str = "writes";
/// The directory of the output that keeps the attachments.
const ATTACHMENTS: &str = "attachments";
const VERSION: &str = "version";
const CASE: &str = "case";
const BUILD_LOG: &str = "build.log";
const COMMITS: &str = "commits.git";

/// The link to the attachment `name` from the writes, its path from them.
fn attachment_link(name: &str) -> String {
    format!("../{ATTACHMENTS}/{name}")
}

/// The directory of a replay's output: one that held nothing.
#[derive(Clone, Debug)]
pub struct Output {
    dir: PathBuf,
    /// The ending written as the outcome, once it is: no outcome replaces
    /// it.
    ended: Arc<Mutex<Option<Ending>>>,
}

/// Why a directory cannot take a replay's output.
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
        let unusable = |error| OutputRefusal::Unusable {
            dir: dir.to_path_buf(),
            error,
        };
        std::fs::create_dir_all(dir).map_err(unusable)?;
        let dir = std::path::absolute(dir).map_err(unusable)?;
        if std::fs::read_dir(&dir).map_err(unusable)?.next().is_some() {
            return Err(OutputRefusal::NotEmpty(dir));
        }
        Ok(Output {
            dir,
            ended: Arc::default(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Writes `interrupted` as the outcome unless the run's ending is
    /// written, and gives the status the replay exits with for `signal`:
    /// its own, or 0 once the run ended.
    pub fn signalled(&self, signal: Signal) -> i32 {
        match self.interrupted() {
            Ok(false) => 0,
            Ok(true) => signal.exit_status(),
            Err(e) => {
                error!(error = format!("{e:#}"), "writing the outcome failed");
                signal.exit_status()
            }
        }
    }

    /// Writes `interrupted` as the outcome unless the run's ending is
    /// written, and gives whether it wrote it.
    ///
    /// # Errors
    ///
    /// When the file cannot be written.
    pub fn interrupted(&self) -> Result<bool> {
        let ended = self.lock_ended();
        if ended.is_some() {
            return Ok(false);
        }
        self.write_outcome(OutcomeName::Unended(Unended::Interrupted))?;
        Ok(true)
    }

    fn in_flight(&self) -> Result<()> {
        let _ended = self.lock_ended();
        self.write_outcome(OutcomeName::Unended(Unended::InFlight))
    }

    fn ended(&self, ending: Ending) -> Result<()> {
        let mut ended = self.lock_ended();
        self.write_outcome(OutcomeName::Ended(ending))?;
        *ended = Some(ending);
        Ok(())
    }

    fn lock_ended(&self) -> MutexGuard<'_, Option<Ending>> {
        self.ended
            .lock()
            .expect("the output's lock is not poisoned")
    }

    fn write_outcome(&self, outcome: OutcomeName) -> Result<()> {
        let name = serde_json::to_value(outcome)?;
        let name = name.as_str().context("an outcome's name is a string")?;
        self.write(OUTCOME, &format!("{name}\n"))
    }

    /// Replaces the file `name` with one holding `text`, so that a reader
    /// sees the one or the other whole.
    fn write(&self, name: &str, text: &str) -> Result<()> {
        write_whole(
            &self.dir.join(name),
            &self.dir.join(format!(".{name}")),
            text,
        )
    }
}

/// Replaces the file at `path` with one holding `text`, written first at
/// `partial` in the same directory, so that a reader sees the one or the
/// other whole. A failed write leaves no file at `partial`.
fn write_whole(path: &Path, partial: &Path, text: &str) -> Result<()> {
    let written = std::fs::write(partial, text).and_then(|()| std::fs::rename(partial, path));
    if written.is_err() {
        let _ = std::fs::remove_file(partial);
    }
    written.with_context(|| format!("writing {}", path.display()))
}

/// Runs the stage of `case` on its item with `config` and `harness`, with
/// the case in place of GitHub, and keeps what it did in `out`: the version
/// and the ID of the case's run first, `in-flight` as the outcome once the
/// run begins, and its ending once it ends. Its sessions' `gh` cannot read the case's repository on GitHub
/// (`docs/specs/benchmarks.md`, The repository on GitHub). `config`'s work
/// directory is that of the case's repository. The run's clones are kept in
/// a directory of this replay's own under it, removed through `config`'s
/// winding up once the run ends, or by a later replay when a signal ends
/// this one first.
///
/// # Errors
///
/// When the output cannot be written, the case's model of GitHub cannot be
/// seeded, a file on the case's default branch would hold the run, the
/// environment cannot be had or its build fails, the gate of the sessions'
/// `gh` cannot be started, or the run cannot be begun: then the run does not
/// run. Once the run ended, only when its ending cannot be written.
pub async fn run(
    case: &Case,
    out: &Output,
    harness: Arc<dyn Harness>,
    config: FactoryConfig,
) -> Result<Ending> {
    out.write(VERSION, &format!("{COMMAND} {VERSION_AND_COMMIT}\n"))?;
    out.write(CASE, &format!("{}\n", case.run()))?;
    let (work_dir, winding_up) = (config.work_dir.clone(), config.winding_up.clone());
    in_own_dir(
        &work_dir,
        winding_up,
        "replay",
        async |replay_dir, clones_winding_up| {
            let ending = run_in(case, out, harness, config, replay_dir, clones_winding_up).await?;
            out.ended(ending)?;
            Ok(ending)
        },
    )
    .await?
}

/// Runs `work` in a new directory of its own, named `prefix`, among the
/// replays' under `work_dir`, with what winds up the clones it makes there,
/// and removes the directory through `winding_up` once those are wound up.
/// The directories of earlier ones that a signal ended are removed
/// meanwhile.
///
/// # Errors
///
/// When the directory cannot be made: `work` does not run then.
async fn in_own_dir<T>(
    work_dir: &Path,
    winding_up: WindingUp,
    prefix: &str,
    work: impl AsyncFnOnce(&Path, &WindingUp) -> T,
) -> Result<T> {
    let replays = Clones::new(&work_dir.join("replays"), winding_up);
    replays.sweep();
    let own = replays
        .make(prefix)
        .await
        .with_context(|| format!("creating the {prefix}'s own directory"))?;
    let clones_winding_up = WindingUp::default();
    let done = work(own.path(), &clones_winding_up).await;
    clones_winding_up.wait().await;
    drop(own);
    Ok(done)
}

async fn run_in(
    case: &Case,
    out: &Output,
    harness: Arc<dyn Harness>,
    config: FactoryConfig,
    replay_dir: &Path,
    clones_winding_up: &WindingUp,
) -> Result<Ending> {
    let kind = case.stage().kind();
    let setting = Setting::prepare(
        case,
        &RunKind::Stage(kind),
        &config,
        harness.clone(),
        &replay_dir.join("store"),
        &out.path().join(COMMITS),
        |output| out.write(BUILD_LOG, output),
    )
    .await?;
    let store: Arc<dyn Store> = Arc::new(Journal::new(setting.store.clone(), out.path()));
    let clock = setting.clock.clone();
    let settings = setting.settings.clone();
    let shell = Shell::new(ShellParts {
        sessions: setting.sessions(
            case,
            store,
            harness,
            &config,
            Clones::new(replay_dir, clones_winding_up.clone()),
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
        operator_prompts: config.prompts,
        settings,
        work_dir: config.work_dir,
    });
    let now = clock.now();
    let snapshot = shell
        .sessions
        .store
        .snapshot(shell.policy.windows(now))
        .await
        .context("taking a snapshot of the case")?;
    let issue = case.item();
    let work = case.stage().work(issue);
    let running = RunFiles::new(out.path())
        .begin(
            Begin::Work {
                item: issue,
                stage: kind,
            },
            now,
        )
        .await?;
    out.in_flight()?;
    info!(run = %running.id(), case = %case.run(), "replaying");
    let ctx = run::Ctx {
        with: shell.run_with(&setting.repo, RunKind::Stage(kind)),
        as_of: now,
        reworked: None,
        cancel: Cancellation::default(),
        run: running,
    };
    let ended = run::run(Arc::new(shell), work, Arc::new(snapshot), ctx).await;
    drop(setting);
    Ok(ended)
}

/// What a case gives the sessions run on it in place of GitHub
/// (`docs/specs/benchmarks.md`, Replaying): a model of GitHub seeded from
/// the case, Linewright's files on its default branch, its environment and
/// a `gh` kept from the case's repository. That `gh` reaches GitHub only
/// while the setting lives.
pub struct Setting {
    /// The model of GitHub, as the case's run found it.
    pub store: Arc<dyn Store>,
    /// The clock of the model, running from the case's start.
    pub clock: Arc<dyn Clock>,
    /// Linewright's files on the case's default branch.
    pub repo: RepoLayer,
    /// The repository's settings, as the case's properties and the config
    /// give them.
    pub settings: RepoSettings,
    /// What sessions copy the environment from.
    pub environment: Source,
    gate: Gate,
}

impl Setting {
    /// Prepares the setting of a `kind` of run on `case` with `config` and
    /// `harness`: seeds the model of GitHub at `root`, its repository at
    /// `repository`, refuses a file on the case's default branch that would
    /// hold `kind`, prepares the environment, handing the output of its
    /// build to `build_log` when this built it, and starts the gate of the
    /// sessions' `gh`.
    ///
    /// # Errors
    ///
    /// When the model of GitHub cannot be seeded, a file on the case's
    /// default branch would hold `kind`, the environment cannot be had or
    /// its build fails, `build_log` fails, or the gate cannot be started.
    pub async fn prepare(
        case: &Case,
        kind: &RunKind<'_>,
        config: &FactoryConfig,
        harness: Arc<dyn Harness>,
        root: &Path,
        repository: &Path,
        build_log: impl Fn(&str) -> Result<()>,
    ) -> Result<Setting> {
        let clock: Arc<dyn Clock> = Arc::new(RunningFrom::new(case.start()));
        let seeded = LocalStore::from_case(root, repository, case, clock.clone())
            .await
            .context("seeding the model of GitHub from the case")?;
        let store: Arc<dyn Store> = Arc::new(seeded);
        let properties = case.properties();
        let mut settings = properties.settings.clone();
        if let Some(method) = config.merge_method {
            settings.merge_method = method;
        }
        let repo = RepoLayer::read(store.as_ref(), &settings.default_branch, &config.prompts)
            .await
            .context("reading Linewright's files in the case")?;
        if let Some(file) = holding(&repo.unusable, kind).first() {
            let (refused, held) = match kind {
                RunKind::Judge => ("judged", "the judge"),
                RunKind::Stage(_) | RunKind::Automation(_) => ("replayed", "the run"),
            };
            bail!(
                "the case cannot be {refused}: {} on its default branch would hold {held}: {}",
                file.path,
                file.error
            );
        }
        let head = case
            .heads()
            .get(&settings.default_branch)
            .context("the case lacks the head of its default branch")?;
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
        let prepared = replays::prepare(
            &replays::dir(config.database.work_dir()),
            head,
            builder,
            config.work.setup(&repo.settings),
            SystemClock.now(),
            max_age,
        )
        .await;
        let environment = match prepared {
            Ok(in_use) => {
                if let Some(output) = in_use.built_output() {
                    build_log(output)?;
                }
                in_use.source()
            }
            Err(replays::Unprepared::Failed(failed)) => {
                if let Some(output) = &failed.output {
                    build_log(output)?;
                }
                return Err(failed.into());
            }
            Err(e @ replays::Unprepared::Unavailable(_)) => return Err(e.into()),
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

    /// How far the sessions' `gh` reaches: not to the case's repository.
    pub fn gh(&self) -> GhReach {
        self.gate.gh()
    }

    /// The parts the sessions run on `case` in this setting share, with
    /// `config` and `harness`: they fetch from and clone `store`, make their
    /// clones in `clones`, and end at the usage limit rather than wait it
    /// out.
    pub(crate) fn sessions(
        &self,
        case: &Case,
        store: Arc<dyn Store>,
        harness: Arc<dyn Harness>,
        config: &FactoryConfig,
        clones: Clones,
    ) -> Sessions {
        let (report_refusal, _refusals) = tokio::sync::mpsc::unbounded_channel();
        Sessions {
            store,
            harness,
            bash: config.bash.clone(),
            clock: self.clock.clone(),
            identity: case.properties().identity.clone(),
            default_branch: self.settings.default_branch.clone(),
            secrets: config.secrets.clone(),
            clones,
            environment: self.environment.clone(),
            gh: self.gh(),
            result_retries: config.result_retries,
            waits_out_usage_limit: false,
            report_refusal,
        }
    }
}

/// Runs `work` unless a signal arrives first. Then it drops `work`, which
/// ends the commands it runs, and exits the process at once with the status
/// `on_signal` gives for the signal (`docs/specs/operation.md`, Stopping).
///
/// # Errors
///
/// When it cannot listen for signals; `work` does not run then.
pub async fn unless_signalled<T>(
    work: impl Future<Output = T>,
    on_signal: impl FnOnce(Signal) -> i32,
) -> Result<T> {
    let mut signals = Signals::new().context("listening for signals")?;
    let signal = tokio::select! {
        done = work => return Ok(done),
        signal = signals.next() => signal,
    };
    let status = on_signal(signal);
    #[expect(
        clippy::exit,
        reason = "a signal ends a replay or its judge at once, leaving in the output what it did"
    )]
    std::process::exit(status);
}
