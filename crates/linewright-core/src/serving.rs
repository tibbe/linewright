//! `lw serve` (`docs/specs/operation.md`), over whichever store, harness and
//! clock its caller starts the factory with, and what starting one takes.

use std::future::Future;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use axum::Router;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::access::people::People;
use crate::api::{self, Api, Publish, Sources};
use crate::briefs::OperatorBriefs;
use crate::child;
use crate::clock::Clock;
use crate::config::{Config, Ui};
use crate::database::Database;
use crate::ending::{Ends, Event, Signals, Step};
use crate::factory::{Factory, FactoryConfig, ServeEnd, SessionToken};
use crate::harness::{HarnessSettings, Variables};
use crate::listening::Listening;
use crate::runs::Runs;
use crate::secrets::Secrets;
use crate::session_env::variables_set_for_sessions;
use crate::status;
use crate::store::SESSION_TOKEN_VARIABLE;
use crate::variable_names::VariableNames;
use crate::webhook::{Listener, Pending};
use crate::winding_up::WindingUp;
use crate::work_settings::{Environments, SessionWork, Split};

/// Serves `config`'s repository: claims its local API in `socket_dir`,
/// listens for webhooks and serves the web UI, `ui`'s router, where `config`
/// says, and only then calls `start` for the factory and serves with it
/// until a drain or a stop. The pages and the API tell the time by `clock`. A signal ends it as
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
    ui: UiRouter,
    start: Start,
) -> Result<()>
where
    Start: FnOnce(Ends, Database) -> Started,
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
    let database = Database::open(&work_dir.resolve()).await?;
    let pending = Pending::default();
    let listener = match &config.webhooks {
        Some(webhooks) => Some(Listener::bind(webhooks, pending.clone(), clock.clone()).await?),
        None => None,
    };
    let (publish, links) = Publish::new();
    let sources = Sources {
        serving: status::Serving {
            since,
            max_runs: config.max_runs,
        },
        served: links.served,
        polling: links.polling,
        deliveries: listener.as_ref().map(Listener::tally),
        clock,
        work_dir,
        runs: Runs::new(database.clone()),
        ends: ends.clone(),
        environment: links.environment,
        automations: links.automations,
        benchmark_runs: links.benchmark_runs,
        exiting: winding_up.exiting(),
    };
    let ui = match &config.ui {
        Some(settings) => Some(UiListener::bind(settings, sources.clone(), ui).await?),
        None => {
            info!(reason = "`public-url` is not set", "serving no web UI");
            None
        }
    };
    let listeners = Listeners {
        webhooks: listener,
        ui,
        api: Api::serve(claimed, sources),
    };
    let stopped = run_factory(config, &ends, &pending, &publish, |ends| {
        start(ends, database)
    })
    .await;
    listeners.let_go_after(winding_up).await;
    stopped
}

/// The router of the web UI over what `serve` answers from, signing people
/// in as the settings say, its event streams ended by the token.
pub type UiRouter = fn(Sources, &Ui, CancellationToken) -> Router;

/// The web UI listening on its port, until [`UiListener::stop`].
pub struct UiListener {
    address: SocketAddr,
    /// Ends the event streams.
    stopping: CancellationToken,
    listening: Listening,
}

impl UiListener {
    /// Serves the web UI `router` gives on `ui`'s address, with an error
    /// naming `ui-address` when it cannot.
    pub async fn bind(ui: &Ui, sources: Sources, router: UiRouter) -> Result<UiListener> {
        let socket = ui.address.bind("ui-address").await?;
        let address = socket.local_addr()?;
        let stopping = CancellationToken::new();
        if let Some(manager) = &ui.manager {
            let made = People::new(sources.runs.database().clone())
                .at_startup(manager)
                .await?;
            for person in made {
                info!(
                    person = %person.id,
                    identity = person.logged(),
                    "made the person `manager` names an enabled manager"
                );
            }
        }
        let router = router(sources, ui, stopping.clone());
        let listening = Listening::spawn(socket, router, "serving the web UI");
        info!(address = %address, "serving the web UI");
        Ok(UiListener {
            address,
            stopping,
            listening,
        })
    }

    /// The address it listens on, with the port the system chose for port 0.
    pub fn local_address(&self) -> SocketAddr {
        self.address
    }

    /// Ends the event streams, and returns once it no longer listens.
    pub async fn stop(self) {
        self.stopping.cancel();
        self.listening.stop().await;
    }
}

/// What `serve` listens on, the API holding the repository's claim.
pub struct Listeners {
    pub webhooks: Option<Listener>,
    pub ui: Option<UiListener>,
    pub api: Api,
}

impl Listeners {
    /// Once all `winding_up` waits for has ended, stops the webhook
    /// listener, then the web UI, and only once it has stopped the API,
    /// releasing the claim, so that a `serve` that starts once the claim is
    /// released can listen on the same `ui-address`
    /// (`docs/specs/operation.md`, Stopping, "Exiting").
    pub async fn let_go_after(self, winding_up: &WindingUp) {
        winding_up.wait().await;
        if let Some(webhooks) = self.webhooks {
            webhooks.stop().await;
        }
        if let Some(ui) = self.ui {
            ui.stop().await;
        }
        self.api.stop().await;
    }
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

/// What a factory starts with besides its store and its clock: `bash`,
/// the operator's briefs and settings for sessions, the token sessions are
/// given and the secrets redacted from what they say, and their
/// environments.
pub struct Starting {
    program: String,
    commit: &'static str,
    bash: PathBuf,
    briefs: OperatorBriefs,
    work: SessionWork,
    sessions: SessionCredentials,
    environments: Environments,
    trial_environments: Environments,
}

impl Starting {
    /// Reads them from `config` and Linewright's environment, for sessions
    /// of Claude Code as `program` given what `token` reads, once the tools
    /// are found.
    ///
    /// # Errors
    ///
    /// When the operator's briefs cannot be read, `git`, `bash` or
    /// `program` is not on the path, `token` fails, or a secret cannot be
    /// read.
    pub async fn new(
        config: &Config,
        program: &str,
        harness: &Variables,
        commit: &'static str,
        token: impl FnOnce() -> Result<SessionToken>,
    ) -> Result<Starting> {
        let briefs = briefs(config)?;
        let work = config.work.clone().for_sessions()?;
        require_tool("git").await?;
        let bash = require_bash()?;
        require_tool(program).await?;
        let sessions = SessionCredentials::new(token()?, &config.secrets, harness)?;
        let environments = environments(config, harness);
        let trial_environments = split_environments(config, harness, true).environments;
        Ok(Starting {
            program: program.into(),
            commit,
            bash,
            briefs,
            work,
            sessions,
            environments,
            trial_environments,
        })
    }

    /// What the harness runs its sessions with, each turn of its program
    /// run inside `container`'s wrapper ([`child::contained`]) and told when
    /// it started by `clock`.
    pub fn harness(&self, container: PathBuf, clock: Arc<dyn Clock>) -> HarnessSettings {
        HarnessSettings {
            program: self.program.clone(),
            bash: self.bash.clone(),
            github_token: self.sessions.token.given().to_string(),
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
        database: Database,
        ends: Ends,
        winding_up: WindingUp,
    ) -> FactoryConfig {
        FactoryConfig {
            spec_glob: config.spec_glob.clone(),
            spec_lookback: config.spec_lookback,
            done_window: config.done_window,
            max_runs: config.max_runs,
            max_trials: config.max_trials,
            ci_grace: config.ci_grace,
            ci_timeout: config.ci_timeout,
            ci_attempts: config.ci_attempts,
            result_retries: config.result_retries,
            bash: self.bash,
            work: self.work,
            environments: self.environments,
            trial_environments: self.trial_environments,
            merge_method: config.merge_method,
            auto_merge: config.auto_merge,
            require_label: config.require_label.clone(),
            briefs: self.briefs,
            work_dir,
            database,
            session_token: self.sessions.token,
            secrets: self.sessions.secrets,
            ends,
            winding_up,
            commit: self.commit,
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
    /// being the `secrets` setting, warning of those too short to redact,
    /// and of sessions given Linewright's token, which can write to GitHub.
    fn new(
        token: SessionToken,
        names: &VariableNames,
        harness: &Variables,
    ) -> Result<SessionCredentials> {
        let secrets = Secrets::of(names, std::env::vars_os(), harness).map_err(|e| anyhow!(e))?;
        for variable in secrets.short() {
            warn!(
                variable,
                "this secret is 1 to 5 characters, so Linewright neither redacts it \
                 nor refuses commits that hold it"
            );
        }
        if let SessionToken::Linewright(_) = token {
            warn!(
                "sessions can write to GitHub: they are given Linewright's token; set \
                 {SESSION_TOKEN_VARIABLE} to a token that only reads to keep them from it"
            );
        }
        Ok(SessionCredentials { token, secrets })
    }
}

/// What sessions are given of Linewright's environment, and what they are
/// not, their `gh` reaching GitHub through a gate when `gh_through_gate`.
fn split_environments(config: &Config, harness: &Variables, gh_through_gate: bool) -> Split {
    config.work.environments(
        &config.session_env,
        std::env::vars_os(),
        &variables_set_for_sessions(harness, gh_through_gate),
    )
}

/// What sessions are given of Linewright's environment. Logs the names of
/// the variables they are not given and, of those, the ones each
/// `[stage.NAME]` table gives its sessions; and the names, never the values, that
/// `session-env` sets.
fn environments(config: &Config, harness: &Variables) -> Environments {
    let split = split_environments(config, harness, false);
    info!(
        variables = split.withheld.join(" "),
        "agent sessions are not given these variables of Linewright's environment; \
         `session-pass-env` names the ones they need"
    );
    for (stage, added) in &split.added {
        info!(
            stage = stage.name(),
            variables = added.join(" "),
            "of those, the sessions of `[stage.NAME]` are given these, which `session-pass-env` \
             in that table names"
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
            "`session-env` in `[stage.NAME]` sets these variables in that table's sessions, \
             besides those it takes from the whole pipeline's"
        );
    }
    split.environments
}

/// The operator's briefs. Logs which files they provide, by their paths
/// under `briefs/`.
///
/// # Errors
///
/// When the `briefs` directory `config` names cannot be read, or holds a
/// file Linewright cannot use.
pub fn briefs(config: &Config) -> Result<OperatorBriefs> {
    let briefs = OperatorBriefs::read(config.briefs.as_deref())?;
    if config.briefs.is_some() {
        let provided: Vec<&str> = briefs.layer().files.keys().map(String::as_str).collect();
        info!(
            files = provided.join(" "),
            "the operator's briefs provide these files"
        );
    }
    Ok(briefs)
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
