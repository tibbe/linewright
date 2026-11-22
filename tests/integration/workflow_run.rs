use std::num::NonZeroU32;

use chrono::TimeDelta;
use linewright::automation::{self, Automation};
use linewright::domain::Timestamp;
use linewright::runs::{Ending, State};
use linewright::workflow_run::{
    AttemptConclusion, AttemptId, CompletedAttempt, Conclusion, Filters, NonEmptySet, Seen,
    Triggered, TriggeredRun, TriggersSeen, WorkflowRunId, awaited, trigger, within_limit,
};

use crate::common::at;

/// The non-empty set of `values`.
#[track_caller]
pub(crate) fn set<T: Ord>(values: impl IntoIterator<Item = T>) -> NonEmptySet<T> {
    values.into_iter().collect::<Vec<T>>().try_into().unwrap()
}

/// A failed attempt of CI on main.
#[track_caller]
fn ci_failed_on_main() -> CompletedAttempt {
    CompletedAttempt {
        id: AttemptId {
            run: WorkflowRunId(18_000_000_001),
            attempt: NonZeroU32::MIN,
        },
        workflow: "CI".into(),
        run_created_at: at("2026-10-01T12:00:00Z"),
        completed_at: at("2026-10-01T12:20:00Z"),
        conclusion: AttemptConclusion::Known(Conclusion::Failure),
        branch: Some("main".into()),
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
        link: "https://github.com/owner/repo/actions/runs/18000000001/attempts/1".into(),
    }
}

#[test_log::test]
fn filters_left_out_match_every_attempt() {
    let without_a_branch = CompletedAttempt {
        branch: None,
        conclusion: AttemptConclusion::Other("unheard_of".into()),
        ..ci_failed_on_main()
    };

    assert!(Filters::default().matches(&ci_failed_on_main()));
    assert!(Filters::default().matches(&without_a_branch));
}

#[test_log::test]
fn a_workflows_filter_matches_the_attempts_of_the_workflows_it_lists() {
    let filters = |workflows: &[&str]| Filters {
        workflows: Some(set(workflows.iter().map(ToString::to_string))),
        ..Filters::default()
    };

    assert!(filters(&["Deploy", "CI"]).matches(&ci_failed_on_main()));
    assert!(!filters(&["Deploy"]).matches(&ci_failed_on_main()));
    assert!(!filters(&["ci"]).matches(&ci_failed_on_main()));
}

#[test_log::test]
fn a_branches_filter_matches_the_attempts_on_the_branches_it_lists() {
    let filters = |branches: &[&str]| Filters {
        branches: Some(set(branches.iter().map(ToString::to_string))),
        ..Filters::default()
    };

    assert!(filters(&["main", "next"]).matches(&ci_failed_on_main()));
    assert!(!filters(&["next"]).matches(&ci_failed_on_main()));
}

#[test_log::test]
fn a_branches_filter_matches_no_attempt_without_a_branch() {
    let of_a_fork = CompletedAttempt {
        branch: None,
        ..ci_failed_on_main()
    };
    let main = Filters {
        branches: Some(set(["main".to_string()])),
        ..Filters::default()
    };

    assert!(!main.matches(&of_a_fork));
}

#[test_log::test]
fn a_conclusions_filter_matches_the_attempts_with_the_conclusions_it_lists() {
    let filters = |conclusions: &[Conclusion]| Filters {
        conclusions: Some(set(conclusions.iter().copied())),
        ..Filters::default()
    };
    let unheard_of = CompletedAttempt {
        conclusion: AttemptConclusion::Other("unheard_of".into()),
        ..ci_failed_on_main()
    };

    assert!(filters(&[Conclusion::TimedOut, Conclusion::Failure]).matches(&ci_failed_on_main()));
    assert!(!filters(&[Conclusion::Success]).matches(&ci_failed_on_main()));
    assert!(!filters(&[Conclusion::Failure]).matches(&unheard_of));
}

#[test_log::test]
fn an_attempt_matches_only_when_it_matches_every_filter() {
    let filters = Filters {
        workflows: Some(set(["CI".to_string()])),
        branches: Some(set(["main".to_string()])),
        conclusions: Some(set([Conclusion::Failure])),
    };
    let on_next = CompletedAttempt {
        branch: Some("next".into()),
        ..ci_failed_on_main()
    };

    assert!(filters.matches(&ci_failed_on_main()));
    assert!(!filters.matches(&on_next));
}

/// 12:00 on the day of the attempts, plus `minutes`.
fn t(minutes: i64) -> Timestamp {
    at("2026-10-01T12:00:00Z") + TimeDelta::minutes(minutes)
}

/// The automation `name`, run when CI fails on main.
#[track_caller]
fn fix_ci(name: &str) -> Automation {
    automation::parse(
        &name.parse().unwrap(),
        "---\non:\n  workflow_run:\n    workflows: [CI]\n    branches: [main]\n    \
         conclusions: [failure]\n---\nFix it.\n",
    )
    .unwrap()
}

/// The filters of [`fix_ci`].
fn ci_fails_on_main() -> Filters {
    Filters {
        workflows: Some(set(["CI".to_string()])),
        branches: Some(set(["main".to_string()])),
        conclusions: Some(set([Conclusion::Failure])),
    }
}

/// Every automation of `names` seen with `filters` since `minute`.
#[track_caller]
fn seen(names: &[&str], filters: &Filters, minute: i64) -> TriggersSeen {
    names
        .iter()
        .map(|name| {
            (
                name.parse().unwrap(),
                Seen {
                    filters: filters.clone(),
                    since: t(minute),
                },
            )
        })
        .collect()
}

/// Attempt `attempt` of the run `run` of CI on main, which failed at
/// `completed` minutes past 12:00, its run created 20 minutes before.
#[track_caller]
fn failed(run: u64, attempt: u32, completed: i64) -> CompletedAttempt {
    CompletedAttempt {
        id: AttemptId {
            run: WorkflowRunId(run),
            attempt: NonZeroU32::new(attempt).unwrap(),
        },
        run_created_at: t(completed - 20),
        completed_at: t(completed),
        link: format!("https://github.com/owner/repo/actions/runs/{run}/attempts/{attempt}"),
        ..ci_failed_on_main()
    }
}

/// The run of the automation `name` that `attempt` started, in `state`.
#[track_caller]
fn run_of(name: &str, attempt: &CompletedAttempt, state: State) -> TriggeredRun {
    TriggeredRun {
        automation: name.parse().unwrap(),
        attempt: attempt.id,
        state,
    }
}

fn ended(ending: Ending) -> State {
    State::Ended { at: t(50), ending }
}

/// What `trigger` decides at 13:00 with room for four runs and no holds.
fn triggered(
    automations: &[Automation],
    seen: &TriggersSeen,
    attempts: &[CompletedAttempt],
    runs: &[TriggeredRun],
) -> Triggered {
    trigger(automations, seen, attempts, runs, |_| false, 4, t(60))
}

/// The runs `trigger` decides, as the automations' names and the attempts.
fn runs(triggered: &Triggered) -> Vec<(String, CompletedAttempt)> {
    triggered
        .runs
        .iter()
        .map(|(automation, attempt)| (automation.name.to_string(), attempt.clone()))
        .collect()
}

#[test_log::test]
fn an_attempt_that_matches_the_filters_runs_the_automation_and_one_that_does_not_leaves_it() {
    let succeeded = CompletedAttempt {
        conclusion: AttemptConclusion::Known(Conclusion::Success),
        ..failed(2, 1, 30)
    };
    let seen = seen(&["fix-ci.md"], &ci_fails_on_main(), 0);

    let matching = triggered(&[fix_ci("fix-ci.md")], &seen, &[failed(1, 1, 30)], &[]);
    let not_matching = triggered(&[fix_ci("fix-ci.md")], &seen, &[succeeded], &[]);

    assert_eq!(
        runs(&matching),
        vec![("fix-ci.md".into(), failed(1, 1, 30))]
    );
    assert_eq!(matching.seen, seen);
    assert_eq!(runs(&not_matching), vec![]);
}

#[test_log::test]
fn an_attempt_that_completed_before_the_automation_was_first_seen_leaves_it() {
    let seen_at_12_30 = seen(&["fix-ci.md"], &ci_fails_on_main(), 30);

    let decided = triggered(
        &[fix_ci("fix-ci.md")],
        &seen_at_12_30,
        &[failed(1, 1, 29), failed(2, 1, 30)],
        &[],
    );

    assert_eq!(runs(&decided), vec![("fix-ci.md".into(), failed(2, 1, 30))]);
}

#[test_log::test]
fn an_automation_seen_for_the_first_time_is_seen_now_and_no_attempt_before_runs_it() {
    let decided = triggered(
        &[fix_ci("fix-ci.md")],
        &TriggersSeen::default(),
        &[failed(1, 1, 59)],
        &[],
    );

    assert_eq!(runs(&decided), vec![]);
    assert_eq!(decided.seen, seen(&["fix-ci.md"], &ci_fails_on_main(), 60));
}

#[test_log::test]
fn once_its_filters_change_only_attempts_that_completed_since_run_the_automation() {
    let seen_with_other_filters = seen(&["fix-ci.md"], &Filters::default(), 0);
    let at_13_00 = triggered(
        &[fix_ci("fix-ci.md")],
        &seen_with_other_filters,
        &[failed(1, 1, 30)],
        &[],
    );
    let at_13_10 = trigger(
        &[fix_ci("fix-ci.md")],
        &at_13_00.seen,
        &[failed(1, 1, 30), failed(2, 1, 65)],
        &[],
        |_| false,
        4,
        t(70),
    );

    assert_eq!(runs(&at_13_00), vec![]);
    assert_eq!(at_13_00.seen, seen(&["fix-ci.md"], &ci_fails_on_main(), 60));
    assert_eq!(
        runs(&at_13_10),
        vec![("fix-ci.md".into(), failed(2, 1, 65))]
    );
}

#[test_log::test]
fn filters_listed_in_another_order_are_the_same_filters() {
    let reordered = automation::parse(
        &"fix-ci.md".parse().unwrap(),
        "---\non:\n  workflow_run:\n    conclusions: [failure]\n    branches: [main]\n    \
         workflows: [CI]\n---\nFix it.\n",
    )
    .unwrap();
    let seen = seen(&["fix-ci.md"], &ci_fails_on_main(), 0);

    assert_eq!(triggered(&[reordered], &seen, &[], &[]).seen, seen);
}

#[test_log::test]
fn an_automation_not_on_workflow_runs_loses_its_filters_seen() {
    let hourly = automation::parse(
        &"fix-ci.md".parse().unwrap(),
        "---\non:\n  schedule: \"0 * * * *\"\n---\nFix it.\n",
    )
    .unwrap();
    let seen = seen(&["fix-ci.md"], &ci_fails_on_main(), 0);

    let decided = triggered(&[hourly], &seen, &[failed(1, 1, 30)], &[]);

    assert_eq!(runs(&decided), vec![]);
    assert_eq!(decided.seen, TriggersSeen::default());
}

#[test_log::test]
fn an_automations_runs_go_one_at_a_time_in_the_order_its_attempts_completed() {
    let seen = seen(&["fix-ci.md"], &ci_fails_on_main(), 0);
    let re_run_of_the_first = failed(1, 2, 40);
    let attempts = [
        re_run_of_the_first.clone(),
        failed(2, 1, 35),
        failed(1, 1, 30),
    ];

    let first = triggered(&[fix_ci("fix-ci.md")], &seen, &attempts, &[]);
    let second = triggered(
        &[fix_ci("fix-ci.md")],
        &seen,
        &attempts,
        &[run_of("fix-ci.md", &failed(1, 1, 30), ended(Ending::Done))],
    );
    let third = triggered(
        &[fix_ci("fix-ci.md")],
        &seen,
        &attempts,
        &[
            run_of("fix-ci.md", &failed(1, 1, 30), ended(Ending::Done)),
            run_of("fix-ci.md", &failed(2, 1, 35), ended(Ending::Done)),
        ],
    );

    assert_eq!(runs(&first), vec![("fix-ci.md".into(), failed(1, 1, 30))]);
    assert_eq!(runs(&second), vec![("fix-ci.md".into(), failed(2, 1, 35))]);
    assert_eq!(
        runs(&third),
        vec![("fix-ci.md".into(), re_run_of_the_first)]
    );
}

#[test_log::test]
fn each_automation_runs_on_its_own_earliest_attempt_and_earlier_ones_go_first() {
    let seen = seen(&["a.md", "b.md"], &ci_fails_on_main(), 0);
    let attempts = [failed(1, 1, 30), failed(2, 1, 35)];
    let a_ran_the_first = [run_of("a.md", &failed(1, 1, 30), ended(Ending::Done))];

    let decided = triggered(
        &[fix_ci("b.md"), fix_ci("a.md")],
        &seen,
        &attempts,
        &a_ran_the_first,
    );

    assert_eq!(
        runs(&decided),
        vec![
            ("b.md".into(), failed(1, 1, 30)),
            ("a.md".into(), failed(2, 1, 35)),
        ]
    );
}

#[test_log::test]
fn an_attempt_whose_run_was_cancelled_or_interrupted_runs_the_automation_again() {
    let seen = seen(&["fix-ci.md"], &ci_fails_on_main(), 0);
    let attempt = failed(1, 1, 30);

    for state in [
        ended(Ending::Cancelled),
        ended(Ending::UsageLimit),
        State::Interrupted,
    ] {
        let decided = triggered(
            &[fix_ci("fix-ci.md")],
            &seen,
            std::slice::from_ref(&attempt),
            &[run_of("fix-ci.md", &attempt, state)],
        );

        assert_eq!(
            runs(&decided),
            vec![("fix-ci.md".into(), attempt.clone())],
            "after a run {state:?}"
        );
    }
}

#[test_log::test]
fn an_attempt_whose_run_failed_ended_or_is_in_flight_does_not_run_the_automation_again() {
    let seen = seen(&["fix-ci.md"], &ci_fails_on_main(), 0);
    let attempt = failed(1, 1, 30);

    for state in [
        ended(Ending::Failed),
        ended(Ending::Done),
        ended(Ending::HandedBack),
        State::InFlight,
    ] {
        let decided = triggered(
            &[fix_ci("fix-ci.md")],
            &seen,
            std::slice::from_ref(&attempt),
            &[run_of("fix-ci.md", &attempt, state)],
        );

        assert_eq!(runs(&decided), vec![], "after a run {state:?}");
    }
}

#[test_log::test]
fn a_run_of_another_automation_on_the_attempt_leaves_it_pending() {
    let seen = seen(&["fix-ci.md"], &ci_fails_on_main(), 0);
    let attempt = failed(1, 1, 30);

    let decided = triggered(
        &[fix_ci("fix-ci.md")],
        &seen,
        std::slice::from_ref(&attempt),
        &[run_of("other.md", &attempt, ended(Ending::Done))],
    );

    assert_eq!(runs(&decided), vec![("fix-ci.md".into(), attempt)]);
}

#[test_log::test]
fn a_held_automation_keeps_its_attempts_pending_and_its_filters_seen() {
    let seen = seen(&["a.md", "b.md"], &ci_fails_on_main(), 0);
    let holding_a = |automation: &Automation| automation.name.to_string() == "a.md";

    let decided = trigger(
        &[fix_ci("a.md"), fix_ci("b.md")],
        &seen,
        &[failed(1, 1, 30)],
        &[],
        holding_a,
        4,
        t(60),
    );

    assert_eq!(runs(&decided), vec![("b.md".into(), failed(1, 1, 30))]);
    assert_eq!(decided.seen, seen);
}

#[test_log::test]
fn the_runs_beyond_the_room_wait_and_the_earliest_attempts_get_it() {
    let seen = seen(&["a.md", "b.md", "c.md"], &ci_fails_on_main(), 0);
    let attempts = [failed(1, 1, 40), failed(2, 1, 30)];
    let ran = [
        run_of("a.md", &failed(2, 1, 30), ended(Ending::Done)),
        run_of("b.md", &failed(1, 1, 40), ended(Ending::Done)),
        run_of("b.md", &failed(2, 1, 30), ended(Ending::Done)),
    ];
    let automations = [fix_ci("a.md"), fix_ci("b.md"), fix_ci("c.md")];

    let decided = trigger(&automations, &seen, &attempts, &ran, |_| false, 1, t(60));
    let no_room = trigger(&automations, &seen, &attempts, &ran, |_| false, 0, t(60));

    assert_eq!(runs(&decided), vec![("c.md".into(), failed(2, 1, 30))]);
    assert_eq!(runs(&no_room), vec![]);
    assert_eq!(no_room.seen, seen);
}

#[test_log::test]
fn an_attempt_is_awaited_while_an_automation_has_it_pending() {
    let seen = seen(&["fix-ci.md"], &ci_fails_on_main(), 0);
    let automations = [fix_ci("fix-ci.md")];
    let ran = [run_of("fix-ci.md", &failed(1, 1, 30), ended(Ending::Done))];
    let succeeded = CompletedAttempt {
        conclusion: AttemptConclusion::Known(Conclusion::Success),
        ..failed(3, 1, 30)
    };

    let awaited = awaited(&automations, &seen, &ran, t(60));

    assert!(awaited(&failed(2, 1, 30)));
    assert!(!awaited(&failed(1, 1, 30)));
    assert!(!awaited(&succeeded));
}

#[test_log::test]
fn an_attempt_whose_run_is_in_flight_is_awaited_but_not_pending() {
    let seen = seen(&["fix-ci.md"], &ci_fails_on_main(), 0);
    let automations = [fix_ci("fix-ci.md")];
    let running = [run_of("fix-ci.md", &failed(1, 1, 30), State::InFlight)];

    let decided = triggered(&automations, &seen, &[failed(1, 1, 30)], &running);
    let awaited = awaited(&automations, &seen, &running, t(60));

    assert_eq!(runs(&decided), vec![]);
    assert!(
        awaited(&failed(1, 1, 30)),
        "it runs again if its run is cancelled"
    );
}

#[test_log::test]
fn an_attempt_is_within_the_limit_until_ci_timeout_after_its_run_was_created() {
    let created_at_12_10 = failed(1, 1, 30);
    let six_hours = TimeDelta::hours(6);

    assert!(within_limit(&created_at_12_10, t(10 + 360), six_hours));
    assert!(!within_limit(&created_at_12_10, t(10 + 361), six_hours));
}
