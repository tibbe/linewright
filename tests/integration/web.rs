use crate::common;

mod board;
mod frame;
mod item;
mod run;
mod runs;
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
    _dir: tempfile::TempDir,
}

impl Web {
    async fn start(world: World) -> Web {
        let World {
            clock,
            factory,
            dir,
            store,
            ..
        } = world;
        let since = clock.now();
        let pending = Pending::default();
        let listener = Listener::bind(&webhooks("127.0.0.1:0"), pending.clone(), clock.clone())
            .await
            .unwrap();
        let serving = Serving::start(factory, Duration::from_hours(1), pending);
        let stopping = CancellationToken::new();
        let router = api::router(
            Sources {
                serving: linewright::status::Serving { since, max_runs: 4 },
                served: serving.served.clone(),
                deliveries: Some(listener.tally()),
                clock: clock.clone(),
                work_dir: linewright::runs::RepoWorkDir {
                    // As `common::config` has it, under the repository's name.
                    base: dir.path().join("work"),
                    repo: REPO.into(),
                },
                ends: linewright::ending::Ends::new(),
            },
            stopping.clone(),
        );
        let mut served = serving.served.clone();
        tokio::time::timeout(Duration::from_secs(10), served.wait_for(Option::is_some))
            .await
            .expect("the first poll")
            .unwrap();
        Web {
            serving,
            listener,
            router,
            stopping,
            store,
            _dir: dir,
        }
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

async fn eventually_shown(web: &Web, path: &str, shown: impl Fn(&[String]) -> bool) -> Html {
    let waited = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let html = web.page(path).await;
            if shown(&item_lines(&html)) {
                return html;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    waited.expect("shown in time")
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

/// The runs page's lines, as a reader reads them.
fn runs_shown(html: &Html) -> Vec<String> {
    select(html, "#runs li").into_iter().map(text).collect()
}
