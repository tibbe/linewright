use std::io::{IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use clap::{CommandFactory as _, FromArgMatches as _};
use tracing::{error, info};

use crate::api::{self, EventId, RequestError, Resume, Unanswered};
use crate::automation::AutomationName;
use crate::benchmarks::{
    self, Benchmark, BenchmarkAnswer, BenchmarkId, BenchmarkRunAnswer, BenchmarkRunId,
    BenchmarkRunRecord, BenchmarkRunState, BenchmarkRuns, Benchmarks, BenchmarksAnswer,
    ConfigurationId, Repetitions, Shown, TaskId,
};
use crate::by_hand::HandTaken;
use crate::clock::{Clock, SystemClock};
use crate::config::{
    AutomationCommand, BenchmarkCommand, BenchmarkRunCommand, Cli, Command, Config,
    EnvironmentCommand, Invocation, LogFormat, ResolveError, Settings, Since,
};
use crate::database::Database;
use crate::domain::{Number, Repo, Timestamp};
use crate::drain::{self, Asked, Exiting, Said, Stop, Taken, Timeout, Waiting, Was};
use crate::ending::{Ends, StopBy};
use crate::environment::shown::{self, Followed, FollowingBuild};
use crate::environment::{self, BuildEnded, BuildId, RebuildSaid, RebuildTaken};
use crate::export::{self, ExportError};
use crate::factory::{Factory, FactoryConfig, SessionToken};
use crate::harness::{Harness, HarnessSettings, Program, Variables};
use crate::logs::{self, FollowQuery, Following, Which};
use crate::name::Name;
use crate::repo_files::RepoLayer;
use crate::runs::{self, Query, RepoWorkDir, RunId, Runs};
use crate::serving::{self, OnUsageLimit, Starting, UiRouter, briefs};
use crate::store::Store;
use crate::store::{SESSION_TOKEN_VARIABLE, TOKEN_VARIABLES};
use crate::task::{CRITERIA_REQUIRED, Criteria, Task, TaskStage};
use crate::transcript::{self, After, InFlight, Verbosity};
use crate::trial::scorer::{self, ScorerRecord};
use crate::trial::{self, Configuration, Output, Under};
use crate::winding_up::{Remaining, WindingUp};
use crate::work_settings::RepoSettings;
use crate::{COMMAND, VERSION, version_and_commit};
use crate::{contain, explain, status};

const SUCCESS: u8 = 0;

/// The exit code for any error but the ones with codes of their own.
const FAILURE: u8 = 1;

/// The exit code for a command line that is wrong, the one Clap gives a
/// command line it cannot parse.
const WRONG_COMMAND_LINE: u8 = 2;

/// The systems Linewright works with, which `lw` joins to it.
#[async_trait(?Send)]
pub trait Systems {
    /// The store of `repo`, as the account `token` is of.
    ///
    /// # Errors
    ///
    /// When the store does not answer, or refuses the token.
    async fn store(&self, repo: &Repo, token: String) -> Result<Arc<dyn Store>>;

    /// The harness's program.
    fn harness_program(&self) -> Program;

    /// The harness that runs its sessions with `settings`.
    fn harness(&self, settings: HarnessSettings) -> Arc<dyn Harness>;

    /// The router of the web UI.
    fn ui(&self) -> UiRouter;
}

/// The Linewright that runs.
#[derive(Clone, Copy)]
struct Lw<'a> {
    /// The commit it is built from, abbreviated, or `unknown`.
    commit: &'static str,
    systems: &'a dyn Systems,
}

impl Lw<'_> {
    /// The store of `config`'s repository, as the account `token` is of.
    async fn connect(self, config: &Config, token: String) -> Result<Arc<dyn Store>> {
        self.systems.store(&config.required_repo()?, token).await
    }

    /// What a factory starts with, as [`Starting::new`] reads it, or for a
    /// trial's sessions when `for_a_trial`, as [`Starting::running`] does,
    /// and the harness that runs its sessions.
    async fn starting(
        self,
        config: &Config,
        for_a_trial: bool,
        token: impl FnOnce() -> Result<SessionToken>,
    ) -> Result<(Starting, Arc<dyn Harness>)> {
        let program = self.systems.harness_program();
        let starting = if for_a_trial {
            Starting::running(config, program.name, &program.variables, self.commit, token).await?
        } else {
            Starting::new(config, program.name, &program.variables, self.commit, token).await?
        };
        let settings = starting.harness(own_program()?, Arc::new(SystemClock));
        let harness = self.systems.harness(settings);
        Ok((starting, harness))
    }

    /// The harness and the factory config for a trial of `task` or its
    /// scorer, whose sessions are given `token`.
    ///
    /// # Errors
    ///
    /// As [`Starting::running`], and when Linewright's own program is not
    /// found.
    async fn on_task(
        self,
        config: &Config,
        token: SessionToken,
        task: &Task,
        winding_up: &WindingUp,
    ) -> Result<(Arc<dyn Harness>, FactoryConfig)> {
        let (starting, harness) = self.starting(config, true, || Ok(token)).await?;
        let work_dir = RepoWorkDir {
            base: config.work_dir.clone(),
            repo: task.properties().settings.repo.clone(),
        };
        let database = Database::open(&work_dir.resolve()).await?;
        let factory_config = starting.factory_config(
            config,
            database.work_dir().to_path_buf(),
            database,
            OnUsageLimit::End,
            Ends::new(),
            winding_up.clone(),
        );
        Ok((harness, factory_config))
    }

    /// Starts the factory in the work directory of `database`, or, if
    /// `None`, of the repository's database, opened once Linewright's
    /// settings pass the checks of its start.
    async fn start(
        self,
        config: &Config,
        database: Option<Database>,
        on_usage_limit: OnUsageLimit,
        ends: Ends,
        winding_up: &WindingUp,
    ) -> Result<Factory> {
        let (starting, harness) = self.starting(config, false, session_token).await?;
        let database = match database {
            Some(database) => database,
            None => Database::open(&config.repo_work_dir()?.resolve()).await?,
        };
        let github = self.connect(config, token()?).await?;
        let factory_config = starting.factory_config(
            config,
            database.work_dir().to_path_buf(),
            database,
            on_usage_limit,
            ends,
            winding_up.clone(),
        );
        Factory::start(github, harness, Arc::new(SystemClock), factory_config).await
    }
}

/// Runs `lw` as its command line says, built from the commit `commit`, over
/// `systems`, and gives its exit status.
pub fn main(commit: &'static str, systems: &dyn Systems) -> ExitCode {
    let lw = Lw { commit, systems };
    let matches = Cli::command()
        .version(version_and_commit(commit))
        .get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let command = match cli.invocation {
        Invocation::Contain { program } => {
            let (name, args) = program.split_first().expect("clap requires the program");
            return contain::main(name, args);
        }
        Invocation::Command(command) => command,
    };
    let config = match resolve(&cli.settings, &systems.harness_program().variables) {
        Ok(config) => config,
        Err(status) => return ExitCode::from(status),
    };
    let log = Log::new(config.log_format, commit);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the runtime builds");
    let status = runtime.block_on(run(lw, command, config, &log));
    drop(runtime);
    log.exit(status);
    ExitCode::from(status)
}

/// The config `settings` give, or else, having said why not, the exit status.
fn resolve(settings: &Settings, harness: &Variables) -> Result<Config, u8> {
    let cwd = std::env::current_dir().unwrap_or_default();
    Config::resolve(
        settings,
        &cwd,
        &|key| std::env::var(key).ok(),
        std::io::stderr().is_terminal(),
        harness,
    )
    .map_err(|e| {
        complain(&e);
        match e {
            ResolveError::CommandLine(_) => WRONG_COMMAND_LINE,
            ResolveError::Elsewhere(_) => FAILURE,
        }
    })
}

fn complain(message: impl std::fmt::Display) {
    eprintln!("{COMMAND}: {message}");
}

/// The log a command keeps when it does work itself, begun by the command as
/// it starts that work; a command that never begins it logs nothing.
struct Log {
    format: LogFormat,
    commit: &'static str,
    begun: std::sync::Once,
}

/// What the log holds without `LINEWRIGHT_LOG`: Linewright's own lines from
/// info on, and only the warnings of rmcp, which logs each request it
/// answers at info.
/// <https://docs.rs/rmcp/3.11.0/src/rmcp/service.rs.html>
const DEFAULT_LOG: &str = "info,rmcp=warn";

impl Log {
    fn new(format: LogFormat, commit: &'static str) -> Log {
        Log {
            format,
            commit,
            begun: std::sync::Once::new(),
        }
    }

    /// Begins the log with its first line, once: events before it are lost.
    fn begin(&self) {
        self.begun.call_once(|| {
            let filter = tracing_subscriber::EnvFilter::try_from_env("LINEWRIGHT_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG));
            let builder = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr);
            match self.format {
                LogFormat::Text => builder.init(),
                LogFormat::Json => builder.json().init(),
            }
            info!(version = %VERSION, commit = %self.commit, "starting");
        });
    }

    /// Says why the command failed: in the log if it began, else in one
    /// line.
    fn fail(&self, e: &anyhow::Error) {
        if self.begun.is_completed() {
            error!(error = format!("{e:#}"), "failed");
        } else {
            complain(format!("{e:#}"));
        }
    }

    /// Ends the log, if it began, with the exit `status`.
    fn exit(&self, status: u8) {
        if self.begun.is_completed() {
            info!(status, "exiting");
        }
    }
}

async fn ask<T: serde::de::DeserializeOwned>(
    config: &Config,
    resource: &str,
    what: &str,
    json: bool,
    render: fn(&T, bool) -> String,
) -> u8 {
    let repo = match config.required_repo() {
        Ok(repo) => repo,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let body = match api::get_answer(&socket_dir(), &repo, resource).await {
        Ok(body) => body,
        Err(e) => return print_answer(Err(refused(&e))),
    };
    let text = if json {
        String::from_utf8_lossy(&body).into_owned()
    } else {
        match read_as::<T>(&body, what) {
            Ok(answer) => render(&answer, colour()),
            Err(e) => return print_answer(Err(e)),
        }
    };
    print!("{text}");
    if json && !text.ends_with('\n') {
        println!();
    }
    SUCCESS
}

/// How often `drain` asks what is in flight while it waits.
const DRAIN_ASKS_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

async fn drain(config: &Config, no_wait: bool, timeout: Option<Timeout>, json: bool) -> u8 {
    let say = |lines: &[Said]| {
        for said in lines {
            if json {
                print!("{}", drain::json(said));
            } else {
                print!("{}", drain::render(said, colour()));
            }
        }
        let _ = std::io::stdout().flush();
    };
    let repo = match config.required_repo() {
        Ok(repo) => repo,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let dir = socket_dir();
    let resource = format!("/api/drain?{}", api::query_string(&Asked { timeout }));
    let taken = match api::post_answer(&dir, &repo, &resource).await {
        Ok(body) => match read_as::<Taken>(&body, "a drain taken") {
            Ok(taken) => taken,
            Err(e) => return print_answer(Err(e)),
        },
        Err(e) => return print_answer(Err(refused(&e))),
    };
    let mut waiting = Waiting::default();
    say(&[Said::Taken { was: taken.was }]);
    say(&waiting.seen(&taken.runs));
    if no_wait {
        return SUCCESS;
    }
    let mut stop = (taken.was != Was::Stopping).then(|| {
        let (dir, repo) = (dir.clone(), repo.to_string());
        tokio::spawn(async move {
            let body = api::get_answer_once(&dir, &repo, "/api/stop").await.ok()?;
            read_as::<Stop>(&body, "a stop").ok()?.by
        })
    });
    let stop_begun =
        |by: Result<Option<StopBy>, tokio::task::JoinError>| drain::stop_begun(by.ok().flatten());
    loop {
        if let Some(answering) = &mut stop {
            tokio::select! {
                () = tokio::time::sleep(DRAIN_ASKS_EVERY) => {}
                answered = answering => {
                    stop = None;
                    say(&stop_begun(answered));
                    continue;
                }
            }
        } else {
            tokio::time::sleep(DRAIN_ASKS_EVERY).await;
        }
        let answered = match api::get_answer(&dir, &repo, "/api/status").await {
            Ok(body) => match read_as::<status::Status>(&body, "a status") {
                Ok(status) => Some(status),
                Err(e) => return print_answer(Err(e)),
            },
            Err(RequestError::NotServing { .. }) => match api::is_claimed(&dir, &repo) {
                Ok(true) => continue,
                Ok(false) => None,
                Err(e) => return print_answer(Err(failed(&e))),
            },
            Err(RequestError::HungUp(_)) => continue,
            Err(e) => return print_answer(Err(refused(&e))),
        };
        let of_same_serve =
            answered.filter(|status| status.linewright.serving_since == taken.serving_since);
        match of_same_serve {
            Some(status) => {
                say(&waiting.seen(&status.run_ids()));
                if let Some(waits) = exiting(&dir, &repo, taken.serving_since).await {
                    say(&waiting.seen_exiting(&waits));
                }
            }
            None => {
                if let Some(answering) = stop.take() {
                    say(&stop_begun(answering.await));
                }
                say(&waiting.exited());
                return SUCCESS;
            }
        }
    }
}

/// What the `serve` that began serving at `serving_since` waits for before it
/// exits; `None` if it does not answer that.
async fn exiting(dir: &Path, repo: &str, serving_since: Timestamp) -> Option<Vec<Remaining>> {
    let body = api::get_answer(dir, repo, "/api/exiting").await.ok()?;
    let exiting = read_as::<Exiting>(&body, "what serve waits for before it exits").ok()?;
    (exiting.serving_since == serving_since).then_some(exiting.waits)
}

/// Shows the environment as `serve` answers, or else as it is kept, then
/// with `follow` the build in progress, if any, until it ends.
async fn show_environment(lw: Lw<'_>, config: &Config, follow: bool, json: bool) -> u8 {
    let work_dir = match config.repo_work_dir() {
        Ok(work_dir) => work_dir,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let answer = match api::get_answer(&socket_dir(), &work_dir.repo, "/api/environment").await {
        Ok(body) => read_as::<shown::EnvironmentAnswer>(&body, "an environment"),
        Err(RequestError::NotServing { .. }) => kept_environment(lw, config, &work_dir)
            .await
            .map_err(|e| failed(&e)),
        Err(e) => Err(refused(&e)),
    };
    let answer = match answer {
        Ok(answer) => answer,
        Err(e) => return print_answer(Err(e)),
    };
    let answer = &answer;
    if json {
        println!(
            "{}",
            serde_json::to_string(answer).expect("the answers serialize")
        );
    } else {
        print!("{}", shown::render(answer, colour()));
    }
    let in_progress = answer
        .environment
        .iter()
        .flat_map(|environment| &environment.builds)
        .find(|build| build.in_progress());
    match in_progress {
        Some(build) if follow => {
            let from = build.so_far.as_ref().map_or(0, |so_far| so_far.to);
            follow_build(config, build.build, from, json).await
        }
        Some(_) | None => SUCCESS,
    }
}

/// The environment as it is kept in `work_dir`.
async fn kept_environment(
    lw: Lw<'_>,
    config: &Config,
    work_dir: &RepoWorkDir,
) -> Result<shown::EnvironmentAnswer> {
    let work_dir = work_dir.resolve();
    let db = Database::open_existing(&work_dir).await?;
    let dir = environment::dir(&work_dir);
    let setup_command = match shown::setup_given(&dir, db.as_ref()).await? {
        Some(_) => false,
        None => setup_command_before_any_poll(lw, config).await?,
    };
    shown::answer(&dir, db.as_ref(), setup_command).await
}

/// Whether there is a setup command, as the operator's settings and the
/// repository's file on its default branch now give it: before any poll has
/// kept what it read.
async fn setup_command_before_any_poll(lw: Lw<'_>, config: &Config) -> Result<bool> {
    if config.work.setup(&RepoSettings::default()).is_some() {
        return Ok(true);
    }
    let github = lw.connect(config, token()?).await?;
    let branch = github
        .settings()
        .await
        .context("reading the repository's settings")?
        .default_branch;
    let repo = RepoLayer::read(&*github, &branch, &briefs(config)?)
        .await
        .context("reading Linewright's files in the repository")?;
    Ok(config.work.setup(&repo.settings).is_some())
}

/// Prints build `build`'s output past the first `from` bytes as it comes,
/// then its end: through `serve`, or else from what is kept.
async fn follow_build(config: &Config, build: BuildId, from: u64, json: bool) -> u8 {
    let work_dir = match config.repo_work_dir() {
        Ok(work_dir) => work_dir,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let print = |followed: &Followed| {
        match (followed, json) {
            (Followed::Output { text, .. }, false) => print!("{}", shown::render_output(text)),
            (Followed::Output { .. }, true) => {}
            (Followed::Ended(ended), false) => {
                print!("{}", shown::render_line(ended, "abandoned", colour()));
            }
            (Followed::Ended(ended), true) => println!(
                "{}",
                serde_json::to_string(ended).expect("the answers serialize")
            ),
        }
        let _ = std::io::stdout().flush();
    };
    let resource = format!(
        "/api/environment/follow?{}",
        api::query_string(&api::BuildQuery { build })
    );
    let mut offset = from;
    let mut hang_ups = api::HangUps::default();
    loop {
        let mut ended = false;
        let mut unread = None;
        let mut sent = false;
        let last_id = (offset > 0).then(|| api::OutputAt { build, offset }.to_string());
        let followed = api::get_events(
            &socket_dir(),
            &work_dir.repo,
            &resource,
            last_id.as_deref(),
            |event| {
                sent = true;
                let read = match event.event.as_str() {
                    api::OUTPUT_EVENT => {
                        let at = event.id.as_deref().map(str::parse::<api::OutputAt>);
                        match (serde_json::from_str::<String>(&event.data), at) {
                            (Ok(text), Some(Ok(at))) => {
                                offset = at.offset;
                                Some(Followed::Output {
                                    text,
                                    to: at.offset,
                                })
                            }
                            _ => None,
                        }
                    }
                    api::END_EVENT => {
                        ended = true;
                        serde_json::from_str(&event.data).ok().map(Followed::Ended)
                    }
                    _ => return,
                };
                match read {
                    Some(followed) => print(&followed),
                    None => unread = Some(event),
                }
            },
        )
        .await;
        if let Some(event) = unread {
            let error = anyhow!("`serve`'s event does not read as a build's: {event:?}");
            return print_answer(Err(failed(&error)));
        }
        if sent {
            hang_ups.sent();
        }
        match followed {
            Ok(()) if ended => return SUCCESS,
            Ok(()) => tokio::time::sleep(api::ASK_AGAIN_AFTER).await,
            Err(RequestError::HungUp(e)) => {
                if !hang_ups.ask_again() {
                    return print_answer(Err(failed(&e)));
                }
                tokio::time::sleep(api::ASK_AGAIN_AFTER).await;
            }
            Err(RequestError::NotServing { .. }) => {
                let dir = environment::dir(&work_dir.resolve());
                let mut following = FollowingBuild::start(&dir, build, offset);
                loop {
                    match following.next().await {
                        Ok(Some(next)) => print(&next),
                        Ok(None) => return SUCCESS,
                        Err(e) => return print_answer(Err(failed(&e))),
                    }
                }
            }
            Err(e) => return print_answer(Err(refused(&e))),
        }
    }
}

/// Asks `serve` to build the environment and, unless `no_wait`, waits for
/// the build to end; without `serve`, builds it here.
async fn rebuild_environment(
    lw: Lw<'_>,
    config: &Config,
    no_wait: bool,
    json: bool,
    winding_up: &WindingUp,
    log: &Log,
) -> Result<u8> {
    let say = |said: &RebuildSaid| {
        if json {
            println!(
                "{}",
                serde_json::to_string(said).expect("the answers serialize")
            );
        } else {
            print!("{}", said.render());
        }
        let _ = std::io::stdout().flush();
    };
    let repo = match config.required_repo() {
        Ok(repo) => repo,
        Err(e) => return Ok(print_answer(Err(failed(&e)))),
    };
    let dir = socket_dir();
    let taken = match api::post_answer(&dir, &repo, "/api/environment/rebuild").await {
        Ok(body) => match read_as::<RebuildTaken>(&body, "a rebuild taken") {
            Ok(taken) => taken,
            Err(e) => return Ok(print_answer(Err(e))),
        },
        Err(RequestError::NotServing { .. }) => {
            return rebuild_here(lw, config, say, winding_up, log).await;
        }
        Err(e) => return Ok(print_answer(Err(refused(&e)))),
    };
    let build = match &taken {
        RebuildTaken::Started { build } | RebuildTaken::Queued { build } => *build,
        RebuildTaken::NoSetupCommand => return Ok(no_setup_command()),
        RebuildTaken::Refused { reason } => {
            return Ok(print_answer(Err((1, format!("no build starts: {reason}")))));
        }
    };
    say(&RebuildSaid::Taken(taken));
    if no_wait {
        return Ok(SUCCESS);
    }
    let resource = format!("/api/environment/rebuild/{build}");
    Ok(match api::get_answer(&dir, &repo, &resource).await {
        Ok(body) => match read_as::<BuildEnded>(&body, "a build ended") {
            Ok(ended) => {
                let code = ended_code(&ended);
                say(&RebuildSaid::Ended(ended));
                code
            }
            Err(e) => print_answer(Err(e)),
        },
        Err(e) => print_answer(Err(refused(&e))),
    })
}

/// Builds the environment without `serve`, logging as it does the work, and
/// says how the build ended.
async fn rebuild_here(
    lw: Lw<'_>,
    config: &Config,
    say: impl Fn(&RebuildSaid),
    winding_up: &WindingUp,
    log: &Log,
) -> Result<u8> {
    log.begin();
    let mut factory = lw
        .start(config, None, OnUsageLimit::End, Ends::new(), winding_up)
        .await?;
    let ended = factory
        .rebuild_environment()
        .await?
        .context(NO_SETUP_COMMAND)?;
    let code = ended_code(&ended);
    say(&RebuildSaid::Ended(ended));
    Ok(code)
}

const NO_SETUP_COMMAND: &str =
    "there is no setup command: set `setup.command` to build the environment";

fn no_setup_command() -> u8 {
    print_answer(Err((1, NO_SETUP_COMMAND.into())))
}

fn ended_code(ended: &BuildEnded) -> u8 {
    match ended {
        BuildEnded::Succeeded { .. } => SUCCESS,
        BuildEnded::Failed { .. } | BuildEnded::NotStarted { .. } => FAILURE,
    }
}

fn read_as<T: serde::de::DeserializeOwned>(body: &[u8], what: &str) -> Result<T, (u8, String)> {
    serde_json::from_slice(body)
        .map_err(|e| (1, format!("`serve`'s answer does not read as {what}: {e}")))
}

async fn list_runs(config: &Config, query: &Query, json: bool) -> u8 {
    let resource = format!("/api/runs?{}", api::query_string(query));
    let answer = ask_or_read(config, &resource, "runs", async |runs| {
        runs::answer(runs, query, SystemClock.now()).await
    })
    .await;
    print_answer(answer.map(|answer| answer.text(json)))
}

#[derive(Clone, Copy)]
struct Printing {
    verbosity: Verbosity,
    format: Format,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Json,
}

impl Printing {
    fn transcript(self, transcript: &transcript::Transcript) -> String {
        match self.format {
            Format::Text => transcript::render(transcript, self.verbosity, colour()),
            Format::Json => transcript::json_lines(transcript, self.verbosity),
        }
    }

    fn line(self, line: &transcript::Line) {
        match self.format {
            Format::Text => print!(
                "{}",
                transcript::render_line(line, self.verbosity, colour())
            ),
            Format::Json => print!("{}", transcript::json_line(line).unwrap_or_default()),
        }
        let _ = std::io::stdout().flush();
    }
}

async fn show_logs(config: &Config, which: &Which, printing: Printing, follow: bool) -> u8 {
    let resource = format!("/api/logs?{}", api::query_string(which));
    let answer = ask_or_read(config, &resource, "a transcript", async |runs| {
        logs::answer(runs, which).await
    })
    .await;
    if follow
        && let Ok(answer) = &answer
        && let transcript = answer.value()
        && let Some(in_flight) = transcript.in_flight
    {
        let query = FollowQuery {
            which: Which::Run {
                id: transcript.run.id().clone(),
                item: which.item(),
            },
            verbosity: printing.verbosity,
        };
        let after = match in_flight {
            InFlight::Unread => None,
            InFlight::ReadTo(read) => {
                print!("{}", printing.transcript(transcript));
                Some(Resume {
                    run: transcript.run.id().clone(),
                    after: After::Read(read),
                })
            }
        };
        return follow_logs(config, query, after, printing).await;
    }
    print_answer(answer.map(|answer| printing.transcript(answer.value())))
}

async fn follow_logs(
    config: &Config,
    query: FollowQuery,
    mut after: Option<Resume>,
    printing: Printing,
) -> u8 {
    let work_dir = match config.repo_work_dir() {
        Ok(work_dir) => work_dir,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let resource = format!("/api/logs/follow?{}", api::query_string(&query));
    let mut hang_ups = api::HangUps::default();
    loop {
        let mut ended = false;
        let mut unread = None;
        let mut sent = false;
        let last_id = after.as_ref().map(Resume::to_string);
        let followed = api::get_events(
            &socket_dir(),
            &work_dir.repo,
            &resource,
            last_id.as_deref(),
            |event| {
                sent = true;
                match event.event.as_str() {
                    api::LINE_EVENT => {
                        let line = serde_json::from_str::<transcript::Line>(&event.data);
                        let id = event.id.as_deref().map(str::parse::<EventId>);
                        match (line, id) {
                            (Ok(line), Some(Ok(id))) => {
                                printing.line(&line);
                                after = Some(id.into());
                            }
                            _ => unread = Some(event),
                        }
                    }
                    api::END_EVENT => ended = true,
                    _ => {}
                }
            },
        )
        .await;
        if let Some(event) = unread {
            let error = anyhow!("`serve`'s event does not read as a transcript line: {event:?}");
            return print_answer(Err(failed(&error)));
        }
        if sent {
            hang_ups.sent();
        }
        match followed {
            Ok(()) if ended => return SUCCESS,
            Ok(()) => tokio::time::sleep(api::ASK_AGAIN_AFTER).await,
            Err(RequestError::HungUp(e)) => {
                if !hang_ups.ask_again() {
                    return print_answer(Err(failed(&e)));
                }
                tokio::time::sleep(api::ASK_AGAIN_AFTER).await;
            }
            Err(RequestError::NotServing { .. }) => {
                let after = after.map(|resume| resume.after);
                return follow_records(&work_dir, &query, after, printing).await;
            }
            Err(e) => return print_answer(Err(refused(&e))),
        }
    }
}

async fn follow_records(
    work_dir: &RepoWorkDir,
    query: &FollowQuery,
    after: Option<After>,
    printing: Printing,
) -> u8 {
    let runs = match existing_runs(&work_dir.resolve()).await {
        Ok(runs) => runs,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let started = Following::start(runs.as_ref(), &query.which, query.verbosity, after).await;
    let mut following = match started {
        Ok(following) => following,
        Err(e) => return print_answer(Err(unanswered(e.into()))),
    };
    loop {
        match following.next().await {
            Ok(Some(lines)) => {
                for followed in &lines {
                    printing.line(&followed.line);
                }
            }
            Ok(None) => return SUCCESS,
            Err(e) => return print_answer(Err(failed(&e))),
        }
    }
}

fn print_answer(answer: Result<String, (u8, String)>) -> u8 {
    match answer {
        Ok(text) => {
            print!("{text}");
            SUCCESS
        }
        Err((code, message)) => {
            complain(message);
            code
        }
    }
}

async fn ask_or_read<T: serde::de::DeserializeOwned, E: Into<Unanswered>>(
    config: &Config,
    resource: &str,
    what: &str,
    read: impl AsyncFnOnce(Option<&Runs>) -> Result<T, E>,
) -> Result<Answer<T>, (u8, String)> {
    let work_dir = config.repo_work_dir().map_err(|e| failed(&e))?;
    match api::get_answer(&socket_dir(), &work_dir.repo, resource).await {
        Ok(body) => {
            let value = read_as(&body, what)?;
            Ok(Answer::Served { body, value })
        }
        Err(RequestError::NotServing { .. }) => {
            let runs = existing_runs(&work_dir.resolve())
                .await
                .map_err(|e| failed(&e))?;
            match read(runs.as_ref()).await.map_err(Into::into) {
                Ok(value) => Ok(Answer::Read(value)),
                Err(e) => Err(unanswered(e)),
            }
        }
        Err(e) => Err(refused(&e)),
    }
}

fn failed(e: &anyhow::Error) -> (u8, String) {
    (FAILURE, format!("{e:#}"))
}

fn unanswered(e: Unanswered) -> (u8, String) {
    match e {
        Unanswered::NoSuch { error, .. } => refused(&RequestError::NotFound(error)),
        Unanswered::Failed(e) => failed(&e),
    }
}

/// An error's whole chain, as [`failed`] gives it.
fn refused(e: &RequestError) -> (u8, String) {
    (e.exit_code(), format!("{e:#}"))
}

enum Answer<T> {
    Served { body: axum::body::Bytes, value: T },
    Read(T),
}

impl<T> Answer<T> {
    fn value(&self) -> &T {
        match self {
            Answer::Served { value, .. } | Answer::Read(value) => value,
        }
    }
}

impl Answer<runs::Listing> {
    fn text(&self, json: bool) -> String {
        match (self, json) {
            (Answer::Served { body, .. }, true) => {
                format!("{}\n", String::from_utf8_lossy(body))
            }
            (Answer::Read(listing), true) => {
                format!(
                    "{}\n",
                    serde_json::to_string(listing).expect("the answers serialize")
                )
            }
            (answer, false) => runs::render(answer.value(), colour()),
        }
    }
}

fn colour() -> bool {
    std::io::stdout().is_terminal()
}

fn socket_dir() -> PathBuf {
    api::socket_dir(
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        &std::env::temp_dir(),
        rustix::process::getuid().as_raw(),
    )
}

/// Runs `command`, then waits for what its work left before Linewright
/// exits; returns the exit status.
async fn run(lw: Lw<'_>, command: Command, config: Config, log: &Log) -> u8 {
    let winding_up = WindingUp::default();
    let done = work(lw, command, config, &winding_up, log).await;
    winding_up.wait().await;
    done.unwrap_or_else(|e| {
        log.fail(&e);
        FAILURE
    })
}

/// Does what `command` says, beginning `log` first if it does work itself.
async fn work(
    lw: Lw<'_>,
    command: Command,
    config: Config,
    winding_up: &WindingUp,
    log: &Log,
) -> Result<u8> {
    let done = match command {
        Command::Status { json } => {
            let resource = "/api/status";
            return Ok(ask(&config, resource, "a status", json, status::render).await);
        }
        Command::Drain {
            no_wait,
            timeout,
            json,
        } => {
            return Ok(drain(&config, no_wait, timeout, json).await);
        }
        Command::Environment {
            command: None,
            follow,
            json,
        } => {
            return Ok(show_environment(lw, &config, follow, json).await);
        }
        Command::Environment {
            command: Some(EnvironmentCommand::Rebuild { no_wait, json }),
            ..
        } => {
            return rebuild_environment(lw, &config, no_wait, json, winding_up, log).await;
        }
        Command::Explain { number, json } => {
            let resource = format!("/api/items/{number}");
            let what = "an explanation";
            return Ok(ask(&config, &resource, what, json, explain::render_answer).await);
        }
        Command::Runs {
            item,
            stage,
            since,
            limit,
            json,
        } => {
            let query = Query {
                item: item.map(Number),
                stage,
                since: since.map(|since: Since| since.at(SystemClock.now())),
                limit,
            };
            return Ok(list_runs(&config, &query, json).await);
        }
        Command::Logs {
            which,
            verbose,
            json,
            follow,
        } => {
            let printing = Printing {
                verbosity: Verbosity::of(verbose),
                format: if json { Format::Json } else { Format::Text },
            };
            return Ok(show_logs(&config, &which, printing, follow).await);
        }
        Command::Export { id, dir } => {
            return Ok(export(&config, &id, &dir).await);
        }
        Command::Serve => {
            log.begin();
            serving::serve(
                &config,
                Arc::new(SystemClock),
                &socket_dir(),
                winding_up,
                lw.systems.ui(),
                |ends, database| {
                    lw.start(
                        &config,
                        Some(database),
                        OnUsageLimit::Wait,
                        ends,
                        winding_up,
                    )
                },
            )
            .await
        }
        Command::Once => {
            log.begin();
            let mut factory = lw
                .start(&config, None, OnUsageLimit::End, Ends::new(), winding_up)
                .await?;
            let prepared = factory.prepare_environment().await;
            if let Err(e) = &prepared {
                error!(
                    error = format!("{e:#}"),
                    "no run starts: there is no environment"
                );
            }
            factory.cycle().await?;
            factory.settle().await;
            prepared
        }
        Command::Capture {
            id,
            name,
            benchmark,
            criteria,
            json,
        } => return Ok(capture(&config, id, benchmark, name, &criteria, json).await),
        Command::Benchmarks { json } => return Ok(list_benchmarks(&config, json).await),
        Command::Benchmark {
            command: BenchmarkCommand::Show { benchmark, json },
        } => return Ok(show_benchmark(&config, benchmark, json).await),
        Command::Benchmark {
            command: BenchmarkCommand::Add { name, stage, json },
        } => return Ok(make_benchmark(&config, name, stage, json).await),
        Command::Benchmark {
            command:
                BenchmarkCommand::Rename {
                    benchmark,
                    name,
                    json,
                },
        } => return Ok(rename_benchmark(&config, benchmark, name, json).await),
        Command::Benchmark {
            command:
                BenchmarkCommand::EditTask {
                    benchmark,
                    task,
                    name,
                    criteria,
                    json,
                },
        } => {
            return Ok(edit_task(&config, benchmark, task, name, criteria.as_deref(), json).await);
        }
        Command::Benchmark {
            command:
                BenchmarkCommand::RemoveTask {
                    benchmark,
                    task,
                    json,
                },
        } => return Ok(remove_task(&config, benchmark, task, json).await),
        Command::BenchmarkRun {
            command: BenchmarkRunCommand::Show { id, json },
        } => return Ok(show_benchmark_run(&config, id, json).await),
        Command::BenchmarkRun {
            command:
                BenchmarkRunCommand::Launch {
                    benchmark,
                    configurations,
                    repetitions,
                    json,
                },
        } => {
            return Ok(
                launch_benchmark_run(&config, benchmark, configurations, repetitions, json).await,
            );
        }
        Command::BenchmarkRun {
            command:
                BenchmarkRunCommand::RenameConfiguration {
                    id,
                    configuration,
                    name,
                    json,
                },
        } => return Ok(rename_configuration(&config, id, configuration, name, json).await),
        Command::BenchmarkRun {
            command: BenchmarkRunCommand::Cancel { id, json },
        } => return Ok(cancel_benchmark_run(&config, id, json).await),
        Command::Trial {
            task,
            out,
            configuration,
        } => {
            return Ok(trial(
                lw,
                &config,
                &task,
                &out,
                configuration.as_deref(),
                winding_up,
                log,
            )
            .await);
        }
        Command::Score { task, dir } => {
            return Ok(score(lw, &config, &task, &dir, winding_up, log).await);
        }
        Command::Automation {
            command: AutomationCommand::Run { name, json },
        } => return Ok(run_by_hand(&config, &name, json).await),
    };
    done.map(|()| SUCCESS)
}

/// Asks `serve` to run the automation `name`, and says the run's ID or
/// what the run waits for.
async fn run_by_hand(config: &Config, name: &AutomationName, json: bool) -> u8 {
    let repo = match config.required_repo() {
        Ok(repo) => repo,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let name = name.to_string();
    let encoded = percent_encoding::utf8_percent_encode(&name, percent_encoding::NON_ALPHANUMERIC);
    let resource = format!("/api/automations/{encoded}/run");
    let body = match api::post_answer(&socket_dir(), &repo, &resource).await {
        Ok(body) => body,
        Err(e) => return print_answer(Err(refused(&e))),
    };
    print_answer(
        read_as::<HandTaken>(&body, "a request by hand taken").map(|taken| {
            if json {
                format!("{}\n", String::from_utf8_lossy(&body))
            } else {
                taken.render()
            }
        }),
    )
}

/// Writes run `id` into `dir`, with or without `serve`.
async fn export(config: &Config, id: &RunId, dir: &Path) -> u8 {
    let work_dir = match config.repo_work_dir() {
        Ok(work_dir) => work_dir.resolve(),
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let runs = match existing_runs(&work_dir).await {
        Ok(runs) => runs,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    print_answer(
        export::run(runs.as_ref(), id, dir)
            .await
            .map(|dir| format!("Exported {id} into {}\n", dir.display()))
            .map_err(|e| match e {
                ExportError::NoSuchRun(_) => refused(&RequestError::NotFound(e.to_string())),
                ExportError::Refused(_) | ExportError::Failed(_) => failed(&anyhow::Error::from(e)),
            }),
    )
}

/// The benchmarks of the work directory's database, with or without `serve`:
/// none when there is no database.
async fn existing_benchmarks(config: &Config) -> Result<Option<Benchmarks>> {
    let work_dir = config.repo_work_dir()?.resolve();
    Ok(Database::open_existing(&work_dir)
        .await?
        .map(Benchmarks::new))
}

fn no_such_benchmark(id: BenchmarkId) -> (u8, String) {
    refused(&RequestError::NotFound(api::no_such_benchmark(&id)))
}

/// `answer` as JSON, or as `render` gives it as text.
fn answer_text<T: serde::Serialize>(
    answer: &T,
    json: bool,
    render: impl Fn(&T) -> String,
) -> String {
    if json {
        format!(
            "{}\n",
            serde_json::to_string(answer).expect("the answers serialize")
        )
    } else {
        render(answer)
    }
}

async fn list_benchmarks(config: &Config, json: bool) -> u8 {
    let listed = async {
        match existing_benchmarks(config).await? {
            Some(benchmarks) => benchmarks.list().await,
            None => Ok(vec![]),
        }
    };
    print_answer(
        listed
            .await
            .map(|benchmarks| {
                answer_text(
                    &BenchmarksAnswer { benchmarks },
                    json,
                    benchmarks::render_benchmarks,
                )
            })
            .map_err(|e| failed(&e)),
    )
}

async fn show_benchmark(config: &Config, id: BenchmarkId, json: bool) -> u8 {
    let shown = async {
        match existing_benchmarks(config).await? {
            Some(benchmarks) => benchmarks.show(id).await,
            None => Ok(None),
        }
    };
    let shown = match shown.await {
        Ok(Some(shown)) => shown,
        Ok(None) => return print_answer(Err(no_such_benchmark(id))),
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let mut states = Vec::with_capacity(shown.benchmark_runs.len());
    for record in &shown.benchmark_runs {
        match benchmark_run_shown(config, record).await {
            Ok(state) => states.push(state),
            Err(refused) => return print_answer(Err(refused)),
        }
    }
    let answer = BenchmarkAnswer::new(&shown, states);
    print_answer(Ok(answer_text(&answer, json, benchmarks::render_benchmark)))
}

/// How the benchmark run `record` shows: an open one as `serve` shows it, and
/// as waiting for `serve` when it does not run.
async fn benchmark_run_shown(
    config: &Config,
    record: &BenchmarkRunRecord,
) -> Result<Shown, (u8, String)> {
    if record.state != BenchmarkRunState::Open {
        return Ok(Shown::without_serve(record.state));
    }
    let resource = format!("/api/benchmark-runs/{}", record.id);
    let answer = ask_or_read(config, &resource, "a benchmark run's state", async |_| {
        Ok::<_, Unanswered>(Shown::without_serve(record.state))
    })
    .await?;
    Ok(answer.value().clone())
}

/// The benchmark runs of the work directory's database, with or without
/// `serve`: none when there is no database.
async fn existing_benchmark_runs(config: &Config) -> Result<Option<BenchmarkRuns>> {
    let work_dir = config.repo_work_dir()?.resolve();
    Ok(Database::open_existing(&work_dir)
        .await?
        .map(BenchmarkRuns::new))
}

fn no_such_benchmark_run(id: BenchmarkRunId) -> (u8, String) {
    refused(&RequestError::NotFound(api::no_such_benchmark_run(&id)))
}

/// Shows the benchmark run `id`: an open one as `serve` shows it, or as
/// waiting for `serve` when it does not run.
async fn show_benchmark_run(config: &Config, id: BenchmarkRunId, json: bool) -> u8 {
    let shown = async {
        match existing_benchmark_runs(config).await? {
            Some(benchmark_runs) => benchmark_runs.show(id).await,
            None => Ok(None),
        }
    };
    let record = match shown.await {
        Ok(Some(record)) => record,
        Ok(None) => return print_answer(Err(no_such_benchmark_run(id))),
        Err(e) => return print_answer(Err(failed(&e))),
    };
    print_answer(benchmark_run_shown(config, &record).await.map(|state| {
        answer_text(
            &BenchmarkRunAnswer::new(&record, state),
            json,
            benchmarks::render_benchmark_run,
        )
    }))
}

/// Asks `serve` to change something: `method` on `resource`, with the JSON
/// `body` if there is one. Prints its answer, as it is with `json` and
/// otherwise as `render` gives it of what reads as `what`.
async fn act<T: serde::de::DeserializeOwned>(
    config: &Config,
    method: hyper::Method,
    resource: &str,
    body: Option<&impl serde::Serialize>,
    what: &str,
    json: bool,
    render: impl Fn(&T) -> String,
) -> u8 {
    let repo = match config.required_repo() {
        Ok(repo) => repo,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let asked = api::act_answer(&socket_dir(), &repo.to_string(), method, resource, body).await;
    let answered = match asked {
        Ok(answered) => answered,
        Err(e) => return print_answer(Err(refused(&e))),
    };
    if json {
        return print_answer(Ok(format!("{}\n", String::from_utf8_lossy(&answered))));
    }
    print_answer(read_as::<T>(&answered, what).map(|read| render(&read)))
}

const NO_BODY: Option<&()> = None;

fn render_captured(captured: &api::Captured) -> String {
    format!(
        "Captured {} as the task {} ({}) in the benchmark {} ({}), in {}\n",
        captured.run,
        captured.task,
        captured.name,
        captured.benchmark.id,
        captured.benchmark.name,
        captured.dir.display(),
    )
}

fn render_made(benchmark: &Benchmark) -> String {
    format!(
        "Made the benchmark {} ({}), of {}\n",
        benchmark.id, benchmark.name, benchmark.stage
    )
}

fn render_renamed(renamed: &api::Renamed) -> String {
    format!("Renamed the benchmark {} to {}\n", renamed.id, renamed.name)
}

fn render_edited(edited: &api::Edited) -> String {
    format!(
        "Changed the task {} of the benchmark {}\n",
        edited.task, edited.benchmark
    )
}

fn render_removed(removed: &api::Removed) -> String {
    format!(
        "Removed the task {} ({}) from the benchmark {}\n",
        removed.task, removed.name, removed.benchmark
    )
}

fn render_launched(launched: &api::LaunchedAnswer) -> String {
    format!(
        "Launched benchmark run {} on the benchmark {} ({}): {} trials\n",
        launched.id, launched.benchmark, launched.benchmark_name, launched.trials
    )
}

fn render_renamed_configuration(renamed: &api::RenamedConfiguration) -> String {
    format!(
        "Renamed the configuration {} of benchmark run {} to {}\n",
        renamed.configuration, renamed.id, renamed.name
    )
}

fn render_cancelled(cancelled: &api::CancelledAnswer) -> String {
    format!("Cancelled benchmark run {}\n", cancelled.id)
}

/// Asks `serve` to capture run `run` as the task `name` of the benchmark
/// `benchmark`, with the criteria in the file `criteria`, refusing criteria
/// that do not read or hold only white space before asking.
async fn capture(
    config: &Config,
    run: RunId,
    benchmark: BenchmarkId,
    name: Name,
    criteria: &Path,
    json: bool,
) -> u8 {
    let criteria = match read_criteria(criteria) {
        Ok(criteria) => criteria,
        Err(refused) => return print_answer(Err(refused)),
    };
    let asked = api::CaptureAsked {
        run,
        benchmark,
        name,
        criteria,
    };
    act(
        config,
        hyper::Method::POST,
        "/api/tasks",
        Some(&asked),
        "a captured task",
        json,
        render_captured,
    )
    .await
}

async fn make_benchmark(config: &Config, name: Name, stage: TaskStage, json: bool) -> u8 {
    let asked = api::BenchmarkAsked { name, stage };
    act(
        config,
        hyper::Method::POST,
        "/api/benchmarks",
        Some(&asked),
        "a benchmark",
        json,
        render_made,
    )
    .await
}

async fn rename_benchmark(config: &Config, id: BenchmarkId, name: Name, json: bool) -> u8 {
    let resource = format!("/api/benchmarks/{id}");
    let asked = api::RenameAsked { name };
    act(
        config,
        hyper::Method::PATCH,
        &resource,
        Some(&asked),
        "a renamed benchmark",
        json,
        render_renamed,
    )
    .await
}

/// Asks `serve` to change the task `task` of the benchmark `benchmark` to the
/// name `name` and the criteria in the file `criteria`, failing on a file
/// that does not read and refusing criteria that hold only white space
/// before asking.
async fn edit_task(
    config: &Config,
    benchmark: BenchmarkId,
    task: TaskId,
    name: Option<Name>,
    criteria: Option<&Path>,
    json: bool,
) -> u8 {
    let criteria = match criteria.map(read_criteria).transpose() {
        Ok(criteria) => criteria,
        Err(refused) => return print_answer(Err(refused)),
    };
    let resource = format!("/api/benchmarks/{benchmark}/tasks/{task}");
    let asked = api::TaskChangeAsked { name, criteria };
    act(
        config,
        hyper::Method::PATCH,
        &resource,
        Some(&asked),
        "a changed task",
        json,
        render_edited,
    )
    .await
}

/// The criteria in the file `file`, failing on a file that does not read
/// and refusing one that holds only white space.
fn read_criteria(file: &Path) -> Result<Criteria, (u8, String)> {
    let criteria = std::fs::read_to_string(file)
        .map_err(|e| failed(&anyhow::Error::from(e).context("reading the criteria")))?;
    Criteria::parse(&criteria).ok_or_else(|| (FAILURE, CRITERIA_REQUIRED.to_string()))
}

async fn remove_task(config: &Config, benchmark: BenchmarkId, task: TaskId, json: bool) -> u8 {
    let resource = format!("/api/benchmarks/{benchmark}/tasks/{task}");
    act(
        config,
        hyper::Method::DELETE,
        &resource,
        NO_BODY,
        "a removed task",
        json,
        render_removed,
    )
    .await
}

/// Asks `serve` to launch a benchmark run on the benchmark `benchmark` under
/// `as-is` and the configurations in the directories `configurations`, a
/// relative one taken from the current directory.
async fn launch_benchmark_run(
    config: &Config,
    benchmark: BenchmarkId,
    configurations: Vec<(Name, PathBuf)>,
    repetitions: Repetitions,
    json: bool,
) -> u8 {
    let mut asked = Vec::with_capacity(configurations.len());
    for (name, dir) in configurations {
        match api::AbsolutePath::of(&dir) {
            Ok(dir) => asked.push(api::ConfigurationAsked { name, dir }),
            Err(e) => {
                let e = anyhow::Error::from(e).context(format!("reading {}", dir.display()));
                return print_answer(Err(failed(&e)));
            }
        }
    }
    let asked = api::LaunchAsked {
        benchmark,
        configurations: asked,
        repetitions,
    };
    act(
        config,
        hyper::Method::POST,
        "/api/benchmark-runs",
        Some(&asked),
        "a launched benchmark run",
        json,
        render_launched,
    )
    .await
}

async fn rename_configuration(
    config: &Config,
    id: BenchmarkRunId,
    configuration: ConfigurationId,
    name: Name,
    json: bool,
) -> u8 {
    let resource = format!("/api/benchmark-runs/{id}/configurations/{configuration}");
    let asked = api::RenameAsked { name };
    act(
        config,
        hyper::Method::PATCH,
        &resource,
        Some(&asked),
        "a renamed configuration",
        json,
        render_renamed_configuration,
    )
    .await
}

async fn cancel_benchmark_run(config: &Config, id: BenchmarkRunId, json: bool) -> u8 {
    let resource = format!("/api/benchmark-runs/{id}/cancel");
    act(
        config,
        hyper::Method::POST,
        &resource,
        NO_BODY,
        "a cancelled benchmark run",
        json,
        render_cancelled,
    )
    .await
}

/// Runs the task in the directory `task` into the output `out`, with
/// the configuration in the directory `configuration` if one is given: 0
/// once its run ended, whatever its outcome, and 1 when it did not run or
/// end. A configuration with an error, or no model, is refused before the
/// output is made.
async fn trial(
    lw: Lw<'_>,
    config: &Config,
    task: &Path,
    out: &Path,
    configuration: Option<&Path>,
    winding_up: &WindingUp,
    log: &Log,
) -> u8 {
    let token = match session_token() {
        Ok(token) => token,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    if let Err(e) = config.work.clone().for_sessions() {
        return print_answer(Err(failed(&e.into())));
    }
    let configuration = match configuration.map(Configuration::read).transpose() {
        Ok(configuration) => configuration,
        Err(e) => return print_answer(Err(failed(&e.into()))),
    };
    let out = match Output::make(out) {
        Ok(out) => out,
        Err(e) => return print_answer(Err(failed(&e.into()))),
    };
    let running = trial::unless_signalled(
        Box::pin(async {
            let task = match Task::open(task).await {
                Ok(task) => task,
                Err(e) => {
                    out.finished();
                    return print_answer(Err((FAILURE, format!("the task cannot be run: {e}"))));
                }
            };
            log.begin();
            let ran = async {
                let (harness, factory_config) =
                    lw.on_task(config, token, &task, winding_up).await?;
                Box::pin(trial::run(
                    &task,
                    &out,
                    harness,
                    factory_config,
                    configuration.as_ref(),
                    &Under::command(),
                ))
                .await
            }
            .await;
            out.finished();
            match ran {
                Ok(_) => SUCCESS,
                Err(e) => {
                    log.fail(&e.into());
                    FAILURE
                }
            }
        }),
        |signal| out.signalled(signal),
    );
    running
        .await
        .unwrap_or_else(|e| print_answer(Err(failed(&e))))
}

/// Scores `dir`, the output of a trial of the task in the directory
/// `task`, and keeps the score in it: 0 once the scorer made one,
/// whatever it is, and 1 when it made none or the task or the output is
/// refused. No model is refused before the scorer's record is made. A
/// signal ends it at once with the signal's status, keeping nothing of the
/// scorer.
async fn score(
    lw: Lw<'_>,
    config: &Config,
    task: &Path,
    dir: &Path,
    winding_up: &WindingUp,
    log: &Log,
) -> u8 {
    let token = match session_token() {
        Ok(token) => token,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    if let Err(e) = config.work.clone().for_sessions() {
        return print_answer(Err(failed(&e.into())));
    }
    let task = match Task::open(task).await {
        Ok(task) => task,
        Err(e) => return print_answer(Err((FAILURE, format!("the task cannot be scored: {e}")))),
    };
    let scoreable = match scorer::ScoreableTrial::open(dir, &task).await {
        Ok(scoreable) => scoreable,
        Err(e) => return print_answer(Err((FAILURE, format!("the output cannot be scored: {e}")))),
    };
    let record = match ScorerRecord::make(&scoreable) {
        Ok(record) => record,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let record_dir = record.dir().to_path_buf();
    log.begin();
    let scoring = trial::unless_signalled(
        Box::pin(async {
            let (harness, factory_config) = lw
                .on_task(config, token, &task, winding_up)
                .await
                .map_err(scorer::Unscored::Unexpected)?;
            scorer::run(
                &task,
                &scoreable,
                record,
                harness,
                factory_config,
                &Under::command(),
            )
            .await
        }),
        |signal| {
            scorer::discard(&record_dir);
            complain("A signal ended the scorer.");
            signal.exit_status()
        },
    );
    let score = match scoring.await {
        Ok(Ok(score)) => score,
        Ok(Err(unscored)) => return print_answer(Err((FAILURE, unscored.told()))),
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let summary = score.summary();
    print_answer(
        scorer::keep(&scoreable, score)
            .map(|()| summary)
            .map_err(|unscored| (FAILURE, unscored.told())),
    )
}

/// Linewright's token, as `gh` reads it: `GH_TOKEN`, then `GITHUB_TOKEN`, an
/// empty one being unset.
///
/// # Errors
///
/// When neither `GH_TOKEN` nor `GITHUB_TOKEN` is set.
fn token() -> Result<String> {
    TOKEN_VARIABLES
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|t| !t.is_empty()))
        .context("set GH_TOKEN or GITHUB_TOKEN to the token of Linewright's account")
}

/// The token sessions are given: the separate one if set, else Linewright's,
/// which a command that runs sessions needs.
///
/// # Errors
///
/// When Linewright's [`token`] is unset.
fn session_token() -> Result<SessionToken> {
    Ok(SessionToken::new(
        token()?,
        std::env::var(SESSION_TOKEN_VARIABLE).ok(),
    ))
}

/// The `lw` running, which wraps each session's turns.
fn own_program() -> Result<PathBuf> {
    std::env::current_exe().context("finding Linewright's own program")
}

/// The runs of the database of `work_dir`, or `None`, making nothing, when
/// there is no database.
async fn existing_runs(work_dir: &Path) -> Result<Option<Runs>> {
    Ok(Database::open_existing(work_dir).await?.map(Runs::new))
}
