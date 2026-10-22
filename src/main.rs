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
use linewright::config::{AutomationCommand, Cli, Command, Config, LogFormat, ResolveError, Since};
use linewright::credentials::Credentials;
use linewright::domain::Number;
use linewright::drain::{self, Asked, Said, Stop, Taken, Timeout, Waiting, Was};
use linewright::ending::{Ends, Event, Signal, Step, StopBy};
use linewright::factory::{Factory, FactoryConfig, ServeEnd, SessionToken};
use linewright::harness::claude::ClaudeCode;
use linewright::logs::{self, FollowQuery, Following, Which};
use linewright::runs::{self, Query, RepoWorkDir};
use linewright::store::github::{GitHub, SESSION_TOKEN_VARIABLE, TOKEN_VARIABLES};
use linewright::transcript::{self, After, InFlight, Verbosity};
use linewright::webhook::{Listener, Pending};
use linewright::{COMMAND, COMMIT, VERSION};
use linewright::{explain, status};
use tokio::signal::unix::signal;
use tracing::{error, info};

const HARNESS_PROGRAM: &str = "claude";

/// Clap exits with 2 too for a command line it cannot parse.
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
) -> ExitCode {
    let repo = match config.required_repo() {
        Ok(repo) => repo,
        Err(e) => return print_answer(Err(failed(&e))),
    };
    let body = match api::get_answer(&socket_dir(), &repo, resource).await {
        Ok(body) => body,
        Err(e) => return print_answer(Err(refused(e))),
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
    ExitCode::SUCCESS
}

/// How often `drain` asks what is in flight while it waits.
const DRAIN_ASKS_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

async fn drain(config: &Config, no_wait: bool, timeout: Option<Timeout>, json: bool) -> ExitCode {
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
        Err(e) => return print_answer(Err(refused(e))),
    };
    let mut waiting = Waiting::default();
    say(&[Said::Taken { was: taken.was }]);
    say(&waiting.seen(&taken.runs));
    if no_wait {
        return ExitCode::SUCCESS;
    }
    // Answered as the stop begins, however soon `serve` exits after it.
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
        let status = match api::get_answer(&dir, &repo, "/api/status").await {
            Ok(body) => match read_as::<status::Status>(&body, "a status") {
                Ok(status) => Some(status),
                Err(e) => return print_answer(Err(e)),
            },
            Err(RequestError::NotServing { .. }) => None,
            // Asked again: a `serve` that hangs up is about to exit.
            Err(RequestError::HungUp(_)) => continue,
            Err(e) => return print_answer(Err(refused(e))),
        };
        // One started since, as a supervisor restarts it, is another.
        match status.filter(|status| status.linewright.serving_since == taken.serving_since) {
            Some(status) => say(&waiting.seen(&status.run_ids())),
            None => {
                // Answered before `serve` stopped answering.
                if let Some(answering) = stop.take() {
                    say(&stop_begun(answering.await));
                }
                say(&waiting.exited());
                return ExitCode::SUCCESS;
            }
        }
    }
}

fn read_as<T: serde::de::DeserializeOwned>(body: &[u8], what: &str) -> Result<T, (u8, String)> {
    serde_json::from_slice(body)
        .map_err(|e| (1, format!("`serve`'s answer does not read as {what}: {e}")))
}

async fn list_runs(config: &Config, query: &Query, json: bool) -> ExitCode {
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
            Format::Text => print!("{}", transcript::render_line(line, colour())),
            Format::Json => print!("{}", transcript::json_line(line)),
        }
        let _ = std::io::stdout().flush();
    }
}

async fn show_logs(config: &Config, which: &Which, printing: Printing, follow: bool) -> ExitCode {
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
) -> ExitCode {
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
            Ok(()) if ended => return ExitCode::SUCCESS,
            // Broken off: asked again, after the last line read.
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
            Err(e) => return print_answer(Err(refused(e))),
        }
    }
}

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
            Err(e) => return print_answer(Err(failed(&e))),
        }
    }
}

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
        Err(e) => Err(refused(e)),
    }
}

fn failed(e: &anyhow::Error) -> (u8, String) {
    (1, format!("{e:#}"))
}

fn unanswered(e: Unanswered) -> (u8, String) {
    match e {
        Unanswered::NoSuch { error, .. } => (NO_SUCH_ITEM, error),
        Unanswered::Failed(e) => failed(&e),
    }
}

fn refused(e: RequestError) -> (u8, String) {
    match e {
        RequestError::NotServing { .. } => (NOT_SERVING, e.to_string()),
        RequestError::NotFound(_) => (NO_SUCH_ITEM, e.to_string()),
        RequestError::HungUp(e) | RequestError::Other(e) => failed(&e),
    }
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

async fn run(command: Command, config: Config) -> Result<ExitCode> {
    let done = match command {
        // Its errors go to standard error as they are, not to the log.
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
            // First, so a log names the build it came from.
            info!(version = %VERSION, commit = %COMMIT, "starting");
            // Listening before startup, which talks to GitHub: in a container
            // `lw` can be PID 1, which the kernel sends no signal it
            // has no handler for.
            let signals = Signals::new().context("listening for stop signals")?;
            let ends = Ends::new();
            end_on(signals, ends.clone());
            let clock: Arc<dyn Clock> = Arc::new(SystemClock);
            let since = clock.now();
            // Before anything else, so that a second `serve` for the
            // repository stops at once.
            let work_dir = config.repo_work_dir()?;
            let claimed = api::claim(&socket_dir(), &work_dir.repo)?;
            let pending = Pending::default();
            let listener = match &config.webhooks {
                Some(webhooks) => {
                    Some(Listener::bind(webhooks, pending.clone(), clock.clone()).await?)
                }
                None => None,
            };
            let (served, answered_from) = tokio::sync::watch::channel(None);
            let (failed_polls, failures_from) = tokio::sync::watch::channel(None);
            let api = Api::serve(
                claimed,
                Sources {
                    serving: status::Serving {
                        since,
                        max_runs: config.max_runs,
                    },
                    served: answered_from,
                    failed_polls: failures_from,
                    deliveries: listener.as_ref().map(Listener::tally),
                    clock,
                    work_dir,
                    ends: ends.clone(),
                },
            );
            let stopped = serve(&config, &ends, &pending, &served, &failed_polls).await;
            if let Some(listener) = listener {
                listener.stop().await;
            }
            api.stop().await;
            stopped
        }
        Command::Once => {
            let mut factory = start(&config, OnUsageLimit::End, Ends::new()).await?;
            factory.cycle().await?;
            factory.settle().await;
            Ok(())
        }
        Command::Automation {
            command: AutomationCommand::Run { name },
        } => {
            start(&config, OnUsageLimit::End, Ends::new())
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
            let briefs = briefs(&config)?;
            let bash = require_bash()?;
            require_tool(HARNESS_PROGRAM).await?;
            let sessions = SessionCredentials::new(SessionToken::Withheld);
            let harness = harness(&config, bash, &sessions);
            let factory_config = factory_config(
                &config,
                briefs,
                out.clone(),
                OnUsageLimit::End,
                sessions,
                Ends::new(),
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
    done.map(|()| ExitCode::SUCCESS)
}

async fn serve(
    config: &Config,
    ends: &Ends,
    pending: &Pending,
    served: &tokio::sync::watch::Sender<Option<Served>>,
    failed_polls: &tokio::sync::watch::Sender<Option<status::FailedPolls>>,
) -> Result<()> {
    // No run can be in flight yet, so every drain or stop ends `serve` alike.
    let mut factory = tokio::select! {
        factory = start(config, OnUsageLimit::Wait, ends.clone()) => factory?,
        () = ends.drain_or_stop_begun() => {
            info!("ended while starting");
            return Ok(());
        }
    };
    match factory
        .serve(config.poll_interval, pending, served, failed_polls)
        .await
    {
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
                    reason = "a second signal exits at once, without stopping the runs in flight"
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

/// The token sessions are given and Linewright's credentials, redacted from
/// what they say, for the harness and the factory alike.
struct SessionCredentials {
    token: SessionToken,
    credentials: Credentials,
}

impl SessionCredentials {
    /// Reads the credentials to redact from Linewright's environment.
    fn new(token: SessionToken) -> SessionCredentials {
        SessionCredentials {
            token,
            credentials: Credentials::of(std::env::vars_os()),
        }
    }
}

/// Logs the names of the variables of Linewright's environment its sessions
/// are not given, so that an operator whose build fails only in a session
/// sees what to add to `session-env`.
fn harness(config: &Config, bash: PathBuf, sessions: &SessionCredentials) -> ClaudeCode {
    let github_token = sessions.token.given().map(str::to_string);
    let split = config.session_env.split(
        std::env::vars_os(),
        &ClaudeCode::variables_set_for_sessions(github_token.as_deref()),
    );
    info!(
        variables = split.withheld.join(" "),
        "agent sessions are not given these variables of Linewright's environment; \
         `session-env` names the ones they need"
    );
    ClaudeCode {
        program: HARNESS_PROGRAM.into(),
        bash,
        model: config.model.clone(),
        github_token,
        environment: split.given,
        redaction: sessions.credentials.redaction().clone(),
    }
}

/// Read before anything else at startup, so a mistake in them is found at once.
fn briefs(config: &Config) -> Result<Briefs> {
    match &config.briefs {
        Some(dir) => Briefs::load(dir),
        None => Ok(Briefs::shipped()),
    }
}

fn factory_config(
    config: &Config,
    briefs: Briefs,
    work_dir: std::path::PathBuf,
    on_usage_limit: OnUsageLimit,
    sessions: SessionCredentials,
    ends: Ends,
) -> FactoryConfig {
    FactoryConfig {
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
        schedule_automations: true,
        waits_out_usage_limit: matches!(on_usage_limit, OnUsageLimit::Wait),
        session_token: sessions.token,
        credentials: sessions.credentials,
        ends,
    }
}

#[derive(Clone, Copy)]
enum OnUsageLimit {
    Wait,
    End,
}

async fn start(config: &Config, on_usage_limit: OnUsageLimit, ends: Ends) -> Result<Factory> {
    let briefs = briefs(config)?;
    require_tool("git").await?;
    let bash = require_bash()?;
    require_tool(HARNESS_PROGRAM).await?;
    let token = token()?;
    let sessions = SessionCredentials::new(SessionToken::new(
        token.clone(),
        std::env::var(SESSION_TOKEN_VARIABLE).ok(),
    ));
    let harness = harness(config, bash, &sessions);
    let github = connect(config, token).await?;
    let factory_config = factory_config(
        config,
        briefs,
        work_dir(config, &github),
        on_usage_limit,
        sessions,
        ends,
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
    /// sent back to back are a single stop.
    async fn next(&mut self) -> Signal {
        tokio::select! {
            _ = self.quit.recv() => Signal::Quit,
            _ = self.interrupt.recv() => Signal::Interrupt,
            _ = self.terminate.recv() => Signal::Terminate,
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

/// Claude Code takes the shell it is told to use as a path, not a name to
/// look up.
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
