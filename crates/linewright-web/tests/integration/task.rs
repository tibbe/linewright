//! Task (`docs/specs/benchmarks.md`, Task and Remove task).

use super::benchmarks::{edit_dialog, edit_form, open_tabs, saved_regions, with_benchmarks};
use super::shell::crumbs;
use super::*;

/// #1130 as GitHub gave it.
const ISSUE_1130: &str = linewright_core::store::fixtures::REST_ISSUE_1130;

/// [`with_benchmarks`]'s pages, with the task `gadgets`, 3, of `board`,
/// captured from #1130's run `1130/implement-1`, which started at 2026-10-13
/// 10:00 UTC with the issue as GitHub gave it.
pub(super) async fn with_a_task() -> Pages {
    let pages = Box::pin(with_benchmarks()).await;
    let work_dir = pages.path("");
    let id = keep_task(
        &work_dir,
        "board",
        "gadgets",
        "1130/implement-1",
        "Ships the gadgets.\n\nAnd tests them.\n",
    )
    .await;
    let issue: serde_json::Value =
        serde_json::from_str(ISSUE_1130).unwrap_or_else(|e| panic!("#1130's fixture: {e}"));
    keep_task_facts(
        &work_dir,
        id,
        "1130/implement-1",
        "2026-10-13T10:00:00Z",
        &issue,
    );
    pages
}

/// Removes the task `id` of `pages` through the back door, as a removal
/// leaves it: its row in the removed tasks, and its directory gone.
async fn removed(pages: &Pages, id: i64) {
    let work_dir = pages.path("");
    let pool = back_door(&work_dir).await;
    for statement in [
        "INSERT INTO removed_tasks (id, benchmark, name) SELECT id, benchmark, name FROM tasks WHERE id = ?",
        "DELETE FROM tasks WHERE id = ?",
    ] {
        sqlx::query(statement)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("removing the task {id}: {e}"));
    }
    pool.close().await;
    std::fs::remove_dir_all(work_dir.join("tasks").join(id.to_string()))
        .unwrap_or_else(|e| panic!("removing the task {id}'s directory: {e}"));
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
                "Spec change: Show no working line between turns #1130".into()
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
    let item = select(&html, "#task .stats .stat")[1];
    assert_title_then_number_gap_1_apart(item);
    assert_no_words_loose_in_a_flex(&html, "#task");
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
/// `docs/specs/design_system.md`, Figures, Absent.
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

/// `docs/specs/benchmarks.md`, Task, Heading, Edit task and Remove task;
/// `docs/specs/components.md`, Confirmation.
#[test_log::test(tokio::test)]
async fn remove_is_with_the_screens_buttons_and_asks_first_naming_the_task() {
    let pages = with_benchmarks().await;

    let html = pages.page("/tasks/2").await;

    let buttons: Vec<(String, Option<&str>)> = select(&html, "#buttons button")
        .into_iter()
        .map(|button| (text(button), button.attr("commandfor")))
        .collect();
    assert_eq!(
        buttons,
        [
            ("Edit".to_string(), Some("edit-task")),
            ("Remove".to_string(), Some("remove-task"))
        ]
    );
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

/// `docs/specs/design_system.md`, Actions, What was removed;
/// `docs/specs/benchmarks.md`, Task, After "Remove";
/// `docs/specs/components.md`, Success message.
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

/// `docs/specs/shell.md`, No such thing.
#[test_log::test(tokio::test)]
async fn a_task_that_does_not_exist_or_was_removed_is_no_such_thing() {
    let pages = with_benchmarks().await;
    removed(&pages, 1).await;

    for address in ["/tasks/1", "/tasks/9", "/tasks/lanes"] {
        let (status, body) = pages.get(address).await;
        super::shell::assert_not_found(status, &body, address);
    }
}

/// Task's stream sends the shell's regions, then its facts, its criteria and
/// the question of its removal, which names it.
#[test_log::test(tokio::test)]
async fn tasks_stream_sends_the_shells_regions_and_its_own() {
    let pages = with_a_task().await;
    let mut events = stream(&pages, "/events/tasks/3").await;

    let first = regions(&events.recv().await.unwrap());

    let ids: Vec<&str> = first.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "holds",
            "title",
            "crumbs",
            "heading",
            "buttons",
            "task",
            "criteria",
            "remove-task_question",
            "edit-task-values"
        ]
    );
    let html = pages.page("/tasks/3").await;
    for id in ids {
        assert_eq!(select(&html, &format!("[id='{id}']")).len(), 1, "{id}");
    }
}

/// `docs/specs/benchmarks.md`, Edit task; `docs/specs/components.md`, Edit
/// dialog.
#[test_log::test(tokio::test)]
async fn edit_opens_the_edit_task_dialog_holding_its_name_and_criteria() {
    let pages = with_a_task().await;

    let html = pages.page("/tasks/3").await;

    let form = edit_form(
        edit_dialog(&html),
        "Edit task",
        &["Name", "Criteria"],
        "/tasks/3/edit",
    );
    assert_eq!(
        making_shown(&form),
        MakingShown {
            values: vec![
                ("name", "gadgets".into()),
                ("criteria", "Ships the gadgets.\n\nAnd tests them.\n".into())
            ],
            refused: vec![],
            whole: None,
        }
    );
}

/// `docs/specs/components.md`, Form dialog, Done: the dialog closes holding
/// what was saved, the criteria with the line breaks the browser sends as
/// they were typed, and Task shows the new name and criteria, and opens
/// Edit task holding them.
#[test_log::test(tokio::test)]
async fn saving_a_tasks_name_and_criteria_closes_the_dialog_and_task_shows_them() {
    let pages = with_a_task().await;
    let mut events = stream(&pages, "/events/tasks/3").await;
    events.recv().await.unwrap();

    let (status, headers, answer) = post_answer(
        &pages,
        "/tasks/3/edit",
        &[SAME_ORIGIN],
        &[
            ("name", " Gadgets, shipped "),
            ("criteria", "Ships them.\r\nAnd tests them.\r\n"),
        ],
    )
    .await;

    assert_eq!(status, 200, "{}", answer.html());
    assert_eq!(headers.get("hx-trigger").unwrap(), "made");
    assert_eq!(
        making_shown(&answer),
        MakingShown {
            values: vec![
                ("name", "Gadgets, shipped".into()),
                ("criteria", "Ships them.\nAnd tests them.\n".into())
            ],
            refused: vec![],
            whole: None,
        }
    );
    let changed = saved_regions(&mut events, "heading", ".card-title", "Gadgets, shipped").await;
    assert_eq!(
        text(select(region_of(&changed, "remove-task_question"), "p")[0]),
        "Remove the task Gadgets, shipped and its directory?"
    );
    assert_eq!(
        select(region_of(&changed, "criteria"), "pre")[0]
            .text()
            .collect::<String>(),
        "Ships them.\nAnd tests them.\n"
    );
    let values = select(region_of(&changed, "edit-task-values"), "div")[0];
    assert_eq!(
        (values.attr("data-name"), values.attr("data-criteria")),
        (
            Some("Gadgets, shipped"),
            Some("Ships them.\nAnd tests them.\n")
        )
    );
    let html = pages.page("/tasks/3").await;
    assert_eq!(
        select(&html, "dialog[id='edit-task'] form")[0].attr("data-values"),
        Some("edit-task-values")
    );
    assert_eq!(select(&html, "[id='edit-task-values']").len(), 1);
}

/// `docs/specs/benchmarks.md`, Edit task, Refusals;
/// `docs/specs/components.md`, Form dialog, Required and A refusal, and Edit
/// dialog, A thing that is gone.
#[test_log::test(tokio::test)]
async fn a_refused_task_change_keeps_what_the_reader_typed_with_its_reason() {
    let pages = with_benchmarks().await;
    removed(&pages, 2).await;
    let lanes = keep_task(
        &pages.path(""),
        "board",
        "lanes",
        "1/implement-2",
        "Stacks.\n",
    )
    .await;
    assert_eq!(lanes, 3);

    for (path, name, criteria, status, refused, whole) in [
        (
            "/tasks/1/edit",
            " ",
            "Ships.",
            422,
            vec![("name", "Required.")],
            None,
        ),
        (
            "/tasks/1/edit",
            "columns",
            " \n",
            422,
            vec![("criteria", "Required.")],
            None,
        ),
        (
            "/tasks/1/edit",
            "lanes",
            "Ships.",
            422,
            vec![("name", "A task named lanes already exists in board.")],
            None,
        ),
        (
            "/tasks/2/edit",
            "lanes",
            "Ships.",
            404,
            vec![],
            Some("No such task 2"),
        ),
        (
            "/tasks/9/edit",
            "more",
            "Ships.",
            404,
            vec![],
            Some("No such task 9"),
        ),
    ] {
        let (answered, headers, form) = post_answer(
            &pages,
            path,
            &[SAME_ORIGIN],
            &[("name", name), ("criteria", criteria)],
        )
        .await;

        assert_eq!(answered, status, "{path} {name:?}");
        assert!(headers.get("hx-trigger").is_none());
        assert_eq!(
            making_shown(&form),
            MakingShown {
                values: vec![
                    ("name", name.to_string()),
                    ("criteria", criteria.to_string())
                ],
                refused: refused
                    .iter()
                    .map(|(field, reason)| (*field, (*reason).to_string()))
                    .collect(),
                whole: whole.map(String::from),
            },
            "{path} {name:?}"
        );
        assert_eq!(text(select(&form, "legend")[0]), "Edit task");
    }
    let (cross_site, _) = post(
        &pages,
        "/tasks/1/edit",
        &[CROSS_SITE],
        &[("name", "fresh"), ("criteria", "Ships.")],
    )
    .await;
    assert_eq!(cross_site, 403);
    let html = pages.page("/tasks/1").await;
    assert_eq!(text(select(&html, "#heading .card-title")[0]), "columns");
    assert_eq!(
        select(&html, "#criteria pre")[0].text().collect::<String>(),
        "Names the columns.\nAnd orders them.\n"
    );
}

/// Edit task holds criteria that start with a line break whole, which a
/// browser's parser would drop from a textarea that starts with it.
/// <https://html.spec.whatwg.org/multipage/parsing.html#parsing-main-inbody>
#[test_log::test(tokio::test)]
async fn edit_task_holds_criteria_that_start_with_a_line_break_whole() {
    let pages = with_benchmarks().await;
    keep_task(
        &pages.path(""),
        "board",
        "spaced",
        "1/implement-3",
        "\nStarts low.\n",
    )
    .await;

    let html = pages.page("/tasks/3").await;

    assert_eq!(
        select(&html, "dialog[id='edit-task'] textarea[name=criteria]")[0]
            .text()
            .collect::<String>(),
        "\nStarts low.\n"
    );
}
