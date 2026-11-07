//! `serve-fake`: `lw serve` over fakes, for an agent to drive Linewright's
//! web UI in a browser. A local store the seed sets up stands in for GitHub,
//! and `fake-claude`, playing the seed's scenario, for Claude Code, so
//! nothing it does reaches either. It serves the web UI on a free port of
//! `127.0.0.1`, and once the factory has started prints its URL and then the
//! `playwright-cli` command that sets the browser's clock to its own. It keeps
//! all it writes in a temporary directory it removes once a signal stops it.
//!
//! It runs `lw` and `fake-claude` from beside itself, as
//! `cargo build --features fake-claude` leaves them.

#[path = "../fake-claude/scenario.rs"]
mod scenario;
mod seed;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use linewright::clock::{Clock, RunningFrom};
use linewright::config::{Config, Settings};
use linewright::factory::{Factory, SessionToken};
use linewright::reconcile::WorkKind;
use linewright::serving::{self, OnUsageLimit, Starting};
use linewright::store::local::LocalStore;
use linewright::winding_up::WindingUp;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
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
    let agent = fake_agent(dir, &beside.join("fake-claude"))?;
    let port = free_port()?;
    let config = config(dir, port)?;
    let clock: Arc<dyn Clock> = Arc::new(RunningFrom::new(seed::started_at()));
    let store =
        Arc::new(LocalStore::create(&dir.join("store"), seed::repository()?, clock.clone()).await?);
    seed::seed(&store).await?;
    let winding_up = WindingUp::default();
    let url = format!("http://127.0.0.1:{port}/");
    serving::serve(
        &config,
        clock.clone(),
        &dir.join("run"),
        &winding_up,
        |ends| async {
            let starting = Starting::new(&config, &agent, || Ok(SessionToken::Withheld)).await?;
            let harness = starting.harness(beside.join("lw"), clock.clone());
            let factory_config = starting.factory_config(
                &config,
                config.work_dir.join(seed::REPO),
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

/// The `playwright-cli` command that has the browser tell the time `clock`
/// does, as the pages work out how long ago and how long for in the browser.
fn browser_clock(clock: &dyn Clock) -> String {
    let ahead = (clock.now() - chrono::Utc::now()).num_milliseconds();
    format!(
        "playwright-cli run-code \"async page => {{ await page.clock.setSystemTime(Date.now() + ({ahead})); }}\""
    )
}

/// `fake-claude` as `dir/claude`, with the seed's scenario beside it.
fn fake_agent(dir: &Path, fake_claude: &Path) -> Result<String> {
    let program = dir.join("claude");
    std::os::unix::fs::symlink(fake_claude, &program)?;
    std::fs::write(
        dir.join("claude.json"),
        serde_json::to_string(&seed::scenario())?,
    )?;
    program
        .into_os_string()
        .into_string()
        .map_err(|path| anyhow::anyhow!("{} is not UTF-8", PathBuf::from(path).display()))
}

/// Serving the seed's repository from `dir` on `port`, each stage's sessions
/// told their stage, as `fake-claude` takes its steps by it.
fn config(dir: &Path, port: u16) -> Result<Config> {
    let mut text = format!(
        "repo = \"{repo}\"\nwork-dir = \"{work}\"\nui-address = \"127.0.0.1:{port}\"\n\
         poll-interval = \"1s\"\n",
        repo = seed::REPO,
        work = dir.join("work").display(),
    );
    for stage in WorkKind::ALL {
        let name = stage.name();
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
    Config::resolve(&settings, dir, &|_| None, false).map_err(|e| anyhow::anyhow!("{e}"))
}

/// A port of `127.0.0.1` nothing listens on.
fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}
