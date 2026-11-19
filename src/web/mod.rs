//! The web UI (`docs/specs/web/architecture.md`, Rendering): pages rendered
//! on the server, each kept up by one event stream of the regions that changed.

mod automations_page;
mod benchmark_page;
mod benchmark_run_page;
mod benchmarks_page;
mod board_page;
mod capture;
mod cards;
mod confirmation;
mod facts;
mod html;
mod item_page;
mod launch;
mod making;
mod names;
mod run_page;
mod runs_page;
mod runs_table;
mod server_page;
mod shell;
mod table;
mod write;

use shell::{Frame, Trail};

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
use axum::routing::{get, post};
use axum::{Extension, Router};
use rust_embed::Embed;
use sha2::Digest as _;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::api::{Sources, Unanswered};
use crate::benchmarks::{BenchmarkName, BenchmarkRunId, BenchmarkRuns, Benchmarks};
use crate::domain::Number;
use crate::host::{HostName, UiHosts};
use crate::listening::{Address, Listening};
use crate::runs::RunId;

/// Where the web UI is served and the names it answers besides the local
/// ones and IP addresses (`docs/specs/operation.md`, Serving the UI).
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
/// streams.
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
        .route(
            "/suites",
            get(benchmarks_page::benchmarks_page).post(benchmarks_page::make_benchmark),
        )
        .route("/events/suites", get(benchmarks_page::benchmarks_events))
        .route(
            "/suites/{name}",
            get(benchmark_page::benchmark_page).post(benchmark_page::add_task),
        )
        .route(
            "/suites/{name}/remove-case",
            post(benchmark_page::remove_task),
        )
        .route("/suites/{name}/launch", post(launch::launch))
        .route(
            "/events/suites/{name}",
            get(benchmark_page::benchmark_events),
        )
        .route(
            "/experiments/{id}",
            get(benchmark_run_page::benchmark_run_page),
        )
        .route(
            "/events/experiments/{id}",
            get(benchmark_run_page::benchmark_run_events),
        )
        .route("/experiments/{id}/cancel", post(benchmark_run_page::cancel))
        .route(
            "/experiments/{id}/{configuration}/{case}/{repetition}/{of}",
            get(benchmark_run_page::trial_page),
        )
        .route(
            "/events/experiments/{id}/{configuration}/{case}/{repetition}/{of}",
            get(benchmark_run_page::trial_events),
        )
        .route("/server", get(server_page::server_page))
        .route("/events/server", get(server_page::server_events))
        .route(
            "/runs/{*id}",
            get(run_page::run_page).post(capture::capture),
        )
        .route("/events/runs/{*id}", get(run_page::run_events))
        .route("/assets/{name}", get(asset))
        .layer(middleware::from_fn(confined))
        .fallback(|| async { (StatusCode::NOT_FOUND, "no such page") })
        .layer(middleware::from_fn_with_state(hosts, misdirected))
        .layer(Extension(stopping))
        .with_state(sources)
}

/// Refuses a request whose `Host` is not local, an IP address or one of
/// `ui-hosts`.
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
/// its own images, `data:` images and those from `github.com` and
/// `avatars.githubusercontent.com`, such as the repository owner's avatar.
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
    /// Where the page is, which its breadcrumbs and its title show.
    trail: Trail,
    /// The count of cards in Needs a person, on Board.
    needing_a_person: Option<usize>,
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
    fn new(trail: Trail, events: String) -> Page {
        let path = |name| HASHED.names[name].as_str();
        Page {
            trail,
            needing_a_person: None,
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

    /// The page, its title led by `count`, of cards in Needs a person
    /// (`docs/specs/board.md`, Board, Needing a person).
    fn needing_a_person(self, count: usize) -> Page {
        Page {
            needing_a_person: Some(count),
            ..self
        }
    }

    /// Its title: its trail's, led by the count of cards in Needs a person
    /// where it has one.
    fn title(&self) -> String {
        let title = self.trail.title();
        match self.needing_a_person {
            Some(count) => format!("({count}) {title}"),
            None => title,
        }
    }

    fn filling_the_viewport(self) -> Page {
        Page {
            fills_viewport: true,
            ..self
        }
    }
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

/// What a link named that does not exist, as the screens name it
/// (`docs/specs/design_system.md`, Screen states, Nothing there). A run,
/// benchmark or benchmark run is named as the path gave it, whether or not it
/// parses.
#[derive(Clone)]
enum Missing {
    Item(Number),
    Run(String),
    Benchmark(String),
    BenchmarkRun(String),
}

impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Missing::Item(number) => write!(f, "No such item {number}"),
            Missing::Run(id) => write!(f, "No such run {id}"),
            Missing::Benchmark(name) => write!(f, "No such suite {name}"),
            Missing::BenchmarkRun(id) => write!(f, "No such experiment {id}"),
        }
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "nothing_there.html")]
struct NothingThere {
    page: Page,
    frame: Frame,
    /// The id of the page's region it stands in for.
    region: &'static str,
    missing: Missing,
}

/// The screen of `missing`, standing in for the page's `region`, answered
/// with 404.
fn nothing_there(page: Page, frame: Frame, region: &'static str, missing: Missing) -> Response {
    let page = NothingThere {
        page,
        frame,
        region,
        missing,
    };
    (StatusCode::NOT_FOUND, page).into_response()
}

/// How often a stream looks for what changed.
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

/// The benchmarks of `serve`'s work directory.
fn benchmarks(sources: &Sources) -> Benchmarks {
    Benchmarks::new(sources.runs.database().clone())
}

/// The benchmark runs of `serve`'s work directory.
fn benchmark_runs(sources: &Sources) -> BenchmarkRuns {
    BenchmarkRuns::new(sources.runs.database().clone())
}

/// The path of benchmark run `id`'s page.
fn benchmark_run_path(id: BenchmarkRunId) -> String {
    format!("/experiments/{id}")
}

/// The path of benchmark `name`'s page.
fn benchmark_path(name: &BenchmarkName) -> String {
    format!("/suites/{name}")
}

/// `path` with `query`, if there is one.
fn with_query(path: &str, query: &str) -> String {
    if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    }
}

/// A filter's field, if it is set: an empty one is unset
/// (`docs/specs/design_system.md`, Filters, Unset).
fn given(field: &str) -> Option<&str> {
    Some(field).filter(|field| !field.is_empty())
}

/// `path` with the query of `filters`, each a name and its field, of those
/// that are set.
fn with_filters(path: &str, filters: &[(&str, &str)]) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    for (name, field) in filters {
        if let Some(value) = given(field) {
            query.append_pair(name, value);
        }
    }
    with_query(path, &query.finish())
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
fn events<Regions>(
    regions: impl Fn() -> Regions + Send + 'static,
    stopping: CancellationToken,
) -> Response
where
    Regions: Future<Output = Vec<(&'static str, String)>> + Send,
{
    let events = futures_util::stream::unfold(
        (Sent::default(), samples(), regions, stopping),
        |(mut sent, mut samples, regions, stopping)| async move {
            loop {
                tokio::select! {
                    _ = samples.tick() => {}
                    () = stopping.cancelled() => return None,
                }
                if let Some(message) = sent.changed(regions().await) {
                    let event = Ok::<_, Infallible>(Event::default().data(message));
                    return Some((event, (sent, samples, regions, stopping)));
                }
            }
        },
    );
    crate::event_stream::respond(events)
}
