use super::*;

/// #1 and #2, each triaged in a run of its own.
async fn with_two_issues_triaged() -> World {
    let world = with_issue_for_triage().await;
    world
        .store
        .open_issue("reporter", "Import fails", "It hangs.", &["needs-triage"]);
    world
}

/// Whether `shown` reads as a duration `runs` words, in seconds.
fn seconds(shown: &str) -> bool {
    shown
        .strip_suffix('s')
        .is_some_and(|seconds| seconds.parse::<u32>().is_ok())
}

/// Whether `shown` reads as a time on 2026-10-01 from `hour_minute`, as the
/// commands print one.
fn started(shown: &str, hour_minute: &str) -> bool {
    shown
        .strip_prefix(&format!("2026-10-01 {hour_minute}:"))
        .is_some_and(|rest| rest.ends_with(" UTC"))
}

#[tokio::test]
async fn the_runs_page_shows_the_runs_as_a_table_narrowed_by_its_form() {
    let web = Web::start(with_two_issues_triaged().await).await;
    let done = |html: &Html| {
        runs_shown(html)
            .iter()
            .filter(|row| row[0] == "done")
            .count()
            == 2
    };
    web.eventually_page("/runs", done).await;

    let html = web
        .page("/runs?item=&stage=triage&since=2026-10-01&limit=1")
        .await;

    assert_eq!(
        runs_heads(&html),
        ["Outcome", "Run", "Item", "Stage", "Started", "Duration"]
    );
    let shown = runs_shown(&html);
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert_eq!(
        shown[0][..4],
        ["done", "2/triage-1", "#2: Import fails", "triage"]
    );
    assert!(started(&shown[0][4], "12:00"), "{shown:?}");
    assert!(seconds(&shown[0][5]), "{shown:?}");
    assert_eq!(shown[1], ["left out: 1 earlier run; show them all"]);
    let last = select(&html, "#runs tbody tr:last-child > td");
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].attr("colspan"), Some("6"));
    let links: Vec<&str> = select(&html, "#runs tbody a:not(.github)")
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
    let mut world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    world.hourly_by_schedule_then_by_hand().await;
    let web = Web::start(world).await;

    let html = web.page("/runs").await;

    let shown = runs_shown(&html);
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert_eq!(shown[0][..4], ["done", "hourly/2", "automation hourly", ""]);
    assert!(
        shown[0][4]
            .strip_suffix(" by hand")
            .is_some_and(|at| started(at, "13:05")),
        "{shown:?}"
    );
    assert!(seconds(&shown[0][5]), "{shown:?}");
    assert_eq!(shown[1][..4], ["done", "hourly/1", "automation hourly", ""]);
    assert!(started(&shown[1][4], "13:00"), "{shown:?}");
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
    web.stop().await;
}

#[tokio::test]
async fn the_runs_page_leaves_an_interrupted_runs_duration_empty() {
    let web = Web::start(with_ticket().await).await;
    record_interrupted(&web, 1, 1, "2026-10-01T11:00:00Z");

    let html = web.page("/runs").await;

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
    web.stop().await;
}

#[tokio::test]
async fn the_runs_page_with_no_runs_to_list_says_so_in_place_of_the_table() {
    let web = Web::start(with_ticket().await).await;

    let html = web.page("/runs").await;

    assert_eq!(text(select(&html, "#runs")[0]), "No runs");
    assert_eq!(
        select(&html, "#runs table"),
        [] as [scraper::ElementRef<'_>; 0]
    );
    web.stop().await;
}

#[tokio::test]
async fn the_runs_page_says_why_a_narrowing_does_not_read() {
    let web = Web::start(with_ticket().await).await;

    let (status, body) = web.get("/runs?limit=many").await;

    assert_eq!(status, 400);
    let html = Html::parse_document(&body);
    assert_eq!(
        text(select(&html, "#runs")[0]),
        "\"many\" is not a number of runs"
    );
    web.stop().await;
}

#[tokio::test]
async fn the_runs_table_pins_its_heads_in_a_box_scrolling_both_ways_with_titles_on_one_line() {
    let web = Web::start(with_two_issues_triaged().await).await;
    let html = web
        .eventually_page("/runs", |html| runs_shown(html).len() == 2)
        .await;

    let tables = classes(&html, "#runs .scroll-box > table");
    assert_eq!(tables.len(), 1, "{tables:?}");
    assert!(
        tables[0].contains(&"table") && tables[0].contains(&"table-pin-rows"),
        "{tables:?}"
    );
    assert_eq!(select(&html, "form ~ #runs .scroll-box").len(), 1);
    for title in classes(&html, "#runs tbody td:nth-child(3) > *") {
        assert!(
            [
                "line-clamp-1",
                "hover:line-clamp-none",
                "focus-within:line-clamp-none"
            ]
            .iter()
            .all(|class| title.contains(class)),
            "{title:?}"
        );
    }
    web.stop().await;
}
