//! The web UI (`docs/specs/monitoring.md`, The web UI): pages rendered on
//! the server, each kept up by one event stream of the regions that changed.

mod board_page;
mod html;
mod item_page;
mod run_page;
mod runs_page;

use html::Html;

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::Write as _;
use std::sync::LazyLock;
use std::time::Duration;

use askama::Template;
use askama_web::WebTemplate;
use axum::Router;
use axum::extract::{Path, Request};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rust_embed::Embed;
use sha2::Digest as _;
use tokio_util::sync::CancellationToken;

use crate::api::{self, Sources, Unanswered};
use crate::phrase::{Phrase, Tone};
use crate::status::{self, Fact};

pub(crate) fn routes() -> Router<Sources> {
    Router::new()
        .route("/", get(board_page::board_page))
        .route("/events/", get(board_page::board_events))
        .route("/items/{number}", get(item_page::item_page))
        .route("/events/items/{number}", get(item_page::item_events))
        .route("/runs", get(runs_page::runs_page))
        .route("/events/runs", get(runs_page::runs_events))
        .route("/runs/{*id}", get(run_page::run_page))
        .route("/events/runs/{*id}", get(run_page::run_events))
        .route("/assets/{name}", get(asset))
        .layer(middleware::from_fn(confined))
}

/// Has the browser run only the scripts Linewright serves.
async fn confined(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'self'"),
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
            assets: PageAssets {
                css: path("linewright.css"),
                script: path("linewright.js"),
                htmx: path("htmx.min.js"),
                sse: path("hx-sse.js"),
            },
        }
    }
}

/// The frame every page has around its own regions
/// (`docs/specs/monitoring.md`, Navigation, and Holds and problems).
struct Frame {
    repo: String,
    facts: Vec<FrameFact>,
    /// A usage limit in force and a drain under way.
    banners: Vec<Html>,
}

struct FrameFact {
    html: Html,
    problem: bool,
}

impl Frame {
    fn of(sources: &Sources) -> Frame {
        let repo = sources.work_dir.repo.clone();
        let status = api::status_answer(sources);
        let html = |phrase: &Phrase| html::phrase(phrase, &repo);
        let mut facts = vec![];
        let mut banners = vec![];
        for fact in status::linewright_facts(&status.linewright)
            .into_iter()
            .chain(status::automation_facts(&status))
        {
            match fact {
                Fact::Plain(phrase) => facts.push(FrameFact {
                    html: html(&phrase),
                    problem: false,
                }),
                Fact::Problem(phrase) => facts.push(FrameFact {
                    html: html(&phrase),
                    problem: true,
                }),
                Fact::Warning(phrase) => banners.push(html(&phrase)),
            }
        }
        Frame {
            repo,
            facts,
            banners,
        }
    }
}

#[derive(Template)]
#[template(path = "layout.html", block = "frame")]
struct FrameRegion<'a> {
    frame: &'a Frame,
}

/// The frame's regions, which every page's stream sends before its own.
fn frame_regions(frame: &Frame) -> Vec<(&'static str, String)> {
    vec![("bar", render(&FrameRegion { frame }))]
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

/// How often a stream looks for what changed, as a run's activity changes
/// with nothing sent on `serve`'s view.
const SAMPLED_EVERY: Duration = Duration::from_secs(1);

fn samples() -> tokio::time::Interval {
    let mut samples = tokio::time::interval(SAMPLED_EVERY);
    samples.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    samples
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
    Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response()
}
