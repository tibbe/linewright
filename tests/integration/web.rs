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
use linewright::api::Sources;
use linewright::clock::Clock;
use linewright::host::UiHosts;
use linewright::web;
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
    dir: tempfile::TempDir,
}

/// The work directory of `REPO` under `dir`, as `sources` has it, where its
/// runs keep their records.
fn work_dir(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("work").join(REPO)
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
        let sources = Sources {
            deliveries: Some(listener.tally()),
            ..sources(
                &dir,
                &clock,
                since,
                serving.served.clone(),
                serving.polling.clone(),
            )
        };
        let router = web::router(sources, UiHosts::default(), stopping.clone());
        Web {
            serving,
            listener,
            router,
            stopping,
            store,
            harness,
            clock,
            dir,
        }
    }

    /// A factory beside `serve`'s, as `linewright automation run` starts
    /// one, over the work directory a world adjusted to `work_dir/REPO`.
    fn work_dir(&self) -> std::path::PathBuf {
        work_dir(&self.dir)
    }

    async fn factory_beside(&self) -> linewright::factory::Factory {
        linewright::factory::Factory::start(
            self.store.clone(),
            self.harness.clone(),
            self.clock.clone(),
            config(&self.work_dir()),
        )
        .await
        .unwrap()
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

impl Routed for Web {
    fn router(&self) -> &Router {
        &self.router
    }
}

/// The web UI's router driven in-process over a `Served` written through
/// the back door: a view built by hand over a local store, with no factory
/// and no poll.
struct Pages {
    router: Router,
    sources: Sources,
    served: tokio::sync::watch::Sender<Option<linewright::api::Served>>,
    /// Set by [`Pages::with_deliveries`].
    listener: Option<Listener>,
    store: std::sync::Arc<linewright::store::local::LocalStore>,
    stopping: CancellationToken,
    dir: tempfile::TempDir,
}

impl Pages {
    /// The pages of `view`, as `serve` shows them once a poll has made it.
    async fn of(view: linewright::status::View) -> Pages {
        Pages::over(view, false).await
    }

    /// [`Pages::of`]'s, with webhooks on: a bare webhook listener, that
    /// nothing takes the deliveries of, tallies them for the pages.
    async fn with_deliveries(view: linewright::status::View) -> Pages {
        Pages::over(view, true).await
    }

    async fn over(view: linewright::status::View, deliveries: bool) -> Pages {
        let dir = tempfile::tempdir().unwrap();
        let clock = std::sync::Arc::new(FakeClock::new());
        let store = std::sync::Arc::new(
            local_store(
                &dir.path().join("store"),
                &Repository::default(),
                clock.clone(),
            )
            .await,
        );
        let listener = if deliveries {
            let listener =
                Listener::bind(&webhooks("127.0.0.1:0"), Pending::default(), clock.clone())
                    .await
                    .unwrap();
            Some(listener)
        } else {
            None
        };
        let polling = linewright::status::Polling {
            next: view.polls.next,
            failed: None,
        };
        let served = Some(linewright::api::Served::new(view, store.clone()));
        let (served, served_receiver) = tokio::sync::watch::channel(served);
        // Kept open by the receivers `Sources` holds.
        let (_, polling) = tokio::sync::watch::channel(Some(polling));
        let since = clock.now();
        let sources = Sources {
            deliveries: listener.as_ref().map(Listener::tally),
            ..sources(&dir, &clock, since, served_receiver, polling)
        };
        let stopping = CancellationToken::new();
        let router = web::router(sources.clone(), UiHosts::default(), stopping.clone());
        Pages {
            router,
            sources,
            served,
            listener,
            store,
            stopping,
            dir,
        }
    }

    /// The webhook listener of [`Pages::with_deliveries`].
    fn deliveries(&self) -> &Listener {
        self.listener.as_ref().expect("webhooks on")
    }

    /// Publishes `view` as the next one `serve` shows, as a poll would.
    fn publish(&self, view: linewright::status::View) {
        let served = linewright::api::Served::new(view, self.store.clone());
        self.served.send_replace(Some(served));
    }

    /// The web UI listening on a port of its own, answering `hosts` besides
    /// the local names and IP addresses.
    async fn listen(&self, hosts: &[&str]) -> web::Listener {
        let ui = web::Ui {
            address: "127.0.0.1:0".parse().unwrap(),
            hosts: UiHosts::new(hosts.iter().copied()).unwrap(),
        };
        web::Listener::bind(&ui, self.sources.clone())
            .await
            .unwrap()
    }

    /// Writes the run `record`, as its run would have kept it.
    fn record(&self, record: &serde_json::Value) {
        write_record(&work_dir(&self.dir), record);
    }

    /// Writes each of `files`, a path under the work directory and what it
    /// holds, as runs leave them there.
    fn write(&self, files: &[(&str, &str)]) {
        write_files(&work_dir(&self.dir), files);
    }

    /// Adds `text` to the end of `path` under the work directory, as a
    /// session adds to its output while it runs.
    fn append(&self, path: &str, text: &str) {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(self.path(path))
            .unwrap();
        std::io::Write::write_all(&mut file, text.as_bytes()).unwrap();
    }

    /// The work directory's path of `path`.
    fn path(&self, path: &str) -> std::path::PathBuf {
        work_dir(&self.dir).join(path)
    }
}

impl Routed for Pages {
    fn router(&self) -> &Router {
        &self.router
    }
}

impl Drop for Pages {
    /// Ends the event streams.
    fn drop(&mut self) {
        self.stopping.cancel();
    }
}

/// What answers the web UI's requests in-process.
trait Routed {
    fn router(&self) -> &Router;

    async fn response(&self, path: &str) -> axum::response::Response {
        routed(self.router(), hyper::Method::GET, Some("localhost"), path).await
    }

    async fn get(&self, path: &str) -> (u16, String) {
        status_and_body(self.response(path).await).await
    }

    async fn page(&self, path: &str) -> Html {
        let (status, body) = self.get(path).await;
        assert_eq!(status, 200, "{body}");
        Html::parse_document(&body)
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

/// Where each of `links` goes.
fn hrefs(links: Vec<ElementRef>) -> Vec<String> {
    links
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .map(str::to_string)
        .collect()
}

/// Whether `element` has every one of `classes`.
fn has_classes(element: ElementRef, classes: &[&str]) -> bool {
    classes.iter().all(|class| {
        element
            .value()
            .has_class(class, scraper::CaseSensitivity::CaseSensitive)
    })
}

/// Asserts that `selector` selects something in `html`, all of it in Muted
/// text and in no translucent text colour: `docs/specs/web/design_system.md`,
/// Colour scheme: secondary text is in Muted text, and the web UI draws in
/// no colour but its own.
fn assert_in_muted_text(html: &Html, selector: &str) {
    let selected = select(html, selector);
    assert!(!selected.is_empty(), "{selector}");
    for element in selected {
        assert!(
            has_classes(element, &["text-muted"])
                && !element
                    .value()
                    .classes()
                    .any(|class| class.starts_with("text-") && class.contains('/')),
            "{}",
            element.html()
        );
    }
}

/// Classes that would grow cut text in place on hover or on focus, moving
/// what is around it.
fn grows_in_place(element: ElementRef<'_>) -> Vec<&str> {
    element
        .value()
        .classes()
        .filter(|class| {
            [
                "hover:",
                "focus-within:",
                "group-hover:",
                "group-focus-within:",
            ]
            .iter()
            .any(|variant| class.starts_with(variant))
        })
        .collect()
}

/// Whether `element` has none of `classes`.
fn lacks_classes(element: ElementRef, classes: &[&str]) -> bool {
    !classes.iter().any(|class| has_classes(element, &[class]))
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

/// The item page's facts, each label and its value's text, in order.
fn item_facts(html: &Html) -> Vec<(String, String)> {
    let labels = select(html, "#item dl > dt");
    let values = select(html, "#item dl > dd");
    assert_eq!(labels.len(), values.len(), "{}", html.html());
    labels
        .into_iter()
        .map(text)
        .zip(values.into_iter().map(text))
        .collect()
}

/// #1, ready for Linewright.
fn a_ticket() -> linewright::status::View {
    view(vec![ticket(1, "Do the thing")], vec![])
}

async fn with_ticket() -> World {
    let world = World::new().await;
    open_issue_linewright_labeled(
        &world.store,
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
async fn stream(web: &impl Routed, path: &str) -> tokio::sync::mpsc::UnboundedReceiver<String> {
    let response = web.response(path).await;
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
fn record_interrupted(pages: &Pages, item: u64, n: u32, started_at: &str) {
    let record = serde_json::json!({
        "item": item,
        "stage": "triage",
        "n": n,
        "started_at": started_at,
        "end": null,
        "sessions": [],
    });
    pages.record(&record);
}

/// Writes the record of a run, an item's or an automation's, into
/// `work_dir`, where its run keeps it.
fn write_record(work_dir: &std::path::Path, record: &serde_json::Value) {
    let path = match record["automation"].as_str() {
        Some(automation) => format!("runs/automations/{automation}/{}.json", record["n"]),
        None => format!(
            "runs/items/{}/{}-{}.json",
            record["item"],
            record["stage"].as_str().unwrap(),
            record["n"]
        ),
    };
    write_files(work_dir, &[(&path, &record.to_string())]);
}
