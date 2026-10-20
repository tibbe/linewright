use crate::common;

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

const REPO: &str = "owner/widgets";

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
                deliveries: None,
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
}

fn select<'a>(html: &'a Html, selector: &str) -> Vec<ElementRef<'a>> {
    html.select(&Selector::parse(selector).unwrap()).collect()
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

fn without_last_read(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .filter(|line| !line.starts_with("last read: "))
        .collect()
}

fn read_at_a_time(line: &str) -> bool {
    line.strip_prefix("last read: 2026-10-01 ")
        .is_some_and(|time| time.ends_with(" UTC"))
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

#[tokio::test]
async fn the_item_page_shows_what_explain_shows_with_its_items_linked_and_its_times_as_times() {
    let web = Web::start(with_ticket().await).await;

    let html = web.page("/items/1").await;

    let lines = item_lines(&html);
    assert_eq!(
        without_last_read(lines.clone()),
        [
            "#1: Do the thing",
            "state: with the maintainer",
            "waiting for: the maintainer: to label it `ready-for-agent` or `needs-triage`",
        ]
    );
    assert!(read_at_a_time(&lines[2]), "{lines:?}");
    let links: Vec<(&str, &str)> = select(&html, "#item h1 a")
        .into_iter()
        .map(|a| {
            (
                a.attr("href").unwrap(),
                a.attr("aria-label").unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        links,
        [
            ("/items/1", ""),
            ("https://github.com/owner/widgets/issues/1", "#1 on GitHub"),
        ]
    );
    let last_read = select(&html, "#item li time");
    assert_eq!(last_read.len(), 1);
    let at = last_read[0].attr("datetime").unwrap();
    assert!(
        at.starts_with("2026-10-01T12:") && at.ends_with('Z'),
        "{at}"
    );
    web.stop().await;
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

#[tokio::test]
async fn the_item_page_of_an_item_linewright_leaves_says_why() {
    let world = with_ticket().await;
    let closed = world.store.open_issue(
        "maintainer",
        "Old idea",
        "Maybe.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.close_issue_as(closed).unwrap();
    let web = Web::start(world).await;

    let html = web.page(&format!("/items/{}", closed.0)).await;

    let lines = item_lines(&html);
    assert_eq!(
        without_last_read(lines.clone()),
        [
            format!("{closed}: Old idea"),
            "state: closed issue".to_string(),
            "left alone: it is closed".to_string(),
        ]
    );
    assert!(read_at_a_time(&lines[2]), "{lines:?}");
    let closed_state = select(&html, "#item li svg.icon");
    assert_eq!(closed_state.len(), 1);
    assert_no_status_by_colour_alone(&html);
    web.stop().await;
}

#[tokio::test]
async fn the_item_page_of_no_item_says_there_is_none() {
    let web = Web::start(with_ticket().await).await;

    let (status, body) = web.get("/items/99").await;

    assert_eq!(status, 404);
    let html = Html::parse_document(&body);
    assert_eq!(
        select(&html, "main p")
            .into_iter()
            .map(text)
            .collect::<Vec<_>>(),
        ["there is no issue or pull request #99"]
    );
    assert_eq!(select(&html, "#bar").len(), 1);
    web.stop().await;
}

async fn with_issue_for_triage() -> World {
    let world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    world.harness.on(WorkKind::Triage, |_| {
        Ok(serde_json::json!({
            "outcome": "ready-for-agent",
            "category": "bug",
            "comment": "## Agent Brief\n\n**Summary:** It crashes."
        })
        .to_string())
    });
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world
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

#[tokio::test]
async fn the_item_page_shows_the_run_in_flight_ticking_and_then_the_run_past() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let web = Web::start(world).await;

    let in_flight = "run in flight: 1/triage-1: #1 triage, running for ";
    let running_html = eventually_shown(&web, "/items/1", |lines| {
        lines.iter().any(|line| line.starts_with(in_flight))
    })
    .await;
    let running = select(&running_html, "#item li time[data-since]");
    assert!(
        running.first().is_some_and(|time| time
            .attr("data-since")
            .unwrap()
            .starts_with("2026-10-01T12:")),
        "{}",
        running_html.html()
    );
    let status: Vec<String> = select(&running_html, "#item .status")
        .into_iter()
        .map(text)
        .collect();
    assert!(status.contains(&"run in flight".to_string()), "{status:?}");

    drop(agent);
    let past = "past run: 1/triage-1: done — #1 triage, started ";
    let html = eventually_shown(&web, "/items/1", |lines| {
        lines.iter().any(|line| line.starts_with(past))
    })
    .await;
    let done: Vec<ElementRef> = select(&html, "#item .status.success");
    assert_eq!(done.into_iter().map(text).collect::<Vec<_>>(), ["done"]);
    assert_no_status_by_colour_alone(&html);
    web.stop().await;
}

fn bar_facts(html: &Html) -> Vec<String> {
    select(html, "#bar .facts li")
        .into_iter()
        .map(text)
        .collect()
}

#[tokio::test]
async fn the_bar_names_the_repository_and_shows_the_facts_as_status_does() {
    let world = with_ticket().await;
    world
        .store
        .commit(
            "maintainer",
            "main",
            &[(
                ".linewright/automations/weekly.md",
                "---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n",
            )],
            "Add an automation",
        )
        .await
        .unwrap();
    let web = Web::start(world).await;

    let html = web.page("/items/1").await;

    let facts = bar_facts(&html);
    let labels: Vec<&str> = facts
        .iter()
        .map(|fact| {
            fact.split_once(": ")
                .map_or(fact.as_str(), |(label, _)| label)
        })
        .collect();
    assert_eq!(
        labels,
        [
            "serving since",
            "last poll",
            "next poll",
            "runs in flight",
            "automation weekly last run",
            "automation weekly next run",
        ],
        "{facts:?}"
    );
    assert_eq!(facts[3], "runs in flight: 0 of 4");
    assert!(
        facts[1]
            .strip_prefix("last poll: 2026-10-01 12:00:")
            .is_some_and(|rest| rest.ends_with(" UTC")),
        "{facts:?}"
    );
    let repo = select(&html, "#bar a.repo");
    assert_eq!(
        repo[0].attr("href"),
        Some("https://github.com/owner/widgets")
    );
    assert_eq!(text(repo[0]), "owner/widgets");
    let links: Vec<_> = select(&html, "#bar nav a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert!(
        links.contains(&"/") && links.contains(&"/runs"),
        "{links:?}"
    );
    assert_eq!(banners(&html), Vec::<String>::new());
    web.stop().await;
}

#[tokio::test]
async fn the_bar_shows_a_usage_limit_in_force_as_a_banner() {
    let world = with_issue_for_triage().await;
    world
        .harness
        .on(WorkKind::Triage, |_| refused_for_the_usage_limit());
    let web = Web::start(world).await;

    let html = eventually_shown_bar(&web, |banners| !banners.is_empty()).await;

    assert_eq!(
        banners(&html),
        ["usage limit: in force until 2026-10-13 07:50:00 UTC"]
    );
    let facts = bar_facts(&html);
    assert!(
        !facts.iter().any(|fact| fact.starts_with("usage limit")),
        "{facts:?}"
    );
    web.stop().await;
}

#[tokio::test]
async fn the_bar_shows_a_drain_under_way_as_a_banner() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let web = Web::start(world).await;
    eventually_shown(&web, "/items/1", |lines| {
        lines.iter().any(|line| line.starts_with("run in flight: "))
    })
    .await;

    web.serving.drain();

    let html = eventually_shown_bar(&web, |banners| !banners.is_empty()).await;
    let banners = banners(&html);
    assert!(
        banners.len() == 1 && banners[0].starts_with("drain: under way since 2026-10-01 "),
        "{banners:?}"
    );
    drop(agent);
    web.stop().await;
}

fn banners(html: &Html) -> Vec<String> {
    select(html, "#bar .banner").into_iter().map(text).collect()
}

async fn eventually_shown_bar(web: &Web, shown: impl Fn(&[String]) -> bool) -> Html {
    let waited = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let html = web.page("/items/1").await;
            if shown(&banners(&html)) {
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

#[tokio::test]
async fn the_item_pages_stream_sends_every_region_then_those_serves_view_changed() {
    let web = Web::start(with_ticket().await).await;
    let mut events = stream(&web, "/events/items/1").await;

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<String> = regions(&first).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, ["bar", "item"]);
    let quiet = tokio::time::timeout(Duration::from_millis(2500), events.recv()).await;
    assert!(quiet.is_err(), "sent with nothing changed: {quiet:?}");

    web.store
        .unlabel_as(linewright::domain::Number(1), "ready-for-human")
        .unwrap();
    web.store
        .label_as(linewright::domain::Number(1), "needs-info")
        .unwrap();
    web.deliver_issue(1).await;
    let changed = tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .unwrap()
        .unwrap();

    let item = regions(&changed)
        .into_iter()
        .find(|(id, _)| id == "item")
        .map(|(_, html)| item_lines(&html))
        .expect("the item's region");
    assert_eq!(
        without_last_read(item),
        [
            "#1: Do the thing",
            "state: waiting on the reporter",
            "waiting for: the reporter: labeled `needs-info`",
        ]
    );
    web.stop().await;
}

#[tokio::test]
async fn the_item_pages_stream_sends_the_item_again_when_its_runs_agent_produces_output() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let web = Web::start(world).await;
    eventually_shown(&web, "/items/1", |lines| {
        lines
            .iter()
            .any(|line| line.starts_with("run in flight: ") && line.contains(", quiet for "))
    })
    .await;
    let mut events = stream(&web, "/events/items/1").await;
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let quiet = tokio::time::timeout(Duration::from_millis(2500), events.recv()).await;
    assert!(quiet.is_err(), "sent with nothing changed: {quiet:?}");

    agent.says();
    let heard = tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .unwrap()
        .unwrap();

    let ids: Vec<String> = regions(&heard).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, ["item"]);
    drop(agent);
    web.stop().await;
}

#[tokio::test]
async fn a_request_whose_host_is_not_local_is_refused_for_pages_assets_and_streams() {
    let web = Web::start(with_ticket().await).await;
    let html = web.page("/items/1").await;
    let assets: Vec<String> = select(&html, "link[rel=stylesheet], script[src]")
        .into_iter()
        .map(|asset| {
            asset
                .attr("href")
                .or_else(|| asset.attr("src"))
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(assets.len(), 3);
    let paths = ["/items/1", "/events/items/1"]
        .into_iter()
        .map(str::to_string)
        .chain(assets);

    for path in paths {
        let refused = web.get_from("evil.example", &path).await;
        let answered = web.get_from("localhost", &path).await;
        assert_eq!(refused.status(), 403, "{path}");
        assert_eq!(answered.status(), 200, "{path}");
    }
    web.stop().await;
}

#[tokio::test]
async fn pages_run_only_linewrights_own_scripts_and_assets_are_kept_for_good() {
    let web = Web::start(with_ticket().await).await;

    let page = web.get_from("localhost", "/items/1").await;
    let html = web.page("/items/1").await;
    let script = select(&html, "script[src]")[0]
        .attr("src")
        .unwrap()
        .to_string();
    let asset = web.get_from("localhost", &script).await;
    let stale = web
        .get_from("localhost", "/assets/htmx-0000000000000000.min.js")
        .await;

    assert_eq!(
        page.headers()["content-security-policy"],
        "default-src 'self'"
    );
    assert_eq!(select(&html, "script:not([src])").len(), 0);
    assert!(
        asset.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    assert_eq!(stale.status(), 404);
    web.stop().await;
}

/// #1 with the maintainer; #2 cut into #3, which waits on the reporter;
/// and #4, closed.
async fn with_a_board() -> World {
    let world = with_ticket().await;
    let tracking = world.store.open_issue(
        "maintainer",
        "Big thing",
        "Cut it.",
        &["linewright:designed"],
    );
    let part = world
        .store
        .open_issue(LINEWRIGHT, "Part one", "Of it.", &["needs-info"]);
    world.store.add_sub_issue(tracking, part).await.unwrap();
    let closed = world.store.open_issue(
        "maintainer",
        "Old idea",
        "Maybe.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.close_issue_as(closed).unwrap();
    world
}

/// Each lane's heading, then each card as its column and its lines, as a
/// reader reads them.
fn board_shown(html: &Html) -> Vec<Vec<String>> {
    let cells = Selector::parse(".cell").unwrap();
    select(html, "#board .lane")
        .into_iter()
        .map(|lane| {
            let heading = lane
                .select(&Selector::parse(".lane-head").unwrap())
                .map(text)
                .collect::<String>();
            let cards = lane.select(&cells).flat_map(|cell| {
                let column = cell.attr("aria-label").unwrap().to_string();
                cell.select(&Selector::parse(".card").unwrap())
                    .map(move |card| {
                        let lines: Vec<String> = card
                            .select(&Selector::parse("h3, .standing, .line li, .pull").unwrap())
                            .map(text)
                            .collect();
                        format!("{column}: {}", lines.join(" | "))
                    })
                    .collect::<Vec<_>>()
            });
            std::iter::once(heading).chain(cards).collect()
        })
        .collect()
}

#[tokio::test]
async fn the_board_shows_the_cards_in_their_columns_and_lanes_worded_as_explain_words_them() {
    let web = Web::start(with_a_board().await).await;

    let html = web.page("/").await;

    assert_eq!(
        board_shown(&html),
        [
            vec![
                "#2: Big thing — 0/1 tickets closed".to_string(),
                "Needs a person: #3: Part one | waiting on the reporter \
                 | waiting for: the reporter: labeled `needs-info`"
                    .to_string(),
            ],
            vec![
                String::new(),
                "Needs a person: #1: Do the thing | with the maintainer \
                 | waiting for: the maintainer: to label it `ready-for-agent` or `needs-triage`"
                    .to_string(),
                "Done: #4: Old idea | closed issue | closed".to_string(),
            ],
        ]
    );
    let columns: Vec<String> = select(&html, "#board .column")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(
        columns,
        [
            "Triage",
            "Needs a person",
            "Ready",
            "Working",
            "In review",
            "Done"
        ]
    );
    let links: Vec<&str> = select(&html, "#board .card h3 a:not(.github)")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(links, ["/items/3", "/items/1", "/items/4"]);
    let title = select(&html, "title");
    assert_eq!(text(title[0]), "(2) Board · owner/widgets");
    assert_no_status_by_colour_alone(&html);
    web.stop().await;
}

#[tokio::test]
async fn the_boards_stream_sends_the_board_and_the_tab_title_after_a_poll_moves_a_card() {
    let web = Web::start(with_a_board().await).await;
    let mut events = stream(&web, "/events/").await;
    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<String> = regions(&first).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, ["bar", "title", "board"]);

    web.store
        .close_issue_as(linewright::domain::Number(1))
        .unwrap();
    web.deliver_issue(1).await;
    let moved = loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .unwrap()
            .unwrap();
        let regions = regions(&event);
        if regions.iter().any(|(id, _)| id == "board") {
            break regions;
        }
    };

    let board = &moved.iter().find(|(id, _)| id == "board").unwrap().1;
    let rest = board_shown(board).pop().unwrap();
    assert!(
        rest.contains(&"Done: #1: Do the thing | closed issue | closed".to_string()),
        "{rest:?}"
    );
    let title = &moved.iter().find(|(id, _)| id == "title").unwrap().1;
    assert_eq!(text(select(title, "title")[0]), "(1) Board · owner/widgets");
    web.stop().await;
}
