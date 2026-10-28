use super::*;

/// The fact labeled `label` among `facts`.
fn fact<'a>(facts: &'a [(String, String)], label: &str) -> &'a str {
    facts
        .iter()
        .find(|(shown, _)| shown == label)
        .map(|(_, value)| value.as_str())
        .unwrap_or_else(|| panic!("no {label}: {facts:?}"))
}

#[tokio::test]
async fn the_item_pages_facts_are_a_card_of_labels_in_the_specs_order_with_the_state_in_the_boards_terms()
 {
    let web = Web::start(with_ticket().await).await;

    let html = web.page("/items/1").await;

    // `docs/specs/design_system.md`, The item page, Facts and Cards: the
    // facts are labels beside their values, drawn as a card is; the state
    // is the column and the card's line; the waits are plain text.
    assert_eq!(select(&html, "#item dl.card").len(), 1, "{}", html.html());
    assert_in_muted_text(&html, "#item dt");
    let facts = item_facts(&html);
    let labels: Vec<&str> = facts.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(labels, ["State", "Waiting for"]);
    let state = fact(&facts, "State");
    assert!(
        state.starts_with("Needs a person · decision needed · 2026-"),
        "{state}"
    );
    let decision = select(&html, "#item dd")[0];
    assert_eq!(
        select_in(decision, ".status-text svg.icon").len(),
        1,
        "{}",
        decision.html()
    );
    assert_eq!(
        fact(&facts, "Waiting for"),
        "the maintainer: to label it `ready-for-agent` or `needs-triage`"
    );
    let waits = select(&html, "#item dd")[1];
    assert!(select_in(waits, "svg").is_empty(), "{}", waits.html());
    assert!(
        select(&html, "#item dl .badge").is_empty(),
        "{}",
        html.html()
    );
    assert_no_status_by_colour_alone(&html);
    web.stop().await;
}

#[tokio::test]
async fn the_item_pages_heading_is_its_number_and_title_linked_and_when_serve_last_read_it() {
    let web = Web::start(with_ticket().await).await;

    let html = web.page("/items/1").await;

    assert_eq!(
        select(&html, "#item h1")
            .into_iter()
            .map(text)
            .collect::<Vec<_>>(),
        ["#1: Do the thing"]
    );
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
    let as_of = select(&html, "#item .as-of");
    assert_eq!(as_of.len(), 1, "{}", html.html());
    assert!(
        text(as_of[0]).starts_with("as of 2026-10-01 12:"),
        "{}",
        as_of[0].html()
    );
    assert_in_muted_text(&html, "#item .as-of");
    let at = select_in(as_of[0], "time")[0].attr("datetime").unwrap();
    assert!(
        at.starts_with("2026-10-01T12:") && at.ends_with('Z'),
        "{at}"
    );
    web.stop().await;
}

#[tokio::test]
async fn the_item_page_of_an_item_linewright_leaves_says_left_alone_and_why() {
    let world = with_ticket().await;
    let unlabeled = world
        .store
        .open_issue("maintainer", "Old idea", "Maybe.", &[]);
    let web = Web::start(world).await;

    let html = web.page(&format!("/items/{}", unlabeled.0)).await;

    let facts = item_facts(&html);
    assert_eq!(
        fact(&facts, "State"),
        "Left alone · labeled none of `needs-triage`, `needs-info`, `ready-for-agent` or \
         `ready-for-human`"
    );
    assert!(
        select(&html, "#item dl .badge").is_empty(),
        "{}",
        html.html()
    );
    web.stop().await;
}

#[tokio::test]
async fn the_item_page_of_an_issue_closed_in_the_window_shows_it_in_done() {
    let world = with_ticket().await;
    let closed = world.store.open_issue(
        LINEWRIGHT,
        "Old idea",
        "Maybe.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.close_issue_as_completed(closed).unwrap();
    let web = Web::start(world).await;

    let html = web.page(&format!("/items/{}", closed.0)).await;

    let state = fact(&item_facts(&html), "State").to_string();
    assert!(
        state.starts_with("Done · closed as completed · 2026-"),
        "{state}"
    );
    web.stop().await;
}

#[tokio::test]
async fn with_a_required_label_the_item_page_of_a_closed_issue_carrying_it_linewright_never_worked_says_left_alone()
 {
    let world = World::with(|c| c.require_label = Some(REQUIRED.into())).await;
    let closed = world.store.open_issue(
        "maintainer",
        "Old idea",
        "Maybe.",
        &["ready-for-agent", REQUIRED],
    );
    world.store.close_issue_as_completed(closed).unwrap();
    let web = Web::start(world).await;

    let html = web.page(&format!("/items/{}", closed.0)).await;

    assert_eq!(fact(&item_facts(&html), "State"), "Left alone · closed");
    web.stop().await;
}

#[tokio::test]
async fn with_a_required_label_the_item_page_of_a_closed_issue_without_it_linewright_never_worked_says_left_alone()
 {
    let world = World::with(|c| c.require_label = Some(REQUIRED.into())).await;
    let closed = world
        .store
        .open_issue("maintainer", "Old idea", "Maybe.", &["ready-for-agent"]);
    world.store.close_issue_as_completed(closed).unwrap();
    let web = Web::start(world).await;

    let html = web.page(&format!("/items/{}", closed.0)).await;

    assert_eq!(fact(&item_facts(&html), "State"), "Left alone · closed");
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
    let facts = item_facts(&running_html);
    assert!(
        !facts.iter().any(|(label, _)| label.contains("run")),
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
    let done_facts = item_facts(&html);
    assert!(
        !done_facts.iter().any(|(label, _)| label.contains("run")),
        "{done_facts:?}"
    );
    assert_no_status_by_colour_alone(&html);
    web.stop().await;
}

#[tokio::test]
async fn the_item_page_shows_the_latest_ten_runs_and_links_to_all_of_them_on_the_runs_page() {
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
    let all = runs_shown(&web.page("/runs?item=1").await);
    assert_eq!(all.len(), 12, "{all:?}");
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
        .map(|(_, html)| item_facts(&html))
        .expect("the item's region");
    let state = fact(&item, "State");
    assert!(
        state.starts_with("Needs a person · people asked · 2026-"),
        "{state}"
    );
    assert_eq!(
        fact(&item, "Waiting for"),
        "the people asked: labeled `needs-info`"
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

#[tokio::test]
async fn the_item_page_names_a_blocker_it_waits_for_by_its_number_and_title() {
    let world = with_ticket().await;
    let blocker = world.store.open_issue(
        "maintainer",
        "Fix the parser",
        "First.",
        &["ready-for-human"],
    );
    let blocked = world.store.open_issue(
        "maintainer",
        "Then",
        "After.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.add_blocked_by(blocked, blocker).await.unwrap();
    let web = Web::start(world).await;

    let html = web
        .eventually_page(&format!("/items/{}", blocked.0), |html| {
            item_facts(html)
                .iter()
                .any(|(label, _)| label == "Waiting for")
        })
        .await;

    // `docs/specs/design_system.md`, Navigation: named by its number and
    // its title, linking to its page and to GitHub.
    assert_eq!(
        fact(&item_facts(&html), "Waiting for"),
        "#2: Fix the parser, which blocks it"
    );
    let links: Vec<&str> = select(&html, "#item dd")[1]
        .select(&Selector::parse("a").unwrap())
        .map(|a| a.attr("href").unwrap())
        .collect();
    assert_eq!(
        links,
        ["/items/2", "https://github.com/owner/widgets/issues/2"]
    );
    // A card's line names it by its number alone.
    assert!(
        fact(&item_facts(&html), "State").starts_with("Ready · blocked by #2 · 2026-"),
        "{html:?}",
        html = item_facts(&html)
    );
    web.stop().await;
}

#[tokio::test]
async fn the_item_page_names_a_blocker_the_snapshot_does_not_hold_by_its_number_alone() {
    let world = with_ticket().await;
    let first = world
        .store
        .open_issue("maintainer", "First", "Before.", &["ready-for-human"]);
    let blocked = world.store.open_issue(
        "maintainer",
        "Then",
        "After.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.add_blocked_by(blocked, first).await.unwrap();
    let web = Web::start(world).await;
    web.page("/").await;
    // Opened since the poll, and read only as #3's blocker.
    let blocker = web
        .store
        .open_issue("maintainer", "Fix the parser", "Too.", &[]);
    web.store.add_blocked_by(blocked, blocker).await.unwrap();
    web.deliver_issue(blocked.0).await;

    let html = web
        .eventually_page(&format!("/items/{}", blocked.0), |html| {
            fact(&item_facts(html), "Waiting for").contains("#4")
        })
        .await;

    let waits: Vec<String> = select(&html, "#item dd")[1]
        .select(&Selector::parse("div").unwrap())
        .map(text)
        .collect();
    assert_eq!(waits, ["#2: First, which blocks it", "#4, which blocks it"]);
    web.stop().await;
}

/// `with_ticket`'s, and #2, cut into #3, which waits on the maintainer.
async fn with_a_lane() -> World {
    let world = with_ticket().await;
    let tracking = world.store.open_issue(
        "maintainer",
        "Big thing",
        "Cut it.",
        &["linewright:designed"],
    );
    let part = world
        .store
        .open_issue(LINEWRIGHT, "Part one", "Of it.", &["ready-for-human"]);
    world.store.add_sub_issue(tracking, part).await.unwrap();
    world
}

/// `docs/specs/design_system.md`, The item page: a tracking issue's State is
/// "Tracking", with no status of its lane's own (Out of scope).
#[tokio::test]
async fn an_unfinished_tracking_issues_state_is_tracking_alone() {
    let web = Web::start(with_a_lane().await).await;

    let html = web.page("/items/2").await;

    assert_eq!(fact(&item_facts(&html), "State"), "Tracking");
    let value = select(&html, "#item dd")[0];
    assert!(select_in(value, "svg").is_empty(), "{}", value.html());
    web.stop().await;
}

/// `docs/specs/design_system.md`, The item page, Related items: a tracking
/// issue is shown as its lane head is, with no card status's icon.
#[tokio::test]
async fn a_tickets_tracking_issue_is_shown_as_its_lane_head_is() {
    let web = Web::start(with_a_lane().await).await;

    let board = web.page("/").await;
    let html = web.page("/items/3").await;

    let head = text(select(&board, "#lane-2 .lane-head")[0]);
    assert_eq!(head, "#2: Big thing — 0/1 tickets closed");
    let facts = item_facts(&html);
    assert_eq!(fact(&facts, "Tracking issue"), head);
    let at = facts
        .iter()
        .position(|(label, _)| label == "Tracking issue")
        .unwrap();
    let value = select(&html, "#item dd")[at];
    assert!(select_in(value, "svg").is_empty(), "{}", value.html());
    let links: Vec<&str> = select_in(value, "a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(
        links,
        ["/items/2", "https://github.com/owner/widgets/issues/2"]
    );
    web.stop().await;
}

#[tokio::test]
async fn a_finished_tracking_issues_state_is_tracking_and_when_it_finished() {
    let world = with_a_lane().await;
    world
        .store
        .close_issue_as_completed(linewright::domain::Number(3))
        .unwrap();
    let web = Web::start(world).await;

    let html = web.page("/items/2").await;

    let state = fact(&item_facts(&html), "State").to_string();
    assert!(state.starts_with("Tracking · finished 2026-"), "{state}");
    web.stop().await;
}

#[tokio::test]
async fn a_tracking_issue_whose_lane_left_the_board_is_tracking_and_when_it_finished() {
    let world = with_a_lane().await;
    world
        .store
        .close_issue_as_completed(linewright::domain::Number(3))
        .unwrap();
    // Past the Done window, so that #2's lane leaves the board.
    world.clock.advance(chrono::TimeDelta::days(8));
    let web = Web::start(world).await;

    let html = web.page("/items/2").await;

    let state = fact(&item_facts(&html), "State").to_string();
    assert!(state.starts_with("Tracking · finished 2026-"), "{state}");
    web.stop().await;
}

#[tokio::test]
async fn the_item_page_names_the_items_a_left_alone_reason_names_by_number_and_title() {
    let world = with_ticket().await;
    for (branch, title) in [("first", "First try"), ("second", "Second try")] {
        world
            .store
            .open_pull_as(
                LINEWRIGHT,
                branch,
                title,
                "Closes #1.",
                &[(&format!("src/{branch}.txt"), "try\n")],
            )
            .await
            .unwrap();
    }
    let web = Web::start(world).await;

    let html = web.page("/items/2").await;

    assert_eq!(
        fact(&item_facts(&html), "State"),
        "Left alone · for #1: Do the thing, which Linewright works on through #3: Second try"
    );
    web.stop().await;
}

#[tokio::test]
async fn the_item_pages_card_line_names_its_pull_request_by_number_alone_and_its_waits_by_title() {
    let world = with_ticket().await;
    let pr = world
        .store
        .open_pull_as(
            LINEWRIGHT,
            "first",
            "First try",
            "Closes #1.",
            &[("src/first.txt", "try\n")],
        )
        .await
        .unwrap();
    world
        .store
        .set_checks(
            pr,
            vec![check("test", linewright::domain::CheckState::Pending, "")],
        )
        .await
        .unwrap();
    let web = Web::start(world).await;

    let facts = item_facts(&web.page("/items/1").await);

    assert_eq!(fact(&facts, "State"), "In review · #2 · checks running");
    assert_eq!(
        fact(&facts, "Waiting for"),
        "#2: First try, its pull request, until it is merged or closed"
    );
    web.stop().await;
}

#[tokio::test]
async fn the_item_pages_stream_names_a_blocker_by_its_number_and_title() {
    let world = with_ticket().await;
    let blocker = world.store.open_issue(
        "maintainer",
        "Fix the parser",
        "First.",
        &["ready-for-human"],
    );
    let blocked = world.store.open_issue(
        "maintainer",
        "Then",
        "After.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.add_blocked_by(blocked, blocker).await.unwrap();
    let web = Web::start(world).await;
    let mut events = stream(&web, &format!("/events/items/{}", blocked.0)).await;

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    let item = regions(&first)
        .into_iter()
        .find(|(id, _)| id == "item")
        .map(|(_, html)| item_facts(&html))
        .expect("the item's region");
    assert_eq!(
        fact(&item, "Waiting for"),
        "#2: Fix the parser, which blocks it"
    );
    web.stop().await;
}
