use serde_json::json;

use super::benchmark_runs::{launched, with_benchmark_run};
use super::*;

/// The pages of #1, `Ship gh`, with the implement benchmark `board`, 1,
/// holding the tasks `columns`, 1, captured from `1/implement-1` and of two
/// lines of criteria, and `lanes`, 2, and the empty triage benchmark
/// `triage-basics`, 2.
pub(super) async fn with_benchmarks() -> Pages {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
    let work_dir = pages.path("");
    keep_benchmark(&work_dir, "board", "implement").await;
    keep_benchmark(&work_dir, "triage-basics", "triage").await;
    keep_task(
        &work_dir,
        "board",
        "columns",
        "1/implement-1",
        "Names the columns.\nAnd orders them.\n",
    )
    .await;
    keep_task(
        &work_dir,
        "board",
        "lanes",
        "1/implement-2",
        "Stacks them.\n",
    )
    .await;
    pages
}

/// `docs/specs/benchmarks.md`, Benchmarks; `docs/specs/ui_patterns.md`,
/// Tables.
#[test_log::test(tokio::test)]
async fn the_benchmarks_page_shows_a_row_per_benchmark_viewed_from_its_end_in_a_card_with_its_filters_and_no_name()
 {
    let pages = with_benchmarks().await;
    launched(&pages, "board", &[], 1, "2026-11-17T10:00:00Z").await;
    launched(&pages, "board", &[], 1, "2026-11-17T12:00:00Z").await;

    let html = pages.page("/benchmarks").await;

    assert_eq!(table_cards(&html), [unnamed_card(&[], &["name", "stage"])]);
    let heads: Vec<String> = select(&html, "#benchmarks thead th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(heads, ["Benchmark", "Stage", "Last benchmark run", ""]);
    assert_eq!(
        rows(&html, "#benchmarks"),
        [
            ["board", "implement", "2026-11-17 12:00:00 UTC", "View"],
            ["triage-basics", "triage", "", "View"],
        ]
    );
    let names: Vec<String> = select(&html, "#benchmarks tbody tr > th:first-child")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(names, ["board", "triage-basics"]);
    assert_eq!(select(&html, "#benchmarks tbody th a").len(), 0);
    assert_eq!(
        views(&html, "#benchmarks"),
        [
            (
                "View".to_string(),
                "View benchmark board".to_string(),
                "/benchmarks/1".to_string()
            ),
            (
                "View".into(),
                "View benchmark triage-basics".into(),
                "/benchmarks/2".into()
            ),
        ]
    );
    assert_eq!(
        hrefs(select(&html, "#benchmarks tbody td:nth-child(3) a")),
        ["/benchmark-runs/2"]
    );
    assert_eq!(select(&html, ".table-card #benchmarks table").len(), 1);
    assert_eq!(events_path(&html), Some("/events/benchmarks"));
}

/// `docs/specs/ui_patterns.md`, Screen states, Empty.
#[test_log::test(tokio::test)]
async fn the_benchmarks_page_with_no_benchmarks_says_so() {
    let pages = Pages::of(view(vec![], vec![])).await;

    let html = pages.page("/benchmarks").await;

    assert_eq!(text(select(&html, "#benchmarks p")[0]), "No benchmarks");
}

/// `docs/specs/benchmarks.md`, Benchmarks, Filters;
/// `docs/specs/ui_patterns.md`, Filters.
#[test_log::test(tokio::test)]
async fn the_benchmarks_filters_narrow_the_benchmarks_and_are_carried_in_the_pages_link_and_stream()
{
    let pages = with_benchmarks().await;
    keep_benchmark(&pages.path(""), "triage-boards", "triage").await;

    let by_both = pages.page("/benchmarks?name=BOA&stage=triage").await;
    let unset = pages.page("/benchmarks?name=&stage=").await;

    let names = |html: &Html| -> Vec<String> {
        rows(html, "#benchmarks")
            .into_iter()
            .map(|row| row[0].clone())
            .collect()
    };
    assert_eq!(names(&by_both), ["triage-boards"]);
    assert_eq!(names(&unset), ["board", "triage-basics", "triage-boards"]);
    assert_eq!(
        filters(&by_both),
        [("name", "BOA".to_string()), ("stage", "triage".to_string())]
    );
    assert_eq!(
        events_path(&by_both),
        Some("/events/benchmarks?name=BOA&stage=triage")
    );
    assert_eq!(
        select(&by_both, "form[data-filters]")[0].attr("data-filters"),
        Some("/events/benchmarks")
    );
    let stages: Vec<String> = select(&unset, "form[data-filters] select[name=stage] option")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(stages, ["All stages", "triage", "design", "implement"]);
}

/// `docs/specs/ui_patterns.md`, Tables, Pages.
#[test_log::test(tokio::test)]
async fn the_benchmarks_page_shows_its_benchmarks_25_at_a_time_its_link_carrying_the_page_and_the_filters()
 {
    let pages = with_benchmarks().await;
    for n in 1..=26 {
        keep_benchmark(&pages.path(""), &format!("bench-{n:02}"), "implement").await;
    }

    let first = pages.page("/benchmarks?stage=implement").await;
    let second = pages.page("/benchmarks?stage=implement&page=2").await;

    let names = |html: &Html| -> Vec<String> {
        rows(html, "#benchmarks")
            .into_iter()
            .map(|row| row[0].clone())
            .collect()
    };
    assert_eq!(names(&first).len(), 25);
    assert_eq!(names(&second), ["bench-26", "board"]);
    assert_eq!(
        pages_nav(&first, "#benchmarks"),
        Some(PagesNav {
            label: "Pages of benchmarks".into(),
            showing: "Showing 1–25 of 27 benchmarks".into(),
            items: vec![
                no_link("Previous"),
                page_link(
                    "1*",
                    "/benchmarks?stage=implement",
                    "/events/benchmarks?stage=implement"
                ),
                page_link(
                    "2",
                    "/benchmarks?stage=implement&page=2",
                    "/events/benchmarks?stage=implement&page=2"
                ),
                page_link(
                    "Next",
                    "/benchmarks?stage=implement&page=2",
                    "/events/benchmarks?stage=implement&page=2"
                ),
            ],
        })
    );
    assert_eq!(
        events_path(&second),
        Some("/events/benchmarks?stage=implement&page=2")
    );
    let mut events = stream(&pages, "/events/benchmarks?stage=implement&page=2").await;
    let sent = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        names(region_of(&regions(&sent), "benchmarks")),
        ["bench-26", "board"]
    );
}

/// `docs/specs/ui_patterns.md`, Filters: a filter that searches.
#[test_log::test(tokio::test)]
async fn the_benchmarks_name_filter_is_a_search_with_a_magnifier_at_its_start_taking_the_spare_width()
 {
    let pages = with_benchmarks().await;

    let html = pages.page("/benchmarks?name=boa").await;

    let search = select(&html, ".table-filters input[name=name]")[0];
    assert_eq!(search.attr("type"), Some("search"));
    assert_eq!(search.attr("value"), Some("boa"));
    assert_eq!(search.attr("aria-label"), Some("Name"));
    let field = ElementRef::wrap(search.parent().unwrap()).unwrap();
    assert_eq!(field.value().name(), "label");
    assert!(
        has_classes(field, &["input", "input-sm", "grow"]),
        "{}",
        field.html()
    );
    let first = field.children().find_map(ElementRef::wrap).unwrap();
    assert_eq!(first.value().name(), "svg", "{}", field.html());
    assert_eq!(first.attr("aria-hidden"), Some("true"));
    let row = select(&html, "form.table-filters")[0];
    assert!(has_classes(row, &["flex"]), "{}", row.html());
}

#[test_log::test(tokio::test)]
async fn the_benchmarks_pages_stream_sends_the_benchmarks_its_filters_keep() {
    let pages = with_benchmarks().await;
    let mut events = stream(&pages, "/events/benchmarks?stage=triage").await;

    let event = events.recv().await.unwrap();

    let (_, benchmarks) = regions(&event)
        .into_iter()
        .find(|(id, _)| id == "benchmarks")
        .expect("the benchmarks");
    assert_eq!(
        rows(&benchmarks, "#benchmarks"),
        [["triage-basics", "triage", "", "View"]]
    );
}

/// `docs/specs/ui_patterns.md`, Pages: a screen is at the address its section
/// gives it, and at no other.
#[test_log::test(tokio::test)]
async fn suites_and_experiments_are_at_no_address() {
    let pages = with_benchmarks().await;
    launched(&pages, "board", &[], 1, "2026-11-17T10:00:00Z").await;
    let nowhere = pages.get("/nowhere").await;

    for path in [
        "/suites",
        "/suites/board",
        "/experiments/1",
        "/events/suites",
        "/events/experiments/1",
    ] {
        assert_eq!(pages.get(path).await, nowhere, "{path}");
    }
}

#[test_log::test(tokio::test)]
async fn a_stage_to_filter_on_that_is_not_a_benchmarks_says_so_in_place_of_the_benchmarks() {
    let pages = with_benchmarks().await;

    let (status, body) = pages.get("/benchmarks?stage=rework").await;

    assert_eq!(status, 400);
    let html = Html::parse_document(&body);
    assert_eq!(
        text(select(&html, "#benchmarks p")[0]),
        "\"rework\" is not a stage of a benchmark: one of triage, design, implement"
    );
}

/// `docs/specs/benchmarks.md`, New benchmark; `docs/specs/ui_patterns.md`,
/// Dialogs and Making.
#[test_log::test(tokio::test)]
async fn new_opens_the_new_benchmark_dialog_outside_the_benchmarks_with_name_and_stage_on_triage() {
    let pages = with_benchmarks().await;

    let html = pages.page("/benchmarks").await;

    let new = select(&html, "#buttons button[command=show-modal]");
    assert_eq!(new.len(), 1);
    assert_eq!(text(new[0]), "New");
    assert_eq!(new[0].attr("type"), Some("button"));
    let id = new[0].attr("commandfor").unwrap();
    let dialog = select(&html, &format!("dialog[id='{id}']"));
    assert_eq!(dialog.len(), 1, "{}", html.html());
    let dialog = dialog[0];
    assert!(has_classes(dialog, &["modal"]), "{}", dialog.html());
    assert_eq!(select_in(dialog, ".modal-backdrop").len(), 0);
    assert_eq!(
        select(&html, "#benchmarks dialog, #buttons dialog").len(),
        0
    );
    let legend = select_in(dialog, "form fieldset > legend.fieldset-legend");
    assert_eq!(text(legend[0]), "New benchmark");
    assert_eq!(
        dialog.attr("aria-labelledby"),
        legend[0].attr("id"),
        "{}",
        dialog.html()
    );
    let form = select_in(dialog, "form[hx-post]")[0];
    assert_eq!(form.attr("hx-post"), Some("/benchmarks"));
    assert_eq!(form.attr("action"), Some("/benchmarks"));
    assert_eq!(form.attr("hx-target"), Some("this"));
    assert!(form.attr("data-new").is_some(), "{}", form.html());
    let labels: Vec<String> = select_in(form, "fieldset label")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(labels, ["Name", "Stage"]);
    for label in select_in(form, "fieldset label") {
        let field = label.attr("for").unwrap();
        assert_eq!(
            select_in(form, &format!("[id='{field}']")).len(),
            1,
            "{field}"
        );
    }
    let stages: Vec<&str> = select_in(form, "select[name=stage] option")
        .into_iter()
        .filter_map(|option| option.attr("value"))
        .collect();
    assert_eq!(stages, ["triage", "design", "implement"]);
    let fragment = Html::parse_fragment(&form.inner_html());
    assert_eq!(
        making_shown(&fragment),
        MakingShown {
            values: vec![("name", String::new()), ("stage", "triage".into())],
            refused: vec![],
            whole: None,
        }
    );
    let name = select(&fragment, "input[name=name]")[0];
    assert_eq!(name.attr("required"), Some(""));
    assert_eq!(name.attr("pattern"), Some(r"[\s\S]*\S[\s\S]*"));
    let stage = select(&fragment, "select[name=stage]")[0];
    assert_eq!(stage.attr("required"), Some(""));
    assert_eq!(stage.attr("pattern"), None);
    let hints: Vec<String> = select(&fragment, ".validator-hint")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(hints, ["Required.", "Required."]);
    assert_eq!(select(&fragment, ".optional").len(), 0);
    let buttons = select_in(form, ".modal-action button");
    assert_eq!(
        buttons
            .iter()
            .map(|button| text(*button))
            .collect::<Vec<_>>(),
        ["Cancel", "Create"]
    );
    assert_eq!(buttons[0].attr("command"), Some("close"));
    assert_eq!(buttons[0].attr("commandfor"), Some(id));
    assert!(has_classes(buttons[1], &["btn-primary"]));
    assert_eq!(buttons[1].attr("type"), Some("submit"));
}

/// `docs/specs/ui_patterns.md`, Making.
#[test_log::test(tokio::test)]
async fn making_a_benchmark_answers_the_form_as_it_starts_closing_the_dialog_and_the_stream_then_sends_it()
 {
    let pages = Pages::of(view(vec![], vec![])).await;
    let mut events = stream(&pages, "/events/benchmarks").await;
    events.recv().await.unwrap();

    let (status, headers, answer) = post_answer(
        &pages,
        "/benchmarks",
        &[SAME_ORIGIN],
        &[("name", "board"), ("stage", "design")],
    )
    .await;

    assert_eq!(status, 200, "{}", answer.html());
    assert_eq!(headers.get("hx-trigger").unwrap(), "made");
    assert_eq!(
        making_shown(&answer),
        MakingShown {
            values: vec![("name", String::new()), ("stage", "triage".into())],
            refused: vec![],
            whole: None,
        }
    );
    assert_eq!(text(select(&answer, "legend")[0]), "New benchmark");
    let benchmarks = loop {
        let event = events.recv().await.unwrap();
        if let Some((_, benchmarks)) = regions(&event)
            .into_iter()
            .find(|(id, _)| id == "benchmarks")
        {
            break benchmarks;
        }
    };
    assert_eq!(
        rows(&benchmarks, "#benchmarks"),
        [["board", "design", "", "View"]]
    );
}

/// `docs/specs/benchmarks.md`, New benchmark, Refusals;
/// `docs/specs/ui_patterns.md`, Making, Required, A field and A refusal:
/// each refusal of a field is under it, and one of no field above the
/// buttons.
#[test_log::test(tokio::test)]
async fn a_refused_benchmark_answers_the_form_as_sent_with_each_reason_in_its_place() {
    const NOT_A_STAGE: &str =
        "\"rework\" is not a stage of a benchmark: one of triage, design, implement";
    let pages = with_benchmarks().await;
    let cases = [
        ("", "design", "design", vec![("name", "Required.")]),
        (" \t ", "design", "design", vec![("name", "Required.")]),
        (
            "\u{200b}",
            "implement",
            "implement",
            vec![("name", "Required.")],
        ),
        (
            "board",
            "design",
            "design",
            vec![("name", "A benchmark named board already exists.")],
        ),
        ("fresh", "rework", "triage", vec![("stage", NOT_A_STAGE)]),
        (
            "",
            "rework",
            "triage",
            vec![("name", "Required."), ("stage", NOT_A_STAGE)],
        ),
    ];

    for (name, stage, kept, refused) in cases {
        let (status, headers, form) = post_answer(
            &pages,
            "/benchmarks",
            &[SAME_ORIGIN],
            &[("name", name), ("stage", stage)],
        )
        .await;

        assert_eq!(status, 422, "{name:?} {stage:?}");
        assert!(headers.get("hx-trigger").is_none());
        let shown = making_shown(&form);
        assert_eq!(
            shown.refused,
            refused
                .iter()
                .map(|(field, reason)| (*field, (*reason).to_string()))
                .collect::<Vec<_>>(),
            "{name:?} {stage:?}"
        );
        assert_eq!(shown.whole, None, "{name:?} {stage:?}");
        assert_eq!(
            shown.values,
            [("name", name.to_string()), ("stage", kept.to_string())]
        );
    }
    let (cross_site, _, cross_site_form) = post_answer(
        &pages,
        "/benchmarks",
        &[CROSS_SITE],
        &[("name", "fresh"), ("stage", "design")],
    )
    .await;
    assert_eq!(cross_site, 403);
    let shown = making_shown(&cross_site_form);
    assert_eq!(
        shown.values,
        [("name", "fresh".into()), ("stage", "design".into())]
    );
    assert_eq!(shown.refused, []);
    assert!(shown.whole.is_some(), "{}", cross_site_form.html());
    let html = pages.page("/benchmarks").await;
    assert_eq!(
        rows(&html, "#benchmarks"),
        [
            ["board", "implement", "", "View"],
            ["triage-basics", "triage", "", "View"]
        ]
    );
}

/// `docs/specs/ui_patterns.md`, Tables, Pages: each of Benchmark's tables
/// has its own page, which its link carries beside the other's.
#[test_log::test(tokio::test)]
async fn the_benchmark_pages_tables_each_show_their_rows_25_at_a_time_on_a_page_of_their_own() {
    let pages = with_benchmarks().await;
    for n in 1..=26 {
        keep_task(
            &pages.path(""),
            "board",
            &format!("task-{n:02}"),
            "1/implement-1",
            "Does it.\n",
        )
        .await;
    }
    for n in 1..=26 {
        launched(
            &pages,
            "board",
            &["short"],
            1,
            &format!("2026-10-{n:02}T12:00:00Z"),
        )
        .await;
    }

    let html = pages.page("/benchmarks/1?runs-page=2&tasks-page=2").await;

    let tasks: Vec<String> = select(&html, "#tasks tbody tr > th > span")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(tasks, ["task-24", "task-25", "task-26"]);
    assert_eq!(
        pages_nav(&html, "#tasks"),
        Some(PagesNav {
            label: "Pages of tasks".into(),
            showing: "Showing 26–28 of 28 tasks".into(),
            items: vec![
                page_link(
                    "Previous",
                    "/benchmarks/1/tasks?runs-page=2",
                    "/events/benchmarks/1?runs-page=2"
                ),
                page_link(
                    "1",
                    "/benchmarks/1/tasks?runs-page=2",
                    "/events/benchmarks/1?runs-page=2"
                ),
                page_link(
                    "2*",
                    "/benchmarks/1/tasks?runs-page=2&tasks-page=2",
                    "/events/benchmarks/1?runs-page=2&tasks-page=2"
                ),
                no_link("Next"),
            ],
        })
    );
    assert_eq!(rows(&html, "#benchmark-runs").len(), 1);
    assert_eq!(
        pages_nav(&html, "#benchmark-runs").unwrap().showing,
        "Showing 26–26 of 26 benchmark runs"
    );
    assert_eq!(
        pages_nav(&html, "#benchmark-runs").unwrap().items[0],
        page_link(
            "Previous",
            "/benchmarks/1/runs?tasks-page=2",
            "/events/benchmarks/1?tasks-page=2"
        )
    );
    assert_eq!(
        events_path(&html),
        Some("/events/benchmarks/1?runs-page=2&tasks-page=2")
    );
    let mut events = stream(&pages, "/events/benchmarks/1?tasks-page=2").await;
    let sent = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let sent = regions(&sent);
    assert_eq!(
        rows(region_of(&sent, "benchmark-runs"), "#benchmark-runs").len(),
        25
    );
    assert_eq!(
        pages_nav(region_of(&sent, "tasks"), "#tasks")
            .unwrap()
            .showing,
        "Showing 26–28 of 28 tasks"
    );
}

/// A tab of the Benchmark page: its label, its href, whether it is open, and
/// the ids of the regions its panel holds.
type Tab = (String, String, bool, Vec<String>);

/// The tabs of the Benchmark page `html`, in order. Each is a link with
/// `role="tab"` in a daisyUI `tabs tabs-border`, the open one `tab-active`
/// and `aria-selected`, and controls its panel, which shows only while it is
/// open.
fn tabs(html: &Html) -> Vec<Tab> {
    let lists = select(html, "main [role=tablist]");
    assert_eq!(lists.len(), 1, "{}", html.html());
    assert!(has_classes(lists[0], &["tabs", "tabs-border"]));
    select_in(lists[0], ":scope > a[role=tab]")
        .into_iter()
        .map(|tab| {
            assert!(has_classes(tab, &["tab"]), "{}", tab.html());
            let open = has_classes(tab, &["tab-active"]);
            assert_eq!(
                tab.attr("aria-selected"),
                Some(if open { "true" } else { "false" }),
                "{}",
                tab.html()
            );
            let controls = tab.attr("aria-controls").expect("the tab's panel");
            let panels = select(html, &format!("main [role=tabpanel]#{controls}"));
            assert_eq!(panels.len(), 1, "{}", html.html());
            assert_eq!(
                panels[0].attr("hidden").is_none(),
                open,
                "{}",
                panels[0].html()
            );
            (
                text(tab),
                tab.attr("href").unwrap_or_default().to_string(),
                open,
                select_in(panels[0], "[id]:is(#benchmark-runs, #tasks)")
                    .into_iter()
                    .filter_map(|region| region.attr("id").map(str::to_string))
                    .collect(),
            )
        })
        .collect()
}

/// Of the tabs of the Benchmark page `html`, each one's label and whether it
/// is open.
pub(super) fn open_tabs(html: &Html) -> Vec<(String, bool)> {
    tabs(html)
        .into_iter()
        .map(|(label, _, open, _)| (label, open))
        .collect()
}

/// `docs/specs/benchmarks.md`, Benchmark, Heading and Tabs;
/// `docs/specs/ui_patterns.md`, Tabs, Addresses.
#[test_log::test(tokio::test)]
async fn the_benchmark_page_heads_with_its_name_and_stage_and_opens_on_tasks_while_it_has_no_benchmark_runs()
 {
    let pages = with_benchmarks().await;

    let html = pages.page("/benchmarks/1").await;

    assert_eq!(text(select(&html, "#heading h1")[0]), "board");
    assert_eq!(text(select(&html, "#heading .line")[0]), "implement");
    assert_eq!(
        tabs(&html),
        [
            (
                "Runs".to_string(),
                "/benchmarks/1/runs".to_string(),
                false,
                vec!["benchmark-runs".to_string()]
            ),
            (
                "Tasks".to_string(),
                "/benchmarks/1/tasks".to_string(),
                true,
                vec!["tasks".to_string()]
            ),
        ]
    );
    assert_eq!(select(&html, "#benchmark-runs [role=tablist], #tasks [role=tablist], #benchmark-runs [role=tab], #tasks [role=tab]").len(), 0);
}

/// `docs/specs/benchmarks.md`, Benchmark, Tabs.
#[test_log::test(tokio::test)]
async fn the_benchmark_page_opens_on_benchmark_runs_once_it_has_one() {
    let pages = with_benchmarks().await;
    launched(&pages, "board", &[], 1, "2026-11-17T10:00:00Z").await;

    let html = pages.page("/benchmarks/1").await;

    assert_eq!(
        open_tabs(&html),
        [("Runs".to_string(), true), ("Tasks".to_string(), false)]
    );
}

/// `docs/specs/ui_patterns.md`, Tabs, Addresses and Links: each tab is at its
/// own address, whatever the page opens on without one.
#[test_log::test(tokio::test)]
async fn each_tab_of_the_benchmark_page_opens_at_its_own_address() {
    let launched_on = with_benchmarks().await;
    launched(&launched_on, "board", &[], 1, "2026-11-17T10:00:00Z").await;
    let empty = Pages::of(view(vec![], vec![])).await;
    keep_benchmark(&empty.path(""), "fresh", "implement").await;

    for (web, path, open) in [
        (&launched_on, "/benchmarks/1/tasks", [false, true]),
        (&launched_on, "/benchmarks/1/runs", [true, false]),
        (&empty, "/benchmarks/1/runs", [true, false]),
        (&empty, "/benchmarks/1/tasks", [false, true]),
    ] {
        let html = web.page(path).await;

        assert_eq!(
            open_tabs(&html),
            [
                ("Runs".to_string(), open[0]),
                ("Tasks".to_string(), open[1])
            ],
            "{path}"
        );
    }
}

/// `docs/specs/ui_patterns.md`, Tabs, Addresses: a segment that names no tab is
/// an unknown page.
#[test_log::test(tokio::test)]
async fn a_benchmark_tab_that_does_not_exist_is_no_page() {
    let pages = with_benchmarks().await;

    for path in [
        "/benchmarks/1/nope",
        "/benchmarks/1/Tasks",
        "/benchmarks/1/launch",
        "/benchmarks/1/remove-task",
        "/benchmarks/9/nope",
    ] {
        let (status, body) = pages.get(path).await;

        assert_eq!((status, body.as_str()), (404, "no such page"), "{path}");
    }
    let (status, body) = pages.get("/benchmarks/9/tasks").await;
    assert_eq!(status, 404);
    let html = Html::parse_document(&body);
    assert_eq!(text(select(&html, "main #tasks")[0]), "No such benchmark 9");
}

/// `docs/specs/ui_patterns.md`, Tabs, Links: a tab's link carries the
/// page's pages, and its tables' page links go to their own tab's address.
#[test_log::test(tokio::test)]
async fn the_benchmark_pages_tab_links_carry_its_pages_and_open_on_the_page_asked_for() {
    let pages = with_benchmarks().await;
    for n in 1..=26 {
        keep_task(
            &pages.path(""),
            "board",
            &format!("task-{n:02}"),
            "1/implement-1",
            "Does it.\n",
        )
        .await;
    }
    launched(&pages, "board", &[], 1, "2026-11-17T10:00:00Z").await;

    let html = pages.page("/benchmarks/1/tasks?tasks-page=2").await;

    let hrefs: Vec<(String, String, bool)> = tabs(&html)
        .into_iter()
        .map(|(label, href, open, _)| (label, href, open))
        .collect();
    assert_eq!(
        hrefs,
        [
            (
                "Runs".to_string(),
                "/benchmarks/1/runs?tasks-page=2".to_string(),
                false
            ),
            (
                "Tasks".to_string(),
                "/benchmarks/1/tasks?tasks-page=2".to_string(),
                true
            ),
        ]
    );
    assert_eq!(
        pages_nav(&html, "#tasks").unwrap().showing,
        "Showing 26–28 of 28 tasks"
    );
    assert_eq!(
        pages_nav(&html, "#tasks").unwrap().items[0],
        page_link("Previous", "/benchmarks/1/tasks", "/events/benchmarks/1")
    );
    assert_eq!(
        events_path(&html),
        Some("/events/benchmarks/1?tasks-page=2")
    );
}

#[test_log::test(tokio::test)]
async fn every_write_from_another_site_is_refused_and_writes_nothing() {
    let pages = with_benchmarks().await;
    let writes: [(&str, &[(&str, &str)]); 4] = [
        ("/benchmarks", &[("name", "fresh"), ("stage", "design")]),
        ("/benchmarks/1/edit", &[("name", "fresh")]),
        (
            "/benchmarks/1",
            &[
                ("run", "1/implement-1"),
                ("name", "more"),
                ("criteria", "Ships.\n"),
            ],
        ),
        ("/benchmarks/1/remove-task", &[("task", "1")]),
    ];

    for (path, fields) in writes {
        let (status, _) = post(&pages, path, &[CROSS_SITE], fields).await;
        assert_eq!(status, 403, "{path}");
    }

    let html = pages.page("/benchmarks").await;
    assert_eq!(
        rows(&html, "#benchmarks"),
        [
            ["board", "implement", "", "View"],
            ["triage-basics", "triage", "", "View"]
        ]
    );
    assert_eq!(tasks(&pages), ["1", "2"]);
}

/// `docs/specs/ui_patterns.md`, Screen states, Nothing there.
#[test_log::test(tokio::test)]
async fn the_page_of_no_benchmark_says_there_is_none() {
    let pages = with_benchmarks().await;

    let (no_benchmark, no_benchmark_body) = pages.get("/benchmarks/9").await;
    let (no_name, no_name_body) = pages.get("/benchmarks/Not%20a%20name").await;

    for (status, body, shown) in [
        (no_benchmark, no_benchmark_body, "No such benchmark 9"),
        (no_name, no_name_body, "No such benchmark Not a name"),
    ] {
        assert_eq!(status, 404);
        let html = Html::parse_document(&body);
        assert_eq!(text(select(&html, "main #tasks")[0]), shown);
    }
}

#[test_log::test(tokio::test)]
async fn a_benchmark_that_does_not_exist_answers_404() {
    let pages = with_benchmarks().await;

    let (page, _) = pages.get("/benchmarks/9").await;
    let (capture, _) = post(
        &pages,
        "/benchmarks/9",
        &[SAME_ORIGIN],
        &[
            ("run", "1/implement-1"),
            ("name", "more"),
            ("criteria", "Ships.\n"),
        ],
    )
    .await;
    let (removal, _) = post(
        &pages,
        "/benchmarks/9/remove-task",
        &[SAME_ORIGIN],
        &[("task", "1")],
    )
    .await;

    assert_eq!([page, capture, removal], [404, 404, 404]);
    assert_eq!(tasks(&pages), ["1", "2"]);
}

/// `docs/specs/benchmarks.md`, Benchmark, Tasks; `docs/specs/ui_patterns.md`,
/// Tables.
#[test_log::test(tokio::test)]
async fn the_benchmark_page_shows_each_task_in_capture_order_with_the_first_line_of_its_criteria_and_its_run()
 {
    let pages = with_benchmarks().await;
    keep_task(
        &pages.path(""),
        "board",
        "aisles",
        "1/implement-1",
        "\n  Walks the aisles.\nSlowly.\n",
    )
    .await;

    let html = pages.page("/benchmarks/1").await;

    assert_eq!(events_path(&html), Some("/events/benchmarks/1"));
    let heads: Vec<String> = select(&html, "#tasks thead th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(heads, ["Task", "Captured from", ""]);
    assert_eq!(
        rows(&html, "#tasks"),
        [
            ["columns Names the columns.", "1/implement-1", "View"],
            ["lanes Stacks them.", "1/implement-2", "View"],
            ["aisles Walks the aisles.", "1/implement-1", "View"],
        ]
    );
    assert_eq!(
        hrefs(select(&html, "#tasks tbody td:nth-child(2) a")),
        [
            "/runs/1/implement-1",
            "/runs/1/implement-2",
            "/runs/1/implement-1"
        ]
    );
}

/// `docs/specs/benchmarks.md`, Benchmark, Tasks, and Task: a task's row opens
/// Task by "View" at its end, and opens nothing in place;
/// `docs/specs/ui_patterns.md`, Tables, A row's screen.
#[test_log::test(tokio::test)]
async fn a_tasks_row_opens_its_task_by_view_and_nothing_in_place() {
    let pages = with_benchmarks().await;

    let html = pages.page("/benchmarks/1").await;

    let views: Vec<(Option<&str>, Option<&str>)> = select(&html, "#tasks tbody td a.row-view")
        .into_iter()
        .map(|view| {
            assert!(
                has_classes(view, &["btn", "btn-ghost", "btn-xs"]),
                "{}",
                view.html()
            );
            (view.attr("href"), view.attr("aria-label"))
        })
        .collect();
    assert_eq!(
        views,
        [
            (Some("/tasks/1"), Some("View task columns")),
            (Some("/tasks/2"), Some("View task lanes")),
        ]
    );
    assert_eq!(select(&html, "#tasks tbody tr").len(), 2);
    assert_eq!(select(&html, "#tasks button").len(), 0, "{}", html.html());
    assert_eq!(confirmations(&html), []);
}

/// `docs/specs/ui_patterns.md`, Screen states, Empty;
/// `docs/specs/benchmarks.md`, New task, Fields.
#[test_log::test(tokio::test)]
async fn a_benchmark_with_no_tasks_benchmark_runs_or_runs_of_its_stage_says_so() {
    let pages = with_benchmarks().await;

    let html = pages.page("/benchmarks/2").await;

    assert_eq!(text(select(&html, "#tasks p")[0]), "No tasks");
    assert_eq!(
        text(select(&html, "#benchmark-runs p")[0]),
        "No benchmark runs"
    );
    let run = select(&html, "dialog#new-task select[name=run]");
    assert!(run[0].attr("disabled").is_some(), "{}", run[0].html());
    assert_eq!(text(run[0]), "No runs");
    let add = select(&html, "dialog#new-task button[type=submit]");
    assert_eq!(text(add[0]), "Add task");
    assert!(add[0].attr("disabled").is_some(), "{}", add[0].html());
}

#[test_log::test(tokio::test)]
async fn removing_a_task_the_benchmark_does_not_hold_is_refused() {
    let pages = with_benchmarks().await;

    let (status, answer) = post(
        &pages,
        "/benchmarks/2/remove-task",
        &[SAME_ORIGIN],
        &[("task", "1")],
    )
    .await;

    assert_eq!(status, 404);
    assert_eq!(said(&answer), "Not removed: the benchmark 2 has no task 1");
    assert_eq!(tasks(&pages), ["1", "2"]);
}

/// Keeps #1's run `n` of `stage`, started at `started` and ended half an
/// hour later, and, given the default branch's `head`, the whole input it
/// recorded.
async fn record_run(pages: &Pages, stage: &str, n: u32, started: &str, head: Option<&str>) {
    pages
        .record(&json!({
            "item": 1,
            "stage": stage,
            "n": n,
            "started_at": started,
            "end": {"at": "2026-10-13T11:30:00Z", "ending": "done"},
            "sessions": [],
        }))
        .await;
    if let Some(head) = head {
        pages
            .input(
                &format!("1/{stage}-{n}"),
                &json!({
                    "api_version": linewright_core::store::API_VERSION,
                    "start": started,
                    "items": [{"number": 1, "resources": {"issues/1": {"number": 1, "title": "Ship gh"}}}],
                    "heads": {"main": head},
                    "properties": {"resources": {"account": {"login": "factory", "id": 7}}},
                }),
            )
            .await;
    }
}

/// The default branch's head, committed in `pages`' store.
async fn committed(pages: &Pages) -> String {
    pages
        .store
        .commit("maintainer", "main", &[("README.md", "Gadgets\n")], "Start")
        .await
        .unwrap_or_else(|e| panic!("committing on main: {e:#}"))
}

/// [`with_benchmarks`]'s pages, with #1's runs: its design run 1, and its
/// implement runs 1 and 2, the design run and the last implement run
/// having recorded their whole input.
async fn with_runs() -> Pages {
    let pages = Box::pin(with_benchmarks()).await;
    let head = committed(&pages).await;
    record_run(&pages, "design", 1, "2026-10-13T09:00:00Z", Some(&head)).await;
    record_run(&pages, "implement", 1, "2026-10-13T10:00:00Z", None).await;
    record_run(&pages, "implement", 2, "2026-10-13T11:00:00Z", Some(&head)).await;
    pages
}

/// A "New" dialog's form as a reader meets it: the name and value of each
/// field, and the refusal above its buttons, if any.
type FormShown<'a> = (Vec<(&'a str, String)>, Option<String>);

/// The New task dialog's form as a reader meets it: the value of each field,
/// a select's its chosen option's, and the refusal above its buttons, if any.
fn new_task_form(form: &Html) -> FormShown<'_> {
    let shown = making_shown(form);
    (shown.values, shown.whole)
}

/// `docs/specs/benchmarks.md`, New task; `docs/specs/ui_patterns.md`,
/// Dialogs and Making.
#[test_log::test(tokio::test)]
async fn new_opens_the_new_task_dialog_offering_the_runs_of_the_benchmarks_stage_that_can_be_captured()
 {
    let pages = with_runs().await;

    let html = pages.page("/benchmarks/1").await;

    let buttons: Vec<&str> = select(&html, ".table-top button[command=show-modal]")
        .into_iter()
        .filter_map(|button| button.attr("commandfor"))
        .collect();
    assert_eq!(buttons, ["new-benchmark-run", "new-task"]);
    let dialog = select(&html, "dialog#new-task")[0];
    assert!(has_classes(dialog, &["modal"]));
    assert_eq!(select_in(dialog, ".modal-backdrop").len(), 0);
    assert_eq!(select(&html, "#tasks dialog#new-task").len(), 0);
    let legend = select_in(dialog, "form fieldset > legend.fieldset-legend");
    assert_eq!(text(legend[0]), "New task in board");
    assert_eq!(dialog.attr("aria-labelledby"), legend[0].attr("id"));
    let form = select_in(dialog, "form[hx-post]")[0];
    assert_eq!(form.attr("hx-post"), Some("/benchmarks/1"));
    assert_eq!(form.attr("hx-target"), Some("this"));
    let labels: Vec<String> = select_in(form, "fieldset label")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(labels, ["Run", "Name", "Criteria"]);
    for label in select_in(form, "fieldset label") {
        let field = label.attr("for").unwrap();
        assert_eq!(
            select_in(form, &format!("[id='{field}']")).len(),
            1,
            "{field}"
        );
    }
    let runs: Vec<Option<&str>> = select_in(form, "select[name=run] option")
        .into_iter()
        .map(|option| option.attr("value"))
        .collect();
    assert_eq!(runs, [Some("1/implement-2")]);
    let fragment = Html::parse_fragment(&form.inner_html());
    assert_eq!(
        new_task_form(&fragment),
        (
            vec![
                ("run", String::new()),
                ("name", String::new()),
                ("criteria", String::new())
            ],
            None
        )
    );
    let required: Vec<(&str, Option<&str>)> = select(&fragment, "[name][required]")
        .into_iter()
        .map(|field| (field.attr("name").unwrap(), field.attr("pattern")))
        .collect();
    assert_eq!(
        required,
        [
            ("run", None),
            ("name", Some(r"[\s\S]*\S[\s\S]*")),
            ("criteria", None)
        ]
    );
    let hints: Vec<String> = select(&fragment, ".validator-hint")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(hints, ["Required."; 3]);
    assert_eq!(select(&fragment, ".optional").len(), 0);
    let actions = select_in(form, ".modal-action button");
    let action_labels: Vec<String> = actions.iter().map(|button| text(*button)).collect();
    assert_eq!(action_labels, ["Cancel", "Add task"]);
    assert!(has_classes(actions[1], &["btn-primary"]));
    assert!(actions[1].attr("disabled").is_none());
}

/// `docs/specs/benchmarks.md`, New task; `docs/specs/ui_patterns.md`,
/// Making.
#[test_log::test(tokio::test)]
async fn adding_a_task_answers_the_form_as_it_starts_closing_the_dialog_and_the_task_shows_last() {
    let pages = with_runs().await;

    let (status, headers, answer) = post_answer(
        &pages,
        "/benchmarks/1",
        &[SAME_ORIGIN],
        &[
            ("run", "1/implement-2"),
            ("name", "gadgets"),
            ("criteria", "Ships gh.\n"),
        ],
    )
    .await;

    assert_eq!(status, 200, "{}", answer.html());
    assert_eq!(headers.get("hx-trigger").unwrap(), "made");
    assert_eq!(
        new_task_form(&answer),
        (
            vec![
                ("run", String::new()),
                ("name", String::new()),
                ("criteria", String::new())
            ],
            None
        )
    );
    assert_eq!(text(select(&answer, "legend")[0]), "New task in board");
    assert_eq!(tasks(&pages), ["1", "2", "3"]);
    let html = pages.page("/benchmarks/1").await;
    let names: Vec<String> = select(&html, "#tasks tbody th > span:first-child")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(names, ["columns", "lanes", "gadgets"]);
}

/// A New task sent, and how it is refused.
struct Case {
    run: &'static str,
    name: &'static str,
    criteria: &'static str,
    status: u16,
    /// Each field refused, by name, with the reason under it.
    refused: Vec<(&'static str, &'static str)>,
    /// The reason above the buttons, if any.
    whole: Option<&'static str>,
}

/// `docs/specs/benchmarks.md`, Capturing, Refusals on a screen;
/// `docs/specs/ui_patterns.md`, Making, Required and A refusal: each
/// refusal of a field is under it, every empty required field at once, and a
/// refusal of the run above the buttons.
#[test_log::test(tokio::test)]
async fn a_refused_task_answers_the_form_as_sent_with_each_reason_in_its_place() {
    let pages = with_runs().await;
    let sent = |run, name, criteria| Case {
        run,
        name,
        criteria,
        status: 422,
        refused: vec![],
        whole: None,
    };
    let cases = [
        Case {
            refused: vec![("name", "A task named columns already exists in board.")],
            ..sent("1/implement-2", "columns", "Ships gh.\n")
        },
        Case {
            refused: vec![
                ("run", "Required."),
                ("name", "Required."),
                ("criteria", "Required."),
            ],
            ..sent("", " ", "")
        },
        Case {
            whole: Some("Run 1/design-1 is of design, and board holds tasks of implement."),
            ..sent("1/design-1", "gadgets", "Ships gh.\n")
        },
        Case {
            status: 404,
            whole: Some("No such run 1/implement-9."),
            ..sent("1/implement-9", "gadgets", "Ships gh.\n")
        },
    ];

    for case in cases {
        let Case {
            run,
            name,
            criteria,
            ..
        } = case;
        let (status, headers, form) = post_answer(
            &pages,
            "/benchmarks/1",
            &[SAME_ORIGIN],
            &[("run", run), ("name", name), ("criteria", criteria)],
        )
        .await;

        assert_eq!(status, case.status, "{run:?} {name:?} {criteria:?}");
        assert!(headers.get("hx-trigger").is_none());
        assert_eq!(
            placed(&form),
            (
                case.refused
                    .iter()
                    .map(|(field, reason)| (*field, (*reason).to_string()))
                    .collect::<Vec<_>>(),
                case.whole.map(str::to_string)
            ),
            "{run:?} {name:?} {criteria:?}"
        );
        let values = new_task_form(&form).0;
        assert_eq!(
            values[1..],
            [
                ("name", name.to_string()),
                ("criteria", criteria.to_string())
            ],
            "{run:?}"
        );
    }
    let (cross_site, _, cross_site_form) = post_answer(
        &pages,
        "/benchmarks/1",
        &[CROSS_SITE],
        &[
            ("run", "1/implement-2"),
            ("name", "gadgets"),
            ("criteria", "Ships gh.\n"),
        ],
    )
    .await;
    assert_eq!(cross_site, 403);
    assert_eq!(
        new_task_form(&cross_site_form).0,
        [
            ("run", String::new()),
            ("name", "gadgets".into()),
            ("criteria", "Ships gh.\n".into())
        ]
    );
    assert_eq!(tasks(&pages), ["1", "2"]);
}

/// `docs/specs/benchmarks.md`, Capturing: a run that is not a triage, design
/// or implement run is refused above the buttons.
#[test_log::test(tokio::test)]
async fn a_task_of_a_run_that_cannot_be_captured_is_refused_above_the_buttons() {
    let pages = with_runs().await;
    record_run(&pages, "rework", 1, "2026-10-13T11:20:00Z", None).await;

    let (status, form) = add(&pages, "1/rework-1", "Ships gh.\n").await;

    assert_eq!(status, 422);
    let (refused, whole) = placed(&form);
    assert_eq!(refused, []);
    assert!(whole.is_some_and(|whole| whole.contains("1/rework-1")));
    assert_eq!(tasks(&pages), ["1", "2"]);
}

/// `docs/specs/benchmarks.md`, New task: a benchmark that no longer exists
/// is refused above the buttons, New task having no field that chose it.
#[test_log::test(tokio::test)]
async fn a_task_in_a_benchmark_that_no_longer_exists_is_refused_above_the_buttons() {
    let pages = with_runs().await;

    let (status, _, form) = post_answer(
        &pages,
        "/benchmarks/gone",
        &[SAME_ORIGIN],
        &[
            ("run", "1/implement-2"),
            ("name", "gadgets"),
            ("criteria", "Ships gh.\n"),
        ],
    )
    .await;

    assert_eq!(status, 404);
    assert_eq!(
        placed(&form),
        (vec![], Some("there is no benchmark gone".into()))
    );
    assert_eq!(tasks(&pages), ["1", "2"]);
}

/// `docs/specs/benchmarks.md`, Capturing, Refusals on a screen: a capture
/// that fails says why above the buttons.
#[test_log::test(tokio::test)]
async fn a_failed_task_says_why_above_the_buttons() {
    let pages = with_runs().await;
    let missing = "0123456789abcdef0123456789abcdef01234567";
    record_run(
        &pages,
        "implement",
        3,
        "2026-10-13T11:10:00Z",
        Some(missing),
    )
    .await;

    let (status, form) = add(&pages, "1/implement-3", "Ships gh.\n").await;

    assert_eq!(status, 500);
    let (refused, whole) = placed(&form);
    assert_eq!(refused, []);
    let whole = whole.unwrap();
    assert!(whole.contains(missing), "{whole}");
    assert_eq!(tasks(&pages), ["1", "2"]);
}

/// Adds a task capturing run `run` as `gadgets` with `criteria` to the
/// benchmark `board`: the status and the form it answers with.
async fn add(pages: &Pages, run: &str, criteria: &str) -> (u16, Html) {
    let (status, _, form) = post_answer(
        pages,
        "/benchmarks/1",
        &[SAME_ORIGIN],
        &[("run", run), ("name", "gadgets"), ("criteria", criteria)],
    )
    .await;
    (status, form)
}

/// `docs/specs/benchmarks.md`, Capturing, Refusals on a screen.
#[test_log::test(tokio::test)]
async fn a_task_of_a_run_that_recorded_no_input_is_refused_saying_so() {
    let pages = with_runs().await;

    let (status, form) = add(&pages, "1/implement-1", "Ships gh.\n").await;

    assert_eq!(
        (status, new_task_form(&form).1),
        (422, Some("Run 1/implement-1 recorded no input.".into()))
    );
    assert_eq!(tasks(&pages), ["1", "2"]);
}

/// `docs/specs/benchmarks.md`, Capturing, Refusals on a screen.
#[test_log::test(tokio::test)]
async fn a_task_of_a_run_whose_record_lacks_a_part_is_refused_naming_it() {
    let pages = with_runs().await;
    let head = committed(&pages).await;
    record_run(&pages, "implement", 3, "2026-10-13T11:10:00Z", None).await;
    pages
        .input(
            "1/implement-3",
            &json!({
                "api_version": linewright_core::store::API_VERSION,
                "start": "2026-10-13T11:10:00Z",
                "items": [{"number": 1, "resources": {"issues/1": {"number": 1, "title": "Ship gh"}}}],
                "heads": {"main": head},
            }),
        )
        .await;

    let (status, form) = add(&pages, "1/implement-3", "Ships gh.\n").await;

    assert_eq!(
        (status, new_task_form(&form).1),
        (
            422,
            Some("The record of run 1/implement-3 lacks properties.".into())
        )
    );
    assert_eq!(tasks(&pages), ["1", "2"]);
}

/// `docs/specs/benchmarks.md`, Capturing, Before the first poll and Refusals
/// on a screen.
#[test_log::test(tokio::test)]
async fn before_the_first_poll_add_task_is_offered_and_a_task_refused() {
    let pages = with_runs().await;
    pages.served.send_replace(None);

    let html = pages.page("/benchmarks/1").await;
    let (status, form) = add(&pages, "1/implement-2", "Ships gh.\n").await;

    let add = select(&html, "dialog#new-task button[type=submit]");
    assert!(add[0].attr("disabled").is_none(), "{}", add[0].html());
    assert_eq!(select(&html, "#new-task-ready span").len(), 0);
    assert_eq!(
        (status, new_task_form(&form).1),
        (
            503,
            Some("Linewright can't capture a run before its first poll.".into())
        )
    );
    assert_eq!(tasks(&pages), ["1", "2"]);
}

#[test_log::test(tokio::test)]
async fn the_benchmark_pages_stream_sends_its_tasks_as_they_change() {
    let pages = with_benchmarks().await;
    let mut events = stream(&pages, "/events/benchmarks/1").await;
    events.recv().await.unwrap();

    post(
        &pages,
        "/benchmarks/1/remove-task",
        &[SAME_ORIGIN],
        &[("task", "2")],
    )
    .await;

    let tasks = loop {
        let event = events.recv().await.unwrap();
        if let Some((_, tasks)) = regions(&event).into_iter().find(|(id, _)| id == "tasks") {
            break tasks;
        }
    };
    let names: Vec<String> = select(&tasks, "tbody th > span:first-child")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(names, ["columns"]);
}

#[test_log::test(tokio::test)]
async fn the_benchmark_pages_stream_sends_the_runs_to_capture_as_they_change() {
    let pages = with_runs().await;
    let mut events = stream(&pages, "/events/benchmarks/1").await;
    events.recv().await.unwrap();
    let head = committed(&pages).await;

    record_run(&pages, "implement", 3, "2026-10-13T12:00:00Z", Some(&head)).await;

    let runs = next_region(&mut events, "new-task-run").await;
    let offered: Vec<Option<&str>> = select(&runs, "select[name=run] option")
        .into_iter()
        .map(|option| option.attr("value"))
        .collect();
    assert_eq!(offered, [Some("1/implement-3"), Some("1/implement-2")]);
}

/// The region `id` of the next event of `events` that sends it.
async fn next_region(events: &mut tokio::sync::mpsc::UnboundedReceiver<String>, id: &str) -> Html {
    loop {
        let event = events
            .recv()
            .await
            .unwrap_or_else(|| panic!("the stream ended before {id}"));
        if let Some((_, region)) = regions(&event).into_iter().find(|(sent, _)| sent == id) {
            return region;
        }
    }
}

/// `docs/specs/benchmarks.md`, Benchmark, Runs, and New task, Fields.
#[test_log::test(tokio::test)]
async fn the_benchmark_pages_stream_offers_new_benchmark_run_once_it_has_a_task_and_add_task_once_it_has_a_run()
 {
    let pages = with_benchmarks().await;
    let mut events = stream(&pages, "/events/benchmarks/2").await;
    let first = events.recv().await.unwrap();
    let sent = regions(&first);
    let region = |id: &str| {
        sent.iter()
            .find(|(sent, _)| sent == id)
            .map(|(_, region)| region.clone())
            .unwrap()
    };
    assert_eq!(
        select(&region("new-benchmark-run-offer"), "button").len(),
        0
    );
    let unready = region("new-task-ready");
    assert!(
        select(&unready, "button[type=submit]")[0]
            .attr("disabled")
            .is_some()
    );

    let head = committed(&pages).await;
    record_run(&pages, "triage", 1, "2026-10-13T09:00:00Z", Some(&head)).await;
    let ready = next_region(&mut events, "new-task-ready").await;
    keep_task(
        &pages.path(""),
        "triage-basics",
        "sorts",
        "1/triage-1",
        "Sorts it.\n",
    )
    .await;
    let offer = next_region(&mut events, "new-benchmark-run-offer").await;

    let add = select(&ready, "button[type=submit]");
    assert_eq!(text(add[0]), "Add task");
    assert!(add[0].attr("disabled").is_none(), "{}", ready.html());
    let new: Vec<String> = select(&offer, "button[command=show-modal]")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(new, ["New"]);
    assert_eq!(
        select(&offer, "#new-benchmark-run-offer")[0].attr("data-tasks"),
        Some("1")
    );
}

/// `docs/specs/design_system.md`, Type and Colour; `docs/specs/ui_patterns.md`,
/// Type and Status: a task whose criteria do not read says so in secondary text
/// after `failed`'s icon, never in text coloured `error`, whose fill is drawn
/// only behind its content colour.
#[test_log::test(tokio::test)]
async fn a_task_whose_criteria_do_not_read_says_so_in_secondary_text_after_faileds_icon() {
    let pages = with_benchmarks().await;
    keep_task(&pages.path(""), "board", "blank", "1/implement-1", "  \n").await;

    let html = pages.page("/benchmarks/1").await;

    let said: Vec<ElementRef> = select(&html, "#tasks tbody th > div")
        .into_iter()
        .filter(|line| text(*line).contains("the criteria do not read"))
        .collect();
    assert_eq!(said.len(), 1, "{}", html.html());
    let said = said[0];
    assert!(
        has_classes(said, &["text-base-content/60"]),
        "{}",
        said.html()
    );
    assert!(!said.html().contains("text-error"), "{}", said.html());
    let icon = select_in(said, ":scope > svg:first-child");
    assert_eq!(
        icon.first().and_then(|icon| icon.attr("data-icon")),
        Some("x-circle"),
        "{}",
        said.html()
    );
}

/// `docs/specs/ui_patterns.md`, Making: the reason for a refusal is a
/// daisyUI `alert` in the `alert-error` style, as daisyUI draws it.
#[test_log::test(tokio::test)]
async fn a_refused_writes_answer_is_an_error_alert_as_daisyui_draws_it() {
    let pages = with_benchmarks().await;

    let (_, answer) = post(
        &pages,
        "/benchmarks/2/remove-task",
        &[SAME_ORIGIN],
        &[("task", "1")],
    )
    .await;

    let alerts = select(&answer, ".alert");
    assert_eq!(alerts.len(), 1, "{}", answer.html());
    let mut classes: Vec<&str> = alerts[0].value().classes().collect();
    classes.sort_unstable();
    assert_eq!(classes, ["alert", "alert-error"]);
}

/// `docs/specs/benchmarks.md`, Names and IDs: a name is any text, which every
/// screen shows as text, and no HTML id is made from it.
#[test_log::test(tokio::test)]
async fn a_name_holding_markup_shows_as_text_on_every_screen_and_names_no_id() {
    const BENCHMARK: &str = "<script>quokka()</script> & Co";
    const TASK: &str = "<script>wombat()</script>";
    const CONFIGURATION: &str = "<script>numbat()</script>";
    let pages = with_benchmark_run().await;
    let head = committed(&pages).await;
    record_run(&pages, "implement", 2, "2026-10-13T11:00:00Z", Some(&head)).await;
    let pool = back_door(&pages.path("")).await;
    for (update, name) in [
        ("UPDATE benchmarks SET name = ? WHERE id = 1", BENCHMARK),
        ("UPDATE tasks SET name = ? WHERE id = 1", TASK),
        (
            "UPDATE benchmark_run_configurations SET name = ? WHERE benchmark_run = 1 AND seq = 1",
            CONFIGURATION,
        ),
    ] {
        sqlx::query(update).bind(name).execute(&pool).await.unwrap();
    }
    pool.close().await;

    for (path, names) in [
        ("/benchmarks", &[BENCHMARK][..]),
        ("/benchmarks/1/tasks", &[BENCHMARK, TASK][..]),
        ("/benchmarks/1/runs", &[BENCHMARK][..]),
        ("/tasks/1", &[BENCHMARK, TASK][..]),
        ("/benchmark-runs/1", &[BENCHMARK, TASK, CONFIGURATION][..]),
        ("/benchmark-runs/1/1/1/1/run", &[BENCHMARK, TASK][..]),
        ("/runs/1/implement-2", &[BENCHMARK][..]),
    ] {
        let (status, body) = pages.get(path).await;
        assert_eq!(status, 200, "{path}: {body}");

        assert!(!body.contains("<script>"), "{path}: {body}");
        let html = Html::parse_document(&body);
        let shown: String = html.root_element().text().collect();
        let titles: String = select(&html, "title")
            .into_iter()
            .map(|title| title.text().collect::<String>())
            .collect();
        for name in names {
            assert!(
                shown.contains(name) || titles.contains(name),
                "{path}: {name}: {body}"
            );
        }
        let ids: Vec<&str> = select(&html, "[id]")
            .into_iter()
            .filter_map(|element| element.attr("id"))
            .filter(|id| {
                ["quokka", "wombat", "numbat"]
                    .iter()
                    .any(|word| id.contains(word))
            })
            .collect();
        assert_eq!(ids, Vec::<&str>::new(), "{path}");
    }
}

/// `docs/specs/benchmarks.md`, Names and IDs, and Benchmarks, The table: a
/// benchmark is named in plain words, and the table is in the order of their
/// names by the Unicode Collation Algorithm.
#[test_log::test(tokio::test)]
async fn benchmarks_named_in_plain_words_are_listed_by_the_unicode_collation_algorithm() {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;

    for name in ["Éclair", "Banana", "apple"] {
        let (status, _, form) = post_answer(
            &pages,
            "/benchmarks",
            &[SAME_ORIGIN],
            &[("name", name), ("stage", "design")],
        )
        .await;
        assert_eq!(status, 200, "{name:?}: {}", form.html());
    }

    let html = pages.page("/benchmarks").await;
    assert_eq!(
        rows(&html, "#benchmarks"),
        [
            ["apple", "design", "", "View"],
            ["Banana", "design", "", "View"],
            ["Éclair", "design", "", "View"],
        ]
    );
    let filtered = pages.page("/benchmarks?name=BAN").await;
    assert_eq!(
        rows(&filtered, "#benchmarks"),
        [["Banana", "design", "", "View"]]
    );
}

/// The dialog "Edit" opens on `html`: its button among the screen's buttons,
/// a plain `btn` with `pencil-square` before its label
/// (`docs/specs/ui_patterns.md`, Editing and Icons), and the form of
/// the dialog it opens.
pub(super) fn edit_dialog(html: &Html) -> ElementRef<'_> {
    let edit: Vec<ElementRef> = select(html, "#buttons button")
        .into_iter()
        .filter(|button| text(*button) == "Edit")
        .collect();
    assert_eq!(edit.len(), 1, "{}", html.html());
    assert!(has_classes(edit[0], &["btn"]), "{}", edit[0].html());
    assert!(
        !edit[0]
            .value()
            .classes()
            .any(|class| class.starts_with("btn-") && class != "btn-sm"),
        "{}",
        edit[0].html()
    );
    assert_eq!(
        select_in(edit[0], ":scope > svg:first-child")[0].attr("data-icon"),
        Some("pencil-square")
    );
    let id = edit[0].attr("commandfor").unwrap();
    let dialog = select(html, &format!("dialog[id='{id}']"));
    assert_eq!(dialog.len(), 1, "{}", html.html());
    dialog[0]
}

/// A dialog's form drawn as Editing says (`docs/specs/ui_patterns.md`,
/// Editing, The dialog; `docs/specs/ui_patterns.md`, Editing): titled
/// `title`, with the fields `labels` above them, then "Cancel" and "Save",
/// posting to `action`, and what its form holds.
pub(super) fn edit_form(
    dialog: ElementRef<'_>,
    title: &str,
    labels: &[&str],
    action: &str,
) -> Html {
    let legend = select_in(dialog, "form fieldset > legend.fieldset-legend");
    assert_eq!(text(legend[0]), title);
    assert_eq!(dialog.attr("aria-labelledby"), legend[0].attr("id"));
    let form = select_in(dialog, "form[hx-post]")[0];
    assert_eq!(form.attr("hx-post"), Some(action));
    assert_eq!(form.attr("action"), Some(action));
    assert_eq!(form.attr("hx-target"), Some("this"));
    let shown: Vec<String> = select_in(form, "fieldset label")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(shown, labels);
    let buttons = select_in(form, ".modal-action button");
    assert_eq!(
        buttons
            .iter()
            .map(|button| text(*button))
            .collect::<Vec<_>>(),
        ["Cancel", "Save"]
    );
    assert_eq!(buttons[0].attr("commandfor"), dialog.attr("id"));
    assert_eq!(buttons[1].attr("type"), Some("submit"));
    assert!(has_classes(buttons[1], &["btn-primary"]));
    assert_eq!(
        select_in(buttons[1], ":scope > svg:first-child")[0].attr("data-icon"),
        Some("check")
    );
    Html::parse_fragment(&form.inner_html())
}

/// `docs/specs/benchmarks.md`, Edit benchmark; `docs/specs/ui_patterns.md`,
/// Editing, Edit and The dialog.
#[test_log::test(tokio::test)]
async fn edit_opens_the_edit_benchmark_dialog_holding_its_name() {
    let pages = with_benchmarks().await;

    let html = pages.page("/benchmarks/1").await;

    let form = edit_form(
        edit_dialog(&html),
        "Edit benchmark",
        &["Name"],
        "/benchmarks/1/edit",
    );
    assert_eq!(
        making_shown(&form),
        MakingShown {
            values: vec![("name", "board".into())],
            refused: vec![],
            whole: None,
        }
    );
}

/// `docs/specs/ui_patterns.md`, Editing, Saving: the dialog closes holding
/// what was saved, and the screen shows the new name.
#[test_log::test(tokio::test)]
async fn saving_a_benchmarks_name_closes_the_dialog_and_the_screen_shows_it() {
    let pages = with_benchmarks().await;
    let mut events = stream(&pages, "/events/benchmarks/1").await;
    events.recv().await.unwrap();

    let (status, headers, answer) = post_answer(
        &pages,
        "/benchmarks/1/edit",
        &[SAME_ORIGIN],
        &[("name", "  Board games ")],
    )
    .await;

    assert_eq!(status, 200, "{}", answer.html());
    assert_eq!(headers.get("hx-trigger").unwrap(), "made");
    assert_eq!(
        making_shown(&answer),
        MakingShown {
            values: vec![("name", "Board games".into())],
            refused: vec![],
            whole: None,
        }
    );
    let changed = saved_regions(&mut events, "heading", "h1", "Board games").await;
    assert_eq!(
        text(select(region_of(&changed, "new-task-title"), "legend")[0]),
        "New task in Board games"
    );
    assert_eq!(
        text(select(region_of(&changed, "new-benchmark-run-title"), "legend")[0]),
        "New benchmark run on Board games"
    );
    assert_eq!(
        select(region_of(&changed, "edit-benchmark-values"), "div")[0].attr("data-name"),
        Some("Board games")
    );
    let html = pages.page("/benchmarks/1").await;
    for (id, _) in &changed {
        assert_eq!(select(&html, &format!("[id='{id}']")).len(), 1, "{id}");
    }
    assert_eq!(
        select(&html, "dialog[id='edit-benchmark'] form")[0].attr("data-values"),
        Some("edit-benchmark-values")
    );
}

/// `docs/specs/benchmarks.md`, Edit benchmark, Refusals;
/// `docs/specs/ui_patterns.md`, Editing, Required and refused, and A thing
/// that is gone.
#[test_log::test(tokio::test)]
async fn a_refused_benchmark_name_keeps_what_the_reader_typed_with_its_reason() {
    let pages = with_benchmarks().await;

    for (path, name, status, refused, whole) in [
        (
            "/benchmarks/1/edit",
            " ",
            422,
            vec![("name", "Required.")],
            None,
        ),
        (
            "/benchmarks/1/edit",
            "\u{200b}",
            422,
            vec![("name", "Required.")],
            None,
        ),
        (
            "/benchmarks/1/edit",
            "triage-basics",
            422,
            vec![("name", "A benchmark named triage-basics already exists.")],
            None,
        ),
        (
            "/benchmarks/9/edit",
            "fresh",
            404,
            vec![],
            Some("No such benchmark 9"),
        ),
    ] {
        let (answered, headers, form) =
            post_answer(&pages, path, &[SAME_ORIGIN], &[("name", name)]).await;

        assert_eq!(answered, status, "{path} {name:?}");
        assert!(headers.get("hx-trigger").is_none());
        assert_eq!(
            making_shown(&form),
            MakingShown {
                values: vec![("name", name.to_string())],
                refused: refused
                    .iter()
                    .map(|(field, reason)| (*field, (*reason).to_string()))
                    .collect(),
                whole: whole.map(String::from),
            },
            "{path} {name:?}"
        );
        assert_eq!(text(select(&form, "legend")[0]), "Edit benchmark");
    }
    let html = pages.page("/benchmarks/1").await;
    assert_eq!(text(select(&html, "#heading h1")[0]), "board");
}

/// The regions of the first event from `events` whose region `region` holds
/// `text` in its `selector`: the regions a save changed, which a stream sends
/// together.
pub(super) async fn saved_regions(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    region: &str,
    selector: &str,
    text_now: &str,
) -> Vec<(String, Html)> {
    loop {
        let changed = regions(&events.recv().await.unwrap());
        if changed.iter().any(|(id, html)| {
            id == region
                && select(html, selector)
                    .first()
                    .is_some_and(|shown| text(*shown) == text_now)
        }) {
            return changed;
        }
    }
}
