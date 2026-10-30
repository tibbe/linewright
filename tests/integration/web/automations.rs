use super::*;

use linewright::automation::{AutomationFiles, LastScheduledRuns, Unparsed};
use linewright::status::View;
use serde_json::json;

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

pub(super) const WEEKLY: &str = "---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n";

/// Why `.linewright/automations/broken.md` does not parse.
const BROKEN: &str = "`on.schedule` is \"every hour\", not a cron expression";

/// A view whose automations are `parsed`, each a name and its file, and
/// the file `.linewright/automations/broken.md` if `broken`.
pub(super) fn with(parsed: &[(&str, &str)], broken: bool) -> View {
    View {
        automations: AutomationFiles {
            parsed: parsed
                .iter()
                .map(|(name, file)| {
                    linewright::automation::parse(&name.parse().unwrap(), file).unwrap()
                })
                .collect(),
            unparsed: if broken {
                vec![Unparsed {
                    path: ".linewright/automations/broken.md".into(),
                    reason: BROKEN.into(),
                }]
            } else {
                vec![]
            },
        },
        ..view(vec![], vec![])
    }
}

/// Writes the record of weekly's run `n`, by hand, started at `started`,
/// ended a minute later as `ending` or, if none, still in flight.
fn record_weekly(pages: &Pages, n: u32, started: &str, ending: Option<&str>) {
    let ended = at(started) + chrono::TimeDelta::minutes(1);
    let record = json!({
        "automation": "weekly",
        "n": n,
        "started_by": "hand",
        "started_at": started,
        "end": ending.map(|ending| json!({"at": ended, "ending": ending})),
        "sessions": [],
    });
    pages.record(&record);
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
    let pages = Pages::of(View {
        last_scheduled_runs: ["hourly", "weekly"]
            .map(|name| (name.parse().unwrap(), at("2026-10-01T12:00:00Z")))
            .into_iter()
            .collect::<LastScheduledRuns>(),
        ..with(&[("weekly", WEEKLY), ("hourly", HOURLY)], true)
    })
    .await;
    record_weekly(&pages, 1, "2026-10-01T11:00:00Z", Some("done"));

    let html = pages.page("/automations").await;

    assert_eq!(
        unparsed_shown(&html),
        [format!(
            ".linewright/automations/broken.md: does not parse: {BROKEN}"
        )]
    );
    assert_eq!(
        rows_shown(&html),
        [
            [
                "hourly",
                "At minute 0 past every hour.",
                "none yet, first seen 2026-10-01 12:00:00 UTC",
                "2026-10-01 13:00:00 UTC"
            ],
            [
                "weekly",
                "At 06:00, on Monday.",
                "weekly/1 by hand, started 2026-10-01 11:00:00 UTC — done",
                "2026-10-05 06:00:00 UTC"
            ],
        ]
    );
    let links: Vec<&str> = select(&html, "#automations tbody a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(links, ["/runs/weekly/1"]);
    assert_eq!(select(&html, "#automations tbody svg.icon").len(), 1);
    assert_no_status_by_colour_alone(&html);
    assert_eq!(events_path(&html), Some("/events/automations"));
}

#[tokio::test]
async fn the_automations_table_pins_its_heads_in_a_box_scrolling_both_ways_under_the_unparsed_files()
 {
    let pages = Pages::of(with(&[("weekly", WEEKLY)], true)).await;

    let html = pages.page("/automations").await;

    let tables = classes(&html, "#automations .scroll-box > table");
    assert_eq!(tables.len(), 1, "{tables:?}");
    assert!(
        tables[0].contains(&"table") && tables[0].contains(&"table-pin-rows"),
        "{tables:?}"
    );
    let after_the_unparsed = select(&html, "#automations .unparsed ~ .scroll-box");
    assert_eq!(after_the_unparsed.len(), 1);
}

#[tokio::test]
async fn an_automation_file_that_does_not_parse_is_a_hold_on_every_page_linking_to_the_automations_page()
 {
    let pages = Pages::of(with(&[], true)).await;

    for path in ["/", "/runs", "/server", "/automations"] {
        assert_eq!(
            holds(&pages.page(path).await),
            [(
                format!(".linewright/automations/broken.md: does not parse: {BROKEN}"),
                "/automations".to_string()
            )],
            "{path}"
        );
    }
}

#[tokio::test]
async fn an_automation_whose_last_run_failed_is_a_hold_linking_to_the_run_until_a_newer_one_starts()
{
    let pages = Pages::of(with(&[("weekly", WEEKLY)], false)).await;
    record_weekly(&pages, 1, "2026-10-01T11:00:00Z", Some("failed"));

    let held = [(
        "automation weekly: last run weekly/1 failed".to_string(),
        "/runs/weekly/1".to_string(),
    )];
    for path in ["/", "/runs", "/automations"] {
        assert_eq!(holds(&pages.page(path).await), held, "{path}");
    }

    record_weekly(&pages, 2, "2026-10-01T11:30:00Z", None);
    let in_flight = lock_of(&pages.path("runs/automations/weekly/2.json"));
    let running = pages.page("/automations").await;
    assert_eq!(
        rows_shown(&running)[0][2],
        "weekly/2 by hand, started 2026-10-01 11:30:00 UTC — in flight"
    );
    assert_eq!(holds(&running), Vec::<(String, String)>::new());

    record_weekly(&pages, 2, "2026-10-01T11:30:00Z", Some("done"));
    drop(in_flight);
    let done = pages.page("/automations").await;
    assert_eq!(
        rows_shown(&done)[0][2],
        "weekly/2 by hand, started 2026-10-01 11:30:00 UTC — done"
    );
    assert_eq!(holds(&done), Vec::<(String, String)>::new());
}

#[tokio::test]
async fn a_run_handed_back_is_a_failed_last_run() {
    let pages = Pages::of(with(&[("weekly", WEEKLY)], false)).await;
    record_weekly(&pages, 1, "2026-10-01T11:00:00Z", Some("handed-back"));

    let held = holds(&pages.page("/automations").await);

    assert_eq!(
        held,
        [(
            "automation weekly: last run weekly/1 handed back".to_string(),
            "/runs/weekly/1".to_string()
        )]
    );
}

#[tokio::test]
async fn before_the_first_poll_the_automations_page_says_there_has_been_none() {
    let dir = tempfile::tempdir().unwrap();
    let clock = std::sync::Arc::new(FakeClock::new());
    let (_unpolled, served) = tokio::sync::watch::channel(None);
    let (_unended, polling) = tokio::sync::watch::channel(None);
    let router = web::router(
        sources(&dir, &clock, clock.now(), served, polling),
        UiHosts::default(),
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
