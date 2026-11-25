//! People (`docs/specs/access.md`, People, Edit person and Disable person).

use super::*;

/// The people signed in to `pages`, in the order they were added: Ada
/// Lovelace, the manager whose browser the tests ask from, is person 1.
/// Vera Viewer is person 2 and Carl Contributor person 3.
async fn three_people(pages: &Pages) -> (AsRole<'_>, AsRole<'_>) {
    let viewer = pages.as_viewer().await;
    let contributor = pages.as_contributor().await;
    (viewer, contributor)
}

/// Each row of People: its cells' text, each piece of it apart, but the
/// buttons'.
fn rows(html: &Html) -> Vec<Vec<String>> {
    select(html, "#people tbody tr")
        .into_iter()
        .map(|row| {
            select_in(row, "th, td")
                .into_iter()
                .filter(|cell| select_in(*cell, "button").is_empty())
                .map(|cell| {
                    cell.text()
                        .map(str::trim)
                        .filter(|piece| !piece.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect()
        })
        .collect()
}

/// Each button on the rows of People: its label, whether it is disabled,
/// and the tip that shows on hover, if any.
fn row_buttons(html: &Html) -> Vec<(String, bool, Option<String>)> {
    select(html, "#people tbody button")
        .into_iter()
        .map(|button| {
            let tip = button
                .parent()
                .and_then(ElementRef::wrap)
                .and_then(|parent| parent.attr("data-tip"))
                .map(str::to_string);
            (text(button), button.attr("disabled").is_some(), tip)
        })
        .collect()
}

/// `docs/specs/access.md`, People (the screen); `docs/specs/shell.md`,
/// Navigation.
#[test_log::test(tokio::test)]
async fn people_lists_everyone_in_the_order_of_their_names() {
    let pages = Pages::of(a_ticket()).await;
    three_people(&pages).await;

    let html = pages.page("/people").await;

    assert_eq!(
        hrefs(select(&html, "nav[aria-label=Linewright] > ul.menu a")),
        [
            "/",
            "/runs",
            "/automations",
            "/benchmarks",
            "/server",
            "/people"
        ]
    );
    assert_eq!(text(select(&html, "title")[0]), "People · Linewright");
    assert_eq!(
        select(&html, "#people thead th")
            .into_iter()
            .map(text)
            .collect::<Vec<_>>(),
        ["Person", "Role", "State", "Signed in", ""]
    );
    let rows = rows(&html);
    assert_eq!(
        rows.iter().map(|row| row[..3].to_vec()).collect::<Vec<_>>(),
        [
            ["A Ada Lovelace ada@example.com", "manager", "enabled"],
            [
                "C Carl Contributor carl@example.com",
                "contributor",
                "enabled"
            ],
            ["V Vera Viewer vera@example.com", "viewer", "enabled"],
        ]
    );
    let logos = select(&html, "#people tbody [data-logo=google]");
    assert_eq!(logos.len(), 3);
    assert_eq!(logos[0].attr("aria-label"), Some("Google"));
    assert!(
        has_classes(select(&html, "#people tbody .badge")[0], &["badge-success"]),
        "enabled shows as done"
    );
    assert_eq!(
        select(&html, "[data-icon=users]").len(),
        1,
        "the navigation's icon of People"
    );
}

/// `docs/specs/access.md`, People (the screen), A row's buttons, and The
/// named manager and the last manager.
#[test_log::test(tokio::test)]
async fn the_last_manager_cannot_be_edited_or_disabled_and_says_why_on_hover() {
    let pages = Pages::of(a_ticket()).await;
    three_people(&pages).await;

    let html = pages.page("/people").await;

    let last = Some("Ada Lovelace is the last manager.".to_string());
    assert_eq!(
        row_buttons(&html),
        [
            ("Edit".into(), true, last.clone()),
            ("Disable".into(), true, last),
            ("Edit".into(), false, None),
            ("Disable".into(), false, None),
            ("Edit".into(), false, None),
            ("Disable".into(), false, None),
        ]
    );
    assert_eq!(
        select(&html, "#people tbody button [data-icon=lock-closed]").len(),
        3
    );
}

/// `docs/specs/access.md`, People, The named manager.
#[test_log::test(tokio::test)]
async fn the_named_manager_cannot_be_edited_or_disabled() {
    let pages = Pages::of(a_ticket()).await;
    pages.as_role(Role::Manager, "Grace Hopper").await;
    let asked = pages.with_manager("google:ada@example.com");

    let html = page_with_cookie(&asked, "/people", "").await;

    let buttons = row_buttons(&html);
    let named = Some("The `manager` setting names Ada Lovelace.".to_string());
    assert_eq!(
        buttons[..2],
        [
            ("Edit".into(), true, named.clone()),
            ("Disable".into(), true, named)
        ]
    );
    assert!(buttons[2..].iter().all(|(_, disabled, _)| !disabled));
}

/// `docs/specs/access.md`, Actions, What the reader's role does not allow.
#[test_log::test(tokio::test)]
async fn a_viewer_or_contributor_sees_people_without_its_buttons() {
    let pages = Pages::of(a_ticket()).await;
    let (viewer, contributor) = three_people(&pages).await;

    for reader in [&viewer, &contributor] {
        let html = reader.page("/people").await;

        assert_eq!(rows(&html).len(), 3);
        assert_eq!(row_buttons(&html), []);
        assert_eq!(select(&html, "dialog").len(), 0);
    }
    let (status, answer) = post(&viewer, "/people/3/disable", &[SAME_ORIGIN], &[]).await;
    assert_eq!(status, 403);
    assert!(
        answer
            .html()
            .contains("This needs the manage access level."),
        "{}",
        answer.html()
    );
}

/// `docs/specs/access.md`, Edit person; `docs/specs/access.md`, Edit
/// person: Role is a `select`.
#[test_log::test(tokio::test)]
async fn edit_person_changes_a_role() {
    let pages = Pages::of(a_ticket()).await;
    let (viewer, _) = three_people(&pages).await;
    let before = pages.page("/people").await;
    let role = select(&before, "dialog#edit-person-2 select[name=role] option")
        .into_iter()
        .map(|option| (text(option), option.attr("selected").is_some()))
        .collect::<Vec<_>>();
    let (log, _guard) = Log::capture();

    let (status, answer) = post(
        &pages,
        "/people/2/edit",
        &[SAME_ORIGIN],
        &[("role", "contributor")],
    )
    .await;

    assert_eq!(
        role,
        [
            ("viewer".into(), true),
            ("contributor".into(), false),
            ("manager".into(), false),
        ]
    );
    assert_eq!(
        text(select(&before, "#edit-person-2-title")[0]),
        "Edit person"
    );
    assert_eq!(status, 200, "{}", answer.html());
    assert_eq!(rows(&pages.page("/people").await)[2][1], "contributor");
    let html = viewer.page("/benchmarks").await;
    assert_eq!(
        select(&html, "#buttons button")
            .into_iter()
            .map(text)
            .collect::<Vec<_>>(),
        ["New"],
        "the new role counts at the next request"
    );
    let logged = log.lines("INFO");
    assert!(
        logged
            .iter()
            .any(|line| line.contains("Change a person's role: done")
                && line.contains(r#"action="edit_person""#)
                && line.contains("vera@example.com")),
        "{logged:?}"
    );
}

/// `docs/specs/access.md`, Edit person, Refusals: the message is above the
/// buttons.
#[test_log::test(tokio::test)]
async fn edit_person_refuses_to_take_the_role_from_the_last_manager() {
    let pages = Pages::of(a_ticket()).await;

    let (status, answer) = post(
        &pages,
        "/people/1/edit",
        &[SAME_ORIGIN],
        &[("role", "viewer")],
    )
    .await;
    let (gone, missing) = post(
        &pages,
        "/people/9/edit",
        &[SAME_ORIGIN],
        &[("role", "viewer")],
    )
    .await;

    assert_eq!(status, 422);
    assert_eq!(
        text(select(&answer, ".alert-error")[0]),
        "Ada Lovelace is the last manager."
    );
    assert_eq!(rows(&pages.page("/people").await)[0][1], "manager");
    assert_eq!(gone, 404);
    assert_eq!(
        text(select(&missing, ".alert-error")[0]),
        "No such person 9"
    );
}

/// `docs/specs/access.md`, Disable person and People, Disabling;
/// "Enable" on People.
#[test_log::test(tokio::test)]
async fn disabling_a_person_signs_them_out_and_enabling_lets_them_sign_in_again() {
    let pages = Pages::of(a_ticket()).await;
    let (viewer, _) = three_people(&pages).await;
    let html = pages.page("/people").await;
    let disable = confirmations(&html)
        .into_iter()
        .find(|confirmation| confirmation.action == "/people/2/disable")
        .expect("Disable on Vera's row");

    let (status, _) = post(&pages, "/people/2/disable", &[SAME_ORIGIN], &[]).await;

    assert_eq!(
        disable,
        Confirmation {
            asking: "Disable".into(),
            question: "Disable Vera Viewer? This signs them out and ends their authorizations."
                .into(),
            turn_down: "Cancel".into(),
            action: "/people/2/disable".into(),
            fields: vec![],
        }
    );
    assert_eq!(status, 200);
    assert_eq!(
        text(select(&viewer.page("/").await, ".sign-in .alert")[0]),
        "vera@example.com is disabled."
    );
    let after = pages.page("/people").await;
    assert_eq!(rows(&after)[2][2], "disabled");
    let vera = select(&after, "#people tbody tr")[2];
    assert!(has_classes(
        select_in(vera, ".badge")[0],
        &["badge-neutral"]
    ));
    let enable = select_in(vera, "form[hx-post='/people/2/enable'] button");
    assert_eq!(text(enable[0]), "Enable");
    assert_eq!(select_in(enable[0], "[data-icon=lock-open]").len(), 1);

    let (enabled, _) = post(&pages, "/people/2/enable", &[SAME_ORIGIN], &[]).await;

    assert_eq!(enabled, 200);
    assert_eq!(rows(&pages.page("/people").await)[2][2], "enabled");
    assert_eq!(
        viewer.response("/").await.status(),
        303,
        "enabling signs no browser back in"
    );
    let (refused, answer) = post(&pages, "/people/1/disable", &[SAME_ORIGIN], &[]).await;
    assert_eq!(refused, 422);
    assert!(answer.html().contains("Ada Lovelace is the last manager."));
    let (cross_site, _) = post(&pages, "/people/3/disable", &[CROSS_SITE], &[]).await;
    assert_eq!(cross_site, 403);
}

/// The next event of `events` that sends the region `id`, its regions.
async fn next_sending(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    id: &str,
) -> Vec<(String, Html)> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let regions = regions(&events.recv().await.expect("the stream runs"));
            if regions.iter().any(|(region, _)| region == id) {
                return regions;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {id} in time"))
}

/// `docs/specs/design_system.md`, Screens, Live; `docs/specs/access.md`, Roles,
/// At each request: a row People gains has its dialogs, and a reader made
/// a manager gets the buttons, without opening People again.
#[test_log::test(tokio::test)]
async fn people_keeps_up_with_new_people_and_with_the_readers_role() {
    let pages = Pages::of(a_ticket()).await;
    let mut managing = stream(&pages, "/events/people").await;
    next_sending(&mut managing, "people-dialogs").await;
    let viewer = pages.as_viewer().await;

    let regions = next_sending(&mut managing, "people-dialogs").await;

    let dialogs = region_of(&regions, "people-dialogs");
    assert_eq!(select(dialogs, "dialog#edit-person-2").len(), 1);
    assert_eq!(select(dialogs, "dialog#disable-person-2").len(), 1);
    let mut viewing = stream(&viewer, "/events/people").await;
    let first = next_sending(&mut viewing, "people").await;
    assert_eq!(row_buttons(region_of(&first, "people")), []);
    assert_eq!(
        select(region_of(&first, "people-dialogs"), "dialog").len(),
        0
    );

    let (status, _) = post(
        &pages,
        "/people/2/edit",
        &[SAME_ORIGIN],
        &[("role", "manager")],
    )
    .await;

    assert_eq!(status, 200);
    let managed = next_sending(&mut viewing, "people-dialogs").await;
    assert_eq!(row_buttons(region_of(&managed, "people")).len(), 4);
    assert_eq!(
        select(region_of(&managed, "people-dialogs"), "dialog").len(),
        4
    );
}
