use super::experiments::launched;
use super::*;

use std::panic::Location;

use serde_json::json;

/// The pages of #1, `Ship gh`, with the implement suite `board`, holding
/// the cases `columns`, captured from `1/implement-1` and of two lines of
/// criteria, and `lanes`, and the empty triage suite `triage-basics`.
pub(super) async fn with_suites() -> Pages {
    let pages = Pages::of(view(vec![ticket(1, "Ship gh")], vec![])).await;
    let work_dir = pages.path("");
    keep_suite(&work_dir, "board", "implement").await;
    keep_suite(&work_dir, "triage-basics", "triage").await;
    keep_case(
        &work_dir,
        "board",
        "columns",
        "1/implement-1",
        "Names the columns.\nAnd orders them.\n",
    )
    .await;
    keep_case(
        &work_dir,
        "board",
        "lanes",
        "1/implement-2",
        "Stacks them.\n",
    )
    .await;
    pages
}

/// `docs/specs/benchmarks.md`, Suites; `docs/specs/design_system.md`,
/// Tables.
#[test_log::test(tokio::test)]
async fn the_suites_page_shows_a_row_per_suite_by_name_in_a_card_with_its_filters_and_new() {
    let pages = with_suites().await;
    launched(&pages, "board", &[], 1, "2026-11-17T10:00:00Z").await;
    launched(&pages, "board", &[], 1, "2026-11-17T12:00:00Z").await;

    let html = pages.page("/suites").await;

    assert_eq!(
        table_bars(&html),
        [(
            "Suites".to_string(),
            vec!["name".to_string(), "stage".into(), "New".into()]
        )]
    );
    let heads: Vec<String> = select(&html, "#suites thead th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(heads, ["", "Stage", "Last experiment"]);
    assert_eq!(
        rows(&html, "#suites"),
        [
            ["board", "implement", "2026-11-17 12:00:00 UTC"],
            ["triage-basics", "triage", ""],
        ]
    );
    let names: Vec<String> = select(&html, "#suites tbody tr > th:first-child")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(names, ["board", "triage-basics"]);
    assert_eq!(
        hrefs(select(&html, "#suites tbody a")),
        ["/suites/board", "/experiments/2", "/suites/triage-basics"]
    );
    assert_eq!(select(&html, ".table-card #suites table").len(), 1);
    assert_eq!(events_path(&html), Some("/events/suites"));
}

/// `docs/specs/design_system.md`, Screen states, Empty.
#[test_log::test(tokio::test)]
async fn the_suites_page_with_no_suites_says_so() {
    let pages = Pages::of(view(vec![], vec![])).await;

    let html = pages.page("/suites").await;

    assert_eq!(text(select(&html, "#suites p")[0]), "No suites");
}

/// `docs/specs/benchmarks.md`, Suites, Filters; `docs/specs/design_system.md`,
/// Filters.
#[test_log::test(tokio::test)]
async fn the_suites_filters_narrow_the_suites_and_are_carried_in_the_pages_link_and_stream() {
    let pages = with_suites().await;
    keep_suite(&pages.path(""), "triage-boards", "triage").await;

    let by_both = pages.page("/suites?name=BOA&stage=triage").await;
    let unset = pages.page("/suites?name=&stage=").await;

    let names = |html: &Html| -> Vec<String> {
        rows(html, "#suites")
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
        Some("/events/suites?name=BOA&stage=triage")
    );
    assert_eq!(
        select(&by_both, "form[data-filters]")[0].attr("data-filters"),
        Some("/events/suites")
    );
    let stages: Vec<String> = select(&unset, "form[data-filters] select[name=stage] option")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(stages, ["All stages", "triage", "design", "implement"]);
}

#[test_log::test(tokio::test)]
async fn the_suites_pages_stream_sends_the_suites_its_filters_keep() {
    let pages = with_suites().await;
    let mut events = stream(&pages, "/events/suites?stage=triage").await;

    let event = events.recv().await.unwrap();

    let (_, suites) = regions(&event)
        .into_iter()
        .find(|(id, _)| id == "suites")
        .expect("the suites");
    assert_eq!(rows(&suites, "#suites"), [["triage-basics", "triage", ""]]);
}

#[test_log::test(tokio::test)]
async fn a_stage_to_filter_on_that_is_not_a_suites_says_so_in_place_of_the_suites() {
    let pages = with_suites().await;

    let (status, body) = pages.get("/suites?stage=rework").await;

    assert_eq!(status, 400);
    let html = Html::parse_document(&body);
    assert_eq!(
        text(select(&html, "#suites p")[0]),
        "\"rework\" is not a stage of a suite: one of triage, design, implement"
    );
}

/// The New suite dialog's form as a reader meets it: the value of each field,
/// a select's its chosen option's, and the refusal above its buttons, if any.
#[track_caller]
fn new_suite_form(form: &Html) -> (Vec<(&str, String)>, Option<String>) {
    let refusals = select(form, ".alert");
    assert!(refusals.len() <= 1, "{}", form.html());
    (
        field_values(select(form, "input[name], select[name]")),
        refusals.first().map(|refusal| text(*refusal)),
    )
}

/// `docs/specs/benchmarks.md`, New suite; `docs/specs/design_system.md`,
/// Dialogs and Making; `docs/specs/web/design_system.md`, Navigation and
/// Making.
#[test_log::test(tokio::test)]
async fn new_opens_the_new_suite_dialog_outside_the_suites_with_name_and_stage_on_triage() {
    let pages = with_suites().await;

    let html = pages.page("/suites").await;

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
    assert_eq!(select(&html, "#suites dialog, .table-bar dialog").len(), 0);
    let legend = select_in(dialog, "form fieldset > legend.fieldset-legend");
    assert_eq!(text(legend[0]), "New suite");
    assert_eq!(
        dialog.attr("aria-labelledby"),
        legend[0].attr("id"),
        "{}",
        dialog.html()
    );
    let form = select_in(dialog, "form[hx-post]")[0];
    assert_eq!(form.attr("hx-post"), Some("/suites"));
    assert_eq!(form.attr("action"), Some("/suites"));
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
        new_suite_form(&Html::parse_fragment(&form.inner_html())),
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
async fn making_a_suite_answers_the_form_as_it_starts_closing_the_dialog_and_the_stream_then_sends_it()
 {
    let pages = Pages::of(view(vec![], vec![])).await;
    let mut events = stream(&pages, "/events/suites").await;
    events.recv().await.unwrap();

    let (status, headers, answer) = post_answer(
        &pages,
        "/suites",
        &[SAME_ORIGIN],
        &[("name", "board"), ("stage", "design")],
    )
    .await;

    assert_eq!(status, 200, "{}", answer.html());
    assert_eq!(headers.get("hx-trigger").unwrap(), "made");
    assert_eq!(
        new_suite_form(&answer),
        (
            vec![("name", String::new()), ("stage", "triage".into())],
            None
        )
    );
    assert_eq!(text(select(&answer, "legend")[0]), "New suite");
    let suites = loop {
        let event = events.recv().await.unwrap();
        if let Some((_, suites)) = regions(&event).into_iter().find(|(id, _)| id == "suites") {
            break suites;
        }
    };
    assert_eq!(rows(&suites, "#suites"), [["board", "design", ""]]);
}

/// `docs/specs/benchmarks.md`, New suite, Refusals; `docs/specs/design_system.md`,
/// Making, A refusal.
#[test_log::test(tokio::test)]
async fn a_refused_suite_answers_the_form_as_sent_with_the_reason() {
    let pages = with_suites().await;

    let (taken, taken_headers, taken_form) = post_answer(
        &pages,
        "/suites",
        &[SAME_ORIGIN],
        &[("name", "board"), ("stage", "design")],
    )
    .await;
    let (malformed, _, malformed_form) = post_answer(
        &pages,
        "/suites",
        &[SAME_ORIGIN],
        &[("name", "Board 2"), ("stage", "implement")],
    )
    .await;
    let (no_stage, _, no_stage_form) = post_answer(
        &pages,
        "/suites",
        &[SAME_ORIGIN],
        &[("name", "fresh"), ("stage", "rework")],
    )
    .await;
    let (cross_site, _, cross_site_form) = post_answer(
        &pages,
        "/suites",
        &[CROSS_SITE],
        &[("name", "fresh"), ("stage", "design")],
    )
    .await;

    assert_eq!(taken, 422);
    assert!(taken_headers.get("hx-trigger").is_none());
    assert_eq!(
        new_suite_form(&taken_form),
        (
            vec![("name", "board".into()), ("stage", "design".into())],
            Some("A suite named board already exists.".into())
        )
    );
    assert_eq!(malformed, 422);
    assert_eq!(
        new_suite_form(&malformed_form),
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
        new_suite_form(&no_stage_form),
        (
            vec![("name", "fresh".into()), ("stage", "triage".into())],
            Some("\"rework\" is not a stage of a suite: one of triage, design, implement".into())
        )
    );
    assert_eq!(cross_site, 403);
    assert_eq!(
        new_suite_form(&cross_site_form).0,
        [("name", "fresh".into()), ("stage", "design".into())]
    );
    let html = pages.page("/suites").await;
    assert_eq!(
        rows(&html, "#suites"),
        [["board", "implement", ""], ["triage-basics", "triage", ""]]
    );
}

/// `docs/specs/benchmarks.md`, Suite, Layout, Adding a case and Launching.
#[test_log::test(tokio::test)]
async fn the_suite_page_opens_adding_a_case_and_running_from_under_its_heading_above_its_cases() {
    let pages = with_suites().await;

    let html = pages.page("/suites/board").await;

    assert_eq!(
        disclosures(&html, &["capture", "launch"]),
        [
            ("Add case".to_string(), false, "Add case".to_string()),
            ("Run".to_string(), false, "Run experiment".to_string()),
        ]
    );
    let order: Vec<&str> = select(&html, "main h1, main details:has(> form), main section")
        .into_iter()
        .filter_map(|element| match element.value().name() {
            "h1" => Some("heading"),
            "details" => Some("disclosure"),
            _ => element.attr("id"),
        })
        .collect();
    assert_eq!(
        order,
        [
            "heading",
            "disclosure",
            "disclosure",
            "cases",
            "experiments"
        ]
    );
}

#[test_log::test(tokio::test)]
async fn a_case_named_as_another_cases_question_has_a_confirmation_of_its_own() {
    let pages = with_suites().await;
    keep_case(
        &pages.path(""),
        "board",
        "lanes-question",
        "1/implement-1",
        "Asks.\n",
    )
    .await;

    let html = pages.page("/suites/board").await;

    let questions: Vec<String> = confirmations(&html)
        .into_iter()
        .map(|confirmation| confirmation.question)
        .collect();
    assert_eq!(
        questions,
        [
            "Remove the case columns and its directory?",
            "Remove the case lanes and its directory?",
            "Remove the case lanes-question and its directory?",
        ]
    );
}

#[test_log::test(tokio::test)]
async fn every_write_from_another_site_is_refused_and_writes_nothing() {
    let pages = with_suites().await;
    let writes: [(&str, &[(&str, &str)]); 3] = [
        ("/suites", &[("name", "fresh"), ("stage", "design")]),
        (
            "/suites/board",
            &[
                ("run", "1/implement-1"),
                ("name", "more"),
                ("criteria", "Ships.\n"),
            ],
        ),
        ("/suites/board/remove-case", &[("case", "columns")]),
    ];

    for (path, fields) in writes {
        let (status, _) = post(&pages, path, &[CROSS_SITE], fields).await;
        assert_eq!(status, 403, "{path}");
    }

    let html = pages.page("/suites").await;
    assert_eq!(
        rows(&html, "#suites"),
        [["board", "implement", ""], ["triage-basics", "triage", ""]]
    );
    assert_eq!(cases(&pages), ["columns", "lanes"]);
}

/// `docs/specs/design_system.md`, Screen states, Nothing there.
#[test_log::test(tokio::test)]
async fn the_page_of_no_suite_says_there_is_none() {
    let pages = with_suites().await;

    let (no_suite, no_suite_body) = pages.get("/suites/nothing").await;
    let (no_name, no_name_body) = pages.get("/suites/Not%20a%20name").await;

    for (status, body, shown) in [
        (no_suite, no_suite_body, "No such suite nothing"),
        (no_name, no_name_body, "No such suite Not a name"),
    ] {
        assert_eq!(status, 404);
        let html = Html::parse_document(&body);
        assert_eq!(text(select(&html, "main #cases")[0]), shown);
    }
}

#[test_log::test(tokio::test)]
async fn a_suite_that_does_not_exist_answers_404() {
    let pages = with_suites().await;

    let (page, _) = pages.get("/suites/nothing").await;
    let (capture, _) = post(
        &pages,
        "/suites/nothing",
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
        "/suites/nothing/remove-case",
        &[SAME_ORIGIN],
        &[("case", "columns")],
    )
    .await;

    assert_eq!([page, capture, removal], [404, 404, 404]);
    assert_eq!(cases(&pages), ["columns", "lanes"]);
}

#[test_log::test(tokio::test)]
async fn the_suite_page_shows_each_case_with_its_run_and_the_first_line_of_its_criteria() {
    let pages = with_suites().await;

    let html = pages.page("/suites/board").await;

    assert_eq!(text(select(&html, "h1")[0]), "Suite board");
    assert_eq!(events_path(&html), Some("/events/suites/board"));
    let cases = select(&html, "#cases tbody tr:not([data-fold-of])");
    assert_eq!(cases.len(), 2);
    let names: Vec<String> = cases
        .iter()
        .map(|row| text(select_in(*row, "th")[0]))
        .collect();
    assert_eq!(names, ["columns", "lanes"]);
    assert_eq!(
        hrefs(select(&html, "#cases tbody a")),
        ["/runs/1/implement-1", "/runs/1/implement-2"]
    );
    assert_eq!(text(select_in(cases[0], "td")[1]), "Names the columns.");
    let criteria = select(&html, "#cases tbody tr[data-fold-of]");
    assert_eq!(criteria.len(), 2);
    assert_eq!(
        select_in(criteria[0], "pre")[0].text().collect::<String>(),
        "Names the columns.\nAnd orders them.\n"
    );
}

/// `docs/specs/benchmarks.md`, Suite, Cases: a case's row opens on click to
/// the whole of its criteria, as a fold row whose ids are keyed by names.
#[test_log::test(tokio::test)]
async fn a_cases_row_is_a_fold_row_opening_to_its_criteria() {
    let pages = with_suites().await;

    let html = pages.page("/suites/board").await;

    let rows: Vec<(Option<&str>, Option<&str>, bool)> = select(&html, "#cases tbody tr")
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
            (Some("case-columns"), None, false),
            (Some("case-columns_criteria"), Some("case-columns"), true),
            (Some("case-lanes"), None, false),
            (Some("case-lanes_criteria"), Some("case-lanes"), true),
        ]
    );
    let buttons = select(&html, "#case-columns > th:first-child > button");
    assert_eq!(buttons.len(), 1, "{}", html.html());
    assert_eq!(buttons[0].attr("type"), Some("button"));
    assert_eq!(buttons[0].attr("aria-expanded"), Some("false"));
    assert_eq!(
        buttons[0].attr("aria-controls"),
        Some("case-columns_criteria")
    );
    assert_eq!(text(buttons[0]), "columns");
    assert_eq!(select(&html, "#cases details").len(), 0);
}

/// `docs/specs/benchmarks.md`, Suite, Cases, and Remove case;
/// `docs/specs/design_system.md`, Confirming.
#[test_log::test(tokio::test)]
async fn each_case_is_removed_after_a_confirmation_naming_it() {
    let pages = with_suites().await;

    let html = pages.page("/suites/board").await;

    let removal = |case: &str| Confirmation {
        asking: "Remove".into(),
        question: format!("Remove the case {case} and its directory?"),
        turn_down: "Cancel".into(),
        action: "/suites/board/remove-case".into(),
        fields: vec![("case".into(), case.into())],
    };
    assert_eq!(confirmations(&html), [removal("columns"), removal("lanes")]);
    assert!(select(&html, "[hx-confirm]").is_empty(), "{}", html.html());
}

/// `docs/specs/design_system.md`, Screen states, Empty.
#[test_log::test(tokio::test)]
async fn a_suite_with_no_cases_experiments_or_runs_of_its_stage_says_so() {
    let pages = with_suites().await;

    let html = pages.page("/suites/triage-basics").await;

    assert_eq!(text(select(&html, "#cases p")[0]), "No cases");
    assert_eq!(text(select(&html, "#experiments p")[0]), "No experiments");
    assert_eq!(
        text(select(&html, "#capture-runs p")[0]),
        "No runs of triage"
    );
}

#[test_log::test(tokio::test)]
async fn removing_a_case_removes_it_from_the_page_and_from_the_cases() {
    let pages = with_suites().await;

    let (status, answer) = post(
        &pages,
        "/suites/board/remove-case",
        &[SAME_ORIGIN],
        &[("case", "columns")],
    )
    .await;

    assert_eq!(status, 200, "{}", said(&answer));
    assert_eq!(said(&answer), "Removed the case columns");
    assert_eq!(cases(&pages), ["lanes"]);
    let html = pages.page("/suites/board").await;
    let names: Vec<String> = select(&html, "#cases tbody th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(names, ["lanes"]);
}

#[test_log::test(tokio::test)]
async fn removing_a_case_the_suite_does_not_hold_is_refused() {
    let pages = with_suites().await;

    let (status, answer) = post(
        &pages,
        "/suites/triage-basics/remove-case",
        &[SAME_ORIGIN],
        &[("case", "columns")],
    )
    .await;

    assert_eq!(status, 404);
    assert_eq!(
        said(&answer),
        "Not removed: the suite triage-basics has no case columns"
    );
    assert_eq!(cases(&pages), ["columns", "lanes"]);
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

/// [`with_suites`]'s pages, with #1's runs: its design run 1, and its
/// implement runs 1 and 2, the design run and the last implement run
/// having recorded their whole input.
#[track_caller]
fn with_runs() -> impl Future<Output = Pages> {
    let suites = Box::pin(with_suites());
    async move {
        let pages = suites.await;
        let head = committed(&pages).await;
        record_run(&pages, "design", 1, "2026-10-13T09:00:00Z", Some(&head)).await;
        record_run(&pages, "implement", 1, "2026-10-13T10:00:00Z", None).await;
        record_run(&pages, "implement", 2, "2026-10-13T11:00:00Z", Some(&head)).await;
        pages
    }
}

#[test_log::test(tokio::test)]
async fn the_suite_page_offers_to_capture_a_run_of_its_stage_that_can_be_captured_picked_from_a_list()
 {
    let pages = with_runs().await;

    let html = pages.page("/suites/board").await;

    let form = select(&html, "form.capture");
    assert_eq!(form.len(), 1);
    assert_eq!(form[0].attr("hx-post"), Some("/suites/board"));
    let runs: Vec<(Option<&str>, Option<&str>)> = select_in(form[0], "select[name=run] option")
        .into_iter()
        .map(|option| (option.attr("value"), option.attr("data-name")))
        .collect();
    assert_eq!(runs, [(Some("1/implement-2"), Some("1-implement-2"))]);
    let name = select_in(form[0], "input[name=name]");
    assert_eq!(name[0].attr("value"), Some("1-implement-2"));
    assert_eq!(name[0].attr("pattern"), Some(NAME_PATTERN));
    assert_eq!(select_in(form[0], "select[name=suite]").len(), 0);
    assert_eq!(select_in(form[0], "textarea[name=criteria]").len(), 1);
}

#[test_log::test(tokio::test)]
async fn adding_a_case_captures_the_run_into_the_suite_and_the_page_then_shows_it_last() {
    let pages = with_runs().await;

    let (status, answer) = post(
        &pages,
        "/suites/board",
        &[SAME_ORIGIN],
        &[
            ("run", "1/implement-2"),
            ("name", "gadgets"),
            ("criteria", "Ships gh.\n"),
        ],
    )
    .await;

    assert_eq!(status, 200, "{}", said(&answer));
    assert_eq!(
        said(&answer),
        format!(
            "Captured 1/implement-2 as the case `{}` in the suite board",
            pages.path("cases/gadgets").display()
        )
    );
    assert_eq!(cases(&pages), ["columns", "gadgets", "lanes"]);
    let html = pages.page("/suites/board").await;
    let names: Vec<String> = select(&html, "#cases tbody th")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(names, ["columns", "lanes", "gadgets"]);
}

#[test_log::test(tokio::test)]
async fn adding_a_run_of_another_stage_is_refused_and_writes_nothing() {
    let pages = with_runs().await;

    let (status, answer) = post(
        &pages,
        "/suites/board",
        &[SAME_ORIGIN],
        &[
            ("run", "1/design-1"),
            ("name", "gadgets"),
            ("criteria", "Ships gh.\n"),
        ],
    )
    .await;

    assert_eq!(status, 422);
    assert_eq!(
        said(&answer),
        "Not captured: the suite board holds cases of implement, and the run 1/design-1 is of \
         design"
    );
    assert_eq!(cases(&pages), ["columns", "lanes"]);
}

#[test_log::test(tokio::test)]
async fn the_suite_pages_stream_sends_its_cases_as_they_change() {
    let pages = with_suites().await;
    let mut events = stream(&pages, "/events/suites/board").await;
    events.recv().await.unwrap();

    post(
        &pages,
        "/suites/board/remove-case",
        &[SAME_ORIGIN],
        &[("case", "lanes")],
    )
    .await;

    let cases = loop {
        let event = events.recv().await.unwrap();
        if let Some((_, cases)) = regions(&event).into_iter().find(|(id, _)| id == "cases") {
            break cases;
        }
    };
    let names: Vec<String> = select(&cases, "tbody th").into_iter().map(text).collect();
    assert_eq!(names, ["columns"]);
}

#[test_log::test(tokio::test)]
async fn the_suite_pages_stream_sends_the_runs_to_capture_as_they_change() {
    let pages = with_runs().await;
    let mut events = stream(&pages, "/events/suites/board").await;
    events.recv().await.unwrap();
    let head = committed(&pages).await;

    record_run(&pages, "implement", 3, "2026-10-13T12:00:00Z", Some(&head)).await;

    let runs = loop {
        let event = events.recv().await.unwrap();
        if let Some((_, runs)) = regions(&event)
            .into_iter()
            .find(|(id, _)| id == "capture-runs")
        {
            break runs;
        }
    };
    let offered: Vec<Option<&str>> = select(&runs, "select[name=run] option")
        .into_iter()
        .map(|option| option.attr("value"))
        .collect();
    assert_eq!(offered, [Some("1/implement-3"), Some("1/implement-2")]);
}
