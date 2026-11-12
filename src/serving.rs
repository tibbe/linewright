//! `lw serve` (`docs/specs/operation.md`), over whichever store, harness and
//! clock its caller starts the factory with, and what starting one takes.

use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use tokio::signal::unix::signal;
use tracing::{info, warn};

use crate::api::{self, Api, Listeners, Publish, Sources};
use crate::briefs::OperatorPrompts;
use crate::child;
use crate::clock::Clock;
use crate::config::Config;
use crate::ending::{Ends, Event, Signal, Step};
use crate::factory::{Factory, FactoryConfig, ServeEnd, SessionToken};
use crate::harness::claude::ClaudeCode;
use crate::secrets::Secrets;
use crate::status;
use crate::variable_names::VariableNames;
use crate::web;
use crate::webhook::{Listener, Pending};
use crate::winding_up::WindingUp;
use crate::work_settings::Environments;

/// Serves `config`'s repository: claims its local API in `socket_dir`,
/// listens for webhooks and serves the web UI where `config` says, and only
/// then calls `start` for the factory and serves with it until a drain or a
/// stop. The pages and the API tell the time by `clock`. A signal ends it as
/// [`Ends`] says, and exits the process at once when it says so.
///
/// # Errors
///
/// When it cannot listen for signals, claim the API or bind a listener,
/// when `start` fails, or when serving fails.
pub async fn serve<Start, Started>(
    config: &Config,
    clock: Arc<dyn Clock>,
    socket_dir: &Path,
    winding_up: &WindingUp,
    start: Start,
) -> Result<()>
where
    Start: FnOnce(Ends) -> Started,
    Started: Future<Output = Result<Factory>>,
{
    // PID 1, as `lw` can be in a container, is sent only the signals it has
    // a handler for:
    // https://man7.org/linux/man-pages/man2/kill.2.html#NOTES
    let signals = Signals::new().context("listening for stop signals")?;
    let ends = Ends::new();
    end_on(signals, ends.clone());
    let since = clock.now();
    let work_dir = config.repo_work_dir()?;
    let claimed = api::claim(socket_dir, &work_dir.repo)?;
    let pending = Pending::default();
    let listener = match &config.webhooks {
        Some(webhooks) => Some(Listener::bind(webhooks, pending.clone(), clock.clone()).await?),
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
    let stopped = run_factory(config, &ends, &pending, &publish, start).await;
    listeners.let_go_after(winding_up).await;
    stopped
}

async fn run_factory<Start, Started>(
    config: &Config,
    ends: &Ends,
    pending: &Pending,
    publish: &Publish,
    start: Start,
) -> Result<()>
where
    Start: FnOnce(Ends) -> Started,
    Started: Future<Output = Result<Factory>>,
{
    let mut factory = tokio::select! {
        factory = start(ends.clone()) => factory?,
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

/// What a factory starts with besides its store and its clock: `bash`,
/// the operator's prompts, the token sessions are given and the secrets
/// redacted from what they say, and their environments.
pub struct Starting {
    program: String,
    bash: PathBuf,
    prompts: OperatorPrompts,
    sessions: SessionCredentials,
    environments: Environments,
}

impl Starting {
    /// Reads them from `config` and Linewright's environment, for sessions
    /// of Claude Code as `program` given what `token` reads, once the tools
    /// are found.
    ///
    /// # Errors
    ///
    /// When the operator's prompts cannot be read, `git`, `bash` or
    /// `program` is not on the path, `token` fails, or a secret cannot be
    /// read.
    pub async fn new(
        config: &Config,
        program: &str,
        token: impl FnOnce() -> Result<SessionToken>,
    ) -> Result<Starting> {
        let prompts = prompts(config)?;
        require_tool("git").await?;
        let bash = require_bash()?;
        require_tool(program).await?;
        let sessions = SessionCredentials::new(token()?, &config.secrets)?;
        let environments = environments(config, &sessions);
        Ok(Starting {
            program: program.into(),
            bash,
            prompts,
            sessions,
            environments,
        })
    }

    /// Claude Code, each turn of it run inside `container`'s wrapper
    /// ([`child::contained`]) and told when it started by `clock`.
    pub fn harness(&self, container: PathBuf, clock: Arc<dyn Clock>) -> ClaudeCode {
        ClaudeCode {
            program: self.program.clone(),
            bash: self.bash.clone(),
            github_token: self.sessions.token.given().map(str::to_string),
            redaction: self.sessions.secrets.redaction().clone(),
            container,
            clock,
        }
    }

    /// The factory's config, keeping its work in `work_dir`.
    pub fn factory_config(
        self,
        config: &Config,
        work_dir: PathBuf,
        on_usage_limit: OnUsageLimit,
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
            bash: self.bash,
            work: config.work.clone(),
            environments: self.environments,
            merge_method: config.merge_method,
            auto_merge: config.auto_merge,
            require_label: config.require_label.clone(),
            prompts: self.prompts,
            work_dir,
            waits_out_usage_limit: matches!(on_usage_limit, OnUsageLimit::Wait),
            session_token: self.sessions.token,
            secrets: self.sessions.secrets,
            ends,
            winding_up,
        }
    }
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
///
/// # Errors
///
/// When the `prompts` directory `config` names cannot be read.
pub fn prompts(config: &Config) -> Result<OperatorPrompts> {
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

#[derive(Clone, Copy)]
pub enum OnUsageLimit {
    Wait,
    End,
}

/// Fails when `program --version` does not run and succeed.
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

/// The absolute path of the first `bash` on the `PATH`, failing when there
/// is none.
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
