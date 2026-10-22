use crate::common;

mod automations;
mod board;
mod frame;
mod item;
mod run;
mod runs;
mod script;
mod serving;

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use common::*;
use http_body_util::BodyExt as _;
use linewright::api::{self, Sources};
use linewright::clock::Clock;
use linewright::reconcile::WorkKind;
use linewright::store::Store as _;
use linewright::webhook::{Listener, Pending};
use scraper::{ElementRef, Html, Selector};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

/// `serve` polling hourly, so its view holds still but for deliveries, and
/// its router driven in-process.
struct Web {
    serving: Serving,
    listener: Listener,
    router: Router,
    stopping: CancellationToken,
    store: std::sync::Arc<linewright::store::local::LocalStore>,
    harness: std::sync::Arc<ScriptedHarness>,
    clock: std::sync::Arc<FakeClock>,
    _dir: tempfile::TempDir,
}

/// What `serve` answers from, over `served` and `polling`, with webhooks
/// off.
fn sources(
    dir: &tempfile::TempDir,
    clock: &std::sync::Arc<FakeClock>,
    since: linewright::domain::Timestamp,
    served: tokio::sync::watch::Receiver<Option<linewright::api::Served>>,
    polling: tokio::sync::watch::Receiver<Option<linewright::status::Polling>>,
) -> Sources {
    Sources {
        serving: linewright::status::Serving { since, max_runs: 4 },
        served,
        polling,
        deliveries: None,
        clock: clock.clone(),
        work_dir: linewright::runs::RepoWorkDir {
            // As `common::config` has it, under the repository's name.
            base: dir.path().join("work"),
            repo: REPO.parse().unwrap(),
        },
        ends: linewright::ending::Ends::new(),
    }
}

impl Web {
    async fn start(world: World) -> Web {
        let web = Web::polling(world, Duration::from_hours(1)).await;
        let mut served = web.serving.served.clone();
        tokio::time::timeout(Duration::from_secs(10), served.wait_for(Option::is_some))
            .await
            .expect("the first poll")
            .unwrap();
        web
    }

    /// `serve` polling every `poll_interval`, without waiting for its first
    /// poll.
    async fn polling(world: World, poll_interval: Duration) -> Web {
        let World {
            clock,
            factory,
            dir,
            store,
            harness,
        } = world;
        let since = clock.now();
        let pending = Pending::default();
        let listener = Listener::bind(&webhooks("127.0.0.1:0"), pending.clone(), clock.clone())
            .await
            .unwrap();
        let serving = Serving::start(factory, poll_interval, pending);
        let stopping = CancellationToken::new();
        let router = api::router(
            Sources {
                deliveries: Some(listener.tally()),
                ..sources(
                    &dir,
                    &clock,
                    since,
                    serving.served.clone(),
                    serving.polling.clone(),
                )
            },
            stopping.clone(),
        );
        Web {
            serving,
            listener,
            router,
            stopping,
            store,
            harness,
            clock,
            _dir: dir,
        }
    }

    /// A factory beside `serve`'s, as `linewright automation run` starts
    /// one, over the work directory a world adjusted to `work_dir/REPO`.
    async fn factory_beside(&self) -> linewright::factory::Factory {
        linewright::factory::Factory::start(
            self.store.clone(),
            self.harness.clone(),
            self.clock.clone(),
            config(&self._dir.path().join("work").join(REPO)),
        )
        .await
        .unwrap()
    }

    async fn get_from(&self, host: &str, path: &str) -> axum::response::Response {
        let request = axum::http::Request::get(path)
            .header("Host", host)
            .body(Body::empty())
            .unwrap();
        self.router.clone().oneshot(request).await.unwrap()
    }

    async fn get(&self, path: &str) -> (u16, String) {
        let response = self.get_from("localhost", path).await;
        let status = response.status().as_u16();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    async fn page(&self, path: &str) -> Html {
        let (status, body) = self.get(path).await;
        assert_eq!(status, 200, "{body}");
        Html::parse_document(&body)
    }

    /// A delivery naming issue #`number`.
    async fn deliver_issue(&self, number: u64) {
        let body = serde_json::json!({"issue": {"number": number}}).to_string();
        let status = signed(&self.listener, "issues", &body).await;
        assert!(status.is_success(), "{status}");
    }

    async fn stop(self) {
        self.stopping.cancel();
        self.serving.stop().await;
        self.listener.stop().await;
    }

    async fn eventually_page(&self, path: &str, shown: impl Fn(&Html) -> bool) -> Html {
        let waited = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let html = self.page(path).await;
                if shown(&html) {
                    return html;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        waited.expect("shown in time")
    }
}

fn select<'a>(html: &'a Html, selector: &str) -> Vec<ElementRef<'a>> {
    html.select(&Selector::parse(selector).unwrap()).collect()
}

/// What `selector` selects in `element`.
fn select_in<'a>(element: ElementRef<'a>, selector: &str) -> Vec<ElementRef<'a>> {
    element
        .select(&Selector::parse(selector).unwrap())
        .collect()
}

/// The path of the event stream the page connects to.
fn events_path(html: &Html) -> Option<&str> {
    select(html, "body")[0].attr("hx-sse:connect")
}

/// The text a reader sees, with its spacing, and its code in backticks, as
/// the commands print them.
fn text(element: ElementRef) -> String {
    fn gather(element: ElementRef, into: &mut String) {
        for child in element.children() {
            if let Some(text) = child.value().as_text() {
                into.push_str(text);
            } else if let Some(child) = ElementRef::wrap(child) {
                let code = child.value().name() == "code";
                if code {
                    into.push('`');
                }
                gather(child, into);
                if code {
                    into.push('`');
                }
            }
        }
    }
    let mut text = String::new();
    gather(element, &mut text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The item page's heading and lines, as a reader reads them.
fn item_lines(html: &Html) -> Vec<String> {
    select(html, "#item h1, #item li")
        .into_iter()
        .map(text)
        .collect()
}

async fn with_ticket() -> World {
    let world = World::new().await;
    world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-human"],
    );
    world
}

/// Each icon beside the words it stands for.
fn assert_no_status_by_colour_alone(html: &Html) {
    for icon in select(html, "svg.icon") {
        let words = ElementRef::wrap(icon.parent().unwrap()).map(text);
        assert!(
            words.is_some_and(|words| !words.is_empty()),
            "an icon without words: {}",
            icon.html()
        );
    }
}

/// The data of each event the stream at `path` sends.
async fn stream(web: &Web, path: &str) -> tokio::sync::mpsc::UnboundedReceiver<String> {
    let response = web.get_from("localhost", path).await;
    assert_eq!(response.status(), 200);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut body = response.into_body();
    tokio::spawn(async move {
        let mut unread = String::new();
        while let Some(Ok(frame)) = body.frame().await {
            let Some(bytes) = frame.data_ref() else {
                continue;
            };
            unread.push_str(std::str::from_utf8(bytes).unwrap());
            while let Some(end) = unread.find("\n\n") {
                let block: String = unread.drain(..end + 2).collect();
                let data: Vec<&str> = block
                    .lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .collect();
                if !data.is_empty() && sender.send(data.join("\n")).is_err() {
                    return;
                }
            }
        }
    });
    receiver
}

/// The regions an event replaces, by id.
fn regions(event: &str) -> Vec<(String, Html)> {
    let html = Html::parse_fragment(event);
    select(&html, "hx-partial")
        .into_iter()
        .map(|partial| {
            (
                partial.attr("id").unwrap().to_string(),
                Html::parse_fragment(&partial.inner_html()),
            )
        })
        .collect()
}

/// The column heads of the table of runs, as a reader reads them.
fn runs_heads(html: &Html) -> Vec<String> {
    select(html, "#runs thead th")
        .into_iter()
        .map(text)
        .collect()
}

/// Each row of the table of runs, cell by cell, as a reader reads them.
fn runs_shown(html: &Html) -> Vec<Vec<String>> {
    select(html, "#runs tbody tr")
        .into_iter()
        .map(|row| {
            row.select(&Selector::parse("td, th").unwrap())
                .map(text)
                .collect()
        })
        .collect()
}

/// Whether `rows` holds the run `id` with the outcome `outcome`.
fn has_run(rows: &[Vec<String>], outcome: &str, id: &str) -> bool {
    rows.iter()
        .any(|row| row.len() > 1 && row[0] == outcome && row[1] == id)
}

/// The classes of the elements `selector` matches.
fn classes<'a>(html: &'a Html, selector: &str) -> Vec<Vec<&'a str>> {
    select(html, selector)
        .into_iter()
        .map(|element| element.value().classes().collect())
        .collect()
}

/// Records run `n` of #`item`'s triage as started at `started_at` and never
/// ended nor in flight: interrupted, as when its process died.
fn record_interrupted(web: &Web, item: u64, n: u32, started_at: &str) {
    let dir = web
        ._dir
        .path()
        .join("work")
        .join(REPO)
        .join("runs/items")
        .join(item.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    let record = serde_json::json!({
        "item": item,
        "stage": "triage",
        "n": n,
        "started_at": started_at,
        "end": null,
        "sessions": [],
    });
    std::fs::write(dir.join(format!("triage-{n}.json")), record.to_string()).unwrap();
}
