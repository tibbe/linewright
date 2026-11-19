//! The breadcrumbs and the title of every screen (`docs/specs/shell.md`,
//! Breadcrumbs, and `docs/specs/design_system.md`, Titles).

use super::experiments::with_experiment;
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

/// The region `id` of `regions`.
#[track_caller]
fn region<'a>(regions: &'a [(String, Html)], id: &str) -> &'a Html {
    regions
        .iter()
        .find(|(region, _)| region == id)
        .map(|(_, html)| html)
        .unwrap_or_else(|| panic!("no {id}: {regions:?}"))
}

/// [`with_experiment`]'s pages, with the run `1/triage-1` of `Ship gh`.
#[track_caller]
fn with_a_run() -> impl Future<Output = Pages> {
    let pages = with_experiment();
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
async fn every_screen_shows_its_crumbs_and_is_titled_by_the_last() {
    let pages = with_a_run().await;
    let suites = || link("Suites", "/suites");
    let board = || link("board", "/suites/board");
    let experiment = || link("Experiment 1", "/experiments/1");

    for (path, names, shown) in [
        (
            "/items/1",
            vec![link("Board", "/"), here("#1 Ship gh")],
            "#1 Ship gh · Linewright",
        ),
        ("/runs", vec![here("Runs")], "Runs · Linewright"),
        (
            "/runs/1/triage-1",
            vec![link("Runs", "/runs"), here("1/triage-1")],
            "1/triage-1 · Linewright",
        ),
        (
            "/automations",
            vec![here("Automations")],
            "Automations · Linewright",
        ),
        ("/suites", vec![here("Suites")], "Suites · Linewright"),
        (
            "/suites/board",
            vec![suites(), here("board")],
            "board · Linewright",
        ),
        (
            "/experiments/1",
            vec![suites(), board(), here("Experiment 1")],
            "Experiment 1 · Linewright",
        ),
        (
            "/experiments/1/as-is/columns/1/run",
            vec![
                suites(),
                board(),
                experiment(),
                here("Replay as-is/columns/1's run"),
            ],
            "Replay as-is/columns/1's run · Linewright",
        ),
        (
            "/experiments/1/as-is/columns/1/judge",
            vec![
                suites(),
                board(),
                experiment(),
                here("Replay as-is/columns/1's judge"),
            ],
            "Replay as-is/columns/1's judge · Linewright",
        ),
        ("/server", vec![here("Server")], "Server · Linewright"),
    ] {
        let html = pages.page(path).await;

        assert_eq!(crumbs(&html), names, "{path}");
        assert_eq!(title(&html), shown, "{path}");
    }
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

    assert_eq!(crumbs(&html), [here("Board")]);
    assert_eq!(title(&html), "(1) Board · Linewright");
}

#[test_log::test(tokio::test)]
async fn a_no_such_screen_has_the_crumbs_of_the_screen_its_link_names_ending_in_the_name_it_gave() {
    let pages = with_experiment().await;

    for (path, names) in [
        ("/items/99", vec![link("Board", "/"), here("#99")]),
        (
            "/runs/9/triage-1",
            vec![link("Runs", "/runs"), here("9/triage-1")],
        ),
        (
            "/runs/nothing",
            vec![link("Runs", "/runs"), here("nothing")],
        ),
        (
            "/suites/nothing",
            vec![link("Suites", "/suites"), here("nothing")],
        ),
        (
            "/experiments/9",
            vec![link("Suites", "/suites"), here("Experiment 9")],
        ),
        (
            "/experiments/9/as-is/columns/1/run",
            vec![
                link("Suites", "/suites"),
                here("Replay as-is/columns/1's run"),
            ],
        ),
        (
            "/experiments/1/as-is/columns/2/judge",
            vec![
                link("Suites", "/suites"),
                link("board", "/suites/board"),
                link("Experiment 1", "/experiments/1"),
                here("Replay as-is/columns/2's judge"),
            ],
        ),
    ] {
        let (status, body) = pages.get(path).await;
        let html = Html::parse_document(&body);

        assert_eq!(status, 404, "{path}");
        assert_eq!(crumbs(&html), names, "{path}");
    }
}

/// `docs/specs/web/shell.md`: daisyUI `breadcrumbs`, under the notices and
/// above the screen, the last crumb the current page's.
#[test_log::test(tokio::test)]
async fn the_breadcrumbs_are_a_navigation_landmark_between_the_notices_and_the_screen() {
    let pages = Pages::of(a_ticket()).await;

    let html = pages.page("/items/1").await;

    let navs = select(&html, r#"nav[aria-label="Breadcrumb"]"#);
    assert_eq!(navs.len(), 1, "{}", html.html());
    let nav = navs[0];
    assert!(nav.value().classes().any(|class| class == "breadcrumbs"));
    let current = select_in(nav, r#"[aria-current="page"]"#);
    assert_eq!(current.len(), 1, "{}", nav.html());
    assert_eq!(text(current[0]), "#1 Do the thing");
    let last = select_in(nav, "li").pop().unwrap();
    assert_eq!(select_in(last, "a"), [] as [ElementRef<'_>; 0]);
    let order: Vec<&str> = select(&html, "#holds, nav[aria-label=\"Breadcrumb\"], main")
        .into_iter()
        .map(|element| element.value().name())
        .collect();
    assert_eq!(order, ["div", "nav", "main"]);
}

#[test_log::test(tokio::test)]
async fn an_items_crumb_and_title_follow_a_change_of_its_title_through_its_stream() {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
    let mut events = stream(&pages, "/events/items/1").await;
    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let first = regions(&first);
    let ids: Vec<&str> = first.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["holds", "crumbs", "title", "item", "runs"]);

    pages.publish(view(vec![ticket(1, "Ship it")], vec![]));
    let changed = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    let changed = regions(&changed);
    assert_eq!(
        crumbs(region(&changed, "crumbs")),
        [link("Board", "/"), here("#1 Ship it")]
    );
    assert_eq!(title(region(&changed, "title")), "#1 Ship it · Linewright");
}
