use linewright_core::domain::{
    Check, CheckState, Filed, Issue, Linked, Login, Number, Parent, Pull, label,
};
use serde_json::json;

use super::*;

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
fn for_the_maintainer() -> linewright_core::status::View {
    view(
        vec![labeled(1, "Do the thing", &["ready-for-human"])],
        vec![],
    )
}

#[test_log::test(tokio::test)]
async fn the_item_pages_facts_are_a_card_of_labels_in_the_specs_order_with_the_state_in_the_boards_terms()
 {
    let pages = Pages::of(for_the_maintainer()).await;

    let html = pages.page("/items/1").await;

    assert_eq!(select(&html, "#item dl.card").len(), 1, "{}", html.html());
    assert_in_secondary_text(&html, "#item dt");
    assert_eq!(
        item_facts(&html),
        [
            (
                "State".to_string(),
                "Waiting for a decision · decision needed · 2026-09-30 09:00:00 UTC".to_string()
            ),
            (
                "Waiting for".to_string(),
                "the maintainer: to label it `ready-for-agent` or `needs-triage`".to_string()
            ),
        ]
    );
    let decision = select(&html, "#item dd")[0];
    assert_eq!(
        select_in(
            decision,
            ".status-text > .status.status-warning[aria-hidden=true]:first-child + svg.icon + span:last-child"
        )
        .len(),
        1,
        "{}",
        decision.html()
    );
    assert_eq!(
        select_in(decision, ".status-text > .status + svg.icon")
            .into_iter()
            .map(|icon| icon.attr("aria-label"))
            .collect::<Vec<_>>(),
        [Some("waiting")],
        "{}",
        decision.html()
    );
    let waits = select(&html, "#item dd")[1];
    assert!(select_in(waits, "svg").is_empty(), "{}", waits.html());
    assert_badges_hold_no_words(&html, "#item dl");
    assert_no_status_by_colour_alone(&html);
}

#[test_log::test(tokio::test)]
async fn the_item_pages_heading_is_its_item_name_and_when_serve_last_read_it() {
    let pages = Pages::of(for_the_maintainer()).await;

    let html = pages.page("/items/1").await;

    assert_eq!(
        select(&html, "#heading h1")
            .into_iter()
            .map(text)
            .collect::<Vec<_>>(),
        ["Do the thing #1"]
    );
    let h1 = select(&html, "#heading h1")[0];
    assert_eq!(
        links_in(h1),
        [
            ("Do the thing".to_string(), "/items/1".to_string()),
            (
                "#1".to_string(),
                "https://github.com/owner/widgets/issues/1".to_string()
            ),
        ]
    );
    assert!(
        select_in(h1, "[data-tip-cut], .truncate").is_empty(),
        "{}",
        h1.html()
    );
    let as_of = select(&html, "#heading .line");
    assert_eq!(as_of.len(), 1, "{}", html.html());
    assert_eq!(text(as_of[0]), "as of 2026-10-01 12:00:00 UTC");
    assert_in_secondary_text(&html, "#heading .line");
    assert_eq!(
        select_in(as_of[0], "time")[0].attr("datetime"),
        Some("2026-10-01T12:00:00Z")
    );
}

#[test_log::test(tokio::test)]
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
    assert_badges_hold_no_words(&html, "#item dl");
}

/// `docs/specs/shell.md`, No such thing.
#[test_log::test(tokio::test)]
async fn the_item_page_of_no_item_says_there_is_none() {
    let pages = Pages::of(for_the_maintainer()).await;

    let (status, body) = pages.get("/items/99").await;

    assert_eq!(status, 404);
    let html = Html::parse_document(&body);
    assert_eq!(
        select(&html, "main #item")
            .into_iter()
            .map(text)
            .collect::<Vec<_>>(),
        ["No such item #99"]
    );
    assert_eq!(select(&html, ".drawer-side nav").len(), 1);
}

#[test_log::test(tokio::test)]
async fn the_item_page_shows_the_run_in_flight_ticking_as_a_row_and_then_the_run_past() {
    let pages = Pages::of(linewright_core::status::View {
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
    pages.record(&record).await;
    let in_flight = lock_of(&pages.path("runs/items/1/triage-1.json"));

    let running_html = pages.page("/items/1").await;
    record["end"] = json!({"at": "2026-10-01T12:01:00Z", "ending": "done"});
    pages.record(&record).await;
    drop(in_flight);
    pages.publish(for_the_maintainer());
    let html = pages.page("/items/1").await;

    assert_eq!(
        runs_heads(&running_html),
        ["Outcome", "Run", "Stage", "Started", "Duration", ""]
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
    let ticking = select(
        &running_html,
        "#runs tbody td:nth-last-child(2) time[data-since]",
    );
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
            "1m",
            "View"
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

#[test_log::test(tokio::test)]
async fn the_item_page_shows_the_latest_ten_runs_and_links_to_all_of_them_on_the_runs_page() {
    let pages = Pages::of(for_the_maintainer()).await;
    for n in 1..=12 {
        record_interrupted(&pages, 1, n, &format!("2026-10-01T{:02}:00:00Z", n - 1)).await;
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
            "",
            "View"
        ]
    );
    assert_eq!(shown[9][1], "1/triage-3");
    assert_eq!(shown[10], ["left out: 2 earlier runs; show them all"]);
    let last = select(&html, "#runs tbody tr:last-child > td");
    assert_eq!(last[0].attr("colspan"), Some("6"));
    let more = select(&html, "#runs tbody tr:last-child a");
    assert_eq!(more[0].attr("href"), Some("/runs?item=1"));
    let all = runs_shown(&pages.page("/runs?item=1").await);
    assert_eq!(all.len(), 12, "{all:?}");
    let tables = classes(&html, "#runs .scroll-box > table");
    assert!(
        tables[0].contains(&"table") && tables[0].contains(&"table-pin-rows"),
        "{tables:?}"
    );
    assert_eq!(table_cards(&html), [named_card("Runs", &[])]);
    assert_eq!(
        views(&html, "#runs")[0],
        (
            "View".to_string(),
            "View run 1/triage-12".to_string(),
            "/runs/1/triage-12".to_string()
        )
    );
    assert_eq!(
        select(&html, "#item ~ .table-card #runs .scroll-box").len(),
        1
    );
    let start = "start";
    let row = vec![start, start, start, start, "end", start];
    let mut aligned = vec![row; 11];
    aligned.push(vec!["none"]);
    assert_eq!(alignments(&html, "#runs"), aligned);
}

#[test_log::test(tokio::test)]
async fn the_item_page_of_an_item_with_no_runs_says_so() {
    let pages = Pages::of(for_the_maintainer()).await;

    let html = pages.page("/items/1").await;

    assert_eq!(text(select(&html, "#runs")[0]), "No runs");
    assert_eq!(
        select(&html, "#runs table"),
        [] as [scraper::ElementRef<'_>; 0]
    );
}

#[test_log::test(tokio::test)]
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

    record_interrupted(&pages, closed.0, 1, "2026-10-01T11:00:00Z").await;
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
            "",
            "View"
        ]]
    );
}

#[test_log::test(tokio::test)]
async fn the_item_pages_stream_sends_every_region_then_those_serves_view_changed() {
    let web = Web::start(with_ticket().await).await;
    let mut events = stream(&web, "/events/items/1").await;

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let first = regions(&first);
    let ids: Vec<&str> = first.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "holds", "title", "crumbs", "heading", "buttons", "item", "runs"
        ]
    );
    let before = item_facts(region_of(&first, "item"));
    let quiet = tokio::time::timeout(Duration::from_millis(2500), events.recv()).await;
    assert!(quiet.is_err(), "sent with nothing changed: {quiet:?}");

    web.store
        .unlabel_as(linewright_core::domain::Number(1), "ready-for-human")
        .unwrap();
    web.store
        .label_as(
            "maintainer",
            linewright_core::domain::Number(1),
            "needs-info",
        )
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
    assert_ne!(fact(&item, "State"), fact(&before, "State"));
    assert_eq!(item, item_facts(&web.page("/items/1").await));
    web.stop().await;
}

#[test_log::test(tokio::test)]
async fn the_item_pages_stream_sends_nothing_when_its_runs_agent_produces_output() {
    let pages = Pages::of(linewright_core::status::View {
        in_flight: vec![in_flight("1/triage-1", "2026-10-01T12:00:00Z")],
        ..for_the_maintainer()
    })
    .await;
    let output = "sessions/issue-1/20261001T120000Z-triage/output.jsonl";
    let turn = json!({"type": "linewright-turn", "started_at": "2026-10-01T12:00:00Z", "started_by": "brief"});
    pages.write(&[(output, &output_of(&[turn]))]);
    pages
        .record(&json!({
            "item": 1,
            "stage": "triage",
            "n": 1,
            "started_at": "2026-10-01T12:00:00Z",
            "end": null,
            "sessions": ["sessions/issue-1/20261001T120000Z-triage"],
        }))
        .await;
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
fn blocked(unheld: bool) -> linewright_core::status::View {
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

#[test_log::test(tokio::test)]
async fn the_item_page_names_a_blocker_it_waits_for_by_its_item_name() {
    let pages = Pages::of(blocked(false)).await;

    let html = pages.page("/items/3").await;

    assert_eq!(
        fact(&item_facts(&html), "Waiting for"),
        "Fix the parser #2, which blocks it"
    );
    let links: Vec<&str> = select(&html, "#item dd")[1]
        .select(&Selector::parse("a").unwrap())
        .map(|a| a.attr("href").unwrap())
        .collect();
    assert_eq!(
        links,
        ["/items/2", "https://github.com/owner/widgets/issues/2"]
    );
    assert_eq!(
        fact(&item_facts(&html), "State"),
        "Waiting for Linewright · blocked by #2 · 2026-09-30 09:00:00 UTC"
    );
}

#[test_log::test(tokio::test)]
async fn the_item_page_names_a_blocker_the_snapshot_does_not_hold_by_its_number_alone() {
    let pages = Pages::of(blocked(true)).await;

    let html = pages.page("/items/3").await;

    let waits: Vec<String> = select(&html, "#item dd")[1]
        .select(&Selector::parse("div").unwrap())
        .map(text)
        .collect();
    assert_eq!(
        waits,
        ["Fix the parser #2, which blocks it", "#4, which blocks it"]
    );
}

/// #2, cut into #3, which waits on the maintainer.
fn a_lane() -> linewright_core::status::View {
    view(
        vec![
            labeled_by_linewright(Issue {
                labels: vec![label::DESIGNED.into()],
                sub_issues: vec![sub_issue(3, true, LINEWRIGHT)],
                ..ticket(2, "Big thing")
            }),
            Issue {
                author: Login::new(LINEWRIGHT),
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

/// `docs/specs/board.md`, Item: a tracking issue's State is
/// "Tracking", with no status of its lane's own (Out of scope), and its
/// heading is a card with icon, the icon of an issue (Heading, Its card).
#[test_log::test(tokio::test)]
async fn an_unfinished_tracking_issues_state_is_tracking_alone() {
    let pages = Pages::of(a_lane()).await;

    let html = pages.page("/items/2").await;

    assert_eq!(fact(&item_facts(&html), "State"), "Tracking");
    let value = select(&html, "#item dd")[0];
    assert!(select_in(value, "svg").is_empty(), "{}", value.html());
    let heading_icons: Vec<_> = select(&html, "#heading h1 svg[role=img]")
        .into_iter()
        .map(kind)
        .collect();
    assert_eq!(
        heading_icons,
        [("issue-opened".to_string(), "issue".to_string())]
    );
}

/// `docs/specs/board.md`, Item, Related items: a tracking
/// issue is shown as its lane head is, with no card status's icon.
#[test_log::test(tokio::test)]
async fn a_tickets_tracking_issue_is_shown_as_its_lane_head_is() {
    let pages = Pages::of(a_lane()).await;

    let board = pages.page("/").await;
    let html = pages.page("/items/3").await;

    let head = text(select(&board, "#lane-2 .lane-head")[0]);
    assert_eq!(head, "Big thing #2 — 0/1 tickets closed");
    let facts = item_facts(&html);
    assert_eq!(fact(&facts, "Tracking issue"), head);
    let at = facts
        .iter()
        .position(|(label, _)| label == "Tracking issue")
        .unwrap();
    let value = select(&html, "#item dd")[at];
    let icons: Vec<_> = select_in(value, "svg").into_iter().map(kind).collect();
    assert_eq!(icons, [("issue-opened".to_string(), "issue".to_string())]);
    let links: Vec<&str> = select_in(value, "a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(
        links,
        ["/items/2", "https://github.com/owner/widgets/issues/2"]
    );
}

#[test_log::test(tokio::test)]
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
        "First try #2, its pull request, until it is merged or closed"
    );
}

/// The icon of an issue or a pull request, and what a screen reader reads of
/// it.
fn kind(icon: ElementRef) -> (String, String) {
    let attribute = |name| {
        icon.attr(name)
            .unwrap_or_else(|| panic!("no {name}: {}", icon.html()))
            .to_string()
    };
    (attribute("data-icon"), attribute("aria-label"))
}

/// `docs/specs/board.md`, Item, Heading and Related items: the heading is a
/// card with icon, whose icon is that of an issue or of a pull request, and a
/// related item carries the icon of its kind first, then its item name. A
/// screen reader reads each icon.
#[test_log::test(tokio::test)]
async fn the_item_page_shows_whether_it_and_each_related_item_is_an_issue_or_a_pull_request() {
    let pull = Pull {
        title: "First try".into(),
        ..open_pull(2, LINEWRIGHT, &[1], &[LINEWRIGHT])
    };
    let pages = Pages::of(view(vec![ticket(1, "Do the thing")], vec![pull])).await;

    let issue = ("issue-opened".to_string(), "issue".to_string());
    let pull_request = ("git-pull-request".to_string(), "pull request".to_string());
    for (path, related, own, its, name) in [
        (
            "/items/1",
            "Pull requests",
            issue.clone(),
            pull_request.clone(),
            ("First try", 2),
        ),
        (
            "/items/2",
            "Closes",
            pull_request,
            issue,
            ("Do the thing", 1),
        ),
    ] {
        let html = pages.page(path).await;
        let heading_icons: Vec<_> = select(&html, "#heading h1 svg[role=img]")
            .into_iter()
            .map(kind)
            .collect();
        assert_eq!(heading_icons, [own], "{path}");
        let at = item_facts(&html)
            .iter()
            .position(|(label, _)| label == related)
            .unwrap_or_else(|| panic!("{path}: {related}"));
        let value = select(&html, "#item dd")[at];
        let icons: Vec<_> = select_in(value, ".related-item > svg:first-child")
            .into_iter()
            .map(kind)
            .collect();
        assert_eq!(icons, [its], "{path}: {}", value.html());
        let (title, n) = name;
        assert_eq!(
            links_in(value),
            [
                (title.to_string(), format!("/items/{n}")),
                (
                    format!("#{n}"),
                    format!("https://github.com/owner/widgets/issues/{n}")
                ),
            ],
            "{path}"
        );
    }
}

#[test_log::test(tokio::test)]
async fn the_item_pages_stream_names_a_blocker_by_its_item_name() {
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
        "Fix the parser #2, which blocks it"
    );
}

/// The value of the fact labeled `label` on `html`'s item page.
fn fact_value<'a>(html: &'a Html, label: &str) -> ElementRef<'a> {
    let at = item_facts(html)
        .iter()
        .position(|(shown, _)| shown == label)
        .unwrap_or_else(|| panic!("no {label}"));
    select(html, "#item dd")[at]
}

/// `docs/specs/board.md`, Item, Heading: the heading's `card-title`
/// holds, after its icon, #N's title, then a space, then its number in
/// `font-normal`, the two as one text that wraps between words.
#[test_log::test(tokio::test)]
async fn the_item_pages_heading_holds_its_icon_then_its_title_and_number_as_one_text() {
    let pages = Pages::of(for_the_maintainer()).await;

    let html = pages.page("/items/1").await;

    let h1 = select(&html, "#heading h1.card-title")[0];
    assert_eq!(parts(h1), ["icon issue-opened", "span."], "{}", h1.html());
    assert_title_then_number_as_one_text(h1);
    let number = select_in(h1, "a[href^='https://github.com/']")[0];
    assert!(
        has_classes(number, &["font-normal", "text-base-content/60"]),
        "{}",
        number.html()
    );
    assert_no_words_loose_in_a_flex(&html, "#item, #heading");
}

/// `docs/specs/board.md`, Item, Related items: a related item is one line
/// of its icon, its status icon in its card status's colour, its title and
/// its number, `gap-1` apart, the number in the weight of the text around
/// it; and one that Linewright has not read is its number alone, in
/// secondary text.
#[test_log::test(tokio::test)]
async fn a_related_item_is_its_icon_status_icon_title_and_number_gap_1_apart() {
    let pull = Pull {
        title: "First try".into(),
        checks: vec![Check {
            name: "test".into(),
            state: CheckState::Pending,
        }],
        ..open_pull(2, LINEWRIGHT, &[1], &[LINEWRIGHT])
    };
    let for_a_spec_change = Issue {
        author: Login::new(LINEWRIGHT),
        filed: Some(Filed::SpecChange {
            spec_change: Number(9),
        }),
        ..ticket(1, "Do the thing")
    };
    let pages = Pages::of(view(
        vec![
            for_a_spec_change,
            labeled(3, "Part one", &["ready-for-agent"]),
        ],
        vec![pull],
    ))
    .await;

    let item = pages.page("/items/1").await;

    let line = select_in(fact_value(&item, "Pull requests"), ".flex-wrap")[0];
    assert!(
        has_classes(line, &["flex", "justify-end", "gap-4"]),
        "{}",
        line.html()
    );
    let related = select_in(line, ":scope > .related-item")[0];
    assert!(
        has_classes(related, &["inline-flex", "items-center", "gap-1"]),
        "{}",
        related.html()
    );
    assert_eq!(
        parts(related),
        [
            "icon git-pull-request",
            "icon arrow-path",
            "title",
            "number"
        ]
    );
    let status = select_in(related, ":scope > svg")[1];
    assert!(has_classes(status, &["text-info"]), "{}", related.html());
    let number = select_in(related, ":scope > a")[1];
    assert!(
        has_classes(number, &["text-base-content/60"]) && lacks_classes(number, &["font-normal"]),
        "{}",
        related.html()
    );

    let unread = select_in(
        fact_value(&item, "Spec change"),
        ".flex-wrap > .related-item",
    )[0];
    assert_eq!(parts(unread), ["number"], "{}", unread.html());
    assert_eq!(text(unread), "#9");
    assert_in_secondary_text(
        &item,
        "#item .related-item > a[href^='https://github.com/']",
    );

    let unchecked = Pull {
        title: "First try".into(),
        ..open_pull(2, LINEWRIGHT, &[1], &[LINEWRIGHT])
    };
    let quiet = Pages::of(view(vec![ticket(1, "Do the thing")], vec![unchecked])).await;
    let quiet = quiet.page("/items/1").await;
    let statusless = select_in(fact_value(&quiet, "Pull requests"), ".related-item")[0];
    assert_eq!(
        parts(statusless),
        ["icon git-pull-request", "title", "number"],
        "an item whose card shows no card status has no status icon"
    );
    assert_no_words_loose_in_a_flex(&item, "#item, #heading");
    assert_no_words_loose_in_a_flex(&quiet, "#item, #heading");
}

/// `docs/specs/board.md`, Item, Related items: a related tracking issue
/// is a related item with the rest of its lane head `gap-1` after its
/// number, not in the lane head's weight.
#[test_log::test(tokio::test)]
async fn a_related_tracking_issue_has_the_rest_of_its_lane_head_after_its_number() {
    let pages = Pages::of(a_lane()).await;

    let html = pages.page("/items/3").await;

    let value = fact_value(&html, "Tracking issue");
    let related = select_in(value, ".related-item")[0];
    assert!(has_classes(related, &["gap-1"]), "{}", related.html());
    assert_eq!(
        parts(related),
        ["icon issue-opened", "title", "number", "rest"]
    );
    assert_eq!(text(related), "Big thing #2 — 0/1 tickets closed");
    for element in std::iter::once(value).chain(select_in(value, "*")) {
        assert!(
            lacks_classes(element, &["font-semibold", "font-normal"]),
            "{}",
            value.html()
        );
    }
}

/// `docs/specs/board.md`, Item, Items in Waiting for and in what keeps #N
/// out: an item is its title, then a space, then its number, wrapping
/// between words with its sentence, and one that Linewright has not read
/// is its number alone, in secondary text.
#[test_log::test(tokio::test)]
async fn an_item_waited_for_is_its_title_then_its_number_as_one_text() {
    let pages = Pages::of(blocked(true)).await;

    let html = pages.page("/items/3").await;

    let waits = fact_value(&html, "Waiting for");
    let lines = select_in(waits, ":scope > div");
    let titled = assert_title_then_number_as_one_text(lines[0]);
    let unread = select_in(lines[1], ":scope > *")[0];
    assert_eq!(parts(lines[1])[0], "number", "{}", lines[1].html());
    for number in [select_in(titled, ":scope > a")[1], unread] {
        assert!(
            has_classes(number, &["text-base-content/60"])
                && lacks_classes(number, &["font-normal"]),
            "{}",
            number.html()
        );
    }
    assert_no_words_loose_in_a_flex(&html, "#item, #heading");
}

/// `docs/specs/components.md`, Facts: a status among facts is its dot, its
/// icon and its words, `gap-1` apart.
#[test_log::test(tokio::test)]
async fn a_status_among_facts_is_its_dot_icon_and_words_gap_1_apart() {
    let pages = Pages::of(for_the_maintainer()).await;

    let html = pages.page("/items/1").await;

    let status = select(&html, "#item .status-text")[0];
    assert!(
        has_classes(status, &["inline-flex", "gap-1"]),
        "{}",
        status.html()
    );
    assert_eq!(parts(status)[1..], ["icon hand-raised", "span."]);
    let icon = select_in(status, ":scope > svg")[0];
    assert!(
        has_classes(status, &["items-baseline"]) && has_classes(icon, &["self-start", "h-lh"]),
        "the words sit on the line's baseline, and the icon is centred on their first line (`docs/specs/design_system.md`, Icons, Alignment): {}",
        status.html()
    );
}

/// `docs/specs/board.md`, Item, Related items: the items that one fact
/// lists are `gap-4` apart.
#[test_log::test(tokio::test)]
async fn the_items_one_fact_lists_are_gap_4_apart() {
    let pull = |number, title: &str| Pull {
        title: title.into(),
        ..open_pull(number, LINEWRIGHT, &[1], &[LINEWRIGHT])
    };
    let pages = Pages::of(view(
        vec![ticket(1, "Do the thing")],
        vec![pull(2, "First try"), pull(3, "Second try")],
    ))
    .await;

    let html = pages.page("/items/1").await;

    let lines = select_in(fact_value(&html, "Pull requests"), ":scope > div");
    assert_eq!(lines.len(), 1, "{}", lines[0].html());
    let items = select_in(lines[0], ":scope > .flex-wrap")[0];
    assert!(has_classes(items, &["gap-4"]), "{}", items.html());
    assert_eq!(
        select_in(items, ":scope > .related-item")
            .into_iter()
            .map(text)
            .collect::<Vec<_>>(),
        ["First try #2", "Second try #3"]
    );
}
