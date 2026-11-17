use super::*;

use std::panic::Location;

use serde_json::json;

/// The rows of the table on `html`, each its cells' text.
fn rows(html: &Html, table: &str) -> Vec<Vec<String>> {
    select(html, &format!("{table} tbody tr"))
        .into_iter()
        .map(|row| select_in(row, "th, td").into_iter().map(text).collect())
        .collect()
}

/// The pages of #1, `Ship gh`, with the implement suite `board`, holding
/// the cases `columns`, captured from `1/implement-1` and of two lines of
/// criteria, and `lanes`, and the empty triage suite `triage-basics`.
async fn with_suites() -> Pages {
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

#[test_log::test(tokio::test)]
async fn the_suites_page_shows_a_row_per_suite_linking_to_its_page() {
    let pages = with_suites().await;

    let html = pages.page("/suites").await;

    assert_eq!(
        rows(&html, "#suites"),
        [
            ["board", "implement", "2", ""],
            ["triage-basics", "triage", "0", ""],
        ]
    );
    assert_eq!(
        hrefs(select(&html, "#suites tbody a")),
        ["/suites/board", "/suites/triage-basics"]
    );
    assert_eq!(events_path(&html), Some("/events/suites"));
}

#[test_log::test(tokio::test)]
async fn the_suites_page_with_no_suites_says_so_and_offers_to_make_one() {
    let pages = Pages::of(view(vec![], vec![])).await;

    let html = pages.page("/suites").await;

    assert_eq!(text(select(&html, "#suites p")[0]), "no suites");
    let form = select(&html, "form.make-suite");
    assert_eq!(form[0].attr("hx-post"), Some("/suites"));
    let stages: Vec<&str> = select_in(form[0], "select[name=stage] option")
        .into_iter()
        .filter_map(|option| option.attr("value"))
        .collect();
    assert_eq!(stages, ["triage", "design", "implement"]);
}

#[test_log::test(tokio::test)]
async fn making_a_suite_says_so_and_the_page_then_lists_it() {
    let pages = Pages::of(view(vec![], vec![])).await;

    let (status, answer) = post(
        &pages,
        "/suites",
        &[SAME_ORIGIN],
        &[("name", "board"), ("stage", "design")],
    )
    .await;

    assert_eq!(status, 200, "{}", said(&answer));
    assert_eq!(said(&answer), "Made the design suite board");
    let html = pages.page("/suites").await;
    assert_eq!(rows(&html, "#suites"), [["board", "design", "0", ""]]);
}

#[test_log::test(tokio::test)]
async fn a_suites_name_that_is_taken_is_refused() {
    let pages = with_suites().await;

    let (status, answer) = post(
        &pages,
        "/suites",
        &[SAME_ORIGIN],
        &[("name", "board"), ("stage", "design")],
    )
    .await;

    assert_eq!(status, 422);
    assert_eq!(said(&answer), "Not made: a suite is already named board");
    let html = pages.page("/suites").await;
    assert_eq!(rows(&html, "#suites")[0], ["board", "implement", "2", ""]);
}

#[test_log::test(tokio::test)]
async fn a_suite_whose_name_or_stage_does_not_parse_is_refused() {
    let pages = Pages::of(view(vec![], vec![])).await;

    let (name_status, name) = post(
        &pages,
        "/suites",
        &[SAME_ORIGIN],
        &[("name", "Board"), ("stage", "design")],
    )
    .await;
    let (stage_status, stage) = post(
        &pages,
        "/suites",
        &[SAME_ORIGIN],
        &[("name", "board"), ("stage", "rework")],
    )
    .await;

    assert_eq!(name_status, 422);
    assert_eq!(
        said(&name),
        "Not made: \"Board\" is not a suite's name: it is lowercase letters, digits and hyphens"
    );
    assert_eq!(stage_status, 422);
    assert_eq!(
        said(&stage),
        "Not made: \"rework\" is not a stage of a suite: one of triage, design, implement"
    );
    let html = pages.page("/suites").await;
    assert_eq!(rows(&html, "#suites"), Vec::<Vec<String>>::new());
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
        [
            ["board", "implement", "2", ""],
            ["triage-basics", "triage", "0", ""],
        ]
    );
    assert_eq!(cases(&pages), ["columns", "lanes"]);
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
    let cases = select(&html, "#cases tbody tr");
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
    let criteria = select_in(cases[0], "details.criteria")[0];
    assert_eq!(
        text(select_in(criteria, "summary")[0]),
        "Names the columns."
    );
    assert_eq!(
        select_in(criteria, "pre")[0].text().collect::<String>(),
        "Names the columns.\nAnd orders them.\n"
    );
    let removes: Vec<Option<&str>> = select(&html, "#cases form.remove-case input[name=case]")
        .into_iter()
        .map(|input| input.attr("value"))
        .collect();
    assert_eq!(removes, [Some("columns"), Some("lanes")]);
}

#[test_log::test(tokio::test)]
async fn a_suite_with_no_cases_says_so() {
    let pages = with_suites().await;

    let html = pages.page("/suites/triage-basics").await;

    assert_eq!(text(select(&html, "#cases p")[0]), "no cases");
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
    assert_eq!(select_in(form[0], "select[name=suite]").len(), 0);
    assert_eq!(select_in(form[0], "textarea[name=criteria]").len(), 1);
}

#[test_log::test(tokio::test)]
async fn adding_a_case_captures_the_run_into_the_suite_and_the_page_then_shows_it() {
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
    assert_eq!(names, ["columns", "gadgets", "lanes"]);
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
