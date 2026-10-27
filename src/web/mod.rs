//! The web UI (`docs/specs/design_system.md`, The web UI): pages rendered on
//! the server, each kept up by one event stream of the regions that changed.

mod automations_page;
mod board_page;
mod cards;
mod facts;
mod html;
mod item_page;
mod run_page;
mod runs_page;
mod runs_table;
mod server_page;

use automations_page::{Automations, LastRun};
use html::Html;

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::LazyLock;
use std::time::Duration;

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use rust_embed::Embed;
use sha2::Digest as _;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::api::{self, Sources, Unanswered};
use crate::domain::{Repo, Timestamp};
use crate::host::{HostName, UiHosts};
use crate::listening::{Address, Listening};
use crate::phrase::{Phrase, Tone};
use crate::repo_files::FileError;
use crate::runs::{Outcome, RunId};
use crate::status::{self, DrainShown, FailedPolls, Linewright, UnparsedAutomation};

/// Where the web UI is served and the names it answers besides the local
/// ones and IP addresses (`docs/specs/design_system.md`, Serving).
#[derive(Clone, Debug)]
pub struct Ui {
    pub address: Address,
    pub hosts: UiHosts,
}

/// The web UI listening on its port, until [`Listener::stop`].
pub struct Listener {
    address: SocketAddr,
    /// Ends the event streams.
    stopping: CancellationToken,
    listening: Listening,
}

impl Listener {
    /// Serves the web UI on `ui`'s address, with an error naming
    /// `ui-address` when it cannot.
    pub async fn bind(ui: &Ui, sources: Sources) -> anyhow::Result<Listener> {
        let socket = ui.address.bind("ui-address").await?;
        let address = socket.local_addr()?;
        let stopping = CancellationToken::new();
        let router = router(sources, ui.hosts.clone(), stopping.clone());
        let listening = Listening::spawn(socket, router, "serving the web UI");
        info!(address = %address, "serving the web UI");
        Ok(Listener {
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

/// The web UI: its pages, their event streams and their assets, answering
/// only the requests whose `Host` `hosts` answers. `stopping` ends the event
/// streams, so that stopping waits only for the answers being given.
pub fn router(sources: Sources, hosts: UiHosts, stopping: CancellationToken) -> Router {
    Router::new()
        .route("/", get(board_page::board_page))
        .route("/events/", get(board_page::board_events))
        .route("/items/{number}", get(item_page::item_page))
        .route("/events/items/{number}", get(item_page::item_events))
        .route("/runs", get(runs_page::runs_page))
        .route("/events/runs", get(runs_page::runs_events))
        .route("/automations", get(automations_page::automations_page))
        .route(
            "/events/automations",
            get(automations_page::automations_events),
        )
        .route("/server", get(server_page::server_page))
        .route("/events/server", get(server_page::server_events))
        .route("/runs/{*id}", get(run_page::run_page))
        .route("/events/runs/{*id}", get(run_page::run_events))
        .route("/assets/{name}", get(asset))
        .layer(middleware::from_fn(confined))
        .fallback(|| async { (StatusCode::NOT_FOUND, "no such page") })
        .layer(middleware::from_fn_with_state(hosts, misdirected))
        .layer(Extension(stopping))
        .with_state(sources)
}

/// Refuses a request whose `Host` is not local, an IP address or one of
/// `ui-hosts`, such as one a web page sends under a name it controls that
/// it has pointed at the UI's address.
async fn misdirected(State(hosts): State<UiHosts>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let reason = match HostName::of(headers) {
        Some(host) if hosts.answers(&host) => return next.run(request).await,
        Some(_) => "its Host is not local, an IP address or one of `ui-hosts`",
        None if headers.contains_key(header::HOST) => "its Host cannot be read",
        None => "it has no Host",
    };
    let host = headers.get(header::HOST);
    warn!(
        host = %host.map_or("none".into(), |host| String::from_utf8_lossy(host.as_bytes())),
        reason,
        "refused a request for the web UI"
    );
    (
        StatusCode::MISDIRECTED_REQUEST,
        "this name is not one the web UI answers to",
    )
        .into_response()
}

/// Has the browser run only the scripts Linewright serves, and show only
/// its own images, the repository owner's GitHub avatar, which
/// `github.com` redirects to `avatars.githubusercontent.com`, and `data:`
/// images, which daisyUI's CSS draws some of its parts from (the tooltip's
/// tail, the noise texture).
async fn confined(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data: https://github.com https://avatars.githubusercontent.com",
        ),
    );
    response
}

#[derive(Embed)]
#[folder = "assets/"]
#[include = "*.css"]
#[include = "*.js"]
struct Assets;

/// Each asset under a name holding a hash of its content, so that a
/// browser may keep it for good: a changed asset has a new name.
struct Hashed {
    /// By the name the templates give.
    names: HashMap<&'static str, String>,
    /// The name and content type each hashed name stands for.
    files: HashMap<String, (&'static str, &'static str)>,
}

const CSS: &str = "text/css; charset=utf-8";
const JAVASCRIPT: &str = "text/javascript; charset=utf-8";
const ASSETS: [(&str, &str); 4] = [
    ("linewright.css", CSS),
    ("linewright.js", JAVASCRIPT),
    ("htmx.min.js", JAVASCRIPT),
    ("hx-sse.js", JAVASCRIPT),
];

static HASHED: LazyLock<Hashed> = LazyLock::new(|| {
    let mut hashed = Hashed {
        names: HashMap::new(),
        files: HashMap::new(),
    };
    for (name, content_type) in ASSETS {
        let file = Assets::get(name).expect("the assets are embedded");
        let digest = hex::encode(sha2::Sha256::digest(&file.data));
        let (stem, extension) = name.split_once('.').expect("an asset has an extension");
        let path = format!("{stem}-{}.{extension}", &digest[..16]);
        hashed.files.insert(path.clone(), (name, content_type));
        hashed.names.insert(name, format!("/assets/{path}"));
    }
    hashed
});

async fn asset(Path(name): Path<String>) -> Response {
    let Some((file, content_type)) =
        HASHED
            .files
            .get(&name)
            .and_then(|(original, content_type)| {
                Assets::get(original).map(|file| (file, *content_type))
            })
    else {
        return (StatusCode::NOT_FOUND, "no such asset").into_response();
    };
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        file.data,
    )
        .into_response()
}

/// What every page's layout holds.
struct Page {
    title: String,
    /// The path of the page's event stream.
    events: String,
    /// Whether the page is as tall as the viewport, its `main` a flex column
    /// whose `.scroll-box` scrolls rather than the page.
    fills_viewport: bool,
    assets: PageAssets,
}

struct PageAssets {
    css: &'static str,
    script: &'static str,
    htmx: &'static str,
    /// htmx's extension that keeps the page up from its event stream.
    sse: &'static str,
}

impl Page {
    fn new(title: String, events: String) -> Page {
        let path = |name| HASHED.names[name].as_str();
        Page {
            title,
            events,
            fills_viewport: false,
            assets: PageAssets {
                css: path("linewright.css"),
                script: path("linewright.js"),
                htmx: path("htmx.min.js"),
                sse: path("hx-sse.js"),
            },
        }
    }

    fn filling_the_viewport(self) -> Page {
        Page {
            fills_viewport: true,
            ..self
        }
    }
}

/// The icon of a link in the navigation, drawn by `macros.html`'s
/// `nav_icon`.
#[derive(Clone, Copy)]
enum NavIcon {
    Board,
    Runs,
    Automations,
    Server,
}

/// The frame every page has around its own regions
/// (`docs/specs/design_system.md`, Navigation, and Holds and problems).
struct Frame {
    repo: Repo,
    holds: Vec<HoldShown>,
}

/// A hold on Linewright's work, shown on every page while it lasts
/// (`docs/specs/design_system.md`, Holds and problems).
enum Hold {
    UsageLimit {
        resets_at: Timestamp,
    },
    Drain(DrainShown),
    /// A webhook delivery refused, with none accepted since.
    WebhooksRefused {
        last_refused: Timestamp,
    },
    /// Polls failed, with none succeeded since.
    PollsFailed(FailedPolls),
    UnparsedAutomation(UnparsedAutomation),
    /// A file of the repository's Linewright cannot use.
    UnusableFile(FileError),
    /// An automation whose last run failed.
    AutomationFailed {
        name: String,
        run: RunId,
        outcome: Outcome,
    },
}

impl Hold {
    fn of(linewright: &Linewright, automations: Option<&Automations>) -> Vec<Hold> {
        let failed_polls = linewright.failed_polls.clone().map(Hold::PollsFailed);
        let usage_limit = linewright
            .usage_limit_resets_at
            .map(|resets_at| Hold::UsageLimit { resets_at });
        let drain = linewright.drain.map(Hold::Drain);
        let refused = linewright
            .webhooks
            .and_then(|received| received.refused_since_accepted())
            .map(|last_refused| Hold::WebhooksRefused { last_refused });
        let unparsed = automations
            .into_iter()
            .flat_map(|automations| &automations.unparsed)
            .map(|file| Hold::UnparsedAutomation(file.clone()));
        let unusable = linewright
            .unusable_files
            .iter()
            .map(|file| Hold::UnusableFile(file.clone()));
        let failed = automations
            .into_iter()
            .flat_map(|automations| &automations.rows)
            .filter_map(|row| match &row.last_run {
                LastRun::Run(run) if run.outcome.failed() => Some(Hold::AutomationFailed {
                    name: row.name.clone(),
                    run: run.id.clone(),
                    outcome: run.outcome,
                }),
                LastRun::Run(_) | LastRun::NoneYet { .. } => None,
            });
        [usage_limit, drain, refused, failed_polls]
            .into_iter()
            .flatten()
            .chain(unparsed)
            .chain(unusable)
            .chain(failed)
            .collect()
    }

    fn phrase(&self) -> Phrase {
        match self {
            Hold::PollsFailed(failed) => status::polls_failed_since(failed),
            Hold::UsageLimit { resets_at } => status::usage_limit(*resets_at),
            Hold::Drain(drain) => status::drain_under_way(*drain),
            Hold::WebhooksRefused { last_refused } => Phrase::from("webhook delivery refused: ")
                .time(*last_refused)
                .text(", none accepted since"),
            Hold::UnparsedAutomation(file) => status::does_not_parse(file),
            Hold::UnusableFile(file) => status::unusable_file(file),
            Hold::AutomationFailed { name, run, outcome } => {
                format!("automation {name}: last run {run} {}", outcome.word()).into()
            }
        }
    }

    /// Its tone (`docs/specs/web/design_system.md`, Status): a usage limit
    /// shows neutral, the tone it shares with a cancelled run.
    fn tone(&self) -> Tone {
        match self {
            Hold::UsageLimit { .. } => Tone::Cancelled,
            Hold::Drain(_) => Tone::InProgress,
            Hold::WebhooksRefused { .. } => Tone::Pending,
            Hold::PollsFailed(_)
            | Hold::UnparsedAutomation(_)
            | Hold::UnusableFile(_)
            | Hold::AutomationFailed { .. } => Tone::Failure,
        }
    }

    /// The page that tells more of it.
    fn href(&self) -> String {
        match self {
            Hold::PollsFailed(_)
            | Hold::UsageLimit { .. }
            | Hold::Drain(_)
            | Hold::WebhooksRefused { .. }
            | Hold::UnusableFile(_) => "/server".into(),
            Hold::UnparsedAutomation(_) => "/automations".into(),
            Hold::AutomationFailed { run, .. } => run_path(run),
        }
    }
}

struct HoldShown {
    alert: &'static str,
    phrase: Html,
    href: String,
}

/// What the frame shows, from one read of `serve`'s view and the runs.
struct Read {
    /// What `status` says of Linewright.
    linewright: Linewright,
    /// `None` before the first poll.
    automations: Option<Automations>,
}

impl Read {
    fn of(sources: &Sources) -> Read {
        let view = api::view(sources);
        let recorded = api::recorded_or_none(sources);
        let now = sources.clock.now();
        let status = api::status_of(sources, view.as_ref(), &recorded);
        Read {
            automations: view
                .as_ref()
                .map(|view| Automations::of(&status, view, &recorded, now)),
            linewright: status.linewright,
        }
    }
}

impl Frame {
    fn of(sources: &Sources) -> Frame {
        Frame::of_read(sources, &Read::of(sources))
    }

    fn of_read(sources: &Sources, read: &Read) -> Frame {
        let linewright = &read.linewright;
        let repo = sources.work_dir.repo.clone();
        let html = |phrase: &Phrase| html::phrase(phrase, &repo);
        let holds = Hold::of(linewright, read.automations.as_ref())
            .iter()
            .map(|hold| HoldShown {
                alert: hold.tone().alert(),
                phrase: html(&hold.phrase()),
                href: hold.href(),
            })
            .collect();
        Frame { repo, holds }
    }
}

#[derive(Template)]
#[template(path = "layout.html", block = "holds")]
struct HoldsRegion<'a> {
    frame: &'a Frame,
}

/// The frame's regions, which every page's stream sends before its own.
fn frame_regions(frame: &Frame) -> Vec<(&'static str, String)> {
    vec![("holds", render(&HoldsRegion { frame }))]
}

#[derive(Template, WebTemplate)]
#[template(path = "error.html")]
struct ErrorPage {
    page: Page,
    frame: Frame,
    /// The id of the page's region the error stands in for.
    region: &'static str,
    error: String,
}

/// How often a stream looks for what changed, as what its regions read on
/// disk, such as run records, changes with nothing sent on `serve`'s view.
const SAMPLED_EVERY: Duration = Duration::from_secs(1);

fn samples() -> tokio::time::Interval {
    let mut samples = tokio::time::interval(SAMPLED_EVERY);
    samples.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    samples
}

/// The path of run `id`'s page.
fn run_path(id: &RunId) -> String {
    format!("/runs/{id}")
}

/// `path` with `query`, if there is one.
fn with_query(path: &str, query: &str) -> String {
    if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    }
}

/// What a page says in place of what `serve` did not answer, and the
/// status it answers with.
fn unanswered(e: impl Into<Unanswered>) -> (StatusCode, String) {
    match e.into() {
        Unanswered::NoSuch { error, .. } => (StatusCode::NOT_FOUND, error),
        Unanswered::Failed(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

fn render(template: &impl Template) -> String {
    template.render().expect("the templates render")
}

/// What a stream last sent of each region.
#[derive(Default)]
struct Sent(HashMap<&'static str, String>);

impl Sent {
    /// The regions of `regions` that changed since they were last sent, as
    /// partials that morph into the page, if any did.
    fn changed(&mut self, regions: Vec<(&'static str, String)>) -> Option<String> {
        let mut message = String::new();
        for (id, region) in regions {
            let settled = html::settled(&region);
            if self.0.get(id) == Some(&settled) {
                continue;
            }
            self.0.insert(id, settled);
            let _ = write!(
                message,
                r#"<hx-partial id="{id}" hx-swap="outerMorph">{region}</hx-partial>"#
            );
        }
        Some(message).filter(|message| !message.is_empty())
    }
}

/// Sends every region `regions` gives at once, then each time one of them
/// changed, each as a partial that morphs into the page.
fn events(
    regions: impl Fn() -> Vec<(&'static str, String)> + Send + 'static,
    stopping: CancellationToken,
) -> Response {
    let events = futures_util::stream::unfold(
        (Sent::default(), samples(), regions, stopping),
        |(mut sent, mut samples, regions, stopping)| async move {
            loop {
                tokio::select! {
                    _ = samples.tick() => {}
                    () = stopping.cancelled() => return None,
                }
                if let Some(message) = sent.changed(regions()) {
                    let event = Ok::<_, Infallible>(Event::default().data(message));
                    return Some((event, (sent, samples, regions, stopping)));
                }
            }
        },
    );
    crate::event_stream::respond(events)
}
