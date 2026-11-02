//! The environment: the repository made ready for agents' work once, by the
//! setup command, that every session's clone starts from
//! (`docs/specs/sessions.md`, Environment).

mod kept;
pub mod lifecycle;

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use tokio::task::{JoinError, JoinHandle};
use tracing::{info, warn};

use crate::bash_command::{self, Status};
use crate::child;
use crate::clock::Clock;
use crate::domain::Timestamp;
use crate::git;
use crate::harness::Harness;
use crate::redaction::Redaction;
use crate::session_env::Given;
use crate::store::Store;
use crate::transcript::MaintenanceEnded;
use crate::work_settings::SetupSettings;

use kept::{BuildRecord, Builds, Built};
use lifecycle::{Basis, Effect, Event, Fingerprint, State};

/// What a build needs of Linewright.
pub struct Builder {
    pub store: Arc<dyn Store>,
    pub harness: Arc<dyn Harness>,
    /// The `bash` that runs the setup command.
    pub bash: PathBuf,
    pub clock: Arc<dyn Clock>,
    /// Linewright's secrets, redacted from what a build keeps.
    pub redaction: Redaction,
    /// The variables the whole pipeline's sessions are given, which a build
    /// is given too.
    pub given: Arc<Given>,
    pub default_branch: String,
}

impl Builder {
    /// The whole environment a build runs in, its temporary directory being
    /// `tmp`.
    fn variables(&self, tmp: &Path) -> Vec<(OsString, OsString)> {
        self.harness.environment(&self.given, tmp, None)
    }

    /// The fingerprint of `settings`' command and the variables it runs
    /// with.
    fn fingerprint(&self, settings: &SetupSettings) -> Fingerprint {
        Fingerprint::of(settings.command.as_str(), self.variables(Path::new("/")))
    }
}

/// The `setup` table as the repository's files gave it at a poll.
#[derive(Clone, Debug)]
pub enum SetupRead {
    /// `config.toml` cannot be used, which holds the builds too
    /// (`docs/specs/repository.md`, Linewright's files).
    Unusable,
    /// `None` without a setup command.
    Given(Option<SetupSettings>),
}

/// The `setup` table as polls have given it.
enum Setup {
    Unset,
    Set(SetupSettings),
    /// The setup command was removed after this one, which a build queued
    /// meanwhile runs.
    Removed(SetupSettings),
}

impl Setup {
    fn active(&self) -> Option<&SetupSettings> {
        match self {
            Setup::Set(settings) => Some(settings),
            Setup::Unset | Setup::Removed(_) => None,
        }
    }

    fn last(&self) -> Option<&SetupSettings> {
        match self {
            Setup::Set(settings) | Setup::Removed(settings) => Some(settings),
            Setup::Unset => None,
        }
    }

    /// This, after a poll gave `given`.
    fn after(self, given: Option<SetupSettings>) -> Setup {
        match (given, self) {
            (Some(settings), _) => Setup::Set(settings),
            (None, Setup::Set(last) | Setup::Removed(last)) => Setup::Removed(last),
            (None, Setup::Unset) => Setup::Unset,
        }
    }
}

/// What holds back the builds that polls and requests would start.
#[derive(Clone, Copy, Debug, Default)]
struct Holds {
    /// Polls start none once the environment is prepared for a run without
    /// `serve`.
    prepared: bool,
    draining: bool,
    /// The `config.toml` last read cannot be used.
    unusable: bool,
}

impl Holds {
    fn any(self) -> bool {
        self.prepared || self.draining || self.unusable
    }
}

/// The environment and its builds, which the factory drives from its polls.
pub struct Environment {
    builds: Builds,
    builder: Arc<Builder>,
    state: State,
    /// What the state file names, and its build: the environment sessions
    /// start from.
    built: Option<(Built, Arc<BuildDir>)>,
    /// The latest build started since Linewright started.
    tried: Option<Basis>,
    setup: Setup,
    holds: Holds,
    building: Option<JoinHandle<Result<Built, String>>>,
    /// Why the latest build that failed did, if one has since Linewright
    /// started.
    failure: Option<String>,
    source: Source,
}

/// How a build ended, for [`Environment::ended`].
pub struct BuildEnd(Result<Result<Built, String>, JoinError>);

impl Environment {
    /// Opens the environment kept under `dir`, removing every build the
    /// state file doesn't name but the record of the latest that failed.
    pub async fn open(dir: PathBuf, builder: Builder) -> Result<Environment> {
        let builds = Builds::new(dir);
        let built = builds
            .open()
            .await
            .context("opening the environment's builds")?
            .map(|built| {
                let build = Arc::new(BuildDir::new(builds.build_dir(built.id)));
                (built, build)
            });
        Ok(Environment {
            state: if built.is_some() {
                State::Ready
            } else {
                State::None
            },
            builds,
            builder: Arc::new(builder),
            built,
            tried: None,
            setup: Setup::Unset,
            holds: Holds::default(),
            building: None,
            failure: None,
            source: Source::default(),
        })
    }

    /// What sessions copy the environment from.
    pub fn source(&self) -> Source {
        self.source.clone()
    }

    /// Takes the `setup` table read at a poll, and starts the build that
    /// `serve` starting or a trigger asks for. Without a setup command there
    /// is no environment, and what is kept of it stays as it is.
    pub fn poll(&mut self, read: SetupRead, now: Timestamp) {
        let Some(settings) = self.take_read(read) else {
            return;
        };
        if self.holds.any() {
            return;
        }
        if let Some(event) = self.polled(&settings, now) {
            info!(?event, "the environment is to be built");
            self.on(event, None);
        }
    }

    /// Builds the environment again, as asked: once the build in progress
    /// ends, if there is one.
    pub fn rebuild(&mut self) {
        if self.setup.active().is_some() && !self.holds.any() {
            self.on(Event::Triggered, None);
        }
    }

    /// Whether sessions can start: with no setup command, or from an
    /// environment.
    pub fn ready(&self) -> bool {
        self.setup.active().is_none() || self.state.has_environment()
    }

    pub fn is_building(&self) -> bool {
        self.building.is_some()
    }

    /// Ends once the build in progress ends; never without one.
    pub async fn next_build_end(&mut self) -> BuildEnd {
        match &mut self.building {
            Some(handle) => BuildEnd(handle.await),
            None => std::future::pending().await,
        }
    }

    /// Takes in how the build in progress ended.
    pub fn ended(&mut self, BuildEnd(ended): BuildEnd) {
        self.building = None;
        match ended {
            Ok(Ok(built)) => {
                info!(commit = built.commit, "the environment was built");
                self.failure = None;
                self.on(Event::Succeeded, Some(built));
            }
            Ok(Err(why)) => self.failed(why),
            Err(e) if e.is_cancelled() => {}
            Err(e) => self.failed(format!("the build panicked: {e}")),
        }
    }

    fn failed(&mut self, why: String) {
        warn!(error = why, "the build of the environment failed");
        self.failure = Some(why);
        self.on(Event::Failed, None);
    }

    /// Waits for the build in progress, and those queued after it, to end.
    pub async fn settle(&mut self) {
        while self.is_building() {
            let end = self.next_build_end().await;
            self.ended(end);
        }
    }

    /// A drain began: no build starts any more.
    pub fn drain(&mut self) {
        self.holds.draining = true;
        self.state = lifecycle::unqueued(self.state);
    }

    /// `serve` exits: the build in progress, if any, is abandoned.
    pub async fn exit(&mut self) {
        self.on(Event::Exiting, None);
        if let Some(handle) = self.building.take() {
            let _ = handle.await;
        }
    }

    /// Makes the environment ready for a run without `serve`, with the
    /// `setup` table `read`: it is used when no trigger applies to it, and
    /// otherwise built first. Polls start no build from then on. `Err` says
    /// why no environment can be had.
    pub async fn prepare(&mut self, read: SetupRead, now: Timestamp) -> Result<()> {
        let settings = self.take_read(read);
        if let Some(settings) = settings.filter(|_| !self.holds.any())
            && let Some(event) = self.polled(&settings, now)
        {
            self.on(event, None);
            self.settle().await;
        }
        self.holds.prepared = true;
        if self.ready() {
            return Ok(());
        }
        Err(anyhow!(
            "the build of the environment failed: {}",
            self.failure.as_deref().unwrap_or("it ended")
        ))
    }

    /// Takes `read` in, and returns the `setup` table now in force, if any.
    fn take_read(&mut self, read: SetupRead) -> Option<SetupSettings> {
        match read {
            SetupRead::Unusable => {
                self.holds.unusable = true;
                self.state = lifecycle::unqueued(self.state);
            }
            SetupRead::Given(given) => {
                self.holds.unusable = false;
                self.setup = std::mem::replace(&mut self.setup, Setup::Unset).after(given);
            }
        }
        self.publish();
        self.setup.active().cloned()
    }

    /// The event a poll with `settings` brings.
    fn polled(&self, settings: &SetupSettings, now: Timestamp) -> Option<Event> {
        let built = self.built.as_ref().map(|(built, _)| built.basis());
        lifecycle::polled(
            self.state,
            built.as_ref(),
            self.tried.as_ref(),
            &self.builder.fingerprint(settings),
            settings.max_age,
            now,
        )
    }

    /// Lets sessions copy from the environment there is.
    fn publish(&self) {
        let current = self
            .built
            .as_ref()
            .filter(|_| self.setup.active().is_some())
            .map(|(_, build)| build.clone());
        *self.source.0.lock().expect("no copy panics holding it") = current;
    }

    /// Takes `event` in, `succeeded` being the build that succeeded, if it
    /// is one.
    fn on(&mut self, event: Event, mut succeeded: Option<Built>) {
        let (next, effects) = lifecycle::on(self.state, event);
        self.state = next;
        for effect in effects {
            match effect {
                Effect::StartBuild => self.start_build(),
                Effect::Replace => {
                    let built = succeeded.take().expect("a build that succeeded replaces");
                    self.replace(built);
                }
                Effect::KeepLast => info!("the environment as last built stays"),
                Effect::Abandon => {
                    if let Some(handle) = &self.building {
                        info!("the build of the environment in progress is abandoned");
                        handle.abort();
                    }
                }
            }
        }
    }

    fn start_build(&mut self) {
        let settings = self
            .setup
            .last()
            .cloned()
            .expect("a build starts only once a poll gave a setup command");
        let fingerprint = self.builder.fingerprint(&settings);
        self.tried = Some(Basis {
            fingerprint: fingerprint.clone(),
            since: self.builder.clock.now(),
        });
        info!(
            command = settings.command.as_str(),
            "the environment is building"
        );
        let (builder, builds) = (self.builder.clone(), self.builds.clone());
        self.building = Some(tokio::spawn(async move {
            let abandoned = Abandoned::default();
            build(&builder, &builds, &settings, fingerprint, &abandoned).await
        }));
    }

    /// Makes `built`, which the state file names, the environment. The build
    /// it replaces is removed once no copy from it is in progress.
    fn replace(&mut self, built: Built) {
        let build = Arc::new(BuildDir::new(self.builds.build_dir(built.id)));
        if let Some((_, replaced)) = self.built.replace((built, build)) {
            replaced.retire();
        }
        self.publish();
    }
}

/// Abandoning a build mid-way aborts its task, which drops this: what is
/// left of the build is removed, as at the next start.
#[derive(Default)]
struct Abandoned {
    dir: Mutex<Option<PathBuf>>,
}

impl Abandoned {
    fn building_in(&self, dir: &Path) {
        *self.dir.lock().expect("never poisoned") = Some(dir.to_path_buf());
    }

    fn done(&self) {
        *self.dir.lock().expect("never poisoned") = None;
    }
}

impl Drop for Abandoned {
    fn drop(&mut self) {
        if let Some(dir) = self.dir.get_mut().ok().and_then(Option::take) {
            remove_aside(dir);
        }
    }
}

/// Removes the build `dir` unless a copy from it is in progress, off the
/// runtime's threads when there is one.
fn remove_aside(dir: PathBuf) {
    let remove = move || {
        if let Err(e) = kept::remove_unleased(&dir) {
            warn!(error = format!("{e:#}"), "removing a build failed");
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => drop(runtime.spawn_blocking(remove)),
        Err(_) => remove(),
    }
}

/// Builds the environment with `settings` under `builds`' lock, and names
/// the build in the state file once it succeeds. `Err` says why it failed.
async fn build(
    builder: &Builder,
    builds: &Builds,
    settings: &SetupSettings,
    fingerprint: Fingerprint,
    abandoned: &Abandoned,
) -> Result<Built, String> {
    let _lock = builds.lock().await.map_err(|e| format!("{e:#}"))?;
    let id = builds.next_id().map_err(|e| format!("{e:#}"))?;
    let dir = builds.build_dir(id);
    abandoned.building_in(&dir);
    let at = builder.clock.now();
    let mut record = BuildRecord::started(builder.redaction.apply(settings.command.as_str()), at);
    let why = match run(builder, &dir, settings, &mut record).await {
        Ok((commit, ran)) if ran.status == Status::Exited(0) => {
            let built = Built {
                id,
                commit,
                at,
                fingerprint,
            };
            match kept::remove(&dir.join(kept::TMP)).and_then(|()| builds.write_state(&built)) {
                Ok(()) => {
                    abandoned.done();
                    return Ok(built);
                }
                Err(e) => kept_error(builder, &mut record, &dir, &e),
            }
        }
        Ok((_, ran)) => format!(
            "the setup command {}; its output is in {}",
            describe(ran.status, settings),
            dir.join(kept::OUTPUT).display()
        ),
        Err(e) => kept_error(builder, &mut record, &dir, &e),
    };
    let removed = kept::remove(&dir.join(kept::REPO))
        .and_then(|()| kept::remove(&dir.join(kept::TMP)))
        .and_then(|()| builds.keep_only_the_latest_failed());
    abandoned.done();
    if let Err(e) = removed {
        warn!(
            error = format!("{e:#}"),
            "removing what failed builds left failed"
        );
    }
    Err(why)
}

/// `error`, redacted, kept in `record` in `dir` as why its build failed.
fn kept_error(
    builder: &Builder,
    record: &mut BuildRecord,
    dir: &Path,
    error: &anyhow::Error,
) -> String {
    let error = builder.redaction.apply(&format!("{error:#}"));
    record.error = Some(error.clone());
    if let Err(e) = record.save(dir) {
        warn!(error = format!("{e:#}"), "keeping a build's record failed");
    }
    error
}

/// How a setup command that did not succeed ended, after "the setup
/// command".
fn describe(status: Status, settings: &SetupSettings) -> String {
    match status {
        Status::Exited(code) => format!("exited with status {code}"),
        Status::Signalled(signal) => format!("was ended by signal {signal}"),
        Status::TimedOut => format!(
            "ran longer than `setup.timeout`, {}",
            humantime::format_duration(settings.timeout)
        ),
        Status::Cancelled => "was cancelled".into(),
    }
}

/// Runs the setup command in a clone of the default branch in `dir`, which
/// keeps `record`, and returns the commit it cloned and how the command
/// ran. Once the command ends, what it left running is ended.
async fn run(
    builder: &Builder,
    dir: &Path,
    settings: &SetupSettings,
    record: &mut BuildRecord,
) -> Result<(String, bash_command::Ran)> {
    kept::create(dir).await?;
    record.save(dir)?;
    let repo = dir.join(kept::REPO);
    builder
        .store
        .clone_repo(
            &repo,
            &builder.default_branch,
            &[("maintenance.auto", "false")],
        )
        .await
        .context("cloning the default branch")?;
    let commit = git::head(&repo).await?;
    record.commit = Some(commit.clone());
    record.save(dir)?;
    let tmp = dir.join(kept::TMP);
    tokio::fs::create_dir(&tmp)
        .await
        .context("creating the build's temporary directory")?;
    let request = bash_command::Request {
        command: settings.command.as_str(),
        dir: &repo,
        environment: builder.variables(&tmp),
        deadline: tokio::time::Instant::now() + settings.timeout,
        log: &dir.join(kept::OUTPUT),
    };
    let (ran, leftovers) = bash_command::run(
        &builder.bash,
        request,
        &builder.redaction,
        std::future::pending(),
    )
    .await
    .context("running the setup command")?;
    tokio::task::spawn_blocking(move || leftovers.end())
        .await
        .context("ending what the setup command left running")?;
    record.line.ended = Some(MaintenanceEnded {
        status: ran.status,
        duration_seconds: ran.duration.as_secs_f64(),
        output: ran.output.clone(),
    });
    record.save(dir)?;
    Ok((commit, ran))
}

/// A build's directory, removed once it is retired and no copy from it is
/// in progress.
#[derive(Debug)]
struct BuildDir {
    path: PathBuf,
    retired: AtomicBool,
}

impl BuildDir {
    fn new(path: PathBuf) -> BuildDir {
        BuildDir {
            path,
            retired: AtomicBool::new(false),
        }
    }

    fn retire(&self) {
        self.retired.store(true, Ordering::Relaxed);
    }
}

impl Drop for BuildDir {
    fn drop(&mut self) {
        if self.retired.load(Ordering::Relaxed) {
            remove_aside(std::mem::take(&mut self.path));
        }
    }
}

/// Where sessions copy the environment from, shared with the runs.
#[derive(Clone, Debug, Default)]
pub struct Source(Arc<Mutex<Option<Arc<BuildDir>>>>);

impl Source {
    /// The environment as it is now, held until the lease is dropped:
    /// `None` without one.
    pub fn lease(&self) -> Option<Lease> {
        let current = self.0.lock().expect("no copy panics holding it").clone();
        current.map(|build| Lease { build })
    }
}

/// A hold on one build of the environment, which is not removed while one
/// is held.
pub struct Lease {
    build: Arc<BuildDir>,
}

impl Lease {
    /// Copies every path git ignores in the environment into `clone`, as a
    /// copy-on-write copy where the filesystem allows, never overwriting a
    /// file `clone` has (`docs/specs/sessions.md`, Environment). No process
    /// removes the build while the copy is in progress.
    pub async fn copy_into(&self, clone: &Path) -> Result<()> {
        let _copying = kept::lease(&self.build.path).await?;
        let repo = self.build.path.join(kept::REPO);
        let listed = git::git_bytes(
            &repo,
            &[
                "ls-files",
                "-z",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
            ],
        )
        .await
        .context("listing the environment's ignored paths")?;
        for path in listed
            .split(|&byte| byte == 0)
            .filter(|path| !path.is_empty())
        {
            let path: PathBuf = Path::new(OsStr::from_bytes(path)).components().collect();
            copy(&repo.join(&path), &clone.join(&path))
                .await
                .with_context(|| format!("copying {} from the environment", path.display()))?;
        }
        Ok(())
    }
}

/// Copies `from` to `to`, merging into a directory `to` already is and
/// leaving each file already there as it is.
fn copy<'a>(
    from: &'a Path,
    to: &'a Path,
) -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let Some(there) = metadata_if_any(to).await? else {
            return copy_absent(from, to).await;
        };
        if !there.is_dir() || !tokio::fs::symlink_metadata(from).await?.is_dir() {
            return Ok(());
        }
        let mut entries = tokio::fs::read_dir(from).await?;
        while let Some(entry) = entries.next_entry().await? {
            copy(&entry.path(), &to.join(entry.file_name())).await?;
        }
        Ok(())
    })
}

async fn metadata_if_any(path: &Path) -> Result<Option<std::fs::Metadata>> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Copies `from` to `to`, which does not exist, as a copy-on-write copy
/// where the filesystem allows one.
async fn copy_absent(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // GNU cp clones with `--reflink=auto`
    // (https://www.gnu.org/software/coreutils/manual/html_node/cp-invocation.html),
    // and macOS's with `-c`, falling back to a plain copy where cloning is
    // not supported (https://keith.github.io/xcode-man-pages/cp.1.html).
    let clone = if cfg!(target_os = "macos") {
        "-c"
    } else {
        "--reflink=auto"
    };
    let output = child::command("cp")
        .args(["-a", clone])
        .arg(from)
        .arg(to)
        .output()
        .await
        .context("running cp")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}
