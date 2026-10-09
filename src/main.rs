use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Parser;
use linewright::briefs::Briefs;
use linewright::case;
use linewright::clock::{Clock, SystemClock};
use linewright::config::{Cli, Command, Config, LogFormat, WorkflowCommand};
use linewright::domain::Number;
use linewright::factory::{Factory, FactoryConfig};
use linewright::reconcile::Policy;
use linewright::runtime::claude::ClaudeCode;
use linewright::status;
use linewright::store::Store;
use linewright::store::github::{GitHub, TOKEN_VARIABLES};
use tokio::signal::unix::{Signal, SignalKind, signal};
use tracing::{error, info};

const RUNTIME: &str = "claude";

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
            eprintln!("linewright: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    init_logging(config.log_format);
    match run(cli.command, config).await {
        Ok(()) => ExitCode::SUCCESS,
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

async fn run(command: Command, config: Config) -> Result<()> {
    match command {
        Command::Serve => {
            // Listening before startup, which talks to GitHub: in a container
            // `linewright` can be PID 1, which the kernel sends no signal it
            // has no handler for.
            let mut signals = StopSignals::new().context("listening for stop signals")?;
            let mut factory = tokio::select! {
                factory = start(&config) => factory?,
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
            while !stopping.is_cancelled() {
                if let Err(e) = factory.cycle().await {
                    error!(error = format!("{e:#}"), "poll failed");
                }
                tokio::select! {
                    _ = tokio::time::sleep(config.poll_interval) => {}
                    () = stopping.cancelled() => {}
                }
            }
            info!("stopping: cancelling the runs in flight");
            factory.stop().await;
            Ok(())
        }
        Command::Once => {
            let mut factory = start(&config).await?;
            factory.cycle().await?;
            factory.settle().await;
            Ok(())
        }
        Command::Status => {
            let github = connect(&config, token()?).await?;
            let clock = SystemClock;
            let now = clock.now();
            let snapshot = github.snapshot(now - config.spec_lookback).await?;
            let policy = policy(&config, &github);
            let lines = status::report(&snapshot, now, &policy);
            if lines.is_empty() {
                println!("Nothing in flight.");
            }
            for line in lines {
                println!("{line}");
            }
            Ok(())
        }
        Command::Workflow {
            command: WorkflowCommand::Run { name },
        } => start(&config).await?.run_workflow(&name).await,
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
            require_tool(RUNTIME).await?;
            let factory_config = factory_config(&config, out.clone())?;
            case::replay(
                &case_dir,
                &out,
                Arc::new(runtime(&config, None)),
                Arc::new(SystemClock),
                factory_config,
            )
            .await?;
            println!("Replayed {} into {}", case_dir.display(), out.display());
            Ok(())
        }
    }
}

/// The token as `gh` reads it: `GH_TOKEN`, then `GITHUB_TOKEN`.
fn token() -> Result<String> {
    TOKEN_VARIABLES
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|t| !t.is_empty()))
        .context("set GH_TOKEN or GITHUB_TOKEN to the machine user's token")
}

async fn connect(config: &Config, token: String) -> Result<GitHub> {
    let repo = config.repo.as_deref().context("set `repo` (owner/name)")?;
    GitHub::connect(repo, token).await
}

fn policy(config: &Config, github: &GitHub) -> Policy {
    Policy {
        machine_user: github.machine_user(),
        spec_glob: config.spec_glob.clone(),
        spec_lookback: config.spec_lookback,
        max_workers: config.max_workers,
        ci_grace: config.ci_grace,
        ci_timeout: config.ci_timeout,
        ci_attempts: config.ci_attempts,
    }
}

/// The agent runtime, its sessions given `github_token`: the machine user's
/// in production, none in replay (`docs/specs/product.md`, "Replay").
fn runtime(config: &Config, github_token: Option<String>) -> ClaudeCode {
    ClaudeCode {
        program: RUNTIME.into(),
        model: config.model.clone(),
        github_token,
    }
}

fn factory_config(config: &Config, work_dir: std::path::PathBuf) -> Result<FactoryConfig> {
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
        briefs,
        work_dir,
    })
}

/// Startup: refuse to start when the factory cannot work.
async fn start(config: &Config) -> Result<Factory> {
    require_tool("git").await?;
    require_tool(RUNTIME).await?;
    let token = token()?;
    let github = connect(config, token.clone()).await?;
    let work_dir = config.work_dir.join(github.full_name());
    let factory_config = factory_config(config, work_dir)?;
    Factory::start(
        Arc::new(github),
        Arc::new(runtime(config, Some(token))),
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
