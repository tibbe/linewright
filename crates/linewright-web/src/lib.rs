//! The web UI (`docs/specs/architecture.md`, The web UI, Rendering and Live
//! updates): pages rendered
//! on the server, each kept up by one event stream of the regions that changed.

mod authorization_server;
pub(crate) mod authorizations_page;
mod automations_page;
mod benchmark_page;
mod benchmark_run_page;
mod benchmarks_page;
mod board_page;
mod buttons;
mod caller;
mod capture;
mod card_page;
mod cards;
mod confirmation;
mod editing;
mod facts;
pub(crate) mod html;
mod icons;
mod item_page;
pub(crate) mod launch;
mod making;
mod mcp;
mod names;
mod pages;
mod people_page;
mod run_page;
mod runs_page;
mod runs_table;
mod server_page;
mod shell;
mod sign_in;
mod stats;
mod table;
mod task_page;
mod write;

use benchmark_page::BenchmarkTab;
use buttons::ScreenButton;
use shell::{Frame, Missing, Navigation, Place, Thing};

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::Write as _;
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
use tracing::warn;

use linewright_core::access::oauth;
use linewright_core::api::{Sources, Unanswered};
use linewright_core::benchmarks::{BenchmarkId, BenchmarkRunId, TaskId};
use linewright_core::config::Ui;
use linewright_core::host::{HostName, PublicUrl};

/// The web UI: its pages, their event streams and their assets, answering
/// only the requests whose `Host` is `ui`'s public URL's host, and signing
/// people in as `ui` says. A screen opened without a sign-in goes to Sign
/// in. `stopping` ends the event streams.
pub fn router(sources: Sources, ui: &Ui, stopping: CancellationToken) -> Router {
    let signed = Router::new()
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
            "/benchmarks",
            get(benchmarks_page::benchmarks_page).post(benchmarks_page::make_benchmark),
        )
        .route(
            "/events/benchmarks",
            get(benchmarks_page::benchmarks_events),
        )
        .route(
            "/benchmarks/{id}",
            get(benchmark_page::benchmark_page).post(benchmark_page::add_task),
        )
        .route(
            "/benchmarks/{id}/{tab}",
            get(benchmark_page::benchmark_tab_page),
        )
        .route(
            "/benchmarks/{id}/remove-task",
            get(no_such_page).post(benchmark_page::remove_task),
        )
        .route(
            "/benchmarks/{id}/edit",
            get(no_such_page).post(editing::save_benchmark),
        )
        .route(
            "/benchmarks/{id}/launch",
            get(no_such_page).post(launch::launch),
        )
        .route(
            "/events/benchmarks/{id}",
            get(benchmark_page::benchmark_events),
        )
        .route("/tasks/{id}", get(task_page::task_page))
        .route(
            "/tasks/{id}/edit",
            get(no_such_page).post(editing::save_task),
        )
        .route("/events/tasks/{id}", get(task_page::task_events))
        .route(
            "/benchmark-runs/{id}",
            get(benchmark_run_page::benchmark_run_page),
        )
        .route(
            "/events/benchmark-runs/{id}",
            get(benchmark_run_page::benchmark_run_events),
        )
        .route(
            "/benchmark-runs/{id}/configurations/{configuration}",
            get(no_such_page).post(editing::save_configuration),
        )
        .route(
            "/benchmark-runs/{id}/cancel",
            post(benchmark_run_page::cancel),
        )
        .route(
            "/benchmark-runs/{id}/{configuration}/{task}/{repetition}/{of}",
            get(benchmark_run_page::trial_page),
        )
        .route(
            "/events/benchmark-runs/{id}/{configuration}/{task}/{repetition}/{of}",
            get(benchmark_run_page::trial_events),
        )
        .route(
            authorizations_page::PATH,
            get(authorizations_page::authorizations_page),
        )
        .route(
            "/events/authorizations",
            get(authorizations_page::authorizations_events),
        )
        .route(
            "/authorizations/{id}/end",
            get(no_such_page).post(authorizations_page::end),
        )
        .route("/people", get(people_page::people_page))
        .route("/events/people", get(people_page::people_events))
        .route(
            "/people/{id}/edit",
            get(no_such_page).post(people_page::save_person),
        )
        .route(
            "/people/{id}/disable",
            get(no_such_page).post(people_page::disable),
        )
        .route(
            "/people/{id}/enable",
            get(no_such_page).post(people_page::enable),
        )
        .route("/server", get(server_page::server_page))
        .route("/events/server", get(server_page::server_events))
        .route(
            "/runs/{*id}",
            get(run_page::run_page).post(capture::capture),
        )
        .route("/events/runs/{*id}", get(run_page::run_events))
        .route(Navigation::PATH, post(shell::remember_navigation))
        .route_layer(middleware::from_fn_with_state(
            sources.clone(),
            caller::signed_in,
        ));
    let open = Router::new()
        .route(sign_in::SIGN_IN_PATH, get(sign_in::sign_in_page))
        .route(sign_in::SIGN_OUT_PATH, post(sign_in::sign_out))
        .route(sign_in::BEGIN_PATH, get(no_such_page).post(sign_in::begin))
        .route(sign_in::CALLBACK_PATH, get(sign_in::callback))
        .route(
            oauth::SERVER_METADATA_PATH,
            get(authorization_server::server_metadata),
        )
        .route(
            oauth::RESOURCE_METADATA_PATH,
            get(authorization_server::resource_metadata),
        )
        .route(oauth::REGISTER_PATH, post(authorization_server::register))
        .route(
            oauth::AUTHORIZE_PATH,
            get(authorization_server::authorize).post(authorization_server::decide),
        )
        .route(oauth::TOKEN_PATH, post(authorization_server::token))
        .route("/assets/{name}", get(asset));
    let sign_in = sign_in::SignIn {
        public_url: ui.public_url.clone(),
        providers: ui.providers.clone(),
        manager: ui.manager.clone(),
        http: linewright_core::http::client_builder_trusting_the_host_store_alone()
            .build()
            .expect("an HTTP client builds"),
    };
    let server = oauth::Server::new(ui.public_url.clone());
    let mcp = crate::mcp::router(sources.clone(), &server, ui.manager.clone(), &ui.public_url);
    signed
        .merge(open)
        .layer(middleware::from_fn(caller::cross_site))
        .merge(mcp)
        .layer(middleware::from_fn(confined))
        .fallback(no_such_page)
        .layer(middleware::from_fn_with_state(
            ui.public_url.clone(),
            misdirected,
        ))
        .layer(Extension(people_page::Named(ui.manager.clone())))
        .layer(Extension(std::sync::Arc::new(sign_in)))
        .layer(Extension(std::sync::Arc::new(server)))
        .layer(Extension(stopping))
        .with_state(sources)
}

/// Refuses a request whose `Host` is not the public URL's host.
async fn misdirected(
    State(public_url): State<PublicUrl>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let reason = match HostName::of(headers) {
        Some(host) if public_url.answers(&host) => return next.run(request).await,
        Some(_) => "its Host is not the host of `public-url`",
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
/// its own images, `data:` images, those from `github.com` and
/// `avatars.githubusercontent.com`, such as the repository owner's avatar,
/// and Google's avatars, from `googleusercontent.com`; and draw no page
/// of the web UI in another's frame.
async fn confined(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data: https://github.com https://avatars.githubusercontent.com https://*.googleusercontent.com; frame-ancestors 'none'",
        ),
    );
    response
}

#[derive(Embed)]
#[folder = "assets/"]
#[include = "*.css"]
#[include = "*.js"]
#[include = "*.svg"]
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
const SVG: &str = "image/svg+xml";
const ASSETS: [(&str, &str); 6] = [
    ("linewright.css", CSS),
    ("linewright.js", JAVASCRIPT),
    ("htmx.min.js", JAVASCRIPT),
    ("hx-sse.js", JAVASCRIPT),
    ("google-sign-in-light.svg", SVG),
    ("google-sign-in-dark.svg", SVG),
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
    /// Where the page is, which its breadcrumbs, its heading and its title
    /// show.
    place: Place,
    /// The screen's buttons, in order.
    buttons: Vec<ScreenButton>,
    /// The count of cards in Needs a person, on Board.
    needing_a_person: Option<usize>,
    /// The path of the page's event stream.
    events: String,
    /// Whether the page is as tall as the viewport, its `main` a flex column
    /// whose `.scroll-box` scrolls rather than the page.
    fills_viewport: bool,
    /// The navigation it draws from Tailwind's `lg` width.
    navigation: Navigation,
    /// What the screen says under its heading of what was made or removed
    /// (`docs/specs/ui_patterns.md`, Making, What was made, and Confirming,
    /// What was removed), as it opens.
    said: Option<html::Html>,
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
    fn new(place: Place, events: String, navigation: Navigation) -> Page {
        let path = |name| HASHED.names[name].as_str();
        Page {
            place,
            buttons: vec![],
            needing_a_person: None,
            events,
            fills_viewport: false,
            navigation,
            said: None,
            assets: PageAssets {
                css: path("linewright.css"),
                script: path("linewright.js"),
                htmx: path("htmx.min.js"),
                sse: path("hx-sse.js"),
            },
        }
    }

    /// The page saying `said` under its heading, as a success alert.
    fn saying(self, said: &html::Html) -> Page {
        Page {
            said: Some(making::success(said)),
            ..self
        }
    }

    /// The page with the screen's buttons `buttons`, in order.
    fn buttons(self, buttons: Vec<ScreenButton>) -> Page {
        Page { buttons, ..self }
    }

    /// The page, its title led by `count`, of cards in Needs a person
    /// (`docs/specs/board.md`, Board, Needing a person).
    fn needing_a_person(self, count: usize) -> Page {
        Page {
            needing_a_person: Some(count),
            ..self
        }
    }

    /// Its title: its place's, led by the count of cards in Needs a person
    /// where it has one.
    fn title(&self) -> String {
        let title = self.place.title();
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

#[derive(Template, WebTemplate)]
#[template(path = "nothing_there.html")]
struct NothingThere {
    page: Page,
    frame: Frame,
    /// The id of the page's region it stands in for.
    region: &'static str,
    missing: Missing,
}

/// The screen of `missing`, the page `page` gives of it, standing in for
/// the page's `region`, answered with 404.
fn nothing_there(
    page: impl FnOnce(Thing) -> Page,
    missing: Missing,
    frame: Frame,
    region: &'static str,
) -> Response {
    let page = NothingThere {
        page: page(Thing::Missing(missing.clone())),
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

/// The path of run `id`'s page, each character of `id` that a path
/// segment cannot hold as it is percent-encoded, so that the router's
/// `{*id}` decodes it back to `id`.
fn run_path(id: impl std::fmt::Display) -> String {
    format!(
        "/runs/{}",
        percent_encoding::utf8_percent_encode(&id.to_string(), NOT_IN_A_PATH)
    )
}

/// What a path segment cannot hold as it is, and `%`, which starts an
/// encoded one. <https://url.spec.whatwg.org/#path-percent-encode-set>
const NOT_IN_A_PATH: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// The path of benchmark run `id`'s page.
fn benchmark_run_path(id: BenchmarkRunId) -> String {
    format!("/benchmark-runs/{id}")
}

/// The path of task `id`'s page.
fn task_path(id: TaskId) -> String {
    format!("/tasks/{id}")
}

/// The path of benchmark `id`'s page.
fn benchmark_path(id: BenchmarkId) -> String {
    format!("/benchmarks/{id}")
}

/// The path of tab `tab` of benchmark `id`'s page
/// (`docs/specs/ui_patterns.md`, Tabs, Addresses).
fn benchmark_tab_path(id: BenchmarkId, tab: BenchmarkTab) -> String {
    format!("{}/{}", benchmark_path(id), tab.segment())
}

/// The `Cookie` headers of a request's `headers`.
fn cookie_headers(headers: &axum::http::HeaderMap) -> impl Iterator<Item = String> + '_ {
    headers
        .get_all(header::COOKIE)
        .iter()
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
}

/// The value of each cookie named `name` that the `Cookie` headers `cookies`
/// hold, in order. <https://www.rfc-editor.org/rfc/rfc6265#section-5.4>
fn cookie_values(cookies: impl IntoIterator<Item = impl AsRef<str>>, name: &str) -> Vec<String> {
    cookies
        .into_iter()
        .flat_map(|header| {
            header
                .as_ref()
                .split(';')
                .filter_map(|cookie| cookie.trim().split_once('='))
                .filter(|(named, _)| *named == name)
                .map(|(_, value)| value.to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The answer to a path that is no page's.
async fn no_such_page() -> Response {
    (StatusCode::NOT_FOUND, "no such page").into_response()
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
/// (`docs/specs/ui_patterns.md`, Filters, Unset).
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
/// changed, each as a partial that morphs into the page, until the reader's
/// browser no longer signs anyone in, which it says with
/// [`caller::signed_out`], or its sign-in does not read.
fn events<Regions>(regions: impl Fn() -> Regions + Send + 'static, watch: caller::Watch) -> Response
where
    Regions: Future<Output = Vec<(&'static str, String)>> + Send,
{
    let events = futures_util::stream::unfold(
        (Sent::default(), samples(), regions, watch, false),
        |(mut sent, mut samples, regions, watch, signed_out)| async move {
            if signed_out {
                return None;
            }
            loop {
                tokio::select! {
                    _ = samples.tick() => {}
                    () = watch.stopping().cancelled() => return None,
                }
                match watch.signed().await {
                    caller::Signed::In => {}
                    caller::Signed::Out => {
                        let event = Ok::<_, Infallible>(caller::signed_out());
                        return Some((event, (sent, samples, regions, watch, true)));
                    }
                    caller::Signed::Unread => return None,
                }
                if let Some(message) = sent.changed(regions().await) {
                    let event = Ok(Event::default().data(message));
                    return Some((event, (sent, samples, regions, watch, false)));
                }
            }
        },
    );
    linewright_core::event_stream::respond(events)
}
