use super::*;

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

#[tokio::test]
async fn the_item_page_of_an_item_linewright_leaves_says_why() {
    let world = with_ticket().await;
    let closed = world.store.open_issue(
        "maintainer",
        "Old idea",
        "Maybe.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.close_issue_as_completed(closed).unwrap();
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
    assert_eq!(select(&html, ".drawer-side nav").len(), 1);
    web.stop().await;
}

#[tokio::test]
async fn the_item_page_shows_the_run_in_flight_ticking_as_a_row_and_then_the_run_past() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let web = Web::start(world).await;

    let running_html = web
        .eventually_page("/items/1", |html| {
            has_run(&runs_shown(html), "in flight", "1/triage-1")
        })
        .await;
    assert_eq!(
        runs_heads(&running_html),
        ["Outcome", "Run", "Stage", "Started", "Duration"]
    );
    let running = select(&running_html, "#runs tbody td:last-child time[data-since]");
    assert!(
        running.first().is_some_and(|time| time
            .attr("data-since")
            .unwrap()
            .starts_with("2026-10-01T12:")),
        "{}",
        running_html.html()
    );
    let facts = item_lines(&running_html);
    assert!(
        !facts.iter().any(|line| line.starts_with("run in flight")),
        "{facts:?}"
    );

    drop(agent);
    let html = web
        .eventually_page("/items/1", |html| {
            has_run(&runs_shown(html), "done", "1/triage-1")
        })
        .await;
    let shown = runs_shown(&html);
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0][..3], ["done", "1/triage-1", "triage"]);
    let done: Vec<ElementRef> = select(&html, "#runs .tone.badge-success");
    assert_eq!(done.into_iter().map(text).collect::<Vec<_>>(), ["done"]);
    let lines = item_lines(&html);
    assert!(
        !lines.iter().any(|line| line.starts_with("past run")),
        "{lines:?}"
    );
    assert_no_status_by_colour_alone(&html);
    web.stop().await;
}

#[tokio::test]
async fn the_item_page_shows_the_latest_ten_runs_and_links_the_rest_to_the_runs_page() {
    let web = Web::start(with_ticket().await).await;
    for n in 1..=12 {
        record_interrupted(&web, 1, n, &format!("2026-10-01T{:02}:00:00Z", n - 1));
    }

    let html = web.page("/items/1").await;

    let shown = runs_shown(&html);
    assert_eq!(shown.len(), 11, "{shown:?}");
    assert_eq!(
        shown[0],
        [
            "interrupted",
            "1/triage-12",
            "triage",
            "2026-10-01 11:00:00 UTC",
            ""
        ]
    );
    assert_eq!(shown[9][1], "1/triage-3");
    assert_eq!(shown[10], ["left out: 2 earlier runs; show them all"]);
    let last = select(&html, "#runs tbody tr:last-child > td");
    assert_eq!(last[0].attr("colspan"), Some("5"));
    let more = select(&html, "#runs tbody tr:last-child a");
    assert_eq!(more[0].attr("href"), Some("/runs?item=1"));
    let tables = classes(&html, "#runs .scroll-box > table");
    assert!(
        tables[0].contains(&"table") && tables[0].contains(&"table-pin-rows"),
        "{tables:?}"
    );
    assert_eq!(select(&html, "#item ~ #runs .scroll-box").len(), 1);
    web.stop().await;
}

#[tokio::test]
async fn the_item_page_of_an_item_with_no_runs_says_so() {
    let web = Web::start(with_ticket().await).await;

    let html = web.page("/items/1").await;

    assert_eq!(text(select(&html, "#runs")[0]), "No runs");
    assert_eq!(
        select(&html, "#runs table"),
        [] as [scraper::ElementRef<'_>; 0]
    );
    web.stop().await;
}

#[tokio::test]
async fn the_item_pages_stream_sends_the_runs_of_an_item_linewright_leaves_as_they_change() {
    let world = with_ticket().await;
    let closed = world
        .store
        .open_issue("maintainer", "Old idea", "Maybe.", &["ready-for-agent"]);
    world.store.close_issue_as_completed(closed).unwrap();
    let web = Web::start(world).await;
    let mut events = stream(&web, &format!("/events/items/{}", closed.0)).await;
    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let none = regions(&first)
        .into_iter()
        .find(|(id, _)| id == "runs")
        .map(|(_, html)| text(select(&html, "#runs")[0]));
    assert_eq!(none.as_deref(), Some("No runs"));

    record_interrupted(&web, closed.0, 1, "2026-10-01T11:00:00Z");
    let changed = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    let runs = regions(&changed)
        .into_iter()
        .find(|(id, _)| id == "runs")
        .map(|(_, html)| runs_shown(&html))
        .expect("the runs' region");
    assert_eq!(
        runs,
        [[
            "interrupted",
            &format!("{}/triage-1", closed.0),
            "triage",
            "2026-10-01 11:00:00 UTC",
            ""
        ]]
    );
    web.stop().await;
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
    assert_eq!(ids, ["holds", "item", "runs"]);
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
async fn the_item_pages_stream_sends_nothing_when_its_runs_agent_produces_output() {
    let world = with_issue_for_triage().await;
    let agent = world.harness.hold();
    let web = Web::start(world).await;
    web.eventually_page("/items/1", |html| {
        has_run(&runs_shown(html), "in flight", "1/triage-1")
    })
    .await;
    let mut events = stream(&web, "/events/items/1").await;
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    agent.writes("Reading the issue.\n");

    let heard = tokio::time::timeout(Duration::from_millis(2500), events.recv()).await;
    assert!(heard.is_err(), "sent on output alone: {heard:?}");
    drop(agent);
    web.stop().await;
}
