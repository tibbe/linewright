use std::num::NonZeroU32;

use crate::common::at;
use linewright::workflow_run::{
    AttemptConclusion, AttemptId, CompletedAttempt, Conclusion, Filters, NonEmptySet, WorkflowRunId,
};

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
