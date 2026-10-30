use super::*;

use serde_json::json;

/// #1 and #2, each triaged in a run of its own, #2's the later.
async fn two_issues_triaged() -> Pages {
    let pages = Pages::of(view(
        vec![ticket(1, "Export fails"), ticket(2, "Import fails")],
        vec![],
    ))
    .await;
    for (item, started, ended) in [
        (1, "2026-10-01T12:00:00Z", "2026-10-01T12:01:00Z"),
        (2, "2026-10-01T12:05:00Z", "2026-10-01T12:07:30Z"),
    ] {
        pages.record(&json!({
            "item": item,
            "stage": "triage",
            "n": 1,
            "started_at": started,
            "end": {"at": ended, "ending": "done"},
            "sessions": [],
        }));
    }
    pages
}

#[tokio::test]
async fn the_runs_page_shows_the_runs_as_a_table_narrowed_by_its_form() {
    let pages = two_issues_triaged().await;

    let html = pages
        .page("/runs?item=&stage=triage&since=2026-10-01")
        .await;

    assert_eq!(
        runs_heads(&html),
        ["Outcome", "Run", "Item", "Stage", "Started", "Duration"]
    );
    assert_eq!(
        runs_shown(&html),
        [
            [
                "done",
                "2/triage-1",
                "#2: Import fails",
                "triage",
                "2026-10-01 12:05:00 UTC",
                "2m 30s"
            ],
            [
                "done",
                "1/triage-1",
                "#1: Export fails",
                "triage",
                "2026-10-01 12:00:00 UTC",
                "1m"
            ],
        ]
    );
    let links: Vec<&str> = select(&html, "#runs tbody a:not(.github)")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(
        links,
        [
            "/runs/2/triage-1",
            "/items/2",
            "/runs/1/triage-1",
            "/items/1"
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
        ]
    );
    assert_eq!(select(&html, "form")[0].attr("method"), Some("get"));
    assert_no_status_by_colour_alone(&html);
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
    assert_eq!(ids, ["holds", "runs"]);
    let running = runs_shown(&first[1].1);
    assert!(has_run(&running, "in flight", "1/triage-1"), "{running:?}");
    let ticking = select(&first[1].1, "#runs tbody td:last-child time[data-since]");
    assert!(
        ticking.first().is_some_and(|time| time
            .attr("data-since")
            .unwrap()
            .starts_with("2026-10-01T12:")),
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
        ended.len() == 1 && has_run(&ended, "done", "1/triage-1"),
        "{ended:?}"
    );
    web.stop().await;
}

#[tokio::test]
async fn the_runs_page_shows_an_automations_run_linking_to_the_automations_and_marks_one_by_hand() {
    let pages = Pages::of(view(vec![], vec![])).await;
    for (n, by, started, ended) in [
        (
            1,
            "schedule",
            "2026-10-01T13:00:00Z",
            "2026-10-01T13:01:00Z",
        ),
        (2, "hand", "2026-10-01T13:05:00Z", "2026-10-01T13:05:40Z"),
    ] {
        let record = json!({
            "automation": "hourly",
            "n": n,
            "started_by": by,
            "started_at": started,
            "end": {"at": ended, "ending": "done"},
            "sessions": [],
        });
        pages.record(&record);
    }

    let html = pages.page("/runs").await;

    assert_eq!(
        runs_shown(&html),
        [
            [
                "done",
                "hourly/2",
                "automation hourly",
                "",
                "2026-10-01 13:05:00 UTC by hand",
                "40s"
            ],
            [
                "done",
                "hourly/1",
                "automation hourly",
                "",
                "2026-10-01 13:00:00 UTC",
                "1m"
            ],
        ]
    );
    let items: Vec<&str> = select(&html, "#runs tbody td:nth-child(3) a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(items, ["/automations", "/automations"]);
    let by_hand = select(&html, "#runs tbody .badge:not(.tone)");
    assert_eq!(
        by_hand.into_iter().map(text).collect::<Vec<_>>(),
        ["by hand"]
    );
    let badge = classes(&html, "#runs tbody .badge:not(.tone)");
    assert!(badge[0].contains(&"badge-ghost"), "{badge:?}");
}

#[tokio::test]
async fn the_runs_page_leaves_an_interrupted_runs_duration_empty() {
    let pages = Pages::of(a_ticket()).await;
    record_interrupted(&pages, 1, 1, "2026-10-01T11:00:00Z");

    let html = pages.page("/runs").await;

    let shown = runs_shown(&html);
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(
        shown[0],
        [
            "interrupted",
            "1/triage-1",
            "#1: Do the thing",
            "triage",
            "2026-10-01 11:00:00 UTC",
            ""
        ]
    );
    assert_no_status_by_colour_alone(&html);
}

#[tokio::test]
async fn the_runs_page_names_an_item_linewright_does_not_hold_by_its_number_alone() {
    let pages = Pages::of(a_ticket()).await;
    record_interrupted(&pages, 9, 1, "2026-10-01T11:00:00Z");

    let html = pages.page("/runs").await;

    let shown = runs_shown(&html);
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0][2], "#9", "{shown:?}");
    let links: Vec<&str> = select(&html, "#runs tbody td:nth-child(3) a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(
        links,
        ["/items/9", &format!("https://github.com/{REPO}/issues/9")]
    );
}

#[tokio::test]
async fn the_runs_page_with_no_runs_to_list_says_so_in_place_of_the_table() {
    let pages = Pages::of(a_ticket()).await;

    let html = pages.page("/runs").await;

    assert_eq!(text(select(&html, "#runs")[0]), "No runs");
    assert_eq!(
        select(&html, "#runs table"),
        [] as [scraper::ElementRef<'_>; 0]
    );
}

#[tokio::test]
async fn the_runs_page_narrowed_to_an_item_without_runs_says_so_in_place_of_the_table() {
    let pages = Pages::of(a_ticket()).await;

    let (status, body) = pages.get("/runs?item=5").await;

    assert_eq!(status, 200);
    let html = Html::parse_document(&body);
    assert_eq!(text(select(&html, "#runs")[0]), "No runs");
    let events_path = events_path(&html).unwrap().to_string();
    assert_eq!(events_path, "/events/runs?item=5");
    let mut events = stream(&pages, &events_path).await;
    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let runs = regions(&first)
        .into_iter()
        .find(|(id, _)| id == "runs")
        .map(|(_, runs)| runs)
        .unwrap();
    assert_eq!(text(select(&runs, "#runs")[0]), "No runs");
}

#[tokio::test]
async fn the_runs_page_says_why_a_narrowing_does_not_read() {
    let pages = Pages::of(a_ticket()).await;

    let (status, body) = pages.get("/runs?item=abc").await;

    assert_eq!(status, 400);
    let html = Html::parse_document(&body);
    assert_eq!(
        text(select(&html, "#runs")[0]),
        "\"abc\" is not an issue or pull request number"
    );
}

#[tokio::test]
async fn the_runs_table_pins_its_heads_in_a_box_scrolling_both_ways_with_titles_on_one_line_whole_in_the_tooltip()
 {
    let pages = two_issues_triaged().await;

    let html = pages.page("/runs").await;

    let tables = classes(&html, "#runs .scroll-box > table");
    assert_eq!(tables.len(), 1, "{tables:?}");
    assert!(
        tables[0].contains(&"table") && tables[0].contains(&"table-pin-rows"),
        "{tables:?}"
    );
    assert_eq!(select(&html, "form ~ #runs .scroll-box").len(), 1);
    let titles = select(&html, "#runs tbody td:nth-child(3) > *");
    assert_eq!(titles.len(), 2);
    for title in titles {
        assert!(has_classes(title, &["line-clamp-1"]), "{}", title.html());
        assert_eq!(
            grows_in_place(title),
            Vec::<&str>::new(),
            "{}",
            title.html()
        );
        assert!(title.attr("data-tip-cut").is_some(), "{}", title.html());
    }
    assert_eq!(select(&html, "[title]").len(), 0);
}

/// `docs/specs/web/design_system.md`, Status: handed back is `warning`.
#[tokio::test]
async fn the_runs_and_item_pages_show_a_handed_back_runs_outcome_in_warning() {
    let pages = Pages::of(view(vec![ticket(1, "Do it")], vec![])).await;
    pages.record(&json!({
        "item": 1,
        "stage": "implement",
        "n": 1,
        "started_at": "2026-10-01T11:00:00Z",
        "end": {"at": "2026-10-01T11:10:00Z", "ending": "handed-back"},
        "sessions": [],
    }));

    for page in ["/runs", "/items/1"] {
        let html = pages.page(page).await;

        assert!(
            has_run(&runs_shown(&html), "handed back", "1/implement-1"),
            "{page}"
        );
        let badges: Vec<String> = select(&html, "#runs .tone.badge-warning")
            .into_iter()
            .map(text)
            .collect();
        assert_eq!(badges, ["handed back"], "{page}");
        assert_no_status_by_colour_alone(&html);
    }
}
