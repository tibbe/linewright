use super::*;

/// #1 with the maintainer; #2 cut into #3, which waits on the reporter;
/// and #4, closed.
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
    world.store.add_sub_issue(tracking, part).await.unwrap();
    let closed = world.store.open_issue(
        "maintainer",
        "Old idea",
        "Maybe.",
        &["ready-for-agent", "linewright:designed"],
    );
    world.store.close_issue_as(closed).unwrap();
    world
}

/// Each lane's heading, then each card as its column and its lines, as a
/// reader reads them.
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
                            .select(&Selector::parse("h3, .standing, .line li, .pull").unwrap())
                            .map(text)
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
async fn the_board_shows_the_cards_in_their_columns_and_lanes_worded_as_explain_words_them() {
    let web = Web::start(with_a_board().await).await;

    let html = web.page("/").await;

    assert_eq!(
        board_shown(&html),
        [
            vec![
                "#2: Big thing — 0/1 tickets closed".to_string(),
                "Needs a person: #3: Part one | waiting on the reporter \
                 | waiting for: the reporter: labeled `needs-info`"
                    .to_string(),
            ],
            vec![
                "No tracking issue — 2 cards".to_string(),
                "Needs a person: #1: Do the thing | with the maintainer \
                 | waiting for: the maintainer: to label it `ready-for-agent` or `needs-triage`"
                    .to_string(),
                "Done: #4: Old idea | closed issue | closed".to_string(),
            ],
        ]
    );
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
async fn the_boards_stream_sends_the_board_and_the_tab_title_after_a_poll_moves_a_card() {
    let web = Web::start(with_a_board().await).await;
    let mut events = stream(&web, "/events/").await;
    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<String> = regions(&first).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, ["bar", "title", "board"]);

    web.store
        .close_issue_as(linewright::domain::Number(1))
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
    let rest = board_shown(board).pop().unwrap();
    assert!(
        rest.contains(&"Done: #1: Do the thing | closed issue | closed".to_string()),
        "{rest:?}"
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
    world.store.close_issue_as(part).unwrap();
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
                "Needs a person: #3: Part one | waiting on the reporter \
                 | waiting for: the reporter: labeled `needs-info`"
                    .to_string(),
            ],
            vec![
                "No tracking issue — 2 cards".to_string(),
                "Needs a person: #1: Do the thing | with the maintainer \
                 | waiting for: the maintainer: to label it `ready-for-agent` or `needs-triage`"
                    .to_string(),
                "Done: #4: Old idea | closed issue | closed".to_string(),
            ],
            vec![
                "#5: Small thing — 1/1 tickets closed — finished 2026-10-01 12:00:SS UTC"
                    .to_string(),
                "Done: #6: Only part | closed issue | closed".to_string(),
            ],
        ]
    );
    let folds: Vec<(Option<&str>, bool)> = select(&html, "#board details")
        .into_iter()
        .map(|fold| (fold.attr("id"), fold.attr("open").is_some()))
        .collect();
    assert_eq!(folds, [(Some("finished"), false), (Some("lane-5"), false)]);
    let summaries: Vec<String> = select(&html, "#finished > summary")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(summaries, ["1 finished lane"]);
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
async fn a_board_whose_lanes_are_all_finished_shows_the_column_heads_and_one_closed_fold() {
    let world = with_ticket().await;
    world
        .store
        .close_issue_as(linewright::domain::Number(1))
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
            "h2: Triage",
            "h2: Needs a person",
            "h2: Ready",
            "h2: Working",
            "h2: In review",
            "h2: Done",
            "details: 1 finished lane No tracking issue — 1 card — finished \
             2026-10-01 12:00:SS UTC #1: Do the thing closed issue closed",
        ]
    );
    let lanes: Vec<Option<&str>> = select(&html, "#finished details")
        .into_iter()
        .map(|fold| fold.attr("id"))
        .collect();
    assert_eq!(lanes, [Some("lane-none")]);
    web.stop().await;
}
