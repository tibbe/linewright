//! The web UI (`docs/specs/monitoring.md`, The web UI): pages rendered on
//! the server, each kept up by one event stream of the regions that changed.

mod html;
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
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use rust_embed::Embed;
use sha2::Digest as _;
use tokio_util::sync::CancellationToken;

use crate::api::{self, Sources, Unanswered, Unexplained};
use crate::board::{Board, Card, CardLine, Column, Ended};
use crate::domain::Number;
use crate::explain::{self, Line};
use crate::phrase::{Phrase, Tone};
use crate::runs;
use crate::status::{self, Fact};

pub(crate) fn routes() -> Router<Sources> {
    Router::new()
        .route("/", get(board_page))
        .route("/events/", get(board_events))
        .route("/items/{number}", get(item_page))
        .route("/events/items/{number}", get(item_events))
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
const ASSETS: [(&str, &str); 3] = [
    ("linewright.css", CSS),
    ("linewright.js", JAVASCRIPT),
    ("htmx.min.js", JAVASCRIPT),
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
            },
        }
    }
}

/// The bar every page has (`docs/specs/monitoring.md`, The bar).
struct Bar {
    repo: String,
    facts: Vec<BarFact>,
    /// A usage limit in force and a drain under way.
    banners: Vec<Html>,
}

struct BarFact {
    html: Html,
    problem: bool,
}

impl Bar {
    fn of(sources: &Sources) -> Bar {
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
                Fact::Plain(phrase) => facts.push(BarFact {
                    html: html(&phrase),
                    problem: false,
                }),
                Fact::Problem(phrase) => facts.push(BarFact {
                    html: html(&phrase),
                    problem: true,
                }),
                Fact::Warning(phrase) => banners.push(html(&phrase)),
            }
        }
        Bar {
            repo,
            facts,
            banners,
        }
    }
}

#[derive(Template)]
#[template(path = "layout.html", block = "bar")]
struct BarRegion<'a> {
    bar: &'a Bar,
}

/// The board (`docs/specs/monitoring.md`, The board), ready for the page.
struct BoardShown {
    columns: [&'static str; Column::ALL.len()],
    lanes: Vec<LaneShown>,
}

struct LaneShown {
    /// The tracking issue and its tickets closed of total.
    head: Option<Html>,
    cells: Vec<CellShown>,
}

struct CellShown {
    column: &'static str,
    cards: Vec<CardShown>,
}

/// A card (`docs/specs/monitoring.md`, Cards).
struct CardShown {
    heading: Html,
    standing: Html,
    /// Its run in flight, each wait, or how it ended.
    line: Vec<StatusShown>,
    pull: Option<PullShown>,
}

/// A status's word and what follows it.
struct StatusShown {
    tone: Tone,
    word: &'static str,
    rest: Html,
}

struct PullShown {
    item: Html,
    tone: Option<Tone>,
    ci: &'static str,
}

impl BoardShown {
    fn of(board: &Board, repo: &str) -> BoardShown {
        let lanes = board
            .lanes
            .iter()
            .map(|lane| LaneShown {
                head: lane.head.as_ref().map(|head| {
                    html::titled(head.number, &head.title, repo).then(&html::escape(&format!(
                        " — {}/{} tickets closed",
                        head.tickets_closed, head.tickets
                    )))
                }),
                cells: Column::ALL
                    .into_iter()
                    .map(|column| CellShown {
                        column: column.name(),
                        cards: lane
                            .cards(column)
                            .iter()
                            .map(|card| CardShown::of(card, repo))
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        BoardShown {
            columns: Column::ALL.map(Column::name),
            lanes,
        }
    }
}

impl CardShown {
    fn of(card: &Card, repo: &str) -> CardShown {
        let html = |phrase: &Phrase| html::phrase(phrase, repo);
        let after = |phrase: &Phrase| html::escape(": ").then(&html(phrase));
        let line = match &card.line {
            CardLine::Run(run) => vec![StatusShown {
                tone: Tone::InProgress,
                word: "run in flight",
                rest: after(&run.describe()),
            }],
            CardLine::Waits(waits) => {
                let mut waits: Vec<Phrase> = waits.iter().map(status::describe_wait).collect();
                if waits.is_empty() {
                    waits.push(explain::NOTHING_WAITED_FOR.into());
                }
                waits
                    .iter()
                    .map(|wait| StatusShown {
                        tone: Tone::Pending,
                        word: "waiting for",
                        rest: after(wait),
                    })
                    .collect()
            }
            CardLine::Ended(ended) => vec![ended_shown(*ended, repo)],
        };
        CardShown {
            heading: html::titled(card.number, &card.title, repo),
            standing: html(&card.standing),
            line,
            pull: card.pull.map(|pull| PullShown {
                item: html::item(pull.pull_request, repo),
                tone: pull.ci.tone(),
                ci: pull.ci.describe(),
            }),
        }
    }
}

fn ended_shown(ended: Ended, repo: &str) -> StatusShown {
    let (tone, word, rest) = match ended {
        Ended::MergedBy(pull) => (
            Tone::Success,
            "merged",
            html::escape(" by ").then(&html::item(pull, repo)),
        ),
        Ended::ClosedUnmerged => (Tone::Cancelled, "closed without merging", Html::default()),
        Ended::Closed => (Tone::Cancelled, "closed", Html::default()),
        Ended::NothingToBuild => (
            Tone::Success,
            "designed",
            html::escape(": nothing to build"),
        ),
    };
    StatusShown { tone, word, rest }
}

#[derive(Template, WebTemplate)]
#[template(path = "board.html")]
struct BoardPage {
    page: Page,
    bar: Bar,
    board: BoardShown,
}

#[derive(Template)]
#[template(path = "board.html", block = "main")]
struct BoardRegion<'a> {
    board: &'a BoardShown,
}

/// The tab's title, with the count of cards in Needs a person.
fn board_title(board: &Board, repo: &str) -> String {
    format!("({}) Board · {repo}", board.needing_a_person)
}

async fn board_page(State(sources): State<Sources>) -> Response {
    let repo = &sources.work_dir.repo;
    let board = api::board_answer(&sources);
    BoardPage {
        page: Page::new(board_title(&board, repo), "/events/".into()),
        bar: Bar::of(&sources),
        board: BoardShown::of(&board, repo),
    }
    .into_response()
}

/// The board's regions: the bar, the tab's title and the board.
async fn board_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
) -> Response {
    let regions = move || {
        let repo = &sources.work_dir.repo;
        let bar = Bar::of(&sources);
        let board = api::board_answer(&sources);
        let title = format!(
            r#"<title id="title">{}</title>"#,
            html::escape(&board_title(&board, repo))
        );
        let shown = BoardShown::of(&board, repo);
        vec![
            ("bar", render(&BarRegion { bar: &bar })),
            ("title", title),
            ("board", render(&BoardRegion { board: &shown })),
        ]
    };
    events(regions, stopping)
}

/// What `explain` shows of an item, ready for the page.
struct ItemShown {
    heading: Html,
    title: String,
    lines: Vec<LineShown>,
}

/// A line as `explain` prints it, with the tone of what it says.
enum LineShown {
    /// With the tone of the state it names, if any, by its words.
    Fact {
        label: &'static str,
        tone: Option<Tone>,
        phrase: Html,
    },
    /// A wait or a run in flight, whose label is its status.
    Status {
        label: &'static str,
        tone: Tone,
        phrase: Html,
    },
    Past {
        label: &'static str,
        run: Html,
        outcome: Tone,
        word: &'static str,
        about: Html,
    },
}

impl ItemShown {
    fn of(answer: &explain::Answer, repo: &str) -> ItemShown {
        let html = |phrase: &Phrase| html::phrase(phrase, repo);
        let lines = answer
            .lines()
            .iter()
            .map(|line| {
                let label = line.label();
                match line {
                    Line::Fact { phrase, tone, .. } => LineShown::Fact {
                        label,
                        tone: *tone,
                        phrase: html(phrase),
                    },
                    Line::Wait(wait) => LineShown::Status {
                        label,
                        tone: Tone::Pending,
                        phrase: html(&status::describe_wait(wait)),
                    },
                    Line::InFlight(run) => LineShown::Status {
                        label,
                        tone: Tone::InProgress,
                        phrase: html::escape(&format!("{}: ", run.id)).then(&html(&run.describe())),
                    },
                    Line::Past(run) => LineShown::Past {
                        label,
                        run: html::escape(&run.id.to_string()),
                        outcome: run.outcome.tone(),
                        word: run.outcome.word(),
                        about: html(&runs::describe_run(run)),
                    },
                }
            })
            .collect();
        ItemShown {
            heading: html::item(answer.number(), repo),
            title: answer.title().to_string(),
            lines,
        }
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "item.html")]
struct ItemPage {
    page: Page,
    bar: Bar,
    item: ItemShown,
}

#[derive(Template)]
#[template(path = "item.html", block = "main")]
struct ItemRegion<'a> {
    item: &'a ItemShown,
}

#[derive(Template, WebTemplate)]
#[template(path = "error.html")]
struct ErrorPage {
    page: Page,
    bar: Bar,
    /// The id of the page's region the error stands in for.
    region: &'static str,
    error: String,
}

async fn item_page(State(sources): State<Sources>, Path(number): Path<Number>) -> Response {
    let page = Page::new(
        format!("{number} · {}", sources.work_dir.repo),
        format!("/events/items/{}", number.0),
    );
    let bar = Bar::of(&sources);
    match api::item_answer(&sources, number).await {
        Ok(answer) => ItemPage {
            item: ItemShown::of(&answer, &sources.work_dir.repo),
            page,
            bar,
        }
        .into_response(),
        Err(e) => error_page(page, bar, &e),
    }
}

fn error_page(page: Page, bar: Bar, e: &Unexplained) -> Response {
    let page = ErrorPage {
        page,
        bar,
        region: "item",
        error: e.to_string(),
    };
    (e.status(), page).into_response()
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

/// The item page's regions: the bar, and the item as `serve`'s view
/// explains it, without reading GitHub, when it holds the item.
async fn item_events(
    State(sources): State<Sources>,
    Extension(stopping): Extension<CancellationToken>,
    Path(number): Path<Number>,
) -> Response {
    let regions = move || {
        let bar = Bar::of(&sources);
        let mut regions = vec![("bar", render(&BarRegion { bar: &bar }))];
        if let Ok(in_view) = api::explaining(&sources)
            && let Some(answer) =
                explain::explain_in_view(&in_view.view, &in_view.recorded, number, in_view.now)
        {
            let item = ItemShown::of(&answer, &sources.work_dir.repo);
            regions.push(("item", render(&ItemRegion { item: &item })));
        }
        regions
    };
    events(regions, stopping)
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
