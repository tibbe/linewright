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
                String::new(),
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
