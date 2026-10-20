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
    assert_eq!(assets.len(), 4);
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
    let scripts: Vec<String> = select(&html, "script[src]")
        .into_iter()
        .map(|script| script.attr("src").unwrap().to_string())
        .collect();
    let stale = web
        .get_from("localhost", "/assets/htmx-0000000000000000.min.js")
        .await;

    assert_eq!(
        page.headers()["content-security-policy"],
        "default-src 'self'"
    );
    assert_eq!(select(&html, "script:not([src])").len(), 0);
    let named: Vec<(&str, &str)> = scripts
        .iter()
        .map(|script| {
            let name = script.strip_prefix("/assets/").unwrap();
            let (hashed, extension) = name.split_once('.').unwrap();
            let (stem, _hash) = hashed.rsplit_once('-').unwrap();
            (stem, extension)
        })
        .collect();
    assert_eq!(
        named,
        [("htmx", "min.js"), ("hx-sse", "js"), ("linewright", "js")]
    );
    for script in &scripts {
        let asset = web.get_from("localhost", script).await;
        assert_eq!(asset.status(), 200, "{script}");
        assert_eq!(
            asset.headers()["content-type"],
            "text/javascript; charset=utf-8"
        );
        assert!(
            asset.headers()["cache-control"]
                .to_str()
                .unwrap()
                .contains("immutable")
        );
    }
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

/// #1 and #2, each triaged in a run of its own.
async fn with_two_issues_triaged() -> World {
    let world = with_issue_for_triage().await;
    world
        .store
        .open_issue("reporter", "Import fails", "It hangs.", &["needs-triage"]);
    world
}

impl Web {
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

/// The runs page's lines, as a reader reads them.
fn runs_shown(html: &Html) -> Vec<String> {
    select(html, "#runs li").into_iter().map(text).collect()
}

#[tokio::test]
async fn the_runs_page_lists_the_runs_as_runs_does_narrowed_by_its_form() {
    let web = Web::start(with_two_issues_triaged().await).await;
    let done = |html: &Html| {
        runs_shown(html)
            .iter()
            .filter(|run| run.contains(": done — "))
            .count()
            == 2
    };
    web.eventually_page("/runs", done).await;

    let html = web
        .page("/runs?item=&stage=triage&since=2026-10-01&limit=1")
        .await;

    let shown = runs_shown(&html);
    assert_eq!(shown.len(), 2, "{shown:?}");
    let run = shown[0]
        .strip_prefix("2/triage-1: done — #2 triage, started 2026-10-01 12:00:")
        .and_then(|rest| rest.split_once(" UTC, ran "));
    assert!(
        run.is_some_and(|(_, ran)| ran
            .strip_suffix('s')
            .is_some_and(|seconds| seconds.parse::<u32>().is_ok())),
        "{shown:?}"
    );
    assert_eq!(shown[1], "left out: 1 earlier run; show them all");
    let links: Vec<&str> = select(&html, "#runs li a:not(.github)")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(
        links,
        [
            "/runs/2/triage-1",
            "/items/2",
            "/runs?stage=triage&since=2026-10-01"
        ]
    );
    let fields: Vec<(&str, String)> = select(&html, "form input, form select")
        .into_iter()
        .map(|field| {
            let value = match field.value().name() {
                "select" => field
                    .select(&Selector::parse("option[selected]").unwrap())
                    .filter_map(|option| option.attr("value"))
                    .collect(),
                _ => field.attr("value").unwrap_or_default().to_string(),
            };
            (field.attr("name").unwrap(), value)
        })
        .collect();
    assert_eq!(
        fields,
        [
            ("item", String::new()),
            ("stage", "triage".to_string()),
            ("since", "2026-10-01".to_string()),
            ("limit", "1".to_string()),
        ]
    );
    assert_eq!(select(&html, "form")[0].attr("method"), Some("get"));
    assert_no_status_by_colour_alone(&html);
    web.stop().await;
}

#[tokio::test]
async fn the_runs_pages_stream_lists_the_runs_again_as_one_starts_and_ends() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let web = Web::start(world).await;
    let html = web
        .eventually_page("/runs?stage=triage", |html| !runs_shown(html).is_empty())
        .await;
    let events_path = events_path(&html).unwrap().to_string();
    assert_eq!(events_path, "/events/runs?stage=triage");
    let mut events = stream(&web, &events_path).await;
    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let first = regions(&first);
    let ids: Vec<&str> = first.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["bar", "runs"]);
    let running = runs_shown(&first[1].1);
    assert!(
        running.len() == 1 && running[0].starts_with("1/triage-1: in flight — "),
        "{running:?}"
    );

    drop(agent);
    let ended = loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let Some((_, runs)) = regions(&event).into_iter().find(|(id, _)| id == "runs") {
            break runs_shown(&runs);
        }
    };

    assert!(
        ended.len() == 1 && ended[0].starts_with("1/triage-1: done — "),
        "{ended:?}"
    );
    web.stop().await;
}

/// #1 implemented in a run whose transcript is the one recorded, with a
/// sub-agent's work and a call that failed.
async fn implemented_as_recorded() -> World {
    let world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    crate::logs::implement_as_recorded(&world);
    world.store.open_issue(
        "maintainer",
        "Ship gh",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world
}

/// The run page's headers and entries in order, each entry as its line,
/// and with `io` its fold's input and result too, as a reader reads them.
fn transcript_shown(html: &Html, io: bool) -> Vec<String> {
    let parts = Selector::parse(if io { ".line, .io" } else { ".line" }).unwrap();
    select(html, "#transcript h1, #transcript h2, #transcript .entry")
        .into_iter()
        .map(|shown| {
            if !shown.value().classes().any(|class| class == "entry") {
                return text(shown);
            }
            let own: Vec<String> = shown
                .select(&parts)
                .filter(|part| entry_of(*part) == Some(shown))
                .map(text)
                .collect();
            own.join(" ")
        })
        .collect()
}

/// The entry `element` is part of.
fn entry_of(element: ElementRef) -> Option<ElementRef> {
    element
        .ancestors()
        .filter_map(ElementRef::wrap)
        .find(|ancestor| ancestor.value().classes().any(|class| class == "entry"))
}

async fn run_page_of_implemented() -> (Web, Html) {
    let web = Web::start(implemented_as_recorded().await).await;
    let html = web
        .eventually_page("/runs?item=1", |html| {
            runs_shown(html)
                .iter()
                .any(|run| run.starts_with("1/implement-1: done"))
        })
        .await;
    let link = select(&html, "#runs li a")[0]
        .attr("href")
        .unwrap()
        .to_string();
    assert_eq!(link, "/runs/1/implement-1");
    let page = web.page(&link).await;
    (web, page)
}

/// Each of `shown`, cut to its first `chars` characters.
fn cut(shown: &[String], chars: usize) -> Vec<String> {
    shown
        .iter()
        .map(|line| line.chars().take(chars).collect())
        .collect()
}

#[tokio::test]
async fn the_run_page_shows_what_logs_shows_with_each_tool_call_folded_to_its_line() {
    let (web, html) = run_page_of_implemented().await;

    // As `lw logs 1` prints them.
    assert_eq!(
        cut(&transcript_shown(&html, false), 100),
        [
            "run: 1/implement-1",
            "session 1: started 2026-10-13 10:09:08 UTC",
            "0:03 Bash(ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"im…) → exit ",
            "0:14 Bash(which podman buildah nerdctl hadolint; ls /var/run/docker.sock 2>&1) → exit 2",
            "0:15 Skill(linewright:tdd) → loaded",
            "0:18 Can't build images here (no Docker), and the repo has no test seam for the Dockerfile, so I'll ",
            "0:27 Read(Dockerfile) → 16 lines",
            "0:35 Edit(Dockerfile) → edited",
            "2:00 Agent(Standards review) → done, 2 tool uses, 16s",
            "2:02 Bash(git diff origin/main...HEAD --stat && git diff origin/main...HEAD && git log -1…) → exit 0",
            "2:45 Both reviews came back clean. One optional fix: the README and the image label use the serial c",
            "2:59 {\"outcome\":\"done\",\"pr_title\":\"Ship gh and python3 in the base image\",\"pr_body\":\"## Summary\\n\\nT",
            "3:22 follow-up: Your final message could not be used: The final message holds no JSON object. End wi",
            "3:25 Bash(git commit -qam \"Ship gh and python3 in the base image…) → exit 0, 1 line",
            "3:28 {\"outcome\":\"done\",\"pr_title\":\"Ship gh and python3 in the base image\",\"pr_body\":\"## Summary\\n\\nT",
        ]
    );
    let calls = select(&html, "#transcript .entry > details.call");
    assert_eq!(calls.len(), 8);
    for call in &calls {
        let line = text(select_in(*call, "summary .line")[0]);
        let failed = line.ends_with("→ exit 2");
        assert_eq!(call.attr("open").is_some(), failed, "{line}");
        let io: Vec<String> = select_in(*call, ".io > *")
            .into_iter()
            .map(|part| text(part))
            .collect();
        assert!(
            io.first()
                .is_some_and(|input| input.starts_with("input: {")),
            "{line}: {io:?}"
        );
        assert!(
            io.get(1)
                .is_some_and(|result| result.starts_with("result: ")),
            "{line}: {io:?}"
        );
    }
    let counted = select(&html, "#transcript details.call > summary > .count");
    assert_eq!(counted.len(), 1);
    let agent_call = counted[0]
        .ancestors()
        .find_map(ElementRef::wrap)
        .and_then(|summary| ElementRef::wrap(summary.parent().unwrap()))
        .unwrap();
    assert_eq!(agent_call.attr("open"), None);
    assert_eq!(
        text(select_in(agent_call, "summary")[0]),
        "2:00 Agent(Standards review) → done, 2 tool uses, 16s 1 entry"
    );
    // Its prompt is context, which `logs` hides, and one tool call.
    let work: Vec<String> = entry_lines_in(agent_call)
        .into_iter()
        .map(|(_, line)| line)
        .collect();
    assert_eq!(cut(&work, 30), ["2:02 Bash(git diff origin/main"]);
    assert_no_status_by_colour_alone(&html);
    web.stop().await;
}

fn select_in<'a>(element: ElementRef<'a>, selector: &str) -> Vec<ElementRef<'a>> {
    element
        .select(&Selector::parse(selector).unwrap())
        .collect()
}

#[tokio::test]
async fn the_run_pages_verbose_switch_shows_what_logs_verbose_shows_with_everything_open() {
    let (web, html) = run_page_of_implemented().await;
    let switch = select(&html, "[role=switch]");
    assert_eq!(switch[0].attr("aria-checked"), Some("false"));
    let verbose_path = switch[0].attr("href").unwrap().to_string();
    assert_eq!(verbose_path, "/runs/1/implement-1?verbose=");

    let verbose = web.page(&verbose_path).await;

    // As `lw logs 1 --verbose` prints them.
    assert_eq!(
        cut(&transcript_shown(&verbose, true), 40),
        [
            "run: 1/implement-1",
            "session 1: started 2026-10-13 10:09:08 U",
            "0:00 brief: # Your task Implement the ti",
            "0:00 event: {\"analytics_disabled\":false,",
            "0:03 Bash(ls; cat Dockerfile; ls deploy;",
            "0:04 thinking: The listing has a Dockerf",
            "0:14 Bash(which podman buildah nerdctl h",
            "0:15 Skill(linewright:tdd) → loaded inpu",
            "0:15 context: Base directory for this sk",
            "0:18 Can't build images here (no Docker)",
            "0:27 Read(Dockerfile) → 16 lines input: ",
            "0:35 Edit(Dockerfile) → edited input: { ",
            "2:00 Agent(Standards review) → done, 2 t",
            "2:00 context: Review a diff in /srv/line",
            "2:02 Bash(git diff origin/main...HEAD --",
            "2:00 event: {\"description\":\"Standards re",
            "2:02 event: {\"output_file\":\"/srv/linewri",
            "2:45 Both reviews came back clean. One o",
            "2:59 {\"outcome\":\"done\",\"pr_title\":\"Ship ",
            "2:59 event: {\"api_error_status\":null,\"du",
            "3:22 follow-up: Your final message could",
            "3:22 event: {\"analytics_disabled\":false,",
            "3:25 Bash(git commit -qam \"Ship gh and p",
            "3:28 {\"outcome\":\"done\",\"pr_title\":\"Ship ",
            "3:28 event: {\"api_error_status\":null,\"du",
        ]
    );
    let shown = transcript_shown(&verbose, true);
    assert!(
        shown[4].contains(
            r#"→ exit 0, 19 lines input: { "command": "ls; cat Dockerfile; ls deploy; cat deploy/Dockerfile 2>/dev/null; grep -n -i \"image\\|git\\b\" README.md | head -50; cat CODING_STANDARDS.md | head -80" } result: AGENTS.md CODING_STANDARDS.md"#
        ),
        "{}",
        shown[4]
    );
    assert_eq!(
        shown[5],
        "0:04 thinking: The listing has a Dockerfile. I'll check what the image ships."
    );
    let folds = select(&verbose, "#transcript details");
    assert_eq!(folds.len(), 8);
    assert!(folds.iter().all(|fold| fold.attr("open").is_some()));
    let count = select(&verbose, "#transcript details.call > summary > .count");
    assert_eq!(text(count[0]), "2 entries");
    let back = select(&verbose, "[role=switch]");
    assert_eq!(back[0].attr("aria-checked"), Some("true"));
    assert_eq!(back[0].attr("href"), Some("/runs/1/implement-1"));
    web.stop().await;
}

/// Each event the stream at `path` sends, as its ID, if it has one, and
/// its data, resuming after the event `last_id` if one is given.
async fn stream_with_ids(
    web: &Web,
    path: &str,
    last_id: Option<&str>,
) -> tokio::sync::mpsc::UnboundedReceiver<(Option<String>, String)> {
    let mut request = axum::http::Request::get(path).header("Host", "localhost");
    if let Some(last_id) = last_id {
        request = request.header("Last-Event-ID", last_id);
    }
    let response = web
        .router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
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
                let id = block
                    .lines()
                    .find_map(|line| line.strip_prefix("id: "))
                    .map(str::to_string);
                let data: Vec<&str> = block
                    .lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .collect();
                if !data.is_empty() && sender.send((id, data.join("\n"))).is_err() {
                    return;
                }
            }
        }
    });
    receiver
}

/// The transcript's events, those with an ID, until none comes for a
/// while.
async fn transcript_events(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<(Option<String>, String)>,
    until: impl Fn(&[(String, String)]) -> bool,
) -> Vec<(String, String)> {
    let mut read = vec![];
    let waited = tokio::time::timeout(Duration::from_secs(10), async {
        while !until(&read) {
            let (id, data) = events.recv().await.unwrap();
            if let Some(id) = id {
                read.push((id, data));
            }
        }
    })
    .await;
    assert!(waited.is_ok(), "{read:?}");
    read
}

/// Each change an event makes to the page: where, how, and the markup.
fn patches(event: &str) -> Vec<(String, String, Html)> {
    let html = Html::parse_fragment(event);
    select(&html, "hx-partial")
        .into_iter()
        .map(|partial| {
            (
                partial.attr("hx-target").unwrap().to_string(),
                partial.attr("hx-swap").unwrap().to_string(),
                Html::parse_fragment(&partial.inner_html()),
            )
        })
        .collect()
}

/// Each entry's line, by its anchor.
fn entry_lines(html: &Html) -> Vec<(String, String)> {
    entry_lines_in(html.root_element())
}

fn entry_lines_in(element: ElementRef) -> Vec<(String, String)> {
    select_in(element, ".entry")
        .into_iter()
        .map(|entry| {
            let line = entry
                .select(&Selector::parse(".line").unwrap())
                .find(|line| entry_of(*line) == Some(entry))
                .map(text)
                .unwrap_or_default();
            (entry.attr("id").unwrap().to_string(), line)
        })
        .collect()
}

#[tokio::test]
async fn the_run_page_follows_a_run_in_flight_from_where_it_was_read_to_and_resumes_by_event_id() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let web = Web::start(world).await;
    let turn = crate::local_api::turn_1();
    // The marker, the start event and the first Bash call, with its result.
    agent.writes(&turn[..4].concat());
    let read = web
        .eventually_page("/runs/1/triage-1", |html| {
            entry_lines(html)
                .iter()
                .any(|(_, line)| line.contains("Bash(ls;"))
        })
        .await;
    let events_path = events_path(&read).unwrap().to_string();
    assert!(
        events_path.starts_with("/events/runs/1/triage-1?follow=1%2Ftriage-1%3A1."),
        "{events_path}"
    );
    let mut events = stream_with_ids(&web, &events_path, None).await;

    // The second Bash call, its result still to come.
    agent.writes(&turn[4..6].concat());
    let made = transcript_events(&mut events, |sent| !sent.is_empty()).await;
    agent.writes(&turn[6]);
    let returned = transcript_events(&mut events, |sent| !sent.is_empty()).await;
    agent.writes(&turn[7..].concat());
    drop(agent);
    web.eventually_page("/runs?item=1", |html| {
        runs_shown(html)
            .iter()
            .any(|run| run.starts_with("1/triage-1: done"))
    })
    .await;
    let rest = transcript_events(&mut events, |sent| {
        sent.iter().any(|(_, data)| {
            data.contains("{&quot;outcome&quot;") || data.contains(r#"{"outcome""#)
        })
    })
    .await;

    let [(_, made_data)] = &made[..] else {
        panic!("one event for the call made: {made:?}")
    };
    let [(added_to, added_by, call)] = &patches(made_data)[..] else {
        panic!("{made_data}")
    };
    assert_eq!(
        (added_to.as_str(), added_by.as_str()),
        ("#s1-entries", "beforeend")
    );
    let call_id = entry_lines(call)[0].0.clone();
    assert!(
        entry_lines(call)[0].1.ends_with("2>&1) → running"),
        "{:?}",
        entry_lines(call)
    );
    let [(_, returned_data)] = &returned[..] else {
        panic!("one event for the call returned: {returned:?}")
    };
    let [(replaced_at, replaced_by, account)] = &patches(returned_data)[..] else {
        panic!("{returned_data}")
    };
    assert_eq!(
        (replaced_at.as_str(), replaced_by.as_str()),
        (format!("#{call_id}-call").as_str(), "outerHTML")
    );
    let replaced = select(account, "details.call");
    assert!(replaced[0].attr("open").is_some());
    assert!(text(select_in(replaced[0], "summary")[0]).ends_with("2>&1) → exit 2"));
    // The account of a call that did not fail keeps its fold as the reader
    // left it.
    let accounts: Vec<Vec<(String, String)>> = rest
        .iter()
        .map(|(_, data)| {
            patches(data)
                .into_iter()
                .map(|(at, by, _)| (at, by))
                .collect()
        })
        .filter(|patches: &Vec<(String, String)>| patches.iter().any(|(_, by)| by == "outerHTML"))
        .collect();
    let replaced_line = |entry: &str| {
        vec![
            (format!("#{entry}-line"), "outerHTML".to_string()),
            (format!("#{entry}-io"), "outerHTML".to_string()),
        ]
    };
    assert!(accounts.contains(&replaced_line("s1-e6")), "{accounts:?}");

    // Followed in pieces, the page is as it reads whole.
    let followed: Vec<(String, String)> = [made, returned, rest.clone()].concat();
    let mut shown = entry_lines(&read);
    for (_, data) in &followed {
        for (target, swap, html) in patches(data) {
            if swap == "beforeend" {
                for (id, line) in entry_lines(&html) {
                    assert!(
                        shown.iter().all(|(shown, _)| *shown != id),
                        "{id} sent twice"
                    );
                    shown.push((id, line));
                }
            } else if let Some(entry) = target.strip_prefix('#').and_then(|id| {
                id.strip_suffix("-call")
                    .or_else(|| id.strip_suffix("-line"))
            }) {
                let line = text(select(&html, ".line")[0]);
                shown.iter_mut().find(|(id, _)| id == entry).unwrap().1 = line;
            }
        }
    }
    let whole = web.page("/runs/1/triage-1").await;
    let mut shown_sorted = shown.clone();
    shown_sorted.sort();
    let mut whole_sorted = entry_lines(&whole);
    whole_sorted.sort();
    assert_eq!(shown_sorted, whole_sorted);

    // Resumed after its first event, the stream sends the rest, once.
    let first = followed[0].0.clone();
    let mut resumed = stream_with_ids(&web, &events_path, Some(&first)).await;
    let again = transcript_events(&mut resumed, |sent| sent.len() >= followed.len() - 1).await;
    assert_eq!(again, followed[1..]);
    let more = tokio::time::timeout(Duration::from_millis(1500), async {
        loop {
            if let (Some(id), _) = resumed.recv().await.unwrap() {
                return id;
            }
        }
    })
    .await;
    assert!(more.is_err(), "sent more: {more:?}");
    web.stop().await;
}

fn heading(html: &Html) -> Vec<String> {
    select(html, "#transcript h1, #transcript h2")
        .into_iter()
        .map(text)
        .collect()
}

#[tokio::test]
async fn an_automations_run_whose_id_holds_a_slash_has_its_page() {
    let world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
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
    world.harness.on_automation(|_| {
        Ok(serde_json::json!({"summary": "Nothing needs doing.", "issues": []}).to_string())
    });
    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();
    let web = Web::start(world).await;

    let runs = web.page("/runs").await;
    let html = web.page("/runs/weekly/1").await;

    let links: Vec<&str> = select(&runs, "#runs li a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(links, ["/runs/weekly/1"]);
    let shown = heading(&html);
    assert_eq!(shown[0], "run: weekly/1");
    assert!(shown[1].starts_with("session 1"), "{shown:?}");
    web.stop().await;
}

#[tokio::test]
async fn the_page_of_a_run_that_started_no_session_says_so() {
    let mut world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    let (_, pr) = world.ticket_in_review().await;
    world
        .store
        .merge_change(
            "maintainer",
            "Other work",
            "",
            &[("src/other.txt", "other\n")],
        )
        .await
        .unwrap();
    // Brought up to date without a session: nothing conflicts.
    world.poll().await;
    let web = Web::start(world).await;

    let html = web.page(&format!("/runs/{}/update-1", pr.0)).await;

    assert_eq!(
        heading(&html),
        [format!("run: {}/update-1 (it started no session)", pr.0)]
    );
    assert_eq!(select(&html, "#transcript .entry").len(), 0);
    assert_eq!(events_path(&html), Some("/events/runs/2/update-1"));
    web.stop().await;
}

#[tokio::test]
async fn the_page_of_no_run_says_there_is_none() {
    let web = Web::start(with_ticket().await).await;

    let (no_run, no_run_body) = web.get("/runs/1/rework-9").await;
    let (no_id, no_id_body) = web.get("/runs/nothing").await;

    for (status, body, error) in [
        (no_run, no_run_body, "there is no run 1/rework-9"),
        (no_id, no_id_body, "\"nothing\" is not a run ID"),
    ] {
        assert_eq!(status, 404);
        let html = Html::parse_document(&body);
        let shown: Vec<String> = select(&html, "#transcript p")
            .into_iter()
            .map(text)
            .collect();
        assert_eq!(shown, [error]);
    }
    web.stop().await;
}
