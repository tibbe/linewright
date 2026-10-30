use super::*;

/// #1 with the maintainer; #2 cut into #3, which waits on alice, asked;
/// and #4, which Linewright filed, closed.
async fn with_a_board() -> World {
    let world = with_ticket().await;
    let tracking = world.store.open_issue(
        "maintainer",
        "Big thing",
        "Cut it.",
        &["linewright:designed"],
    );
    let part = world
        .store
        .open_issue(LINEWRIGHT, "Part one", "Of it.", &["needs-info"]);
    world.store.assign_as(LINEWRIGHT, part, "alice").unwrap();
    world.store.add_sub_issue(tracking, part).await.unwrap();
    let closed = world.store.open_issue(
        LINEWRIGHT,
        "Old idea",
        "Maybe.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.close_issue_as_completed(closed).unwrap();
    world
}

/// `element`'s text, with each duration kept up as it runs, which the fake
/// clock sets as the test runs, as `DURATION`.
fn untimed(element: ElementRef) -> String {
    let ticking = Selector::parse("time[data-since]").unwrap();
    element.select(&ticking).fold(text(element), |shown, time| {
        shown.replacen(&text(time), "DURATION", 1)
    })
}

/// Each lane's heading, then each card as its column, its heading and its
/// line, as a reader reads them.
fn board_shown(html: &Html) -> Vec<Vec<String>> {
    let cells = Selector::parse(".cell").unwrap();
    select(html, "#board .lane")
        .into_iter()
        .map(|lane| {
            let heading = lane
                .select(&Selector::parse(".lane-head").unwrap())
                .map(text)
                .collect::<String>();
            let cards = lane.select(&cells).flat_map(|cell| {
                let column = cell.attr("aria-label").unwrap().to_string();
                cell.select(&Selector::parse(".card").unwrap())
                    .map(move |card| {
                        let lines: Vec<String> = card
                            .select(&Selector::parse("h3, .line").unwrap())
                            .map(untimed)
                            .collect();
                        format!("{column}: {}", lines.join(" | "))
                    })
                    .collect::<Vec<_>>()
            });
            std::iter::once(heading).chain(cards).collect()
        })
        .collect()
}

#[tokio::test]
async fn the_board_shows_the_cards_in_their_columns_and_lanes_each_with_its_line() {
    let web = Web::start(with_a_board().await).await;

    let html = web.page("/").await;

    let shown: Vec<Vec<String>> = board_shown(&html)
        .iter()
        .map(|lane| lane.iter().map(|line| unclocked(line)).collect())
        .collect();
    assert_eq!(
        shown,
        [
            vec![
                "#2: Big thing — 0/1 tickets closed".to_string(),
                "Needs a person: #3: Part one | asked alice · 2026-10-01 12:00:SS UTC".to_string(),
            ],
            vec![
                "#1: Do the thing".to_string(),
                "Needs a person: #1: Do the thing | decision needed · 2026-10-01 12:00:SS UTC"
                    .to_string(),
            ],
            vec![
                "#4: Old idea — finished 2026-10-01 12:00:SS UTC".to_string(),
                "Done: #4: Old idea | closed as completed · 2026-10-01 12:00:SS UTC".to_string(),
            ],
        ]
    );
    let lanes: Vec<Option<&str>> = select(&html, "#board .lane")
        .into_iter()
        .map(|lane| lane.attr("id"))
        .collect();
    assert_eq!(lanes, [Some("lane-2"), Some("lane-1"), Some("lane-4")]);
    let columns: Vec<String> = select(&html, "#board .column")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(
        columns,
        [
            "Triage",
            "Needs a person",
            "Ready",
            "Working",
            "In review",
            "Done"
        ]
    );
    let links: Vec<&str> = select(&html, "#board .card h3 a:not(.github)")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(links, ["/items/3", "/items/1", "/items/4"]);
    let title = select(&html, "title");
    assert_eq!(text(title[0]), "(2) Board · owner/widgets");
    assert_no_status_by_colour_alone(&html);
    web.stop().await;
}

#[tokio::test]
async fn a_card_waiting_on_the_people_asked_names_them_as_a_list_or_says_people_asked() {
    let world = with_ticket().await;
    let two =
        open_issue_linewright_labeled(&world.store, "maintainer", "Two", "Who?", &["needs-info"]);
    for person in ["alice", "bob"] {
        world.store.assign_as(LINEWRIGHT, two, person).unwrap();
    }
    let three =
        open_issue_linewright_labeled(&world.store, "maintainer", "Three", "Who?", &["needs-info"]);
    for person in ["alice", "bob", "carol"] {
        world.store.assign_as(LINEWRIGHT, three, person).unwrap();
    }
    open_issue_linewright_labeled(
        &world.store,
        "maintainer",
        "Nobody",
        "Who?",
        &["needs-info"],
    );
    let linewright = open_issue_linewright_labeled(
        &world.store,
        "maintainer",
        "Linewright",
        "Who?",
        &["needs-info"],
    );
    world
        .store
        .assign_as(LINEWRIGHT, linewright, LINEWRIGHT)
        .unwrap();
    let web = Web::start(world).await;

    let html = web.page("/").await;

    // Lanes come latest changed first.
    let lines: Vec<String> = board_shown(&html)
        .iter()
        .flatten()
        .filter(|line| line.starts_with("Needs a person: #") && !line.contains("#1:"))
        .map(|line| unclocked(line))
        .collect();
    assert_eq!(
        lines,
        [
            "Needs a person: #5: Linewright | people asked · 2026-10-01 12:00:SS UTC",
            "Needs a person: #4: Nobody | people asked · 2026-10-01 12:00:SS UTC",
            "Needs a person: #3: Three | asked alice, bob and carol · 2026-10-01 12:00:SS UTC",
            "Needs a person: #2: Two | asked alice and bob · 2026-10-01 12:00:SS UTC",
        ]
    );
    web.stop().await;
}

#[tokio::test]
async fn the_boards_stream_sends_the_board_and_the_tab_title_after_a_poll_moves_a_card() {
    let web = Web::start(with_a_board().await).await;
    let mut events = stream(&web, "/events/").await;
    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<String> = regions(&first).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, ["holds", "title", "board"]);

    web.store
        .assign_as(LINEWRIGHT, linewright::domain::Number(1), LINEWRIGHT)
        .unwrap();
    web.store
        .close_issue_as_completed(linewright::domain::Number(1))
        .unwrap();
    web.deliver_issue(1).await;
    let moved = loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .unwrap()
            .unwrap();
        let regions = regions(&event);
        if regions.iter().any(|(id, _)| id == "board") {
            break regions;
        }
    };

    let board = &moved.iter().find(|(id, _)| id == "board").unwrap().1;
    let shown = board_shown(board);
    let lane = shown
        .iter()
        .find(|lane| lane[0].starts_with("#1: Do the thing — finished "))
        .unwrap_or_else(|| panic!("{shown:?}"));
    assert_eq!(
        lane[1..]
            .iter()
            .map(|line| unclocked(line))
            .collect::<Vec<_>>(),
        ["Done: #1: Do the thing | closed as completed · 2026-10-01 12:00:SS UTC"]
    );
    let title = &moved.iter().find(|(id, _)| id == "title").unwrap().1;
    assert_eq!(text(select(title, "title")[0]), "(1) Board · owner/widgets");
    web.stop().await;
}

/// `with_a_board`'s, and #5, cut into #6, which is closed.
async fn with_a_finished_lane() -> World {
    let world = with_a_board().await;
    let tracking = world.store.open_issue(
        "maintainer",
        "Small thing",
        "Cut it.",
        &["linewright:designed"],
    );
    let part = world
        .store
        .open_issue(LINEWRIGHT, "Only part", "Of it.", &["ready-for-agent"]);
    world.store.add_sub_issue(tracking, part).await.unwrap();
    world.store.close_issue_as_completed(part).unwrap();
    world.store.close_issue_as_completed(tracking).unwrap();
    world
}

/// `shown` with the seconds of the times in it, which the fake clock sets as
/// the test runs, as `SS`.
fn unclocked(shown: &str) -> String {
    let mut rest = shown;
    let mut unclocked = String::new();
    while let Some(at) = rest.find("2026-10-01 12:00:") {
        let end = at + "2026-10-01 12:00:".len();
        unclocked.push_str(&rest[..end]);
        unclocked.push_str("SS");
        rest = &rest[end + 2..];
    }
    unclocked.push_str(rest);
    unclocked
}

#[tokio::test]
async fn the_board_folds_its_finished_lanes_under_one_closed_fold_after_the_rest() {
    let web = Web::start(with_a_finished_lane().await).await;

    let html = web.page("/").await;

    let shown: Vec<Vec<String>> = board_shown(&html)
        .iter()
        .map(|lane| lane.iter().map(|line| unclocked(line)).collect())
        .collect();
    assert_eq!(
        shown,
        [
            vec![
                "#2: Big thing — 0/1 tickets closed".to_string(),
                "Needs a person: #3: Part one | asked alice · 2026-10-01 12:00:SS UTC".to_string(),
            ],
            vec![
                "#1: Do the thing".to_string(),
                "Needs a person: #1: Do the thing | decision needed · 2026-10-01 12:00:SS UTC"
                    .to_string(),
            ],
            vec![
                "#5: Small thing — 1/1 tickets closed — finished 2026-10-01 12:00:SS UTC"
                    .to_string(),
                "Done: #6: Only part | closed as completed · 2026-10-01 12:00:SS UTC".to_string(),
            ],
            vec![
                "#4: Old idea — finished 2026-10-01 12:00:SS UTC".to_string(),
                "Done: #4: Old idea | closed as completed · 2026-10-01 12:00:SS UTC".to_string(),
            ],
        ]
    );
    let folds: Vec<(Option<&str>, bool)> = select(&html, "#board details")
        .into_iter()
        .map(|fold| (fold.attr("id"), fold.attr("open").is_some()))
        .collect();
    assert_eq!(
        folds,
        [
            (Some("finished"), false),
            (Some("lane-5"), false),
            (Some("lane-4"), false)
        ]
    );
    let summaries: Vec<String> = select(&html, "#finished > summary")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(summaries, ["2 finished lanes"]);
    let finished: Vec<&str> = select(&html, "#lane-5 > summary time")
        .into_iter()
        .filter_map(|time| time.attr("datetime"))
        .collect();
    let [finished] = finished[..] else {
        panic!("{finished:?}");
    };
    assert!(
        finished.starts_with("2026-10-01T12:00:"),
        "a time the page's script shows relative: {finished}"
    );
    web.stop().await;
}

#[tokio::test]
async fn a_lane_head_and_a_card_title_cut_to_fit_are_whole_in_the_pages_tooltip_not_in_place() {
    let web = Web::start(with_a_finished_lane().await).await;

    let html = web.page("/").await;

    let heads = select(&html, "#board .lane-head");
    assert_eq!(heads.len(), 4);
    for head in heads {
        assert!(lacks_classes(head, &["group"]), "{}", head.html());
        let cut = select_in(head, ":scope > span")[0];
        assert!(has_classes(cut, &["truncate"]), "{}", head.html());
        assert!(cut.attr("data-tip-cut").is_some(), "{}", head.html());
        assert_eq!(grows_in_place(cut), Vec::<&str>::new(), "{}", head.html());
    }
    let titles = select(&html, "#board .card h3");
    assert_ne!(titles, [] as [scraper::ElementRef<'_>; 0]);
    for title in titles {
        assert!(has_classes(title, &["line-clamp-2"]), "{}", title.html());
        assert!(title.attr("data-tip-cut").is_some(), "{}", title.html());
        assert_eq!(
            grows_in_place(title),
            Vec::<&str>::new(),
            "{}",
            title.html()
        );
    }
    web.stop().await;
}

#[tokio::test]
async fn the_board_shows_what_a_hover_shows_in_the_pages_tooltip_never_in_a_title() {
    let web = Web::start(with_a_board().await).await;

    let html = web.page("/").await;

    assert_eq!(select(&html, "[title]").len(), 0);
    let github: Vec<(Option<&str>, Option<&str>)> = select(&html, "#board .card a.github")
        .into_iter()
        .map(|a| (a.attr("aria-label"), a.attr("data-tip")))
        .collect();
    assert_eq!(
        github,
        [
            (Some("#3 on GitHub"), Some("#3 on GitHub")),
            (Some("#1 on GitHub"), Some("#1 on GitHub")),
            (Some("#4 on GitHub"), Some("#4 on GitHub")),
        ]
    );
    let times = select(&html, "#board time[datetime]");
    assert_ne!(times, [] as [scraper::ElementRef<'_>; 0]);
    for time in times {
        assert_eq!(time.attr("tabindex"), Some("0"), "{}", time.html());
    }
    web.stop().await;
}

#[tokio::test]
async fn each_lane_is_a_surface_sunk_into_the_page_with_its_cards_raised_on_it() {
    let web = Web::start(with_a_finished_lane().await).await;

    let html = web.page("/").await;

    let lanes = select(&html, "#board .lane");
    assert_eq!(lanes.len(), 4);
    for lane in lanes {
        assert!(
            has_classes(
                lane,
                &[
                    "bg-base-200",
                    "border-(length:--lane-border)",
                    "border-base-300",
                    "rounded-box"
                ]
            ),
            "{}",
            lane.html()
        );
        for head in select_in(lane, ".lane-head") {
            assert!(lacks_classes(head, &["border-t"]), "{}", head.html());
        }
    }
    let cards = select(&html, "#board .card");
    assert_eq!(cards.len(), 4);
    for card in cards {
        assert!(
            has_classes(
                card,
                &["card", "bg-card", "border", "border-base-300", "shadow-sm"]
            ),
            "{}",
            card.html()
        );
    }
    web.stop().await;
}

/// The icons `element` holds, as their markup.
fn icons(element: ElementRef) -> Vec<String> {
    select_in(element, "svg.icon")
        .into_iter()
        .map(|icon| icon.html())
        .collect()
}

/// `docs/specs/web/design_system.md`, Status: waiting for a person is
/// `warning`.
const WAITING_FOR_A_PERSON: &str = "[--tone:var(--color-warning)]";

/// `docs/specs/design_system.md`, The board, Lanes: a lane's head is its
/// number and title, with its tickets closed of total and when it finished,
/// and no status of its own (Out of scope); `docs/specs/web/design_system.md`,
/// Board: a bar in the Lane colour.
#[tokio::test]
async fn a_lane_head_shows_no_status_of_its_own() {
    let web = Web::start(with_a_finished_lane().await).await;

    let html = web.page("/").await;

    let heads = select(&html, "#board .lane-head");
    assert_eq!(heads.len(), 4);
    for head in heads {
        assert!(
            !head
                .value()
                .classes()
                .any(|class| class == "toned" || class.starts_with("[--tone:")),
            "{}",
            head.html()
        );
        assert_eq!(icons(head), Vec::<String>::new(), "{}", head.html());
    }
    web.stop().await;
}

/// `docs/specs/web/design_system.md`, Status: paused or queued is Muted
/// text.
const PAUSED_OR_QUEUED: &str = "[--tone:var(--color-muted)]";

#[tokio::test]
async fn a_card_paused_for_the_usage_limit_shows_it_in_muted_text() {
    let world = World::with(|config| config.work_dir = config.work_dir.join(REPO)).await;
    world.store.open_issue(
        "maintainer",
        "Do the thing",
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    );
    world
        .harness
        .on(WorkKind::Implement, |_| refused_for_the_usage_limit());
    let web = Web::start(world).await;

    let html = web
        .eventually_page("/", |html| {
            text(select(html, "#board")[0]).contains("paused for the usage limit")
        })
        .await;

    let cards = select(&html, "#board .card");
    assert_eq!(cards.len(), 1);
    let card = cards[0];
    assert!(
        has_classes(card, &["toned", PAUSED_OR_QUEUED]),
        "{}",
        card.html()
    );
    assert_eq!(
        select_in(card, ".line > svg.icon:first-child").len(),
        1,
        "{}",
        card.html()
    );
    assert_no_status_by_colour_alone(&html);
    web.stop().await;
}

/// `docs/specs/web/design_system.md`, Colour scheme: secondary text is in
/// Muted text.
#[tokio::test]
async fn a_cards_line_under_its_title_is_in_muted_text() {
    let web = Web::start(with_a_board().await).await;

    let html = web.page("/").await;

    assert_in_muted_text(&html, "#board .card .line");
    web.stop().await;
}

#[tokio::test]
async fn a_card_with_a_card_status_shows_it_and_a_card_in_done_is_faded() {
    let world = with_a_finished_lane().await;
    let blocker = world.store.open_issue(
        "maintainer",
        "Blocker",
        "First.",
        &["ready-for-agent", "linewright:designed"],
    );
    let blocked = world.store.open_issue(
        "maintainer",
        "Blocked",
        "After.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.add_blocked_by(blocked, blocker).await.unwrap();
    let web = Web::start(world).await;

    let html = web
        .eventually_page("/", |html| {
            text(select(html, "#board")[0]).contains("blocked by")
        })
        .await;

    let mut cards: Vec<(String, bool, bool, usize, bool)> = select(&html, "#board .card")
        .into_iter()
        .map(|card| {
            let title = text(select_in(card, "h3")[0]);
            let toned = has_classes(card, &["toned"]);
            let waiting = has_classes(card, &[WAITING_FOR_A_PERSON]);
            // Before its line.
            let icons = select_in(card, ".line > svg.icon:first-child").len();
            let faded = has_classes(card, &["opacity-75"]);
            (title, toned, waiting, icons, faded)
        })
        .collect();
    cards.retain(|(title, ..)| !title.contains("Blocker"));
    cards.sort();
    assert_eq!(
        cards,
        [
            ("#1: Do the thing".to_string(), true, true, 1, false),
            ("#3: Part one".to_string(), true, true, 1, false),
            ("#4: Old idea".to_string(), false, false, 0, true),
            ("#6: Only part".to_string(), false, false, 0, true),
            ("#8: Blocked".to_string(), false, false, 0, false),
        ]
    );
    web.stop().await;
}

#[tokio::test]
async fn a_board_whose_lanes_are_all_finished_shows_the_column_heads_and_one_closed_fold() {
    let world = with_ticket().await;
    world
        .store
        .assign_as(LINEWRIGHT, linewright::domain::Number(1), LINEWRIGHT)
        .unwrap();
    world
        .store
        .close_issue_as_completed(linewright::domain::Number(1))
        .unwrap();
    let web = Web::start(world).await;

    let html = web.page("/").await;

    let shown: Vec<String> = select(&html, "#board .board > *")
        .into_iter()
        .map(|element| {
            let fold = element.value().attr("open").map_or("", |_| " open");
            unclocked(&format!(
                "{}{fold}: {}",
                element.value().name(),
                text(element)
            ))
        })
        .collect();
    assert_eq!(
        shown,
        [
            "div: Triage Needs a person Ready Working In review Done",
            "details: 1 finished lane #1: Do the thing — finished \
             2026-10-01 12:00:SS UTC #1: Do the thing closed as completed · 2026-10-01 12:00:SS UTC",
        ]
    );
    let lanes: Vec<Option<&str>> = select(&html, "#finished details")
        .into_iter()
        .map(|fold| fold.attr("id"))
        .collect();
    assert_eq!(lanes, [Some("lane-1")]);
    web.stop().await;
}

/// The board's cards, each as its column and what a reader reads of it,
/// and whether it carries the pull request icon.
fn cards_shown(html: &Html) -> Vec<(String, bool)> {
    board_shown(html)
        .into_iter()
        .flat_map(|lane| lane.into_iter().skip(1))
        .map(|card| unclocked(&card))
        .zip(select(html, "#board .card").into_iter().map(|card| {
            card.select(&Selector::parse("h3 svg.pull-request").unwrap())
                .count()
                == 1
        }))
        .collect()
}

#[tokio::test]
async fn an_issue_in_review_shows_its_pull_request_and_once_it_closes_unmerged_that_it_was_handed_back()
 {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world
        .store
        .set_checks(
            pr,
            vec![check("test", linewright::domain::CheckState::Pending, "")],
        )
        .await
        .unwrap();
    let web = Web::start(world).await;

    let html = web.page("/").await;
    let in_review = cards_shown(&html);
    let links: Vec<&str> = select(&html, "#board .card .line a:not(.github)")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    web.store.close_pull_as(pr).unwrap();
    web.store.unlabel_as(ticket, "ready-for-agent").unwrap();
    web.store
        .label_as("maintainer", ticket, "ready-for-human")
        .unwrap();
    web.deliver_issue(ticket.0).await;
    let handed_back = eventually_on_the_board(&web, "handed back · 2026-10-01 12:00:SS UTC").await;

    assert_eq!(
        in_review,
        [(
            format!("In review: {ticket}: Do the thing | {pr} · checks running"),
            false
        )]
    );
    assert_eq!(links, [format!("/items/{}", pr.0)]);
    assert_eq!(
        handed_back,
        [(
            format!(
                "Needs a person: {ticket}: Do the thing | handed back · 2026-10-01 12:00:SS UTC"
            ),
            false
        )]
    );
    // `docs/specs/design_system.md`, Cards: handed back is waiting for a
    // person.
    let board = web.page("/").await;
    let card = select(&board, "#board .card")[0];
    assert!(
        has_classes(card, &["toned", WAITING_FOR_A_PERSON]),
        "{}",
        card.html()
    );
    web.stop().await;
}

#[tokio::test]
async fn a_stalled_card_says_stalled_in_needs_a_person_and_after_its_pull_request_in_review() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world.store.unassign_as(LINEWRIGHT, pr, LINEWRIGHT).unwrap();
    world
        .store
        .label_as("maintainer", pr, "linewright:stalled")
        .unwrap();
    let stalled = open_issue_linewright_labeled(
        &world.store,
        "maintainer",
        "Do another thing",
        "Please.",
        &["ready-for-human", "linewright:stalled"],
    );
    let web = Web::start(world).await;

    let shown = cards_shown(&web.page("/").await);

    assert_eq!(
        shown,
        [
            (
                format!(
                    "Needs a person: {stalled}: Do another thing | stalled · 2026-10-01 12:00:SS UTC"
                ),
                false
            ),
            (
                format!("In review: {ticket}: Do the thing | {pr} · stalled"),
                false
            ),
        ]
    );
    web.stop().await;
}

#[tokio::test]
async fn a_merged_pull_request_of_linewrights_closing_no_issue_is_a_pull_request_card_in_done() {
    let world = World::new().await;
    let pr = world
        .store
        .open_pull_as(
            LINEWRIGHT,
            "tidy",
            "Tidy up",
            "No issue.",
            &[("src/tidy.txt", "tidy\n")],
        )
        .await
        .unwrap();
    world.store.merge_pull(pr).await.unwrap();
    let web = Web::start(world).await;

    let shown = cards_shown(&web.page("/").await);

    assert_eq!(
        shown,
        [(
            format!("Done: {pr}: Tidy up | merged · 2026-10-01 12:00:SS UTC"),
            true
        )]
    );
    web.stop().await;
}

/// Each card of an issue, as [`cards_shown`].
fn issue_cards_shown(html: &Html) -> Vec<String> {
    cards_shown(html)
        .into_iter()
        .filter(|(_, pull_request)| !pull_request)
        .map(|(card, _)| card)
        .collect()
}

#[tokio::test]
async fn an_issue_whose_pull_request_merged_is_in_done_before_github_closes_it() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world
        .store
        .merge_pull_before_its_issues_close(pr)
        .await
        .unwrap();
    let web = Web::start(world).await;

    assert_eq!(
        issue_cards_shown(&web.page("/").await),
        [format!(
            "Done: {ticket}: Do the thing | closed in {pr} · 2026-10-01 12:00:SS UTC"
        )]
    );
    web.stop().await;
}

#[tokio::test]
async fn an_issue_reopened_after_its_pull_request_merged_leaves_done() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world.store.merge_pull(pr).await.unwrap();
    world.store.reopen_issue_as(ticket).unwrap();
    let web = Web::start(world).await;

    let shown = issue_cards_shown(&web.page("/").await);

    assert_eq!(shown.len(), 1);
    assert!(
        shown[0].starts_with(&format!("Working: {ticket}: Do the thing")),
        "{shown:?}"
    );
    web.stop().await;
}

#[tokio::test]
async fn an_issue_whose_pull_request_merged_into_another_branch_is_not_in_done() {
    let mut world = World::new().await;
    let (ticket, pr) = world.ticket_in_review().await;
    world
        .store
        .commit(
            "maintainer",
            "release",
            &[("RELEASE", "1.0\n")],
            "Branch off",
        )
        .await
        .unwrap();
    world.store.retarget(pr, "release").unwrap();
    world.store.merge_pull(pr).await.unwrap();
    let web = Web::start(world).await;

    let shown = issue_cards_shown(&web.page("/").await);

    assert_eq!(shown.len(), 1);
    assert!(
        shown[0].starts_with(&format!("Working: {ticket}: Do the thing")),
        "{shown:?}"
    );
    web.stop().await;
}

#[tokio::test]
async fn an_issue_closed_without_a_merge_shows_githubs_reason_for_closing_it() {
    use linewright::domain::IssueCloseReason::{Completed, Duplicate, NotPlanned};
    let world = World::new().await;
    for (title, reason) in [
        ("Done it", Completed),
        ("Never mind", NotPlanned),
        ("Same as another", Duplicate),
    ] {
        let issue = world.store.open_issue(
            LINEWRIGHT,
            title,
            "Maybe.",
            &["ready-for-agent", "linewright:designed"],
        );
        world.store.close_issue_as(issue, reason).unwrap();
    }
    let web = Web::start(world).await;

    let mut shown: Vec<String> = cards_shown(&web.page("/").await)
        .into_iter()
        .map(|(card, _)| card)
        .collect();
    shown.sort();

    assert_eq!(
        shown,
        [
            "Done: #1: Done it | closed as completed · 2026-10-01 12:00:SS UTC",
            "Done: #2: Never mind | closed as not planned · 2026-10-01 12:00:SS UTC",
            "Done: #3: Same as another | closed as a duplicate · 2026-10-01 12:00:SS UTC",
        ]
    );
    web.stop().await;
}

#[tokio::test]
async fn a_pull_request_with_nothing_else_stopping_it_merging_ends_its_line_with_its_review_decision()
 {
    use linewright::domain::{CheckState, ReviewState};
    let mut world = World::new().await;
    let mut in_review = vec![];
    for _ in 0..5 {
        let (ticket, pr) = world.ticket_in_review().await;
        world
            .store
            .set_checks(pr, vec![check("test", CheckState::Passed, "")])
            .await
            .unwrap();
        in_review.push((ticket, pr));
    }
    let [
        (unruled_ticket, unruled),
        (required_ticket, required),
        (changes_ticket, changes),
        (approved_ticket, approved),
        (approved_unruled_ticket, approved_unruled),
    ] = in_review[..]
    else {
        unreachable!();
    };
    for pr in [required, changes, approved] {
        world.store.require_review(pr);
    }
    world.rework_asked(changes);
    world
        .store
        .review_as(
            "maintainer",
            changes,
            ReviewState::ChangesRequested,
            "Rename it.",
            &[],
        )
        .unwrap();
    for pr in [approved, approved_unruled] {
        world
            .store
            .review_as("maintainer", pr, ReviewState::Approved, "", &[])
            .unwrap();
    }
    // The rework answers the review, whose decision stands.
    world.poll().await;
    let web = Web::start(world).await;

    let mut shown: Vec<String> = cards_shown(&web.page("/").await)
        .into_iter()
        .map(|(card, _)| card)
        .collect();

    // In the tickets' order, whatever the lanes'.
    shown.sort();
    let says = |ticket, line: String| format!("In review: {ticket}: Do the thing | {line}");
    assert_eq!(
        shown,
        [
            says(unruled_ticket, format!("{unruled}")),
            says(required_ticket, format!("{required} · review required")),
            says(changes_ticket, format!("{changes} · changes requested")),
            says(approved_ticket, format!("{approved} · approved")),
            // No rule requires a review, so GitHub decides none.
            says(approved_unruled_ticket, format!("{approved_unruled}")),
        ]
    );
    web.stop().await;
}

#[tokio::test]
async fn a_pull_request_blocked_on_a_code_owners_review_with_no_review_decision_says_review_required()
 {
    use linewright::domain::{CheckState, Requirement};
    let mut world = World::new().await;
    let mut in_review = vec![];
    for state in [CheckState::Passed, CheckState::Pending] {
        let (ticket, pr) = world.ticket_in_review().await;
        world
            .store
            .set_checks(pr, vec![check("test", state, "")])
            .await
            .unwrap();
        in_review.push((ticket, pr));
    }
    let [(ticket, pr), (running_ticket, running)] = in_review[..] else {
        unreachable!();
    };
    // Closes an issue closed before the Done window, so no issue on the
    // board: it is a card of its own.
    let untidy = world
        .store
        .open_issue("maintainer", "Untidy", "Tidy it.", &[]);
    let lone = world
        .store
        .open_pull_as(
            LINEWRIGHT,
            "tidy",
            "Tidy up",
            &format!("Closes {untidy}."),
            &[("src/tidy.txt", "tidy\n")],
        )
        .await
        .unwrap();
    for blocked in [pr, running, lone] {
        world.store.block_merge(
            blocked,
            vec![Requirement::CodeOwnerReview {
                owners: vec!["maintainer".into()],
            }],
        );
    }
    world.poll().await;
    world.store.close_issue_as_completed(untidy).unwrap();
    world.clock.advance(chrono::TimeDelta::days(8));
    let web = Web::start(world).await;

    let html = web.page("/").await;

    let mut shown: Vec<String> = cards_shown(&html)
        .into_iter()
        .map(|(card, _)| card)
        .collect();
    shown.sort();
    assert_eq!(
        shown,
        [
            format!("In review: {ticket}: Do the thing | {pr} · review required"),
            format!("In review: {running_ticket}: Do the thing | {running} · checks running"),
            format!("In review: {lone}: Tidy up | review required"),
        ]
    );
    // `docs/specs/design_system.md`, Cards: "review required" is waiting for
    // a person.
    let mut waiting: Vec<String> = select(&html, "#board .card")
        .into_iter()
        .filter(|card| has_classes(*card, &["toned", WAITING_FOR_A_PERSON]))
        .map(|card| text(select_in(card, "h3")[0]))
        .collect();
    waiting.sort();
    assert_eq!(
        waiting,
        [
            format!("{ticket}: Do the thing"),
            format!("{lone}: Tidy up"),
        ]
    );
    web.stop().await;
}

/// The board's cards once one of them says `line`.
async fn eventually_on_the_board(web: &Web, line: &str) -> Vec<(String, bool)> {
    let waited = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let shown = cards_shown(&web.page("/").await);
            if shown.iter().any(|(card, _)| card.ends_with(line)) {
                return shown;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    waited.expect("shown in time")
}

#[tokio::test]
async fn a_card_waiting_in_its_column_ends_its_line_with_when_it_entered_its_column() {
    let world = with_issue_for_triage().await;
    world
        .harness
        .on(WorkKind::Triage, |_| refused_for_the_usage_limit());
    let blocker = open_issue_linewright_labeled(
        &world.store,
        "maintainer",
        "First",
        "Before.",
        &["ready-for-human"],
    );
    let blocked = world.store.open_issue(
        "maintainer",
        "Then",
        "After.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.add_blocked_by(blocked, blocker).await.unwrap();
    let web = Web::start(world).await;

    let html = web
        .eventually_page("/", |html| {
            cards_shown(html)
                .iter()
                .any(|(card, _)| card.starts_with("Triage: "))
        })
        .await;

    let shown: Vec<String> = cards_shown(&html)
        .into_iter()
        .map(|(card, _)| card)
        .collect();
    assert_eq!(
        shown,
        [
            // The run refused for the usage limit changed #1 last.
            "Triage: #1: Export fails | 2026-10-01 12:00:SS UTC",
            "Needs a person: #2: First | decision needed · 2026-10-01 12:00:SS UTC",
            "Ready: #3: Then | blocked by #2 · 2026-10-01 12:00:SS UTC",
        ]
    );
    let entered: Vec<&str> = select(&html, "#board .card .line time[datetime]")
        .into_iter()
        .filter_map(|time| time.attr("datetime"))
        .collect();
    assert_eq!(entered.len(), 3, "{entered:?}");
    assert!(
        entered
            .iter()
            .all(|at| at.starts_with("2026-10-01T12:00:") && at.ends_with('Z')),
        "{entered:?}"
    );
    assert!(
        select(&html, "#board .card .line time[data-since]").is_empty(),
        "no card's wait ticks as a duration"
    );
    web.stop().await;
}

#[tokio::test]
async fn a_card_linewright_was_unassigned_from_says_so_and_its_own_pull_request_leaves_off_its_number()
 {
    use linewright::domain::CheckState;
    let world = World::new().await;
    let let_go = world.store.open_issue(
        "maintainer",
        "Let go",
        "Later.",
        &["ready-for-agent", "linewright:designed"],
    );
    world
        .store
        .assign_as(LINEWRIGHT, let_go, LINEWRIGHT)
        .unwrap();
    world
        .store
        .unassign_as("maintainer", let_go, LINEWRIGHT)
        .unwrap();
    // An issue closed before the Done window is not on the board.
    let untidy = world
        .store
        .open_issue("maintainer", "Untidy", "Tidy it.", &[]);
    let pr = world
        .store
        .open_pull_as(
            LINEWRIGHT,
            "tidy",
            "Tidy up",
            &format!("Closes {untidy}."),
            &[("src/tidy.txt", "tidy\n")],
        )
        .await
        .unwrap();
    world
        .store
        .set_checks(pr, vec![check("test", CheckState::Pending, "")])
        .await
        .unwrap();
    world.store.close_issue_as_completed(untidy).unwrap();
    world.clock.advance(chrono::TimeDelta::days(8));
    let web = Web::start(world).await;

    let mut shown: Vec<String> = cards_shown(&web.page("/").await)
        .into_iter()
        .map(|(card, _)| card)
        .collect();
    shown.sort();

    assert_eq!(
        shown,
        [
            format!("In review: {pr}: Tidy up | checks running"),
            format!("Ready: {let_go}: Let go | unassigned · 2026-10-01 12:00:SS UTC"),
        ]
    );
    web.stop().await;
}

#[tokio::test]
async fn with_a_required_label_done_holds_only_the_closed_issues_linewright_worked() {
    let world = World::with(|c| c.require_label = Some(REQUIRED.into())).await;
    let open = |author: &str, title: &str, labels: &[&str]| {
        world.store.open_issue(author, title, "Maybe.", labels)
    };
    let theirs = open("maintainer", "Theirs", &["ready-for-agent"]);
    let triaged_by_hand = open("maintainer", "Triaged by hand", &["needs-triage"]);
    let marked = open("maintainer", "Marked", &["ready-for-agent", REQUIRED]);
    let filed = open(LINEWRIGHT, "Filed by Linewright", &["ready-for-agent"]);
    let assigned = open("maintainer", "Assigned", &["ready-for-agent"]);
    world
        .store
        .assign_as("maintainer", assigned, LINEWRIGHT)
        .unwrap();
    let handed_back = open("maintainer", "Handed back", &["ready-for-agent"]);
    world
        .store
        .assign_as(LINEWRIGHT, handed_back, LINEWRIGHT)
        .unwrap();
    world
        .store
        .unassign_as("maintainer", handed_back, LINEWRIGHT)
        .unwrap();
    let ours = open("maintainer", "Closed by ours", &["ready-for-agent"]);
    let pr = world
        .store
        .open_pull_as(
            LINEWRIGHT,
            "ours",
            "Ours",
            &format!("Closes {ours}."),
            &[("src/ours.txt", "ours\n")],
        )
        .await
        .unwrap();
    world.store.merge_pull(pr).await.unwrap();
    for issue in [
        theirs,
        triaged_by_hand,
        marked,
        filed,
        assigned,
        handed_back,
    ] {
        world.store.close_issue_as_completed(issue).unwrap();
    }
    let web = Web::start(world).await;

    let mut shown: Vec<String> = cards_shown(&web.page("/").await)
        .into_iter()
        .filter(|(_, pull_request)| !pull_request)
        .filter_map(|(card, _)| card.split(" | ").next().map(str::to_string))
        .collect();
    shown.sort();

    assert_eq!(
        shown,
        [
            "Done: #4: Filed by Linewright",
            "Done: #6: Handed back",
            "Done: #7: Closed by ours",
        ]
    );
    web.stop().await;
}

#[tokio::test]
async fn with_the_spec_lookback_shorter_than_the_done_window_what_finished_between_them_is_in_done_and_a_spec_change_merged_then_is_left_alone()
 {
    let mut world = World::with(|c| {
        c.spec_lookback = chrono::TimeDelta::days(2);
        c.done_window = chrono::TimeDelta::days(7);
    })
    .await;
    let (ticket, pr) = world.ticket_in_review().await;
    world.store.merge_pull(pr).await.unwrap();
    let ours = world
        .store
        .open_pull_as(
            LINEWRIGHT,
            "ours",
            "Ours",
            "Tidies up.",
            &[("src/ours.txt", "ours\n")],
        )
        .await
        .unwrap();
    world.store.merge_pull(ours).await.unwrap();
    let spec_change = world
        .store
        .merge_change(
            "maintainer",
            "Gadgets",
            "Adds gadgets.",
            &[("docs/spec.md", "# Spec\n\nGadgets exist.\n")],
        )
        .await
        .unwrap();
    world.clock.advance(chrono::TimeDelta::days(4));
    let web = Web::start(world).await;

    let mut shown: Vec<String> = cards_shown(&web.page("/").await)
        .into_iter()
        .filter_map(|(card, _)| card.split(" | ").next().map(str::to_string))
        .collect();
    shown.sort();

    assert_eq!(
        shown,
        [
            format!("Done: {ticket}: Do the thing"),
            format!("Done: {ours}: Ours"),
        ]
    );
    let html = web.page(&format!("/items/{}", spec_change.0)).await;
    let (_, state) = item_facts(&html)
        .into_iter()
        .find(|(label, _)| label == "State")
        .unwrap();
    assert!(
        state.starts_with("Left alone · merged 2026-")
            && state.contains(", before the spec-lookback window opened 2026-"),
        "{state}"
    );
    web.stop().await;
}
