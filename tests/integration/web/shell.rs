//! The shell around every screen: its breadcrumbs, its heading, its buttons
//! and its title (`docs/specs/shell.md`; `docs/specs/design_system.md`,
//! Titles and Headings).

use super::benchmark_runs::with_benchmark_run;
use super::*;

use linewright::domain::Issue;
use serde_json::json;

/// Each crumb's name, and where it links, if it does.
fn crumbs(html: &Html) -> Vec<(String, Option<String>)> {
    select(html, r#"nav[aria-label="Breadcrumb"] li"#)
        .into_iter()
        .map(|crumb| {
            let href = select_in(crumb, "a")
                .first()
                .and_then(|a| a.attr("href"))
                .map(ToString::to_string);
            (text(crumb), href)
        })
        .collect()
}

/// A crumb linking to `href`.
fn link(name: &str, href: &str) -> (String, Option<String>) {
    (name.to_string(), Some(href.to_string()))
}

/// The crumb of the screen itself, which links nowhere.
fn here(name: &str) -> (String, Option<String>) {
    (name.to_string(), None)
}

#[track_caller]
fn title(html: &Html) -> String {
    let titles = select(html, "head title, title#title");
    assert_eq!(titles.len(), 1, "{}", html.html());
    text(titles[0])
}

/// The page's one `h1`.
#[track_caller]
fn h1(html: &Html) -> String {
    let headings = select(html, "h1");
    assert_eq!(headings.len(), 1, "{}", html.html());
    text(headings[0])
}

/// A screen's heading card as a reader reads it, if it shows one.
#[derive(Debug, PartialEq, Eq)]
struct Heading {
    name: String,
    badges: Vec<String>,
    line: Option<String>,
}

/// The heading card of `html`, if it shows one, checking that its name is
/// its `card-title`.
#[track_caller]
fn heading(html: &Html) -> Option<Heading> {
    let card = select(html, "#heading").into_iter().next()?;
    if card.attr("hidden").is_some() {
        return None;
    }
    assert!(has_classes(card, &["card"]), "{}", card.html());
    let names = select_in(card, ".card-title");
    assert_eq!(names.len(), 1, "{}", card.html());
    Some(Heading {
        name: text(names[0]),
        badges: select_in(card, ".badge").into_iter().map(text).collect(),
        line: select_in(card, ".line").first().map(|line| text(*line)),
    })
}

/// A heading card named `name`, with `badges` and `line`.
fn card(name: &str, badges: &[&str], line: Option<&str>) -> Heading {
    Heading {
        name: name.into(),
        badges: badges.iter().map(ToString::to_string).collect(),
        line: line.map(Into::into),
    }
}

/// The screen's buttons, each its label and the dialog it opens.
#[track_caller]
fn buttons(html: &Html) -> Vec<(String, String)> {
    select(html, "#buttons button")
        .into_iter()
        .map(|button| {
            assert_eq!(
                button.attr("command"),
                Some("show-modal"),
                "{}",
                button.html()
            );
            (text(button), button.attr("commandfor").unwrap().to_string())
        })
        .collect()
}

/// [`with_benchmark_run`]'s pages, with the run `1/triage-1` of `Ship gh`.
#[track_caller]
pub(super) fn with_a_run() -> impl Future<Output = Pages> {
    let pages = with_benchmark_run();
    async move {
        let pages = pages.await;
        pages
            .record(&json!({
                "item": 1,
                "stage": "triage",
                "n": 1,
                "started_at": "2026-10-14T12:00:00Z",
                "end": {"at": "2026-10-14T12:00:20Z", "ending": "handed-back"},
                "sessions": [],
            }))
            .await;
        pages
    }
}

#[test_log::test(tokio::test)]
async fn every_screen_shows_its_crumbs_and_its_h1_and_is_titled_by_its_heading() {
    let pages = with_a_run().await;
    let benchmarks = || link("Benchmarks", "/benchmarks");
    let board = || link("board", "/benchmarks/board");
    let benchmark_run = || link("Benchmark run 1", "/benchmark-runs/1");

    for (path, names, named, shown) in [
        ("/items/1", vec![], "#1: Ship gh", "#1 Ship gh · Linewright"),
        (
            "/runs",
            vec![link("Runs", "/runs"), here("All runs")],
            "All runs",
            "Runs · Linewright",
        ),
        (
            "/runs/1/triage-1",
            vec![],
            "1/triage-1 (it started no session)",
            "1/triage-1 · Linewright",
        ),
        (
            "/automations",
            vec![link("Automations", "/automations"), here("All automations")],
            "All automations",
            "Automations · Linewright",
        ),
        (
            "/benchmarks",
            vec![benchmarks(), here("All benchmarks")],
            "All benchmarks",
            "Benchmarks · Linewright",
        ),
        ("/benchmarks/board", vec![], "board", "board · Linewright"),
        (
            "/benchmark-runs/1",
            vec![benchmarks(), board(), here("Benchmark run")],
            "Benchmark run",
            "Benchmark run 1 · Linewright",
        ),
        (
            "/benchmark-runs/1/as-is/columns/1/run",
            vec![benchmarks(), board(), benchmark_run(), here("Trial")],
            "Trial",
            "Trial as-is/columns/1's run · Linewright",
        ),
        (
            "/benchmark-runs/1/as-is/columns/1/scorer",
            vec![benchmarks(), board(), benchmark_run(), here("Trial")],
            "Trial",
            "Trial as-is/columns/1's scorer · Linewright",
        ),
        (
            "/server",
            vec![link("Server", "/server"), here("Status")],
            "Status",
            "Server · Linewright",
        ),
    ] {
        let html = pages.page(path).await;

        assert_eq!(crumbs(&html), names, "{path}");
        assert_eq!(h1(&html), named, "{path}");
        assert_eq!(title(&html), shown, "{path}");
    }
}

/// `docs/specs/shell.md`, Breadcrumbs: the last crumb is the page's `h1`.
#[test_log::test(tokio::test)]
async fn the_last_crumb_is_the_pages_h1_and_links_nowhere() {
    let pages = with_benchmark_run().await;

    let html = pages.page("/benchmark-runs/1").await;

    let nav = select(&html, r#"nav[aria-label="Breadcrumb"]"#)[0];
    let last = select_in(nav, "li").pop().unwrap();
    let h1 = select_in(last, "h1");
    assert_eq!(h1.len(), 1, "{}", nav.html());
    assert_eq!(h1[0].attr("aria-current"), Some("page"));
    assert_eq!(select_in(last, "a"), [] as [ElementRef<'_>; 0]);
}

#[test_log::test(tokio::test)]
async fn the_boards_title_leads_with_its_count_of_cards_in_needs_a_person() {
    let pages = Pages::of(view(
        vec![
            labeled_by_linewright(Issue {
                labels: vec!["needs-info".into()],
                ..ticket(1, "Do the thing")
            }),
            ticket(2, "Do another thing"),
        ],
        vec![],
    ))
    .await;

    let html = pages.page("/").await;

    assert_eq!(crumbs(&html), [link("Board", "/"), here("All items")]);
    assert_eq!(title(&html), "(1) Board · Linewright");
}

/// `docs/specs/design_system.md`, Headings.
#[test_log::test(tokio::test)]
async fn each_screen_of_one_thing_has_a_heading_card_and_no_other_screen_has_one() {
    let pages = with_a_run().await;

    for (path, shown) in [
        ("/runs", None),
        ("/benchmarks", None),
        ("/server", None),
        (
            "/benchmarks/board",
            Some(card("board", &[], Some("implement"))),
        ),
        (
            "/benchmark-runs/1",
            Some(card("Benchmark run 1", &["waiting"], None)),
        ),
        (
            "/runs/1/triage-1",
            Some(card(
                "1/triage-1 (it started no session)",
                &[],
                Some("#1: Ship gh"),
            )),
        ),
    ] {
        let html = pages.page(path).await;

        assert_eq!(heading(&html), shown, "{path}");
    }
    let item = heading(&pages.page("/items/1").await).unwrap();
    assert_eq!(item.name, "#1: Ship gh");
    assert!(
        item.line
            .as_ref()
            .is_some_and(|line| line.starts_with("as of ")),
        "{item:?}"
    );
}

/// `docs/specs/shell.md`, Layout and The screen's buttons;
/// `docs/specs/web/design_system.md`, Buttons and Making.
#[test_log::test(tokio::test)]
async fn the_screens_buttons_sit_at_the_right_of_its_breadcrumbs_or_in_its_heading() {
    let pages = with_benchmark_run().await;

    let benchmarks = pages.page("/benchmarks").await;
    let benchmark_run = pages.page("/benchmark-runs/1").await;

    assert_eq!(
        buttons(&benchmarks),
        [("New".to_string(), "new-benchmark".to_string())]
    );
    let new = select(&benchmarks, "#buttons button")[0];
    assert!(
        has_classes(new, &["btn", "btn-sm", "btn-primary"]),
        "{}",
        new.html()
    );
    assert_eq!(select_in(new, "svg"), [] as [ElementRef<'_>; 0]);
    let crumbs_row = select(&benchmarks, "#crumbs + #buttons");
    assert_eq!(crumbs_row.len(), 1, "{}", benchmarks.html());

    assert_eq!(
        buttons(&benchmark_run),
        [("Cancel".to_string(), "cancel-benchmark-run".to_string())]
    );
    let cancel = select(&benchmark_run, "#buttons button")[0];
    assert!(has_classes(cancel, &["btn", "btn-sm"]), "{}", cancel.html());
    assert!(lacks_classes(cancel, &["btn-primary"]), "{}", cancel.html());
    let icon = select_in(cancel, ":scope > svg:first-child");
    assert_eq!(icon.len(), 1, "{}", cancel.html());
    assert_eq!(
        select(&benchmark_run, "#heading #buttons"),
        [] as [ElementRef<'_>; 0]
    );
}

#[test_log::test(tokio::test)]
async fn a_screen_without_breadcrumbs_has_its_buttons_in_its_heading_card() {
    let pages = super::run::implemented_as_recorded().await;

    let html = pages.page("/runs/1/implement-1").await;

    assert_eq!(
        select(&html, "#heading #buttons").len(),
        1,
        "{}",
        html.html()
    );
}

#[test_log::test(tokio::test)]
async fn a_no_such_screen_has_the_crumbs_of_the_screen_its_link_names_without_a_crumb_for_it() {
    let pages = with_benchmark_run().await;

    for (path, names) in [
        ("/items/99", vec![]),
        ("/runs/9/triage-1", vec![]),
        ("/runs/nothing", vec![]),
        ("/benchmarks/nothing", vec![]),
        (
            "/benchmark-runs/9",
            vec![link("Benchmarks", "/benchmarks"), here("Benchmark run")],
        ),
        (
            "/benchmark-runs/9/as-is/columns/1/run",
            vec![link("Benchmarks", "/benchmarks"), here("Trial")],
        ),
        (
            "/benchmark-runs/1/as-is/columns/2/scorer",
            vec![
                link("Benchmarks", "/benchmarks"),
                link("board", "/benchmarks/board"),
                link("Benchmark run 1", "/benchmark-runs/1"),
                here("Trial"),
            ],
        ),
    ] {
        let (status, body) = pages.get(path).await;
        let html = Html::parse_document(&body);

        assert_eq!(status, 404, "{path}");
        assert_eq!(crumbs(&html), names, "{path}");
        assert_eq!(select(&html, "h1").len(), 1, "{path}");
    }
}

/// `docs/specs/shell.md`, Layout; `docs/specs/web/shell.md`.
#[test_log::test(tokio::test)]
async fn the_repositorys_name_then_the_notices_then_the_breadcrumbs_lead_the_content() {
    let pages = Pages::of(a_ticket()).await;

    let html = pages.page("/runs").await;

    let navs = select(&html, r#"nav[aria-label="Breadcrumb"]"#);
    assert_eq!(navs.len(), 1, "{}", html.html());
    assert!(has_classes(navs[0], &["breadcrumbs"]));
    let order: Vec<&str> = select(
        &html,
        ".drawer-content .repo-name, #holds, nav[aria-label=\"Breadcrumb\"], main",
    )
    .into_iter()
    .map(|element| element.value().name())
    .collect();
    assert_eq!(order, ["p", "div", "nav", "main"]);
    assert_eq!(text(select(&html, ".drawer-content .repo-name")[0]), REPO);
}

/// `docs/specs/web/shell.md`: the rows of the name at the top and of the
/// breadcrumbs are at least as tall as a small button.
#[test_log::test(tokio::test)]
async fn the_rows_of_the_name_at_the_top_and_of_the_breadcrumbs_are_as_tall_as_a_small_button() {
    let pages = Pages::of(a_ticket()).await;

    let html = pages.page("/runs").await;

    for row in [
        select(&html, ".drawer-content .repo-name")[0],
        ElementRef::wrap(select(&html, "#crumbs")[0].parent().unwrap()).unwrap(),
    ] {
        assert!(has_classes(row, &["min-h-8"]), "{}", row.html());
    }
}

#[test_log::test(tokio::test)]
async fn every_stream_sends_the_shells_regions_first() {
    let pages = with_a_run().await;

    for path in [
        "/events/",
        "/events/runs",
        "/events/automations",
        "/events/benchmarks",
        "/events/benchmarks/board",
        "/events/benchmark-runs/1",
        "/events/items/1",
        "/events/runs/1/triage-1",
        "/events/server",
    ] {
        let mut events = stream(&pages, path).await;
        let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        let first = regions(&first);
        let ids: Vec<&str> = first.iter().take(5).map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            ["holds", "title", "crumbs", "heading", "buttons"],
            "{path}"
        );
    }
}

#[test_log::test(tokio::test)]
async fn an_items_heading_and_title_follow_a_change_of_its_title_through_its_stream() {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
    let mut events = stream(&pages, "/events/items/1").await;
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    pages.publish(view(vec![ticket(1, "Ship it")], vec![]));
    let changed = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    let changed = regions(&changed);
    let heading = region_of(&changed, "heading");
    assert_eq!(text(select(heading, "h1.card-title")[0]), "#1: Ship it");
    assert_eq!(
        title(region_of(&changed, "title")),
        "#1 Ship it · Linewright"
    );
}

/// `docs/specs/benchmarks.md`, Benchmark run, Heading: "Cancel" goes once
/// the benchmark run ends, and its dialog stays outside every region.
#[test_log::test(tokio::test)]
async fn cancel_goes_from_the_screens_buttons_through_the_stream_once_the_benchmark_run_ends() {
    let pages = with_benchmark_run().await;
    let html = pages.page("/benchmark-runs/1").await;
    let mut events = stream(&pages, events_path(&html).unwrap()).await;
    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let first = regions(&first);
    assert_eq!(
        select(region_of(&first, "buttons"), "button")
            .into_iter()
            .map(text)
            .collect::<Vec<_>>(),
        ["Cancel"]
    );
    for (id, region) in &first {
        assert_eq!(select(region, "dialog"), [] as [ElementRef<'_>; 0], "{id}");
    }
    assert_eq!(select(&html, "dialog#cancel-benchmark-run").len(), 1);

    linewright::benchmarks::BenchmarkRuns::new(pages.sources.runs.database().clone())
        .cancel(
            "1".parse().unwrap(),
            "2026-11-17T14:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();

    let sent = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let sent = regions(&events.recv().await.unwrap());
            if sent.iter().any(|(id, _)| id == "buttons") {
                break sent;
            }
        }
    })
    .await
    .expect("the buttons sent again");
    assert_eq!(
        select(region_of(&sent, "buttons"), "button"),
        [] as [ElementRef<'_>; 0]
    );
    assert_eq!(
        text(select(region_of(&sent, "heading"), ".badge")[0]),
        "cancelled"
    );
}

/// "Cancel" opens the dialog its page drew, so a stream offers it only to a
/// page that drew it, such as one that showed the benchmark run.
#[test_log::test(tokio::test)]
async fn the_stream_of_a_benchmark_run_page_without_the_cancel_dialog_offers_no_cancel() {
    let pages = with_benchmark_run().await;
    let (_, body) = pages.get("/benchmark-runs/9").await;
    let missing = Html::parse_document(&body);
    let mut events = stream(&pages, "/events/benchmark-runs/1").await;

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(events_path(&missing), Some("/events/benchmark-runs/9"));
    assert_eq!(
        select(region_of(&regions(&first), "buttons"), "button"),
        [] as [ElementRef<'_>; 0]
    );
}

/// `docs/specs/shell.md`, Breadcrumbs, No such thing: a run's "No such"
/// page keeps no heading as its stream goes on, as the screen shows none.
#[test_log::test(tokio::test)]
async fn a_missing_runs_stream_sends_no_heading_card() {
    let pages = with_benchmark_run().await;
    let mut events = stream(&pages, "/events/runs/9/triage-1").await;

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    let first = regions(&first);
    let heading = select(region_of(&first, "heading"), "#heading")[0];
    assert!(heading.attr("hidden").is_some(), "{}", heading.html());
    assert_eq!(title(region_of(&first, "title")), "9/triage-1 · Linewright");
}

/// The navigation the drawer of `html` is drawn as: its `data-navigation`.
#[track_caller]
fn navigation(html: &Html) -> Option<String> {
    let drawers = select(html, ".drawer");
    assert_eq!(drawers.len(), 1, "{}", html.html());
    drawers[0].attr("data-navigation").map(ToString::to_string)
}

/// `docs/specs/web/shell.md`, Remembered; `docs/specs/web/architecture.md`,
/// The reader's choice of navigation: every page, "No such" too, is drawn
/// with the rail for a reader whose cookie chose it, whatever their other
/// cookies hold, and with the sidebar otherwise.
#[test_log::test(tokio::test)]
async fn every_page_draws_the_navigation_the_readers_cookie_chose_and_otherwise_the_sidebar() {
    let pages = with_a_run().await;

    for path in [
        "/",
        "/items/1",
        "/runs",
        "/runs/1/triage-1",
        "/automations",
        "/benchmarks",
        "/benchmarks/board",
        "/benchmark-runs/1",
        "/benchmark-runs/1/as-is/columns/1/run",
        "/server",
        "/items/99",
    ] {
        for (cookie, shown) in [
            ("", None),
            ("navigation=rail", Some("rail")),
            ("name=Zoë; navigation=rail", Some("rail")),
        ] {
            let html = page_with_cookie(&pages, path, cookie).await;
            assert_eq!(navigation(&html).as_deref(), shown, "{path}: {cookie}");
        }
    }
}

/// `docs/specs/web/shell.md`, Remembered: the reader's choice, posted to
/// `/navigation`, comes back as a first-party cookie for a year, which the
/// script cannot read, and a page asked for with it is drawn with that
/// choice. A choice that is neither is refused, with no cookie.
#[test_log::test(tokio::test)]
async fn the_server_remembers_the_readers_choice_of_navigation_for_a_year() {
    let pages = with_a_run().await;

    for (choice, shown) in [("rail", Some("rail")), ("sidebar", None)] {
        let (status, headers, _) =
            post_answer(&pages, "/navigation", &[], &[("navigation", choice)]).await;

        assert_eq!(status, 204, "{choice}");
        let cookies: Vec<_> = headers
            .get_all("set-cookie")
            .iter()
            .map(|cookie| cookie.to_str().unwrap().to_string())
            .collect();
        assert_eq!(
            cookies,
            [format!(
                "navigation={choice}; Path=/; Max-Age=31536000; SameSite=Lax; HttpOnly"
            )]
        );
        let cookie = cookies[0].split(';').next().unwrap();
        let html = page_with_cookie(&pages, "/runs", cookie).await;
        assert_eq!(navigation(&html).as_deref(), shown, "{choice}");
    }

    for fields in [&[("navigation", "drawer")][..], &[]] {
        let (status, headers, _) = post_answer(&pages, "/navigation", &[], fields).await;
        assert_eq!(status, 400, "{fields:?}");
        assert!(headers.get("set-cookie").is_none(), "{fields:?}");
    }
}
