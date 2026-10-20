use super::*;

/// #1 and #2, each triaged in a run of its own.
async fn with_two_issues_triaged() -> World {
    let world = with_issue_for_triage().await;
    world
        .store
        .open_issue("reporter", "Import fails", "It hangs.", &["needs-triage"]);
    world
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
