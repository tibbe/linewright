use super::benchmark_runs::launched;
use super::*;

use std::panic::Location;

use serde_json::json;

/// The pages of #1, `Ship gh`, with the implement benchmark `board`, holding
/// the tasks `columns`, captured from `1/implement-1` and of two lines of
/// criteria, and `lanes`, and the empty triage benchmark `triage-basics`.
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

/// `docs/specs/benchmarks.md`, Benchmarks; `docs/specs/design_system.md`,
/// Tables.
#[test_log::test(tokio::test)]
async fn the_benchmarks_page_shows_a_row_per_benchmark_by_name_in_a_card_with_its_filters_and_new()
{
    let pages = with_benchmarks().await;
    launched(&pages, "board", &[], 1, "2026-11-17T10:00:00Z").await;
    launched(&pages, "board", &[], 1, "2026-11-17T12:00:00Z").await;

    let html = pages.page("/benchmarks").await;

    assert_eq!(
        table_bars(&html),
        [(
            "Benchmarks".to_string(),
            vec!["name".to_string(), "stage".into(), "New".into()]
        )]
    );
    let heads: Vec<String> = select(&html, "#benchmarks thead th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(heads, ["", "Stage", "Last benchmark run"]);
    assert_eq!(
        rows(&html, "#benchmarks"),
        [
            ["board", "implement", "2026-11-17 12:00:00 UTC"],
            ["triage-basics", "triage", ""],
        ]
    );
    let names: Vec<String> = select(&html, "#benchmarks tbody tr > th:first-child")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(names, ["board", "triage-basics"]);
    assert_eq!(
        hrefs(select(&html, "#benchmarks tbody a")),
        [
            "/benchmarks/board",
            "/benchmark-runs/2",
            "/benchmarks/triage-basics"
        ]
    );
    assert_eq!(select(&html, ".table-card #benchmarks table").len(), 1);
    assert_eq!(events_path(&html), Some("/events/benchmarks"));
}

/// `docs/specs/design_system.md`, Screen states, Empty.
#[test_log::test(tokio::test)]
async fn the_benchmarks_page_with_no_benchmarks_says_so() {
    let pages = Pages::of(view(vec![], vec![])).await;

    let html = pages.page("/benchmarks").await;

    assert_eq!(text(select(&html, "#benchmarks p")[0]), "No benchmarks");
}

/// `docs/specs/benchmarks.md`, Benchmarks, Filters;
/// `docs/specs/design_system.md`, Filters.
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
        [["triage-basics", "triage", ""]]
    );
}

/// `docs/specs/web/design_system.md`, the addresses: a screen is at the address
/// the table gives it, and at no other.
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

/// The New benchmark dialog's form as a reader meets it: the value of each
/// field, a select's its chosen option's, and the refusal above its buttons, if
/// any.
#[track_caller]
fn new_benchmark_form(form: &Html) -> (Vec<(&str, String)>, Option<String>) {
    let refusals = select(form, ".alert");
    assert!(refusals.len() <= 1, "{}", form.html());
    (
        field_values(select(form, "input[name], select[name]")),
        refusals.first().map(|refusal| text(*refusal)),
    )
}

/// `docs/specs/benchmarks.md`, New benchmark; `docs/specs/design_system.md`,
/// Dialogs and Making; `docs/specs/web/design_system.md`, Navigation and
/// Making.
#[test_log::test(tokio::test)]
async fn new_opens_the_new_benchmark_dialog_outside_the_benchmarks_with_name_and_stage_on_triage() {
    let pages = with_benchmarks().await;

    let html = pages.page("/benchmarks").await;

    let new = select(&html, ".table-bar button[command=show-modal]");
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
        select(&html, "#benchmarks dialog, .table-bar dialog").len(),
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
    assert_eq!(
        new_benchmark_form(&Html::parse_fragment(&form.inner_html())),
        (
            vec![("name", String::new()), ("stage", "triage".into())],
            None
        )
    );
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

/// `docs/specs/design_system.md`, Making.
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
        new_benchmark_form(&answer),
        (
            vec![("name", String::new()), ("stage", "triage".into())],
            None
        )
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
    assert_eq!(rows(&benchmarks, "#benchmarks"), [["board", "design", ""]]);
}

/// `docs/specs/benchmarks.md`, New benchmark, Refusals;
/// `docs/specs/design_system.md`, Making, A refusal.
#[test_log::test(tokio::test)]
async fn a_refused_benchmark_answers_the_form_as_sent_with_the_reason() {
    let pages = with_benchmarks().await;

    let (taken, taken_headers, taken_form) = post_answer(
        &pages,
        "/benchmarks",
        &[SAME_ORIGIN],
        &[("name", "board"), ("stage", "design")],
    )
    .await;
    let (malformed, _, malformed_form) = post_answer(
        &pages,
        "/benchmarks",
        &[SAME_ORIGIN],
        &[("name", "Board 2"), ("stage", "implement")],
    )
    .await;
    let (no_stage, _, no_stage_form) = post_answer(
        &pages,
        "/benchmarks",
        &[SAME_ORIGIN],
        &[("name", "fresh"), ("stage", "rework")],
    )
    .await;
    let (cross_site, _, cross_site_form) = post_answer(
        &pages,
        "/benchmarks",
        &[CROSS_SITE],
        &[("name", "fresh"), ("stage", "design")],
    )
    .await;

    assert_eq!(taken, 422);
    assert!(taken_headers.get("hx-trigger").is_none());
    assert_eq!(
        new_benchmark_form(&taken_form),
        (
            vec![("name", "board".into()), ("stage", "design".into())],
            Some("A benchmark named board already exists.".into())
        )
    );
    assert_eq!(malformed, 422);
    assert_eq!(
        new_benchmark_form(&malformed_form),
        (
            vec![("name", "Board 2".into()), ("stage", "implement".into())],
            Some(
                "A name holds only lowercase letters, digits and hyphens, and starts with a \
                 letter or a digit."
                    .into()
            )
        )
    );
    assert_eq!(no_stage, 422);
    assert_eq!(
        new_benchmark_form(&no_stage_form),
        (
            vec![("name", "fresh".into()), ("stage", "triage".into())],
            Some(
                "\"rework\" is not a stage of a benchmark: one of triage, design, implement".into()
            )
        )
    );
    assert_eq!(cross_site, 403);
    assert_eq!(
        new_benchmark_form(&cross_site_form).0,
        [("name", "fresh".into()), ("stage", "design".into())]
    );
    let html = pages.page("/benchmarks").await;
    assert_eq!(
        rows(&html, "#benchmarks"),
        [["board", "implement", ""], ["triage-basics", "triage", ""]]
    );
}

/// The tabs of the Benchmark page `html`, in order: each one's label, whether
/// it is checked, and the ids of the regions its panel holds.
#[track_caller]
fn tabs(html: &Html) -> Vec<(String, bool, Vec<String>)> {
    let lists = select(html, "main [role=tablist]");
    assert_eq!(lists.len(), 1, "{}", html.html());
    assert!(has_classes(lists[0], &["tabs", "tabs-border"]));
    select_in(lists[0], ":scope > input.tab")
        .into_iter()
        .map(|tab| {
            assert_eq!(tab.attr("type"), Some("radio"));
            let panel = tab
                .next_siblings()
                .find_map(ElementRef::wrap)
                .expect("the tab's panel");
            assert!(has_classes(panel, &["tab-content"]), "{}", panel.html());
            (
                tab.attr("aria-label").unwrap_or_default().to_string(),
                tab.attr("checked").is_some(),
                select_in(panel, "[id]:is(#benchmark-runs, #tasks)")
                    .into_iter()
                    .filter_map(|region| region.attr("id").map(str::to_string))
                    .collect(),
            )
        })
        .collect()
}

/// `docs/specs/benchmarks.md`, Benchmark, Heading and Tabs;
/// `docs/specs/web/benchmarks.md`, Benchmark.
#[test_log::test(tokio::test)]
async fn the_benchmark_page_heads_with_its_name_and_stage_and_opens_on_tasks_while_it_has_no_benchmark_runs()
 {
    let pages = with_benchmarks().await;

    let html = pages.page("/benchmarks/board").await;

    assert_eq!(text(select(&html, "main h1")[0]), "board");
    assert_eq!(text(select(&html, "main h1 + p")[0]), "implement");
    assert_eq!(
        tabs(&html),
        [
            (
                "Runs".to_string(),
                false,
                vec!["benchmark-runs".to_string()]
            ),
            ("Tasks".to_string(), true, vec!["tasks".to_string()]),
        ]
    );
    assert_eq!(select(&html, "#benchmark-runs [role=tablist], #tasks [role=tablist], #benchmark-runs input.tab, #tasks input.tab").len(), 0);
}

/// `docs/specs/benchmarks.md`, Benchmark, Tabs.
#[test_log::test(tokio::test)]
async fn the_benchmark_page_opens_on_benchmark_runs_once_it_has_one() {
    let pages = with_benchmarks().await;
    launched(&pages, "board", &[], 1, "2026-11-17T10:00:00Z").await;

    let html = pages.page("/benchmarks/board").await;

    let opened: Vec<(String, bool)> = tabs(&html)
        .into_iter()
        .map(|(label, checked, _)| (label, checked))
        .collect();
    assert_eq!(
        opened,
        [("Runs".to_string(), true), ("Tasks".to_string(), false)]
    );
}

#[test_log::test(tokio::test)]
async fn a_task_named_as_another_tasks_question_has_a_confirmation_of_its_own() {
    let pages = with_benchmarks().await;
    keep_task(
        &pages.path(""),
        "board",
        "lanes-question",
        "1/implement-1",
        "Asks.\n",
    )
    .await;

    let html = pages.page("/benchmarks/board").await;

    let questions: Vec<String> = confirmations(&html)
        .into_iter()
        .map(|confirmation| confirmation.question)
        .collect();
    assert_eq!(
        questions,
        [
            "Remove the task columns and its directory?",
            "Remove the task lanes and its directory?",
            "Remove the task lanes-question and its directory?",
        ]
    );
}

#[test_log::test(tokio::test)]
async fn every_write_from_another_site_is_refused_and_writes_nothing() {
    let pages = with_benchmarks().await;
    let writes: [(&str, &[(&str, &str)]); 3] = [
        ("/benchmarks", &[("name", "fresh"), ("stage", "design")]),
        (
            "/benchmarks/board",
            &[
                ("run", "1/implement-1"),
                ("name", "more"),
                ("criteria", "Ships.\n"),
            ],
        ),
        ("/benchmarks/board/remove-task", &[("task", "columns")]),
    ];

    for (path, fields) in writes {
        let (status, _) = post(&pages, path, &[CROSS_SITE], fields).await;
        assert_eq!(status, 403, "{path}");
    }

    let html = pages.page("/benchmarks").await;
    assert_eq!(
        rows(&html, "#benchmarks"),
        [["board", "implement", ""], ["triage-basics", "triage", ""]]
    );
    assert_eq!(tasks(&pages), ["columns", "lanes"]);
}

/// `docs/specs/design_system.md`, Screen states, Nothing there.
#[test_log::test(tokio::test)]
async fn the_page_of_no_benchmark_says_there_is_none() {
    let pages = with_benchmarks().await;

    let (no_benchmark, no_benchmark_body) = pages.get("/benchmarks/nothing").await;
    let (no_name, no_name_body) = pages.get("/benchmarks/Not%20a%20name").await;

    for (status, body, shown) in [
        (no_benchmark, no_benchmark_body, "No such benchmark nothing"),
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

    let (page, _) = pages.get("/benchmarks/nothing").await;
    let (capture, _) = post(
        &pages,
        "/benchmarks/nothing",
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
        "/benchmarks/nothing/remove-task",
        &[SAME_ORIGIN],
        &[("task", "columns")],
    )
    .await;

    assert_eq!([page, capture, removal], [404, 404, 404]);
    assert_eq!(tasks(&pages), ["columns", "lanes"]);
}

/// `docs/specs/benchmarks.md`, Benchmark, Tasks; `docs/specs/design_system.md`,
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

    let html = pages.page("/benchmarks/board").await;

    assert_eq!(events_path(&html), Some("/events/benchmarks/board"));
    let heads: Vec<String> = select(&html, "#tasks thead th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(heads, ["", "Captured from", ""]);
    let tasks = select(&html, "#tasks tbody tr:not([data-fold-of])");
    let shown: Vec<(String, String, String)> = tasks
        .iter()
        .map(|row| {
            let header = select_in(*row, ":scope > th")[0];
            let cells = select_in(*row, ":scope > td");
            (
                text(header),
                text(cells[0]),
                text(select_in(cells[1], "button[command=show-modal]")[0]),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            (
                "columns Names the columns.".to_string(),
                "1/implement-1".to_string(),
                "Remove".to_string()
            ),
            (
                "lanes Stacks them.".into(),
                "1/implement-2".into(),
                "Remove".into()
            ),
            (
                "aisles Walks the aisles.".into(),
                "1/implement-1".into(),
                "Remove".into()
            ),
        ]
    );
    assert_eq!(
        hrefs(select(&html, "#tasks tbody td a")),
        [
            "/runs/1/implement-1",
            "/runs/1/implement-2",
            "/runs/1/implement-1"
        ]
    );
    let criteria = select(&html, "#tasks tbody tr[data-fold-of]");
    assert_eq!(criteria.len(), 3);
    assert_eq!(
        select_in(criteria[0], "pre")[0].text().collect::<String>(),
        "Names the columns.\nAnd orders them.\n"
    );
    assert_eq!(select_in(criteria[0], "td")[0].attr("colspan"), Some("3"));
}

/// `docs/specs/benchmarks.md`, Benchmark, Tasks: a task's row opens on click to
/// the whole of its criteria, as a fold row whose ids are keyed by names.
#[test_log::test(tokio::test)]
async fn a_tasks_row_is_a_fold_row_opening_to_its_criteria() {
    let pages = with_benchmarks().await;

    let html = pages.page("/benchmarks/board").await;

    let rows: Vec<(Option<&str>, Option<&str>, bool)> = select(&html, "#tasks tbody tr")
        .into_iter()
        .map(|row| {
            (
                row.attr("id"),
                row.attr("data-fold-of"),
                row.attr("hidden").is_some(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (Some("task-columns"), None, false),
            (Some("task-columns_criteria"), Some("task-columns"), true),
            (Some("task-lanes"), None, false),
            (Some("task-lanes_criteria"), Some("task-lanes"), true),
        ]
    );
    let buttons = select(&html, "#task-columns > th:first-child > button");
    assert_eq!(buttons.len(), 1, "{}", html.html());
    assert_eq!(buttons[0].attr("type"), Some("button"));
    assert_eq!(buttons[0].attr("aria-expanded"), Some("false"));
    assert_eq!(
        buttons[0].attr("aria-controls"),
        Some("task-columns_criteria")
    );
    assert_eq!(text(buttons[0]), "columns");
    assert_eq!(select(&html, "#tasks details").len(), 0);
}

/// `docs/specs/benchmarks.md`, Benchmark, Tasks, and Remove task;
/// `docs/specs/design_system.md`, Confirming.
#[test_log::test(tokio::test)]
async fn each_task_is_removed_after_a_confirmation_naming_it() {
    let pages = with_benchmarks().await;

    let html = pages.page("/benchmarks/board").await;

    let removal = |task: &str| Confirmation {
        asking: "Remove".into(),
        question: format!("Remove the task {task} and its directory?"),
        turn_down: "Cancel".into(),
        action: "/benchmarks/board/remove-task".into(),
        fields: vec![("task".into(), task.into())],
    };
    assert_eq!(confirmations(&html), [removal("columns"), removal("lanes")]);
    assert!(select(&html, "[hx-confirm]").is_empty(), "{}", html.html());
}

/// `docs/specs/design_system.md`, Screen states, Empty;
/// `docs/specs/benchmarks.md`, New task, Fields.
#[test_log::test(tokio::test)]
async fn a_benchmark_with_no_tasks_benchmark_runs_or_runs_of_its_stage_says_so() {
    let pages = with_benchmarks().await;

    let html = pages.page("/benchmarks/triage-basics").await;

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
async fn removing_a_task_removes_it_from_the_page_and_from_the_tasks() {
    let pages = with_benchmarks().await;

    let (status, answer) = post(
        &pages,
        "/benchmarks/board/remove-task",
        &[SAME_ORIGIN],
        &[("task", "columns")],
    )
    .await;

    assert_eq!(status, 200, "{}", said(&answer));
    assert_eq!(said(&answer), "Removed the task columns");
    assert_eq!(tasks(&pages), ["lanes"]);
    let html = pages.page("/benchmarks/board").await;
    let names: Vec<String> = select(&html, "#tasks tbody th button")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(names, ["lanes"]);
}

#[test_log::test(tokio::test)]
async fn removing_a_task_the_benchmark_does_not_hold_is_refused() {
    let pages = with_benchmarks().await;

    let (status, answer) = post(
        &pages,
        "/benchmarks/triage-basics/remove-task",
        &[SAME_ORIGIN],
        &[("task", "columns")],
    )
    .await;

    assert_eq!(status, 404);
    assert_eq!(
        said(&answer),
        "Not removed: the benchmark triage-basics has no task columns"
    );
    assert_eq!(tasks(&pages), ["columns", "lanes"]);
}

/// Keeps #1's run `n` of `stage`, started at `started` and ended half an
/// hour later, and, given the default branch's `head`, the whole input it
/// recorded.
#[track_caller]
fn record_run<'a>(
    pages: &'a Pages,
    stage: &str,
    n: u32,
    started: &str,
    head: Option<&str>,
) -> impl Future<Output = ()> + 'a {
    let record = pages.record(&json!({
        "item": 1,
        "stage": stage,
        "n": n,
        "started_at": started,
        "end": {"at": "2026-10-13T11:30:00Z", "ending": "done"},
        "sessions": [],
    }));
    let input = head.map(|head| {
        pages.input(
            &format!("1/{stage}-{n}"),
            &json!({
                "api_version": linewright::store::API_VERSION,
                "start": started,
                "items": [{"number": 1, "resources": {"issues/1": {"number": 1, "title": "Ship gh"}}}],
                "heads": {"main": head},
                "properties": {"resources": {"account": {"login": "factory", "id": 7}}},
            }),
        )
    });
    async move {
        record.await;
        if let Some(input) = input {
            input.await;
        }
    }
}

/// The default branch's head, committed in `pages`' store.
#[track_caller]
fn committed(pages: &Pages) -> impl Future<Output = String> + '_ {
    let caller = Location::caller();
    async move {
        pages
            .store
            .commit("maintainer", "main", &[("README.md", "Gadgets\n")], "Start")
            .await
            .unwrap_or_else(|e| panic!("committing on main at {caller}: {e:#}"))
    }
}

/// [`with_benchmarks`]'s pages, with #1's runs: its design run 1, and its
/// implement runs 1 and 2, the design run and the last implement run
/// having recorded their whole input.
#[track_caller]
fn with_runs() -> impl Future<Output = Pages> {
    let benchmarks = Box::pin(with_benchmarks());
    async move {
        let pages = benchmarks.await;
        let head = committed(&pages).await;
        record_run(&pages, "design", 1, "2026-10-13T09:00:00Z", Some(&head)).await;
        record_run(&pages, "implement", 1, "2026-10-13T10:00:00Z", None).await;
        record_run(&pages, "implement", 2, "2026-10-13T11:00:00Z", Some(&head)).await;
        pages
    }
}

/// A "New" dialog's form as a reader meets it: the name and value of each
/// field, and the refusal above its buttons, if any.
type FormShown<'a> = (Vec<(&'a str, String)>, Option<String>);

/// The New task dialog's form as a reader meets it: the value of each field,
/// a select's its chosen option's, and the refusal above its buttons, if any.
#[track_caller]
fn new_task_form(form: &Html) -> FormShown<'_> {
    let refusals = select(form, ".alert");
    assert!(refusals.len() <= 1, "{}", form.html());
    let mut fields = field_values(select(form, "input[name], select[name]"));
    fields.extend(
        select(form, "textarea[name]")
            .into_iter()
            .map(|area| (area.attr("name").unwrap(), area.text().collect())),
    );
    (fields, refusals.first().map(|refusal| text(*refusal)))
}

/// `docs/specs/benchmarks.md`, New task; `docs/specs/design_system.md`,
/// Dialogs and Making.
#[test_log::test(tokio::test)]
async fn new_opens_the_new_task_dialog_offering_the_runs_of_the_benchmarks_stage_that_can_be_captured()
 {
    let pages = with_runs().await;

    let html = pages.page("/benchmarks/board").await;

    let buttons: Vec<&str> = select(&html, ".table-bar button[command=show-modal]")
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
    assert_eq!(form.attr("hx-post"), Some("/benchmarks/board"));
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
    let runs: Vec<(Option<&str>, Option<&str>)> = select_in(form, "select[name=run] option")
        .into_iter()
        .map(|option| (option.attr("value"), option.attr("data-name")))
        .collect();
    assert_eq!(runs, [(Some("1/implement-2"), Some("1-implement-2"))]);
    assert_eq!(
        new_task_form(&Html::parse_fragment(&form.inner_html())),
        (
            vec![
                ("run", String::new()),
                ("name", "1-implement-2".into()),
                ("criteria", String::new())
            ],
            None
        )
    );
    let actions = select_in(form, ".modal-action button");
    let action_labels: Vec<String> = actions.iter().map(|button| text(*button)).collect();
    assert_eq!(action_labels, ["Cancel", "Add task"]);
    assert!(has_classes(actions[1], &["btn-primary"]));
    assert!(actions[1].attr("disabled").is_none());
}

/// `docs/specs/benchmarks.md`, New task; `docs/specs/design_system.md`,
/// Making.
#[test_log::test(tokio::test)]
async fn adding_a_task_answers_the_form_as_it_starts_closing_the_dialog_and_the_task_shows_last() {
    let pages = with_runs().await;

    let (status, headers, answer) = post_answer(
        &pages,
        "/benchmarks/board",
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
                ("name", "1-implement-2".into()),
                ("criteria", String::new())
            ],
            None
        )
    );
    assert_eq!(text(select(&answer, "legend")[0]), "New task in board");
    assert_eq!(tasks(&pages), ["columns", "gadgets", "lanes"]);
    let html = pages.page("/benchmarks/board").await;
    let names: Vec<String> = select(&html, "#tasks tbody th button")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(names, ["columns", "lanes", "gadgets"]);
}

/// `docs/specs/benchmarks.md`, New task, Refusals;
/// `docs/specs/design_system.md`, Making, A refusal.
#[test_log::test(tokio::test)]
async fn a_refused_task_answers_the_form_as_sent_with_the_reason() {
    let pages = with_runs().await;
    let sent = |name: &'static str, criteria: &'static str| {
        [
            ("run", "1/implement-2"),
            ("name", name),
            ("criteria", criteria),
        ]
    };
    let as_sent = |name: &str, criteria: &str| {
        vec![
            ("run", "1/implement-2".to_string()),
            ("name", name.to_string()),
            ("criteria", criteria.to_string()),
        ]
    };

    let mut answers = vec![];
    for fields in [
        sent("columns", "Ships gh.\n"),
        sent("Gadgets 2", "Ships gh.\n"),
        sent("gadgets", "  \n"),
    ] {
        answers.push(post_answer(&pages, "/benchmarks/board", &[SAME_ORIGIN], &fields).await);
    }
    let (other_stage, _, other_stage_form) = post_answer(
        &pages,
        "/benchmarks/board",
        &[SAME_ORIGIN],
        &[
            ("run", "1/design-1"),
            ("name", "gadgets"),
            ("criteria", "Ships gh.\n"),
        ],
    )
    .await;
    let (cross_site, _, cross_site_form) = post_answer(
        &pages,
        "/benchmarks/board",
        &[CROSS_SITE],
        &sent("gadgets", "Ships gh.\n"),
    )
    .await;

    let refused: Vec<(u16, bool, FormShown)> = answers
        .iter()
        .map(|(status, headers, form)| {
            (
                *status,
                headers.get("hx-trigger").is_some(),
                new_task_form(form),
            )
        })
        .collect();
    assert_eq!(
        refused,
        [
            (
                422,
                false,
                (
                    as_sent("columns", "Ships gh.\n"),
                    Some("A task named columns already exists.".into())
                )
            ),
            (
                422,
                false,
                (
                    as_sent("Gadgets 2", "Ships gh.\n"),
                    Some(
                        "A name holds only lowercase letters, digits and hyphens, and starts \
                         with a letter or a digit."
                            .into()
                    )
                )
            ),
            (
                422,
                false,
                (
                    as_sent("gadgets", "  \n"),
                    Some("A task needs criteria.".into())
                )
            ),
        ]
    );
    assert_eq!(other_stage, 422);
    assert_eq!(
        new_task_form(&other_stage_form).1,
        Some(
            "the benchmark board holds tasks of implement, and the run 1/design-1 is of design"
                .into()
        )
    );
    assert_eq!(cross_site, 403);
    assert_eq!(
        new_task_form(&cross_site_form).0,
        [
            ("run", String::new()),
            ("name", "gadgets".into()),
            ("criteria", "Ships gh.\n".into())
        ]
    );
    assert_eq!(tasks(&pages), ["columns", "lanes"]);
}

#[test_log::test(tokio::test)]
async fn the_benchmark_pages_stream_sends_its_tasks_as_they_change() {
    let pages = with_benchmarks().await;
    let mut events = stream(&pages, "/events/benchmarks/board").await;
    events.recv().await.unwrap();

    post(
        &pages,
        "/benchmarks/board/remove-task",
        &[SAME_ORIGIN],
        &[("task", "lanes")],
    )
    .await;

    let tasks = loop {
        let event = events.recv().await.unwrap();
        if let Some((_, tasks)) = regions(&event).into_iter().find(|(id, _)| id == "tasks") {
            break tasks;
        }
    };
    let names: Vec<String> = select(&tasks, "tbody th button")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(names, ["columns"]);
}

#[test_log::test(tokio::test)]
async fn the_benchmark_pages_stream_sends_the_runs_to_capture_as_they_change() {
    let pages = with_runs().await;
    let mut events = stream(&pages, "/events/benchmarks/board").await;
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
#[track_caller]
fn next_region<'a>(
    events: &'a mut tokio::sync::mpsc::UnboundedReceiver<String>,
    id: &'a str,
) -> impl Future<Output = Html> + 'a {
    let caller = Location::caller();
    async move {
        loop {
            let event = events
                .recv()
                .await
                .unwrap_or_else(|| panic!("the stream ended before {id}, at {caller}"));
            if let Some((_, region)) = regions(&event).into_iter().find(|(sent, _)| sent == id) {
                return region;
            }
        }
    }
}

/// `docs/specs/benchmarks.md`, Benchmark, Runs, and New task, Fields.
#[test_log::test(tokio::test)]
async fn the_benchmark_pages_stream_offers_new_benchmark_run_once_it_has_a_task_and_add_task_once_it_has_a_run()
 {
    let pages = with_benchmarks().await;
    let mut events = stream(&pages, "/events/benchmarks/triage-basics").await;
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
