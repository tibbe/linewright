use linewright::domain::{MergeMethod, Requirement};
use linewright::working_pull::{Branch, Checks, Facts, Row, Verdict, row};

/// A green draft with nothing else to do: only marking it ready holds.
fn green_draft() -> Facts {
    Facts {
        feedback: false,
        branch: Branch::Mergeable,
        checks: Checks::Green,
        fix_ci_runs: 0,
        ci_attempts: 2,
        draft: true,
        auto_merge: None,
        held: false,
        failed_merges: vec![],
        verdict: Verdict::NotReported,
    }
}

fn failing() -> Checks {
    Checks::Failed(vec!["test".into()])
}

#[test_log::test]
fn feedback_starts_a_rework_run_before_any_other_row() {
    let facts = Facts {
        feedback: true,
        branch: Branch::Behind,
        checks: failing(),
        fix_ci_runs: 2,
        ..green_draft()
    };

    assert_eq!(row(&facts), Row::Rework);
}

#[test_log::test]
fn a_branch_behind_or_in_conflict_starts_an_update_run_before_the_checks_are_read() {
    let facts = Facts {
        branch: Branch::Behind,
        checks: failing(),
        fix_ci_runs: 2,
        ..green_draft()
    };

    assert_eq!(row(&facts), Row::Update);
}

#[test_log::test]
fn an_unmergeable_branch_starts_an_update_run_before_the_checks_are_read_until_a_run_ends_since_its_last_push()
 {
    let unmergeable = |ended_since_push| Facts {
        branch: Branch::Unmergeable { ended_since_push },
        checks: Checks::NoneAppeared { within_grace: true },
        ..green_draft()
    };

    assert_eq!(row(&unmergeable(false)), Row::Update);
    assert_eq!(row(&unmergeable(true)), Row::WaitForChecks);
}

#[test_log::test]
fn a_failed_check_starts_a_fix_ci_run_while_fewer_than_ci_attempts_have_ended_this_round() {
    let facts = Facts {
        checks: failing(),
        fix_ci_runs: 1,
        ..green_draft()
    };

    assert_eq!(
        row(&facts),
        Row::FixCi {
            failing: vec!["test".into()]
        }
    );
}

#[test_log::test]
fn a_failed_check_hands_it_back_once_ci_attempts_fix_ci_runs_have_ended_this_round() {
    let facts = Facts {
        checks: failing(),
        fix_ci_runs: 2,
        ..green_draft()
    };

    assert_eq!(
        row(&facts),
        Row::HandBackFailing {
            failing: vec!["test".into()],
            attempts: 2,
        }
    );
}

/// GitHub blocking the merge until a code owner reviews it, and whether
/// Linewright `commented` on that.
fn blocked(commented: bool) -> Verdict {
    Verdict::Blocks {
        requires: vec![Requirement::CodeOwnerReview {
            owners: vec!["alice".into()],
        }],
        commented,
    }
}

/// A green pull request ready for review, merged by `auto-merge`, that
/// GitHub allows to merge.
fn mergeable() -> Facts {
    Facts {
        draft: false,
        auto_merge: Some(MergeMethod::Merge),
        verdict: Verdict::Allows,
        ..green_draft()
    }
}

fn failed_twice() -> Vec<String> {
    vec!["Head branch was modified.".into(), "Not mergeable".into()]
}

#[test_log::test]
fn a_green_draft_is_marked_ready_before_anything_auto_merge_does_with_it() {
    let facts = Facts {
        draft: true,
        held: true,
        failed_merges: failed_twice(),
        verdict: blocked(false),
        ..mergeable()
    };

    assert_eq!(row(&facts), Row::MarkReady);
    assert_eq!(
        row(&Facts {
            checks: Checks::NoneAppeared {
                within_grace: false
            },
            ..facts
        }),
        Row::MarkReady
    );
}

#[test_log::test]
fn a_pull_request_ready_for_review_waits_for_review_without_auto_merge_or_while_a_person_holds_its_merge()
 {
    let blocked = Facts {
        failed_merges: failed_twice(),
        verdict: blocked(false),
        ..mergeable()
    };

    assert_eq!(
        row(&Facts {
            auto_merge: None,
            ..blocked.clone()
        }),
        Row::WaitForReview
    );
    assert_eq!(
        row(&Facts {
            held: true,
            ..blocked
        }),
        Row::WaitForReview
    );
}

#[test_log::test]
fn two_failed_merges_this_round_hand_it_back_with_githubs_last_reason_whatever_github_reports() {
    for verdict in [Verdict::Allows, Verdict::NotReported, blocked(false)] {
        let facts = Facts {
            failed_merges: failed_twice(),
            verdict,
            ..mergeable()
        };

        assert_eq!(
            row(&facts),
            Row::HandBackMergeFailed {
                reason: "Not mergeable".into()
            }
        );
    }
}

#[test_log::test]
fn a_merge_github_blocks_is_commented_on_naming_what_it_requires_once_this_round() {
    let blocked = |commented| Facts {
        failed_merges: vec!["Head branch was modified.".into()],
        verdict: Verdict::Blocks {
            requires: vec![],
            commented,
        },
        ..mergeable()
    };

    assert_eq!(
        row(&blocked(false)),
        Row::CommentRequirements { requires: vec![] }
    );
    assert_eq!(
        row(&blocked(true)),
        Row::WaitForRequirements { requires: vec![] }
    );
}

#[test_log::test]
fn a_merge_github_has_not_reported_on_waits_for_it() {
    let facts = Facts {
        verdict: Verdict::NotReported,
        ..mergeable()
    };

    assert_eq!(row(&facts), Row::WaitForMergeState);
}

#[test_log::test]
fn a_merge_github_allows_is_made_with_auto_merges_method_after_one_failed_merge() {
    let facts = Facts {
        auto_merge: Some(MergeMethod::Rebase),
        failed_merges: vec!["Head branch was modified.".into()],
        ..mergeable()
    };

    assert_eq!(
        row(&facts),
        Row::Merge {
            method: MergeMethod::Rebase
        }
    );
}
