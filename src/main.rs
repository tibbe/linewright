use std::io::IsTerminal;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Parser;
use linewright::api::{self, Api, RequestError, Sources};
use linewright::briefs::Briefs;
use linewright::case;
use linewright::clock::{Clock, SystemClock};
use linewright::config::{Cli, Command, Config, LogFormat, ResolveError, WorkflowCommand};
use linewright::domain::Number;
use linewright::factory::{Factory, FactoryConfig};
use linewright::runtime::claude::ClaudeCode;
use linewright::status;
use linewright::store::github::{GitHub, TOKEN_VARIABLES};
use linewright::webhook::{Listener, Pending};
use linewright::{COMMIT, VERSION};
use tokio::signal::unix::{Signal, SignalKind, signal};
use tracing::{error, info};

const RUNTIME: &str = "claude";

/// The exit statuses beyond success and failure (`docs/specs/monitoring.md`,
/// "Output"). Clap exits with 2 too for a command line it cannot parse.
const WRONG_COMMAND_LINE: u8 = 2;
const NOT_SERVING: u8 = 3;

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
            eprintln!("linewright: {e}");
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

/// `linewright status`: asks the `serve` of the configured repository and
/// prints its answer, as text or, with `json`, as it came. Errors go to
/// standard error, without the log's decoration.
async fn status(config: &Config, json: bool) -> ExitCode {
    let repo = match config.required_repo() {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("linewright: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let body = match api::get_answer(&socket_dir(), repo, "/status").await {
        Ok(body) => body,
        Err(e @ RequestError::NotServing { .. }) => {
            eprintln!("linewright: {e}");
            return ExitCode::from(NOT_SERVING);
        }
        Err(RequestError::Other(e)) => {
            eprintln!("linewright: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let text = if json {
        String::from_utf8_lossy(&body).into_owned()
    } else {
        match serde_json::from_slice::<status::Status>(&body) {
            Ok(answer) => status::render(&answer, colour()),
            Err(e) => {
                eprintln!("linewright: `serve`'s answer does not read as a status: {e}");
                return ExitCode::FAILURE;
            }
        }
    };
    print!("{text}");
    if json && !text.ends_with('\n') {
        println!();
    }
    ExitCode::SUCCESS
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
        Command::Status { json } => return Ok(status(&config, json).await),
        Command::Serve => {
            // First, so a log names the build it came from.
            info!(version = %VERSION, commit = %COMMIT, "starting");
            // Listening before startup, which talks to GitHub: in a container
            // `linewright` can be PID 1, which the kernel sends no signal it
            // has no handler for.
            let signals = StopSignals::new().context("listening for stop signals")?;
            let clock: Arc<dyn Clock> = Arc::new(SystemClock);
            let since = clock.now();
            // Before anything else, so that a second `serve` for the
            // repository stops at once.
            let claimed = api::claim(&socket_dir(), config.required_repo()?)?;
            // `once` and the other commands never listen
            // (`docs/specs/webhooks.md`).
            let pending = Pending::default();
            let listener = match &config.webhooks {
                Some(webhooks) => {
                    Some(Listener::bind(webhooks, pending.clone(), clock.clone()).await?)
                }
                None => None,
            };
            let (view, viewed) = tokio::sync::watch::channel(None);
            let api = Api::serve(
                claimed,
                Sources {
                    serving: status::Serving {
                        since,
                        max_runs: config.max_workers,
                    },
                    view: viewed,
                    deliveries: listener.as_ref().map(Listener::tally),
                    clock,
                },
            );
            let served = serve(&config, signals, &pending, &view).await;
            if let Some(listener) = listener {
                listener.stop().await;
            }
            api.stop().await;
            served
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
    view: &tokio::sync::watch::Sender<Option<status::View>>,
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
    factory.serve(config.poll_interval, pending, view).await;
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
        max_workers: config.max_workers,
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
