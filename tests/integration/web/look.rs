//! How the screens are drawn: their cards, the content panel and its
//! spacing, and the sidebar (`docs/specs/web/design_system.md`, Cards and
//! Layout; `docs/specs/web/shell.md`).

use super::shell::with_a_run;
use super::*;

/// A page of each kind of screen in `with_a_run`.
const PATHS: [&str; 10] = [
    "/",
    "/items/1",
    "/runs",
    "/runs/1/triage-1",
    "/automations",
    "/benchmarks",
    "/benchmarks/1",
    "/tasks/1",
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
/// `bg-base-200`, with daisyUI's `shadow-xs` and no border. Tables, the
/// heading, Item's and Server's facts, Benchmark run's `stats` and its chart
/// are each such a card (`docs/specs/web/benchmarks.md`, Benchmark run;
/// `docs/specs/web/board.md`, Item; `docs/specs/web/operation.md`).
#[test_log::test(tokio::test)]
async fn every_card_is_in_base_200_with_a_light_shadow_and_no_border() {
    let pages = with_a_run().await;

    for (path, cards) in [
        ("/runs", vec![".table-card"]),
        ("/items/1", vec!["#heading", "#item dl", ".table-card"]),
        ("/server", vec!["#server dl"]),
        ("/tasks/1", vec!["#heading", "#task .stats", "#criteria"]),
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
                assert_drawn_as_a_card(card, "bg-base-200", &format!("{path}: {selector}"));
            }
        }
        for card in select(&html, ".card") {
            assert_drawn_as_a_card(card, "bg-base-200", path);
            let sizes: Vec<&str> = card
                .value()
                .classes()
                .filter(|class| {
                    ["card-xs", "card-sm", "card-md", "card-lg", "card-xl"].contains(class)
                })
                .collect();
            assert!(
                sizes.is_empty(),
                "{path}: as daisyUI draws it: {}",
                card.html()
            );
        }
    }
}

/// `docs/specs/web/components.md`, Facts: Item's and Server's facts are a
/// card holding a `grid` of rows `gap-3` apart, in `text-sm`, each fact a
/// row with its label at the start, in secondary text, and its value at the
/// end (`docs/specs/web/board.md`, Item; `docs/specs/web/operation.md`).
#[test_log::test(tokio::test)]
async fn item_and_servers_facts_are_rows_each_its_label_at_the_start_and_its_value_at_the_end() {
    let pages = with_a_run().await;

    for (path, facts) in [("/items/1", "#item dl"), ("/server", "#server dl")] {
        let html = pages.page(path).await;

        let card = select(&html, facts);
        assert_eq!(card.len(), 1, "{path}");
        assert!(
            has_classes(card[0], &["grid", "gap-3", "text-sm"]),
            "{path}: {}",
            card[0].html()
        );
        let rows: Vec<ElementRef> = card[0].children().filter_map(ElementRef::wrap).collect();
        assert_ne!(rows.len(), 0, "{path}");
        for row in rows {
            assert!(
                has_classes(row, &["flex", "justify-between"]),
                "{path}: {}",
                row.html()
            );
            let parts: Vec<&str> = row
                .children()
                .filter_map(ElementRef::wrap)
                .map(|part| part.value().name())
                .collect();
            assert_eq!(parts, ["dt", "dd"], "{path}: {}", row.html());
            let label = row.children().find_map(ElementRef::wrap).expect("a label");
            assert!(
                has_classes(label, &["shrink-0"]),
                "{path}: {}",
                label.html()
            );
        }
        for value in select(&html, &format!("{facts} dd")) {
            assert!(
                has_classes(value, &["flex", "flex-col", "items-end"])
                    && !has_classes(value, &["text-end"]),
                "{path}: {}",
                value.html()
            );
        }
        assert_in_secondary_text(&html, &format!("{facts} dt"));
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

/// The name of `icon`'s icon (`docs/specs/web/design_system.md`, Icons).
#[track_caller]
fn icon_name(icon: ElementRef) -> String {
    icon.attr("data-icon")
        .unwrap_or_else(|| panic!("not an icon of the set: {}", icon.html()))
        .to_string()
}

/// `docs/specs/web/design_system.md`, Icons: every icon is one of the set,
/// `size-5` before a `card-title`, `size-[1em]` in a `badge` and `size-4`
/// elsewhere. The chart is a plot, not an icon.
#[test_log::test(tokio::test)]
async fn every_icon_is_one_of_the_set_sized_by_where_it_sits() {
    let pages = with_a_run().await;

    let mut sizes = std::collections::BTreeSet::new();
    for path in PATHS {
        let html = pages.page(path).await;
        for icon in select(&html, "svg:not(#chart svg)") {
            icon_name(icon);
            let parent = ElementRef::wrap(icon.parent().unwrap()).unwrap();
            let before_a_title = has_classes(parent, &["card-title"])
                && parent.children().find_map(ElementRef::wrap) == Some(icon);
            let size = if before_a_title {
                "size-5"
            } else if has_classes(parent, &["badge"]) {
                "size-[1em]"
            } else {
                "size-4"
            };
            let sized: Vec<&str> = icon
                .value()
                .classes()
                .filter(|class| class.starts_with("size-"))
                .collect();
            assert_eq!(sized, [size], "{path}: {}", parent.html());
            sizes.insert(size);
        }
    }
    assert_eq!(
        sizes.into_iter().collect::<Vec<_>>(),
        ["size-4", "size-5", "size-[1em]"]
    );
}

/// `docs/specs/web/design_system.md`, Cards and Icons: a card's title has
/// the icon of the area of what the card holds, but the cards the table
/// names.
#[test_log::test(tokio::test)]
async fn each_cards_title_has_the_icon_of_what_it_holds() {
    let pages = with_a_run().await;

    let mut titles = Vec::new();
    for path in ["/items/1", "/runs/1/triage-1", "/benchmark-runs/1"] {
        let html = pages.page(path).await;
        for title in select(&html, ".card-title") {
            let icon = select_in(title, ":scope > svg")[0];
            titles.push((path, text(title), icon_name(icon)));
        }
    }

    assert_eq!(
        titles
            .iter()
            .map(|(path, title, icon)| (*path, title.clone(), icon.as_str()))
            .collect::<Vec<_>>(),
        [
            ("/items/1", "#1: Ship gh".to_string(), "view-columns"),
            ("/items/1", "Runs".to_string(), "command-line"),
            (
                "/runs/1/triage-1",
                "1/triage-1 (it started no session)".to_string(),
                "command-line"
            ),
            ("/benchmark-runs/1", "Benchmark run 1".to_string(), "beaker"),
            ("/benchmark-runs/1", "Pass rate vs".to_string(), "chart-bar"),
            (
                "/benchmark-runs/1",
                "Configurations".to_string(),
                "adjustments-horizontal"
            ),
            (
                "/benchmark-runs/1",
                "Passes by task".to_string(),
                "table-cells"
            ),
        ]
    );
}

/// `docs/specs/web/design_system.md`, Icons: each button the table names has
/// its icon before its label. A "Cancel" that cancels work opens its
/// confirmation, and one that closes a dialog closes it.
#[test_log::test(tokio::test)]
async fn each_button_has_its_icon_before_its_label() {
    let pages = with_a_run().await;

    let mut buttons = std::collections::BTreeSet::new();
    for path in PATHS.into_iter().chain(["/benchmark-runs/1/1/1/1/run"]) {
        let html = pages.page(path).await;
        for button in select(&html, "button") {
            let label = text(button);
            let closes = button.attr("command") == Some("close")
                || button
                    .parent()
                    .and_then(ElementRef::wrap)
                    .is_some_and(|form| form.attr("method") == Some("dialog"));
            let label = match label.as_str() {
                "Cancel" if closes => "Cancel that closes a dialog".to_string(),
                "Cancel" => "Cancel that cancels work".to_string(),
                _ => label,
            };
            let icon = select_in(button, ":scope > svg:first-child")
                .first()
                .map(|icon| icon_name(*icon));
            buttons.insert((label, icon));
        }
    }
    let named = [
        ("Add task", "plus"),
        ("Cancel that cancels work", "stop-circle"),
        ("Remove", "trash"),
        ("Create", "plus"),
        ("Run benchmark", "play"),
        ("Add a configuration", "plus"),
        ("Keep running", "play"),
        ("Edit", "pencil-square"),
        ("Save", "check"),
        ("Cancel that closes a dialog", "x-mark"),
        ("Jump to the latest entry", "arrow-down"),
    ];
    for (label, icon) in named {
        let shown: Vec<Option<&str>> = buttons
            .iter()
            .filter(|(shown, _)| shown == label)
            .map(|(_, shown)| shown.as_deref())
            .collect();
        assert_eq!(shown, [Some(icon)], "{label}");
    }
}

/// `docs/specs/web/design_system.md`, Icons: the button that opens the
/// navigation, a link to GitHub at the end of a name and "Lost `serve`"
/// each have their icon.
#[test_log::test(tokio::test)]
async fn the_shells_parts_have_their_icons() {
    let pages = with_a_run().await;

    let html = pages.page("/runs").await;

    for (selector, icon) in [
        ("label.drawer-button > svg", "bars-3"),
        ("#runs a.github > svg", "arrow-top-right-on-square"),
        ("#lost > svg:first-child", "signal-slash"),
    ] {
        let icons = select(&html, selector);
        assert!(!icons.is_empty(), "{selector}");
        for shown in icons {
            assert_eq!(icon_name(shown), icon, "{selector}");
        }
    }
    assert_eq!(
        select(&html, ".drawer-side nav > ul.menu a > svg")
            .into_iter()
            .map(icon_name)
            .collect::<Vec<_>>(),
        [
            "view-columns",
            "command-line",
            "bolt",
            "beaker",
            "server",
            "users"
        ]
    );
    assert_eq!(
        select(&html, ".account .menu li > * > svg")
            .into_iter()
            .map(icon_name)
            .collect::<Vec<_>>(),
        ["key", "arrow-right-start-on-rectangle"],
        "the account's menu (`docs/specs/web/shell.md`, The account)"
    );
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

/// `docs/specs/web/design_system.md`, Layout, and
/// `docs/specs/web/components.md`, Content panel: the content is a panel in
/// `bg-base-100`, `p-4`, or `p-10` from `lg`, and from `lg` with `mt-2` above
/// it, a `border-base-300` border along its top and start and its top start corner
/// `rounded-ss-4xl` in a squircle. The parts of a screen are `gap-6` apart.
#[test_log::test(tokio::test)]
async fn the_content_is_a_panel_in_base_100_with_its_spacing_and_from_lg_its_border_and_corner() {
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

/// `docs/specs/web/shell.md`: the sidebar is in `bg-base-200`, `w-64`,
/// `px-2` and `pt-10`, the repository `mx-5` with `gap-2` under it and its
/// avatar `size-4` as the links' icons are, and the links a `menu` with
/// daisyUI's padding.
#[test_log::test(tokio::test)]
async fn the_sidebar_is_in_base_200_with_the_specs_spacing_and_daisyuis_menu() {
    let pages = with_a_run().await;

    let html = pages.page("/runs").await;

    let nav = select(&html, ".drawer-side nav")[0];
    assert!(
        has_classes(nav, &["bg-base-200", "w-64", "px-2", "pt-10", "gap-2"]),
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
    assert_eq!(icons.len(), 6, "{}", menu.html());
    for icon in icons {
        assert!(has_classes(icon, &["size-4"]), "{}", icon.html());
    }
}

/// `docs/specs/web/design_system.md`, Status: a status's badge is a daisyUI
/// `badge`, `badge-sm`, with the icon first.
#[test_log::test(tokio::test)]
async fn every_status_badge_is_a_small_badge_with_its_icon_first() {
    let pages = with_a_run().await;

    let mut badges = 0;
    for path in PATHS {
        let html = pages.page(path).await;
        for badge in select(&html, ".badge.tone") {
            badges += 1;
            assert!(
                has_classes(badge, &["badge", "badge-sm"]),
                "{path}: {}",
                badge.html()
            );
            let first = badge
                .children()
                .find_map(ElementRef::wrap)
                .expect("an icon");
            assert_eq!(first.value().name(), "svg", "{path}: {}", badge.html());
        }
    }
    assert_ne!(badges, 0);
}

/// `docs/specs/web/design_system.md`, Tables, Scrolling: a table is in a
/// wrapper with `overflow-x-auto`, and as it scrolls sideways the top of its
/// card and its filters stay in place, outside every wrapper that scrolls.
#[test_log::test(tokio::test)]
async fn every_table_scrolls_sideways_in_its_wrapper_under_its_cards_top_and_filters() {
    let pages = with_a_run().await;

    for path in PATHS {
        let html = pages.page(path).await;
        for table in select(&html, "table.table") {
            let wrapper = ElementRef::wrap(table.parent().unwrap()).unwrap();
            assert!(
                has_classes(wrapper, &["overflow-x-auto"]),
                "{path}: {}",
                wrapper.html()
            );
        }
        for scrolls in select(&html, ".overflow-x-auto") {
            assert_eq!(
                select_in(scrolls, ".table-top, .table-filters").len(),
                0,
                "{path}: {}",
                scrolls.html()
            );
        }
    }
}

/// `docs/specs/web/shell.md`: the name at the top of the content is in
/// `text-xl` and `font-light`, in secondary text.
#[test_log::test(tokio::test)]
async fn the_name_at_the_top_is_large_and_light_in_secondary_text() {
    let pages = with_a_run().await;

    let html = pages.page("/runs").await;

    assert_in_secondary_text(&html, ".content p.repo-name");
    let name = select(&html, ".content p.repo-name")[0];
    assert!(
        has_classes(name, &["text-xl", "font-light"]),
        "{}",
        name.html()
    );
}

/// `docs/specs/web/runs.md`, Run: the verbose switch is a daisyUI `toggle`,
/// and "Jump to the latest entry" is a button with `shadow-md`, fixed at the
/// bottom right, `bottom-6` and `right-6`.
#[test_log::test(tokio::test)]
async fn runs_verbose_switch_is_a_toggle_and_its_jump_a_button_fixed_at_the_bottom_right() {
    let pages = with_a_run().await;

    let html = pages.page("/runs/1/triage-1").await;

    assert_eq!(select(&html, "[role=switch] .toggle").len(), 1);
    let jump = select(&html, "button#latest");
    assert_eq!(jump.len(), 1, "{}", html.html());
    assert_eq!(text(jump[0]), "Jump to the latest entry");
    assert!(
        has_classes(
            jump[0],
            &["btn", "shadow-md", "fixed", "bottom-6", "right-6"]
        ),
        "{}",
        jump[0].html()
    );
}

/// `docs/specs/web/design_system.md`, Buttons and Making: every button, the
/// navigation's and the dialogs' too, is a small `btn-sm`, but a row's,
/// which is a `btn-xs`; and "New" is a `btn-primary`, wherever it is.
#[test_log::test(tokio::test)]
async fn every_button_is_small_and_new_is_primary() {
    let pages = with_a_run().await;

    for path in PATHS {
        let html = pages.page(path).await;
        for button in select(&html, ".btn") {
            assert!(
                has_classes(button, &["btn-sm"]) || has_classes(button, &["btn-ghost", "btn-xs"]),
                "{path}: {}",
                button.html()
            );
            if text(button) == "New" {
                assert!(
                    has_classes(button, &["btn-primary"]),
                    "{path}: {}",
                    button.html()
                );
            }
        }
    }
}

/// `docs/specs/web/benchmarks.md`, Benchmark run, and Task, whose facts are
/// as on Benchmark run: each fact is a `stat`, its label the `stat-title`, as
/// daisyUI draws it, and its value the `stat-value`, in `text-xl` and
/// `font-semibold`.
#[test_log::test(tokio::test)]
async fn benchmark_runs_and_tasks_facts_are_stats_as_daisyui_draws_them() {
    let pages = with_a_run().await;

    for (path, facts) in [
        ("/benchmark-runs/1", "#benchmark-run"),
        ("/tasks/1", "#task"),
    ] {
        let html = pages.page(path).await;

        let stats = select(&html, &format!("{facts} .stat"));
        assert!(!stats.is_empty(), "{}", html.html());
        for stat in stats {
            for (part, classes) in [
                ("stat-title", &["stat-title"][..]),
                ("stat-value", &["font-semibold", "stat-value", "text-xl"]),
            ] {
                let parts = select_in(stat, &format!(".{part}"));
                assert_eq!(parts.len(), 1, "{part}: {}", stat.html());
                assert_eq!(
                    parts[0].value().classes().collect::<Vec<_>>(),
                    classes,
                    "{}",
                    stat.html()
                );
            }
        }
    }
}
