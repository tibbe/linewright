//! The fix session's brief on its data model: the failing checks the store
//! returns in, the brief's section on each out (`CODING_STANDARDS.md`, "Where
//! tests go"; `docs/specs/pull_requests.md`, CI).

use chrono::{DateTime, Utc};
use linewright::domain::*;
use linewright::prompts::{self, PullTask, Repo, Ticket};

fn issue() -> Issue {
    Issue {
        number: Number(7),
        title: "Count the widgets".into(),
        body: String::new(),
        author: Login::new("reporter"),
        open: true,
        created_at: created(),
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

fn pull() -> Pull {
    Pull {
        number: Number(8),
        title: "Count the widgets".into(),
        body: "Closes #7".into(),
        author: Login::new("factory"),
        created_at: created(),
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
    }
}

fn created() -> Timestamp {
    DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

/// The brief's section on the failing check `name`: from its heading to the
/// next heading.
fn section(failures: &[CheckFailure], name: &str) -> String {
    let issue = issue();
    let ticket = Ticket {
        issue: &issue,
        filed_gaps: vec![],
    };
    let repo = Repo {
        default_branch: "main",
        spec_glob: "docs/specs/**/*.md",
    };
    let brief = prompts::pull_work(&repo, &ticket, &pull(), &PullTask::FixCi(failures), None);
    let heading = format!("#### `{name}`\n");
    let start = brief.find(&heading).expect(&brief);
    let rest = &brief[start + heading.len()..];
    let end = rest.find("\n#").expect(rest);
    format!("{heading}{}", &rest[..=end])
}

#[test]
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
        section(&failures, "test"),
        "#### `test`\n\
         \n\
         Process completed with exit code 101.\n\
         \n\
         ```\n\
         assertion failed: widgets == 2\n\
         ```\n\
         \n\
         The full log: `gh run view --job 52896412311 --repo tibbe/linewright --log`\n\
         \n"
    );
}

#[test]
fn a_failing_commit_status_gives_its_description_and_link_without_a_log() {
    let failures = [CheckFailure {
        name: "ci/circleci".into(),
        summary: "Your tests failed (https://circleci.com/gh/tibbe/linewright/12)".into(),
        log: None,
    }];

    assert_eq!(
        section(&failures, "ci/circleci"),
        "#### `ci/circleci`\n\
         \n\
         Your tests failed (https://circleci.com/gh/tibbe/linewright/12)\n\
         \n"
    );
}

#[test]
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
        section(&failures, "test"),
        "#### `test`\n\
         \n\
         _(empty)_\n\
         \n\
         ```\n\
         assertion failed: widgets == 2\n\
         ```\n\
         \n"
    );
}
