use std::io::{IsTerminal, Write as _};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use linewright::api::{self, Api, EventId, RequestError, Resume, Served, Sources, Unanswered};
use linewright::briefs::Briefs;
use linewright::case;
use linewright::clock::{Clock, SystemClock};
use linewright::config::{Cli, Command, Config, LogFormat, ResolveError, Since, WorkflowCommand};
use linewright::domain::Number;
use linewright::factory::{Factory, FactoryConfig};
use linewright::logs::{self, FollowQuery, Following, Which};
use linewright::runs::{self, Query, RepoWorkDir};
use linewright::runtime::claude::ClaudeCode;
use linewright::store::github::{GitHub, TOKEN_VARIABLES};
use linewright::transcript::{self, After, InFlight, Verbosity};
use linewright::webhook::{Listener, Pending};
use linewright::{COMMAND, COMMIT, VERSION};
use linewright::{explain, status};
use tokio::signal::unix::{Signal, SignalKind, signal};
use tracing::{error, info};

const RUNTIME: &str = "claude";

/// The exit statuses beyond success and failure (`docs/specs/monitoring.md`,
/// "Output"). Clap exits with 2 too for a command line it cannot parse.
const WRONG_COMMAND_LINE: u8 = 2;
const NOT_SERVING: u8 = 3;
const NO_SUCH_ITEM: u8 = 4;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let cwd = std::env::current_dir().unwrap_or_default();
    let config = match Config::resolve(
        &cli.settings,
        &cwd,
        &|key| std::env::var(key).ok(),
        std::io::stderr().is_terminal(),
    ) {
        Ok(config) => config,
        Err(e) => {
            complain(&e);
            return match e {
                ResolveError::CommandLine(_) => ExitCode::from(WRONG_COMMAND_LINE),
                ResolveError::Elsewhere(_) => ExitCode::FAILURE,
            };
        }
    };
    init_logging(config.log_format);
    match run(cli.command, config).await {
        Ok(code) => code,
        Err(e) => {
            error!(error = format!("{e:#}"), "exiting");
            ExitCode::FAILURE
        }
    }
}

/// Prints `message` to standard error as an error of the program's own,
/// without the log's decoration.
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

/// A monitoring command: asks the `serve` of the configured repository for
/// `resource` and prints its answer, a `what`, as text by `render` or, with
/// `json`, as it came. Errors go to standard error, without the log's
/// decoration.
async fn ask<T: serde::de::DeserializeOwned>(
    config: &Config,
    resource: &str,
    what: &str,
    json: bool,
    render: fn(&T, bool) -> String,
) -> ExitCode {
    let repo = match config.required_repo() {
        Ok(repo) => repo,
        Err(e) => return print_answer(Err(failed(e))),
    };
    let body = match api::get_answer(&socket_dir(), repo, resource).await {
        Ok(body) => body,
        Err(e) => return print_answer(Err(refused(e))),
    };
    let text = if json {
        String::from_utf8_lossy(&body).into_owned()
    } else {
        match serde_json::from_slice::<T>(&body) {
            Ok(answer) => render(&answer, colour()),
            Err(e) => {
                let error = anyhow!("`serve`'s answer does not read as {what}: {e}");
                return print_answer(Err(failed(error)));
            }
        }
    };
    print!("{text}");
    if json && !text.ends_with('\n') {
        println!();
    }
    ExitCode::SUCCESS
}

/// `lw runs`: the runs `query` asks for, from the `serve` of the
/// configured repository or, with none running, from the records it keeps,
/// printed as text or, with `json`, as JSON.
async fn list_runs(config: &Config, query: &Query, json: bool) -> ExitCode {
    let resource = format!("/runs?{}", api::query_string(query));
    let answer = ask_or_read(config, &resource, "runs", |work_dir| {
        runs::answer(work_dir, query, SystemClock.now())
    })
    .await;
    print_answer(answer.map(|answer| answer.text(json)))
}

/// How `logs` prints a transcript.
#[derive(Clone, Copy)]
struct Printing {
    verbosity: Verbosity,
    format: Format,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    /// Text, coloured on a terminal.
    Text,
    /// A JSON object per line.
    Json,
}

impl Printing {
    fn transcript(self, transcript: &transcript::Transcript) -> String {
        match self.format {
            Format::Text => transcript::render(transcript, self.verbosity, colour()),
            Format::Json => transcript::json_lines(transcript, self.verbosity),
        }
    }

    /// Prints `line` of a followed transcript at once.
    fn line(self, line: &transcript::Line) {
        match self.format {
            Format::Text => print!("{}", transcript::render_line(line, colour())),
            Format::Json => print!("{}", transcript::json_line(line)),
        }
        let _ = std::io::stdout().flush();
    }
}

/// `lw logs`: the transcript of the run `which` names, from the
/// `serve` of the configured repository or, with none running, from the
/// records it keeps, printed as `printing` says. With `follow`, a run in
/// flight's is followed until the run ends.
async fn show_logs(config: &Config, which: &Which, printing: Printing, follow: bool) -> ExitCode {
    let resource = format!("/logs?{}", api::query_string(which));
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
                id: transcript.run.clone(),
                item: which.item(),
            },
            verbosity: printing.verbosity,
        };
        // So far as `logs` shows it, unless no session has written yet.
        let after = match in_flight {
            InFlight::Unread => None,
            InFlight::ReadTo(read) => {
                print!("{}", printing.transcript(transcript));
                Some(Resume {
                    run: transcript.run.clone(),
                    after: After::Read(read),
                })
            }
        };
        return follow_logs(config, query, after, printing).await;
    }
    print_answer(answer.map(|answer| printing.transcript(answer.value())))
}

/// `lw logs --follow` of the run in flight `query` names: each line of its
/// transcript after `after`, or from the first, as it is written,
/// until the run ends, from the `serve` of the configured repository,
/// resumed after the last line read when its stream breaks off or it hangs
/// up, or, with none running, from the records it keeps.
async fn follow_logs(
    config: &Config,
    query: FollowQuery,
    mut after: Option<Resume>,
    printing: Printing,
) -> ExitCode {
    let work_dir = match config.repo_work_dir() {
        Ok(work_dir) => work_dir,
        Err(e) => return print_answer(Err(failed(e))),
    };
    let resource = format!("/logs/follow?{}", api::query_string(&query));
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
            return print_answer(Err(failed(error)));
        }
        if sent {
            hang_ups.sent();
        }
        match followed {
            Ok(()) if ended => return ExitCode::SUCCESS,
            // Broken off: asked again, after the last line read.
            Ok(()) => tokio::time::sleep(api::ASK_AGAIN_AFTER).await,
            // Hung up on, as when `serve` stops as it is asked: asked again,
            // after the last line read, as `api::get_answer` asks.
            Err(RequestError::HungUp(e)) => {
                if !hang_ups.ask_again() {
                    return print_answer(Err(failed(e)));
                }
                tokio::time::sleep(api::ASK_AGAIN_AFTER).await;
            }
            Err(RequestError::NotServing { .. }) => {
                let after = after.map(|resume| resume.after);
                return follow_records(&work_dir, &query, after, printing).await;
            }
            Err(e) => return print_answer(Err(refused(e))),
        }
    }
}

/// Follows the transcript of the run `query` names in the records in
/// `work_dir`, printing each line after `after`, or from the first, as it
/// is written, until the run ends.
async fn follow_records(
    work_dir: &RepoWorkDir,
    query: &FollowQuery,
    after: Option<After>,
    printing: Printing,
) -> ExitCode {
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
            Ok(None) => return ExitCode::SUCCESS,
            Err(e) => return print_answer(Err(failed(e))),
        }
    }
}

/// Prints `answer`, or its error to standard error, without the log's
/// decoration, and gives the exit status.
fn print_answer(answer: Result<String, (u8, String)>) -> ExitCode {
    match answer {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err((code, message)) => {
            complain(message);
            ExitCode::from(code)
        }
    }
}

/// Asks the `serve` of the configured repository for `resource`, an answer
/// that reads as `what`, or, with none running, reads it from the records
/// with `read`. Fails with the exit status and the error.
async fn ask_or_read<T: serde::de::DeserializeOwned, E: Into<Unanswered>>(
    config: &Config,
    resource: &str,
    what: &str,
    read: impl FnOnce(&RepoWorkDir) -> Result<T, E>,
) -> Result<Answer<T>, (u8, String)> {
    let work_dir = config.repo_work_dir().map_err(failed)?;
    match api::get_answer(&socket_dir(), &work_dir.repo, resource).await {
        Ok(body) => {
            let value = serde_json::from_slice(&body)
                .map_err(|e| (1, format!("`serve`'s answer does not read as {what}: {e}")))?;
            Ok(Answer::Served { body, value })
        }
        Err(RequestError::NotServing { .. }) => match read(&work_dir).map_err(Into::into) {
            Ok(value) => Ok(Answer::Read(value)),
            Err(e) => Err(unanswered(e)),
        },
        Err(e) => Err(refused(e)),
    }
}

/// The exit status and message of `e`.
fn failed(e: anyhow::Error) -> (u8, String) {
    (1, format!("{e:#}"))
}

/// The exit status and message of an answer the records do not give.
fn unanswered(e: Unanswered) -> (u8, String) {
    match e {
        Unanswered::NoSuch { error, .. } => (NO_SUCH_ITEM, error),
        Unanswered::Failed(e) => failed(e),
    }
}

/// The exit status and message of an answer `serve` does not give.
fn refused(e: RequestError) -> (u8, String) {
    match e {
        RequestError::NotServing { .. } => (NOT_SERVING, e.to_string()),
        RequestError::NotFound(_) => (NO_SUCH_ITEM, e.to_string()),
        RequestError::HungUp(e) | RequestError::Other(e) => failed(e),
    }
}

/// An answer: `serve`'s, or one read with no `serve` running.
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
    /// The answer as text or, with `json`, as JSON: `serve`'s as it sent it
    /// (`docs/specs/architecture.md`, "The local API").
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

/// Whether to colour what goes to standard output: only on a terminal.
fn colour() -> bool {
    std::io::stdout().is_terminal()
}

/// The directory the local API's sockets are in, for this process's user
/// and environment.
fn socket_dir() -> PathBuf {
    api::socket_dir(
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        &std::env::temp_dir(),
        rustix::process::getuid().as_raw(),
    )
}

async fn run(command: Command, config: Config) -> Result<ExitCode> {
    let done = match command {
        // Its errors go to standard error as they are, not to the log.
        Command::Status { json } => {
            let resource = "/status";
            return Ok(ask(&config, resource, "a status", json, status::render).await);
        }
        Command::Explain { number, json } => {
            let resource = format!("/items/{number}");
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
            // First, so a log names the build it came from.
            info!(version = %VERSION, commit = %COMMIT, "starting");
            // Listening before startup, which talks to GitHub: in a container
            // `lw` can be PID 1, which the kernel sends no signal it
            // has no handler for.
            let signals = StopSignals::new().context("listening for stop signals")?;
            let clock: Arc<dyn Clock> = Arc::new(SystemClock);
            let since = clock.now();
            // Before anything else, so that a second `serve` for the
            // repository stops at once.
            let work_dir = config.repo_work_dir()?;
            let claimed = api::claim(&socket_dir(), &work_dir.repo)?;
            // `once` and the other commands never listen
            // (`docs/specs/webhooks.md`).
            let pending = Pending::default();
            let listener = match &config.webhooks {
                Some(webhooks) => {
                    Some(Listener::bind(webhooks, pending.clone(), clock.clone()).await?)
                }
                None => None,
            };
            let (served, answered_from) = tokio::sync::watch::channel(None);
            let api = Api::serve(
                claimed,
                Sources {
                    serving: status::Serving {
                        since,
                        max_runs: config.max_runs,
                    },
                    served: answered_from,
                    deliveries: listener.as_ref().map(Listener::tally),
                    clock,
                    work_dir,
                },
            );
            let stopped = serve(&config, signals, &pending, &served).await;
            if let Some(listener) = listener {
                listener.stop().await;
            }
            api.stop().await;
            stopped
        }
        Command::Once => {
            let mut factory = start(&config, OnUsageLimit::End).await?;
            factory.cycle().await?;
            factory.settle().await;
            Ok(())
        }
        Command::Workflow {
            command: WorkflowCommand::Run { name },
        } => {
            start(&config, OnUsageLimit::End)
                .await?
                .run_workflow(&name)
                .await
        }
        Command::Capture { target, dir } => {
            let (repo, number) = target
                .split_once('#')
                .with_context(|| format!("{target:?} is not owner/name#N"))?;
            let number: u64 = number
                .parse()
                .with_context(|| format!("{target:?} is not owner/name#N"))?;
            let github = GitHub::connect(repo, token()?).await?;
            let sessions = config.work_dir.join(repo).join("sessions");
            case::capture(&github, Number(number), &dir, &sessions).await?;
            println!("Captured {target} in {}", dir.display());
            Ok(())
        }
        Command::Replay {
            case: case_dir,
            out,
        } => {
            let bash = require_bash()?;
            require_tool(RUNTIME).await?;
            let factory_config = factory_config(&config, out.clone(), OnUsageLimit::End)?;
            case::replay(
                &case_dir,
                &out,
                Arc::new(runtime(&config, bash, None)),
                Arc::new(SystemClock),
                factory_config,
            )
            .await?;
            println!("Replayed {} into {}", case_dir.display(), out.display());
            Ok(())
        }
    };
    done.map(|()| ExitCode::SUCCESS)
}

/// Starts the factory and serves, polling and reading again the items
/// deliveries add to `pending` and sending its view to `view`, until a stop
/// signal.
async fn serve(
    config: &Config,
    mut signals: StopSignals,
    pending: &Pending,
    served: &tokio::sync::watch::Sender<Option<Served>>,
) -> Result<()> {
    let mut factory = tokio::select! {
        factory = start(config, OnUsageLimit::Wait) => factory?,
        signal = signals.next() => {
            info!(signal = signal.name(), "stopped while starting");
            return Ok(());
        }
    };
    let stopping = factory.stopping();
    tokio::spawn({
        let stopping = stopping.clone();
        async move {
            let first = signals.next().await;
            info!(
                signal = first.name(),
                "stopping after the action in progress; a second signal exits at once"
            );
            stopping.cancel();
            let second = signals.next().await;
            // 128 plus the signal's number, as a shell reports it
            // (<https://www.gnu.org/software/bash/manual/html_node/Exit-Status.html>).
            std::process::exit(128 + second.kind().as_raw_value());
        }
    });
    factory.serve(config.poll_interval, pending, served).await;
    info!("stopping: cancelling the runs in flight");
    factory.stop().await;
    Ok(())
}

/// The token as `gh` reads it: `GH_TOKEN`, then `GITHUB_TOKEN`.
fn token() -> Result<String> {
    TOKEN_VARIABLES
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|t| !t.is_empty()))
        .context("set GH_TOKEN or GITHUB_TOKEN to the token of Linewright's account")
}

async fn connect(config: &Config, token: String) -> Result<GitHub> {
    GitHub::connect(config.required_repo()?, token).await
}

/// The repository's work directory: its clones, session records and the
/// workflows' last runs.
fn work_dir(config: &Config, github: &GitHub) -> std::path::PathBuf {
    config.work_dir.join(github.full_name())
}

/// The agent runtime, its sessions given `github_token`: the token of
/// Linewright's account in production, none in replay (`docs/specs/replay.md`).
fn runtime(config: &Config, bash: PathBuf, github_token: Option<String>) -> ClaudeCode {
    ClaudeCode {
        program: RUNTIME.into(),
        bash,
        model: config.model.clone(),
        github_token,
    }
}

fn factory_config(
    config: &Config,
    work_dir: std::path::PathBuf,
    on_usage_limit: OnUsageLimit,
) -> Result<FactoryConfig> {
    let briefs = match &config.briefs {
        Some(dir) => Briefs::load(dir)?,
        None => Briefs::shipped(),
    };
    Ok(FactoryConfig {
        spec_glob: config.spec_glob.clone(),
        spec_lookback: config.spec_lookback,
        max_runs: config.max_runs,
        ci_grace: config.ci_grace,
        ci_timeout: config.ci_timeout,
        ci_attempts: config.ci_attempts,
        result_retries: config.result_retries,
        session_timeout: config.session_timeout,
        update_method: config.update_branch,
        auto_merge: config.auto_merge,
        merge_method: config.merge_method,
        require_label: config.require_label.clone(),
        briefs,
        work_dir,
        schedule_workflows: true,
        waits_out_usage_limit: matches!(on_usage_limit, OnUsageLimit::Wait),
    })
}

/// What a run refused for the usage limit does: only `serve` waits
/// (`docs/specs/operation.md`, "Usage limit").
enum OnUsageLimit {
    Wait,
    End,
}

/// Startup: refuse to start when the factory cannot work.
async fn start(config: &Config, on_usage_limit: OnUsageLimit) -> Result<Factory> {
    require_tool("git").await?;
    let bash = require_bash()?;
    require_tool(RUNTIME).await?;
    let token = token()?;
    let github = connect(config, token.clone()).await?;
    let factory_config = factory_config(config, work_dir(config, &github), on_usage_limit)?;
    Factory::start(
        Arc::new(github),
        Arc::new(runtime(config, bash, Some(token))),
        Arc::new(SystemClock),
        factory_config,
    )
    .await
}

/// SIGINT (Ctrl-C) and SIGTERM, which `docker stop` and service managers
/// send: both ask `serve` to stop.
struct StopSignals {
    interrupt: Signal,
    terminate: Signal,
}

#[derive(Clone, Copy)]
enum StopSignal {
    Interrupt,
    Terminate,
}

impl StopSignal {
    fn name(self) -> &'static str {
        match self {
            StopSignal::Interrupt => "SIGINT",
            StopSignal::Terminate => "SIGTERM",
        }
    }

    fn kind(self) -> SignalKind {
        match self {
            StopSignal::Interrupt => SignalKind::interrupt(),
            StopSignal::Terminate => SignalKind::terminate(),
        }
    }
}

impl StopSignals {
    fn new() -> std::io::Result<StopSignals> {
        Ok(StopSignals {
            interrupt: signal(StopSignal::Interrupt.kind())?,
            terminate: signal(StopSignal::Terminate.kind())?,
        })
    }

    /// The next stop signal. Signals that arrive before the last one was read
    /// count as one, so two sent back to back are a single stop.
    async fn next(&mut self) -> StopSignal {
        tokio::select! {
            _ = self.interrupt.recv() => StopSignal::Interrupt,
            _ = self.terminate.recv() => StopSignal::Terminate,
        }
    }
}

async fn require_tool(program: &str) -> Result<()> {
    let found = tokio::process::Command::new(program)
        .arg("--version")
        .output()
        .await
        .is_ok_and(|o| o.status.success());
    if !found {
        bail!("`{program}` is not on the path");
    }
    Ok(())
}

/// The first `bash` on the path. Claude Code takes the shell it is told to
/// use as a path, not a name to look up.
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
