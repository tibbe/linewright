use linewright_core::domain::{
    Check, CheckState, ClosedItem, Issue, Linked, Login, Mergeability, MergedPull, Number, Parent,
    Pull, Requirement, label,
};
use linewright_core::status::View;

use super::look::assert_drawn_as_a_card;
use super::*;

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
            &labeled_by_linewright(Issue {
                labels: vec![label::DESIGNED.into()],
                sub_issues: vec![sub_issue(6, false, LINEWRIGHT)],
                ..ticket(5, "Small thing")
            }),
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

/// The icon beside a card's number (`docs/specs/board.md`, Cards, Issue or
/// pull request), and what a screen reader reads of it.
#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Issue,
    PullRequest,
}

/// The board's cards, each as its column and what a reader reads of it,
/// and the icon beside its number.
fn cards_shown(html: &Html) -> Vec<(String, Kind)> {
    board_shown(html)
        .into_iter()
        .flat_map(|lane| lane.into_iter().skip(1))
        .zip(select(html, "#board .card").into_iter().map(|card| {
            let icons = select_in(card, "h3 > svg");
            assert_eq!(icons.len(), 1, "{}", card.html());
            match (icons[0].attr("data-icon"), icons[0].attr("aria-label")) {
                (Some("issue-opened"), Some("issue")) => Kind::Issue,
                (Some("git-pull-request"), Some("pull request")) => Kind::PullRequest,
                _ => panic!("not an issue's or a pull request's icon: {}", card.html()),
            }
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

#[test_log::test(tokio::test)]
async fn the_board_shows_the_cards_in_their_columns_and_lanes_each_with_its_line() {
    let pages = Pages::of(a_board()).await;

    let html = pages.page("/").await;

    assert_eq!(
        board_shown(&html),
        [
            vec![
                "Big thing #2 — 0/1 tickets closed",
                "Needs a person: Part one #3 | asked alice · 2026-09-30 09:00:00 UTC",
            ],
            vec![
                "Do the thing #1",
                "Needs a person: Do the thing #1 | decision needed · 2026-09-30 09:00:00 UTC",
            ],
            vec![
                "Old idea #4 — finished 2026-10-01 09:00:00 UTC",
                "Done: Old idea #4 | closed as completed · 2026-10-01 09:00:00 UTC",
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
    let name = |title: &str, n: u64| {
        vec![
            (title.to_string(), format!("/items/{n}")),
            (
                format!("#{n}"),
                format!("https://github.com/owner/widgets/issues/{n}"),
            ),
        ]
    };
    let cards: Vec<Vec<(String, String)>> = select(&html, "#board .card h3")
        .into_iter()
        .map(links_in)
        .collect();
    assert_eq!(
        cards,
        [
            name("Part one", 3),
            name("Do the thing", 1),
            name("Old idea", 4)
        ]
    );
    let heads: Vec<Vec<(String, String)>> = select(&html, "#board .lane-head")
        .into_iter()
        .map(links_in)
        .collect();
    assert_eq!(
        heads,
        [
            name("Big thing", 2),
            name("Do the thing", 1),
            name("Old idea", 4)
        ]
    );
    let head_icons: Vec<Option<&str>> = select(&html, "#board .lane-head svg:first-child")
        .into_iter()
        .map(|icon| icon.attr("data-icon"))
        .collect();
    assert_eq!(head_icons, [Some("issue-opened"); 3]);
    let title = select(&html, "title");
    assert_eq!(text(title[0]), "(2) Board · Linewright");
    assert_no_status_by_colour_alone(&html);
}

#[test_log::test(tokio::test)]
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
    assert_eq!(
        first,
        [
            "holds", "title", "crumbs", "heading", "buttons", "board", "people"
        ]
    );
    let changed = regions(&next);
    let ids: Vec<&str> = changed.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["title", "board"]);
    assert_eq!(
        text(select(&changed[0].1, "title")[0]),
        "(1) Board · Linewright"
    );
    let lanes: Vec<String> = board_shown(&changed[1].1)
        .into_iter()
        .map(|lane| lane[0].clone())
        .collect();
    assert_eq!(
        lanes,
        [
            "Big thing #2 — 0/1 tickets closed",
            "Old idea #4 — finished 2026-10-01 09:00:00 UTC",
        ]
    );
}

/// The Person `select`'s options, each its value and its text, and
/// ` (chosen)` where it is chosen.
fn person_select(html: &Html) -> Vec<String> {
    let selects = select(html, "form[data-filters] select[name=person]");
    assert_eq!(selects.len(), 1, "one Person select");
    select(html, "form[data-filters] select[name=person] option")
        .into_iter()
        .map(|option| {
            let chosen = if option.attr("selected").is_some() {
                " (chosen)"
            } else {
                ""
            };
            format!(
                "{}: {}{chosen}",
                option.attr("value").unwrap_or_default(),
                text(option)
            )
        })
        .collect()
}

/// The Person filter (`docs/specs/board.md`, Filtering by person):
/// alice, asked on #3, is party to it alone, and the maintainer, who opened
/// #1, #2 and so #3, to all three. Linewright, who filed #4, is party to
/// nothing.
#[test_log::test(tokio::test)]
async fn the_board_narrowed_to_a_person_shows_their_cards_and_counts_the_whole_board_in_its_title()
{
    let pages = Pages::of(a_board()).await;

    let html = pages.page("/?person=alice").await;

    assert_eq!(
        board_shown(&html),
        [vec![
            "Big thing #2 — 0/1 tickets closed",
            "Needs a person: Part one #3 | asked alice · 2026-09-30 09:00:00 UTC",
        ]]
    );
    assert_eq!(select(&html, "#finished").len(), 0);
    assert_eq!(text(select(&html, "title")[0]), "(2) Board · Linewright");
    assert_eq!(
        person_select(&html),
        [
            ": All people",
            "alice: alice (chosen)",
            "maintainer: maintainer",
        ]
    );
    assert_eq!(select(&html, "form[data-filters] input").len(), 0);
    assert_eq!(
        text(select(&html, "form[data-filters] label > .label")[0]),
        "Person"
    );
    let source = select(&html, "form[data-filters] select")[0]
        .attr("data-options")
        .unwrap();
    let people: Vec<&str> = select(&html, &format!("datalist#{source} option"))
        .into_iter()
        .filter_map(|option| option.attr("value"))
        .collect();
    assert_eq!(people, ["alice", "maintainer"]);
    assert_eq!(events_path(&html), Some("/events/?person=alice"));
    assert_eq!(
        select(&html, "form[data-filters]")[0].attr("data-filters"),
        Some("/events/")
    );
    assert_eq!(select(&html, "#board form, #people form").len(), 0);
}

/// A ticket's card matches whoever is party to its tracking issue
/// (`docs/specs/board.md`, Filtering by person).
#[test_log::test(tokio::test)]
async fn the_board_narrowed_to_a_tracking_issues_assignee_shows_its_lane() {
    let mut view = a_board();
    let snapshot = std::sync::Arc::make_mut(&mut view.snapshot);
    let tracking = &mut snapshot.issues[1];
    *tracking = assigned(tracking.clone(), &["carol"]);
    let pages = Pages::of(view).await;

    let html = pages.page("/?person=carol").await;

    assert_eq!(
        board_shown(&html),
        [vec![
            "Big thing #2 — 0/1 tickets closed",
            "Needs a person: Part one #3 | asked alice · 2026-09-30 09:00:00 UTC",
        ]]
    );
    let people: Vec<&str> = select(&html, "datalist#people option")
        .into_iter()
        .filter_map(|option| option.attr("value"))
        .collect();
    assert_eq!(people, ["alice", "carol", "maintainer"]);
}

/// `docs/specs/board.md`, Filtering by person, The people: whoever is chosen is
/// listed, though no card matches them.
#[test_log::test(tokio::test)]
async fn a_chosen_person_no_card_matches_is_listed_and_chosen() {
    let pages = Pages::of(a_board()).await;

    let html = pages.page("/?person=Carol").await;

    assert_eq!(board_shown(&html).len(), 0);
    assert_eq!(
        person_select(&html),
        [
            ": All people",
            "alice: alice",
            "Carol: Carol (chosen)",
            "maintainer: maintainer",
        ]
    );

    let empty = Pages::of(view(vec![], vec![])).await;
    let on_empty = empty.page("/?person=carol").await;

    assert_eq!(
        person_select(&on_empty),
        [": All people", "carol: carol (chosen)",]
    );
    let people: Vec<&str> = select(&on_empty, "datalist#people option")
        .into_iter()
        .filter_map(|option| option.attr("value"))
        .collect();
    assert_eq!(people, ["carol"]);
}

/// A chosen person some card matches is listed once, however their login
/// is cased.
#[test_log::test(tokio::test)]
async fn a_chosen_person_some_card_matches_is_listed_once() {
    let pages = Pages::of(a_board()).await;

    let html = pages.page("/?person=ALICE").await;

    assert_eq!(
        person_select(&html),
        [
            ": All people",
            "alice: alice (chosen)",
            "maintainer: maintainer"
        ]
    );
}

#[test_log::test(tokio::test)]
async fn the_board_with_no_person_or_a_blank_one_shows_every_card() {
    let pages = Pages::of(a_board()).await;

    for path in ["/", "/?person=", "/?person=%20"] {
        let html = pages.page(path).await;

        assert_eq!(board_shown(&html).len(), 3, "{path}");
        assert_eq!(
            person_select(&html),
            [": All people", "alice: alice", "maintainer: maintainer"],
            "{path}"
        );
        assert_eq!(events_path(&html), Some("/events/"), "{path}");
    }
}

#[test_log::test(tokio::test)]
async fn the_boards_stream_narrowed_to_a_person_sends_their_cards_and_the_people() {
    let pages = Pages::of(a_board()).await;
    let mut events = stream(&pages, "/events/?person=alice").await;

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();

    let first = regions(&first);
    let ids: Vec<&str> = first.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "holds", "title", "crumbs", "heading", "buttons", "board", "people"
        ]
    );
    let cards: Vec<String> = select(region_of(&first, "board"), "#board .card h3")
        .into_iter()
        .map(text)
        .collect();
    assert_eq!(cards, ["Part one #3"]);
    let people: Vec<&str> = select(region_of(&first, "people"), "datalist#people option")
        .into_iter()
        .filter_map(|option| option.attr("value"))
        .collect();
    assert_eq!(people, ["alice", "maintainer"]);
}

#[test_log::test(tokio::test)]
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
            card.split_once(": ")
                .filter(|(_, card)| {
                    card.split(" | ")
                        .next()
                        .is_some_and(|name| name.ends_with(" #1"))
                })
                .map(|(column, _)| column.to_string())
        })
        .collect();
    assert_eq!(columns, ["Done"]);
    web.stop().await;
}

#[test_log::test(tokio::test)]
async fn the_board_folds_its_finished_lanes_under_one_closed_fold_after_the_rest() {
    let pages = Pages::of(a_board_with_a_finished_lane()).await;

    let html = pages.page("/").await;

    assert_eq!(
        board_shown(&html),
        [
            vec![
                "Big thing #2 — 0/1 tickets closed",
                "Needs a person: Part one #3 | asked alice · 2026-09-30 09:00:00 UTC",
            ],
            vec![
                "Do the thing #1",
                "Needs a person: Do the thing #1 | decision needed · 2026-09-30 09:00:00 UTC",
            ],
            vec![
                "Small thing #5 — 1/1 tickets closed — finished 2026-10-01 09:40:00 UTC",
                "Done: Only part #6 | closed as completed · 2026-10-01 09:30:00 UTC",
            ],
            vec![
                "Old idea #4 — finished 2026-10-01 09:00:00 UTC",
                "Done: Old idea #4 | closed as completed · 2026-10-01 09:00:00 UTC",
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
    let collapses: Vec<Option<&str>> = select(&html, "#board details")
        .into_iter()
        .filter(|fold| has_classes(*fold, &["collapse", "collapse-arrow"]))
        .filter(|fold| select_in(*fold, ":scope > summary.collapse-title").len() == 1)
        .filter(|fold| select_in(*fold, ":scope > .collapse-content").len() == 1)
        .map(|fold| fold.attr("id"))
        .collect();
    assert_eq!(
        collapses,
        [Some("finished"), Some("lane-5"), Some("lane-4")],
        "each fold a daisyUI collapse, with collapse-arrow"
    );
    assert_eq!(
        select(&html, "#board .divider + #finished").len(),
        1,
        "the finished lanes under a divider"
    );
    let finished: Vec<&str> = select(&html, "#lane-5 > summary time")
        .into_iter()
        .filter_map(|time| time.attr("datetime"))
        .collect();
    assert_eq!(finished, ["2026-10-01T09:40:00Z"]);
}

#[test_log::test(tokio::test)]
async fn a_lane_head_and_a_card_title_cut_to_fit_cut_only_the_title_and_are_whole_in_the_pages_tooltip_not_in_place()
 {
    let pages = Pages::of(a_board_with_a_finished_lane()).await;

    let html = pages.page("/").await;

    let heads = select(&html, "#board .lane-head");
    assert_eq!(heads.len(), 4);
    for head in heads {
        assert!(lacks_classes(head, &["group"]), "{}", head.html());
        assert_cuts_only_the_title(head);
        let line = select_in(head, ".inline-flex")[0];
        assert!(has_classes(line, &["items-center"]), "{}", head.html());
        let tails = select_in(line, ":scope > .tail");
        for tail in tails {
            assert!(has_classes(tail, &["shrink-0"]), "{}", head.html());
        }
        assert_eq!(grows_in_place(line), Vec::<&str>::new(), "{}", head.html());
    }
    let titles = select(&html, "#board .card h3");
    assert_eq!(titles.len(), 4);
    for title in titles {
        assert!(has_classes(title, &["card-title"]), "{}", title.html());
        assert_cuts_only_the_title(title);
        assert_eq!(
            grows_in_place(title),
            Vec::<&str>::new(),
            "{}",
            title.html()
        );
    }
}

#[test_log::test(tokio::test)]
async fn the_board_shows_what_a_hover_shows_in_the_pages_tooltip_never_in_a_title() {
    let pages = Pages::of(a_board()).await;

    let html = pages.page("/").await;

    assert_eq!(select(&html, "[title]").len(), 0);
    let times = select(&html, "#board time[datetime]");
    let each_cards_and_the_finished_lanes = 4;
    assert_eq!(times.len(), each_cards_and_the_finished_lanes);
    for time in times {
        assert_eq!(time.attr("tabindex"), Some("0"), "{}", time.html());
    }
}

/// `docs/specs/board.md`, Board: the column heads are a row in
/// `bg-base-100`, `h-8` tall, each name in `font-semibold`.
#[test_log::test(tokio::test)]
async fn the_column_heads_are_a_row_in_the_page_colour_with_each_name_in_semibold() {
    let pages = Pages::of(a_board()).await;

    let html = pages.page("/").await;

    let rows = select(&html, "#board .board > :first-child");
    assert_eq!(rows.len(), 1, "{}", html.html());
    assert!(
        has_classes(rows[0], &["bg-base-100", "h-8"]),
        "{}",
        rows[0].html()
    );
    let names = select_in(rows[0], ".column");
    assert_eq!(names.len(), 6);
    for name in names {
        assert!(has_classes(name, &["font-semibold"]), "{}", name.html());
    }
}

/// `docs/specs/board.md`, Board, Lanes: a lane is drawn as a card is, with
/// `px-2` at its sides and `pb-2` at its foot, its columns and the cards in a
/// column `gap-2` apart; its head is its top row in `bg-base-200`, at least
/// `h-10` tall, with `pt-2` above its text in `font-semibold`, and no
/// `card-title`; and its cards are `card-sm` cards in `bg-base-100`.
#[test_log::test(tokio::test)]
async fn each_lane_is_drawn_as_a_card_and_its_cards_as_cards_in_the_page_colour() {
    let pages = Pages::of(a_board_with_a_finished_lane()).await;

    let html = pages.page("/").await;

    let lanes = select(&html, "#board .lane");
    assert_eq!(lanes.len(), 4);
    for lane in lanes {
        assert!(
            has_classes(lane, &["rounded-box", "px-2", "pb-2"]),
            "{}",
            lane.html()
        );
        assert_drawn_as_a_card(lane, "bg-base-200", "a lane");
        let heads = select_in(lane, ":scope > .lane-head");
        assert_eq!(heads.len(), 1, "{}", lane.html());
        for head in heads {
            assert!(
                has_classes(head, &["bg-base-200", "min-h-10", "pt-2", "font-semibold"]),
                "{}",
                head.html()
            );
            assert!(
                lacks_classes(head, &["border-t", "card-title"]),
                "{}",
                head.html()
            );
        }
        for cell in select_in(lane, ".cell") {
            let columns = cell.parent().and_then(ElementRef::wrap).unwrap();
            assert!(has_classes(columns, &["gap-2"]), "{}", columns.html());
            assert!(has_classes(cell, &["gap-2"]), "{}", cell.html());
        }
    }
    let cards = select(&html, "#board li");
    assert_eq!(cards.len(), 4);
    for card in cards {
        assert!(has_classes(card, &["card", "card-sm"]), "{}", card.html());
        assert_drawn_as_a_card(card, "bg-base-100", "a board card");
    }
}

/// The statuses' icons `element` holds, as their markup.
fn icons(element: ElementRef) -> Vec<String> {
    select_in(element, STATUS_ICON)
        .into_iter()
        .map(|icon| icon.html())
        .collect()
}

/// An issue no card on the board stands for, so that a pull request closing
/// it is a card of its own.
const OFF_THE_BOARD: u64 = 9;

/// `docs/specs/board.md`, Cards, Card status, and
/// `docs/specs/ui_patterns.md`, Status: waiting for a person shows as
/// waiting, which is `warning`.
const WAITING_FOR_A_PERSON: &str = "[--tone:var(--color-warning)]";

/// `docs/specs/board.md`, Lanes: a lane's head is its number and title, with
/// its tickets closed of total and when it finished, and no status of its own
/// (Out of scope); `docs/specs/board.md`, Board, Lanes: a bar in `bg-base-200`.
#[test_log::test(tokio::test)]
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

/// `docs/specs/board.md`, Cards, Card status, and
/// `docs/specs/ui_patterns.md`, Status: paused or queued shows as
/// paused, which is `base-content/60`.
const PAUSED_OR_QUEUED: &str =
    "[--tone:color-mix(in_oklab,var(--color-base-content)_60%,transparent)]";

/// `docs/specs/board.md`, Cards: a card shows its column, its line, the
/// tone of its card status with that status's icon before its line, and
/// the icon of an issue beside its number. The status's icon is in
/// `base-content`.
#[test_log::test(tokio::test)]
async fn a_card_paused_for_the_usage_limit_shows_its_column_line_tone_and_icons() {
    let claimed = assigned(ticket(1, "Do the thing"), &[LINEWRIGHT]);
    let pages = Pages::of(View {
        memory: limited_until("2026-10-01T13:00:00Z"),
        ..view(vec![claimed], vec![])
    })
    .await;

    let html = pages.page("/").await;

    assert_eq!(
        cards_shown(&html),
        [(
            "Working: Do the thing #1 | paused until the usage limit resets".to_string(),
            Kind::Issue
        )]
    );
    let card = select(&html, "#board .card")[0];
    assert!(
        has_classes(card, &["toned", PAUSED_OR_QUEUED]),
        "{}",
        card.html()
    );
    let icons = select_in(card, ".line > svg.icon:first-child");
    assert_eq!(icons.len(), 1, "{}", card.html());
    assert!(
        has_classes(icons[0], &["text-base-content"]),
        "{}",
        icons[0].html()
    );
    assert_no_status_by_colour_alone(&html);
}

/// `docs/specs/design_system.md`, Type: secondary text is in
/// `text-base-content/60`.
#[test_log::test(tokio::test)]
async fn a_cards_line_under_its_title_is_in_secondary_text() {
    let pages = Pages::of(a_board()).await;

    let html = pages.page("/").await;

    assert_in_secondary_text(&html, "#board .card .line");
}

#[test_log::test(tokio::test)]
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
            let icons_before_its_line = select_in(card, ".line > svg.icon:first-child").len();
            let faded = has_classes(card, &["opacity-60"]);
            (title, toned, waiting, icons_before_its_line, faded)
        })
        .collect();
    cards.sort();
    assert_eq!(
        cards,
        [
            ("Blocked #8".to_string(), false, false, 0, false),
            ("Do the thing #1".to_string(), true, true, 1, false),
            ("Old idea #4".to_string(), false, false, 0, true),
            ("Only part #6".to_string(), false, false, 0, true),
            ("Part one #3".to_string(), true, true, 1, false),
        ]
    );
}

#[test_log::test(tokio::test)]
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
            "div: ",
            "details: 1 finished lane Do the thing #1 — finished \
             2026-10-01 09:00:00 UTC Do the thing #1 closed as completed · 2026-10-01 09:00:00 UTC",
        ]
    );
    let lanes: Vec<Option<&str>> = select(&html, "#finished details")
        .into_iter()
        .map(|fold| fold.attr("id"))
        .collect();
    assert_eq!(lanes, [Some("lane-1")]);
}

/// `docs/specs/board.md`, Cards, Its line: an issue in review names its
/// pull request in its line by its number, after its icon, linking to it on
/// GitHub.
#[test_log::test(tokio::test)]
async fn an_issue_in_review_links_its_pull_request_in_its_line() {
    let pages = Pages::of(view(
        vec![assigned(ticket(1, "Do the thing"), &[LINEWRIGHT])],
        vec![linewrights_pull(2, &[1], &[CheckState::Pending])],
    ))
    .await;

    let html = pages.page("/").await;

    assert_eq!(
        sorted_cards(&html),
        ["In review: Do the thing #1 | #2 · checks running"]
    );
    let line = select(&html, "#board .card .line")[0];
    assert_eq!(
        links_in(line),
        [(
            "#2".to_string(),
            "https://github.com/owner/widgets/issues/2".to_string()
        )]
    );
    let pull_request = select_in(line, "svg[data-icon='git-pull-request'] + a");
    assert_eq!(pull_request.len(), 1, "{}", line.html());
}

#[test_log::test(tokio::test)]
async fn a_merged_pull_request_of_linewrights_closing_no_issue_is_a_pull_request_card_in_done() {
    let pages = Pages::of(with_merged(
        view(vec![], vec![]),
        vec![merged_pull(5, LINEWRIGHT, &[])],
    ))
    .await;

    let html = pages.page("/").await;
    let shown = cards_shown(&html);

    assert_eq!(
        shown,
        [(
            "Done: Merged 5 #5 | merged · 2026-10-01 10:00:00 UTC".to_string(),
            Kind::PullRequest
        )]
    );
    let head_icons: Vec<(Option<&str>, Option<&str>)> =
        select(&html, "#board .lane-head svg:first-child")
            .into_iter()
            .map(|icon| (icon.attr("data-icon"), icon.attr("aria-label")))
            .collect();
    assert_eq!(
        head_icons,
        [(Some("git-pull-request"), Some("pull request"))],
        "its lane's head has a pull request's icon (`docs/specs/board.md`, Lanes)"
    );
}

#[test_log::test(tokio::test)]
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
            blocked(13, OFF_THE_BOARD),
        ],
    ))
    .await;

    let html = pages.page("/").await;

    assert_eq!(
        sorted_cards(&html),
        [
            "In review: Do another thing #2 | #12 · checks running",
            "In review: Do the thing #1 | #11 · review required",
            "In review: Pull 13 #13 | review required",
        ]
    );
    assert_eq!(
        toned(&html, WAITING_FOR_A_PERSON),
        ["Do the thing #1", "Pull 13 #13"]
    );
}

#[test_log::test(tokio::test)]
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
            "Needs a person: First #2 | decision needed · 2026-10-01 09:00:00 UTC",
            "Ready: Then #3 | blocked by #2 · 2026-10-01 10:00:00 UTC",
            "Triage: Export fails #1 | 2026-10-01 08:00:00 UTC",
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

#[test_log::test(tokio::test)]
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
            format!("Done: Do the thing {ticket}"),
            format!("Done: Ours {ours}"),
        ]
    );
    let html = web.page(&format!("/items/{}", spec_change.0)).await;
    let (_, state) = item_facts(&html)
        .into_iter()
        .find(|(label, _)| label == "State")
        .unwrap();
    assert!(
        state.starts_with("Merged · Left alone · merged 2026-")
            && state.contains(", before the spec-lookback window opened 2026-"),
        "{state}"
    );
    web.stop().await;
}

/// `docs/specs/board.md`, Cards, What stops a pull request merging: a
/// pull request Linewright opened and is not assigned to says "unassigned"
/// on its own card.
#[test_log::test(tokio::test)]
async fn a_pull_request_of_linewrights_it_is_not_assigned_to_says_unassigned_on_its_own_card() {
    let pages = Pages::of(view(
        vec![],
        vec![open_pull(5, LINEWRIGHT, &[OFF_THE_BOARD], &[])],
    ))
    .await;

    let shown = sorted_cards(&pages.page("/").await);

    assert_eq!(shown, ["In review: Pull 5 #5 | unassigned"]);
}

/// `docs/specs/board.md`, Cards: a card's `card-title` holds its icon,
/// then its title and its number, `gap-1` apart, at daisyUI's own gap from
/// the icon; its line is `gap-1` under its `card-title`, its card status's
/// icon `gap-1` before it, and a pull request's icon `gap-1` before its
/// number in it.
#[test_log::test(tokio::test)]
async fn a_card_holds_its_icon_title_number_and_line_their_gaps_apart() {
    let pages = Pages::of(view(
        vec![
            assigned(ticket(1, "Do the thing"), &[LINEWRIGHT]),
            labeled(3, "Ask", &["ready-for-human"]),
        ],
        vec![linewrights_pull(2, &[1], &[CheckState::Pending])],
    ))
    .await;

    let html = pages.page("/").await;

    for card in select(&html, "#board .card") {
        let shown = card.html();
        let body = select_in(card, ".card-body")[0];
        assert!(has_classes(body, &["gap-1"]), "{shown}");
        let title = select_in(body, "h3.card-title")[0];
        assert_eq!(
            classes_starting_with(title, &["gap-"]),
            Vec::<&str>::new(),
            "{shown}"
        );
        assert_eq!(
            parts(title),
            [
                "icon issue-opened",
                "span.flex.gap-1.items-baseline.min-w-0"
            ],
            "{shown}"
        );
        let name = select_in(title, ":scope > span")[0];
        assert_eq!(parts(name), ["title", "number"], "{shown}");
        let number = select_in(name, ":scope > a")[1];
        assert!(has_classes(number, &["font-normal"]), "{shown}");
    }
    let mut lines: Vec<Vec<String>> = select(&html, "#board .card .line")
        .into_iter()
        .map(|line| {
            assert!(has_classes(line, &["flex", "gap-1"]), "{}", line.html());
            parts(line)
        })
        .collect();
    lines.sort();
    assert_eq!(
        lines,
        [
            vec!["icon arrow-path".to_string(), "span.".to_string()],
            vec!["icon hand-raised".to_string(), "span.".to_string()],
        ]
    );
    let pull_request = select(&html, "#board .card .line > span > .inline-flex");
    assert_eq!(pull_request.len(), 1, "{}", html.html());
    assert_eq!(parts(pull_request[0]), ["icon git-pull-request", "number"]);
    assert_no_words_loose_in_a_flex(&html, "#board");
}

/// `docs/specs/board.md`, Lanes, A lane's head: its icon, title, number
/// and rest are `gap-1` apart, its number in `font-normal`, and its rest
/// starts with no space.
#[test_log::test(tokio::test)]
async fn a_lane_heads_icon_title_number_and_rest_are_gap_1_apart() {
    let pages = Pages::of(a_board_with_a_finished_lane()).await;

    let html = pages.page("/").await;

    let mut heads = vec![];
    for head in select(&html, "#board .lane-head") {
        let line = select_in(head, ".inline-flex")[0];
        assert!(
            has_classes(line, &["gap-1", "items-center"]),
            "{}",
            head.html()
        );
        let number = select_in(line, ":scope > a[href^='https://github.com/']")[0];
        assert!(has_classes(number, &["font-normal"]), "{}", head.html());
        for rest in select_in(line, ":scope > .tail") {
            let words: String = rest.text().collect();
            assert!(words.starts_with('—'), "{words:?}");
        }
        heads.push(parts(line));
    }
    heads.sort();
    let part = |parts: &[&str]| parts.iter().map(ToString::to_string).collect::<Vec<_>>();
    assert_eq!(
        heads,
        [
            part(&["icon issue-opened", "title", "number"]),
            part(&["icon issue-opened", "title", "number", "rest"]),
            part(&["icon issue-opened", "title", "number", "rest"]),
            part(&["icon issue-opened", "title", "number", "rest"]),
        ]
    );
    assert_no_words_loose_in_a_flex(&html, "#board");
}

/// `docs/specs/board.md`, Item, Related items: a related tracking issue
/// whose lane is finished has its lane head's rest, its tickets closed and
/// when it finished, after its number.
#[test_log::test(tokio::test)]
async fn a_finished_related_tracking_issue_shows_when_it_finished_after_its_number() {
    let pages = Pages::of(a_board_with_a_finished_lane()).await;

    let html = pages.page("/items/6").await;

    let facts = item_facts(&html);
    let at = facts
        .iter()
        .position(|(label, _)| label == "Tracking issue")
        .unwrap_or_else(|| panic!("{facts:?}"));
    let related = select_in(select(&html, "#item dd")[at], ".related-item")[0];
    assert_eq!(
        parts(related),
        ["icon issue-opened", "title", "number", "rest"]
    );
    assert_eq!(
        text(related),
        "Small thing #5 — 1/1 tickets closed — finished 2026-10-01 09:40:00 UTC"
    );
}
