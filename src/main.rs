use std::io::{IsTerminal, Write as _};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use linewright::api::{
    self, Api, EventId, Listeners, Publish, RequestError, Resume, Sources, Unanswered,
};
use linewright::briefs::OperatorPrompts;
use linewright::case;
use linewright::child;
use linewright::clock::{Clock, SystemClock};
use linewright::config::{
    AutomationCommand, Cli, Command, Config, EnvironmentCommand, Invocation, LogFormat,
    ResolveError, Settings, Since,
};
use linewright::domain::{Number, Timestamp};
use linewright::drain::{self, Asked, Exiting, Said, Stop, Taken, Timeout, Waiting, Was};
use linewright::ending::{Ends, Event, Signal, Step, StopBy};
use linewright::environment::shown::{self, Followed, FollowingBuild};
use linewright::environment::{self, BuildEnded, BuildId, RebuildSaid, RebuildTaken};
use linewright::factory::{Factory, FactoryConfig, ServeEnd, SessionToken};
use linewright::harness::claude::ClaudeCode;
use linewright::logs::{self, FollowQuery, Following, Which};
use linewright::repo_files::RepoLayer;
use linewright::runs::{self, Query, RepoWorkDir};
use linewright::secrets::Secrets;
use linewright::store::Store as _;
use linewright::store::github::{GitHub, SESSION_TOKEN_VARIABLE, TOKEN_VARIABLES};
use linewright::transcript::{self, After, InFlight, Verbosity};
use linewright::variable_names::VariableNames;
use linewright::webhook::{Listener, Pending};
use linewright::winding_up::{Remaining, WindingUp};
use linewright::work_settings::{Environments, RepoSettings};
use linewright::{COMMAND, COMMIT, VERSION};
use linewright::{contain, explain, status, web};
use tokio::signal::unix::signal;
use tracing::{error, info, warn};

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
    init_logging(config.log_format);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the runtime builds");
    let status = runtime.block_on(run(command, config));
    drop(runtime);
    info!(status, "exiting");
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

fn init_logging(format: LogFormat) {
    let filter = tracing_subscriber::EnvFilter::try_from_env("LINEWRIGHT_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    match format {
        LogFormat::Text => builder.init(),
        LogFormat::Json => builder.json().init(),
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
    let dir = environment::dir(&work_dir.resolve());
    let setup_command = match shown::setup_given(&dir) {
        Some(_) => false,
        None => setup_command_before_any_poll(config).await?,
    };
    shown::answer(&dir, setup_command)
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
    let repo = RepoLayer::read(&github, &branch, &prompts(config)?)
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
) -> u8 {
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
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let dir = socket_dir();
    let taken = match api::post_answer(&dir, &repo, "/api/environment/rebuild").await {
        Ok(body) => match read_as::<RebuildTaken>(&body, "a rebuild taken") {
            Ok(taken) => taken,
            Err(e) => return print_answer(Err(e)),
        },
        Err(RequestError::NotServing { .. }) => return rebuild_here(config, say, winding_up).await,
        Err(e) => return print_answer(Err(refused(&e))),
    };
    let build = match &taken {
        RebuildTaken::Started { build } | RebuildTaken::Queued { build } => *build,
        RebuildTaken::NoSetupCommand => return no_setup_command(),
        RebuildTaken::Refused { reason } => {
            return print_answer(Err((1, format!("no build starts: {reason}"))));
        }
    };
    say(&RebuildSaid::Taken(taken));
    if no_wait {
        return SUCCESS;
    }
    let resource = format!("/api/environment/rebuild/{build}");
    match api::get_answer(&dir, &repo, &resource).await {
        Ok(body) => match read_as::<BuildEnded>(&body, "a build ended") {
            Ok(ended) => {
                let code = ended_code(&ended);
                say(&RebuildSaid::Ended(ended));
                code
            }
            Err(e) => print_answer(Err(e)),
        },
        Err(e) => print_answer(Err(refused(&e))),
    }
}

/// Builds the environment without `serve`, and says how the build ended.
async fn rebuild_here(config: &Config, say: impl Fn(&RebuildSaid), winding_up: &WindingUp) -> u8 {
    let ended = async {
        let mut factory = start(config, OnUsageLimit::End, Ends::new(), winding_up).await?;
        factory.rebuild_environment().await
    }
    .await;
    match ended {
        Ok(Some(ended)) => {
            let code = ended_code(&ended);
            say(&RebuildSaid::Ended(ended));
            code
        }
        Ok(None) => no_setup_command(),
        Err(e) => print_answer(Err(failed(&e))),
    }
}

fn no_setup_command() -> u8 {
    print_answer(Err((
        1,
        "there is no setup command: set `setup.command` to build the environment".into(),
    )))
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
    let answer = ask_or_read(config, &resource, "runs", |work_dir| {
        runs::answer(work_dir, query, SystemClock.now())
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
            Format::Json => print!("{}", transcript::json_line(line)),
        }
        let _ = std::io::stdout().flush();
    }
}

async fn show_logs(config: &Config, which: &Which, printing: Printing, follow: bool) -> u8 {
    let resource = format!("/api/logs?{}", api::query_string(which));
    let answer = ask_or_read(config, &resource, "a transcript", |work_dir| {
        logs::answer(work_dir, which)
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
    let mut following = match Following::start(work_dir, &query.which, query.verbosity, after) {
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
    read: impl FnOnce(&RepoWorkDir) -> Result<T, E>,
) -> Result<Answer<T>, (u8, String)> {
    let work_dir = config.repo_work_dir().map_err(|e| failed(&e))?;
    match api::get_answer(&socket_dir(), &work_dir.repo, resource).await {
        Ok(body) => {
            let value = read_as(&body, what)?;
            Ok(Answer::Served { body, value })
        }
        Err(RequestError::NotServing { .. }) => match read(&work_dir).map_err(Into::into) {
            Ok(value) => Ok(Answer::Read(value)),
            Err(e) => Err(unanswered(e)),
        },
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
async fn run(command: Command, config: Config) -> u8 {
    let winding_up = WindingUp::default();
    let done = work(command, config, &winding_up).await;
    winding_up.wait().await;
    done.unwrap_or_else(|e| {
        error!(error = format!("{e:#}"), "failed");
        FAILURE
    })
}

async fn work(command: Command, config: Config, winding_up: &WindingUp) -> Result<u8> {
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
            return Ok(rebuild_environment(&config, no_wait, json, winding_up).await);
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
        Command::Serve => {
            info!(version = %VERSION, commit = %COMMIT, "starting");
            // PID 1, as `lw` can be in a container, is sent only the signals
            // it has a handler for:
            // https://man7.org/linux/man-pages/man2/kill.2.html#NOTES
            let signals = Signals::new().context("listening for stop signals")?;
            let ends = Ends::new();
            end_on(signals, ends.clone());
            let clock: Arc<dyn Clock> = Arc::new(SystemClock);
            let since = clock.now();
            let work_dir = config.repo_work_dir()?;
            let claimed = api::claim(&socket_dir(), &work_dir.repo)?;
            let pending = Pending::default();
            let listener = match &config.webhooks {
                Some(webhooks) => {
                    Some(Listener::bind(webhooks, pending.clone(), clock.clone()).await?)
                }
                None => None,
            };
            let (publish, served, polling, environment) = Publish::new();
            let sources = Sources {
                serving: status::Serving {
                    since,
                    max_runs: config.max_runs,
                },
                served,
                polling,
                deliveries: listener.as_ref().map(Listener::tally),
                clock,
                work_dir,
                ends: ends.clone(),
                environment,
                exiting: winding_up.exiting(),
            };
            let ui = match &config.ui {
                Some(ui) => Some(web::Listener::bind(ui, sources.clone()).await?),
                None => None,
            };
            let listeners = Listeners {
                webhooks: listener,
                ui,
                api: Api::serve(claimed, sources),
            };
            let stopped = serve(&config, &ends, &pending, &publish, winding_up).await;
            listeners.let_go_after(winding_up).await;
            stopped
        }
        Command::Once => {
            let mut factory = start(&config, OnUsageLimit::End, Ends::new(), winding_up).await?;
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
        Command::Automation {
            command: AutomationCommand::Run { name },
        } => {
            start(&config, OnUsageLimit::End, Ends::new(), winding_up)
                .await?
                .run_automation(&name)
                .await
        }
        Command::Capture { target, dir } => {
            let (repo, number) = target
                .split_once('#')
                .with_context(|| format!("{target:?} is not owner/name#N"))?;
            let number: u64 = number
                .parse()
                .with_context(|| format!("{target:?} is not owner/name#N"))?;
            let github = GitHub::connect(&repo.parse()?, token()?).await?;
            let sessions = config.work_dir.join(repo).join("sessions");
            case::capture(&github, Number(number), &dir, &sessions).await?;
            println!("Captured {target} in {}", dir.display());
            Ok(())
        }
        Command::Replay {
            case: case_dir,
            out,
        } => {
            let prompts = prompts(&config)?;
            let bash = require_bash()?;
            require_tool(HARNESS_PROGRAM).await?;
            let sessions = SessionCredentials::new(SessionToken::Withheld, &config.secrets)?;
            let environments = environments(&config, &sessions);
            let harness = harness(bash.clone(), &sessions)?;
            let factory_config = factory_config(
                &config,
                bash,
                prompts,
                environments,
                out.clone(),
                OnUsageLimit::End,
                sessions,
                Ends::new(),
                winding_up.clone(),
            );
            case::replay(
                &case_dir,
                &out,
                Arc::new(harness),
                Arc::new(SystemClock),
                factory_config,
            )
            .await?;
            println!("Replayed {} into {}", case_dir.display(), out.display());
            Ok(())
        }
    };
    done.map(|()| SUCCESS)
}

async fn serve(
    config: &Config,
    ends: &Ends,
    pending: &Pending,
    publish: &Publish,
    winding_up: &WindingUp,
) -> Result<()> {
    let mut factory = tokio::select! {
        factory = start(config, OnUsageLimit::Wait, ends.clone(), winding_up) => factory?,
        () = ends.drain_or_stop_begun() => {
            info!("ended while starting");
            return Ok(());
        }
    };
    match factory.serve(config.poll_interval, pending, publish).await {
        ServeEnd::Drained => {}
        ServeEnd::Stop => {
            info!("stopping: cancelling the runs in flight");
            factory.stop().await;
        }
    }
    Ok(())
}

/// Takes each signal as `ends` has it, exiting at once when it says so.
fn end_on(mut signals: Signals, ends: Ends) {
    tokio::spawn(async move {
        loop {
            let signal = signals.next().await;
            if let (_, Step::Exit) = ends.on(Event::Signal(signal)) {
                #[expect(
                    clippy::exit,
                    reason = "a signal while stopping exits at once, without waiting for the runs in flight"
                )]
                std::process::exit(signal.exit_status());
            }
        }
    });
}

/// The token as `gh` reads it: `GH_TOKEN`, then `GITHUB_TOKEN`.
fn token() -> Result<String> {
    TOKEN_VARIABLES
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|t| !t.is_empty()))
        .context("set GH_TOKEN or GITHUB_TOKEN to the token of Linewright's account")
}

async fn connect(config: &Config, token: String) -> Result<GitHub> {
    GitHub::connect(&config.required_repo()?, token).await
}

fn work_dir(config: &Config, github: &GitHub) -> std::path::PathBuf {
    config.work_dir.join(github.full_name())
}

/// The token sessions are given, and Linewright's secrets, which are
/// redacted from what sessions say.
struct SessionCredentials {
    token: SessionToken,
    secrets: Secrets,
}

impl SessionCredentials {
    /// Reads the secrets to redact from Linewright's environment, `names`
    /// being the `secrets` setting, warning of those too short to redact.
    fn new(token: SessionToken, names: &VariableNames) -> Result<SessionCredentials> {
        let secrets = Secrets::of(names, std::env::vars_os()).map_err(|e| anyhow!(e))?;
        for variable in secrets.short() {
            warn!(
                variable,
                "this secret is 1 to 5 characters, so Linewright neither redacts it \
                 nor refuses commits that hold it"
            );
        }
        Ok(SessionCredentials { token, secrets })
    }
}

fn harness(bash: PathBuf, sessions: &SessionCredentials) -> Result<ClaudeCode> {
    Ok(ClaudeCode {
        program: HARNESS_PROGRAM.into(),
        bash,
        github_token: sessions.token.given().map(str::to_string),
        redaction: sessions.secrets.redaction().clone(),
        container: std::env::current_exe().context("finding Linewright's own program")?,
    })
}

/// What sessions are given of Linewright's environment. Logs the names of
/// the variables they are not given and, of those, the ones each stage's
/// table gives its sessions; and the names, never the values, that
/// `session-env` sets.
fn environments(config: &Config, sessions: &SessionCredentials) -> Environments {
    let split = config.work.environments(
        &config.session_env,
        std::env::vars_os(),
        &ClaudeCode::variables_set_for_sessions(sessions.token.given()),
    );
    info!(
        variables = split.withheld.join(" "),
        "agent sessions are not given these variables of Linewright's environment; \
         `session-pass-env` names the ones they need"
    );
    for (stage, added) in &split.added {
        info!(
            stage = stage.name(),
            variables = added.join(" "),
            "of those, the stage's sessions are given these, which `session-pass-env` in its \
             table names"
        );
    }
    if !split.set.is_empty() {
        info!(
            set = split.set.join(" "),
            "`session-env` sets these variables in agent sessions"
        );
    }
    for (stage, set) in &split.stages_set {
        info!(
            stage = stage.name(),
            set = set.own.join(" "),
            taken = set.taken.join(" "),
            "`session-env` in the stage's table sets these variables in its sessions, \
             besides those it takes from the whole pipeline's"
        );
    }
    split.environments
}

/// The operator's prompts. Logs which files they provide.
fn prompts(config: &Config) -> Result<OperatorPrompts> {
    let prompts = OperatorPrompts::read(config.prompts.as_deref())?;
    if config.prompts.is_some() {
        let provided: Vec<&str> = prompts.layer().files.keys().map(String::as_str).collect();
        info!(
            files = provided.join(" "),
            "the operator's prompts provide these files"
        );
    }
    Ok(prompts)
}

#[expect(
    clippy::too_many_arguments,
    reason = "`serve` and `replay` each give most of them differently"
)]
fn factory_config(
    config: &Config,
    bash: PathBuf,
    prompts: OperatorPrompts,
    environments: Environments,
    work_dir: std::path::PathBuf,
    on_usage_limit: OnUsageLimit,
    sessions: SessionCredentials,
    ends: Ends,
    winding_up: WindingUp,
) -> FactoryConfig {
    FactoryConfig {
        spec_glob: config.spec_glob.clone(),
        spec_lookback: config.spec_lookback,
        done_window: config.done_window,
        max_runs: config.max_runs,
        ci_grace: config.ci_grace,
        ci_timeout: config.ci_timeout,
        ci_attempts: config.ci_attempts,
        result_retries: config.result_retries,
        bash,
        work: config.work.clone(),
        environments,
        update_method: config.update_branch,
        auto_merge: config.auto_merge,
        require_label: config.require_label.clone(),
        prompts,
        work_dir,
        schedule_automations: true,
        waits_out_usage_limit: matches!(on_usage_limit, OnUsageLimit::Wait),
        session_token: sessions.token,
        secrets: sessions.secrets,
        ends,
        winding_up,
    }
}

#[derive(Clone, Copy)]
enum OnUsageLimit {
    Wait,
    End,
}

async fn start(
    config: &Config,
    on_usage_limit: OnUsageLimit,
    ends: Ends,
    winding_up: &WindingUp,
) -> Result<Factory> {
    let prompts = prompts(config)?;
    require_tool("git").await?;
    let bash = require_bash()?;
    require_tool(HARNESS_PROGRAM).await?;
    let token = token()?;
    let sessions = SessionCredentials::new(
        SessionToken::new(token.clone(), std::env::var(SESSION_TOKEN_VARIABLE).ok()),
        &config.secrets,
    )?;
    let environments = environments(config, &sessions);
    let harness = harness(bash.clone(), &sessions)?;
    let github = connect(config, token).await?;
    let factory_config = factory_config(
        config,
        bash,
        prompts,
        environments,
        work_dir(config, &github),
        on_usage_limit,
        sessions,
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

struct Signals {
    quit: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

impl Signals {
    fn new() -> std::io::Result<Signals> {
        Ok(Signals {
            quit: signal(Signal::Quit.kind())?,
            interrupt: signal(Signal::Interrupt.kind())?,
            terminate: signal(Signal::Terminate.kind())?,
        })
    }

    /// Signals that arrive before the last one was read count as one, so two
    /// sent back to back take a single way.
    async fn next(&mut self) -> Signal {
        tokio::select! {
            _ = self.quit.recv() => Signal::Quit,
            _ = self.interrupt.recv() => Signal::Interrupt,
            _ = self.terminate.recv() => Signal::Terminate,
        }
    }
}

async fn require_tool(program: &str) -> Result<()> {
    let found = child::command(program)
        .arg("--version")
        .output()
        .await
        .is_ok_and(|o| o.status.success());
    if !found {
        bail!("`{program}` is not on the path");
    }
    Ok(())
}

/// The absolute path of the first `bash` on the `PATH`, or an error when
/// there is none.
fn require_bash() -> Result<PathBuf> {
    let found = std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join("bash"))
        .find(|candidate| is_executable(candidate));
    match found {
        Some(bash) => std::path::absolute(&bash).context("locating `bash`"),
        None => bail!("`bash` is not on the path"),
    }
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
