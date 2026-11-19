//! How the screens are drawn: their cards, the content panel and its
//! spacing, and the sidebar (`docs/specs/web/design_system.md`, Cards and
//! Layout; `docs/specs/web/shell.md`).

use super::shell::with_a_run;
use super::*;

/// A page of each kind of screen in `with_a_run`.
const PATHS: [&str; 9] = [
    "/",
    "/items/1",
    "/runs",
    "/runs/1/triage-1",
    "/automations",
    "/benchmarks",
    "/benchmarks/board",
    "/benchmark-runs/1",
    "/server",
];

/// `class` without its variants, such as `p-10` of `lg:p-10`.
fn utility(class: &str) -> &str {
    class.rsplit(':').next().unwrap_or(class)
}

/// The classes of `element` that set its padding (`p`) or margin (`m`),
/// at any width.
fn spacing(element: ElementRef<'_>, property: char) -> Vec<&str> {
    element
        .value()
        .classes()
        .filter(|class| {
            utility(class)
                .trim_start_matches('-')
                .strip_prefix(property)
                .is_some_and(|rest| {
                    rest.trim_start_matches(['t', 'r', 'b', 'l', 'x', 'y', 's', 'e'])
                        .starts_with('-')
                })
        })
        .collect()
}

/// The classes of `element` that draw a border or a shadow.
fn borders_and_shadows(element: ElementRef<'_>) -> Vec<&str> {
    element
        .value()
        .classes()
        .filter(|class| {
            let class = utility(class);
            class.starts_with("border") || class.starts_with("shadow")
        })
        .collect()
}

/// Asserts that `element` is drawn as a card is, in the colour `colour`:
/// with daisyUI's `shadow-xs` and no border.
#[track_caller]
pub(super) fn assert_drawn_as_a_card(element: ElementRef<'_>, colour: &str, context: &str) {
    assert!(
        has_classes(element, &[colour]),
        "{context}: in {colour}: {}",
        element.html()
    );
    assert_eq!(
        borders_and_shadows(element),
        ["shadow-xs"],
        "{context}: {}",
        element.html()
    );
}

/// `docs/specs/web/design_system.md`, Cards: a card is a daisyUI `card` in
/// the Card colour, with daisyUI's `shadow-xs` and no border. Tables, the
/// heading, Item's and Server's facts, Benchmark run's `stats` and its chart
/// are each such a card (`docs/specs/web/benchmarks.md`, Benchmark run;
/// `docs/specs/web/board.md`, Item; `docs/specs/web/operation.md`).
#[test_log::test(tokio::test)]
async fn every_card_is_in_the_card_colour_with_a_light_shadow_and_no_border() {
    let pages = with_a_run().await;

    for (path, cards) in [
        ("/runs", vec![".table-card"]),
        ("/items/1", vec!["#heading", "#item dl", ".table-card"]),
        ("/server", vec!["#server dl"]),
        (
            "/benchmark-runs/1",
            vec![
                "#heading",
                "#benchmark-run .stats",
                "#benchmark-run-chart",
                ".table-card",
            ],
        ),
    ] {
        let html = pages.page(path).await;
        for selector in cards {
            let selected = select(&html, selector);
            assert!(!selected.is_empty(), "{path}: {selector}");
            for card in selected {
                assert_drawn_as_a_card(card, "bg-card", &format!("{path}: {selector}"));
            }
        }
        for card in select(&html, ".card") {
            assert_drawn_as_a_card(card, "bg-card", path);
        }
    }
}

/// `docs/specs/web/design_system.md`, Cards: a card's title has an icon
/// before it.
#[test_log::test(tokio::test)]
async fn every_cards_title_has_an_icon_before_it() {
    let pages = with_a_run().await;

    let mut titles = 0;
    for path in PATHS {
        let html = pages.page(path).await;
        for title in select(&html, ".card-title") {
            titles += 1;
            let first = title
                .children()
                .find(|child| {
                    child
                        .value()
                        .as_text()
                        .is_none_or(|text| !text.trim().is_empty())
                })
                .and_then(ElementRef::wrap);
            assert!(
                first.is_some_and(|first| first.value().name() == "svg"
                    && first.attr("aria-hidden") == Some("true")),
                "{path}: {}",
                title.html()
            );
        }
    }
    assert!(titles > 0, "the pages have cards with titles");
}

/// `docs/specs/web/benchmarks.md`, Benchmark run: the chart is in a card
/// whose title is "Pass rate vs" and the choice of axis, and it runs to the
/// card's left, right and bottom edges. The choice is beside the title's
/// heading, not in it, so that the heading reads as its words alone.
#[test_log::test(tokio::test)]
async fn the_chart_is_in_a_card_titled_by_its_axis_running_to_its_edges() {
    let pages = with_a_run().await;

    let html = pages.page("/benchmark-runs/1").await;

    let card = select(&html, "#benchmark-run-chart")[0];
    let title = select_in(card, ".card-title");
    assert_eq!(title.len(), 1, "{}", card.html());
    assert_eq!(
        select_in(title[0], "select").len(),
        0,
        "{}",
        title[0].html()
    );
    let axis = title[0]
        .next_siblings()
        .find_map(ElementRef::wrap)
        .expect("the choice after the title");
    assert_eq!(axis.value().name(), "select", "{}", card.html());
    let label = select_in(title[0], "label")[0];
    assert_eq!(text(label), "Pass rate vs");
    assert_eq!(label.attr("for"), axis.attr("id"));
    let chart = select(&html, "#chart")[0];
    assert_eq!(spacing(card, 'p'), Vec::<&str>::new(), "{}", card.html());
    assert_eq!(
        [spacing(chart, 'p'), spacing(chart, 'm')].concat(),
        Vec::<&str>::new(),
        "{}",
        chart.html()
    );
}

/// `docs/specs/web/design_system.md`, Layout: the content is a panel in the
/// Page colour, `p-4`, or `p-10` from `lg`, and from `lg` with `mt-2` above
/// it, a Border along its top and start and its top start corner
/// `rounded-ss-4xl` in a squircle. The parts of a screen are `gap-6` apart.
#[test_log::test(tokio::test)]
async fn the_content_is_a_panel_in_the_page_colour_with_its_spacing_and_from_lg_its_border_and_corner()
 {
    let pages = with_a_run().await;

    for path in PATHS {
        let html = pages.page(path).await;
        let panels = select(&html, ".drawer-content > .content");
        assert_eq!(panels.len(), 1, "{path}");
        let panel = panels[0];
        assert!(
            has_classes(
                panel,
                &[
                    "bg-base-100",
                    "p-4",
                    "lg:p-10",
                    "lg:mt-2",
                    "lg:border-t",
                    "lg:border-s",
                    "border-base-300",
                    "lg:rounded-ss-4xl",
                    "lg:[corner-start-start-shape:squircle]",
                    "flex",
                    "flex-col",
                    "gap-6",
                ]
            ),
            "{path}: {}",
            panel.html()
        );
        assert_eq!(
            select_in(panel, "#holds")[0].inner_html(),
            "",
            "{path}: with no notices, they take no gap"
        );
        let main = select_in(panel, "main");
        assert_eq!(main.len(), 1, "{path}");
        assert!(
            has_classes(main[0], &["flex", "flex-col", "gap-6"]),
            "{path}: {}",
            main[0].html()
        );
        assert_eq!(
            [spacing(main[0], 'p'), spacing(main[0], 'm')].concat(),
            Vec::<&str>::new(),
            "{path}: the panel's padding is the content's"
        );
        for part in main[0].children().filter_map(ElementRef::wrap) {
            assert_eq!(
                spacing(part, 'm'),
                Vec::<&str>::new(),
                "{path}: the gap sets the parts apart: {}",
                part.html()
            );
        }
    }
}

/// `docs/specs/web/shell.md`: the sidebar is in the Card colour, `w-64`,
/// `px-2` and `pt-10`, the repository `mx-5` with `gap-2` under it and its
/// avatar `size-4` as the links' icons are, and the links a `menu` with
/// daisyUI's padding.
#[test_log::test(tokio::test)]
async fn the_sidebar_is_in_the_card_colour_with_the_specs_spacing_and_daisyuis_menu() {
    let pages = with_a_run().await;

    let html = pages.page("/runs").await;

    let nav = select(&html, ".drawer-side nav")[0];
    assert!(
        has_classes(nav, &["bg-card", "w-64", "px-2", "pt-10", "gap-2"]),
        "{}",
        nav.html()
    );
    let repo = select_in(nav, "a.repo")[0];
    assert!(has_classes(repo, &["mx-5"]), "{}", repo.html());
    let avatar = select_in(repo, ".avatar > div")[0];
    let sizes: Vec<&str> = avatar
        .value()
        .classes()
        .filter(|class| class.contains("size-"))
        .collect();
    assert_eq!(sizes, ["size-4"], "{}", avatar.html());
    let menu = select_in(nav, "ul.menu")[0];
    assert_eq!(spacing(menu, 'p'), Vec::<&str>::new(), "{}", menu.html());
    let icons = select_in(menu, "a > svg");
    assert_eq!(icons.len(), 5, "{}", menu.html());
    for icon in icons {
        assert!(has_classes(icon, &["size-4"]), "{}", icon.html());
    }
}
