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
    assert_eq!(select(&html, ".drawer-side nav").len(), 1);
    web.stop().await;
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
    let status: Vec<String> = select(&running_html, "#item .tone")
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
    let done: Vec<ElementRef> = select(&html, "#item .tone.badge-success");
    assert_eq!(done.into_iter().map(text).collect::<Vec<_>>(), ["done"]);
    assert_no_status_by_colour_alone(&html);
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
    assert_eq!(ids, ["status-line", "holds", "item"]);
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
    eventually_shown(&web, "/items/1", |lines| {
        lines.iter().any(|line| line.starts_with("run in flight: "))
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
