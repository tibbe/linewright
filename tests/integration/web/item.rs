use super::*;

use linewright::domain::{Check, CheckState, Issue, Linked, Number, Parent, Pull, label};
use serde_json::json;

/// The fact labeled `label` among `facts`.
fn fact<'a>(facts: &'a [(String, String)], label: &str) -> &'a str {
    facts
        .iter()
        .find(|(shown, _)| shown == label)
        .map(|(_, value)| value.as_str())
        .unwrap_or_else(|| panic!("no {label}: {facts:?}"))
}

/// Issue #`number`, which Linewright labeled `labels` as it was opened at
/// 09:00 on 2026-09-30.
fn labeled(number: u64, title: &str, labels: &[&str]) -> Issue {
    labeled_by_linewright(Issue {
        labels: labels.iter().map(ToString::to_string).collect(),
        ..ticket(number, title)
    })
}

/// #1, which waits on the maintainer.
fn for_the_maintainer() -> linewright::status::View {
    view(
        vec![labeled(1, "Do the thing", &["ready-for-human"])],
        vec![],
    )
}

#[tokio::test]
async fn the_item_pages_facts_are_a_card_of_labels_in_the_specs_order_with_the_state_in_the_boards_terms()
 {
    let pages = Pages::of(for_the_maintainer()).await;

    let html = pages.page("/items/1").await;

    // `docs/specs/design_system.md`, The item page, Facts and Cards: the
    // facts are labels beside their values, drawn as a card is; the state
    // is the column and the card's line; the waits are plain text.
    assert_eq!(select(&html, "#item dl.card").len(), 1, "{}", html.html());
    assert_in_muted_text(&html, "#item dt");
    assert_eq!(
        item_facts(&html),
        [
            (
                "State".to_string(),
                "Needs a person · decision needed · 2026-09-30 09:00:00 UTC".to_string()
            ),
            (
                "Waiting for".to_string(),
                "the maintainer: to label it `ready-for-agent` or `needs-triage`".to_string()
            ),
        ]
    );
    let decision = select(&html, "#item dd")[0];
    assert_eq!(
        select_in(decision, ".status-text svg.icon").len(),
        1,
        "{}",
        decision.html()
    );
    let waits = select(&html, "#item dd")[1];
    assert!(select_in(waits, "svg").is_empty(), "{}", waits.html());
    assert!(
        select(&html, "#item dl .badge").is_empty(),
        "{}",
        html.html()
    );
    assert_no_status_by_colour_alone(&html);
}

#[tokio::test]
async fn the_item_pages_heading_is_its_number_and_title_linked_and_when_serve_last_read_it() {
    let pages = Pages::of(for_the_maintainer()).await;

    let html = pages.page("/items/1").await;

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
    assert_eq!(text(as_of[0]), "as of 2026-10-01 12:00:00 UTC");
    assert_in_muted_text(&html, "#item .as-of");
    assert_eq!(
        select_in(as_of[0], "time")[0].attr("datetime"),
        Some("2026-10-01T12:00:00Z")
    );
}

#[tokio::test]
async fn the_item_page_of_an_item_linewright_leaves_says_left_alone_and_why() {
    let pages = Pages::of(view(vec![], vec![])).await;
    let unlabeled = pages
        .store
        .open_issue("maintainer", "Old idea", "Maybe.", &[]);

    let html = pages.page(&format!("/items/{}", unlabeled.0)).await;

    assert_eq!(
        fact(&item_facts(&html), "State"),
        "Left alone · labeled none of `needs-triage`, `needs-info`, `ready-for-agent` or \
         `ready-for-human`"
    );
    assert!(
        select(&html, "#item dl .badge").is_empty(),
        "{}",
        html.html()
    );
}

#[tokio::test]
async fn the_item_page_of_no_item_says_there_is_none() {
    let pages = Pages::of(for_the_maintainer()).await;

    let (status, body) = pages.get("/items/99").await;

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
}

#[tokio::test]
async fn the_item_page_shows_the_run_in_flight_ticking_as_a_row_and_then_the_run_past() {
    let pages = Pages::of(linewright::status::View {
        in_flight: vec![in_flight("1/triage-1", "2026-10-01T12:00:00Z")],
        ..for_the_maintainer()
    })
    .await;
    let mut record = json!({
        "item": 1,
        "stage": "triage",
        "n": 1,
        "started_at": "2026-10-01T12:00:00Z",
        "end": null,
        "sessions": [],
    });
    pages.record(&record);
    let in_flight = lock_of(&pages.path("runs/items/1/triage-1.json"));

    let running_html = pages.page("/items/1").await;
    record["end"] = json!({"at": "2026-10-01T12:01:00Z", "ending": "done"});
    pages.record(&record);
    drop(in_flight);
    pages.publish(for_the_maintainer());
    let html = pages.page("/items/1").await;

    assert_eq!(
        runs_heads(&running_html),
        ["Outcome", "Run", "Stage", "Started", "Duration"]
    );
    let running = runs_shown(&running_html);
    assert_eq!(running.len(), 1, "{running:?}");
    assert_eq!(
        running[0][..4],
        [
            "in flight",
            "1/triage-1",
            "triage",
            "2026-10-01 12:00:00 UTC"
        ]
    );
    let ticking = select(&running_html, "#runs tbody td:last-child time[data-since]");
    assert_eq!(
        ticking.first().and_then(|time| time.attr("data-since")),
        Some("2026-10-01T12:00:00Z"),
        "{}",
        running_html.html()
    );
    assert_eq!(
        runs_shown(&html),
        [[
            "done",
            "1/triage-1",
            "triage",
            "2026-10-01 12:00:00 UTC",
            "1m"
        ]]
    );
    let done: Vec<ElementRef> = select(&html, "#runs .tone.badge-success");
    assert_eq!(done.into_iter().map(text).collect::<Vec<_>>(), ["done"]);
    for shown in [&running_html, &html] {
        let facts = item_facts(shown);
        assert!(
            !facts.iter().any(|(label, _)| label.contains("run")),
            "{facts:?}"
        );
    }
    assert_no_status_by_colour_alone(&html);
}

#[tokio::test]
async fn the_item_page_shows_the_latest_ten_runs_and_links_to_all_of_them_on_the_runs_page() {
    let pages = Pages::of(for_the_maintainer()).await;
    for n in 1..=12 {
        record_interrupted(&pages, 1, n, &format!("2026-10-01T{:02}:00:00Z", n - 1));
    }

    let html = pages.page("/items/1").await;

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
    let all = runs_shown(&pages.page("/runs?item=1").await);
    assert_eq!(all.len(), 12, "{all:?}");
    let tables = classes(&html, "#runs .scroll-box > table");
    assert!(
        tables[0].contains(&"table") && tables[0].contains(&"table-pin-rows"),
        "{tables:?}"
    );
    assert_eq!(select(&html, "#item ~ #runs .scroll-box").len(), 1);
}

#[tokio::test]
async fn the_item_page_of_an_item_with_no_runs_says_so() {
    let pages = Pages::of(for_the_maintainer()).await;

    let html = pages.page("/items/1").await;

    assert_eq!(text(select(&html, "#runs")[0]), "No runs");
    assert_eq!(
        select(&html, "#runs table"),
        [] as [scraper::ElementRef<'_>; 0]
    );
}

#[tokio::test]
async fn the_item_pages_stream_sends_the_runs_of_an_item_linewright_leaves_as_they_change() {
    let pages = Pages::of(view(vec![], vec![])).await;
    let closed = pages
        .store
        .open_issue("maintainer", "Old idea", "Maybe.", &["ready-for-agent"]);
    pages.store.close_issue_as_completed(closed).unwrap();
    let mut events = stream(&pages, &format!("/events/items/{}", closed.0)).await;
    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let none = regions(&first)
        .into_iter()
        .find(|(id, _)| id == "runs")
        .map(|(_, html)| text(select(&html, "#runs")[0]));
    assert_eq!(none.as_deref(), Some("No runs"));

    record_interrupted(&pages, closed.0, 1, "2026-10-01T11:00:00Z");
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
}

#[tokio::test]
async fn the_item_pages_stream_sends_every_region_then_those_serves_view_changed() {
    let web = Web::start(with_ticket().await).await;
    let mut events = stream(&web, "/events/items/1").await;

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let first = regions(&first);
    let ids: Vec<&str> = first.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["holds", "item", "runs"]);
    let before = item_facts(&first[1].1);
    let quiet = tokio::time::timeout(Duration::from_millis(2500), events.recv()).await;
    assert!(quiet.is_err(), "sent with nothing changed: {quiet:?}");

    web.store
        .unlabel_as(linewright::domain::Number(1), "ready-for-human")
        .unwrap();
    web.store
        .label_as("maintainer", linewright::domain::Number(1), "needs-info")
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
    // What a poll after the delivery reads, which the page shows too.
    assert_ne!(fact(&item, "State"), fact(&before, "State"));
    assert_eq!(item, item_facts(&web.page("/items/1").await));
    web.stop().await;
}

#[tokio::test]
async fn the_item_pages_stream_sends_nothing_when_its_runs_agent_produces_output() {
    let pages = Pages::of(linewright::status::View {
        in_flight: vec![in_flight("1/triage-1", "2026-10-01T12:00:00Z")],
        ..for_the_maintainer()
    })
    .await;
    let output = "sessions/issue-1/20261001T120000Z-triage/output.jsonl";
    let turn = json!({"type": "linewright-turn", "started_at": "2026-10-01T12:00:00Z", "started_by": "brief"});
    pages.write(&[(output, &output_of(&[turn]))]);
    pages.record(&json!({
        "item": 1,
        "stage": "triage",
        "n": 1,
        "started_at": "2026-10-01T12:00:00Z",
        "end": null,
        "sessions": ["sessions/issue-1/20261001T120000Z-triage"],
    }));
    let _in_flight = lock_of(&pages.path("runs/items/1/triage-1.json"));
    let mut events = stream(&pages, "/events/items/1").await;
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    let message = json!({
        "type": "assistant",
        "timestamp": "2026-10-01T12:00:05Z",
        "message": {"content": [{"type": "text", "text": "Reading the issue."}]},
    });
    pages.append(output, &output_of(&[message]));

    let heard = tokio::time::timeout(Duration::from_millis(2500), events.recv()).await;
    assert!(heard.is_err(), "sent on output alone: {heard:?}");
}

/// #2, waiting on the maintainer, which blocks #3, ready, and, if
/// `unheld` is set, so does #4, which the snapshot does not hold.
fn blocked(unheld: bool) -> linewright::status::View {
    let mut blockers = vec![Linked {
        number: Number(2),
        open: true,
    }];
    if unheld {
        blockers.push(Linked {
            number: Number(4),
            open: true,
        });
    }
    view(
        vec![
            labeled(2, "Fix the parser", &["ready-for-human"]),
            Issue {
                blocked_by: blockers,
                ..ticket(3, "Then")
            },
        ],
        vec![],
    )
}

#[tokio::test]
async fn the_item_page_names_a_blocker_it_waits_for_by_its_number_and_title() {
    let pages = Pages::of(blocked(false)).await;

    let html = pages.page("/items/3").await;

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
    assert_eq!(
        fact(&item_facts(&html), "State"),
        "Ready · blocked by #2 · 2026-09-30 09:00:00 UTC"
    );
}

#[tokio::test]
async fn the_item_page_names_a_blocker_the_snapshot_does_not_hold_by_its_number_alone() {
    let pages = Pages::of(blocked(true)).await;

    let html = pages.page("/items/3").await;

    let waits: Vec<String> = select(&html, "#item dd")[1]
        .select(&Selector::parse("div").unwrap())
        .map(text)
        .collect();
    assert_eq!(
        waits,
        ["#2: Fix the parser, which blocks it", "#4, which blocks it"]
    );
}

/// #2, cut into #3, which waits on the maintainer.
fn a_lane() -> linewright::status::View {
    view(
        vec![
            Issue {
                labels: vec![label::DESIGNED.into()],
                sub_issues: vec![sub_issue(3, true, LINEWRIGHT)],
                ..ticket(2, "Big thing")
            },
            Issue {
                author: linewright::domain::Login::new(LINEWRIGHT),
                parent: Some(Parent {
                    number: Number(2),
                    spec_change: None,
                }),
                ..labeled(3, "Part one", &["ready-for-human"])
            },
        ],
        vec![],
    )
}

/// `docs/specs/design_system.md`, The item page: a tracking issue's State is
/// "Tracking", with no status of its lane's own (Out of scope).
#[tokio::test]
async fn an_unfinished_tracking_issues_state_is_tracking_alone() {
    let pages = Pages::of(a_lane()).await;

    let html = pages.page("/items/2").await;

    assert_eq!(fact(&item_facts(&html), "State"), "Tracking");
    let value = select(&html, "#item dd")[0];
    assert!(select_in(value, "svg").is_empty(), "{}", value.html());
}

/// `docs/specs/design_system.md`, The item page, Related items: a tracking
/// issue is shown as its lane head is, with no card status's icon.
#[tokio::test]
async fn a_tickets_tracking_issue_is_shown_as_its_lane_head_is() {
    let pages = Pages::of(a_lane()).await;

    let board = pages.page("/").await;
    let html = pages.page("/items/3").await;

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
}

#[tokio::test]
async fn the_item_pages_card_line_names_its_pull_request_by_number_alone_and_its_waits_by_title() {
    let pull = Pull {
        title: "First try".into(),
        checks: vec![Check {
            name: "test".into(),
            state: CheckState::Pending,
        }],
        ..open_pull(2, LINEWRIGHT, &[1], &[LINEWRIGHT])
    };
    let pages = Pages::of(view(vec![ticket(1, "Do the thing")], vec![pull])).await;

    let facts = item_facts(&pages.page("/items/1").await);

    assert_eq!(fact(&facts, "State"), "In review · #2 · checks running");
    assert_eq!(
        fact(&facts, "Waiting for"),
        "#2: First try, its pull request, until it is merged or closed"
    );
}

#[tokio::test]
async fn the_item_pages_stream_names_a_blocker_by_its_number_and_title() {
    let pages = Pages::of(blocked(false)).await;
    let mut events = stream(&pages, "/events/items/3").await;

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
}
