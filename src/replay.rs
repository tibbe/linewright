//! Running a case's stage on the case, in place of GitHub
//! (`docs/specs/benchmarks.md`, Replaying).

mod journal;

pub use journal::Journal;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result, bail};
use chrono::TimeDelta;
use tracing::{error, info};

use crate::case::Case;
use crate::clock::{Clock, RunningFrom, SystemClock};
use crate::clones::Clones;
use crate::environment::{self, replays};
use crate::factory::FactoryConfig;
use crate::harness::Harness;
use crate::repo_files::{RepoLayer, holding};
use crate::run::{self, Cancellation, Shell, ShellParts};
use crate::runs::{Begin, Ending, OutcomeName, Runs, Unended};
use crate::serving::Signals;
use crate::store::Store;
use crate::store::local::LocalStore;
use crate::winding_up::WindingUp;
use crate::work_settings::RunKind;
use crate::{COMMAND, VERSION_AND_COMMIT};

const OUTCOME: &str = "outcome";
const VERSION: &str = "version";
const BUILD_LOG: &str = "build.log";
const COMMITS: &str = "commits.git";

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
        let path = self.dir.join(name);
        let partial = self.dir.join(format!(".{name}"));
        std::fs::write(&partial, text).with_context(|| format!("writing {}", path.display()))?;
        std::fs::rename(&partial, &path).with_context(|| format!("writing {}", path.display()))
    }
}

/// Runs the stage of `case` on its item with `config` and `harness`, with
/// the case in place of GitHub, and keeps what it did in `out`: the version
/// first, `in-flight` as the outcome once the run begins, and its ending once
/// it ends. `config`'s work directory is that of the case's repository. The
/// run's clones are kept in a directory of this replay's own under it,
/// removed through `config`'s winding up once the run ends, or by a later
/// replay when a signal ends this one first.
///
/// # Errors
///
/// When the output cannot be written, the case's model of GitHub cannot be
/// seeded, a file on the case's default branch would hold the run, the
/// environment cannot be had or its build fails, or the run cannot be begun:
/// then the run does not run. Once the run ended, only when its ending
/// cannot be written.
pub async fn run(
    case: &Case,
    out: &Output,
    harness: Arc<dyn Harness>,
    config: FactoryConfig,
) -> Result<Ending> {
    out.write(VERSION, &format!("{COMMAND} {VERSION_AND_COMMIT}\n"))?;
    let replays = Clones::new(&config.work_dir.join("replays"), config.winding_up.clone());
    replays.sweep();
    let replay_dir = replays
        .make("replay")
        .await
        .context("creating the replay's own directory")?;
    let clones_winding_up = WindingUp::default();
    let ran = run_in(
        case,
        out,
        harness,
        config,
        replay_dir.path(),
        &clones_winding_up,
    )
    .await;
    if let Ok(ending) = ran {
        out.ended(ending)?;
    }
    clones_winding_up.wait().await;
    drop(replay_dir);
    ran
}

async fn run_in(
    case: &Case,
    out: &Output,
    harness: Arc<dyn Harness>,
    config: FactoryConfig,
    replay_dir: &Path,
    clones_winding_up: &WindingUp,
) -> Result<Ending> {
    let clock: Arc<dyn Clock> = Arc::new(RunningFrom::new(case.start()));
    let seeded = LocalStore::from_case(
        &replay_dir.join("store"),
        &out.path().join(COMMITS),
        case,
        clock.clone(),
    )
    .await
    .context("seeding the model of GitHub from the case")?;
    let store: Arc<dyn Store> = Arc::new(Journal::new(Arc::new(seeded), out.path()));
    let properties = case.properties();
    let mut settings = properties.settings.clone();
    if let Some(method) = config.merge_method {
        settings.merge_method = method;
    }
    let kind = case.stage().kind();
    let repo = RepoLayer::read(store.as_ref(), &settings.default_branch, &config.prompts)
        .await
        .context("reading Linewright's files in the case")?;
    if let Some(file) = holding(&repo.unusable, &RunKind::Stage(kind)).first() {
        bail!(
            "the case cannot be replayed: {} on its default branch would hold the run: {}",
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
        harness: harness.clone(),
        bash: config.bash.clone(),
        clock: Arc::new(SystemClock),
        redaction: config.secrets.redaction().clone(),
        given: config.environments.pipeline.clone(),
        default_branch: settings.default_branch.clone(),
        winding_up: config.winding_up.clone(),
    };
    let max_age = TimeDelta::from_std(config.work.setup_max_age(&repo.settings))
        .context("`setup.max-age` is too long")?;
    let prepared = replays::prepare(
        &replays::dir(&config.work_dir),
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
                out.write(BUILD_LOG, output)?;
            }
            in_use
        }
        Err(replays::Unprepared::Failed(failed)) => {
            if let Some(output) = &failed.output {
                out.write(BUILD_LOG, output)?;
            }
            return Err(failed.into());
        }
        Err(e @ replays::Unprepared::Unavailable(_)) => return Err(e.into()),
    };
    let (report_refusal, _refusals) = tokio::sync::mpsc::unbounded_channel();
    let runs = Runs::new(out.path());
    let shell = Shell::new(ShellParts {
        store,
        harness,
        bash: config.bash,
        clock: clock.clone(),
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
        identity: properties.identity.clone(),
        settings,
        result_retries: config.result_retries,
        work_dir: config.work_dir,
        runs: runs.clone(),
        waits_out_usage_limit: false,
        records_input: false,
        secrets: config.secrets,
        report_refusal,
        environment: environment.source(),
        clones: Clones::new(replay_dir, clones_winding_up.clone()),
    });
    let now = clock.now();
    let snapshot = shell
        .store
        .snapshot(shell.policy.windows(now))
        .await
        .context("taking a snapshot of the case")?;
    let issue = case.item();
    let work = case.stage().work(issue);
    let running = runs.begin(
        Begin::Work {
            item: issue,
            stage: kind,
        },
        now,
    )?;
    out.in_flight()?;
    info!(run = %running.id(), case = %case.run(), "replaying");
    let ctx = run::Ctx {
        with: shell.run_with(&repo, RunKind::Stage(kind)),
        as_of: now,
        reworked: None,
        cancel: Cancellation::default(),
        run: running,
    };
    Ok(run::run(Arc::new(shell), work, Arc::new(snapshot), ctx).await)
}

/// Runs `work` unless a signal arrives first. Then it drops `work`, which
/// ends the commands it runs, writes `interrupted` as `out`'s outcome unless
/// the run's ending is written, and exits the process at once: with the
/// signal's status, or 0 once the run ended (`docs/specs/operation.md`,
/// Stopping).
///
/// # Errors
///
/// When it cannot listen for signals; `work` does not run then.
pub async fn unless_signalled<T>(out: &Output, work: impl Future<Output = T>) -> Result<T> {
    let mut signals = Signals::new().context("listening for signals")?;
    let signal = tokio::select! {
        done = work => return Ok(done),
        signal = signals.next() => signal,
    };
    let status = match out.interrupted() {
        Ok(false) => 0,
        Ok(true) => signal.exit_status(),
        Err(e) => {
            error!(error = format!("{e:#}"), "writing the outcome failed");
            signal.exit_status()
        }
    };
    #[expect(
        clippy::exit,
        reason = "a signal ends a replay at once, leaving in its output what its run did"
    )]
    std::process::exit(status);
}
