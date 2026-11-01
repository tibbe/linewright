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
