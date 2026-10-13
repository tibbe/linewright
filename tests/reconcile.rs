//! The functional core, end to end on its data model: a state of the world in,
//! the decisions out (`CODING_STANDARDS.md`, "Where tests go").

use chrono::{DateTime, TimeDelta, Utc};
use linewright::domain::*;
use linewright::marker::{Stage, marked};
use linewright::reconcile::*;
use linewright::spec_glob::SpecGlob;
use linewright::workflow::{LastRuns, Workflow};

const MACHINE: &str = "factory";

fn t(minute: i64) -> Timestamp {
    DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + TimeDelta::minutes(minute)
}

fn policy() -> Policy {
    Policy {
        machine_user: Login::new(MACHINE),
        spec_glob: SpecGlob::default(),
        spec_lookback: TimeDelta::days(7),
        max_workers: 4,
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

#[test]
fn a_needs_triage_issue_is_triaged() {
    let world = snapshot(vec![labeled(issue(1), label::NEEDS_TRIAGE)]);

    assert_eq!(
        decide(&world),
        vec![Action::Start(Work::Triage { issue: Number(1) })]
    );
}

fn comment(author: &str, minute: i64, body: &str) -> Comment {
    Comment {
        author: Login::new(author),
        body: body.to_string(),
        created_at: t(minute),
    }
}

fn triage_comment(minute: i64) -> Comment {
    comment(
        MACHINE,
        minute,
        &marked(Stage::Triage, "What version are you on?"),
    )
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
            Action::Start(Work::Triage { issue: Number(1) }),
        ]
    );
}

#[test]
fn needs_info_waits_while_only_linewright_has_spoken_since_triage() {
    let mut asked = labeled(issue(1), label::NEEDS_INFO);
    asked.comments = vec![
        comment("reporter", 0, "It crashes."),
        triage_comment(1),
        comment(MACHINE, 2, "Anything else?"),
    ];

    assert_eq!(decide(&snapshot(vec![asked])), vec![]);
}

#[test]
fn a_maintainers_answer_to_a_ready_for_human_triage_sends_the_issue_back_to_triage() {
    let mut asked = labeled(issue(1), label::READY_FOR_HUMAN);
    asked.comments = vec![
        triage_comment(1),
        comment("maintainer", 2, "Make them resizable."),
    ];

    assert_eq!(
        decide(&snapshot(vec![asked])),
        vec![
            Action::Relabel {
                issue: Number(1),
                remove: label::READY_FOR_HUMAN.into(),
                add: label::NEEDS_TRIAGE.into(),
            },
            Action::Start(Work::Triage { issue: Number(1) }),
        ]
    );
}

#[test]
fn only_comments_by_others_than_the_machine_user_after_the_last_triage_comment_are_answers() {
    for state in [label::NEEDS_INFO, label::READY_FOR_HUMAN] {
        let mut asked = labeled(issue(1), state);
        asked.comments = vec![
            comment("maintainer", 0, "Should they be resizable?"),
            triage_comment(1),
            comment(MACHINE, 2, "Anything else?"),
        ];

        assert_eq!(decide(&snapshot(vec![asked])), vec![], "{state}");
    }
}

#[test]
fn a_comment_on_a_hand_back_from_after_triage_starts_nothing() {
    for stage in [Stage::Ci, Stage::Review, Stage::Implementation] {
        let mut handed_back = labeled(issue(1), label::READY_FOR_HUMAN);
        handed_back.comments = vec![
            triage_comment(1),
            comment(MACHINE, 3, &marked(stage, "Handed back.")),
            comment("maintainer", 5, "Looking into it."),
        ];

        assert_eq!(decide(&snapshot(vec![handed_back])), vec![], "{stage}");
    }
}

#[test]
fn an_issue_the_machine_user_filed_is_not_triaged_when_a_person_comments() {
    let mut filed = labeled(issue(1), label::READY_FOR_HUMAN);
    filed.author = Login::new(MACHINE);
    filed.comments = vec![triage_comment(1), comment("maintainer", 2, "Agreed.")];

    assert_eq!(decide(&snapshot(vec![filed])), vec![]);
}

#[test]
fn a_label_the_maintainer_applies_to_a_ready_for_human_triage_wins_over_their_comment() {
    let answered_with = |state: &str| {
        let mut answered = labeled(labeled(issue(1), label::READY_FOR_HUMAN), state);
        answered.comments = vec![
            triage_comment(1),
            comment("maintainer", 2, "Make them resizable."),
        ];
        snapshot(vec![answered])
    };

    assert_eq!(
        decide(&answered_with(label::READY_FOR_AGENT)),
        vec![Action::Claim { issue: Number(1) }, implement(1)]
    );
    assert_eq!(
        decide(&answered_with(label::NEEDS_TRIAGE)),
        vec![Action::Start(Work::Triage { issue: Number(1) })]
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
        files: files.iter().map(|f| f.to_string()).collect(),
    }
}

#[test]
fn a_merged_pull_request_that_touches_the_spec_is_cut_into_tickets() {
    let mut world = snapshot(vec![]);
    world.merged = vec![merged(5, &["src/main.rs", "docs/billing/invoices.md"])];

    assert_eq!(
        decide(&world),
        vec![Action::Start(Work::Tickets { pr: Number(5) })]
    );
}

#[test]
fn only_unprocessed_spec_changes_by_people_within_the_lookback_are_picked_up() {
    let code_only = merged(1, &["src/main.rs", "README.md"]);
    let mut processed = merged(2, &["docs/spec.md"]);
    processed.labels.push(label::SPEC_PROCESSED.into());
    let mut by_machine = merged(3, &["docs/spec.md"]);
    by_machine.author = Login::new(MACHINE);
    let mut too_old = merged(4, &["docs/spec.md"]);
    too_old.merged_at = t(10) - TimeDelta::days(8);
    let mut other_base = merged(6, &["docs/spec.md"]);
    other_base.base_ref = "release".into();
    let mut handed_back = merged(7, &["docs/spec.md"]);
    handed_back.labels.push(label::READY_FOR_HUMAN.into());
    let mut world = snapshot(vec![]);
    world.merged = vec![
        code_only,
        processed,
        by_machine,
        too_old,
        other_base,
        handed_back,
    ];

    assert_eq!(decide(&world), vec![]);
}

#[test]
fn an_interrupted_decomposition_is_discarded_so_the_spec_change_is_cut_again() {
    let mut world = snapshot(vec![tracking(9, 5, &[(10, true)])]);
    world.merged = vec![merged(5, &["docs/spec.md"])];
    let cutting = [LiveRun {
        subject: Subject::SpecChange(Number(5)),
        kind: WorkKind::Tickets,
    }];

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
            &cutting,
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
    ticket.author = Login::new(MACHINE);
    ticket
}

fn claimed(mut issue: Issue) -> Issue {
    issue.assignees.push(Login::new(MACHINE));
    issue
}

fn blocked_by(mut issue: Issue, blocker: u64, open: bool) -> Issue {
    issue.blocked_by.push(Linked {
        number: Number(blocker),
        open,
    });
    issue
}

fn implement(issue: u64) -> Action {
    Action::Start(Work::Implement {
        issue: Number(issue),
    })
}

#[test]
fn an_unblocked_ready_for_agent_issue_is_claimed_and_implemented() {
    let world = snapshot(vec![blocked_by(ticket(2), 1, false)]);

    assert_eq!(
        decide(&world),
        vec![Action::Claim { issue: Number(2) }, implement(2)]
    );
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
    issue.events.push(IssueEvent::Unassigned {
        assignee: Login::new(MACHINE),
        actor: Login::new(actor),
        at: t(minute),
    });
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
fn unassigning_the_machine_user_stops_the_work_until_the_label_is_applied_again() {
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
    let handed_back_by_linewright =
        unassigned_by(labeled_at(ticket(4), label::READY_FOR_AGENT, 1), MACHINE, 2);

    assert_eq!(
        decide(&snapshot(vec![
            stopped,
            restarted,
            handed_back_by_linewright
        ])),
        vec![
            Action::Claim { issue: Number(3) },
            implement(3),
            Action::Claim { issue: Number(4) },
            implement(4),
        ]
    );
}

#[test]
fn an_issue_waiting_for_triage_is_not_claimed() {
    let world = snapshot(vec![labeled(ticket(2), label::NEEDS_TRIAGE)]);

    assert_eq!(decide(&world), vec![]);
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
            Action::Start(Work::Triage { issue: Number(3) }),
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
        live(2, WorkKind::Rework),
        live(3, WorkKind::FixCi),
        live(4, WorkKind::Update),
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
        [1, 2, 3, 4, 5].map(|n| Subject::Issue(Number(n))).to_vec()
    );
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
        author: Login::new(MACHINE),
        created_at: t(1),
        head_ref: format!("{closes}-ticket"),
        head_sha: "abc123".into(),
        state: PullState::Open { draft: true },
        closes: vec![Number(closes)],
        pushes: vec![Push {
            by: Login::new(MACHINE),
            at: t(1),
        }],
        mergeability: Mergeability::Clean,
        rebaseability: Rebaseability::Rebaseable,
        checks: vec![],
        comments: vec![],
        reviews: vec![],
        threads: vec![],
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

fn ci_fix_comment(minute: i64) -> Comment {
    comment(
        MACHINE,
        minute,
        &marked(Stage::Ci, "Fixed the failing test."),
    )
}

#[test]
fn a_failing_check_starts_a_fix_and_after_the_last_attempt_the_ticket_is_handed_back() {
    let mut pr = pull(20, 2);
    pr.checks = vec![
        check("test", CheckState::Failed),
        check("lint", CheckState::Passed),
    ];
    let first = in_review(pr.clone());
    pr.comments = vec![ci_fix_comment(3), ci_fix_comment(6)];
    let exhausted = in_review(pr);

    assert_eq!(
        at(&first, 10),
        vec![Action::Start(Work::FixCi {
            issue: Number(2),
            pr: Number(20),
            failing: vec!["test".into()],
        })]
    );
    assert!(matches!(
        at(&exhausted, 10).as_slice(),
        [Action::HandBack { issue: Number(2), stage: Stage::Ci, reason }] if reason.contains("`test`")
    ));
}

#[test]
fn a_ticket_given_back_after_a_ci_timeout_waits_a_full_timeout_again() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("e2e", CheckState::Pending)];
    let mut world = in_review(pr);
    world.issues[0].comments = vec![comment(
        MACHINE,
        6 * 60 + 3,
        &marked(
            Stage::Ci,
            "CI did not complete within 6h: `e2e` still running.",
        ),
    )];

    assert_eq!(at(&world, 6 * 60 + 10), vec![]);
}

#[test]
fn checks_that_never_complete_hand_the_ticket_back_at_the_ci_timeout() {
    let mut pr = pull(20, 2);
    pr.checks = vec![check("e2e", CheckState::Pending)];
    let world = in_review(pr);

    assert_eq!(at(&world, 6 * 60), vec![]);
    assert!(matches!(
        at(&world, 6 * 60 + 2).as_slice(),
        [Action::HandBack { issue: Number(2), stage: Stage::Ci, reason }] if reason.contains("`e2e`")
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
        comment(MACHINE, 3, "Thanks!"),
    ];
    let before_the_push = in_review(pr.clone());
    pr.comments = vec![
        comment("coverage[bot]", 4, "Coverage dropped 0.1%."),
        comment(
            MACHINE,
            5,
            &marked(Stage::Rework, "No change needed: noise."),
        ),
    ];
    let answered_without_a_push = in_review(pr);

    assert_eq!(at(&before_the_push, 10), vec![]);
    assert_eq!(at(&answered_without_a_push, 10), vec![]);
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

/// A green pull request ready for review, up to date, whose branch has a
/// merge commit GitHub cannot rebase.
fn not_rebaseable() -> Pull {
    let mut pr = ready(pull(20, 2));
    pr.checks = vec![check("test", CheckState::Passed)];
    pr.rebaseability = Rebaseability::NotRebaseable;
    pr
}

fn deciding(world: &Snapshot, memory: &Memory, policy: &Policy) -> Vec<Action> {
    reconcile(world, &[], memory, &Workflows::default(), t(10), policy)
}

/// A rebase of #20 at `head`, tried at `minute`.
fn tried(head: &str, minute: i64) -> TriedRebase {
    TriedRebase {
        pr: Number(20),
        head: head.into(),
        at: t(minute),
    }
}

/// An update run of #20 that rebases it, started at minute 10 and
/// remembered as it starts.
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
fn a_ticket_handed_back_since_a_rebase_was_tried_tries_it_again_once_labeled_again() {
    let mut world = in_review(not_rebaseable());
    world.issues[0].comments = vec![comment(
        MACHINE,
        7,
        &marked(Stage::Update, "The run failed twice on its infrastructure."),
    )];

    assert_eq!(
        deciding(&world, &remembering(tried("abc123", 5)), &policy()),
        rebase_update()
    );
}

#[test]
fn a_ticket_labeled_again_since_a_rebase_was_tried_tries_it_again() {
    let mut world = in_review(not_rebaseable());
    world.issues[0].events = vec![IssueEvent::Labeled {
        label: label::READY_FOR_AGENT.into(),
        at: t(7),
    }];

    assert_eq!(
        deciding(&world, &remembering(tried("abc123", 5)), &policy()),
        rebase_update()
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
fn with_auto_merge_by_rebase_a_pull_request_still_not_rebaseable_after_a_rebase_hands_its_ticket_back()
 {
    let world = in_review(not_rebaseable());

    assert_eq!(
        deciding(
            &world,
            &remembering(tried("abc123", 5)),
            &merging_by(MergeMethod::Rebase)
        ),
        vec![Action::HandBack {
            issue: Number(2),
            stage: Stage::Review,
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

/// Linewright's comment on a pull request held for a code owner's review.
fn held_for_a_code_owner(minute: i64) -> Comment {
    comment(
        MACHINE,
        minute,
        "> *Linewright, review*\n\nReady for review: GitHub requires a code owner's review \
         (@alice) before merging. Linewright merges it once GitHub allows it.",
    )
}

#[test]
fn a_pull_request_github_blocks_and_already_held_for_review_waits_also_after_a_push() {
    let mut pr = ready(pull(20, 2));
    pr.mergeability = Mergeability::Blocked {
        requires: code_owner_review(),
    };
    pr.comments = vec![held_for_a_code_owner(2)];
    let held = in_review(pr.clone());
    pr.pushes.push(Push {
        by: Login::new(MACHINE),
        at: t(3),
    });
    let updated_since = in_review(pr);

    assert_eq!(auto_merging(&held, 10), vec![]);
    assert_eq!(auto_merging(&updated_since, 10), vec![]);
}

#[test]
fn a_pull_request_held_for_review_is_merged_once_github_no_longer_blocks_it() {
    let mut pr = ready(pull(20, 2));
    pr.comments = vec![held_for_a_code_owner(7)];

    assert_eq!(auto_merging(&in_review(pr), 10), vec![merge(20, false)]);
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
    pr.comments = vec![held_for_a_code_owner(7)];

    assert_eq!(
        auto_merging(&in_review(pr), 10),
        vec![Action::HoldForReview {
            pr: Number(20),
            draft: false,
            requires,
        }]
    );
}

#[test]
fn a_pull_request_marked_ready_without_auto_merge_is_held_when_github_blocks_it() {
    let mut pr = ready(pull(20, 2));
    pr.mergeability = Mergeability::Blocked {
        requires: code_owner_review(),
    };
    pr.comments = vec![comment(
        MACHINE,
        7,
        &marked(Stage::Review, "CI is green: ready for review."),
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
        by: Login::new(MACHINE),
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
fn with_auto_merge_comments_from_the_machine_user_after_an_approval_do_not_hold_the_merge() {
    let mut pr = reworked(ready(pull(20, 2)));
    pr.reviews = vec![review("reviewer", ReviewState::Approved, "", 2)];
    pr.comments = vec![comment(MACHINE, 3, "Renamed `foo`.")];

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
    FailedMerge {
        pr: Number(pr),
        reason: reason.into(),
    }
}

fn auto_merging_after(world: &Snapshot, failed: &[FailedMerge], minute: i64) -> Vec<Action> {
    let memory = Memory {
        failed_merges: failed.to_vec(),
        ..Memory::default()
    };
    let policy = Policy {
        auto_merge: Some(MergeMethod::Merge),
        ..policy()
    };
    reconcile(
        world,
        &[],
        &memory,
        &Workflows::default(),
        t(minute),
        &policy,
    )
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
fn with_auto_merge_a_green_pull_request_whose_merge_failed_twice_hands_its_ticket_back_with_githubs_reason()
 {
    let world = in_review(ready(pull(20, 2)));
    let failed = [
        failed_merge(20, "Head branch was modified."),
        failed_merge(20, "Pull Request is not mergeable"),
    ];

    assert_eq!(
        auto_merging_after(&world, &failed, 11),
        vec![
            Action::ForgetFailedMerges { pr: Number(20) },
            Action::HandBack {
                issue: Number(2),
                stage: Stage::Review,
                reason: "Merging #20 failed twice. GitHub's reason: Pull Request is not mergeable"
                    .into(),
            },
        ]
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

#[test]
fn with_auto_merge_relabeling_a_ticket_handed_back_for_failed_merges_tries_the_merge_again() {
    let mut world = in_review(ready(pull(20, 2)));
    let ticket = &mut world.issues[0];
    ticket.assignees.retain(|a| a.as_str() != MACHINE);
    ticket.comments = vec![comment(
        MACHINE,
        12,
        &marked(
            Stage::Review,
            "Merging #20 failed twice. GitHub's reason: Pull Request is not mergeable",
        ),
    )];
    ticket.events = vec![IssueEvent::Labeled {
        label: label::READY_FOR_AGENT.into(),
        at: t(15),
    }];

    assert_eq!(
        auto_merging(&world, 20),
        vec![Action::Claim { issue: Number(2) }, merge(20, false)]
    );
}

#[test]
fn a_pull_request_closed_without_merging_hands_the_ticket_back_once() {
    let mut pr = pull(20, 2);
    pr.state = PullState::Closed { at: t(5) };
    let closed = in_review(pr);
    let mut handed_back = closed.clone();
    handed_back.issues[0].comments = vec![comment(
        MACHINE,
        6,
        &marked(Stage::Review, "#20 was closed without merging."),
    )];

    assert!(matches!(
        at(&closed, 10).as_slice(),
        [Action::HandBack {
            issue: Number(2),
            stage: Stage::Review,
            ..
        }]
    ));
    assert_eq!(at(&handed_back, 10), vec![implement(2)]);
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
    tracking.author = Login::new(MACHINE);
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

// Workflows: the due rule (`docs/specs/workflows.md`).

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
        vec![Action::Start(Work::Triage { issue: Number(2) })]
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
            Action::Start(Work::Triage { issue: Number(2) }),
        ]
    );
}

#[test]
fn with_a_required_label_only_a_labeled_unclaimed_ticket_is_claimed() {
    let unlabeled = ticket(1);
    let with_label = labeled(ticket(2), REQUIRED);

    assert_eq!(
        deciding_with_label(&snapshot(vec![unlabeled, with_label])),
        vec![Action::Claim { issue: Number(2) }, implement(2)]
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
        vec![Action::Claim { issue: Number(2) }, implement(2)]
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

    assert_eq!(
        deciding_with_label(&world),
        vec![Action::Start(Work::Tickets { pr: Number(2) })]
    );
}

#[test]
fn with_a_required_label_a_spec_change_labeled_after_it_merged_is_picked_up_on_a_later_poll() {
    let mut pr = merged(1, &["docs/spec.md"]);
    let mut world = snapshot(vec![]);
    world.merged = vec![pr.clone()];
    assert_eq!(deciding_with_label(&world), vec![]);

    pr.labels.push(REQUIRED.into());
    world.merged = vec![pr];
    assert_eq!(
        deciding_with_label(&world),
        vec![Action::Start(Work::Tickets { pr: Number(1) })]
    );
}

#[test]
fn with_a_required_label_a_retried_handback_is_cut_again_only_if_labeled() {
    // The maintainer retries by removing `ready-for-human`.
    let retried_unlabeled = merged(1, &["docs/spec.md"]);
    let mut retried = merged(2, &["docs/spec.md"]);
    retried.labels.push(REQUIRED.into());
    let mut still_handed_back = merged(3, &["docs/spec.md"]);
    still_handed_back.labels = vec![REQUIRED.into(), label::READY_FOR_HUMAN.into()];
    let mut world = snapshot(vec![]);
    world.merged = vec![retried_unlabeled, retried, still_handed_back];

    assert_eq!(
        deciding_with_label(&world),
        vec![Action::Start(Work::Tickets { pr: Number(2) })]
    );
}

#[test]
fn with_a_required_label_processed_and_machine_spec_changes_are_skipped_even_if_labeled() {
    let mut processed = merged(1, &["docs/spec.md"]);
    processed.labels = vec![REQUIRED.into(), label::SPEC_PROCESSED.into()];
    let mut by_machine = merged(2, &["docs/spec.md"]);
    by_machine.labels = vec![REQUIRED.into()];
    by_machine.author = Login::new(MACHINE);
    let mut world = snapshot(vec![]);
    world.merged = vec![processed, by_machine];

    assert_eq!(deciding_with_label(&world), vec![]);
}

#[test]
fn removing_the_required_label_does_not_cancel_a_cut_under_way() {
    let mut world = snapshot(vec![]);
    world.merged = vec![merged(5, &["docs/spec.md"])];
    let runs = [LiveRun {
        subject: Subject::SpecChange(Number(5)),
        kind: WorkKind::Tickets,
    }];

    assert_eq!(
        reconcile(
            &world,
            &runs,
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

// The usage limit (`docs/specs/operation.md`, "Usage limit").

/// A world with work of every kind to start and actions that start no
/// session: an answered question, a run whose ticket lost its label, a
/// ticket out of CI attempts, a green draft and a tracking issue whose
/// tickets are all closed, beside a workflow due since 13:00.
fn work_of_every_kind() -> (Snapshot, Vec<LiveRun>, Workflows) {
    let mut answered = labeled(issue(1), label::NEEDS_INFO);
    answered.comments = vec![triage_comment(1), comment("reporter", 2, "Version 3.")];
    let mut reworked = ready(pull(20, 2));
    reworked.comments = vec![comment("maintainer", 4, "Please rename `foo`.")];
    let mut green = pull(21, 6);
    green.checks = vec![check("test", CheckState::Passed)];
    let mut failing = pull(22, 11);
    failing.checks = vec![check("test", CheckState::Failed)];
    failing.comments = vec![ci_fix_comment(3), ci_fix_comment(6)];
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

fn under_usage_limit_until(resets_at: i64) -> Memory {
    Memory {
        usage_limit_resets_at: Some(t(resets_at)),
        ..Memory::default()
    }
}

/// The decisions at 13:10 with room for every run, under a usage limit
/// that resets at `resets_at` minutes past 12:00.
fn under_usage_limit(resets_at: i64) -> Vec<Action> {
    let (world, live, workflows) = work_of_every_kind();
    let policy = Policy {
        max_workers: 10,
        ..policy()
    };
    let memory = under_usage_limit_until(resets_at);
    reconcile(&world, &live, &memory, &workflows, t(70), &policy)
}

/// The actions in [`work_of_every_kind`] that start no session.
fn sessionless_actions() -> Vec<Action> {
    vec![
        Action::Cancel {
            subject: Subject::Issue(Number(12)),
            reason: "`ready-for-agent` was removed from #12".into(),
        },
        Action::Relabel {
            issue: Number(1),
            remove: label::NEEDS_INFO.into(),
            add: label::NEEDS_TRIAGE.into(),
        },
        Action::MarkReady { pr: Number(21) },
        Action::CloseTracking { issue: Number(9) },
    ]
}

#[test]
fn under_the_usage_limit_no_run_starts_and_no_issue_is_claimed_but_other_actions_are_taken() {
    let actions = under_usage_limit(80);

    let (handed_back, others): (Vec<Action>, Vec<Action>) = actions
        .into_iter()
        .partition(|a| matches!(a, Action::HandBack { .. }));
    assert_eq!(others, sessionless_actions());
    assert!(matches!(
        handed_back.as_slice(),
        [Action::HandBack {
            issue: Number(11),
            stage: Stage::Ci,
            ..
        }]
    ));
}

#[test]
fn once_the_usage_limit_resets_every_run_starts_and_the_due_workflow_runs() {
    let starts: Vec<Action> = under_usage_limit(70)
        .into_iter()
        .filter(|a| !sessionless_actions().contains(a) && !matches!(a, Action::HandBack { .. }))
        .collect();

    assert_eq!(
        starts,
        vec![
            rework(2, 20),
            Action::Start(Work::Triage { issue: Number(1) }),
            Action::Start(Work::Tickets { pr: Number(5) }),
            Action::Claim { issue: Number(3) },
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
