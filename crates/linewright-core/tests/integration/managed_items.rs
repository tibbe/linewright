use linewright_core::board::{Column, board};
use linewright_core::domain::*;
use linewright_core::explain::{explain_fresh, fresh_view};
use linewright_core::item_facts::{StateFact, item_facts};
use linewright_core::lifecycle::State;
use linewright_core::managed::{
    Held, IssueExclusion, LeftAloneIssue, LeftAloneItem, LeftAlonePull, PullExclusion, classify,
    held,
};
use linewright_core::reconcile::NotSpecChange;
use linewright_core::status::View;
use linewright_core::store::AnyItem;
use linewright_core::testing::*;
use rstest::rstest;

/// Every item of a snapshot varied over the managed-items rule
/// (`docs/specs/repository.md`, Managed items), as a store reads each.
fn varied_items() -> Vec<AnyItem> {
    let labeled = |number, label: &str| {
        labeled_by_linewright(Issue {
            labels: vec![label.into()],
            ..ticket(number, &format!("Ticket {number}"))
        })
    };
    let taken_back = |number| Issue {
        labels: vec![],
        ..labeled(number, "ready-for-agent")
    };
    let tracking = |number, tickets: &[(u64, bool)]| Issue {
        author: Login::new(LINEWRIGHT),
        labels: vec![label::DESIGNED.into()],
        filed: Some(Filed::Tracking {
            spec_change: Number(number + 70),
        }),
        sub_issues: tickets
            .iter()
            .map(|&(n, open)| sub_issue(n, open, LINEWRIGHT))
            .collect(),
        ..ticket(number, &format!("Tracking {number}"))
    };
    let cut = |number, tracked_by: u64| Issue {
        author: Login::new(LINEWRIGHT),
        parent: Some(Parent {
            number: Number(tracked_by),
            spec_change: Some(Number(tracked_by + 70)),
        }),
        ..ticket(number, &format!("Ticket {number}"))
    };
    let closing = |issue: Issue, pull: &Pull| Issue {
        closed_by: vec![ClosingPull::from(pull)],
        ..issue
    };
    let linewrights_for_1 = open_pull(50, LINEWRIGHT, &[1], &[LINEWRIGHT]);
    let peoples_for_6 = open_pull(61, "alice", &[6], &[LINEWRIGHT]);
    let commented = MergedPull {
        events: vec![IssueEvent::Commented {
            by: Login::new(LINEWRIGHT),
            at: at("2026-10-01T09:30:00Z"),
        }],
        ..merged_pull(73, "alice", &[])
    };
    let mut peoples_closed = open_pull(62, "alice", &[], &[]);
    peoples_closed.state = PullState::Closed {
        at: at("2026-10-01T10:00:00Z"),
    };
    peoples_closed.labelings = vec![Labeling {
        label: "linewright:stalled".into(),
        by: Some(Login::new(LINEWRIGHT)),
        at: at("2026-10-01T09:30:00Z"),
    }];
    vec![
        AnyItem::HeldIssue(Box::new(closing(ticket(1, "Ready"), &linewrights_for_1))),
        AnyItem::HeldIssue(Box::new(labeled(2, "needs-triage"))),
        AnyItem::LeftOutIssue(Box::new(Issue {
            labels: vec!["ready-for-human".into()],
            ..ticket(3, "A person's")
        })),
        AnyItem::HeldIssue(Box::new(taken_back(4))),
        AnyItem::HeldIssue(Box::new(taken_back(5))),
        AnyItem::HeldIssue(Box::new(closing(taken_back(6), &peoples_for_6))),
        AnyItem::HeldIssue(Box::new(labeled(7, "needs-info"))),
        AnyItem::HeldIssue(Box::new(tracking(8, &[(9, true)]))),
        AnyItem::HeldIssue(Box::new(cut(9, 8))),
        AnyItem::HeldIssue(Box::new(tracking(10, &[(11, false)]))),
        read_closed(labeled(11, "ready-for-agent"), "2026-09-30T12:00:00Z"),
        read_closed(labeled(20, "ready-for-agent"), "2026-09-30T12:00:00Z"),
        read_closed(ticket(21, "Never worked"), "2026-09-30T12:00:00Z"),
        AnyItem::Pull(Box::new(linewrights_for_1)),
        AnyItem::Pull(Box::new(open_pull(51, LINEWRIGHT, &[], &[]))),
        AnyItem::Pull(Box::new(open_pull(60, "alice", &[], &[LINEWRIGHT]))),
        AnyItem::Pull(Box::new(peoples_for_6)),
        AnyItem::Pull(Box::new(peoples_closed)),
        AnyItem::Pull(Box::new(open_pull(63, "alice", &[], &[]))),
        AnyItem::Merged {
            pull: Box::new(merged_pull(70, LINEWRIGHT, &[99])),
            spec_change_issues: vec![],
        },
        AnyItem::Merged {
            pull: Box::new(merged_pull(71, "alice", &[])),
            spec_change_issues: vec![],
        },
        AnyItem::Merged {
            pull: Box::new(MergedPull {
                files: vec!["docs/specs/invoices.md".into()],
                assignees: vec![Login::new(LINEWRIGHT)],
                ..merged_pull(72, "alice", &[])
            }),
            spec_change_issues: vec![],
        },
        AnyItem::Merged {
            pull: Box::new(commented),
            spec_change_issues: vec![],
        },
    ]
}

/// Where the board shows an item (`docs/specs/board.md`, Board).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shown {
    /// A card of its own in the column.
    Card(Column),
    /// On the card of the issue it closes, its own card in the column.
    OnCardOf(u64, Column),
    /// At the head of its lane, unfinished, with no card.
    HeadsLane,
    LeftAlone,
}

#[test_log::test]
fn an_item_is_managed_exactly_when_the_board_gives_it_a_column_or_a_lane_and_its_page_agrees() {
    let now = at("2026-10-01T12:00:30Z");
    let items = varied_items();
    let mut view = items.iter().fold(view(vec![], vec![]), |view, read| {
        fresh_view(&view, read, now)
    });
    view.in_flight = vec![in_flight("5/implement-1", "2026-10-01T11:50:00Z")];
    let persons_pull_linewright_worked_closed_unmerged = ClosedItem {
        number: Number(62),
        title: "Pull 62".into(),
        closed_at: at("2026-10-01T10:00:00Z"),
        updated_at: at("2026-10-01T10:00:00Z"),
        kind: ClosedKind::Pull {
            closes: vec![],
            author: Some(Login::new("alice")),
            assignees: vec![],
            review_requests: vec![],
            reviewers: vec![],
        },
    };
    std::sync::Arc::make_mut(&mut view.snapshot)
        .closed
        .push(persons_pull_linewright_worked_closed_unmerged);
    let board = board(&view, &[], now);
    let expected = [
        (1, Shown::Card(Column::InReview)),
        (2, Shown::Card(Column::Triage)),
        (3, Shown::LeftAlone),
        (4, Shown::LeftAlone),
        (5, Shown::Card(Column::Working)),
        (6, Shown::Card(Column::InReview)),
        (7, Shown::Card(Column::NeedsAPerson)),
        (8, Shown::HeadsLane),
        (9, Shown::Card(Column::Ready)),
        (10, Shown::HeadsLane),
        (11, Shown::Card(Column::Done)),
        (20, Shown::Card(Column::Done)),
        (21, Shown::LeftAlone),
        (50, Shown::OnCardOf(1, Column::InReview)),
        (51, Shown::Card(Column::InReview)),
        (60, Shown::Card(Column::InReview)),
        (61, Shown::OnCardOf(6, Column::InReview)),
        (62, Shown::Card(Column::Done)),
        (63, Shown::LeftAlone),
        (70, Shown::Card(Column::Done)),
        (71, Shown::LeftAlone),
        (72, Shown::LeftAlone),
        (73, Shown::LeftAlone),
    ];
    assert_eq!(
        items.iter().map(|read| read.number().0).collect::<Vec<_>>(),
        expected.map(|(number, _)| number)
    );

    for (read, (number, shown)) in items.iter().zip(expected) {
        let number = Number(number);
        let closed;
        let held = match held(&view.snapshot, number) {
            Some(held) => held,
            None => match read {
                AnyItem::HeldIssue(issue) | AnyItem::LeftOutIssue(issue) => Held::Issue(issue),
                AnyItem::ClosedIssue { issue, closing } => {
                    closed = ClosedItem::of_issue(issue, *closing);
                    Held::Closed(&closed)
                }
                AnyItem::Pull(pull) => Held::Pull(pull),
                AnyItem::Merged { pull, .. } => Held::Merged(pull),
            },
        };
        let managed = classify(&view, held, now);
        let on_the_board = match shown {
            Shown::Card(column) => board.card(number).map(|(on, _)| on) == Some(column),
            Shown::OnCardOf(issue, _) => {
                board.card(number).is_none() && board.card(Number(issue)).is_some()
            }
            Shown::HeadsLane => {
                board.card(number).is_none()
                    && board
                        .lane_headed_by(number)
                        .is_some_and(|lane| lane.finished().is_none())
            }
            Shown::LeftAlone => {
                board.card(number).is_none() && board.lane_headed_by(number).is_none()
            }
        };
        let placed = managed
            .as_ref()
            .ok()
            .map(linewright_core::managed::Managed::column);
        let answer = explain_fresh(&view, &[], read, now);
        let page = item_facts(&answer, &view, &board, &[], now).state;

        assert!(on_the_board, "#{number}: {shown:?} on {board:?}");
        assert_eq!(
            managed.is_ok(),
            !matches!(shown, Shown::LeftAlone),
            "#{number}: {managed:?}"
        );
        match shown {
            Shown::Card(column) | Shown::OnCardOf(_, column) => {
                assert_eq!(placed, Some(Some(column)), "#{number}");
                assert!(
                    matches!(page, StateFact::Card { column: on, .. } if on == column),
                    "#{number}: {page:?}"
                );
            }
            Shown::HeadsLane => {
                assert_eq!(placed, Some(None), "#{number}");
                assert_eq!(
                    page,
                    StateFact::Tracking {
                        state: Some(State::Tracking),
                        finished: None
                    },
                    "#{number}"
                );
            }
            Shown::LeftAlone => {
                assert!(
                    matches!(page, StateFact::LeftAlone { .. }),
                    "#{number}: {page:?}"
                );
            }
        }
    }
}

/// What `classify` says of an item: the column of its card, none for a
/// tracking issue, or why Linewright leaves the item alone.
type Classified = Result<Option<Column>, LeftAloneItem>;

/// The required label of a view that requires one.
const REQUIRED: &str = "linewright";

/// A view holding `issues`, open, and nothing else.
fn issues(open: Vec<Issue>) -> View {
    view(open, vec![])
}

/// `view`, with `REQUIRED` its required label.
fn requiring(mut view: View) -> View {
    view.policy.require_label = Some(REQUIRED.into());
    view
}

/// `view`, with the run `id` in flight.
fn running(mut view: View, id: &str) -> View {
    view.in_flight = vec![in_flight(id, "2026-10-01T11:50:00Z")];
    view
}

/// A view holding `linewrights`, Linewright's pull requests, and
/// `peoples_pulls`, open or closed unmerged, beside `issues`.
fn with_pulls(issues: Vec<Issue>, linewrights: Vec<Pull>, peoples_pulls: Vec<Pull>) -> View {
    let mut view = view(issues, linewrights);
    std::sync::Arc::make_mut(&mut view.snapshot).peoples_pulls = peoples_pulls;
    view
}

fn merged(pull: MergedPull) -> View {
    let mut view = issues(vec![]);
    std::sync::Arc::make_mut(&mut view.snapshot).merged = vec![pull];
    view
}

/// A view holding #1, a person's issue closed by hand, its labels `labels`
/// and worked only as `work` tells.
fn closed_issue(labels: &[&str], work: WorkFacts) -> View {
    let mut closed = closed_item(&ticket(1, "Ticket 1"), "2026-09-30T12:00:00Z");
    if let ClosedKind::Issue { facts, .. } = &mut closed.kind {
        facts.labels = labels.iter().map(ToString::to_string).collect();
        facts.work = work;
    }
    closed_items(vec![closed])
}

fn closed_items(closed: Vec<ClosedItem>) -> View {
    let mut view = issues(vec![]);
    std::sync::Arc::make_mut(&mut view.snapshot).closed = closed;
    view
}

/// #1, a person's issue labeled `label`, that Linewright has not worked.
fn persons_issue(label: &str) -> Issue {
    Issue {
        labels: vec![label.into()],
        ..ticket(1, "Ticket 1")
    }
}

/// #1, labeled `ready-for-agent` and claimed by Linewright, whose status
/// label a person removed since.
fn taken_back() -> Issue {
    Issue {
        labels: vec![label::DESIGNED.into()],
        assignees: vec![Login::new(LINEWRIGHT)],
        events: vec![
            linewright_labeling("ready-for-agent", "2026-09-30T10:00:00Z"),
            assigning_linewright("2026-09-30T10:00:00Z"),
            IssueEvent::Unlabeled {
                label: "ready-for-agent".into(),
                at: at("2026-09-30T11:00:00Z"),
            },
        ],
        ..ticket(1, "Ticket 1")
    }
}

/// #1, a tracking issue Linewright filed, with a ticket for each of
/// `tickets`, by its number and whether it is open.
fn tracking(tickets: &[(u64, bool)]) -> Issue {
    Issue {
        author: Login::new(LINEWRIGHT),
        labels: vec![label::DESIGNED.into()],
        filed: Some(Filed::Tracking {
            spec_change: Number(70),
        }),
        sub_issues: tickets
            .iter()
            .map(|&(n, open)| sub_issue(n, open, LINEWRIGHT))
            .collect(),
        ..ticket(1, "Tracking 1")
    }
}

fn worked_by(events: Vec<IssueEvent>) -> WorkFacts {
    WorkFacts {
        events,
        ..WorkFacts::default()
    }
}

fn open_issue_left_alone(why: IssueExclusion) -> Classified {
    Err(LeftAloneItem::Issue(LeftAloneIssue::Open {
        left_alone: why,
    }))
}

fn closed_issue_left_alone() -> Classified {
    Err(LeftAloneItem::Issue(LeftAloneIssue::Closed))
}

/// An open issue left alone for want of the required label.
fn left_alone_unmarked() -> Classified {
    open_issue_left_alone(IssueExclusion::NotMarked {
        label: REQUIRED.into(),
    })
}

/// `docs/specs/repository.md`, Managed items: Linewright manages an item it
/// has worked or that asks for its work, while the item has a state. A
/// managed item's card is in its state's column (`docs/specs/board.md`,
/// Board), and a tracking issue heads its lane with none.
#[rstest]
#[case::an_issue_linewright_labeled_as_a_run_ended(
    issues(vec![labeled_by_linewright(persons_issue("needs-info"))]),
    Ok(Some(Column::NeedsAPerson))
)]
#[case::an_issue_linewright_labeled_in_a_hand_back(
    issues(vec![labeled_by_linewright(persons_issue("ready-for-human"))]),
    Ok(Some(Column::NeedsAPerson))
)]
#[case::a_persons_issue_waiting_for_an_answer(
    issues(vec![persons_issue("needs-info")]),
    open_issue_left_alone(IssueExclusion::WaitsOnAPerson { label: StatusLabel::NeedsInfo })
)]
#[case::a_persons_issue_waiting_for_a_decision(
    issues(vec![persons_issue("ready-for-human")]),
    open_issue_left_alone(IssueExclusion::WaitsOnAPerson { label: StatusLabel::ReadyForHuman })
)]
#[case::a_persons_issue_asking_for_triage(
    issues(vec![persons_issue("needs-triage")]),
    Ok(Some(Column::Triage))
)]
#[case::a_persons_issue_asking_for_linewrights_work(
    issues(vec![ticket(1, "Ticket 1")]),
    Ok(Some(Column::Ready))
)]
#[case::a_persons_issue_asking_for_linewrights_work_carrying_the_required_label(
    requiring(issues(vec![Issue {
        labels: vec!["ready-for-agent".into(), REQUIRED.into()],
        ..ticket(1, "Ticket 1")
    }])),
    Ok(Some(Column::Ready))
)]
#[case::a_persons_issue_asking_for_linewrights_work_without_the_required_label(
    requiring(issues(vec![ticket(1, "Ticket 1")])),
    left_alone_unmarked()
)]
#[case::a_persons_issue_waiting_for_an_answer_without_the_required_label(
    requiring(issues(vec![persons_issue("needs-info")])),
    open_issue_left_alone(IssueExclusion::WaitsOnAPerson { label: StatusLabel::NeedsInfo })
)]
#[case::an_issue_without_the_required_label_linewright_is_assigned_to(
    requiring(issues(vec![Issue {
        assignees: vec![Login::new(LINEWRIGHT)],
        ..ticket(1, "Ticket 1")
    }])),
    Ok(Some(Column::Working))
)]
#[case::an_issue_without_the_required_label_linewright_opened(
    requiring(issues(vec![Issue {
        author: Login::new(LINEWRIGHT),
        ..ticket(1, "Ticket 1")
    }])),
    Ok(Some(Column::Ready))
)]
#[case::an_issue_without_the_required_label_linewright_handed_back_in_a_stall(
    requiring(issues(vec![Issue {
        labels: vec!["ready-for-agent".into(), label::STALLED.into()],
        events: vec![
            assigning_linewright("2026-10-01T10:00:00Z"),
            linewright_labeling(label::STALLED, "2026-10-01T11:00:00Z"),
            linewright_letting_go("2026-10-01T11:00:00Z"),
        ],
        ..ticket(1, "Ticket 1")
    }])),
    Ok(Some(Column::Ready))
)]
#[case::an_issue_without_the_required_label_linewright_handed_back_and_a_person_took_up(
    requiring(issues(vec![Issue {
        events: vec![
            assigning_linewright("2026-10-01T10:00:00Z"),
            linewright_labeling(label::READY_FOR_HUMAN, "2026-10-01T10:20:00Z"),
            linewright_letting_go("2026-10-01T10:20:00Z"),
            IssueEvent::Unlabeled {
                label: label::READY_FOR_HUMAN.into(),
                at: at("2026-10-01T10:30:00Z"),
            },
        ],
        ..ticket(1, "Ticket 1")
    }])),
    Ok(Some(Column::Ready))
)]
#[case::an_issue_without_the_required_label_a_person_unassigned_linewright_from(
    requiring(issues(vec![Issue {
        events: vec![
            assigning_linewright("2026-10-01T10:00:00Z"),
            unassigning_linewright("2026-10-01T10:30:00Z"),
        ],
        ..ticket(1, "Ticket 1")
    }])),
    left_alone_unmarked()
)]
#[case::an_issue_without_the_required_label_linewrights_pull_request_closes(
    requiring(issues(vec![closed_by(ticket(1, "Ticket 1"), 43, LINEWRIGHT)])),
    Ok(Some(Column::InReview))
)]
#[case::an_issue_without_the_required_label_a_persons_pull_request_closes(
    requiring(issues(vec![closed_by(ticket(1, "Ticket 1"), 43, "alice")])),
    left_alone_unmarked()
)]
#[case::an_issue_linewright_worked_whose_status_label_a_person_removed(
    issues(vec![taken_back()]),
    open_issue_left_alone(IssueExclusion::NoPipelineLabel)
)]
#[case::an_issue_linewright_labeled_whose_status_label_a_person_removed(
    issues(vec![Issue {
        labels: vec![],
        events: vec![
            linewright_labeling("ready-for-agent", "2026-09-30T10:00:00Z"),
            IssueEvent::Unlabeled {
                label: "ready-for-agent".into(),
                at: at("2026-09-30T11:00:00Z"),
            },
        ],
        ..ticket(1, "Ticket 1")
    }]),
    open_issue_left_alone(IssueExclusion::NoPipelineLabel)
)]
#[case::an_issue_with_no_status_label_while_its_run_is_in_flight(
    running(issues(vec![taken_back()]), "1/implement-1"),
    Ok(Some(Column::Working))
)]
#[case::an_issue_with_no_status_label_linewrights_pull_request_closes(
    with_pulls(vec![taken_back()], vec![open_pull(10, LINEWRIGHT, &[1], &[LINEWRIGHT])], vec![]),
    Ok(Some(Column::InReview))
)]
#[case::an_issue_with_no_status_label_a_persons_pull_request_closes(
    issues(vec![closed_by(taken_back(), 20, "alice")]),
    Ok(Some(Column::InReview))
)]
#[case::an_issue_linewright_is_assigned_to_with_no_status_label(
    issues(vec![Issue {
        labels: vec!["bug".into()],
        assignees: vec![Login::new(LINEWRIGHT)],
        ..ticket(1, "Ticket 1")
    }]),
    open_issue_left_alone(IssueExclusion::NoPipelineLabel)
)]
#[case::a_tracking_issue_with_a_ticket_open(issues(vec![tracking(&[(2, true)])]), Ok(None))]
#[case::a_tracking_issue_with_none_of_its_tickets_open(
    issues(vec![tracking(&[(2, false)])]),
    Ok(None)
)]
#[case::a_closed_issue_linewright_never_worked(
    closed_issue(&["ready-for-agent"], WorkFacts::default()),
    closed_issue_left_alone()
)]
#[case::a_closed_issue_without_the_required_label_linewright_never_worked(
    requiring(closed_issue(&["ready-for-agent"], WorkFacts {
        author: Some(Login::new("maintainer")),
        assignees: vec![Login::new("alice")],
        ..WorkFacts::default()
    })),
    closed_issue_left_alone()
)]
#[case::a_closed_issue_carrying_the_required_label_linewright_never_worked(
    requiring(closed_issue(&["ready-for-agent", REQUIRED], WorkFacts::default())),
    closed_issue_left_alone()
)]
#[case::a_closed_issue_without_the_required_label_linewright_filed(
    requiring(closed_issue(&["ready-for-agent"], WorkFacts {
        author: Some(Login::new(LINEWRIGHT)),
        ..WorkFacts::default()
    })),
    Ok(Some(Column::Done))
)]
#[case::a_closed_issue_without_the_required_label_assigned_to_linewright_by_hand(
    requiring(closed_issue(&["ready-for-agent"], WorkFacts {
        assignees: vec![Login::new(LINEWRIGHT)],
        events: vec![IssueEvent::Assignment(Assignment::Assigned {
            assignee: Login::new(LINEWRIGHT),
            actor: Login::new("maintainer"),
            at: at("2026-09-29T10:00:00Z"),
        })],
        ..WorkFacts::default()
    })),
    closed_issue_left_alone()
)]
#[case::a_closed_issue_without_the_required_label_linewright_only_commented_on(
    requiring(closed_issue(&["ready-for-agent"], worked_by(vec![IssueEvent::Commented {
        by: Login::new(LINEWRIGHT),
        at: at("2026-09-29T10:00:00Z"),
    }]))),
    closed_issue_left_alone()
)]
#[case::a_closed_issue_without_the_required_label_a_person_unassigned_linewright_from(
    requiring(closed_issue(&["ready-for-agent"], worked_by(vec![
        assigning_linewright("2026-09-29T10:00:00Z"),
        unassigning_linewright("2026-09-29T11:00:00Z"),
    ]))),
    closed_issue_left_alone()
)]
#[case::a_closed_issue_without_the_required_label_linewright_handed_back(
    requiring(closed_issue(&["ready-for-agent"], worked_by(vec![
        assigning_linewright("2026-09-29T10:00:00Z"),
        linewright_labeling(label::READY_FOR_HUMAN, "2026-09-29T10:30:00Z"),
        linewright_letting_go("2026-09-29T10:30:00Z"),
    ]))),
    Ok(Some(Column::Done))
)]
#[case::a_closed_issue_without_the_required_label_linewrights_pull_request_closed(
    requiring(closed_issue(&["ready-for-agent"], WorkFacts {
        closed_by: vec![ClosingPull {
            number: Number(10),
            base_ref: "main".into(),
            author: Login::new(LINEWRIGHT),
            state: PullState::Closed {
                at: at("2026-09-30T11:00:00Z"),
            },
            updated_at: at("2026-09-30T11:00:00Z"),
        }],
        ..WorkFacts::default()
    })),
    Ok(Some(Column::Done))
)]
#[case::linewrights_open_pull_request(
    with_pulls(vec![], vec![open_pull(1, LINEWRIGHT, &[], &[LINEWRIGHT])], vec![]),
    Ok(Some(Column::InReview))
)]
#[case::a_persons_open_pull_request_linewright_is_assigned_to(
    with_pulls(vec![], vec![], vec![open_pull(1, "alice", &[], &[LINEWRIGHT])]),
    Ok(Some(Column::InReview))
)]
#[case::a_persons_open_pull_request_linewright_has_not_worked(
    with_pulls(vec![], vec![], vec![open_pull(1, "alice", &[], &[])]),
    Err(LeftAloneItem::PullRequest(LeftAlonePull::Open {
        left_alone: PullExclusion::NotWorked { author: Login::new("alice") },
    }))
)]
#[case::a_persons_pull_request_linewright_has_not_worked_closed_unmerged(
    with_pulls(vec![], vec![], vec![Pull {
        state: PullState::Closed { at: at("2026-10-01T10:00:00Z") },
        ..open_pull(1, "alice", &[], &[])
    }]),
    Err(LeftAloneItem::PullRequest(LeftAlonePull::Closed))
)]
#[case::linewrights_pull_request_read_closed_unmerged(
    with_pulls(vec![], vec![Pull {
        state: PullState::Closed { at: at("2026-10-01T10:00:00Z") },
        ..open_pull(1, LINEWRIGHT, &[], &[])
    }], vec![]),
    Ok(Some(Column::Done))
)]
#[case::linewrights_pull_request_closed_unmerged(
    closed_items(vec![ClosedItem {
        number: Number(1),
        title: "Pull 1".into(),
        closed_at: at("2026-10-01T10:00:00Z"),
        updated_at: at("2026-10-01T10:00:00Z"),
        kind: ClosedKind::Pull {
            closes: vec![],
            author: Some(Login::new(LINEWRIGHT)),
            assignees: vec![],
            review_requests: vec![],
            reviewers: vec![],
        },
    }]),
    Ok(Some(Column::Done))
)]
#[case::linewrights_merged_pull_request(merged(merged_pull(1, LINEWRIGHT, &[99])), Ok(Some(Column::Done)))]
#[case::a_persons_merged_pull_request_changing_no_spec(
    merged(merged_pull(1, "alice", &[])),
    Err(LeftAloneItem::PullRequest(LeftAlonePull::Merged {
        left_alone: NotSpecChange::NoSpecFile { spec_glob: "docs/**/*.md".into() },
    }))
)]
#[case::a_persons_merged_spec_change(
    merged(MergedPull {
        files: vec!["docs/specs/invoices.md".into()],
        ..merged_pull(1, "alice", &[])
    }),
    Err(LeftAloneItem::PullRequest(LeftAlonePull::TakingUp))
)]
#[test_log::test]
fn linewright_manages_an_item_it_worked_or_that_asks_for_its_work_while_it_has_a_state(
    #[case] view: View,
    #[case] expected: Classified,
) {
    let held = held(&view.snapshot, Number(1)).expect("the view holds #1");

    let classified = classify(&view, held, at(NOW))
        .map(|managed| managed.column())
        .map_err(|unmanaged| unmanaged.why);

    assert_eq!(classified, expected);
}
