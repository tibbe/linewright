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
        pages
            .record(&json!({
                "item": item,
                "stage": "triage",
                "n": 1,
                "started_at": started,
                "end": {"at": ended, "ending": "done"},
                "sessions": [],
            }))
            .await;
    }
    pages
}

#[test_log::test(tokio::test)]
async fn the_runs_page_shows_the_runs_as_a_table_narrowed_by_its_filters() {
    let pages = two_issues_triaged().await;

    let html = pages
        .page("/runs?item=&stage=triage&since=2026-10-01")
        .await;

    assert_eq!(
        runs_heads(&html),
        ["Outcome", "Run", "Item", "Stage", "Started", "Duration", ""]
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
                "2m 30s",
                "View"
            ],
            [
                "done",
                "1/triage-1",
                "#1: Export fails",
                "triage",
                "2026-10-01 12:00:00 UTC",
                "1m",
                "View"
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
            "/items/2",
            "/runs/2/triage-1",
            "/items/1",
            "/runs/1/triage-1"
        ]
    );
    assert_eq!(
        views(&html, "#runs"),
        [
            (
                "View".to_string(),
                "View run 2/triage-1".to_string(),
                "/runs/2/triage-1".to_string()
            ),
            (
                "View".into(),
                "View run 1/triage-1".into(),
                "/runs/1/triage-1".into()
            ),
        ]
    );
    assert_eq!(
        filters(&html),
        [
            ("item", String::new()),
            ("stage", "triage".to_string()),
            ("since", "2026-10-01".to_string()),
        ]
    );
    assert_eq!(
        select(&html, "form[data-filters]")[0].attr("data-filters"),
        Some("/events/runs")
    );
    assert_no_status_by_colour_alone(&html);
}

/// `docs/specs/design_system.md`, Filters; `docs/specs/web/design_system.md`,
/// Filters; `docs/specs/web/runs.md`, Runs.
#[test_log::test(tokio::test)]
async fn the_runs_pages_filters_are_labelled_unset_and_unbuttoned_with_a_stage_reading_all_stages()
{
    let pages = two_issues_triaged().await;

    let html = pages.page("/runs").await;

    assert_eq!(
        filters(&html),
        [
            ("item", String::new()),
            ("stage", String::new()),
            ("since", String::new()),
        ]
    );
    let labels: Vec<String> = select(&html, "form[data-filters] label > .label")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(labels, ["Item", "Stage", "Since"]);
    let looks: Vec<&str> = classes(&html, "form[data-filters] > label")
        .into_iter()
        .filter_map(|classes| {
            ["input", "select"]
                .into_iter()
                .find(|look| classes.contains(look))
        })
        .collect();
    assert_eq!(looks, ["input", "select", "input"]);
    let stages = select(&html, "form[data-filters] select option");
    assert_eq!(text(stages[0]), "All stages");
    assert_eq!(stages[0].attr("value"), Some(""));
    assert_eq!(select(&html, "form[data-filters] button").len(), 0);
    let runs_links: Vec<&str> = select(&html, ".drawer-side nav a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .filter(|href| href.starts_with("/runs"))
        .collect();
    assert_eq!(runs_links, ["/runs"]);
}

#[test_log::test(tokio::test)]
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
    assert_eq!(
        ids,
        ["holds", "title", "crumbs", "heading", "buttons", "runs"]
    );
    let running = runs_shown(region_of(&first, "runs"));
    assert!(has_run(&running, "in flight", "1/triage-1"), "{running:?}");
    let ticking = select(
        region_of(&first, "runs"),
        "#runs tbody td:nth-last-child(2) time[data-since]",
    );
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

#[test_log::test(tokio::test)]
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
        pages.record(&record).await;
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
                "40s",
                "View"
            ],
            [
                "done",
                "hourly/1",
                "automation hourly",
                "",
                "2026-10-01 13:00:00 UTC",
                "1m",
                "View"
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
    assert_eq!(
        classes(&html, "#runs tbody .badge:not(.tone)"),
        [["badge", "badge-sm"]],
        "docs/specs/web/runs.md, Runs: a badge-sm with no colour or style class"
    );
}

#[test_log::test(tokio::test)]
async fn the_runs_page_leaves_an_interrupted_runs_duration_empty() {
    let pages = Pages::of(a_ticket()).await;
    record_interrupted(&pages, 1, 1, "2026-10-01T11:00:00Z").await;

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
            "",
            "View"
        ]
    );
    assert_no_status_by_colour_alone(&html);
}

#[test_log::test(tokio::test)]
async fn the_runs_page_names_an_item_linewright_does_not_hold_by_its_number_alone() {
    let pages = Pages::of(a_ticket()).await;
    record_interrupted(&pages, 9, 1, "2026-10-01T11:00:00Z").await;

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

#[test_log::test(tokio::test)]
async fn the_runs_page_with_no_runs_to_list_says_so_in_place_of_the_table() {
    let pages = Pages::of(a_ticket()).await;

    let html = pages.page("/runs").await;

    assert_eq!(text(select(&html, "#runs")[0]), "No runs");
    assert_eq!(
        select(&html, "#runs table"),
        [] as [scraper::ElementRef<'_>; 0]
    );
}

#[test_log::test(tokio::test)]
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

#[test_log::test(tokio::test)]
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

/// `docs/specs/design_system.md`, Tables; `docs/specs/web/design_system.md`,
/// Tables.
#[test_log::test(tokio::test)]
async fn the_runs_table_is_in_a_card_showing_no_name_with_the_filters_in_a_row_and_durations_aligned_right()
 {
    let pages = two_issues_triaged().await;

    let html = pages.page("/runs").await;

    assert_eq!(
        table_cards(&html),
        [unnamed_card(&[], &["item", "stage", "since"])]
    );
    assert_eq!(select(&html, ".table-card #runs table").len(), 1);
    let start = "start";
    let row = [start, start, start, start, start, "end", start];
    assert_eq!(alignments(&html, "#runs"), [row, row, row]);
}

#[test_log::test(tokio::test)]
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
    let titles = select(&html, "#runs tbody td:nth-child(3) > *");
    assert_eq!(titles.len(), 2);
    for title in titles {
        assert_cuts_only_the_title(title, 1);
        assert_eq!(
            grows_in_place(title),
            Vec::<&str>::new(),
            "{}",
            title.html()
        );
    }
    assert_eq!(select(&html, "[title]").len(), 0);
}

/// `docs/specs/runs.md`, Ending, and `docs/specs/web/design_system.md`,
/// Status: handed back shows as waiting, which is `warning`.
#[test_log::test(tokio::test)]
async fn the_runs_and_item_pages_show_a_handed_back_runs_outcome_in_warning() {
    let pages = Pages::of(view(vec![ticket(1, "Do it")], vec![])).await;
    pages
        .record(&json!({
            "item": 1,
            "stage": "implement",
            "n": 1,
            "started_at": "2026-10-01T11:00:00Z",
            "end": {"at": "2026-10-01T11:10:00Z", "ending": "handed-back"},
            "sessions": [],
        }))
        .await;

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

/// #1, triaged in `count` runs, the `n`th started `n` minutes after noon.
async fn triaged_in_runs(count: u32) -> Pages {
    let pages = Pages::of(view(vec![ticket(1, "Export fails")], vec![])).await;
    for n in 1..=count {
        pages
            .record(&json!({
                "item": 1,
                "stage": "triage",
                "n": n,
                "started_at": format!("2026-10-01T12:{n:02}:00Z"),
                "end": {"at": format!("2026-10-01T12:{n:02}:30Z"), "ending": "done"},
                "sessions": [],
            }))
            .await;
    }
    pages
}

/// The run IDs of the table of runs on `html`, in order.
#[track_caller]
fn run_ids(html: &Html) -> Vec<String> {
    runs_shown(html)
        .into_iter()
        .map(|row| row[1].clone())
        .collect()
}

/// `docs/specs/design_system.md`, Tables, Pages;
/// `docs/specs/web/design_system.md`, Tables.
#[test_log::test(tokio::test)]
async fn the_runs_page_shows_its_runs_25_at_a_time_with_the_pages_under_the_table() {
    let pages = triaged_in_runs(30).await;

    let first = pages.page("/runs").await;
    let second = pages.page("/runs?stage=triage&page=2").await;

    let newest: Vec<String> = (6..=30).rev().map(|n| format!("1/triage-{n}")).collect();
    assert_eq!(run_ids(&first), newest);
    assert_eq!(
        pages_nav(&first, "#runs"),
        Some(PagesNav {
            label: "Pages of runs".into(),
            showing: "Showing 1–25 of 30 runs".into(),
            items: vec![
                no_link("Previous"),
                page_link("1*", "/runs", "/events/runs"),
                page_link("2", "/runs?page=2", "/events/runs?page=2"),
                page_link("Next", "/runs?page=2", "/events/runs?page=2"),
            ],
        })
    );
    assert_eq!(
        run_ids(&second),
        [
            "1/triage-5",
            "1/triage-4",
            "1/triage-3",
            "1/triage-2",
            "1/triage-1"
        ]
    );
    assert_eq!(
        pages_nav(&second, "#runs").unwrap().items,
        [
            page_link(
                "Previous",
                "/runs?stage=triage",
                "/events/runs?stage=triage"
            ),
            page_link("1", "/runs?stage=triage", "/events/runs?stage=triage"),
            page_link(
                "2*",
                "/runs?stage=triage&page=2",
                "/events/runs?stage=triage&page=2"
            ),
            no_link("Next"),
        ]
    );
    assert_eq!(
        events_path(&second),
        Some("/events/runs?stage=triage&page=2")
    );
    assert_eq!(
        select(&second, "form[data-filters]")[0].attr("data-filters"),
        Some("/events/runs")
    );
}

/// `docs/specs/design_system.md`, Tables, Pages, Updates.
#[test_log::test(tokio::test)]
async fn the_runs_pages_stream_keeps_the_readers_page_number_as_runs_come() {
    let pages = triaged_in_runs(26).await;
    let mut events = stream(&pages, "/events/runs?page=2").await;
    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run_ids(region_of(&regions(&first), "runs")), ["1/triage-1"]);

    pages
        .record(&json!({
            "item": 1,
            "stage": "triage",
            "n": 27,
            "started_at": "2026-10-01T12:40:00Z",
            "end": {"at": "2026-10-01T12:40:30Z", "ending": "done"},
            "sessions": [],
        }))
        .await;
    let runs = loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let Some((_, runs)) = regions(&event).into_iter().find(|(id, _)| id == "runs") {
            break runs;
        }
    };

    assert_eq!(run_ids(&runs), ["1/triage-2", "1/triage-1"]);
    assert_eq!(
        pages_nav(&runs, "#runs").unwrap().showing,
        "Showing 26–27 of 27 runs"
    );
}
