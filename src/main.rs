use std::io::{IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use linewright::api::{self, EventId, RequestError, Resume, Unanswered};
use linewright::benchmarks::{
    self, BenchmarkAnswer, BenchmarkName, BenchmarkRunAnswer, BenchmarkRunId, BenchmarkRunRecord,
    BenchmarkRunState, BenchmarkRuns, Benchmarks, BenchmarksAnswer, CancelRefusal,
    ConfigurationName, LaunchRefusal, RemoveError, Repetitions, Shown,
};
use linewright::clock::{Clock, SystemClock};
use linewright::config::{
    AutomationCommand, BenchmarkCommand, BenchmarkRunCommand, Cli, Command, Config,
    EnvironmentCommand, Invocation, LogFormat, ResolveError, Settings, Since,
};
use linewright::database::Database;
use linewright::domain::{Number, Timestamp};
use linewright::drain::{self, Asked, Exiting, Said, Stop, Taken, Timeout, Waiting, Was};
use linewright::ending::{Ends, StopBy};
use linewright::environment::shown::{self, Followed, FollowingBuild};
use linewright::environment::{self, BuildEnded, BuildId, RebuildSaid, RebuildTaken};
use linewright::export::{self, ExportError};
use linewright::factory::{Factory, FactoryConfig, SessionToken};
use linewright::harness::claude::ClaudeCode;
use linewright::logs::{self, FollowQuery, Following, Which};
use linewright::repo_files::RepoLayer;
use linewright::runs::{self, Query, RepoWorkDir, RunId, Runs};
use linewright::serving::{self, OnUsageLimit, Starting, briefs};
use linewright::store::Store as _;
use linewright::store::github::{GitHub, SESSION_TOKEN_VARIABLE, TOKEN_VARIABLES};
use linewright::task::{
    self, BenchmarkChoice, CRITERIA_REQUIRED, CaptureError, Criteria, Refusal, Task, TaskName,
    TaskStage,
};
use linewright::transcript::{self, After, InFlight, Verbosity};
use linewright::trial::scorer::{self, ScorerRecord};
use linewright::trial::{self, Configuration, Output, Under};
use linewright::winding_up::{Remaining, WindingUp};
use linewright::work_settings::RepoSettings;
use linewright::{COMMAND, COMMIT, VERSION};
use linewright::{contain, explain, status};
use tracing::{error, info};

const HARNESS_PROGRAM: &str = "claude";

const SUCCESS: u8 = 0;

/// The exit code for any error but the ones with codes of their own.
const FAILURE: u8 = 1;

/// The exit code for a command line that is wrong, the one Clap gives a
/// command line it cannot parse.
const WRONG_COMMAND_LINE: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let command = match cli.invocation {
        Invocation::Contain { program } => {
            let (name, args) = program.split_first().expect("clap requires the program");
            return contain::main(name, args);
        }
        Invocation::Command(command) => command,
    };
    let config = match resolve(&cli.settings) {
        Ok(config) => config,
        Err(status) => return ExitCode::from(status),
    };
    let log = Log::new(config.log_format);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the runtime builds");
    let status = runtime.block_on(run(command, config, &log));
    drop(runtime);
    log.exit(status);
    ExitCode::from(status)
}

/// The config `settings` give, or else, having said why not, the exit status.
fn resolve(settings: &Settings) -> Result<Config, u8> {
    let cwd = std::env::current_dir().unwrap_or_default();
    Config::resolve(
        settings,
        &cwd,
        &|key| std::env::var(key).ok(),
        std::io::stderr().is_terminal(),
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
    begun: std::sync::Once,
}

impl Log {
    fn new(format: LogFormat) -> Log {
        Log {
            format,
            begun: std::sync::Once::new(),
        }
    }

    /// Begins the log with its first line, once: events before it are lost.
    fn begin(&self) {
        self.begun.call_once(|| {
            let filter = tracing_subscriber::EnvFilter::try_from_env("LINEWRIGHT_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
            let builder = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr);
            match self.format {
                LogFormat::Text => builder.init(),
                LogFormat::Json => builder.json().init(),
            }
            info!(version = %VERSION, commit = %COMMIT, "starting");
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
async fn show_environment(config: &Config, follow: bool, json: bool) -> u8 {
    let work_dir = match config.repo_work_dir() {
        Ok(work_dir) => work_dir,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let answer = match api::get_answer(&socket_dir(), &work_dir.repo, "/api/environment").await {
        Ok(body) => read_as::<shown::EnvironmentAnswer>(&body, "an environment"),
        Err(RequestError::NotServing { .. }) => kept_environment(config, &work_dir)
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
    config: &Config,
    work_dir: &RepoWorkDir,
) -> Result<shown::EnvironmentAnswer> {
    let work_dir = work_dir.resolve();
    let db = Database::open_existing(&work_dir).await?;
    let dir = environment::dir(&work_dir);
    let setup_command = match shown::setup_given(&dir, db.as_ref()).await? {
        Some(_) => false,
        None => setup_command_before_any_poll(config).await?,
    };
    shown::answer(&dir, db.as_ref(), setup_command).await
}

/// Whether there is a setup command, as the operator's settings and the
/// repository's file on its default branch now give it: before any poll has
/// kept what it read.
async fn setup_command_before_any_poll(config: &Config) -> Result<bool> {
    if config.work.setup(&RepoSettings::default()).is_some() {
        return Ok(true);
    }
    let github = connect(config, token()?).await?;
    let branch = github
        .settings()
        .await
        .context("reading the repository's settings")?
        .default_branch;
    let repo = RepoLayer::read(&github, &branch, &briefs(config)?)
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
            return rebuild_here(config, say, winding_up, log).await;
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
    config: &Config,
    say: impl Fn(&RebuildSaid),
    winding_up: &WindingUp,
    log: &Log,
) -> Result<u8> {
    log.begin();
    let mut factory = start(config, None, OnUsageLimit::End, Ends::new(), winding_up).await?;
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
async fn run(command: Command, config: Config, log: &Log) -> u8 {
    let winding_up = WindingUp::default();
    let done = work(command, config, &winding_up, log).await;
    winding_up.wait().await;
    done.unwrap_or_else(|e| {
        log.fail(&e);
        FAILURE
    })
}

/// Does what `command` says, beginning `log` first if it does work itself.
async fn work(command: Command, config: Config, winding_up: &WindingUp, log: &Log) -> Result<u8> {
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
            return Ok(show_environment(&config, follow, json).await);
        }
        Command::Environment {
            command: Some(EnvironmentCommand::Rebuild { no_wait, json }),
            ..
        } => {
            return rebuild_environment(&config, no_wait, json, winding_up, log).await;
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
                |ends, database| {
                    start(
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
            let mut factory =
                start(&config, None, OnUsageLimit::End, Ends::new(), winding_up).await?;
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
        } => {
            let capture = Capture {
                id,
                name,
                benchmark,
                criteria,
            };
            return Ok(capture.run(&config, json).await);
        }
        Command::Benchmarks { json } => return Ok(list_benchmarks(&config, json).await),
        Command::Benchmark {
            command: BenchmarkCommand::Show { name, json },
        } => return Ok(show_benchmark(&config, &name, json).await),
        Command::Benchmark {
            command: BenchmarkCommand::Add { name, stage, json },
        } => return Ok(make_benchmark(&config, &name, stage, json).await),
        Command::Benchmark {
            command: BenchmarkCommand::RemoveTask { name, task, json },
        } => return Ok(remove_task(&config, &name, &task, json).await),
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
            return Ok(launch_benchmark_run(
                &config,
                &benchmark,
                &configurations,
                repetitions,
                json,
            )
            .await);
        }
        Command::BenchmarkRun {
            command: BenchmarkRunCommand::Cancel { id, json },
        } => return Ok(cancel_benchmark_run(&config, id, json).await),
        Command::Trial {
            task,
            out,
            configuration,
        } => {
            return Ok(trial(
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
            return Ok(score(&config, &task, &dir, winding_up, log).await);
        }
        Command::Automation {
            command: AutomationCommand::Run { name },
        } => {
            log.begin();
            start(&config, None, OnUsageLimit::End, Ends::new(), winding_up)
                .await?
                .run_automation(&name)
                .await
        }
    };
    done.map(|()| SUCCESS)
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

/// The benchmarks of the work directory's database, made when there is none.
async fn made_benchmarks(config: &Config) -> Result<Benchmarks> {
    let work_dir = config.repo_work_dir()?.resolve();
    Ok(Benchmarks::new(Database::open(&work_dir).await?))
}

fn no_such_benchmark(name: &BenchmarkName) -> (u8, String) {
    refused(&RequestError::NotFound(format!(
        "there is no benchmark {name}"
    )))
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

async fn show_benchmark(config: &Config, name: &BenchmarkName, json: bool) -> u8 {
    let shown = async {
        match existing_benchmarks(config).await? {
            Some(benchmarks) => benchmarks.show(name).await,
            None => Ok(None),
        }
    };
    let shown = match shown.await {
        Ok(Some(shown)) => shown,
        Ok(None) => return print_answer(Err(no_such_benchmark(name))),
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

/// Tells `serve`, if it runs, that a benchmark run changed, so that it acts
/// on it before its next poll.
async fn nudge_serve(config: &Config) {
    let Ok(work_dir) = config.repo_work_dir() else {
        return;
    };
    match api::post_answer(&socket_dir(), &work_dir.repo, "/api/benchmark-runs/changed").await {
        Ok(_) | Err(RequestError::NotServing { .. }) => {}
        Err(e) => complain(format!(
            "`serve` was not told of the change, and takes it up at its next poll: {e:#}"
        )),
    }
}

async fn make_benchmark(config: &Config, name: &BenchmarkName, stage: TaskStage, json: bool) -> u8 {
    let benchmarks = match made_benchmarks(config).await {
        Ok(benchmarks) => benchmarks,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    print_answer(
        benchmarks
            .make(name, stage)
            .await
            .map(|benchmark| {
                answer_text(&benchmark, json, |benchmark| {
                    format!(
                        "Made the benchmark {}, of {}\n",
                        benchmark.name, benchmark.stage
                    )
                })
            })
            .map_err(|e| (FAILURE, format!("{e:#}"))),
    )
}

/// What `benchmark remove-task` answers.
#[derive(serde::Serialize)]
struct Removed<'a> {
    benchmark: &'a BenchmarkName,
    task: &'a TaskName,
}

async fn remove_task(config: &Config, name: &BenchmarkName, task: &TaskName, json: bool) -> u8 {
    let benchmarks = match existing_benchmarks(config).await {
        Ok(Some(benchmarks)) => benchmarks,
        Ok(None) => return print_answer(Err(no_such_benchmark(name))),
        Err(e) => return print_answer(Err(failed(&e))),
    };
    print_answer(
        benchmarks
            .remove_task(name, task)
            .await
            .map(|()| {
                answer_text(
                    &Removed {
                        benchmark: name,
                        task,
                    },
                    json,
                    |removed| {
                        format!(
                            "Removed the task {} from the benchmark {}\n",
                            removed.task, removed.benchmark
                        )
                    },
                )
            })
            .map_err(|e| match e {
                RemoveError::NoSuchBenchmark(_) | RemoveError::NoSuchTask { .. } => {
                    refused(&RequestError::NotFound(e.to_string()))
                }
                RemoveError::Failed(e) => failed(&e),
            }),
    )
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
    refused(&RequestError::NotFound(format!(
        "there is no benchmark run {id}"
    )))
}

/// What `benchmark-run launch` answers.
#[derive(serde::Serialize)]
struct LaunchedAnswer<'a> {
    id: BenchmarkRunId,
    benchmark: &'a BenchmarkName,
    trials: usize,
}

/// Launches a benchmark run on the benchmark `benchmark` under `as-is` and the
/// configurations in the directories `configurations`, refusing a benchmark
/// that does not exist and then a configuration with an error before anything
/// is written.
async fn launch_benchmark_run(
    config: &Config,
    benchmark: &BenchmarkName,
    configurations: &[(ConfigurationName, PathBuf)],
    repetitions: Repetitions,
    json: bool,
) -> u8 {
    let database = async {
        let work_dir = config.repo_work_dir()?.resolve();
        let Some(db) = Database::open_existing(&work_dir).await? else {
            return Ok(None);
        };
        let found = Benchmarks::new(db.clone()).find(benchmark).await?;
        anyhow::Ok(found.map(|_| db))
    };
    let db = match database.await {
        Ok(Some(db)) => db,
        Ok(None) => return print_answer(Err(no_such_benchmark(benchmark))),
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let mut read_configurations = Vec::with_capacity(configurations.len());
    for (name, dir) in configurations {
        match Configuration::read(dir) {
            Ok(configuration) => read_configurations.push((name.clone(), configuration)),
            Err(e) => return print_answer(Err((FAILURE, e.to_string()))),
        }
    }
    let launched = BenchmarkRuns::new(db)
        .launch(
            benchmark,
            read_configurations,
            repetitions,
            SystemClock.now(),
        )
        .await;
    if launched.is_ok() {
        nudge_serve(config).await;
    }
    print_answer(
        launched
            .map(|launched| {
                let answer = LaunchedAnswer {
                    id: launched.id,
                    benchmark,
                    trials: launched.trials,
                };
                answer_text(&answer, json, |answer| {
                    format!(
                        "Launched benchmark run {} on the benchmark {}: {} trials\n",
                        answer.id, answer.benchmark, answer.trials
                    )
                })
            })
            .map_err(|e| match e {
                LaunchRefusal::NoSuchBenchmark(_) => {
                    refused(&RequestError::NotFound(e.to_string()))
                }
                LaunchRefusal::NoTasks(_) | LaunchRefusal::DuplicateConfiguration(_) => {
                    (FAILURE, e.to_string())
                }
                LaunchRefusal::Failed(e) => failed(&e),
            }),
    )
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

/// What `benchmark-run cancel` answers.
#[derive(serde::Serialize)]
struct Cancelled {
    id: BenchmarkRunId,
}

async fn cancel_benchmark_run(config: &Config, id: BenchmarkRunId, json: bool) -> u8 {
    let benchmark_runs = match existing_benchmark_runs(config).await {
        Ok(Some(benchmark_runs)) => benchmark_runs,
        Ok(None) => return print_answer(Err(no_such_benchmark_run(id))),
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let cancelling = benchmark_runs.cancel(id, SystemClock.now()).await;
    if cancelling.is_ok() {
        nudge_serve(config).await;
    }
    print_answer(
        cancelling
            .map(|()| {
                answer_text(&Cancelled { id }, json, |cancelled| {
                    format!("Cancelled benchmark run {}\n", cancelled.id)
                })
            })
            .map_err(|e| match e {
                CancelRefusal::NoSuchBenchmarkRun(_) => no_such_benchmark_run(id),
                CancelRefusal::NotOpen { .. } => (FAILURE, e.to_string()),
                CancelRefusal::Failed(e) => failed(&e),
            }),
    )
}

/// What `capture` is asked to capture.
struct Capture {
    id: RunId,
    name: TaskName,
    benchmark: BenchmarkName,
    /// The file that holds the criteria.
    criteria: PathBuf,
}

/// What `capture` answers.
#[derive(serde::Serialize)]
struct Captured<'a> {
    run: &'a RunId,
    task: &'a TaskName,
    benchmark: &'a BenchmarkName,
    dir: &'a Path,
}

impl Capture {
    /// Captures run `id` as the task `name` of the benchmark `benchmark`, with
    /// the criteria in the file `criteria`, refusing before connecting to
    /// GitHub what it can.
    async fn run(&self, config: &Config, json: bool) -> u8 {
        let Capture {
            id,
            name,
            benchmark,
            criteria,
        } = self;
        let refusal = |e: CaptureError| match e.refusal() {
            Refusal::NoSuch => refused(&RequestError::NotFound(e.why())),
            Refusal::Refused | Refusal::Failed => (FAILURE, e.why()),
        };
        let work_dir = match config.repo_work_dir() {
            Ok(work_dir) => work_dir.resolve(),
            Err(e) => return print_answer(Err(failed(&e))),
        };
        let runs = match existing_runs(&work_dir).await {
            Ok(Some(runs)) => runs,
            Ok(None) => return print_answer(Err(refusal(CaptureError::NoSuchRun(id.clone())))),
            Err(e) => return print_answer(Err(failed(&e))),
        };
        let benchmarks = Benchmarks::new(runs.database().clone());
        let choice = BenchmarkChoice::Named(benchmark.clone());
        let capturable = match task::capturable(&runs, &benchmarks, id, name, &choice).await {
            Ok(capturable) => capturable,
            Err(e) => return print_answer(Err(refusal(e))),
        };
        let criteria = match std::fs::read_to_string(criteria) {
            Ok(criteria) => criteria,
            Err(e) => {
                let e = anyhow::Error::from(e).context("reading the criteria");
                return print_answer(Err(failed(&e)));
            }
        };
        let Some(criteria) = Criteria::parse(&criteria) else {
            return print_answer(Err((FAILURE, CRITERIA_REQUIRED.to_string())));
        };
        let github = match token() {
            Ok(token) => connect(config, token).await,
            Err(e) => Err(e),
        };
        let github = match github {
            Ok(github) => github,
            Err(e) => return print_answer(Err(failed(&e))),
        };
        print_answer(
            task::capture(&github, capturable, &criteria, &benchmarks)
                .await
                .map(|dir| {
                    let captured = Captured {
                        run: id,
                        task: name,
                        benchmark,
                        dir: &dir,
                    };
                    answer_text(&captured, json, |captured| {
                        format!(
                            "Captured {} as the task {} in the benchmark {}\n",
                            captured.run,
                            captured.dir.display(),
                            captured.benchmark
                        )
                    })
                })
                .map_err(refusal),
        )
    }
}

/// Runs the task in the directory `task` into the output `out`, with
/// the configuration in the directory `configuration` if one is given: 0
/// once its run ended, whatever its outcome, and 1 when it did not run or
/// end. A configuration with an error is refused before the output is
/// made.
async fn trial(
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
                let (harness, factory_config) = on_task(config, token, &task, winding_up).await?;
                Box::pin(trial::run(
                    &task,
                    &out,
                    Arc::new(harness),
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
                    log.fail(&e);
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

/// The harness and the factory config of a command that runs sessions on
/// `task`, with `token` as their token, in the work directory of the task's
/// repository.
///
/// # Errors
///
/// As [`Starting::running`], and when Linewright's own program is not
/// found.
async fn on_task(
    config: &Config,
    token: SessionToken,
    task: &Task,
    winding_up: &WindingUp,
) -> Result<(ClaudeCode, FactoryConfig)> {
    let starting = Starting::running(config, HARNESS_PROGRAM, || Ok(token)).await?;
    let harness = starting.harness(own_program()?, Arc::new(SystemClock));
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

/// Scores `dir`, the output of a trial of the task in the directory
/// `task`, and keeps the score in it: 0 once the scorer made one,
/// whatever it is, and 1 when it made none or the task or the output is
/// refused. A signal ends it at once with the signal's status, keeping
/// nothing of the scorer.
async fn score(config: &Config, task: &Path, dir: &Path, winding_up: &WindingUp, log: &Log) -> u8 {
    let token = match session_token() {
        Ok(token) => token,
        Err(e) => return print_answer(Err(failed(&e))),
    };
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
            let (harness, factory_config) = on_task(config, token, &task, winding_up)
                .await
                .map_err(scorer::Unscored::Failed)?;
            scorer::run(
                &task,
                &scoreable,
                record,
                Arc::new(harness),
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
        Ok(Err(unscored)) => return print_answer(Err((FAILURE, unscored.to_string()))),
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let summary = score.summary();
    print_answer(
        scorer::keep(&scoreable, score)
            .map(|()| summary)
            .map_err(|e| {
                let unscored = scorer::Unscored::Failed(e.context("keeping the score"));
                (FAILURE, unscored.to_string())
            }),
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

async fn connect(config: &Config, token: String) -> Result<GitHub> {
    GitHub::connect(&config.required_repo()?, token).await
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

/// Starts the factory in the work directory of `database`, or, if `None`,
/// of the repository's database, opened once Linewright's settings pass the
/// checks of its start.
async fn start(
    config: &Config,
    database: Option<Database>,
    on_usage_limit: OnUsageLimit,
    ends: Ends,
    winding_up: &WindingUp,
) -> Result<Factory> {
    let starting = Starting::new(config, HARNESS_PROGRAM, session_token).await?;
    let database = match database {
        Some(database) => database,
        None => Database::open(&config.repo_work_dir()?.resolve()).await?,
    };
    let harness = starting.harness(own_program()?, Arc::new(SystemClock));
    let github = connect(config, token()?).await?;
    let factory_config = starting.factory_config(
        config,
        database.work_dir().to_path_buf(),
        database,
        on_usage_limit,
        ends,
        winding_up.clone(),
    );
    Factory::start(
        Arc::new(github),
        Arc::new(harness),
        Arc::new(SystemClock),
        factory_config,
    )
    .await
}
