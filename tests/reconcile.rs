mod views;

use chrono::{DateTime, TimeDelta, Utc};
use linewright::domain::*;
use linewright::reconcile::*;
use linewright::runtime::UsageLimit;
use linewright::spec_glob::SpecGlob;
use linewright::workflow::{LastRuns, Workflow};

const LINEWRIGHT: &str = "factory";

fn t(minute: i64) -> Timestamp {
    DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + TimeDelta::minutes(minute)
}

fn policy() -> Policy {
    Policy {
        linewright: Login::new(LINEWRIGHT),
        spec_glob: SpecGlob::default(),
        spec_lookback: TimeDelta::days(7),
        max_runs: 4,
        ci_grace: TimeDelta::minutes(5),
        ci_timeout: TimeDelta::hours(6),
        ci_attempts: 2,
        auto_merge: None,
        update_rule: UpdateRule::RebaseOnConflict,
        allowed_merge_methods: MergeMethods::all(),
        require_label: None,
    }
}

fn issue(number: u64) -> Issue {
    Issue {
        number: Number(number),
        title: format!("Issue {number}"),
        body: String::new(),
        author: Login::new("reporter"),
        open: true,
        created_at: t(0),
        labels: vec![],
        assignees: vec![],
        comments: vec![],
        blocked_by: vec![],
        sub_issues: vec![],
        parent: None,
        events: vec![],
        filed: None,
        closed_by: vec![],
    }
}

fn labeled(mut issue: Issue, label: &str) -> Issue {
    issue.labels.push(label.to_string());
    issue
}

fn snapshot(issues: Vec<Issue>) -> Snapshot {
    Snapshot {
        default_branch: "main".into(),
        issues,
        pulls: vec![],
        merged: vec![],
        ticket_branches: vec![],
    }
}

fn decide(snapshot: &Snapshot) -> Vec<Action> {
    reconcile(
        snapshot,
        &[],
        &Memory::default(),
        &Workflows::default(),
        t(10),
        &policy(),
    )
}

fn triage(issue: u64) -> Action {
    Action::Start(Work::Triage {
        issue: Number(issue),
    })
}

#[test]
fn a_needs_triage_issue_is_claimed_then_triaged() {
    let world = snapshot(vec![labeled(issue(1), label::NEEDS_TRIAGE)]);

    assert_eq!(decide(&world), vec![claim(1), triage(1)]);
}

fn comment(author: &str, minute: i64, body: &str) -> Comment {
    Comment {
        author: Login::new(author),
        body: body.to_string(),
        created_at: t(minute),
    }
}

fn triage_comment(minute: i64) -> Comment {
    comment(LINEWRIGHT, minute, "What version are you on?")
}

#[test]
fn a_reply_to_needs_info_sends_the_issue_back_to_triage() {
    let mut asked = labeled(issue(1), label::NEEDS_INFO);
    asked.comments = vec![triage_comment(1), comment("reporter", 2, "Version 3.")];

    assert_eq!(
        decide(&snapshot(vec![asked])),
        vec![
            Action::Relabel {
                issue: Number(1),
                remove: label::NEEDS_INFO.into(),
                add: label::NEEDS_TRIAGE.into(),
            },
            claim(1),
            triage(1),
        ]
    );
}

#[test]
fn a_reply_to_a_question_opening_with_the_old_marker_line_sends_the_issue_back_to_triage() {
    let mut asked = labeled(issue(1), label::NEEDS_INFO);
    asked.comments = vec![
        comment(
            LINEWRIGHT,
            1,
            "> *Linewright, triage*\n\nWhat version are you on?",
        ),
        comment("maintainer", 2, "Version 3, I checked."),
    ];

    assert_eq!(
        decide(&snapshot(vec![asked])),
        vec![
            Action::Relabel {
                issue: Number(1),
                remove: label::NEEDS_INFO.into(),
                add: label::NEEDS_TRIAGE.into(),
            },
            claim(1),
            triage(1),
        ]
    );
}

#[test]
fn needs_info_waits_while_only_linewright_has_spoken_since_triage() {
    let mut asked = labeled(issue(1), label::NEEDS_INFO);
    asked.comments = vec![
        comment("reporter", 0, "It crashes."),
        triage_comment(1),
        comment(LINEWRIGHT, 2, "Anything else?"),
    ];

    assert_eq!(decide(&snapshot(vec![asked])), vec![]);
}

#[test]
fn a_comment_on_a_ready_for_human_issue_starts_nothing() {
    let mut handed_back = labeled(issue(1), label::READY_FOR_HUMAN);
    handed_back.comments = vec![
        comment(LINEWRIGHT, 1, "Should they be resizable?"),
        comment("maintainer", 2, "Make them resizable."),
        comment("reporter", 3, "Please do."),
    ];

    assert_eq!(decide(&snapshot(vec![handed_back])), vec![]);
}

#[test]
fn an_issue_linewright_filed_is_not_triaged_when_a_person_comments() {
    let mut filed = labeled(issue(1), label::READY_FOR_HUMAN);
    filed.author = Login::new(LINEWRIGHT);
    filed.comments = vec![triage_comment(1), comment("maintainer", 2, "Agreed.")];

    assert_eq!(decide(&snapshot(vec![filed])), vec![]);
}

#[test]
fn a_label_the_maintainer_applies_to_a_ready_for_human_triage_wins_over_their_comment() {
    let answered_with = |state: StatusLabel| {
        let mut answered = labeled_both(StatusLabel::ReadyForHuman, state);
        answered.comments = vec![
            triage_comment(1),
            comment("maintainer", 2, "Make them resizable."),
        ];
        snapshot(vec![answered])
    };
    let unlabel_ready_for_human = Action::RemoveStatusLabel {
        issue: Number(1),
        label: StatusLabel::ReadyForHuman,
    };

    assert_eq!(
        decide(&answered_with(StatusLabel::ReadyForAgent)),
        vec![unlabel_ready_for_human.clone(), claim(1), implement(1)]
    );
    assert_eq!(
        decide(&answered_with(StatusLabel::NeedsTriage)),
        vec![unlabel_ready_for_human, claim(1), triage(1)]
    );
}

fn labeled_both(older: StatusLabel, newer: StatusLabel) -> Issue {
    views::relabeled(issue(1), older.label(), newer.label())
}

#[test]
fn of_two_status_labels_the_newer_is_kept_and_followed() {
    for older in StatusLabel::ALL {
        for newer in StatusLabel::ALL.into_iter().filter(|l| *l != older) {
            let unlabel = Action::RemoveStatusLabel {
                issue: Number(1),
                label: older,
            };
            let follow = match newer {
                StatusLabel::NeedsTriage => vec![claim(1), triage(1)],
                StatusLabel::ReadyForAgent => {
                    vec![claim(1), implement(1)]
                }
                StatusLabel::NeedsInfo | StatusLabel::ReadyForHuman => vec![],
            };

            assert_eq!(
                decide(&snapshot(vec![labeled_both(older, newer)])),
                [vec![unlabel], follow].concat(),
                "`{older}` applied before `{newer}`"
            );
        }
    }
}

#[test]
fn of_several_status_labels_all_but_the_newest_are_removed() {
    // `needs-triage` was applied before the events read, so it is the oldest.
    let issue = labeled(
        labeled(
            labeled_both(StatusLabel::NeedsInfo, StatusLabel::ReadyForHuman),
            label::NEEDS_TRIAGE,
        ),
        label::BUG,
    );

    assert_eq!(
        decide(&snapshot(vec![issue])),
        [StatusLabel::NeedsTriage, StatusLabel::NeedsInfo].map(|label| Action::RemoveStatusLabel {
            issue: Number(1),
            label,
        })
    );
}

#[test]
fn a_run_is_cancelled_when_a_newer_status_label_stops_asking_for_it() {
    let world = snapshot(vec![labeled_both(
        StatusLabel::NeedsTriage,
        StatusLabel::ReadyForHuman,
    )]);

    assert_eq!(
        reconcile(
            &world,
            &[live(1, WorkKind::Triage)],
            &Memory::default(),
            &Workflows::default(),
            t(10),
            &policy()
        ),
        vec![
            Action::Cancel {
                subject: Subject::Issue(Number(1)),
                reason: "`needs-triage` was removed from #1".into(),
            },
            Action::RemoveStatusLabel {
                issue: Number(1),
                label: StatusLabel::NeedsTriage,
            },
        ]
    );
}

fn merged(number: u64, files: &[&str]) -> MergedPull {
    MergedPull {
        number: Number(number),
        title: format!("Spec PR {number}"),
        body: String::new(),
        author: Login::new("maintainer"),
        merged_at: t(0),
        base_ref: "main".into(),
        labels: vec![],
        closes: vec![],
        files: files.iter().map(|f| f.to_string()).collect(),
        assignees: vec![],
        events: vec![],
    }
}

fn cut(pr: u64) -> Vec<Action> {
    vec![
        Action::Claim {
            item: Item::Pull(Number(pr)),
        },
        Action::Start(Work::Tickets { pr: Number(pr) }),
    ]
}

fn assigned(mut pr: MergedPull, minute: i64) -> MergedPull {
    pr.assignees.push(Login::new(LINEWRIGHT));
    pr.events.push(IssueEvent::Assignment(Assignment::Assigned {
        assignee: Login::new(LINEWRIGHT),
        at: t(minute),
    }));
    pr
}

fn unassigned(mut pr: MergedPull, actor: &str, minute: i64) -> MergedPull {
    pr.assignees.retain(|a| a.as_str() != LINEWRIGHT);
    pr.events
        .push(IssueEvent::Assignment(Assignment::Unassigned {
            assignee: Login::new(LINEWRIGHT),
            actor: Login::new(actor),
            at: t(minute),
        }));
    pr
}

fn labeled_merged(mut pr: MergedPull, label: &str, minute: i64) -> MergedPull {
    if !pr.has_label(label) {
        pr.labels.push(label.into());
    }
    pr.events.push(IssueEvent::Labeled {
        label: label.into(),
        at: t(minute),
    });
    pr
}

fn cutting(pr: u64) -> [LiveRun; 1] {
    [LiveRun {
        subject: Subject::SpecChange(Number(pr)),
        kind: WorkKind::Tickets,
    }]
}

#[test]
fn a_merged_pull_request_that_touches_the_spec_is_claimed_and_cut_into_tickets() {
    let mut world = snapshot(vec![]);
    world.merged = vec![merged(5, &["src/main.rs", "docs/billing/invoices.md"])];

    assert_eq!(decide(&world), cut(5));
}

#[test]
fn a_spec_change_linewright_is_assigned_to_is_cut_without_claiming_it_again() {
    let mut world = snapshot(vec![]);
    world.merged = vec![assigned(merged(5, &["docs/spec.md"]), 1)];

    assert_eq!(
        decide(&world),
        vec![Action::Start(Work::Tickets { pr: Number(5) })]
    );
}

#[test]
fn a_spec_change_linewright_was_unassigned_from_is_not_cut_again_on_its_own() {
    let handed_back = unassigned(assigned(merged(5, &["docs/spec.md"]), 1), LINEWRIGHT, 2);
    let paused = unassigned(assigned(merged(6, &["docs/spec.md"]), 1), "maintainer", 2);
    let mut world = snapshot(vec![]);
    world.merged = vec![handed_back, paused];

    assert_eq!(decide(&world), vec![]);
}

#[test]
fn a_spec_change_linewright_is_assigned_to_again_is_cut_again() {
    let retried = assigned(
        unassigned(assigned(merged(5, &["docs/spec.md"]), 1), LINEWRIGHT, 2),
        3,
    );
    let mut world = snapshot(vec![]);
    world.merged = vec![retried];

    assert_eq!(
        decide(&world),
        vec![Action::Start(Work::Tickets { pr: Number(5) })]
    );
}

#[test]
fn a_ready_for_human_label_on_a_spec_change_stops_nothing() {
    let mut labeled = merged(5, &["docs/spec.md"]);
    labeled.labels.push(label::READY_FOR_HUMAN.into());
    let mut world = snapshot(vec![]);
    world.merged = vec![labeled.clone()];
    assert_eq!(decide(&world), cut(5));

    world.merged = vec![assigned(labeled, 1)];
    assert_eq!(
        reconcile(
            &world,
            &cutting(5),
            &Memory::default(),
            &Workflows::default(),
            t(10),
            &policy()
        ),
        vec![]
    );
}

#[test]
fn a_cut_that_ended_since_the_snapshot_is_neither_started_again_nor_cancelled() {
    let mut world = snapshot(vec![]);
    world.merged = vec![merged(5, &["docs/spec.md"])];
    let memory = Memory {
        ended_cuts: vec![Number(5)],
        ..Memory::default()
    };

    assert_eq!(
        reconcile(
            &world,
            &[],
            &memory,
            &Workflows::default(),
            t(10),
            &policy()
        ),
        vec![]
    );
}

#[test]
fn a_cut_stopped_part_way_is_discarded_also_when_linewright_was_unassigned_from_it() {
    let mut world = snapshot(vec![tracking(9, 5, &[(10, true)])]);
    world.merged = vec![unassigned(
        assigned(merged(5, &["docs/spec.md"]), 1),
        "maintainer",
        2,
    )];

    assert_eq!(
        decide(&world),
        vec![Action::DiscardTickets {
            tracking: Number(9),
            tickets: vec![Number(10)],
        }]
    );
}

#[test]
fn unassigning_linewright_from_a_spec_change_cancels_its_cut() {
    let mut world = snapshot(vec![]);
    world.merged = vec![unassigned(
        assigned(merged(5, &["docs/spec.md"]), 1),
        "maintainer",
        2,
    )];

    assert_eq!(
        reconcile(
            &world,
            &cutting(5),
            &Memory::default(),
            &Workflows::default(),
            t(10),
            &policy()
        ),
        vec![Action::Cancel {
            subject: Subject::SpecChange(Number(5)),
            reason: "factory was unassigned from #5".into(),
        }]
    );
}

#[test]
fn only_unprocessed_spec_changes_by_people_within_the_lookback_are_picked_up() {
    let code_only = merged(1, &["src/main.rs", "README.md"]);
    let mut processed = merged(2, &["docs/spec.md"]);
    processed.labels.push(label::SPEC_PROCESSED.into());
    let mut by_linewright = merged(3, &["docs/spec.md"]);
    by_linewright.author = Login::new(LINEWRIGHT);
    let mut too_old = merged(4, &["docs/spec.md"]);
    too_old.merged_at = t(10) - TimeDelta::days(8);
    let mut other_base = merged(6, &["docs/spec.md"]);
    other_base.base_ref = "release".into();
    let mut world = snapshot(vec![]);
    world.merged = vec![code_only, processed, by_linewright, too_old, other_base];

    assert_eq!(decide(&world), vec![]);
}

#[test]
fn an_interrupted_decomposition_is_discarded_so_the_spec_change_is_cut_again() {
    let mut world = snapshot(vec![tracking(9, 5, &[(10, true)])]);
    world.merged = vec![assigned(merged(5, &["docs/spec.md"]), 1)];

    assert_eq!(
        decide(&world),
        vec![Action::DiscardTickets {
            tracking: Number(9),
            tickets: vec![Number(10)],
        }]
    );
    assert_eq!(
        reconcile(
            &world,
            &cutting(5),
            &Memory::default(),
            &Workflows::default(),
            t(10),
            &policy()
        ),
        vec![]
    );
}

fn ticket(number: u64) -> Issue {
    let mut ticket = labeled(issue(number), label::READY_FOR_AGENT);
    ticket.author = Login::new(LINEWRIGHT);
    ticket
}

fn claimed(mut issue: Issue) -> Issue {
    issue.assignees.push(Login::new(LINEWRIGHT));
    issue
}

fn blocked_by(mut issue: Issue, blocker: u64, open: bool) -> Issue {
    issue.blocked_by.push(Linked {
        number: Number(blocker),
        open,
    });
    issue
}

fn claim(issue: u64) -> Action {
    Action::Claim {
        item: Item::Issue(Number(issue)),
    }
}

fn implement(issue: u64) -> Action {
    Action::Start(Work::Implement {
        issue: Number(issue),
    })
}

#[test]
fn an_unblocked_ready_for_agent_issue_is_claimed_and_implemented() {
    let world = snapshot(vec![blocked_by(ticket(2), 1, false)]);

    assert_eq!(decide(&world), vec![claim(2), implement(2)]);
}

#[test]
fn a_ticket_waits_until_every_issue_blocking_it_is_closed() {
    let world = snapshot(vec![blocked_by(blocked_by(ticket(3), 1, false), 2, true)]);

    assert_eq!(decide(&world), vec![]);
}

#[test]
fn a_claimed_ticket_with_no_pull_request_and_no_live_run_starts_a_fresh_run() {
    let world = snapshot(vec![claimed(ticket(2))]);

    assert_eq!(decide(&world), vec![implement(2)]);
}

#[test]
fn a_ticket_with_a_live_run_is_left_to_it() {
    let world = snapshot(vec![claimed(ticket(2))]);
    let live = [LiveRun {
        subject: Subject::Issue(Number(2)),
        kind: WorkKind::Implement,
    }];

    assert_eq!(
        reconcile(
            &world,
            &live,
            &Memory::default(),
            &Workflows::default(),
            t(10),
            &policy()
        ),
        vec![]
    );
}

fn unassigned_by(mut issue: Issue, actor: &str, minute: i64) -> Issue {
    issue
        .events
        .push(IssueEvent::Assignment(Assignment::Unassigned {
            assignee: Login::new(LINEWRIGHT),
            actor: Login::new(actor),
            at: t(minute),
        }));
    issue
}

fn labeled_at(mut issue: Issue, label: &str, minute: i64) -> Issue {
    issue.events.push(IssueEvent::Labeled {
        label: label.into(),
        at: t(minute),
    });
    issue
}

#[test]
fn unassigning_linewright_stops_the_work_until_the_label_is_applied_again() {
    let stopped = unassigned_by(
        labeled_at(ticket(2), label::READY_FOR_AGENT, 1),
        "maintainer",
        2,
    );
    let restarted = labeled_at(
        unassigned_by(
            labeled_at(ticket(3), label::READY_FOR_AGENT, 1),
            "maintainer",
            2,
        ),
        label::READY_FOR_AGENT,
        3,
    );
    let handed_back_by_linewright = unassigned_by(
        labeled_at(ticket(4), label::READY_FOR_AGENT, 1),
        LINEWRIGHT,
        2,
    );

    assert_eq!(
        decide(&snapshot(vec![
            stopped,
            restarted,
            handed_back_by_linewright
        ])),
        vec![claim(3), implement(3), claim(4), implement(4),]
    );
}

#[test]
fn a_ticket_sent_back_to_triage_is_triaged_not_implemented() {
    let sent_back = labeled_at(
        labeled_at(
            labeled(ticket(2), label::NEEDS_TRIAGE),
            label::READY_FOR_AGENT,
            1,
        ),
        label::NEEDS_TRIAGE,
        2,
    );

    assert_eq!(
        decide(&snapshot(vec![sent_back])),
        vec![
            Action::RemoveStatusLabel {
                issue: Number(2),
                label: StatusLabel::ReadyForAgent,
            },
            claim(2),
            triage(2)
        ]
    );
}

fn live(issue: u64, kind: WorkKind) -> LiveRun {
    LiveRun {
        subject: Subject::Issue(Number(issue)),
        kind,
    }
}

#[test]
fn runs_start_up_to_the_ceiling_triage_before_tickets_before_fresh_implementations() {
    let mut world = snapshot(vec![
        claimed(ticket(1)),
        ticket(2),
        labeled(issue(3), label::NEEDS_TRIAGE),
        claimed(ticket(4)),
        claimed(ticket(9)),
    ]);
    world.merged = vec![merged(5, &["docs/spec.md"])];
    let busy = [live(9, WorkKind::Implement)];

    assert_eq!(
        reconcile(
            &world,
            &busy,
            &Memory::default(),
            &Workflows::default(),
            t(10),
            &policy()
        ),
        vec![
            claim(3),
            triage(3),
            Action::Claim {
                item: Item::Pull(Number(5))
            },
            Action::Start(Work::Tickets { pr: Number(5) }),
            implement(1),
        ]
    );
}

#[test]
fn a_run_is_cancelled_when_its_issue_stops_qualifying() {
    let mut unlabeled = claimed(issue(1));
    unlabeled.labels.clear();
    let unassigned = ticket(2);
    let mut closed = claimed(ticket(3));
    closed.open = false;
    let triaged = labeled(issue(5), label::READY_FOR_AGENT);
    let still_wanted = claimed(ticket(6));
    let world = snapshot(vec![unlabeled, unassigned, closed, triaged, still_wanted]);
    let runs = [
        live(1, WorkKind::Implement),
        live(2, WorkKind::Implement),
        live(3, WorkKind::Implement),
        live(5, WorkKind::Triage),
        live(6, WorkKind::Implement),
    ];

    let cancelled: Vec<Subject> = reconcile(
        &world,
        &runs,
        &Memory::default(),
        &Workflows::default(),
        t(10),
        &policy(),
    )
    .into_iter()
    .filter_map(|a| match a {
        Action::Cancel { subject, .. } => Some(subject),
        _ => None,
    })
    .collect();

    assert_eq!(
        cancelled,
        [1, 2, 3, 5].map(|n| Subject::Issue(Number(n))).to_vec()
    );
}

#[test]
fn a_pull_requests_run_is_cancelled_when_it_is_merged_closed_or_linewright_is_unassigned() {
    let mut merged = pull(20, 2);
    merged.state = PullState::Merged { at: t(5) };
    let mut closed = pull(21, 3);
    closed.state = PullState::Closed { at: t(5) };
    let unassigned = unassigned_at(pull(22, 4), "maintainer", 5, None);
    let still_wanted = labeled_pull(pull(23, 6), label::READY_FOR_HUMAN);
    let mut world = snapshot(vec![ticket(2), ticket(3), ticket(4), ticket(6)]);
    world.pulls = vec![merged, closed, unassigned, still_wanted];
    let runs = [20, 21, 22, 23, 24].map(|pr| live_on_pull(pr, WorkKind::Rework));

    let cancelled: Vec<(Subject, String)> = reconcile(
        &world,
        &runs,
        &Memory::default(),
        &Workflows::default(),
        t(10),
        &policy(),
    )
    .into_iter()
    .filter_map(|a| match a {
        Action::Cancel { subject, reason } => Some((subject, reason)),
        _ => None,
    })
    .collect();

    assert_eq!(
        cancelled,
        [
            (20, "#20 was merged"),
            (21, "#21 was closed"),
            (22, "factory was unassigned from #22"),
            (24, "#24 is no longer open"),
        ]
        .map(|(n, reason)| (Subject::Pull(Number(n)), reason.to_string()))
        .to_vec()
    );
}

#[test]
fn a_pull_request_linewright_is_unassigned_from_starts_nothing_and_waits_for_the_maintainer() {
    for by in ["maintainer", LINEWRIGHT] {
        let mut pr = unassigned_at(pull(20, 2), by, 5, None);
        pr.checks = vec![check("test", CheckState::Failed)];
        pr.comments = vec![comment("maintainer", 6, "Let me look at this.")];
        let world = in_review(pr);

        assert_eq!(at(&world, 10), vec![]);
        assert_eq!(waits_on(&world, 20), [Wait::Unassigned]);
    }
}

#[test]
fn a_pull_request_labeled_ready_for_human_is_worked_as_without_the_label() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("test", CheckState::Failed)];
    let world = in_review(labeled_pull(pr, label::READY_FOR_HUMAN));

    assert_eq!(at(&world, 10), vec![fix_ci(2, 20)]);
}

#[test]
fn linewrights_pull_request_it_was_never_assigned_to_is_claimed_and_worked() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("test", CheckState::Failed)];
    pr.assignees.clear();
    pr.assignments.clear();
    let world = in_review(pr);
    let runs = [live_on_pull(20, WorkKind::FixCi)];

    assert_eq!(
        at(&world, 10),
        vec![
            Action::Claim {
                item: Item::Pull(Number(20)),
            },
            fix_ci(2, 20)
        ]
    );
    assert_eq!(
        reconcile(
            &world,
            &runs,
            &Memory::default(),
            &Workflows::default(),
            t(10),
            &policy(),
        ),
        vec![Action::Claim {
            item: Item::Pull(Number(20)),
        }]
    );
}

#[test]
fn a_pull_request_assigned_to_someone_else_alone_is_claimed_once() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("test", CheckState::Passed)];
    pr.assignees = vec![Login::new("maintainer")];
    pr.assignments = vec![Assignment::Assigned {
        assignee: Login::new("maintainer"),
        at: t(2),
    }];

    assert_eq!(
        at(&in_review(pr), 10),
        vec![
            Action::Claim {
                item: Item::Pull(Number(20)),
            },
            Action::MarkReady { pr: Number(20) }
        ]
    );
}

#[test]
fn a_ticket_waits_while_its_pull_request_is_open_whatever_its_labels_and_assignees() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("test", CheckState::Pending)];
    let claimed = in_review(pr);
    let mut handed_back = claimed.clone();
    handed_back.issues[0] = labeled(issue(2), label::READY_FOR_HUMAN);
    let mut unlabeled = claimed.clone();
    unlabeled.issues[0].labels.clear();
    let mut for_triage = claimed.clone();
    for_triage.issues[0] = labeled(issue(2), label::NEEDS_TRIAGE);

    for world in [claimed, handed_back, unlabeled, for_triage] {
        assert!(
            !decide(&world)
                .iter()
                .any(|a| matches!(a, Action::Start(Work::Triage { .. })))
        );
        assert_eq!(
            waits_on(&world, 2),
            [Wait::PullRequest {
                pull_request: Number(20)
            }]
        );
        assert_eq!(
            waits_on(&world, 20),
            [Wait::Ci {
                pull_request: Number(20)
            }]
        );
    }
}

#[test]
fn a_pull_request_with_its_run_in_flight_waits_for_it() {
    let world = in_review(pull(20, 2));
    let runs = [live_on_pull(20, WorkKind::Update)];

    assert_eq!(
        waits(&world, &runs, &Memory::default(), 10, &policy()).remove(&Number(20)),
        Some(vec![Wait::Run {
            stage: WorkKind::Update
        }])
    );
}

fn ticket_branch(number: u64, issue: BranchIssue) -> TicketBranch {
    TicketBranch {
        name: TicketBranchName::parse(&format!("{number}-gadgets")).unwrap(),
        issue,
        open_pull_from_branch: false,
    }
}

fn read(issue: Issue) -> BranchIssue {
    BranchIssue::Read(Box::new(issue))
}

fn closed_unpulled(number: u64) -> BranchIssue {
    read(closed(ticket(number)))
}

fn closed(mut issue: Issue) -> Issue {
    issue.open = false;
    issue
}

fn deleted_branches(actions: Vec<Action>) -> Vec<String> {
    actions
        .into_iter()
        .filter_map(|a| match a {
            Action::DeleteBranch { branch } => Some(branch.into()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_branch_of_a_closed_ticket_is_deleted_unless_a_pull_request_is_open_for_it() {
    let mut closed_by_pull = closed(ticket(2));
    closed_by_pull.closed_by = vec![ClosingPull {
        number: Number(20),
        author: Login::new("alice"),
        state: PullState::Open { draft: false },
    }];
    let mut closed_by_merged_pull = closed(ticket(7));
    closed_by_merged_pull.closed_by = vec![ClosingPull {
        number: Number(70),
        author: Login::new("alice"),
        state: PullState::Merged { at: t(5) },
    }];
    let mut closed_by_open_pull = closed(ticket(6));
    closed_by_open_pull.closed_by = closed_by_pull.closed_by.clone();
    let mut world = snapshot(vec![
        closed(ticket(1)),
        closed_by_pull,
        closed(ticket(3)),
        ticket(4),
        closed_by_merged_pull,
    ]);
    let mut pulled_from = ticket_branch(3, BranchIssue::Held);
    pulled_from.open_pull_from_branch = true;
    world.ticket_branches = vec![
        ticket_branch(1, BranchIssue::Held),
        ticket_branch(2, BranchIssue::Held),
        pulled_from,
        ticket_branch(4, BranchIssue::Held),
        ticket_branch(5, closed_unpulled(5)),
        ticket_branch(6, read(closed_by_open_pull)),
        ticket_branch(7, BranchIssue::Held),
        ticket_branch(8, read(ticket(8))),
    ];

    assert_eq!(
        deleted_branches(decide(&world)),
        ["1-gadgets", "5-gadgets", "7-gadgets"]
    );
}

#[test]
fn the_branch_of_a_closed_ticket_waits_for_its_run_to_stop() {
    let mut world = snapshot(vec![closed(claimed(ticket(1)))]);
    world.ticket_branches = vec![ticket_branch(1, BranchIssue::Held)];

    let actions = reconcile(
        &world,
        &[live(1, WorkKind::Implement)],
        &Memory::default(),
        &Workflows::default(),
        t(10),
        &policy(),
    );

    assert_eq!(deleted_branches(actions), Vec::<String>::new());
}

#[test]
fn the_branch_of_a_ticket_read_again_is_kept_or_deleted_as_the_ticket_now_is() {
    // Read closed with its branch, then reopened; read open with its branch,
    // then closed; held with its branch, then left out of the snapshot.
    let mut world = snapshot(vec![ticket(1), closed(ticket(2))]);
    world.ticket_branches = vec![
        ticket_branch(1, closed_unpulled(1)),
        ticket_branch(2, BranchIssue::Held),
        ticket_branch(3, BranchIssue::Held),
    ];

    assert_eq!(deleted_branches(decide(&world)), ["2-gadgets"]);
}

#[test]
fn a_tickets_run_is_cancelled_when_its_spec_change_is_marked_processed() {
    let mut processed = merged(5, &["docs/spec.md"]);
    processed.labels.push(label::SPEC_PROCESSED.into());
    let mut world = snapshot(vec![]);
    world.merged = vec![processed];
    let runs = [LiveRun {
        subject: Subject::SpecChange(Number(5)),
        kind: WorkKind::Tickets,
    }];

    assert!(matches!(
        reconcile(
            &world,
            &runs,
            &Memory::default(),
            &Workflows::default(),
            t(10),
            &policy()
        )
        .as_slice(),
        [Action::Cancel {
            subject: Subject::SpecChange(Number(5)),
            ..
        }]
    ));
}

fn pull(number: u64, closes: u64) -> Pull {
    Pull {
        number: Number(number),
        title: format!("Ticket {closes}"),
        body: format!("Closes #{closes}"),
        author: Login::new(LINEWRIGHT),
        created_at: t(1),
        head_ref: format!("{closes}-ticket"),
        head_sha: "abc123".into(),
        state: PullState::Open { draft: true },
        closes: vec![Number(closes)],
        pushes: vec![Push {
            by: Login::new(LINEWRIGHT),
            at: t(1),
        }],
        mergeability: Mergeability::Clean,
        rebaseability: Rebaseability::Rebaseable,
        checks: vec![],
        comments: vec![],
        reviews: vec![],
        threads: vec![],
        labels: vec![],
        assignees: vec![Login::new(LINEWRIGHT)],
        assignments: vec![Assignment::Assigned {
            assignee: Login::new(LINEWRIGHT),
            at: t(1),
        }],
    }
}

fn check(name: &str, state: CheckState) -> Check {
    Check {
        name: name.into(),
        state,
    }
}

fn in_review(pr: Pull) -> Snapshot {
    let issue = pr.closes[0].0;
    let mut ticket = claimed(ticket(issue));
    ticket.assignees.push(Login::new("maintainer"));
    let mut world = snapshot(vec![ticket]);
    world.pulls = vec![pr];
    world
}

fn at(world: &Snapshot, minute: i64) -> Vec<Action> {
    reconcile(
        world,
        &[],
        &Memory::default(),
        &Workflows::default(),
        t(minute),
        &policy(),
    )
}

#[test]
fn a_green_draft_is_marked_ready_for_review_with_no_one_asked_to_review_it() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("test", CheckState::Passed)];

    assert_eq!(
        at(&in_review(pr), 10),
        vec![Action::MarkReady { pr: Number(20) }]
    );
}

#[test]
fn a_draft_with_no_checks_by_the_end_of_the_grace_period_has_no_ci() {
    let world = in_review(pull(20, 2));

    assert_eq!(at(&world, 5), vec![]);
    assert!(matches!(
        at(&world, 7).as_slice(),
        [Action::MarkReady { .. }]
    ));
}

#[test]
fn a_draft_waits_while_checks_run() {
    let mut pr = pull(20, 2);
    pr.checks = vec![
        check("test", CheckState::Passed),
        check("lint", CheckState::Pending),
    ];

    assert_eq!(at(&in_review(pr), 30), vec![]);
}

fn done(pr: u64, kind: WorkKind, minute: i64) -> DoneRun {
    DoneRun {
        pr: Number(pr),
        kind,
        done_at: t(minute),
    }
}

fn deciding_at(world: &Snapshot, memory: &Memory, policy: &Policy, minute: i64) -> Vec<Action> {
    reconcile(world, &[], memory, &Workflows::default(), t(minute), policy)
}

fn after_runs(world: &Snapshot, runs: &[DoneRun], minute: i64) -> Vec<Action> {
    let memory = Memory {
        done_runs: runs.to_vec(),
        ..Memory::default()
    };
    deciding_at(world, &memory, &policy(), minute)
}

#[test]
fn a_failing_check_starts_a_fix_and_after_the_last_attempt_the_pull_request_is_handed_back() {
    let mut pr = pull(20, 2);
    pr.checks = vec![
        check("test", CheckState::Failed),
        check("lint", CheckState::Passed),
    ];
    let world = in_review(pr);
    let fixed = [done(20, WorkKind::FixCi, 3), done(20, WorkKind::FixCi, 6)];

    assert_eq!(
        at(&world, 10),
        vec![Action::Start(Work::FixCi {
            issue: Number(2),
            pr: Number(20),
            failing: vec!["test".into()],
        })]
    );
    assert!(matches!(
        after_runs(&world, &fixed, 10).as_slice(),
        [Action::HandBack {
            item: Item::Pull(Number(20)), reason }] if reason.contains("`test`")
    ));
}

#[test]
fn fix_attempts_count_only_the_pull_requests_own_fix_runs_since_its_last_rework() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("test", CheckState::Failed)];
    let world = in_review(pr);
    let runs = [
        done(20, WorkKind::FixCi, 2),
        done(20, WorkKind::FixCi, 3),
        done(20, WorkKind::Rework, 4),
        done(20, WorkKind::Update, 5),
        done(20, WorkKind::FixCi, 6),
        done(21, WorkKind::FixCi, 7),
    ];

    assert_eq!(after_runs(&world, &runs, 10), vec![fix_ci(2, 20)]);
}

#[test]
fn a_pull_request_handed_back_after_a_ci_timeout_waits_a_full_timeout_again_once_reassigned() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("e2e", CheckState::Pending)];
    pr.comments = vec![comment(
        LINEWRIGHT,
        6 * 60 + 3,
        "CI did not complete within 6h: `e2e` still running.",
    )];
    let world = in_review(unassigned_at(pr, LINEWRIGHT, 6 * 60 + 3, Some(6 * 60 + 5)));

    assert_eq!(at(&world, 6 * 60 + 10), vec![]);
    assert!(matches!(
        at(&world, 12 * 60 + 6).as_slice(),
        [Action::HandBack {
            item: Item::Pull(Number(20)),
            ..
        }]
    ));
}

#[test]
fn reassigning_linewright_gives_the_pull_request_fresh_ci_fix_attempts() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("test", CheckState::Failed)];
    let world = in_review(unassigned_at(pr, LINEWRIGHT, 8, Some(9)));
    let fixed = [done(20, WorkKind::FixCi, 3), done(20, WorkKind::FixCi, 6)];

    assert_eq!(
        after_runs(&world, &fixed, 10),
        vec![Action::Start(Work::FixCi {
            issue: Number(2),
            pr: Number(20),
            failing: vec!["test".into()],
        })]
    );
}

#[test]
fn reassigning_linewright_to_a_pull_request_a_rework_handed_back_addresses_its_feedback_again() {
    let mut pr = ready(pull(20, 2));
    pr.comments = vec![
        comment("maintainer", 4, "Please rename `foo`."),
        comment(LINEWRIGHT, 5, "The run failed twice on its infrastructure."),
    ];
    let world = in_review(unassigned_at(pr, LINEWRIGHT, 5, Some(7)));

    assert_eq!(at(&world, 10), vec![rework(2, 20)]);
}

#[test]
fn reassigning_linewright_reworks_for_the_feedback_left_while_it_was_unassigned() {
    let mut pr = ready(pull(20, 2));
    pr.reviews = vec![review(
        "maintainer",
        ReviewState::ChangesRequested,
        "Rename `foo`.",
        6,
    )];
    let paused = in_review(unassigned_at(pr.clone(), "maintainer", 5, None));
    let resumed = in_review(unassigned_at(pr, "maintainer", 5, Some(8)));

    assert_eq!(at(&paused, 10), vec![]);
    assert_eq!(at(&resumed, 10), vec![rework(2, 20)]);
}

#[test]
fn checks_that_never_complete_hand_the_pull_request_back_at_the_ci_timeout() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("e2e", CheckState::Pending)];
    let world = in_review(pr);

    assert_eq!(at(&world, 6 * 60), vec![]);
    assert!(matches!(
        at(&world, 6 * 60 + 2).as_slice(),
        [Action::HandBack {
            item: Item::Pull(Number(20)), reason }] if reason.contains("`e2e`")
    ));
}

fn rework(issue: u64, pr: u64) -> Action {
    Action::Start(Work::Rework {
        issue: Number(issue),
        pr: Number(pr),
    })
}

fn ready(mut pr: Pull) -> Pull {
    pr.state = PullState::Open { draft: false };
    pr.checks = vec![check("test", CheckState::Passed)];
    pr
}

#[test]
fn a_comment_after_linewrights_last_push_starts_a_rework_run() {
    let mut pr = ready(pull(20, 2));
    pr.comments = vec![comment("maintainer", 4, "Please rename `foo`.")];

    assert_eq!(at(&in_review(pr), 10), vec![rework(2, 20)]);
}

fn review(author: &str, state: ReviewState, body: &str, minute: i64) -> Review {
    Review {
        author: Login::new(author),
        state,
        body: body.into(),
        submitted_at: t(minute),
    }
}

#[test]
fn a_review_with_comments_starts_a_rework_run_but_one_that_only_approves_does_not() {
    let mut pr = ready(pull(20, 2));
    pr.reviews = vec![review("maintainer", ReviewState::Approved, "", 4)];
    let approved = in_review(pr.clone());
    pr.threads = vec![Thread {
        id: "T1".into(),
        path: "src/lib.rs".into(),
        resolved: false,
        comments: vec![comment("maintainer", 4, "Nit: typo.")],
    }];
    let approved_with_a_comment = in_review(pr);

    assert_eq!(at(&approved, 10), vec![]);
    assert_eq!(at(&approved_with_a_comment, 10), vec![rework(2, 20)]);
}

#[test]
fn feedback_already_answered_starts_nothing() {
    let mut pr = ready(pull(20, 2));
    pr.comments = vec![
        comment("reviewer", 0, "Looks odd."),
        comment(LINEWRIGHT, 3, "Thanks!"),
    ];
    let before_the_push = in_review(pr.clone());
    pr.comments = vec![comment("coverage[bot]", 4, "Coverage dropped 0.1%.")];
    let answered_without_a_push = in_review(pr);

    assert_eq!(at(&before_the_push, 10), vec![]);
    assert_eq!(
        after_runs(
            &answered_without_a_push,
            &[done(20, WorkKind::Rework, 5)],
            10
        ),
        vec![]
    );
}

#[test]
fn feedback_after_a_rework_that_needed_no_change_starts_another() {
    let mut pr = ready(pull(20, 2));
    pr.comments = vec![
        comment("coverage[bot]", 4, "Coverage dropped 0.1%."),
        comment("maintainer", 6, "Please rename `foo`."),
    ];
    let world = in_review(pr);

    assert_eq!(
        after_runs(&world, &[done(20, WorkKind::Rework, 5)], 10),
        vec![rework(2, 20)]
    );
}

#[test]
fn linewrights_comments_answer_no_feedback() {
    let mut pr = ready(pull(20, 2));
    pr.comments = vec![
        comment("coverage[bot]", 4, "Coverage dropped 0.1%."),
        comment(
            LINEWRIGHT,
            5,
            "> *Linewright, rework*\n\nNo change needed: noise.",
        ),
    ];

    assert_eq!(at(&in_review(pr), 10), vec![rework(2, 20)]);
}

fn update(issue: u64, pr: u64) -> Action {
    Action::Start(Work::Update {
        issue: Number(issue),
        pr: Number(pr),
    })
}

#[test]
fn a_pull_request_behind_or_in_conflict_with_the_default_branch_is_brought_up_to_date() {
    let mut behind = ready(pull(20, 2));
    behind.mergeability = Mergeability::Behind;
    let mut conflicting = ready(pull(20, 2));
    conflicting.mergeability = Mergeability::Conflicting;

    assert_eq!(at(&in_review(behind), 10), vec![update(2, 20)]);
    assert_eq!(at(&in_review(conflicting), 10), vec![update(2, 20)]);
}

fn not_rebaseable() -> Pull {
    let mut pr = ready(pull(20, 2));
    pr.checks = vec![check("test", CheckState::Passed)];
    pr.rebaseability = Rebaseability::NotRebaseable;
    pr
}

fn deciding(world: &Snapshot, memory: &Memory, policy: &Policy) -> Vec<Action> {
    deciding_at(world, memory, policy, 10)
}

fn tried(head: &str, minute: i64) -> TriedRebase {
    TriedRebase {
        pr: Number(20),
        head: head.into(),
        at: t(minute),
    }
}

fn rebase_update() -> Vec<Action> {
    vec![Action::RememberRebase(tried("abc123", 10)), update(2, 20)]
}

#[test]
fn a_pull_request_github_reports_not_rebaseable_is_rebased_where_rebase_merging_is_allowed() {
    let world = in_review(not_rebaseable());
    let rebasing = Policy {
        update_rule: UpdateRule::Always(UpdateMethod::Rebase),
        ..policy()
    };

    assert_eq!(at(&world, 10), rebase_update());
    assert_eq!(
        deciding(&world, &Memory::default(), &rebasing),
        rebase_update()
    );
}

#[test]
fn rebaseability_starts_no_run_without_rebase_merging_or_with_update_branch_merge() {
    let world = in_review(not_rebaseable());
    let no_rebase_merging = Policy {
        allowed_merge_methods: [MergeMethod::Merge, MergeMethod::Squash]
            .into_iter()
            .collect(),
        update_rule: UpdateRule::Always(UpdateMethod::Merge),
        ..policy()
    };
    let linear_squash_only = Policy {
        allowed_merge_methods: [MergeMethod::Squash].into_iter().collect(),
        update_rule: UpdateRule::Always(UpdateMethod::Rebase),
        ..policy()
    };
    let update_branch_merge = Policy {
        update_rule: UpdateRule::Always(UpdateMethod::Merge),
        ..policy()
    };

    for policy in [no_rebase_merging, linear_squash_only, update_branch_merge] {
        assert_eq!(deciding(&world, &Memory::default(), &policy), vec![]);
    }
}

#[test]
fn a_pull_request_whose_rebaseability_is_not_known_yet_waits_rather_than_starting_a_run() {
    let mut pr = not_rebaseable();
    pr.rebaseability = Rebaseability::Unknown;

    assert_eq!(at(&in_review(pr), 10), vec![]);
}

fn remembering(rebase: TriedRebase) -> Memory {
    Memory {
        tried_rebases: vec![rebase],
        ..Memory::default()
    }
}

#[test]
fn a_pull_request_still_not_rebaseable_after_a_rebase_at_its_head_starts_no_more_runs() {
    let world = in_review(not_rebaseable());

    assert_eq!(
        deciding(&world, &remembering(tried("abc123", 5)), &policy()),
        vec![]
    );
    assert_eq!(
        deciding(&world, &remembering(tried("an-earlier-head", 5)), &policy()),
        rebase_update()
    );
}

#[test]
fn a_pull_request_behind_is_rebased_again_though_a_rebase_was_tried_at_its_head() {
    let mut pr = not_rebaseable();
    pr.mergeability = Mergeability::Behind;

    assert_eq!(
        deciding(&in_review(pr), &remembering(tried("abc123", 5)), &policy()),
        rebase_update()
    );
}

#[test]
fn a_pull_request_handed_back_since_a_rebase_was_tried_tries_it_again_once_reassigned() {
    let unassigned = in_review(unassigned_at(not_rebaseable(), LINEWRIGHT, 7, None));
    let reassigned = in_review(unassigned_at(not_rebaseable(), LINEWRIGHT, 7, Some(8)));

    assert_eq!(
        deciding(&unassigned, &remembering(tried("abc123", 5)), &policy()),
        vec![]
    );
    assert_eq!(
        deciding(&reassigned, &remembering(tried("abc123", 5)), &policy()),
        rebase_update()
    );
}

#[test]
fn a_ticket_labeled_again_since_a_rebase_was_tried_does_not_try_it_again() {
    let mut world = in_review(not_rebaseable());
    world.issues[0].events = vec![IssueEvent::Labeled {
        label: label::READY_FOR_AGENT.into(),
        at: t(7),
    }];

    assert_eq!(
        deciding(&world, &remembering(tried("abc123", 5)), &policy()),
        vec![]
    );
}

fn merging_by(method: MergeMethod) -> Policy {
    Policy {
        auto_merge: Some(method),
        ..policy()
    }
}

#[test]
fn with_auto_merge_by_rebase_a_pull_request_not_rebaseable_is_rebased_and_not_merged() {
    let world = in_review(not_rebaseable());
    let mut unknown = not_rebaseable();
    unknown.rebaseability = Rebaseability::Unknown;
    let mut rebaseable = not_rebaseable();
    rebaseable.rebaseability = Rebaseability::Rebaseable;
    let by_rebase = merging_by(MergeMethod::Rebase);

    assert_eq!(
        deciding(&world, &Memory::default(), &by_rebase),
        rebase_update()
    );
    assert_eq!(
        deciding(&in_review(unknown), &Memory::default(), &by_rebase),
        vec![]
    );
    assert_eq!(
        deciding(&in_review(rebaseable), &Memory::default(), &by_rebase),
        vec![merge_by(20, false, MergeMethod::Rebase)]
    );
}

#[test]
fn with_auto_merge_by_rebase_a_pull_request_still_not_rebaseable_after_a_rebase_is_handed_back() {
    let world = in_review(not_rebaseable());

    assert_eq!(
        deciding(
            &world,
            &remembering(tried("abc123", 5)),
            &merging_by(MergeMethod::Rebase)
        ),
        vec![Action::HandBack {
            item: Item::Pull(Number(20)),
            reason: "GitHub reports #20 not rebaseable though Linewright rebased it onto the \
                     default branch, so it cannot be merged by rebase."
                .into(),
        }]
    );
}

#[test]
fn with_auto_merge_by_another_method_a_pull_request_still_not_rebaseable_after_a_rebase_is_merged()
{
    let world = in_review(not_rebaseable());

    assert_eq!(
        deciding(
            &world,
            &remembering(tried("abc123", 5)),
            &merging_by(MergeMethod::Squash)
        ),
        vec![merge_by(20, false, MergeMethod::Squash)]
    );
}

fn auto_merging(world: &Snapshot, minute: i64) -> Vec<Action> {
    auto_merging_after(world, &[], minute)
}

fn merge(pr: u64, draft: bool) -> Action {
    merge_by(pr, draft, MergeMethod::Merge)
}

fn merge_by(pr: u64, draft: bool, method: MergeMethod) -> Action {
    Action::Merge {
        pr: Number(pr),
        head: "abc123".into(),
        draft,
        method,
    }
}

#[test]
fn with_auto_merge_a_green_draft_is_merged_instead_of_marked_ready() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("test", CheckState::Passed)];

    assert_eq!(auto_merging(&in_review(pr), 10), vec![merge(20, true)]);
}

#[test]
fn with_auto_merge_a_draft_with_no_ci_by_the_end_of_the_grace_period_is_merged() {
    let world = in_review(pull(20, 2));

    assert_eq!(auto_merging(&world, 5), vec![]);
    assert!(matches!(
        auto_merging(&world, 7).as_slice(),
        [Action::Merge { draft: true, .. }]
    ));
}

#[test]
fn with_auto_merge_a_green_pull_request_already_ready_for_review_is_merged() {
    let pr = ready(pull(20, 2));

    assert_eq!(auto_merging(&in_review(pr), 10), vec![merge(20, false)]);
}

#[test]
fn with_auto_merge_a_pull_request_not_yet_green_keeps_its_handling_and_is_not_merged() {
    let mut pending = ready(pull(20, 2));
    pending.checks = vec![check("test", CheckState::Pending)];
    let mut failing = ready(pull(20, 2));
    failing.checks = vec![check("test", CheckState::Failed)];
    let mut commented = ready(pull(20, 2));
    commented.comments = vec![comment("maintainer", 4, "Please rename `foo`.")];
    let mut behind = ready(pull(20, 2));
    behind.mergeability = Mergeability::Behind;
    let mut conflicting = ready(pull(20, 2));
    conflicting.mergeability = Mergeability::Conflicting;

    assert_eq!(auto_merging(&in_review(pending), 10), vec![]);
    assert_eq!(
        auto_merging(&in_review(failing), 10),
        vec![Action::Start(Work::FixCi {
            issue: Number(2),
            pr: Number(20),
            failing: vec!["test".into()],
        })]
    );
    assert_eq!(auto_merging(&in_review(commented), 10), vec![rework(2, 20)]);
    assert_eq!(auto_merging(&in_review(behind), 10), vec![update(2, 20)]);
    assert_eq!(
        auto_merging(&in_review(conflicting), 10),
        vec![update(2, 20)]
    );
}

#[test]
fn with_auto_merge_a_pull_request_whose_mergeability_is_not_known_yet_waits() {
    let mut pr = ready(pull(20, 2));
    pr.mergeability = Mergeability::Unknown;

    assert_eq!(auto_merging(&in_review(pr), 10), vec![]);
}

fn code_owner_review() -> Vec<Requirement> {
    vec![Requirement::CodeOwnerReview {
        owners: vec!["alice".into()],
    }]
}

#[test]
fn with_auto_merge_a_green_pull_request_github_blocks_is_held_for_review_and_not_merged() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("test", CheckState::Passed)];
    pr.mergeability = Mergeability::Blocked {
        requires: code_owner_review(),
    };

    assert_eq!(
        auto_merging(&in_review(pr), 10),
        vec![Action::HoldForReview {
            pr: Number(20),
            draft: true,
            requires: code_owner_review(),
        }]
    );
}

#[test]
fn a_pull_request_github_blocks_is_held_for_review_after_it_is_marked_ready() {
    let mut pr = ready(pull(20, 2));
    pr.mergeability = Mergeability::Blocked {
        requires: code_owner_review(),
    };

    assert_eq!(
        auto_merging(&in_review(pr), 10),
        vec![Action::HoldForReview {
            pr: Number(20),
            draft: false,
            requires: code_owner_review(),
        }]
    );
}

fn held_for_a_code_owner() -> Memory {
    let mut memory = Memory::default();
    memory.held.hold(Number(20), &code_owner_review());
    memory
}

fn auto_merging_remembering(world: &Snapshot, memory: &Memory, minute: i64) -> Vec<Action> {
    deciding_at(world, memory, &merging_by(MergeMethod::Merge), minute)
}

#[test]
fn a_pull_request_github_blocks_and_already_held_for_review_waits_also_after_a_push() {
    let mut pr = ready(pull(20, 2));
    pr.mergeability = Mergeability::Blocked {
        requires: code_owner_review(),
    };
    let held = in_review(pr.clone());
    pr.pushes.push(Push {
        by: Login::new(LINEWRIGHT),
        at: t(3),
    });
    let updated_since = in_review(pr);

    assert_eq!(
        auto_merging_remembering(&held, &held_for_a_code_owner(), 10),
        vec![]
    );
    assert_eq!(
        auto_merging_remembering(&updated_since, &held_for_a_code_owner(), 10),
        vec![]
    );
}

#[test]
fn a_pull_request_held_for_review_is_merged_once_github_no_longer_blocks_it() {
    let world = in_review(ready(pull(20, 2)));

    assert_eq!(
        auto_merging_remembering(&world, &held_for_a_code_owner(), 10),
        vec![merge(20, false)]
    );
}

#[test]
fn a_pull_request_is_held_again_when_what_github_requires_changes() {
    let mut pr = ready(pull(20, 2));
    let requires = vec![Requirement::Check {
        name: "deploy".into(),
    }];
    pr.mergeability = Mergeability::Blocked {
        requires: requires.clone(),
    };

    assert_eq!(
        auto_merging_remembering(&in_review(pr), &held_for_a_code_owner(), 10),
        vec![Action::HoldForReview {
            pr: Number(20),
            draft: false,
            requires,
        }]
    );
}

#[test]
fn a_pull_request_is_held_whatever_linewrights_comments_on_it_say() {
    let mut pr = ready(pull(20, 2));
    pr.mergeability = Mergeability::Blocked {
        requires: code_owner_review(),
    };
    pr.comments = vec![comment(
        LINEWRIGHT,
        7,
        "> *Linewright, review*\n\nReady for review: GitHub requires a code owner's review \
         (@alice) before merging. Linewright merges it once GitHub allows it.",
    )];

    assert!(matches!(
        auto_merging(&in_review(pr), 10).as_slice(),
        [Action::HoldForReview { draft: false, .. }]
    ));
}

#[test]
fn with_auto_merge_a_green_draft_whose_merge_state_is_unknown_is_marked_ready_first() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("test", CheckState::Passed)];
    pr.mergeability = Mergeability::Unknown;

    assert_eq!(
        auto_merging(&in_review(pr), 10),
        vec![Action::MarkReadyForMergeState { pr: Number(20) }]
    );
}

#[test]
fn without_auto_merge_a_green_pull_request_github_blocks_is_marked_ready_as_any_other() {
    let mut draft = pull(20, 2);
    draft.checks = vec![check("test", CheckState::Passed)];
    draft.mergeability = Mergeability::Blocked {
        requires: code_owner_review(),
    };
    let ready_for_review = ready(draft.clone());

    assert_eq!(
        at(&in_review(draft), 10),
        vec![Action::MarkReady { pr: Number(20) }]
    );
    assert_eq!(at(&in_review(ready_for_review), 10), vec![]);
}

/// The pull request with a push by Linewright at minute 5, which answers the
/// feedback before it.
fn reworked(mut pr: Pull) -> Pull {
    pr.pushes.push(Push {
        by: Login::new(LINEWRIGHT),
        at: t(5),
    });
    pr
}

#[test]
fn with_auto_merge_a_green_pull_request_a_person_commented_on_waits_for_their_approval() {
    let mut pr = reworked(pull(20, 2));
    pr.checks = vec![check("test", CheckState::Passed)];
    pr.comments = vec![comment("reviewer", 3, "Please rename `foo`.")];
    let draft = in_review(pr.clone());
    let ready = in_review(ready(pr));

    assert_eq!(
        auto_merging(&draft, 10),
        vec![Action::MarkReady { pr: Number(20) }]
    );
    assert_eq!(auto_merging(&ready, 10), vec![]);
}

#[test]
fn with_auto_merge_a_pull_request_a_person_commented_on_merges_once_they_approve() {
    let mut pr = reworked(ready(pull(20, 2)));
    pr.threads = vec![Thread {
        id: "T1".into(),
        path: "src/lib.rs".into(),
        resolved: false,
        comments: vec![comment("reviewer", 3, "Nit: typo.")],
    }];
    pr.reviews = vec![review("reviewer", ReviewState::Approved, "", 6)];

    assert_eq!(auto_merging(&in_review(pr), 10), vec![merge(20, false)]);
}

#[test]
fn with_auto_merge_a_person_whose_latest_review_does_not_approve_holds_the_merge() {
    let approved_then = |state| {
        let mut pr = reworked(ready(pull(20, 2)));
        pr.reviews = vec![
            review("reviewer", ReviewState::Approved, "", 2),
            review("reviewer", state, "", 3),
        ];
        in_review(pr)
    };

    assert_eq!(
        auto_merging(&approved_then(ReviewState::ChangesRequested), 10),
        vec![]
    );
    assert_eq!(
        auto_merging(&approved_then(ReviewState::Dismissed), 10),
        vec![]
    );
}

#[test]
fn with_auto_merge_a_person_who_comments_after_approving_holds_the_merge_until_they_approve_again()
{
    let mut pr = reworked(pull(20, 2));
    pr.checks = vec![check("test", CheckState::Passed)];
    pr.reviews = vec![review("reviewer", ReviewState::Approved, "", 2)];
    pr.comments = vec![comment("reviewer", 3, "Also change `bar`.")];
    let commented = in_review(pr.clone());
    pr.state = PullState::Open { draft: false };
    pr.reviews
        .push(review("reviewer", ReviewState::Approved, "", 6));
    let approved_again = in_review(pr);

    assert_eq!(
        auto_merging(&commented, 10),
        vec![Action::MarkReady { pr: Number(20) }]
    );
    assert_eq!(auto_merging(&approved_again, 10), vec![merge(20, false)]);
}

#[test]
fn with_auto_merge_a_person_who_replies_in_a_thread_after_approving_holds_the_merge_until_they_approve_again()
 {
    let mut pr = reworked(ready(pull(20, 2)));
    pr.reviews = vec![review("reviewer", ReviewState::Approved, "", 2)];
    pr.threads = vec![Thread {
        id: "T1".into(),
        path: "src/lib.rs".into(),
        resolved: false,
        comments: vec![
            comment("maintainer", 1, "Why `foo`?"),
            comment("reviewer", 3, "Agreed, rename it."),
        ],
    }];
    pr.reviews
        .push(review("maintainer", ReviewState::Approved, "", 4));
    let replied = in_review(pr.clone());
    pr.reviews
        .push(review("reviewer", ReviewState::Approved, "", 6));
    let approved_again = in_review(pr);

    assert_eq!(auto_merging(&replied, 10), vec![]);
    assert_eq!(auto_merging(&approved_again, 10), vec![merge(20, false)]);
}

#[test]
fn with_auto_merge_an_approval_with_inline_comments_does_not_hold_the_merge() {
    let mut pr = reworked(ready(pull(20, 2)));
    pr.threads = vec![Thread {
        id: "T1".into(),
        path: "src/lib.rs".into(),
        resolved: false,
        comments: vec![comment("reviewer", 3, "Nit: typo.")],
    }];
    pr.reviews = vec![review("reviewer", ReviewState::Approved, "", 3)];

    assert_eq!(auto_merging(&in_review(pr), 10), vec![merge(20, false)]);
}

#[test]
fn with_auto_merge_a_pull_request_merges_once_each_persons_newest_comment_or_review_approves() {
    let mut pr = reworked(ready(pull(20, 2)));
    pr.reviews = vec![
        review("reviewer", ReviewState::Approved, "", 2),
        review("maintainer", ReviewState::Approved, "", 2),
    ];
    pr.comments = vec![comment("maintainer", 3, "Also change `bar`.")];
    let one_commented = in_review(pr.clone());
    pr.comments
        .push(comment("reviewer", 4, "And `baz` while at it."));
    pr.reviews
        .push(review("maintainer", ReviewState::Approved, "", 6));
    let other_commented = in_review(pr.clone());
    pr.reviews
        .push(review("reviewer", ReviewState::Approved, "", 7));
    let both_approved = in_review(pr);

    assert_eq!(auto_merging(&one_commented, 10), vec![]);
    assert_eq!(auto_merging(&other_commented, 10), vec![]);
    assert_eq!(auto_merging(&both_approved, 10), vec![merge(20, false)]);
}

#[test]
fn with_auto_merge_comments_from_linewright_after_an_approval_do_not_hold_the_merge() {
    let mut pr = reworked(ready(pull(20, 2)));
    pr.reviews = vec![review("reviewer", ReviewState::Approved, "", 2)];
    pr.comments = vec![comment(LINEWRIGHT, 3, "Renamed `foo`.")];

    assert_eq!(auto_merging(&in_review(pr), 10), vec![merge(20, false)]);
}

#[test]
fn with_auto_merge_a_pull_request_two_people_commented_on_merges_once_both_approve() {
    let mut pr = reworked(ready(pull(20, 2)));
    pr.comments = vec![
        comment("reviewer", 2, "Please rename `foo`."),
        comment("maintainer", 3, "And `bar`."),
    ];
    pr.reviews = vec![review("reviewer", ReviewState::Approved, "", 6)];
    let one_approved = in_review(pr.clone());
    pr.reviews
        .push(review("maintainer", ReviewState::Approved, "", 7));
    let both_approved = in_review(pr);

    assert_eq!(auto_merging(&one_approved, 10), vec![]);
    assert_eq!(auto_merging(&both_approved, 10), vec![merge(20, false)]);
}

#[test]
fn with_auto_merge_comments_and_reviews_from_bots_do_not_hold_the_merge() {
    let mut pr = reworked(ready(pull(20, 2)));
    pr.comments = vec![comment("coverage[bot]", 2, "Coverage dropped 0.1%.")];
    pr.reviews = vec![review(
        "linter[bot]",
        ReviewState::ChangesRequested,
        "Style issues.",
        3,
    )];

    assert_eq!(auto_merging(&in_review(pr), 10), vec![merge(20, false)]);
}

fn failed_merge(pr: u64, reason: &str) -> FailedMerge {
    failed_merge_at(pr, reason, 9)
}

fn failed_merge_at(pr: u64, reason: &str, minute: i64) -> FailedMerge {
    FailedMerge {
        pr: Number(pr),
        reason: reason.into(),
        at: t(minute),
    }
}

fn auto_merging_after(world: &Snapshot, failed: &[FailedMerge], minute: i64) -> Vec<Action> {
    let memory = Memory {
        failed_merges: failed.to_vec(),
        ..Memory::default()
    };
    auto_merging_remembering(world, &memory, minute)
}

#[test]
fn with_auto_merge_a_green_pull_request_whose_merge_failed_once_is_merged_again() {
    let world = in_review(ready(pull(20, 2)));
    let failed = [failed_merge(20, "Head branch was modified.")];

    assert_eq!(
        auto_merging_after(&world, &failed, 10),
        vec![merge(20, false)]
    );
}

#[test]
fn with_auto_merge_a_green_pull_request_whose_merge_failed_twice_is_handed_back_with_githubs_reason()
 {
    let world = in_review(ready(pull(20, 2)));
    let failed = [
        failed_merge(20, "Head branch was modified."),
        failed_merge(20, "Pull Request is not mergeable"),
    ];

    assert_eq!(
        auto_merging_after(&world, &failed, 11),
        vec![Action::HandBack {
            item: Item::Pull(Number(20)),
            reason: "Merging #20 failed twice. GitHub's reason: Pull Request is not mergeable"
                .into(),
        }]
    );
}

#[test]
fn with_auto_merge_failed_merges_of_another_pull_request_do_not_count() {
    let world = in_review(ready(pull(20, 2)));
    let failed = [
        failed_merge(21, "Pull Request is not mergeable"),
        failed_merge(21, "Pull Request is not mergeable"),
    ];

    assert_eq!(
        auto_merging_after(&world, &failed, 11),
        vec![merge(20, false)]
    );
}

/// `pr` with Linewright unassigned by `actor` at `unassigned`, and assigned
/// again at `reassigned` if it was.
fn unassigned_at(mut pr: Pull, actor: &str, unassigned: i64, reassigned: Option<i64>) -> Pull {
    let linewright = Login::new(LINEWRIGHT);
    pr.assignees.retain(|a| a != &linewright);
    pr.assignments.push(Assignment::Unassigned {
        assignee: linewright.clone(),
        actor: Login::new(actor),
        at: t(unassigned),
    });
    if let Some(reassigned) = reassigned {
        pr.assignees.push(linewright.clone());
        pr.assignments.push(Assignment::Assigned {
            assignee: linewright,
            at: t(reassigned),
        });
    }
    pr
}

fn labeled_pull(mut pr: Pull, label: &str) -> Pull {
    pr.labels.push(label.into());
    pr
}

#[test]
fn with_auto_merge_reassigning_linewright_to_a_pull_request_handed_back_for_failed_merges_tries_the_merge_again()
 {
    let world = in_review(unassigned_at(ready(pull(20, 2)), LINEWRIGHT, 12, Some(15)));
    let failed = [
        failed_merge_at(20, "Pull Request is not mergeable", 8),
        failed_merge_at(20, "Pull Request is not mergeable", 11),
        failed_merge_at(20, "Head branch was modified.", 17),
    ];

    assert_eq!(
        auto_merging_after(&world, &failed, 20),
        vec![merge(20, false)]
    );
}

#[test]
fn a_pull_request_closed_without_merging_hands_the_ticket_back_once() {
    let mut pr = pull(20, 2);
    pr.state = PullState::Closed { at: t(5) };
    let closed = in_review(pr);
    let mut relabeled = closed.clone();
    relabeled.issues[0] = labeled_at(relabeled.issues[0].clone(), label::READY_FOR_AGENT, 6);

    assert!(matches!(
        at(&closed, 10).as_slice(),
        [Action::HandBack {
            item: Item::Issue(Number(2)),
            ..
        }]
    ));
    assert_eq!(at(&relabeled, 10), vec![implement(2)]);
}

#[test]
fn a_ticket_whose_pull_request_merged_waits_for_github_to_close_it_until_labeled_again() {
    let mut pr = pull(20, 2);
    pr.state = PullState::Merged { at: t(5) };
    let merged = in_review(pr);
    let mut unclaimed = merged.clone();
    unclaimed.issues[0].assignees.clear();
    let mut labeled_as_it_merged = merged.clone();
    labeled_as_it_merged.issues[0] = labeled_at(
        labeled_as_it_merged.issues[0].clone(),
        label::READY_FOR_AGENT,
        5,
    );
    let mut relabeled = merged.clone();
    relabeled.issues[0] = labeled_at(relabeled.issues[0].clone(), label::READY_FOR_AGENT, 7);

    assert_eq!(at(&merged, 10), vec![]);
    assert_eq!(at(&unclaimed, 10), vec![]);
    assert_eq!(at(&labeled_as_it_merged, 10), vec![]);
    assert_eq!(at(&relabeled, 10), vec![implement(2)]);
}

fn tracking(number: u64, pr: u64, tickets: &[(u64, bool)]) -> Issue {
    let mut tracking = issue(number);
    tracking.author = Login::new(LINEWRIGHT);
    tracking.title = format!("Spec change: Spec PR {pr}");
    tracking.filed = Some(Filed::Tracking {
        spec_change: Number(pr),
    });
    tracking.sub_issues = tickets
        .iter()
        .map(|&(n, open)| Linked {
            number: Number(n),
            open,
        })
        .collect();
    tracking
}

#[test]
fn a_tracking_issue_is_closed_when_all_its_tickets_are() {
    let world = snapshot(vec![
        tracking(9, 5, &[(10, false), (11, false)]),
        tracking(12, 6, &[(13, false), (14, true)]),
    ]);

    assert_eq!(
        decide(&world),
        vec![Action::CloseTracking { issue: Number(9) }]
    );
}

fn hourly(name: &str) -> Workflow {
    linewright::workflow::parse(
        &name.parse().unwrap(),
        "---\non:\n  schedule: \"0 * * * *\"\n---\nLook.\n",
    )
    .unwrap()
}

fn last_runs(runs: &[(&str, i64)]) -> LastRuns {
    runs.iter()
        .map(|&(name, minute)| (name.parse().unwrap(), t(minute)))
        .collect()
}

fn workflows(defined: &[&str], last: &[(&str, i64)], in_flight: &[&str]) -> Workflows {
    Workflows {
        defined: defined.iter().map(|name| hourly(name)).collect(),
        last_runs: last_runs(last),
        in_flight: in_flight.iter().map(|name| name.parse().unwrap()).collect(),
    }
}

/// The decisions at `minute` past 12:00, when ticks fall on the hour.
fn scheduled(workflows: &Workflows, minute: i64) -> Vec<Action> {
    reconcile(
        &snapshot(vec![]),
        &[],
        &Memory::default(),
        workflows,
        t(minute),
        &policy(),
    )
}

#[test]
fn a_workflow_whose_schedule_ticked_since_its_last_run_runs_and_the_run_is_recorded() {
    let world = workflows(&["hourly"], &[("hourly", 30)], &[]);

    assert_eq!(
        scheduled(&world, 70),
        vec![
            Action::RunWorkflow(Box::new(hourly("hourly"))),
            Action::RecordLastRuns(last_runs(&[("hourly", 70)])),
        ]
    );
}

#[test]
fn a_workflow_whose_schedule_has_not_ticked_since_its_last_run_waits() {
    let world = workflows(&["hourly"], &[("hourly", 30)], &[]);

    assert_eq!(scheduled(&world, 59), vec![]);
}

#[test]
fn a_workflow_seen_for_the_first_time_does_not_run_and_counts_as_run_then() {
    let world = workflows(&["hourly"], &[], &[]);

    assert_eq!(
        scheduled(&world, 70),
        vec![Action::RecordLastRuns(last_runs(&[("hourly", 70)]))]
    );
}

#[test]
fn ticks_missed_since_the_last_run_collapse_into_one_run() {
    // Five ticks, 13:00 to 17:00, since the last run at 12:30.
    let world = workflows(&["hourly"], &[("hourly", 30)], &[]);

    assert_eq!(
        scheduled(&world, 310),
        vec![
            Action::RunWorkflow(Box::new(hourly("hourly"))),
            Action::RecordLastRuns(last_runs(&[("hourly", 310)])),
        ]
    );
}

#[test]
fn ticks_that_pass_while_a_run_is_in_flight_make_one_more_run_once_it_ends() {
    // The run in flight started at 12:30; 13:00 and 14:00 tick meanwhile.
    let running = workflows(&["hourly"], &[("hourly", 30)], &["hourly"]);
    let ended = workflows(&["hourly"], &[("hourly", 30)], &[]);

    assert_eq!(scheduled(&running, 130), vec![]);
    assert_eq!(
        scheduled(&ended, 131),
        vec![
            Action::RunWorkflow(Box::new(hourly("hourly"))),
            Action::RecordLastRuns(last_runs(&[("hourly", 131)])),
        ]
    );
}

#[test]
fn workflow_runs_count_against_the_ceiling_and_get_the_room_the_pipeline_leaves() {
    // A ceiling of 4: two pipeline runs and one workflow run in flight, a
    // ticket to implement, and two workflows due.
    let world = snapshot(vec![
        claimed(ticket(1)),
        claimed(ticket(8)),
        claimed(ticket(9)),
    ]);
    let busy = [live(8, WorkKind::Implement), live(9, WorkKind::Implement)];
    let due = workflows(&["a", "b", "c"], &[("a", 30), ("b", 30), ("c", 30)], &["a"]);
    let quieter = snapshot(vec![claimed(ticket(8)), claimed(ticket(9))]);

    assert_eq!(
        reconcile(&world, &busy, &Memory::default(), &due, t(70), &policy()),
        vec![implement(1)]
    );
    assert_eq!(
        reconcile(&quieter, &busy, &Memory::default(), &due, t(70), &policy()),
        vec![
            Action::RunWorkflow(Box::new(hourly("b"))),
            Action::RecordLastRuns(last_runs(&[("a", 30), ("b", 70), ("c", 30)])),
        ]
    );
}

#[test]
fn a_workflow_seen_for_the_first_time_is_recorded_even_with_no_room() {
    let tickets = snapshot([1, 2, 3, 4].map(|n| claimed(ticket(n))).to_vec());
    let busy = [1, 2, 3, 4].map(|n| live(n, WorkKind::Implement));
    let world = workflows(&["hourly"], &[], &[]);

    assert_eq!(
        reconcile(
            &tickets,
            &busy,
            &Memory::default(),
            &world,
            t(70),
            &policy()
        ),
        vec![Action::RecordLastRuns(last_runs(&[("hourly", 70)]))]
    );
}

const REQUIRED: &str = "linewright";

fn requiring_label() -> Policy {
    Policy {
        require_label: Some(REQUIRED.into()),
        ..policy()
    }
}

fn deciding_with_label(world: &Snapshot) -> Vec<Action> {
    deciding(world, &Memory::default(), &requiring_label())
}

#[test]
fn with_a_required_label_only_a_labeled_needs_triage_issue_is_triaged() {
    let unlabeled = labeled(issue(1), label::NEEDS_TRIAGE);
    let with_label = labeled(labeled(issue(2), label::NEEDS_TRIAGE), REQUIRED);

    assert_eq!(
        deciding_with_label(&snapshot(vec![unlabeled, with_label])),
        vec![claim(2), triage(2)]
    );
}

#[test]
fn with_a_required_label_only_a_labeled_issue_answered_after_a_triage_question_is_triaged_again() {
    let mut unlabeled = labeled(issue(1), label::NEEDS_INFO);
    unlabeled.comments = vec![triage_comment(1), comment("reporter", 2, "Version 3.")];
    let mut with_label = labeled(unlabeled.clone(), REQUIRED);
    with_label.number = Number(2);

    assert_eq!(
        deciding_with_label(&snapshot(vec![unlabeled, with_label])),
        vec![
            Action::Relabel {
                issue: Number(2),
                remove: label::NEEDS_INFO.into(),
                add: label::NEEDS_TRIAGE.into(),
            },
            claim(2),
            triage(2),
        ]
    );
}

#[test]
fn with_a_required_label_only_a_labeled_unclaimed_ticket_is_claimed() {
    let unlabeled = ticket(1);
    let with_label = labeled(ticket(2), REQUIRED);

    assert_eq!(
        deciding_with_label(&snapshot(vec![unlabeled, with_label])),
        vec![claim(2), implement(2)]
    );
}

#[test]
fn with_a_required_label_a_ticket_is_claimed_again_after_an_unassignment_only_if_labeled() {
    let restart = |number, required: bool| {
        let mut ticket = labeled_at(
            unassigned_by(
                labeled_at(ticket(number), label::READY_FOR_AGENT, 1),
                "maintainer",
                2,
            ),
            label::READY_FOR_AGENT,
            3,
        );
        if required {
            ticket.labels.push(REQUIRED.into());
        }
        ticket
    };

    assert_eq!(
        deciding_with_label(&snapshot(vec![restart(1, false), restart(2, true)])),
        vec![claim(2), implement(2)]
    );
}

#[test]
fn with_a_required_label_a_reopened_ticket_is_implemented_again_only_if_labeled() {
    let reopened = |required: bool| {
        let mut pr = pull(20, 2);
        pr.state = PullState::Merged { at: t(5) };
        let mut world = in_review(pr);
        world.issues[0] = labeled_at(world.issues[0].clone(), label::READY_FOR_AGENT, 7);
        if required {
            world.issues[0].labels.push(REQUIRED.into());
        }
        world
    };

    assert_eq!(deciding_with_label(&reopened(false)), vec![]);
    assert_eq!(deciding_with_label(&reopened(true)), vec![implement(2)]);
}

#[test]
fn with_a_required_label_work_already_started_carries_on_unlabeled() {
    let mut failing = pull(20, 2);
    failing.checks = vec![check("test", CheckState::Failed)];
    let mut behind = pull(21, 3);
    behind.mergeability = Mergeability::Behind;
    let mut green = pull(22, 4);
    green.checks = vec![check("test", CheckState::Passed)];
    green.state = PullState::Open { draft: false };
    let mut world = in_review(failing);
    for pr in [behind, green] {
        world.issues.push(in_review(pr.clone()).issues.remove(0));
        world.pulls.push(pr);
    }
    world.issues.push(claimed(ticket(5)));
    let policy = Policy {
        auto_merge: Some(MergeMethod::Merge),
        ..requiring_label()
    };

    assert_eq!(
        deciding(&world, &Memory::default(), &policy),
        vec![
            merge_by(22, false, MergeMethod::Merge),
            Action::Start(Work::FixCi {
                issue: Number(2),
                pr: Number(20),
                failing: vec!["test".into()]
            }),
            Action::Start(Work::Update {
                issue: Number(3),
                pr: Number(21)
            }),
            implement(5),
        ]
    );
}

#[test]
fn removing_the_required_label_cancels_no_run_in_flight() {
    let world = snapshot(vec![claimed(ticket(1))]);
    let runs = [live(1, WorkKind::Implement)];

    let actions = reconcile(
        &world,
        &runs,
        &Memory::default(),
        &Workflows::default(),
        t(10),
        &requiring_label(),
    );

    assert_eq!(actions, vec![]);
}

#[test]
fn with_a_required_label_only_a_labeled_spec_change_is_cut_into_tickets() {
    let unlabeled = merged(1, &["docs/spec.md"]);
    let mut with_label = merged(2, &["docs/spec.md"]);
    with_label.labels.push(REQUIRED.into());
    let mut world = snapshot(vec![]);
    world.merged = vec![unlabeled, with_label];

    assert_eq!(deciding_with_label(&world), cut(2));
}

#[test]
fn with_a_required_label_a_spec_change_labeled_after_it_merged_is_picked_up_on_a_later_poll() {
    let mut pr = merged(1, &["docs/spec.md"]);
    let mut world = snapshot(vec![]);
    world.merged = vec![pr.clone()];
    assert_eq!(deciding_with_label(&world), vec![]);

    pr.labels.push(REQUIRED.into());
    world.merged = vec![pr];
    assert_eq!(deciding_with_label(&world), cut(1));
}

#[test]
fn with_a_required_label_a_handed_back_spec_change_is_cut_again_once_the_label_is_applied_again() {
    let handed_back = |number| {
        unassigned(
            assigned(
                labeled_merged(merged(number, &["docs/spec.md"]), REQUIRED, 0),
                1,
            ),
            LINEWRIGHT,
            2,
        )
    };
    let relabeled = labeled_merged(handed_back(2), REQUIRED, 3);
    let mut world = snapshot(vec![]);
    world.merged = vec![handed_back(1), relabeled];

    assert_eq!(deciding_with_label(&world), cut(2));
}

#[test]
fn with_a_required_label_a_spec_change_assigned_to_linewright_by_hand_is_cut_unlabeled() {
    let mut world = snapshot(vec![]);
    world.merged = vec![assigned(merged(1, &["docs/spec.md"]), 1)];

    assert_eq!(
        deciding_with_label(&world),
        vec![Action::Start(Work::Tickets { pr: Number(1) })]
    );
}

#[test]
fn with_a_required_label_processed_and_linewrights_spec_changes_are_skipped_even_if_labeled() {
    let mut processed = merged(1, &["docs/spec.md"]);
    processed.labels = vec![REQUIRED.into(), label::SPEC_PROCESSED.into()];
    let mut by_linewright = merged(2, &["docs/spec.md"]);
    by_linewright.labels = vec![REQUIRED.into()];
    by_linewright.author = Login::new(LINEWRIGHT);
    let mut world = snapshot(vec![]);
    world.merged = vec![processed, by_linewright];

    assert_eq!(deciding_with_label(&world), vec![]);
}

#[test]
fn removing_the_required_label_does_not_cancel_a_cut_under_way() {
    let mut world = snapshot(vec![]);
    world.merged = vec![assigned(merged(5, &["docs/spec.md"]), 1)];

    assert_eq!(
        reconcile(
            &world,
            &cutting(5),
            &Memory::default(),
            &Workflows::default(),
            t(10),
            &requiring_label()
        ),
        vec![]
    );
}

#[test]
fn pending_spec_changes_agree_with_the_required_label() {
    let unlabeled = merged(1, &["docs/spec.md"]);
    let mut with_label = merged(2, &["docs/spec.md"]);
    with_label.labels.push(REQUIRED.into());
    let mut world = snapshot(vec![]);
    world.merged = vec![unlabeled, with_label];

    let numbers = |policy: &Policy| {
        linewright::reconcile::pending_spec_changes(&world, t(10), policy)
            .iter()
            .map(|pr| pr.number)
            .collect::<Vec<_>>()
    };
    assert_eq!(numbers(&requiring_label()), vec![Number(2)]);
    assert_eq!(numbers(&policy()), vec![Number(1), Number(2)]);
}

fn work_of_every_kind() -> (Snapshot, Vec<LiveRun>, Workflows) {
    let mut answered = labeled(issue(1), label::NEEDS_INFO);
    answered.comments = vec![triage_comment(1), comment("reporter", 2, "Version 3.")];
    let mut reworked = ready(pull(20, 2));
    reworked.comments = vec![comment("maintainer", 4, "Please rename `foo`.")];
    let mut green = pull(21, 6);
    green.checks = vec![check("test", CheckState::Passed)];
    let mut failing = pull(22, 11);
    failing.checks = vec![check("test", CheckState::Failed)];
    let mut world = snapshot(vec![
        answered,
        claimed(ticket(2)),
        ticket(3),
        claimed(ticket(4)),
        claimed(ticket(6)),
        tracking(9, 7, &[(10, false)]),
        claimed(ticket(11)),
        claimed(issue(12)),
    ]);
    world.pulls = vec![reworked, green, failing];
    world.merged = vec![merged(5, &["docs/spec.md"])];
    (
        world,
        vec![live(12, WorkKind::Implement)],
        workflows(&["hourly"], &[("hourly", 30)], &[]),
    )
}

fn fixed_twice() -> Vec<DoneRun> {
    vec![done(22, WorkKind::FixCi, 3), done(22, WorkKind::FixCi, 6)]
}

fn under_usage_limit_until(resets_at: i64) -> Memory {
    Memory {
        usage_limit: Some(UsageLimit {
            resets_at: t(resets_at),
        }),
        done_runs: fixed_twice(),
        ..Memory::default()
    }
}

/// The decisions at 13:10 with room for every run, under a usage limit
/// that resets at `resets_at` minutes past 12:00.
fn under_usage_limit(resets_at: i64) -> Vec<Action> {
    let (world, live, workflows) = work_of_every_kind();
    let policy = Policy {
        max_runs: 10,
        ..policy()
    };
    let memory = under_usage_limit_until(resets_at);
    reconcile(&world, &live, &memory, &workflows, t(70), &policy)
}

fn sessionless_actions() -> Vec<Action> {
    vec![
        Action::Cancel {
            subject: Subject::Issue(Number(12)),
            reason: "`ready-for-agent` was removed from #12".into(),
        },
        Action::MarkReady { pr: Number(21) },
        Action::Relabel {
            issue: Number(1),
            remove: label::NEEDS_INFO.into(),
            add: label::NEEDS_TRIAGE.into(),
        },
        Action::CloseTracking { issue: Number(9) },
    ]
}

#[test]
fn under_the_usage_limit_no_run_starts_and_no_issue_is_claimed_but_other_actions_are_taken() {
    let actions = under_usage_limit(80);

    let (handed_back, others): (Vec<Action>, Vec<Action>) = actions.into_iter().partition(|a| {
        matches!(
            a,
            Action::HandBack {
                item: Item::Pull(_),
                ..
            }
        )
    });
    assert_eq!(others, sessionless_actions());
    assert!(matches!(
        handed_back.as_slice(),
        [Action::HandBack {
            item: Item::Pull(Number(22)),
            ..
        }]
    ));
}

#[test]
fn once_the_usage_limit_resets_every_run_starts_and_the_due_workflow_runs() {
    let starts: Vec<Action> = under_usage_limit(70)
        .into_iter()
        .filter(|a| {
            !sessionless_actions().contains(a)
                && !matches!(
                    a,
                    Action::HandBack {
                        item: Item::Pull(_),
                        ..
                    }
                )
        })
        .collect();

    assert_eq!(
        starts,
        vec![
            rework(2, 20),
            claim(1),
            triage(1),
            Action::Claim {
                item: Item::Pull(Number(5))
            },
            Action::Start(Work::Tickets { pr: Number(5) }),
            claim(3),
            implement(3),
            implement(4),
            Action::RunWorkflow(Box::new(hourly("hourly"))),
            Action::RecordLastRuns(last_runs(&[("hourly", 70)])),
        ]
    );
}

#[test]
fn with_auto_merge_a_green_pull_request_is_merged_under_the_usage_limit() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("test", CheckState::Passed)];

    assert_eq!(
        reconcile(
            &in_review(pr),
            &[],
            &under_usage_limit_until(80),
            &Workflows::default(),
            t(10),
            &merging_by(MergeMethod::Merge),
        ),
        vec![merge(20, true)]
    );
}

#[test]
fn the_latest_usage_limit_reset_holds_starts_back() {
    let mut memory = Memory::default();

    assert!(memory.hold_until(t(80)));
    assert!(!memory.hold_until(t(50)));
    assert!(memory.holds_starts(t(70)));
    assert!(memory.hold_until(t(90)));
    assert!(memory.holds_starts(t(85)));
}

#[test]
fn the_usage_limit_is_lifted_once_its_reset_passes() {
    let mut memory = under_usage_limit_until(80);

    assert_eq!(memory.lift_usage_limit(t(79)), None);
    assert!(memory.holds_starts(t(79)));
    assert_eq!(memory.lift_usage_limit(t(80)), Some(t(80)));
    assert!(!memory.holds_starts(t(79)));
    assert_eq!(memory.lift_usage_limit(t(81)), None);
}

fn waits(
    world: &Snapshot,
    live: &[LiveRun],
    memory: &Memory,
    minute: i64,
    policy: &Policy,
) -> std::collections::BTreeMap<Number, Vec<Wait>> {
    decisions(
        world,
        live,
        memory,
        &Workflows::default(),
        t(minute),
        policy,
    )
    .waits
}

fn waits_on(world: &Snapshot, number: u64) -> Vec<Wait> {
    waits(world, &[], &Memory::default(), 10, &policy())
        .remove(&Number(number))
        .unwrap_or_default()
}

#[test]
fn a_blocked_ticket_waits_for_each_open_blocker() {
    let world = snapshot(vec![blocked_by(
        blocked_by(blocked_by(ticket(4), 1, true), 2, false),
        3,
        true,
    )]);

    assert_eq!(
        waits_on(&world, 4),
        [
            Wait::Blocker { issue: Number(1) },
            Wait::Blocker { issue: Number(3) },
        ]
    );
}

fn closed_by(mut issue: Issue, pr: u64, author: &str) -> Issue {
    issue.closed_by.push(ClosingPull {
        number: Number(pr),
        author: Login::new(author),
        state: PullState::Open { draft: false },
    });
    issue
}

#[test]
fn a_ticket_someone_elses_open_pull_request_closes_is_not_claimed_and_waits_for_it() {
    let world = snapshot(vec![closed_by(ticket(2), 43, "alice")]);

    assert_eq!(decide(&world), vec![]);
    assert_eq!(
        waits_on(&world, 2),
        [Wait::ClosingPullRequest {
            pull_request: Number(43)
        }]
    );
}

#[test]
fn a_ticket_only_pull_requests_closed_before_its_claim_close_is_claimed() {
    let mut ticket = ticket(2);
    for number in [43, 44] {
        ticket.closed_by.push(ClosingPull {
            number: Number(number),
            author: Login::new("alice"),
            state: PullState::Closed { at: t(5) },
        });
    }
    let world = snapshot(vec![ticket]);

    assert_eq!(decide(&world), vec![claim(2), implement(2)]);
}

#[test]
fn a_claimed_ticket_someone_elses_open_pull_request_closes_starts_no_run() {
    let world = snapshot(vec![closed_by(claimed(ticket(2)), 43, "renovate[bot]")]);

    assert_eq!(decide(&world), vec![]);
    assert_eq!(
        waits_on(&world, 2),
        [Wait::ClosingPullRequest {
            pull_request: Number(43)
        }]
    );
}

#[test]
fn a_ticket_waits_for_blockers_and_closing_pull_requests_alike() {
    let world = snapshot(vec![closed_by(blocked_by(ticket(4), 1, true), 43, "alice")]);

    assert_eq!(
        waits_on(&world, 4),
        [
            Wait::ClosingPullRequest {
                pull_request: Number(43)
            },
            Wait::Blocker { issue: Number(1) },
        ]
    );
}

fn pull_closing_42_and_12(label: &str) -> Snapshot {
    let mut pr = pull(43, 42);
    pr.closes.push(Number(12));
    pr.checks = vec![check("test", CheckState::Failed)];
    let mut first = closed_by(assigned_at(issue(42), 0), 43, LINEWRIGHT);
    first.labels = vec![label.into()];
    let second = closed_by(assigned_at(ticket(12), 0), 43, LINEWRIGHT);
    let mut world = snapshot(vec![first, second]);
    world.pulls = vec![pr];
    world
}

fn assigned_at(mut issue: Issue, minute: i64) -> Issue {
    issue.assignees.push(Login::new(LINEWRIGHT));
    issue
        .events
        .push(IssueEvent::Assignment(Assignment::Assigned {
            assignee: Login::new(LINEWRIGHT),
            at: t(minute),
        }));
    issue
}

fn closed_unmerged(mut world: Snapshot, number: u64, minute: i64) -> Snapshot {
    let state = PullState::Closed { at: t(minute) };
    for pull in world
        .pulls
        .iter_mut()
        .filter(|p| p.number == Number(number))
    {
        pull.state = state;
    }
    for closing in world
        .issues
        .iter_mut()
        .flat_map(|i| &mut i.closed_by)
        .filter(|p| p.number == Number(number))
    {
        closing.state = state;
    }
    world
}

fn decided(world: &Snapshot) -> Decisions {
    decisions(
        world,
        &[],
        &Memory::default(),
        &Workflows::default(),
        t(10),
        &policy(),
    )
}

fn fix_ci(issue: u64, pr: u64) -> Action {
    Action::Start(Work::FixCi {
        issue: Number(issue),
        pr: Number(pr),
        failing: vec!["test".into()],
    })
}

fn handed_back_issues(actions: &[Action]) -> Vec<u64> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::HandBack {
                item: Item::Issue(number),
                ..
            } => Some(number.0),
            _ => None,
        })
        .collect()
}

#[test]
fn a_pull_request_closing_two_issues_is_worked_while_both_wait_for_it_whatever_their_labels() {
    for label in [label::READY_FOR_AGENT, label::READY_FOR_HUMAN] {
        let world = pull_closing_42_and_12(label);

        let decided = decided(&world);

        assert_eq!(decided.actions, vec![fix_ci(42, 43)]);
        for issue in [12, 42] {
            assert_eq!(
                decided.waits[&Number(issue)],
                [Wait::PullRequest {
                    pull_request: Number(43)
                }]
            );
        }
    }
}

#[test]
fn a_pull_request_closed_unmerged_hands_back_each_claimed_issue_it_closes_once() {
    let world = closed_unmerged(pull_closing_42_and_12(label::READY_FOR_AGENT), 43, 5);
    let mut relabeled = world.clone();
    for issue in &mut relabeled.issues {
        *issue = labeled_at(issue.clone(), label::READY_FOR_AGENT, 6);
    }

    assert_eq!(handed_back_issues(&decided(&world).actions), [42, 12]);
    assert_eq!(
        handed_back_issues(&decided(&relabeled).actions),
        Vec::<u64>::new()
    );
}

#[test]
fn a_ticket_in_review_with_linewrights_pull_request_waits_for_it_alone_whatever_blocks_it() {
    let mut world = in_review(pull(43, 42));
    world.issues[0] = blocked_by(world.issues[0].clone(), 1, true);

    assert_eq!(
        waits_on(&world, 42),
        [Wait::PullRequest {
            pull_request: Number(43)
        }]
    );
}

#[test]
fn a_ticket_claimed_after_the_pull_request_closing_it_was_closed_is_implemented() {
    let world = snapshot(vec![assigned_at(closed_by(ticket(2), 20, LINEWRIGHT), 7)]);
    let world = closed_unmerged(world, 20, 5);

    assert_eq!(decide(&world), vec![implement(2)]);
}

#[test]
fn someone_elses_pull_request_closed_unmerged_hands_back_a_ticket_claimed_before() {
    let world = snapshot(vec![assigned_at(closed_by(ticket(2), 20, "alice"), 0)]);
    let world = closed_unmerged(world, 20, 5);

    assert_eq!(handed_back_issues(&decide(&world)), [2]);
}

#[test]
fn a_ticket_whose_pull_request_no_longer_closes_it_is_worked_again() {
    let mut world = in_review(pull(43, 42));
    world.pulls[0].closes = vec![];

    assert_eq!(decide(&world), vec![implement(42)]);
}

#[test]
fn a_ticket_someone_elses_merged_pull_request_closes_waits_to_be_labeled_again() {
    let mut ticket = claimed(ticket(2));
    ticket.closed_by.push(ClosingPull {
        number: Number(20),
        author: Login::new("alice"),
        state: PullState::Merged { at: t(5) },
    });
    let world = snapshot(vec![ticket]);

    assert_eq!(decide(&world), vec![]);
    assert_eq!(
        waits_on(&world, 2),
        [Wait::Relabel {
            label: StatusLabel::ReadyForAgent,
            since: Relabel::Merged {
                pull_request: Number(20)
            }
        }]
    );
}

#[test]
fn work_beyond_the_ceiling_waits_for_a_free_run_with_the_runs_in_flight_counted() {
    let world = snapshot(vec![
        claimed(ticket(1)),
        claimed(ticket(2)),
        claimed(ticket(3)),
        claimed(ticket(9)),
    ]);
    let busy = [live(9, WorkKind::Implement), live(8, WorkKind::Rework)];

    let waits = waits(&world, &busy, &Memory::default(), 10, &policy());

    assert_eq!(
        waits.get(&Number(3)).map(Vec::as_slice),
        Some(
            &[Wait::FreeRun {
                in_flight: 4,
                max_runs: 4
            }][..]
        )
    );
    assert!(!waits.contains_key(&Number(1)), "{waits:?}");
    assert!(!waits.contains_key(&Number(2)), "{waits:?}");
}

#[test]
fn under_the_usage_limit_work_waits_for_its_reset() {
    let world = snapshot(vec![claimed(ticket(1))]);
    let mut memory = Memory::default();
    memory.hold_until(t(150));

    assert_eq!(
        waits(&world, &[], &memory, 10, &policy()).remove(&Number(1)),
        Some(vec![Wait::UsageLimit { resets_at: t(150) }])
    );
}

#[test]
fn a_spec_change_beyond_the_ceiling_waits_for_a_free_run_too() {
    let mut world = snapshot(vec![]);
    world.merged = vec![merged(5, &["docs/spec.md"])];
    let busy = [1, 2, 3, 4].map(|n| live(n, WorkKind::Implement));

    assert_eq!(
        waits(&world, &busy, &Memory::default(), 10, &policy()).remove(&Number(5)),
        Some(vec![Wait::FreeRun {
            in_flight: 4,
            max_runs: 4
        }])
    );
}

#[test]
fn an_item_with_a_run_in_flight_waits_for_it() {
    let mut world = snapshot(vec![
        claimed(ticket(2)),
        labeled(issue(3), label::NEEDS_TRIAGE),
    ]);
    world.merged = vec![merged(5, &["docs/spec.md"])];
    let runs = [
        live(2, WorkKind::Implement),
        live(3, WorkKind::Triage),
        LiveRun {
            subject: Subject::SpecChange(Number(5)),
            kind: WorkKind::Tickets,
        },
    ];

    let waits = waits(&world, &runs, &Memory::default(), 10, &policy());

    assert_eq!(
        waits,
        [
            (2, WorkKind::Implement),
            (3, WorkKind::Triage),
            (5, WorkKind::Tickets)
        ]
        .map(|(n, stage)| (Number(n), vec![Wait::Run { stage }]))
        .into()
    );
}

#[test]
fn a_pull_request_waits_for_ci_while_its_checks_run_or_before_any_report() {
    let mut running = pull(20, 2);
    running.checks = vec![check("test", CheckState::Pending)];
    let silent = pull(20, 2);

    for (world, minute) in [(in_review(running), 10), (in_review(silent), 5)] {
        assert_eq!(
            waits(&world, &[], &Memory::default(), minute, &policy()).remove(&Number(20)),
            Some(vec![Wait::Ci {
                pull_request: Number(20)
            }])
        );
    }
}

#[test]
fn an_issue_labeled_for_the_maintainer_or_the_reporter_waits_for_them() {
    let world = snapshot(vec![
        labeled(issue(1), label::READY_FOR_HUMAN),
        labeled(issue(2), label::NEEDS_INFO),
    ]);

    assert_eq!(waits_on(&world, 1), [Wait::Maintainer]);
    assert_eq!(waits_on(&world, 2), [Wait::Reporter]);
}

#[test]
fn a_pull_request_ready_for_review_waits_for_the_maintainers_review() {
    let world = in_review(ready(pull(20, 2)));

    assert_eq!(
        waits_on(&world, 20),
        [Wait::Review {
            pull_request: Number(20)
        }]
    );
}

#[test]
fn with_auto_merge_a_pull_request_a_person_holds_waits_for_their_review() {
    let mut pr = ready(pull(20, 2));
    pr.comments = vec![comment("maintainer", 5, "Hmm.")];
    pr.pushes[0].at = t(6);

    assert_eq!(
        waits(
            &in_review(pr),
            &[],
            &Memory::default(),
            10,
            &merging_by(MergeMethod::Merge)
        )
        .remove(&Number(20)),
        Some(vec![Wait::Review {
            pull_request: Number(20)
        }])
    );
}

#[test]
fn with_auto_merge_a_pull_request_held_for_what_github_requires_waits_for_it() {
    let mut pr = ready(pull(20, 2));
    pr.mergeability = Mergeability::Blocked {
        requires: code_owner_review(),
    };

    assert_eq!(
        waits(
            &in_review(pr),
            &[],
            &held_for_a_code_owner(),
            10,
            &merging_by(MergeMethod::Merge)
        )
        .remove(&Number(20)),
        Some(vec![Wait::Requirements {
            pull_request: Number(20),
            requires: code_owner_review(),
        }])
    );
}

#[test]
fn with_auto_merge_a_pull_request_whose_merge_state_is_not_known_waits_for_github() {
    let mut pr = ready(pull(20, 2));
    pr.mergeability = Mergeability::Unknown;

    assert_eq!(
        waits(
            &in_review(pr),
            &[],
            &Memory::default(),
            10,
            &merging_by(MergeMethod::Merge)
        )
        .remove(&Number(20)),
        Some(vec![Wait::MergeState {
            pull_request: Number(20)
        }])
    );
}

#[test]
fn a_tracking_issue_waits_for_its_open_tickets() {
    let tracking = tracking(10, 5, &[(11, false), (12, true), (13, true)]);

    assert_eq!(
        waits_on(&snapshot(vec![tracking]), 10),
        [Wait::Tickets {
            open: vec![Number(12), Number(13)]
        }]
    );
}

#[test]
fn an_issue_without_the_required_label_waits_for_the_maintainer_to_apply_it() {
    let required = Policy {
        require_label: Some("linewright".into()),
        ..policy()
    };
    let world = snapshot(vec![
        labeled(issue(1), label::NEEDS_TRIAGE),
        labeled(issue(2), label::READY_FOR_AGENT),
        labeled(issue(3), label::NEEDS_INFO),
        labeled(issue(4), label::READY_FOR_HUMAN),
    ]);
    let required_label = Wait::RequiredLabel {
        label: "linewright".into(),
    };

    let waits = waits(&world, &[], &Memory::default(), 10, &required);

    assert_eq!(
        waits,
        [
            (1, vec![required_label.clone()]),
            (2, vec![required_label.clone()]),
            (3, vec![Wait::Reporter, required_label.clone()]),
            (4, vec![Wait::Maintainer, required_label]),
        ]
        .map(|(n, on)| (Number(n), on))
        .into()
    );
}

#[test]
fn an_issue_linewright_filed_labeled_needs_triage_is_triaged() {
    let mut filed = labeled(issue(1), label::NEEDS_TRIAGE);
    filed.author = Login::new(LINEWRIGHT);

    assert_eq!(decide(&snapshot(vec![filed])), vec![claim(1), triage(1)]);
}

#[test]
fn a_ticket_linewright_was_unassigned_from_waits_for_ready_for_agent_again() {
    let stopped = unassigned_by(
        labeled_at(ticket(2), label::READY_FOR_AGENT, 1),
        "maintainer",
        2,
    );

    assert_eq!(
        waits_on(&snapshot(vec![stopped]), 2),
        [Wait::Relabel {
            label: StatusLabel::ReadyForAgent,
            since: Relabel::Unassigned {
                required_label: None
            }
        }]
    );
}

#[test]
fn a_ticket_whose_pull_request_merged_waits_for_ready_for_agent_again() {
    let mut pr = ready(pull(20, 2));
    pr.state = PullState::Merged { at: t(5) };

    assert_eq!(
        waits_on(&in_review(pr), 2),
        [Wait::Relabel {
            label: StatusLabel::ReadyForAgent,
            since: Relabel::Merged {
                pull_request: Number(20)
            }
        }]
    );
}

#[test]
fn a_tracking_issue_whose_spec_change_is_being_cut_waits_for_the_cut() {
    let mut world = snapshot(vec![tracking(10, 5, &[(11, true)])]);
    world.merged = vec![merged(5, &["docs/spec.md"])];
    let cut = [LiveRun {
        subject: Subject::SpecChange(Number(5)),
        kind: WorkKind::Tickets,
    }];

    assert_eq!(
        waits(&world, &cut, &Memory::default(), 10, &policy()).remove(&Number(10)),
        Some(vec![Wait::Run {
            stage: WorkKind::Tickets
        }])
    );
}

#[test]
fn a_tracking_issue_of_a_cut_that_stopped_part_way_is_discarded_and_waits_for_nothing() {
    let mut world = snapshot(vec![tracking(10, 5, &[(11, true)])]);
    world.merged = vec![merged(5, &["docs/spec.md"])];

    let decided = decisions(
        &world,
        &[],
        &Memory::default(),
        &Workflows::default(),
        t(10),
        &policy(),
    );

    assert_eq!(
        decided.actions,
        [Action::DiscardTickets {
            tracking: Number(10),
            tickets: vec![Number(11)],
        }]
    );
    assert_eq!(decided.waits.get(&Number(10)), None);
}

fn live_on_pull(pr: u64, kind: WorkKind) -> LiveRun {
    LiveRun {
        subject: Subject::Pull(Number(pr)),
        kind,
    }
}

#[test]
fn a_ticket_losing_its_label_or_linewright_neither_cancels_nor_stops_its_pull_requests_work() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("test", CheckState::Failed)];
    let mut unlabeled = in_review(pr.clone());
    unlabeled.issues[0].labels.clear();
    let mut unassigned = in_review(pr);
    unassigned.issues[0] = unassigned_by(
        labeled_at(ticket(2), label::READY_FOR_AGENT, 0),
        "maintainer",
        2,
    );
    let fixing = [live_on_pull(20, WorkKind::FixCi)];

    for world in [unlabeled, unassigned] {
        assert_eq!(
            at(&world, 10),
            vec![Action::Start(Work::FixCi {
                issue: Number(2),
                pr: Number(20),
                failing: vec!["test".into()],
            })]
        );
        assert_eq!(
            reconcile(
                &world,
                &fixing,
                &Memory::default(),
                &Workflows::default(),
                t(10),
                &policy()
            ),
            vec![]
        );
    }
}

#[test]
fn a_needs_triage_issue_someone_else_is_assigned_to_is_not_claimed_and_waits_for_them() {
    let mut assigned = labeled(issue(1), label::NEEDS_TRIAGE);
    assigned.assignees.push(Login::new("alice"));
    let world = snapshot(vec![assigned]);

    assert_eq!(decide(&world), vec![]);
    assert_eq!(
        waits_on(&world, 1),
        [Wait::Assignee {
            assignee: Login::new("alice")
        }]
    );
}

#[test]
fn a_needs_triage_issue_that_cannot_start_is_not_claimed_and_waits() {
    let blocked = blocked_by(labeled(issue(1), label::NEEDS_TRIAGE), 3, true);
    let closed = closed_by(labeled(issue(2), label::NEEDS_TRIAGE), 43, "alice");
    let world = snapshot(vec![blocked, closed]);

    assert_eq!(decide(&world), vec![]);
    assert_eq!(waits_on(&world, 1), [Wait::Blocker { issue: Number(3) }]);
    assert_eq!(
        waits_on(&world, 2),
        [Wait::ClosingPullRequest {
            pull_request: Number(43)
        }]
    );
}

#[test]
fn a_needs_triage_issue_linewright_is_assigned_to_is_triaged_without_claiming_it_again() {
    let world = snapshot(vec![claimed(labeled(issue(1), label::NEEDS_TRIAGE))]);

    assert_eq!(decide(&world), vec![triage(1)]);
}

fn needs_triage_unassigned_by(number: u64, actor: &str) -> Issue {
    unassigned_by(
        labeled_at(
            labeled(issue(number), label::NEEDS_TRIAGE),
            label::NEEDS_TRIAGE,
            1,
        ),
        actor,
        2,
    )
}

#[test]
fn a_needs_triage_issue_linewright_was_unassigned_from_waits_for_needs_triage_again() {
    let relabel = Wait::Relabel {
        label: StatusLabel::NeedsTriage,
        since: Relabel::Unassigned {
            required_label: None,
        },
    };
    for actor in ["maintainer", LINEWRIGHT] {
        let world = snapshot(vec![needs_triage_unassigned_by(1, actor)]);

        assert_eq!(decide(&world), vec![], "unassigned by {actor}");
        assert_eq!(
            waits_on(&world, 1),
            std::slice::from_ref(&relabel),
            "unassigned by {actor}"
        );
    }
}

#[test]
fn a_needs_triage_issue_labeled_again_after_an_unassignment_is_claimed() {
    let relabeled = labeled_at(
        needs_triage_unassigned_by(1, "maintainer"),
        label::NEEDS_TRIAGE,
        3,
    );

    assert_eq!(
        decide(&snapshot(vec![relabeled])),
        vec![claim(1), triage(1)]
    );
}

#[test]
fn of_a_labeling_and_an_unassignment_at_the_same_time_the_later_event_decides() {
    let labeled_then_unassigned = unassigned_by(
        labeled_at(
            labeled(issue(1), label::NEEDS_TRIAGE),
            label::NEEDS_TRIAGE,
            1,
        ),
        "maintainer",
        1,
    );
    let unassigned_then_labeled = labeled_at(
        needs_triage_unassigned_by(2, "maintainer"),
        label::NEEDS_TRIAGE,
        2,
    );

    assert_eq!(
        decide(&snapshot(vec![
            labeled_then_unassigned,
            unassigned_then_labeled
        ])),
        vec![claim(2), triage(2)]
    );
}

#[test]
fn with_a_required_label_applying_it_after_an_unassignment_lets_triage_claim_the_issue() {
    let stopped = labeled(needs_triage_unassigned_by(1, "maintainer"), REQUIRED);
    let relabeled = labeled_at(
        labeled(needs_triage_unassigned_by(2, "maintainer"), REQUIRED),
        REQUIRED,
        3,
    );

    assert_eq!(
        deciding_with_label(&snapshot(vec![stopped.clone(), relabeled])),
        vec![claim(2), triage(2)]
    );
    let required = Policy {
        require_label: Some(REQUIRED.into()),
        ..policy()
    };
    assert_eq!(
        waits(
            &snapshot(vec![stopped]),
            &[],
            &Memory::default(),
            10,
            &required
        )
        .remove(&Number(1))
        .unwrap_or_default(),
        [Wait::Relabel {
            label: StatusLabel::NeedsTriage,
            since: Relabel::Unassigned {
                required_label: Some(REQUIRED.into()),
            },
        }]
    );
}

#[test]
fn a_reply_to_needs_info_after_linewright_let_go_claims_the_issue_to_triage_it_again() {
    let mut asked = unassigned_by(
        labeled_at(labeled(issue(1), label::NEEDS_INFO), label::NEEDS_INFO, 1),
        LINEWRIGHT,
        1,
    );
    asked.comments = vec![triage_comment(1), comment("reporter", 2, "Version 3.")];

    assert_eq!(
        decide(&snapshot(vec![asked])),
        vec![
            Action::Relabel {
                issue: Number(1),
                remove: label::NEEDS_INFO.into(),
                add: label::NEEDS_TRIAGE.into(),
            },
            claim(1),
            triage(1),
        ]
    );
}

#[test]
fn a_triage_run_is_cancelled_when_linewright_is_unassigned_and_not_otherwise() {
    let unassigned = labeled(issue(1), label::NEEDS_TRIAGE);
    let still_claimed = claimed(labeled(issue(2), label::NEEDS_TRIAGE));
    let world = snapshot(vec![unassigned, still_claimed]);
    let runs = [live(1, WorkKind::Triage), live(2, WorkKind::Triage)];

    assert_eq!(
        reconcile(
            &world,
            &runs,
            &Memory::default(),
            &Workflows::default(),
            t(10),
            &policy(),
        ),
        vec![Action::Cancel {
            subject: Subject::Issue(Number(1)),
            reason: format!("{LINEWRIGHT} was unassigned from #1"),
        }]
    );
}
