//! `serve-fake`: `lw serve` over fakes, for an agent to drive Linewright's
//! web UI in a browser. A local store the seed sets up stands in for GitHub,
//! and `fake-claude`, playing the seed's scenario, for Claude Code, so
//! nothing it does reaches either. It serves the web UI over HTTPS, with a
//! self-signed certificate, on a free port of `127.0.0.1`, and once the
//! factory has started prints its URL and then the `playwright-cli` command
//! that sets the browser's clock to its own. It keeps all it writes in a
//! temporary directory it removes once a signal stops it, but for the work
//! directory when `SERVE_FAKE_WORK_DIR` names one: so that one release can
//! serve the work directory another left. With `SERVE_FAKE_UNPOLLED` set, each
//! poll fails, so that the web UI stays as it is before Linewright's first
//! poll. With `SERVE_FAKE_UNNAMED` set, the `manager` setting names no one, so
//! that the seed's manager is only the last manager.
//!
//! The seed's implement run stays in flight until it is cancelled, so
//! `SIGQUIT` stops `serve-fake`, and `SIGTERM` or `SIGINT` leaves it draining
//! until a second signal.
//!
//! It runs `lw` and `fake-claude` from beside itself, as
//! `cargo build --package linewright --features fake-claude` leaves them.

mod seed;
mod tls;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use linewright::COMMIT;
use linewright_claude::{ClaudeCode, PROGRAM};
use linewright_core::access::providers::fake::{FakeGitHub, FakeGoogle};
use linewright_core::access::providers::{GITHUB_SECRET_VARIABLE, GOOGLE_SECRET_VARIABLE};
use linewright_core::clock::{Clock, RunningFrom};
use linewright_core::config::{Config, Settings};
use linewright_core::factory::{Factory, SessionToken};
use linewright_core::reconcile::WorkKind;
use linewright_core::serving::{self, OnUsageLimit, Starting};
use linewright_core::store::local::LocalStore;
use linewright_core::winding_up::WindingUp;

use linewright_core::fake_claude::scenario::{self, Scenario};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,rmcp=warn")),
        )
        .init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve())
}

async fn serve() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let dir = dir.path();
    let beside = std::env::current_exe()?
        .parent()
        .context("serve-fake is in a directory")?
        .to_path_buf();
    let clock: Arc<dyn Clock> = Arc::new(RunningFrom::new(seed::started_at()));
    let agent = fake_agent(dir, &beside.join("fake-claude"), clock.as_ref())?;
    let front = tls::Front::bind().await?;
    let url = front.url()?;
    let ui_port = free_port()?;
    let google = FakeGoogle::serve(seed::google_accounts(), clock.clone()).await?;
    let github = FakeGitHub::serve(seed::github_accounts()).await?;
    let mut config = config(dir, &url, ui_port)?;
    let ui = config.ui.as_mut().context("serve-fake serves the web UI")?;
    ui.providers
        .google_mut()
        .context("serve-fake signs people in with Google")?
        .endpoints = google.endpoints();
    ui.providers
        .github_mut()
        .context("serve-fake signs people in with GitHub")?
        .endpoints = github.endpoints();
    let store =
        Arc::new(LocalStore::create(&dir.join("store"), seed::repository()?, clock.clone()).await?);
    seed::seed(&store).await?;
    seed::benchmarks(&store, &config.work_dir.join(seed::REPO)).await?;
    seed::interrupted_run(&config.work_dir.join(seed::REPO)).await?;
    seed::earlier_runs(&config.work_dir.join(seed::REPO)).await?;
    seed::close_done(&store)?;
    if std::env::var_os(UNPOLLED).is_some() {
        store.fail_reads("serve-fake was started with SERVE_FAKE_UNPOLLED");
    }
    let winding_up = WindingUp::default();
    tokio::spawn(front.forward(([127, 0, 0, 1], ui_port).into()));
    serving::serve(
        &config,
        clock.clone(),
        &dir.join("run"),
        &winding_up,
        linewright_web::router,
        |ends, database| async {
            let starting = Starting::new(&config, &agent, &PROGRAM.variables, COMMIT, || {
                Ok(SessionToken::Separate("serve-fake".into()))
            })
            .await?;
            seed::people(&database).await?;
            let harness = ClaudeCode(starting.harness(beside.join("lw"), clock.clone()));
            let factory_config = starting.factory_config(
                &config,
                config.work_dir.join(seed::REPO),
                database,
                OnUsageLimit::Wait,
                ends,
                winding_up.clone(),
            );
            let factory =
                Factory::start(store, Arc::new(harness), clock.clone(), factory_config).await?;
            println!("{url}");
            println!("{}", browser_clock(clock.as_ref()));
            Ok(factory)
        },
    )
    .await
}

/// How far `clock` runs ahead of the system's, in milliseconds.
fn ahead_millis(clock: &dyn Clock) -> i64 {
    (clock.now() - chrono::Utc::now()).num_milliseconds()
}

/// The `playwright-cli` command that has the browser tell the time `clock`
/// does, as the pages work out how long ago and how long for in the browser.
fn browser_clock(clock: &dyn Clock) -> String {
    let ahead = ahead_millis(clock);
    format!(
        "playwright-cli run-code \"async page => {{ await page.clock.setSystemTime(Date.now() + ({ahead})); }}\""
    )
}

/// `fake-claude` as `dir/claude`, with the seed's scenario beside it, its
/// events stamped by `clock`.
fn fake_agent(dir: &Path, fake_claude: &Path, clock: &dyn Clock) -> Result<String> {
    let program = dir.join("claude");
    std::os::unix::fs::symlink(fake_claude, &program)?;
    let scenario = Scenario {
        stamped_ahead_millis: Some(ahead_millis(clock)),
        ..seed::scenario()
    };
    std::fs::write(dir.join("claude.json"), serde_json::to_string(&scenario)?)?;
    program
        .into_os_string()
        .into_string()
        .map_err(|path| anyhow::anyhow!("{} is not UTF-8", PathBuf::from(path).display()))
}

/// The variable that names the work directory to keep.
const WORK_DIR: &str = "SERVE_FAKE_WORK_DIR";

/// The variable that, set, fails each poll.
const UNPOLLED: &str = "SERVE_FAKE_UNPOLLED";

/// The variable that, set, leaves the `manager` setting out.
const UNNAMED: &str = "SERVE_FAKE_UNNAMED";

/// Serving the seed's repository from `dir` on `ui_port`, at the public URL
/// `url`, with the work directory [`WORK_DIR`] names or one in `dir`, each
/// stage's sessions told their stage, as `fake-claude` takes its steps by it,
/// and the `manager` setting naming the seed's manager unless [`UNNAMED`] is
/// set.
fn config(dir: &Path, url: &str, ui_port: u16) -> Result<Config> {
    let work = std::env::var_os(WORK_DIR).map_or_else(|| dir.join("work"), PathBuf::from);
    let mut text = format!(
        "repo = \"{repo}\"\nwork-dir = \"{work}\"\nui-address = \"127.0.0.1:{ui_port}\"\n\
         public-url = \"{url}\"\ngoogle-client-id = \"serve-fake\"\n\
         google-signup-domains = [\"{domain}\"]\ngithub-client-id = \"serve-fake\"\n\
         github-signup-orgs = [\"{organization}\"]\n\
         poll-interval = \"1s\"\nmodel = \"claude-opus-5-5\"\n",
        repo = seed::REPO,
        work = work.display(),
        domain = seed::SIGNUP_DOMAIN,
        organization = seed::SIGNUP_ORGANIZATION,
    );
    if std::env::var_os(UNNAMED).is_none() {
        writeln!(text, "manager = \"google:{}\"", seed::MANAGER)?;
    }
    for name in WorkKind::ALL
        .map(WorkKind::name)
        .into_iter()
        .chain(["scorer"])
    {
        write!(
            text,
            "\n[stage.{name}]\nsession-env = {{ {variable} = \"{name}\" }}\n",
            variable = scenario::STAGE_VARIABLE,
        )?;
    }
    let path = dir.join("linewright.toml");
    std::fs::write(&path, text)?;
    let settings = Settings {
        config: Some(path),
        ..Settings::default()
    };
    let env = |name: &str| {
        [GOOGLE_SECRET_VARIABLE, GITHUB_SECRET_VARIABLE]
            .contains(&name)
            .then(|| "serve-fake".to_string())
    };
    Config::resolve(&settings, dir, &env, false, &PROGRAM.variables)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// A port of `127.0.0.1` nothing listens on.
fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}
