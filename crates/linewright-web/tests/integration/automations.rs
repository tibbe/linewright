use linewright_core::automation::{AutomationFiles, LastScheduledRuns, Unparsed};
use linewright_core::status::View;
use serde_json::json;

use super::*;

/// A world working under `work/REPO`, with the automation files `files` on
/// the default branch.
async fn with_automations(files: &[(&str, &str)]) -> World {
    let world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    let files: Vec<(String, &str)> = files
        .iter()
        .map(|(name, text)| (format!(".linewright/automations/{name}"), *text))
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
                    linewright_core::automation::parse(&name.parse().unwrap(), file).unwrap()
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
async fn record_weekly(pages: &Pages, n: u32, started: &str, ending: Option<&str>) {
    let ended = at(started) + chrono::TimeDelta::minutes(1);
    let record = json!({
        "automation": "weekly.md",
        "n": n,
        "started_by": "hand",
        "started_at": started,
        "end": ending.map(|ending| json!({"at": ended, "ending": ending})),
        "sessions": [],
    });
    pages.record(&record).await;
}

/// The unparsed files, as a reader reads them.
fn unparsed_shown(html: &Html) -> Vec<String> {
    select(html, "#automations .unparsed li")
        .into_iter()
        .map(text)
        .collect()
}

/// Each automation's row: its name, trigger, last run and next run.
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

#[test_log::test(tokio::test)]
async fn the_automations_page_lists_the_unparsed_files_then_each_automation_by_name() {
    let pages = Pages::of(View {
        last_scheduled_runs: ["hourly.md", "team/weekly.md"]
            .map(|name| (name.parse().unwrap(), at("2026-10-01T12:00:00Z")))
            .into_iter()
            .collect::<LastScheduledRuns>(),
        ..with(&[("team/weekly.md", WEEKLY), ("hourly.md", HOURLY)], true)
    })
    .await;
    pages
        .record(&json!({
            "automation": "team/weekly.md",
            "n": 1,
            "started_by": "hand",
            "started_at": "2026-10-01T11:00:00Z",
            "end": {"at": "2026-10-01T11:01:00Z", "ending": "done"},
            "sessions": [],
        }))
        .await;

    let html = pages.page("/automations").await;

    assert_eq!(
        unparsed_shown(&html),
        [format!("`broken.md` does not parse: {BROKEN}")]
    );
    assert_eq!(
        rows_shown(&html),
        [
            [
                "hourly.md",
                "At minute 0 past every hour.",
                "none yet, first seen 2026-10-01 12:00:00 UTC",
                "2026-10-01 13:00:00 UTC"
            ],
            [
                "team/weekly.md",
                "At 06:00, on Monday.",
                "team/weekly.md/1 by hand, started 2026-10-01 11:00:00 UTC — done",
                "2026-10-05 06:00:00 UTC"
            ],
        ]
    );
    let links: Vec<&str> = select(&html, "#automations tbody a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(links, ["/runs/team/weekly.md/1"]);
    let run = pages.page("/runs/team/weekly.md/1").await;
    assert!(
        text(select(&run, "h1")[0]).contains("team/weekly.md/1"),
        "{}",
        run.html()
    );
    assert_eq!(
        select(&html, &format!("#automations tbody {STATUS_ICON}")).len(),
        1
    );
    assert_no_status_by_colour_alone(&html);
    assert_eq!(events_path(&html), Some("/events/automations"));
}

#[test_log::test(tokio::test)]
async fn an_automation_on_workflow_runs_shows_its_trigger_in_words_and_no_first_seen_or_next_run() {
    let fix_ci = "---\non:\n  workflow_run:\n    workflows: [CI]\n    branches: [main]\n    conclusions: [failure]\n---\nFix it.\n";
    let everything = "---\non:\n  workflow_run:\n---\nLook.\n";
    let pages = Pages::of(View {
        last_scheduled_runs: [("fix-ci.md".parse().unwrap(), at("2026-10-01T12:00:00Z"))]
            .into_iter()
            .collect::<LastScheduledRuns>(),
        ..with(
            &[("fix-ci.md", fix_ci), ("every-run.md", everything)],
            false,
        )
    })
    .await;

    let html = pages.page("/automations").await;

    assert_eq!(
        rows_shown(&html),
        [
            [
                "every-run.md",
                "When a workflow run completes.",
                "none yet",
                ""
            ],
            [
                "fix-ci.md",
                "When a workflow run of \"CI\" on \"main\" completes as failure.",
                "none yet",
                ""
            ],
        ]
    );
}

/// `docs/specs/components.md`, Table.
#[test_log::test(tokio::test)]
async fn the_automations_table_is_in_a_card_showing_no_name_under_the_unparsed_files_its_rows_named_by_their_headers()
 {
    let pages = Pages::of(with(&[("weekly.md", WEEKLY), ("hourly.md", HOURLY)], true)).await;

    let html = pages.page("/automations").await;

    assert_eq!(table_cards(&html), [unnamed_card(&[], &[])]);
    assert_eq!(
        select(&html, "#automations .unparsed ~ .table-card table").len(),
        1
    );
    let heads: Vec<String> = select(&html, "#automations thead th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(heads, ["Automation", "Trigger", "Last run", "Next run"]);
    let names: Vec<String> = select(&html, "#automations tbody tr > th:first-child")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(names, ["hourly.md", "weekly.md"]);
    let row = ["start"; 4];
    assert_eq!(alignments(&html, "#automations"), [row, row, row]);
}

#[test_log::test(tokio::test)]
async fn the_automations_table_pins_its_heads_in_a_box_scrolling_both_ways() {
    let pages = Pages::of(with(&[("weekly.md", WEEKLY)], true)).await;

    let html = pages.page("/automations").await;

    let tables = classes(&html, "#automations .scroll-box > table");
    assert_eq!(tables.len(), 1, "{tables:?}");
    assert!(
        tables[0].contains(&"table") && tables[0].contains(&"table-pin-rows"),
        "{tables:?}"
    );
}

#[test_log::test(tokio::test)]
async fn an_automation_file_that_does_not_parse_is_a_hold_on_every_page_linking_to_the_automations_page()
 {
    let pages = Pages::of(with(&[], true)).await;

    for path in ["/", "/runs", "/server", "/automations"] {
        assert_eq!(
            holds(&pages.page(path).await),
            [(
                format!("`broken.md` does not parse: {BROKEN}"),
                "/automations".to_string()
            )],
            "{path}"
        );
    }
}

#[test_log::test(tokio::test)]
async fn an_automation_whose_last_run_failed_is_a_hold_linking_to_the_run_until_a_newer_one_starts()
{
    let pages = Pages::of(with(&[("weekly.md", WEEKLY)], false)).await;
    record_weekly(&pages, 1, "2026-10-01T11:00:00Z", Some("failed")).await;

    let held = [(
        "automation weekly.md: last run weekly.md/1 failed".to_string(),
        "/runs/weekly.md/1".to_string(),
    )];
    for path in ["/", "/runs", "/automations"] {
        assert_eq!(holds(&pages.page(path).await), held, "{path}");
    }

    record_weekly(&pages, 2, "2026-10-01T11:30:00Z", None).await;
    let in_flight = lock_of(&pages.path("runs/automations/weekly.md/2.json"));
    let running = pages.page("/automations").await;
    assert_eq!(
        rows_shown(&running)[0][2],
        "weekly.md/2 by hand, started 2026-10-01 11:30:00 UTC — in flight"
    );
    assert_eq!(holds(&running), Vec::<(String, String)>::new());

    record_weekly(&pages, 2, "2026-10-01T11:30:00Z", Some("done")).await;
    drop(in_flight);
    let done = pages.page("/automations").await;
    assert_eq!(
        rows_shown(&done)[0][2],
        "weekly.md/2 by hand, started 2026-10-01 11:30:00 UTC — done"
    );
    assert_eq!(holds(&done), Vec::<(String, String)>::new());
}

#[test_log::test(tokio::test)]
async fn a_run_handed_back_is_a_failed_last_run() {
    let pages = Pages::of(with(&[("weekly.md", WEEKLY)], false)).await;
    record_weekly(&pages, 1, "2026-10-01T11:00:00Z", Some("handed-back")).await;

    let held = holds(&pages.page("/automations").await);

    assert_eq!(
        held,
        [(
            "automation weekly.md: last run weekly.md/1 handed back".to_string(),
            "/runs/weekly.md/1".to_string()
        )]
    );
}

/// `docs/specs/components.md`, Table, Empty.
#[test_log::test(tokio::test)]
async fn the_automations_page_with_no_automations_says_so() {
    let pages = Pages::of(with(&[], false)).await;

    let html = pages.page("/automations").await;

    assert_eq!(text(select(&html, "#automations p")[0]), "No automations");
}

#[test_log::test(tokio::test)]
async fn before_the_first_poll_the_automations_page_says_there_has_been_none() {
    let dir = tempfile::tempdir().unwrap();
    let clock = std::sync::Arc::new(FakeClock::new());
    let (_unpolled, served) = tokio::sync::watch::channel(None);
    let (_unended, polling) = tokio::sync::watch::channel(None);
    let sources = sources(&dir, &clock, clock.now(), served, polling).await;
    let cookie = signed_in_as(
        &sources,
        linewright_core::access::Role::Viewer,
        "Vera Viewer",
    )
    .await;
    let router = linewright_web::router(sources, &ui(), CancellationToken::new());
    let request = axum::http::Request::get("/automations")
        .header("Host", UI_HOST)
        .header("Cookie", cookie)
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

#[test_log::test(tokio::test)]
async fn the_automations_pages_stream_sends_the_frame_and_the_automations_then_what_changed() {
    let web = Web::start(with_automations(&[("weekly.md", WEEKLY)]).await).await;
    let mut events = stream(&web, "/events/automations").await;

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<String> = regions(&first).into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        ids,
        [
            "holds",
            "title",
            "crumbs",
            "heading",
            "buttons",
            "automations"
        ]
    );

    web.harness.on_automation(|_| Ok(nothing_needs_doing()));
    web.serving
        .automations
        .ask("weekly.md".parse().unwrap())
        .await
        .unwrap()
        .unwrap();

    let mut last_seen = None;
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if let Some((_, region)) = regions(&event)
                .into_iter()
                .find(|(id, _)| id == "automations")
            {
                let shown = rows_shown(&region);
                if !shown[0][2].ends_with("— in flight") {
                    return shown;
                }
                last_seen = Some(shown);
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the run never ended; last shown: {last_seen:?}"));
    let [row] = &ended[..] else {
        panic!("{ended:?}");
    };
    let [name, schedule, last_run, next_run] = &row[..] else {
        panic!("{row:?}");
    };
    assert_eq!(
        [name, schedule, next_run],
        [
            "weekly.md",
            "At 06:00, on Monday.",
            "2026-10-05 06:00:00 UTC"
        ]
    );
    assert!(
        last_run.starts_with("weekly.md/1 by hand, started ") && last_run.ends_with(" UTC — done"),
        "{last_run}"
    );
    web.stop().await;
}

/// `docs/specs/components.md`, Table, Pages.
#[test_log::test(tokio::test)]
async fn the_automations_page_shows_its_automations_25_at_a_time() {
    let names: Vec<String> = (1..=27).map(|n| format!("weekly-{n:02}.md")).collect();
    let parsed: Vec<(&str, &str)> = names.iter().map(|name| (name.as_str(), WEEKLY)).collect();
    let pages = Pages::of(with(&parsed, false)).await;

    let first = pages.page("/automations").await;
    let second = pages.page("/automations?page=2").await;

    let shown = |html: &Html| -> Vec<String> {
        select(html, "#automations tbody tr > th:first-child")
            .into_iter()
            .map(text)
            .collect()
    };
    assert_eq!(shown(&first), names[..25]);
    assert_eq!(shown(&second), ["weekly-26.md", "weekly-27.md"]);
    assert_eq!(
        pages_nav(&second, "#automations"),
        Some(PagesNav {
            label: "Pages of automations".into(),
            showing: "Showing 26–27 of 27 automations".into(),
            items: vec![
                page_link("Previous", "/automations", "/events/automations"),
                page_link("1", "/automations", "/events/automations"),
                page_link("2*", "/automations?page=2", "/events/automations?page=2"),
                no_link("Next"),
            ],
        })
    );
    assert_eq!(events_path(&second), Some("/events/automations?page=2"));
}

/// `docs/specs/design_system.md`, Colour; `docs/specs/automations.md`,
/// Automations, A file that does not parse; `docs/specs/shell.md`, Notices,
/// Their look: an automation file that does not parse is a problem, an `alert`
/// in `alert-error` with `failed`'s icon first, not text coloured `error`.
#[test_log::test(tokio::test)]
async fn an_unparsed_automation_file_is_an_error_alert_with_faileds_icon_first() {
    let pages = Pages::of(with(&[("weekly.md", WEEKLY)], true)).await;

    let html = pages.page("/automations").await;

    let files = select(&html, "#automations .unparsed li");
    assert_eq!(files.len(), 1, "{}", html.html());
    let file = files[0];
    assert!(
        has_classes(file, &["alert", "alert-error"]),
        "{}",
        file.html()
    );
    assert!(!file.html().contains("text-error"), "{}", file.html());
    let icon = select_in(file, ":scope > svg:first-child");
    assert_eq!(
        icon.first().and_then(|icon| icon.attr("data-icon")),
        Some("x-circle"),
        "{}",
        file.html()
    );
}

/// `docs/specs/automations.md`, Automations: a file that does not parse is
/// named by its name in the automations' directory, which tells a file in a
/// directory of its own from one directly in it.
#[test_log::test(tokio::test)]
async fn a_file_in_a_directory_of_the_automations_is_named_with_its_directory() {
    let pages = Pages::of(View {
        automations: AutomationFiles {
            parsed: vec![],
            unparsed: vec![Unparsed {
                path: ".linewright/automations/team/weekly.md".into(),
                reason: "an automation is a `.md` file directly in `.linewright/automations/`"
                    .into(),
            }],
        },
        ..view(vec![], vec![])
    })
    .await;

    let html = pages.page("/automations").await;

    assert_eq!(
        unparsed_shown(&html),
        [
            "`team/weekly.md` does not parse: an automation is a `.md` file directly in \
          `.linewright/automations/`"
        ]
    );
}

/// `docs/specs/automations.md`, The automation file: an automation is named
/// by its path, so `a%2Fb.md` and `a/b.md` are two automations, and the link
/// to each one's run opens that run.
#[test_log::test(tokio::test)]
async fn the_link_to_a_run_opens_that_automations_run_whatever_its_names_characters() {
    let names = ["a%2Fb.md", "a/b.md", "100% #1?.md"];
    let parsed: Vec<(&str, &str)> = names.iter().map(|name| (*name, WEEKLY)).collect();
    let pages = Pages::of(with(&parsed, false)).await;
    for (n, name) in (1..).zip(names) {
        pages
            .record(&json!({
                "automation": name,
                "n": n,
                "started_by": "hand",
                "started_at": "2026-10-01T11:00:00Z",
                "end": {"at": "2026-10-01T11:01:00Z", "ending": "done"},
                "sessions": [],
            }))
            .await;
    }

    let html = pages.page("/automations").await;

    let links: Vec<String> = select(&html, "#automations tbody a")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .map(String::from)
        .collect();
    let mut opened = vec![];
    for link in &links {
        opened.push(text(select(&pages.page(link).await, "h1")[0]));
    }
    let in_rows = ["100% #1?.md/3", "a%2Fb.md/1", "a/b.md/2"];
    assert_eq!(opened.len(), in_rows.len(), "{links:?}");
    for (heading, id) in opened.iter().zip(in_rows) {
        assert!(heading.contains(id), "{heading} for {id}, of {links:?}");
    }
}
