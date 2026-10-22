//! The web UI (`docs/specs/design_system.md`, The web UI): pages rendered on
//! the server, each kept up by one event stream of the regions that changed.

mod automations_page;
mod board_page;
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
use std::num::NonZeroU64;
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
use crate::domain::{Repo, Timestamp};
use crate::phrase::Phrase;
use crate::runs::{Outcome, RunId};
use crate::status::{self, DrainShown, FailedPolls, Linewright, UnparsedAutomation};

pub(crate) fn routes() -> Router<Sources> {
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
}

/// Has the browser run only the scripts Linewright serves, and show only
/// its own images and the repository owner's GitHub avatar, which
/// `github.com` redirects to `avatars.githubusercontent.com`.
async fn confined(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' https://github.com https://avatars.githubusercontent.com",
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
    Repo,
    Board,
    Runs,
    Automations,
    Server,
}

impl NavIcon {
    /// Its classes. In the rail, the repository's owner's avatar stands in
    /// for the repository's icon (`docs/specs/web/design_system.md`,
    /// Navigation).
    fn classes(self) -> &'static str {
        match self {
            NavIcon::Repo => "size-5 shrink-0 lg:max-2xl:hidden",
            NavIcon::Board | NavIcon::Runs | NavIcon::Automations | NavIcon::Server => {
                "size-5 shrink-0"
            }
        }
    }
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
    /// Polls failed, with none succeeded since.
    PollsFailed(FailedPolls),
    UsageLimit {
        resets_at: Timestamp,
    },
    Drain(DrainShown),
    /// A webhook delivery refused, with none accepted since.
    WebhooksRefused {
        last_refused: Timestamp,
    },
    UnparsedAutomation(UnparsedAutomation),
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
        [failed_polls, usage_limit, drain, refused]
            .into_iter()
            .flatten()
            .chain(unparsed)
            .chain(failed)
            .collect()
    }

    fn phrase(&self) -> Phrase {
        match self {
            Hold::PollsFailed(failed) if failed.count == NonZeroU64::MIN => {
                Phrase::from("poll failed: ").time(failed.since)
            }
            Hold::PollsFailed(failed) => {
                Phrase::from(format!("polls failed: {} since ", failed.count)).time(failed.since)
            }
            Hold::UsageLimit { resets_at } => status::usage_limit(*resets_at),
            Hold::Drain(drain) => status::drain_under_way(*drain),
            Hold::WebhooksRefused { last_refused } => Phrase::from("webhook delivery refused: ")
                .time(*last_refused)
                .text(", none accepted since"),
            Hold::UnparsedAutomation(file) => status::does_not_parse(file),
            Hold::AutomationFailed { name, run, outcome } => {
                format!("automation {name}: last run {run} {}", outcome.word()).into()
            }
        }
    }

    /// Its alert's classes, in its daisyUI colour
    /// (`docs/specs/web/design_system.md`, Status): a usage limit is
    /// neutral, which is daisyUI's alert with no colour of its own. Written
    /// whole, as `Tone::colour` says why.
    fn alert(&self) -> &'static str {
        match self {
            Hold::UsageLimit { .. } => "alert",
            Hold::Drain(_) | Hold::WebhooksRefused { .. } => "alert alert-warning",
            Hold::PollsFailed(_) | Hold::UnparsedAutomation(_) | Hold::AutomationFailed { .. } => {
                "alert alert-error"
            }
        }
    }

    /// The page that tells more of it.
    fn href(&self) -> String {
        match self {
            Hold::PollsFailed(_)
            | Hold::UsageLimit { .. }
            | Hold::Drain(_)
            | Hold::WebhooksRefused { .. } => "/server".into(),
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
                alert: hold.alert(),
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
    Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response()
}
