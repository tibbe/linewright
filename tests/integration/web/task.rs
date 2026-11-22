//! Task (`docs/specs/benchmarks.md`, Task and Remove task;
//! `docs/specs/web/benchmarks.md`, Task).

use super::benchmarks::{open_tabs, with_benchmarks};
use super::shell::crumbs;
use super::*;

use std::panic::Location;

/// #1130 as GitHub gave it.
const ISSUE_1130: &str = include_str!("../../../src/store/fixtures/rest-issue-1130.json");

/// [`with_benchmarks`]'s pages, with the task `gadgets`, 3, of `board`,
/// captured from #1130's run `1130/implement-1`, which started at 2026-10-13
/// 10:00 UTC with the issue as GitHub gave it.
#[track_caller]
fn with_a_task() -> impl Future<Output = Pages> {
    let caller = Location::caller();
    let pages = Box::pin(with_benchmarks());
    async move {
        let pages = pages.await;
        let work_dir = pages.path("");
        let id = keep_task(
            &work_dir,
            "board",
            "gadgets",
            "1130/implement-1",
            "Ships the gadgets.\n\nAnd tests them.\n",
        )
        .await;
        let issue: serde_json::Value = serde_json::from_str(ISSUE_1130)
            .unwrap_or_else(|e| panic!("#1130's fixture at {caller}: {e}"));
        keep_task_facts(
            &work_dir,
            id,
            "1130/implement-1",
            "2026-10-13T10:00:00Z",
            &issue,
        );
        pages
    }
}

/// Removes the task `id` of `pages` through the back door, as a removal
/// leaves it: its row in the removed tasks, and its directory gone.
#[track_caller]
fn removed(pages: &Pages, id: i64) -> impl Future<Output = ()> + use<> {
    let caller = Location::caller();
    let work_dir = pages.path("");
    async move {
        let pool = back_door(&work_dir).await;
        for statement in [
            "INSERT INTO removed_tasks (id, benchmark, name) SELECT id, benchmark, name FROM tasks WHERE id = ?",
            "DELETE FROM tasks WHERE id = ?",
        ] {
            sqlx::query(statement)
                .bind(id)
                .execute(&pool)
                .await
                .unwrap_or_else(|e| panic!("removing the task {id} at {caller}: {e}"));
        }
        pool.close().await;
        std::fs::remove_dir_all(work_dir.join("tasks").join(id.to_string()))
            .unwrap_or_else(|e| panic!("removing the task {id}'s directory at {caller}: {e}"));
    }
}

/// `docs/specs/benchmarks.md`, Task: the heading, the facts and the criteria;
/// `docs/specs/shell.md`, Breadcrumbs.
#[test_log::test(tokio::test)]
async fn a_task_shows_its_name_and_stage_its_facts_and_the_whole_of_its_criteria() {
    let pages = with_a_task().await;

    let html = pages.page("/tasks/3").await;

    let card = select(&html, "#heading")[0];
    assert_eq!(text(select_in(card, ".card-title")[0]), "gadgets");
    assert_eq!(text(select_in(card, ".line")[0]), "implement");
    assert_eq!(text(select(&html, "title")[0]), "gadgets · Linewright");
    assert_eq!(
        crumbs(&html),
        [
            ("Benchmarks".to_string(), Some("/benchmarks".to_string())),
            ("board".into(), Some("/benchmarks/1".into())),
            ("Task".into(), None),
        ]
    );
    assert_eq!(
        stats(&html, "#task"),
        [
            ("Captured from".to_string(), "1130/implement-1".to_string()),
            (
                "Item".into(),
                "#1130: Spec change: Show no working line between turns".into()
            ),
            ("Started".into(), "2026-10-13 10:00:00 UTC".into()),
        ]
    );
    assert_eq!(
        hrefs(select(&html, "#task .stats a")),
        [
            "/runs/1130/implement-1",
            "/items/1130",
            "https://github.com/owner/widgets/issues/1130"
        ]
    );
    let criteria = select(&html, "#criteria")[0];
    assert_eq!(
        text(select_in(criteria, ".card-title")[0]),
        "Criteria",
        "{}",
        criteria.html()
    );
    assert_eq!(
        select_in(criteria, ".card-title svg")[0].attr("data-icon"),
        Some("clipboard-document-check")
    );
    assert_eq!(
        select_in(criteria, "pre")[0].text().collect::<String>(),
        "Ships the gadgets.\n\nAnd tests them.\n"
    );
    assert_eq!(events_path(&html), Some("/events/tasks/3"));
}

/// `docs/specs/benchmarks.md`, Task: a task whose directory lacks its facts
/// still shows what its row gives, and why the rest does not read;
/// `docs/specs/design_system.md`, Screen states, Absent.
#[test_log::test(tokio::test)]
async fn a_task_whose_facts_do_not_read_shows_them_absent_and_says_why() {
    let pages = with_benchmarks().await;

    let html = pages.page("/tasks/1").await;

    assert_eq!(
        stats(&html, "#task"),
        [
            ("Captured from".to_string(), "1/implement-1".to_string()),
            ("Item".into(), "–".into()),
            ("Started".into(), "–".into()),
        ]
    );
    assert!(
        text(select(&html, "#task")[0]).contains("Item and Started do not read"),
        "{}",
        html.html()
    );
    assert_eq!(
        select(&html, "#criteria pre")[0].text().collect::<String>(),
        "Names the columns.\nAnd orders them.\n"
    );
}

/// `docs/specs/benchmarks.md`, Task, Heading, and Remove task;
/// `docs/specs/design_system.md`, Confirming.
#[test_log::test(tokio::test)]
async fn remove_is_with_the_screens_buttons_and_asks_first_naming_the_task() {
    let pages = with_benchmarks().await;

    let html = pages.page("/tasks/2").await;

    let buttons: Vec<(String, Option<&str>)> = select(&html, "#buttons button")
        .into_iter()
        .map(|button| (text(button), button.attr("commandfor")))
        .collect();
    assert_eq!(buttons, [("Remove".to_string(), Some("remove-task"))]);
    assert_eq!(
        confirmations(&html),
        [Confirmation {
            asking: "Remove".into(),
            question: "Remove the task lanes and its directory?".into(),
            turn_down: "Cancel".into(),
            action: "/benchmarks/1/remove-task".into(),
            fields: vec![("task".into(), "2".into())],
        }]
    );
}

/// The cookie that has benchmark 1's screen say that task `id` was removed
/// from it, as the removal sets it.
fn removed_cookie(id: &str) -> String {
    format!("removed={id}; Path=/benchmarks/1; SameSite=Lax; HttpOnly")
}

/// `docs/specs/design_system.md`, Confirming, What was removed;
/// `docs/specs/benchmarks.md`, Task, After "Remove";
/// `docs/specs/web/design_system.md`, Making.
#[test_log::test(tokio::test)]
async fn removing_a_task_opens_its_benchmarks_tasks_saying_so_until_the_reader_leaves() {
    let pages = with_benchmarks().await;

    let (status, headers, _) = post_answer(
        &pages,
        "/benchmarks/1/remove-task",
        &[SAME_ORIGIN, ("hx-request", "true")],
        &[("task", "1")],
    )
    .await;

    assert_eq!(status, 200);
    let opened = headers.get("hx-redirect").unwrap().to_str().unwrap();
    assert_eq!(opened, "/benchmarks/1/tasks");
    assert_eq!(headers.get("set-cookie").unwrap(), &removed_cookie("1"));
    assert_eq!(tasks(&pages), ["2"]);
    let (_, answered, html) = answer_with_cookie(&pages, opened, "removed=1").await;
    let said = select(&html, "#made .alert");
    assert_eq!(said.len(), 1, "{}", html.html());
    let mut classes: Vec<&str> = said[0].value().classes().collect();
    classes.sort_unstable();
    assert_eq!(classes, ["alert", "alert-success"]);
    assert_eq!(text(said[0]), "Removed the task columns.");
    assert_eq!(
        open_tabs(&html),
        [("Runs".to_string(), false), ("Tasks".into(), true)]
    );
    let names: Vec<String> = select(&html, "#tasks tbody th > span")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(names, ["lanes"]);
    assert_eq!(
        answered.get("set-cookie").unwrap(),
        "removed=; Path=/benchmarks/1; Max-Age=0; SameSite=Lax; HttpOnly"
    );

    let again = pages.page(opened).await;
    assert_eq!(select(&again, "#made > *").len(), 0, "{}", again.html());
}

/// A browser without htmx follows a redirect to the same screen.
#[test_log::test(tokio::test)]
async fn removing_a_task_from_a_plain_form_redirects_to_its_benchmarks_tasks() {
    let pages = with_benchmarks().await;

    let (status, headers, _) = post_answer(
        &pages,
        "/benchmarks/1/remove-task",
        &[SAME_ORIGIN],
        &[("task", "2")],
    )
    .await;

    assert_eq!(status, 303);
    assert_eq!(headers.get("location").unwrap(), "/benchmarks/1/tasks");
    assert_eq!(headers.get("set-cookie").unwrap(), &removed_cookie("2"));
    assert_eq!(tasks(&pages), ["1"]);
}

/// Tasks says nothing removed of a task that was not removed from its
/// benchmark, nor of a cookie that names no task.
#[test_log::test(tokio::test)]
async fn tasks_says_only_that_a_task_removed_from_its_own_benchmark_was_removed() {
    let pages = with_benchmarks().await;
    let sorting = keep_task(
        &pages.path(""),
        "triage-basics",
        "sorting",
        "1/triage-1",
        "Sorts.\n",
    )
    .await;
    removed(&pages, sorting).await;

    for (path, cookie, shown) in [
        ("/benchmarks/1/tasks", "removed=2", None),
        ("/benchmarks/1/tasks", "removed=9", None),
        ("/benchmarks/1/tasks", "removed=lanes", None),
        ("/benchmarks/1/tasks", "removed=3", None),
        (
            "/benchmarks/2/tasks",
            "removed=3",
            Some("Removed the task sorting."),
        ),
    ] {
        let html = page_with_cookie(&pages, path, cookie).await;
        let said: Vec<String> = select(&html, "#made .alert")
            .into_iter()
            .map(text)
            .collect();
        assert_eq!(
            said,
            Vec::from_iter(shown.map(String::from)),
            "{path}: {cookie}"
        );
    }
}

/// `docs/specs/design_system.md`, Screen states, Nothing there;
/// `docs/specs/shell.md`, Breadcrumbs, No such thing.
#[test_log::test(tokio::test)]
async fn a_task_that_does_not_exist_or_was_removed_says_there_is_none() {
    let pages = with_benchmarks().await;
    removed(&pages, 1).await;

    for (path, shown) in [
        ("/tasks/1", "No such task 1"),
        ("/tasks/9", "No such task 9"),
        ("/tasks/lanes", "No such task lanes"),
    ] {
        let (status, body) = pages.get(path).await;
        assert_eq!(status, 404, "{path}");
        let html = Html::parse_document(&body);
        assert_eq!(text(select(&html, "main #task")[0]), shown, "{path}");
        assert_eq!(
            crumbs(&html),
            [
                ("Benchmarks".to_string(), Some("/benchmarks".to_string())),
                ("Task".into(), None),
            ],
            "{path}"
        );
    }
}

/// Task's stream sends the shell's regions, then its facts and its criteria.
#[test_log::test(tokio::test)]
async fn tasks_stream_sends_the_shells_regions_and_its_own() {
    let pages = with_a_task().await;
    let mut events = stream(&pages, "/events/tasks/3").await;

    let first = regions(&events.recv().await.unwrap());

    let ids: Vec<&str> = first.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "holds", "title", "crumbs", "heading", "buttons", "task", "criteria"
        ]
    );
}
