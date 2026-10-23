use super::*;

use linewright::harness::SessionError;

/// A world working under `work/REPO`, where the web UI reads the runs, with
/// the automation files `files` on the default branch.
async fn with_automations(files: &[(&str, &str)]) -> World {
    let world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    let files: Vec<(String, &str)> = files
        .iter()
        .map(|(name, text)| (format!(".linewright/automations/{name}.md"), *text))
        .collect();
    let files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), *t)).collect();
    world
        .store
        .commit("maintainer", "main", &files, "Add automations")
        .await
        .unwrap();
    world
}

const WEEKLY: &str = "---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n";
const BROKEN: &str = "---\non:\n  schedule: \"every hour\"\n---\nLook.\n";

fn failed() -> Result<String, SessionError> {
    Err(SessionError::Failed {
        reason: "it crashed".into(),
        log_tail: String::new(),
    })
}

/// The unparsed files, as a reader reads them.
fn unparsed_shown(html: &Html) -> Vec<String> {
    select(html, "#automations .unparsed li")
        .into_iter()
        .map(text)
        .collect()
}

/// Each automation's row: its name, schedule, last run and next run.
fn rows_shown(html: &Html) -> Vec<Vec<String>> {
    select(html, "#automations tbody tr")
        .into_iter()
        .map(|row| {
            row.select(&Selector::parse("td, th").unwrap())
                .map(text)
                .collect()
        })
        .collect()
}

/// Each hold's words, and where it links.
fn holds(html: &Html) -> Vec<(String, String)> {
    select(html, "#holds .alert")
        .into_iter()
        .map(|alert| {
            let href = alert
                .select(&Selector::parse("a").unwrap())
                .last()
                .and_then(|a| a.attr("href"))
                .unwrap_or_default()
                .to_string();
            let phrase = alert.select(&Selector::parse(".phrase").unwrap()).next();
            (phrase.map(text).unwrap_or_default(), href)
        })
        .collect()
}

#[tokio::test]
async fn the_automations_page_lists_the_unparsed_files_then_each_automation_by_name() {
    let world =
        with_automations(&[("weekly", WEEKLY), ("broken", BROKEN), ("hourly", HOURLY)]).await;
    world.harness.on_automation(|_| Ok(nothing_needs_doing()));
    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();
    let web = Web::start(world).await;

    let html = web.page("/automations").await;

    let unparsed = unparsed_shown(&html);
    assert_eq!(unparsed.len(), 1, "{unparsed:?}");
    assert!(
        unparsed[0].starts_with(
            ".linewright/automations/broken.md: does not parse: `on.schedule` is \"every hour\""
        ),
        "{unparsed:?}"
    );
    let rows = rows_shown(&html);
    assert_eq!(rows.len(), 2, "{rows:?}");
    let (hourly, weekly) = (&rows[0], &rows[1]);
    assert_eq!(hourly[..2], ["hourly", "At minute 0 past every hour."]);
    assert!(
        hourly[2].starts_with("none yet, first seen 2026-10-01 12:00:"),
        "{hourly:?}"
    );
    assert_eq!(hourly[3], "2026-10-01 13:00:00 UTC");
    assert_eq!(weekly[..2], ["weekly", "At 06:00, on Monday."]);
    assert!(
        weekly[2].starts_with("weekly/1 by hand, started 2026-10-01 12:00:")
            && weekly[2].ends_with(" UTC — done"),
        "{weekly:?}"
    );
    assert_eq!(weekly[3], "2026-10-05 06:00:00 UTC");
    let links: Vec<&str> = select(&html, "#automations tbody a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(links, ["/runs/weekly/1"]);
    assert_eq!(select(&html, "#automations tbody svg.icon").len(), 1);
    assert_no_status_by_colour_alone(&html);
    assert_eq!(events_path(&html), Some("/events/automations"));
    web.stop().await;
}

#[tokio::test]
async fn the_automations_table_pins_its_heads_in_a_box_scrolling_both_ways_under_the_unparsed_files()
 {
    let world = with_automations(&[("weekly", WEEKLY), ("broken", BROKEN)]).await;
    let web = Web::start(world).await;

    let html = web.page("/automations").await;

    let tables = classes(&html, "#automations .scroll-box > table");
    assert_eq!(tables.len(), 1, "{tables:?}");
    assert!(
        tables[0].contains(&"table") && tables[0].contains(&"table-pin-rows"),
        "{tables:?}"
    );
    let after_the_unparsed = select(&html, "#automations .unparsed ~ .scroll-box");
    assert_eq!(after_the_unparsed.len(), 1);
    web.stop().await;
}

#[tokio::test]
async fn the_sidebar_links_to_the_automations_page_between_the_runs_and_the_server() {
    let web = Web::start(with_ticket().await).await;

    let html = web.page("/automations").await;

    let links: Vec<&str> = select(&html, ".drawer-side .menu a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(links, ["/", "/runs", "/automations", "/server"]);
    assert_eq!(rows_shown(&html), Vec::<Vec<String>>::new());
    web.stop().await;
}

#[tokio::test]
async fn an_automation_file_that_does_not_parse_is_a_hold_on_every_page_linking_to_the_automations_page()
 {
    let web = Web::start(with_automations(&[("broken", BROKEN)]).await).await;

    for path in ["/", "/runs", "/server", "/automations"] {
        let held = holds(&web.page(path).await);
        assert!(
            held.len() == 1
                && held[0].0.starts_with(
                    ".linewright/automations/broken.md: does not parse: `on.schedule` is"
                )
                && held[0].1 == "/automations",
            "{path}: {held:?}"
        );
    }
    web.stop().await;
}

#[tokio::test]
async fn an_automation_whose_last_run_failed_is_a_hold_linking_to_the_run_until_a_newer_one_starts()
{
    let world = with_automations(&[("weekly", WEEKLY)]).await;
    world.harness.on_automation(|_| failed());
    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap_err();
    let web = Web::start(world).await;

    let held = [(
        "automation weekly: last run weekly/1 failed".to_string(),
        "/runs/weekly/1".to_string(),
    )];
    for path in ["/", "/runs", "/automations"] {
        assert_eq!(holds(&web.page(path).await), held, "{path}");
    }

    web.harness.on_automation(|_| Ok(nothing_needs_doing()));
    let agent = web.harness.hold();
    let beside = web.factory_beside().await;
    let run = tokio::spawn(async move { beside.run_automation(&"weekly".parse().unwrap()).await });
    let in_flight = web
        .eventually_page("/automations", |html| {
            rows_shown(html)[0][2].ends_with("— in flight")
        })
        .await;
    assert_eq!(holds(&in_flight), Vec::<(String, String)>::new());

    drop(agent);
    run.await.unwrap().unwrap();
    let done = web.page("/automations").await;
    assert!(
        rows_shown(&done)[0][2].starts_with("weekly/2 by hand, started ")
            && rows_shown(&done)[0][2].ends_with("— done"),
        "{:?}",
        rows_shown(&done)
    );
    assert_eq!(holds(&done), Vec::<(String, String)>::new());
    web.stop().await;
}

#[tokio::test]
async fn a_run_handed_back_is_a_failed_last_run() {
    let world = with_automations(&[("weekly", WEEKLY)]).await;
    world
        .harness
        .on_automation(|_| Ok("I looked; all is well.".into()));
    world
        .factory
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap_err();
    let web = Web::start(world).await;

    let held = holds(&web.page("/automations").await);

    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0].1, "/runs/weekly/1");
    web.stop().await;
}

#[tokio::test]
async fn before_the_first_poll_the_automations_page_says_there_has_been_none() {
    let world = with_automations(&[("weekly", WEEKLY)]).await;
    let (_unpolled, served) = tokio::sync::watch::channel(None);
    let (_unended, polling) = tokio::sync::watch::channel(None);
    let router = web::router(
        sources(&world.dir, &world.clock, world.clock.now(), served, polling),
        CancellationToken::new(),
    );
    let request = axum::http::Request::get("/automations")
        .header("Host", "localhost")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let html = Html::parse_document(std::str::from_utf8(&body).unwrap());
    assert_eq!(
        text(select(&html, "#automations")[0]),
        "`serve` has not polled yet: ask again after its first poll"
    );
}

#[tokio::test]
async fn the_automations_pages_stream_sends_the_frame_and_the_automations_then_what_changed() {
    let web = Web::start(with_automations(&[("weekly", WEEKLY)]).await).await;
    let mut events = stream(&web, "/events/automations").await;

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<String> = regions(&first).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, ["holds", "automations"]);

    web.harness.on_automation(|_| Ok(nothing_needs_doing()));
    web.factory_beside()
        .await
        .run_automation(&"weekly".parse().unwrap())
        .await
        .unwrap();

    let shown = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if let Some((_, region)) = regions(&event)
                .into_iter()
                .find(|(id, _)| id == "automations")
            {
                return rows_shown(&region);
            }
        }
    })
    .await
    .unwrap();
    assert!(shown[0][2].ends_with("— done"), "{shown:?}");
    web.stop().await;
}
