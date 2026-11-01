use linewright::domain::{MergeMethod, Requirement};
use linewright::working_pull::{Checks, Facts, GitHub, Row, row};

/// A pull request ready for review with nothing left to do but merge it:
/// green, auto-merge on and GitHub allowing the merge.
fn mergeable() -> Facts {
    Facts {
        feedback: false,
        behind: false,
        unmergeable: false,
        rebase_tried: false,
        checks: Checks::Green,
        within_grace: false,
        timed_out: false,
        fixes: 0,
        ci_attempts: 2,
        draft: false,
        auto_merge: Some(MergeMethod::Squash),
        held: false,
        failed_merges: vec![],
        github: GitHub::Allows,
        commented: false,
    }
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|&n| n.into()).collect()
}

fn blocked() -> GitHub {
    GitHub::Blocks {
        requires: vec![Requirement::Approval],
    }
}

/// Each case's facts hold its row and every later row they can, so the row
/// it expects is the first that holds. The rows are in the earlier prose's
/// order, not yet the table's.
#[test_log::test]
fn the_row_is_the_first_that_holds() {
    let failing = Checks::Failed(names(&["test"]));
    let running = Checks::Running(names(&["build"]));
    let cases: Vec<(&str, Facts, Row)> = vec![
        (
            "feedback has come",
            Facts {
                feedback: true,
                behind: true,
                unmergeable: true,
                checks: failing.clone(),
                draft: true,
                ..mergeable()
            },
            Row::Rework,
        ),
        (
            "not rebaseable, and not yet rebased at its head",
            Facts {
                behind: true,
                unmergeable: true,
                checks: failing.clone(),
                draft: true,
                ..mergeable()
            },
            Row::Rebase,
        ),
        (
            "not rebaseable and behind, though rebased at its head",
            Facts {
                behind: true,
                unmergeable: true,
                rebase_tried: true,
                checks: failing.clone(),
                ..mergeable()
            },
            Row::Rebase,
        ),
        (
            "behind",
            Facts {
                behind: true,
                checks: failing.clone(),
                draft: true,
                ..mergeable()
            },
            Row::Update,
        ),
        (
            "no checks, within the grace period",
            Facts {
                unmergeable: true,
                rebase_tried: true,
                checks: Checks::None,
                within_grace: true,
                timed_out: true,
                draft: true,
                ..mergeable()
            },
            Row::WaitForChecks,
        ),
        (
            "a check still running past the timeout",
            Facts {
                checks: running.clone(),
                within_grace: true,
                timed_out: true,
                draft: true,
                ..mergeable()
            },
            Row::HandBackTimedOut {
                running: names(&["build"]),
            },
        ),
        (
            "a check still running",
            Facts {
                checks: running,
                within_grace: true,
                draft: true,
                ..mergeable()
            },
            Row::WaitForChecks,
        ),
        (
            "a check failed, with fix attempts left",
            Facts {
                checks: failing.clone(),
                within_grace: true,
                timed_out: true,
                fixes: 1,
                draft: true,
                ..mergeable()
            },
            Row::FixCi {
                failing: names(&["test"]),
            },
        ),
        (
            "a check failed, with no fix attempts left",
            Facts {
                checks: failing,
                fixes: 2,
                draft: true,
                ..mergeable()
            },
            Row::HandBackFailing {
                failing: names(&["test"]),
                attempts: 2,
            },
        ),
        (
            "a green draft, auto-merge off",
            Facts {
                draft: true,
                auto_merge: None,
                failed_merges: names(&["conflict", "conflict"]),
                ..mergeable()
            },
            Row::MarkReady,
        ),
        (
            "a green draft a person holds",
            Facts {
                draft: true,
                held: true,
                ..mergeable()
            },
            Row::MarkReady,
        ),
        (
            "ready for review, auto-merge off",
            Facts {
                auto_merge: None,
                failed_merges: names(&["conflict", "conflict"]),
                ..mergeable()
            },
            Row::WaitForReview,
        ),
        (
            "ready for review, a person holds it",
            Facts {
                held: true,
                github: blocked(),
                ..mergeable()
            },
            Row::WaitForReview,
        ),
        (
            "two merges failed",
            Facts {
                failed_merges: names(&["first", "second"]),
                draft: true,
                ..mergeable()
            },
            Row::HandBackMergeFailed {
                reason: "second".into(),
            },
        ),
        (
            "GitHub allows it, a draft",
            Facts {
                failed_merges: names(&["first"]),
                draft: true,
                ..mergeable()
            },
            Row::Merge {
                draft: true,
                method: MergeMethod::Squash,
            },
        ),
        (
            "GitHub allows it",
            mergeable(),
            Row::Merge {
                draft: false,
                method: MergeMethod::Squash,
            },
        ),
        (
            "GitHub reports it not rebaseable, though rebased",
            Facts {
                unmergeable: true,
                rebase_tried: true,
                github: GitHub::NotRebaseable,
                ..mergeable()
            },
            Row::HandBackNotRebaseable,
        ),
        (
            "GitHub blocks it, and not commented on",
            Facts {
                github: blocked(),
                ..mergeable()
            },
            Row::CommentRequirements {
                draft: false,
                requires: vec![Requirement::Approval],
            },
        ),
        (
            "GitHub blocks a draft, though commented on",
            Facts {
                github: blocked(),
                draft: true,
                commented: true,
                ..mergeable()
            },
            Row::CommentRequirements {
                draft: true,
                requires: vec![Requirement::Approval],
            },
        ),
        (
            "GitHub blocks it, commented on",
            Facts {
                github: blocked(),
                commented: true,
                ..mergeable()
            },
            Row::WaitForRequirements {
                requires: vec![Requirement::Approval],
            },
        ),
        (
            "GitHub has not reported, a draft",
            Facts {
                github: GitHub::NotReported,
                draft: true,
                ..mergeable()
            },
            Row::MarkReadyForMergeState,
        ),
        (
            "GitHub has not reported whether a draft is rebaseable",
            Facts {
                github: GitHub::RebaseNotReported,
                draft: true,
                ..mergeable()
            },
            Row::WaitForMergeState,
        ),
        (
            "GitHub has not reported",
            Facts {
                github: GitHub::NotReported,
                ..mergeable()
            },
            Row::WaitForMergeState,
        ),
    ];
    for (case, facts, expected) in cases {
        assert_eq!(row(&facts), expected, "{case}");
    }
}
