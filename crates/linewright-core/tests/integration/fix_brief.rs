use chrono::{DateTime, Utc};
use linewright_core::domain::*;
use linewright_core::prompts::{self, PullTask, Repo, Ticket};
use linewright_core::testing;

fn issue() -> Issue {
    Issue {
        number: Number(7),
        title: "Count the widgets".into(),
        body: String::new(),
        author: Login::new("reporter"),
        open: true,
        created_at: created(),
        updated_at: created(),
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

fn pull() -> Pull {
    Pull {
        number: Number(8),
        title: "Count the widgets".into(),
        body: "Closes #7".into(),
        author: Login::new("factory"),
        created_at: created(),
        updated_at: created(),
        base_ref: "main".into(),
        head_ref: "7-count-the-widgets".into(),
        head_sha: "abc123".into(),
        state: PullState::Open { draft: true },
        closes: vec![Number(7)],
        pushes: vec![],
        mergeability: Mergeability::Clean,
        rebaseability: Rebaseability::Rebaseable,
        checks: vec![],
        comments: vec![],
        reviews: vec![],
        threads: vec![],
        labels: vec![],
        assignees: vec![Login::new("factory")],
        labelings: vec![],
        assignments: vec![],
        review_decision: None,
        review_requests: vec![],
    }
}

fn created() -> Timestamp {
    DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

/// What a fix-CI brief for `failures` fills `failures` with.
fn filled_failures(failures: &[CheckFailure]) -> String {
    let issue = issue();
    let ticket = testing::ready(Ticket::read(&issue, None, vec![]));
    let repo_name = "owner/widgets".parse().unwrap();
    let repo = Repo {
        name: &repo_name,
        default_branch: "main",
        spec_glob: "docs/specs/**/*.md",
    };
    let brief = prompts::pull_work(
        &testing::marker_briefs().templates,
        &repo,
        &ticket,
        &pull(),
        &PullTask::FixCi {
            failures,
            conflict: None,
            history: prompts::History::Series,
        },
        None,
        std::path::Path::new("/attachments"),
    );
    testing::filled_in(&brief, "failures").to_string()
}

#[test_log::test]
fn a_failing_actions_check_gives_its_report_its_output_and_the_command_for_its_full_log() {
    let failures = [CheckFailure {
        name: "test".into(),
        summary: "Process completed with exit code 101.".into(),
        log: Some(CheckLog {
            step_output: "assertion failed: widgets == 2\n".into(),
            full: Some(JobLog {
                repo: "tibbe/linewright".into(),
                job: 52_896_412_311,
            }),
        }),
    }];

    assert_eq!(
        filled_failures(&failures),
        "#### `test`\n\
         \n\
         Process completed with exit code 101.\n\
         \n\
         ```\n\
         assertion failed: widgets == 2\n\
         ```\n\
         \n\
         [parts/full-log.md job=«52896412311» repo=«tibbe/linewright»]"
    );
}

#[test_log::test]
fn a_failing_commit_status_gives_its_description_and_link_without_a_log() {
    let failures = [CheckFailure {
        name: "ci/circleci".into(),
        summary: "Your tests failed (https://circleci.com/gh/tibbe/linewright/12)".into(),
        log: None,
    }];

    assert_eq!(
        filled_failures(&failures),
        "#### `ci/circleci`\n\
         \n\
         Your tests failed (https://circleci.com/gh/tibbe/linewright/12)"
    );
}

#[test_log::test]
fn a_check_whose_full_log_cannot_be_read_gives_its_output_without_a_pointer() {
    let failures = [CheckFailure {
        name: "test".into(),
        summary: String::new(),
        log: Some(CheckLog {
            step_output: "assertion failed: widgets == 2".into(),
            full: None,
        }),
    }];

    assert_eq!(
        filled_failures(&failures),
        "#### `test`\n\
         \n\
         _(empty)_\n\
         \n\
         ```\n\
         assertion failed: widgets == 2\n\
         ```"
    );
}
