use super::*;

use linewright::domain::{
    Check, CheckState, ClosedItem, ClosingPull, Issue, IssueCloseReason, IssueClosing, Linked,
    Login, Mergeability, MergedPull, Number, Parent, Pull, PullState, Requirement, ReviewDecision,
    label,
};
use linewright::status::View;

/// Issue #`number`, which Linewright labeled `labels` as it was opened at
/// 09:00 on 2026-09-30.
fn labeled(number: u64, title: &str, labels: &[&str]) -> Issue {
    labeled_by_linewright(Issue {
        labels: labels.iter().map(ToString::to_string).collect(),
        ..ticket(number, title)
    })
}

/// `issue`, last changed on GitHub at `updated_at`.
fn changed(issue: Issue, updated_at: &str) -> Issue {
    Issue {
        updated_at: at(updated_at),
        ..issue
    }
}

/// `issue`, opened by Linewright.
fn filed(issue: Issue) -> Issue {
    Issue {
        author: Login::new(LINEWRIGHT),
        ..issue
    }
}

/// `issue`, a ticket of #`tracking`.
fn ticket_of(issue: Issue, tracking: u64) -> Issue {
    Issue {
        parent: Some(Parent {
            number: Number(tracking),
            spec_change: None,
        }),
        ..filed(issue)
    }
}

/// `issue`, assigned to `assignees`.
fn assigned(issue: Issue, assignees: &[&str]) -> Issue {
    Issue {
        assignees: assignees.iter().map(|login| Login::new(*login)).collect(),
        ..issue
    }
}

/// `issue`, closed at `closed_at` for `reason`.
fn closed(issue: &Issue, closed_at: &str, reason: Option<IssueCloseReason>) -> ClosedItem {
    ClosedItem::of_issue(
        issue,
        IssueClosing {
            at: at(closed_at),
            merged_by: None,
            reason,
        },
    )
}

fn with_closed(mut view: View, closed: Vec<ClosedItem>) -> View {
    std::sync::Arc::make_mut(&mut view.snapshot).closed = closed;
    view
}

fn with_merged(mut view: View, merged: Vec<MergedPull>) -> View {
    std::sync::Arc::make_mut(&mut view.snapshot).merged = merged;
    view
}

/// Linewright's open pull request #`number`, closing `closes`, its checks
/// in `checks`.
fn linewrights_pull(number: u64, closes: &[u64], checks: &[CheckState]) -> Pull {
    Pull {
        checks: checks
            .iter()
            .map(|&state| Check {
                name: "test".into(),
                state,
            })
            .collect(),
        ..open_pull(number, LINEWRIGHT, closes, &[LINEWRIGHT])
    }
}

/// #1 with the maintainer; #2 cut into #3, which waits on alice, asked;
/// and #4, which Linewright filed, closed.
fn a_board() -> View {
    let open = view(
        vec![
            changed(
                labeled(1, "Do the thing", &["ready-for-human"]),
                "2026-10-01T10:00:00Z",
            ),
            changed(
                Issue {
                    labels: vec![label::DESIGNED.into()],
                    sub_issues: vec![sub_issue(3, true, LINEWRIGHT)],
                    ..ticket(2, "Big thing")
                },
                "2026-10-01T11:00:00Z",
            ),
            changed(
                assigned(
                    ticket_of(labeled(3, "Part one", &["needs-info"]), 2),
                    &["alice"],
                ),
                "2026-10-01T11:00:00Z",
            ),
        ],
        vec![],
    );
    let old_idea = filed(labeled(
        4,
        "Old idea",
        &["ready-for-agent", label::DESIGNED],
    ));
    with_closed(open, vec![closed_item(&old_idea, "2026-10-01T09:00:00Z")])
}

/// `a_board`'s, and #5, cut into #6, both closed after #4.
fn a_board_with_a_finished_lane() -> View {
    let view = a_board();
    let mut closed = view.snapshot.closed.clone();
    closed.extend([
        closed_item(
            &ticket_of(labeled(6, "Only part", &["ready-for-agent"]), 5),
            "2026-10-01T09:30:00Z",
        ),
        closed_item(
            &Issue {
                labels: vec![label::DESIGNED.into()],
                sub_issues: vec![sub_issue(6, false, LINEWRIGHT)],
                ..ticket(5, "Small thing")
            },
            "2026-10-01T09:40:00Z",
        ),
    ]);
    with_closed(view, closed)
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

/// The board's cards, each as its column and what a reader reads of it,
/// and whether it carries the pull request icon.
fn cards_shown(html: &Html) -> Vec<(String, bool)> {
    board_shown(html)
        .into_iter()
        .flat_map(|lane| lane.into_iter().skip(1))
        .zip(select(html, "#board .card").into_iter().map(|card| {
            card.select(&Selector::parse("h3 svg.pull-request").unwrap())
                .count()
                == 1
        }))
        .collect()
}

/// The board's cards, as [`cards_shown`], in their text's order.
fn sorted_cards(html: &Html) -> Vec<String> {
    let mut shown: Vec<String> = cards_shown(html)
        .into_iter()
        .map(|(card, _)| card)
        .collect();
    shown.sort();
    shown
}

/// The headings of the cards in `tone`, in their text's order.
fn toned(html: &Html, tone: &str) -> Vec<String> {
    let mut toned: Vec<String> = select(html, "#board .card")
        .into_iter()
        .filter(|card| has_classes(*card, &["toned", tone]))
        .map(|card| text(select_in(card, "h3")[0]))
        .collect();
    toned.sort();
    toned
}

#[tokio::test]
async fn the_board_shows_the_cards_in_their_columns_and_lanes_each_with_its_line() {
    let pages = Pages::of(a_board()).await;

    let html = pages.page("/").await;

    assert_eq!(
        board_shown(&html),
        [
            vec![
                "#2: Big thing — 0/1 tickets closed",
                "Needs a person: #3: Part one | asked alice · 2026-09-30 09:00:00 UTC",
            ],
            vec![
                "#1: Do the thing",
                "Needs a person: #1: Do the thing | decision needed · 2026-09-30 09:00:00 UTC",
            ],
            vec![
                "#4: Old idea — finished 2026-10-01 09:00:00 UTC",
                "Done: #4: Old idea | closed as completed · 2026-10-01 09:00:00 UTC",
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
}

#[tokio::test]
async fn a_card_waiting_on_the_people_asked_names_them_as_a_list_or_says_people_asked() {
    let asking =
        |number, title, people: &[&str]| assigned(labeled(number, title, &["needs-info"]), people);
    let pages = Pages::of(view(
        vec![
            asking(1, "One", &["alice"]),
            asking(2, "Two", &["alice", "bob"]),
            asking(3, "Three", &["alice", "bob", "carol"]),
            asking(4, "Nobody", &[]),
        ],
        vec![],
    ))
    .await;

    let shown = sorted_cards(&pages.page("/").await);

    assert_eq!(
        shown,
        [
            "Needs a person: #1: One | asked alice · 2026-09-30 09:00:00 UTC",
            "Needs a person: #2: Two | asked alice and bob · 2026-09-30 09:00:00 UTC",
            "Needs a person: #3: Three | asked alice, bob and carol · 2026-09-30 09:00:00 UTC",
            "Needs a person: #4: Nobody | people asked · 2026-09-30 09:00:00 UTC",
        ]
    );
}

// Paused, so that the stream samples the view at once.
#[tokio::test(start_paused = true)]
async fn the_boards_stream_sends_the_frame_the_tab_title_and_the_board_then_those_a_new_view_changes()
 {
    let pages = Pages::of(a_board()).await;
    let mut events = stream(&pages, "/events/").await;
    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    let mut moved = a_board();
    std::sync::Arc::make_mut(&mut moved.snapshot)
        .issues
        .retain(|issue| issue.number != Number(1));
    pages.publish(moved);
    let next = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    let first: Vec<String> = regions(&first).into_iter().map(|(id, _)| id).collect();
    assert_eq!(first, ["holds", "title", "board"]);
    let changed = regions(&next);
    let ids: Vec<&str> = changed.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["title", "board"]);
    assert_eq!(
        text(select(&changed[0].1, "title")[0]),
        "(1) Board · owner/widgets"
    );
    let lanes: Vec<String> = board_shown(&changed[1].1)
        .into_iter()
        .map(|lane| lane[0].clone())
        .collect();
    assert_eq!(
        lanes,
        [
            "#2: Big thing — 0/1 tickets closed",
            "#4: Old idea — finished 2026-10-01 09:00:00 UTC",
        ]
    );
}

#[tokio::test]
async fn the_boards_stream_sends_the_board_after_a_delivery_moves_a_card() {
    let web = Web::start(with_ticket().await).await;
    let mut events = stream(&web, "/events/").await;
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    web.store
        .assign_as(LINEWRIGHT, Number(1), LINEWRIGHT)
        .unwrap();
    web.store.close_issue_as_completed(Number(1)).unwrap();
    web.deliver_issue(1).await;
    let board = loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let Some((_, board)) = regions(&event).into_iter().find(|(id, _)| id == "board") {
            break board;
        }
    };

    let columns: Vec<String> = board_shown(&board)
        .into_iter()
        .flat_map(|lane| lane.into_iter().skip(1))
        .filter_map(|card| {
            card.split_once(": #1:")
                .map(|(column, _)| column.to_string())
        })
        .collect();
    assert_eq!(columns, ["Done"]);
    web.stop().await;
}

#[tokio::test]
async fn the_board_folds_its_finished_lanes_under_one_closed_fold_after_the_rest() {
    let pages = Pages::of(a_board_with_a_finished_lane()).await;

    let html = pages.page("/").await;

    assert_eq!(
        board_shown(&html),
        [
            vec![
                "#2: Big thing — 0/1 tickets closed",
                "Needs a person: #3: Part one | asked alice · 2026-09-30 09:00:00 UTC",
            ],
            vec![
                "#1: Do the thing",
                "Needs a person: #1: Do the thing | decision needed · 2026-09-30 09:00:00 UTC",
            ],
            vec![
                "#5: Small thing — 1/1 tickets closed — finished 2026-10-01 09:40:00 UTC",
                "Done: #6: Only part | closed as completed · 2026-10-01 09:30:00 UTC",
            ],
            vec![
                "#4: Old idea — finished 2026-10-01 09:00:00 UTC",
                "Done: #4: Old idea | closed as completed · 2026-10-01 09:00:00 UTC",
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
    // A time the page's script shows relative.
    let finished: Vec<&str> = select(&html, "#lane-5 > summary time")
        .into_iter()
        .filter_map(|time| time.attr("datetime"))
        .collect();
    assert_eq!(finished, ["2026-10-01T09:40:00Z"]);
}

#[tokio::test]
async fn a_lane_head_and_a_card_title_cut_to_fit_are_whole_in_the_pages_tooltip_not_in_place() {
    let pages = Pages::of(a_board_with_a_finished_lane()).await;

    let html = pages.page("/").await;

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
    assert_eq!(titles.len(), 4);
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
}

#[tokio::test]
async fn the_board_shows_what_a_hover_shows_in_the_pages_tooltip_never_in_a_title() {
    let pages = Pages::of(a_board()).await;

    let html = pages.page("/").await;

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
    // Each card's, and the finished lane's.
    assert_eq!(times.len(), 4);
    for time in times {
        assert_eq!(time.attr("tabindex"), Some("0"), "{}", time.html());
    }
}

#[tokio::test]
async fn each_lane_is_a_surface_sunk_into_the_page_with_its_cards_raised_on_it() {
    let pages = Pages::of(a_board_with_a_finished_lane()).await;

    let html = pages.page("/").await;

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
    let pages = Pages::of(a_board_with_a_finished_lane()).await;

    let html = pages.page("/").await;

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
}

/// `docs/specs/web/design_system.md`, Status: paused or queued is Muted
/// text.
const PAUSED_OR_QUEUED: &str = "[--tone:var(--color-muted)]";

#[tokio::test]
async fn a_card_paused_for_the_usage_limit_shows_it_in_muted_text() {
    let claimed = assigned(ticket(1, "Do the thing"), &[LINEWRIGHT]);
    let pages = Pages::of(View {
        memory: limited_until("2026-10-01T13:00:00Z"),
        ..view(vec![claimed], vec![])
    })
    .await;

    let html = pages.page("/").await;

    assert_eq!(
        sorted_cards(&html),
        ["Working: #1: Do the thing | paused for the usage limit"]
    );
    let card = select(&html, "#board .card")[0];
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
}

/// `docs/specs/web/design_system.md`, Colour scheme: secondary text is in
/// Muted text.
#[tokio::test]
async fn a_cards_line_under_its_title_is_in_muted_text() {
    let pages = Pages::of(a_board()).await;

    let html = pages.page("/").await;

    assert_in_muted_text(&html, "#board .card .line");
}

#[tokio::test]
async fn a_card_with_a_card_status_shows_it_and_a_card_in_done_is_faded() {
    let mut view = a_board_with_a_finished_lane();
    std::sync::Arc::make_mut(&mut view.snapshot)
        .issues
        .push(Issue {
            blocked_by: vec![Linked {
                number: Number(7),
                open: true,
            }],
            ..ticket(8, "Blocked")
        });
    let pages = Pages::of(view).await;

    let html = pages.page("/").await;

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
}

#[tokio::test]
async fn a_board_whose_lanes_are_all_finished_shows_the_column_heads_and_one_closed_fold() {
    let done = labeled(1, "Do the thing", &["ready-for-human"]);
    let pages = Pages::of(with_closed(
        view(vec![], vec![]),
        vec![closed_item(&done, "2026-10-01T09:00:00Z")],
    ))
    .await;

    let html = pages.page("/").await;

    let shown: Vec<String> = select(&html, "#board .board > *")
        .into_iter()
        .map(|element| {
            let fold = element.value().attr("open").map_or("", |_| " open");
            format!("{}{fold}: {}", element.value().name(), text(element))
        })
        .collect();
    assert_eq!(
        shown,
        [
            "div: Triage Needs a person Ready Working In review Done",
            "details: 1 finished lane #1: Do the thing — finished \
             2026-10-01 09:00:00 UTC #1: Do the thing closed as completed · 2026-10-01 09:00:00 UTC",
        ]
    );
    let lanes: Vec<Option<&str>> = select(&html, "#finished details")
        .into_iter()
        .map(|fold| fold.attr("id"))
        .collect();
    assert_eq!(lanes, [Some("lane-1")]);
}

/// `issue`, closed by Linewright's pull request #`number` into `main`, as
/// `state` and last changed at `at`.
fn closed_by_linewrights(issue: Issue, number: u64, state: PullState, updated_at: &str) -> Issue {
    Issue {
        closed_by: vec![ClosingPull {
            number: Number(number),
            author: Login::new(LINEWRIGHT),
            state,
            updated_at: at(updated_at),
            base_ref: "main".into(),
        }],
        ..issue
    }
}

#[tokio::test]
async fn an_issue_in_review_shows_its_pull_request_and_once_it_closes_unmerged_that_it_was_handed_back()
 {
    let pages = Pages::of(view(
        vec![
            assigned(ticket(1, "Do the thing"), &[LINEWRIGHT]),
            closed_by_linewrights(
                labeled(3, "Do another thing", &["ready-for-human"]),
                4,
                PullState::Closed {
                    at: at("2026-10-01T10:00:00Z"),
                },
                "2026-10-01T10:00:00Z",
            ),
            labeled(5, "Do a third thing", &["ready-for-human"]),
        ],
        vec![linewrights_pull(2, &[1], &[CheckState::Pending])],
    ))
    .await;
    pages.record(&serde_json::json!({
        "item": 5,
        "stage": "implement",
        "n": 1,
        "started_at": "2026-10-01T09:30:00Z",
        "end": {"at": "2026-10-01T10:00:00Z", "ending": "handed-back"},
        "sessions": [],
    }));

    let html = pages.page("/").await;

    assert_eq!(
        sorted_cards(&html),
        [
            "In review: #1: Do the thing | #2 · checks running",
            "Needs a person: #3: Do another thing | handed back · 2026-09-30 09:00:00 UTC",
            "Needs a person: #5: Do a third thing | handed back · 2026-09-30 09:00:00 UTC",
        ]
    );
    let links: Vec<&str> = select(&html, "#board .card .line a:not(.github)")
        .into_iter()
        .filter_map(|a| a.attr("href"))
        .collect();
    assert_eq!(links, ["/items/2"]);
    // `docs/specs/design_system.md`, Cards: handed back is waiting for a
    // person.
    assert_eq!(
        toned(&html, WAITING_FOR_A_PERSON),
        ["#3: Do another thing", "#5: Do a third thing"]
    );
}

#[tokio::test]
async fn a_stalled_card_says_stalled_in_needs_a_person_and_after_its_pull_request_in_review() {
    let pages = Pages::of(view(
        vec![
            labeled(1, "Do another thing", &["ready-for-human", label::STALLED]),
            assigned(ticket(2, "Do the thing"), &[LINEWRIGHT]),
        ],
        vec![Pull {
            labels: vec![label::STALLED.into()],
            ..linewrights_pull(3, &[2], &[])
        }],
    ))
    .await;

    let shown = sorted_cards(&pages.page("/").await);

    assert_eq!(
        shown,
        [
            "In review: #2: Do the thing | #3 · stalled",
            "Needs a person: #1: Do another thing | stalled · 2026-09-30 09:00:00 UTC",
        ]
    );
}

#[tokio::test]
async fn a_merged_pull_request_of_linewrights_closing_no_issue_is_a_pull_request_card_in_done() {
    let pages = Pages::of(with_merged(
        view(vec![], vec![]),
        vec![merged_pull(5, LINEWRIGHT, &[])],
    ))
    .await;

    let shown = cards_shown(&pages.page("/").await);

    assert_eq!(
        shown,
        [(
            "Done: #5: Merged 5 | merged · 2026-10-01 10:00:00 UTC".to_string(),
            true
        )]
    );
}

#[tokio::test]
async fn an_issue_whose_pull_request_merged_is_in_done_before_github_closes_it() {
    let merged = closed_by_linewrights(
        labeled(1, "Do the thing", &["ready-for-agent", label::DESIGNED]),
        11,
        PullState::Merged {
            at: at("2026-09-30T10:00:00Z"),
        },
        "2026-09-30T10:00:00Z",
    );
    let pages = Pages::of(view(vec![merged], vec![])).await;

    let shown = cards_shown(&pages.page("/").await);

    assert_eq!(
        shown,
        [(
            "Done: #1: Do the thing | closed in #11 · 2026-09-30 10:00:00 UTC".to_string(),
            false
        )]
    );
}

#[tokio::test]
async fn an_issue_closed_without_a_merge_shows_githubs_reason_for_closing_it() {
    use IssueCloseReason::{Completed, Duplicate, NotPlanned};
    let closing = |number, title, reason| {
        closed(
            &filed(labeled(
                number,
                title,
                &["ready-for-agent", label::DESIGNED],
            )),
            &format!("2026-10-01T09:0{number}:00Z"),
            reason,
        )
    };
    let pages = Pages::of(with_closed(
        view(vec![], vec![]),
        vec![
            closing(1, "Done it", Some(Completed)),
            closing(2, "Never mind", Some(NotPlanned)),
            closing(3, "Same as another", Some(Duplicate)),
            closing(4, "Gone", None),
        ],
    ))
    .await;

    let shown = sorted_cards(&pages.page("/").await);

    assert_eq!(
        shown,
        [
            "Done: #1: Done it | closed as completed · 2026-10-01 09:01:00 UTC",
            "Done: #2: Never mind | closed as not planned · 2026-10-01 09:02:00 UTC",
            "Done: #3: Same as another | closed as a duplicate · 2026-10-01 09:03:00 UTC",
            "Done: #4: Gone | closed · 2026-10-01 09:04:00 UTC",
        ]
    );
}

#[tokio::test]
async fn a_pull_request_with_nothing_else_stopping_it_merging_ends_its_line_with_its_review_decision()
 {
    let decided = |number, review_decision| Pull {
        mergeability: Mergeability::Clean,
        review_decision,
        ..linewrights_pull(number, &[number - 10], &[CheckState::Passed])
    };
    let pages = Pages::of(view(
        (1..=4)
            .map(|n| assigned(ticket(n, "Do the thing"), &[LINEWRIGHT]))
            .collect(),
        vec![
            decided(11, None),
            decided(12, Some(ReviewDecision::ReviewRequired)),
            decided(13, Some(ReviewDecision::ChangesRequested)),
            decided(14, Some(ReviewDecision::Approved)),
        ],
    ))
    .await;

    let shown = sorted_cards(&pages.page("/").await);

    assert_eq!(
        shown,
        [
            "In review: #1: Do the thing | #11",
            "In review: #2: Do the thing | #12 · review required",
            "In review: #3: Do the thing | #13 · changes requested",
            "In review: #4: Do the thing | #14 · approved",
        ]
    );
}

#[tokio::test]
async fn a_pull_request_blocked_on_a_code_owners_review_with_no_review_decision_says_review_required()
 {
    let blocked = |number, closes| Pull {
        mergeability: Mergeability::Blocked {
            requires: vec![Requirement::CodeOwnerReview {
                owners: vec!["maintainer".into()],
            }],
        },
        ..linewrights_pull(number, &[closes], &[CheckState::Passed])
    };
    let pages = Pages::of(view(
        vec![
            assigned(ticket(1, "Do the thing"), &[LINEWRIGHT]),
            assigned(ticket(2, "Do another thing"), &[LINEWRIGHT]),
        ],
        vec![
            blocked(11, 1),
            linewrights_pull(12, &[2], &[CheckState::Pending]),
            // Closes no issue on the board, so it is a card of its own.
            blocked(13, 9),
        ],
    ))
    .await;

    let html = pages.page("/").await;

    assert_eq!(
        sorted_cards(&html),
        [
            "In review: #13: Pull 13 | review required",
            "In review: #1: Do the thing | #11 · review required",
            "In review: #2: Do another thing | #12 · checks running",
        ]
    );
    // `docs/specs/design_system.md`, Cards: "review required" is waiting for
    // a person.
    assert_eq!(
        toned(&html, WAITING_FOR_A_PERSON),
        ["#13: Pull 13", "#1: Do the thing"]
    );
}

#[tokio::test]
async fn a_card_waiting_in_its_column_ends_its_line_with_when_it_entered_its_column() {
    let opened_at = |issue: Issue, when| {
        labeled_by_linewright(Issue {
            created_at: at(when),
            updated_at: at(when),
            ..issue
        })
    };
    let pages = Pages::of(view(
        vec![
            opened_at(
                Issue {
                    labels: vec!["needs-triage".into()],
                    ..ticket(1, "Export fails")
                },
                "2026-10-01T08:00:00Z",
            ),
            opened_at(
                Issue {
                    labels: vec!["ready-for-human".into()],
                    ..ticket(2, "First")
                },
                "2026-10-01T09:00:00Z",
            ),
            opened_at(
                Issue {
                    blocked_by: vec![Linked {
                        number: Number(2),
                        open: true,
                    }],
                    ..ticket(3, "Then")
                },
                "2026-10-01T10:00:00Z",
            ),
        ],
        vec![],
    ))
    .await;

    let html = pages.page("/").await;

    assert_eq!(
        sorted_cards(&html),
        [
            "Needs a person: #2: First | decision needed · 2026-10-01 09:00:00 UTC",
            "Ready: #3: Then | blocked by #2 · 2026-10-01 10:00:00 UTC",
            "Triage: #1: Export fails | 2026-10-01 08:00:00 UTC",
        ]
    );
    let mut entered: Vec<&str> = select(&html, "#board .card .line time[datetime]")
        .into_iter()
        .filter_map(|time| time.attr("datetime"))
        .collect();
    entered.sort_unstable();
    assert_eq!(
        entered,
        [
            "2026-10-01T08:00:00Z",
            "2026-10-01T09:00:00Z",
            "2026-10-01T10:00:00Z"
        ]
    );
    assert!(
        select(&html, "#board .card .line time[data-since]").is_empty(),
        "no card's wait ticks as a duration"
    );
}

#[tokio::test]
async fn a_card_linewright_was_unassigned_from_says_so_and_its_own_pull_request_leaves_off_its_number()
 {
    let let_go = Issue {
        events: vec![
            assigning_linewright("2026-10-01T09:00:00Z"),
            unassigning_linewright("2026-10-01T10:00:00Z"),
        ],
        ..ticket(1, "Let go")
    };
    let pages = Pages::of(view(
        vec![let_go],
        // Closes no issue on the board, so it is a card of its own.
        vec![linewrights_pull(5, &[9], &[CheckState::Pending])],
    ))
    .await;

    let shown = sorted_cards(&pages.page("/").await);

    assert_eq!(
        shown,
        [
            "In review: #5: Pull 5 | checks running",
            "Ready: #1: Let go | unassigned · 2026-10-01 10:00:00 UTC",
        ]
    );
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
