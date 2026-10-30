//! What Linewright fills into each first-message prompt's placeholders, byte
//! for byte, against the text in `fixtures/prompts/`. The prompts are a layer
//! holding only their placeholders, so the shipped prompts' wording plays no
//! part (docs/specs/runs.md, "Contract").

use std::borrow::Cow;

use chrono::{DateTime, Utc};

use crate::common;
use linewright::domain::*;
use linewright::git::{BranchStart, Conflict, UnfinishedCommits};
use linewright::prompts::{self, PullTask, Repo, Ticket};
use linewright::reconcile::Feedback;

fn created() -> Timestamp {
    DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn issue(number: u64, title: &str, body: &str) -> Issue {
    Issue {
        body: body.into(),
        author: Login::new("reporter"),
        labels: vec!["bug".into()],
        comments: vec![Comment {
            author: Login::new("maintainer"),
            body: "Seen it too.".into(),
            created_at: created(),
        }],
        ..common::ticket(number, title)
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
        comments: vec![Comment {
            author: Login::new("maintainer"),
            body: "Please name the count.".into(),
            created_at: created(),
        }],
        reviews: vec![],
        threads: vec![],
        labels: vec![],
        assignees: vec![Login::new("factory")],
        labelings: vec![],
        assignments: vec![],
        review_decision: None,
    }
}

/// The session's attachments directory each prompt names.
const ATTACHMENTS: &str = "/work/sessions/issue-7/20261001T120000Z-implement/attachments";

fn repo_name() -> linewright::domain::Repo {
    "owner/widgets".parse().unwrap()
}

fn repo(name: &linewright::domain::Repo) -> Repo<'_> {
    Repo {
        name,
        default_branch: "main",
        spec_glob: "docs/specs/**/*.md",
    }
}

fn expected(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/integration/fixtures/prompts")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn unfinished() -> UnfinishedCommits {
    UnfinishedCommits {
        first: "abc1234".into(),
        rest: vec![],
    }
}

#[test]
fn triage_is_filled_as_before() {
    let name = repo_name();
    let prompt = prompts::triage(
        &common::placeholders_only().prompts,
        &repo(&name),
        &issue(7, "Count the widgets", "They are not counted."),
        &["bug".into(), "enhancement".into()],
        std::path::Path::new(ATTACHMENTS),
    );

    assert_eq!(prompt, expected("triage.txt"));
}

#[test]
fn design_of_a_spec_change_is_filled_as_before() {
    let name = repo_name();
    let pr = MergedPull {
        number: Number(5),
        title: "Spec the widget count".into(),
        body: "Widgets are counted.".into(),
        author: Login::new("maintainer"),
        merged_at: created(),
        updated_at: created(),
        base_ref: "main".into(),
        labels: vec![],
        closes: vec![],
        files: vec!["docs/specs/widgets.md".into()],
        assignees: vec![],
        events: vec![],
    };
    let gap = issue(9, "Which widgets count?", "Say which.");
    let prompt = prompts::design_spec_change(
        &common::placeholders_only().prompts,
        &repo(&name),
        &pr,
        "+Widgets are counted.\n",
        &[&gap],
        std::path::Path::new(ATTACHMENTS),
    );

    assert_eq!(prompt, expected("design-spec-change.txt"));
}

#[test]
fn design_of_an_issue_is_filled_as_before() {
    let name = repo_name();
    let prompt = prompts::design_issue(
        &common::placeholders_only().prompts,
        &repo(&name),
        &issue(7, "Count the widgets", "They are not counted."),
        &[],
        std::path::Path::new(ATTACHMENTS),
    );

    assert_eq!(prompt, expected("design-issue.txt"));
}

#[test]
fn implement_is_filled_as_before() {
    let name = repo_name();
    let ticket_issue = issue(7, "Count the widgets", "Count them.");
    let parent = issue(4, "Widgets", "The design.");
    let ticket = Ticket::new(&ticket_issue, Some(Cow::Borrowed(&parent)), vec![], None);
    let prompt = prompts::implement(
        &common::placeholders_only().prompts,
        &repo(&name),
        &ticket,
        "7-count-the-widgets",
        &BranchStart::Existing {
            unfinished: Some(unfinished()),
        },
        std::path::Path::new(ATTACHMENTS),
    );

    assert_eq!(prompt, expected("implement.txt"));
}

#[test]
fn rework_is_filled_as_before() {
    let name = repo_name();
    let ticket_issue = issue(7, "Count the widgets", "Count them.");
    let ticket = Ticket::new(&ticket_issue, None, vec![], None);
    let pr = pull();
    let comment = Comment {
        author: Login::new("maintainer"),
        body: "Name it `count`.".into(),
        created_at: created(),
    };
    let conflict = Conflict {
        method: UpdateMethod::Rebase,
        files: vec!["src/count.rs".into()],
    };
    let prompt = prompts::pull_work(
        &common::placeholders_only().prompts,
        &repo(&name),
        &ticket,
        &pr,
        &PullTask::Rework {
            feedback: Feedback {
                comments: vec![&comment],
                reviews: vec![],
                threads: vec![],
            },
            conflict: Some(&conflict),
        },
        Some(&unfinished()),
        std::path::Path::new(ATTACHMENTS),
    );

    assert_eq!(prompt, expected("rework.txt"));
}

#[test]
fn fix_ci_is_filled_as_before() {
    let name = repo_name();
    let ticket_issue = issue(7, "Count the widgets", "Count them.");
    let ticket = Ticket::new(&ticket_issue, None, vec![], None);
    let failures = [CheckFailure {
        name: "test".into(),
        summary: "1 test failed".into(),
        log: None,
    }];
    let prompt = prompts::pull_work(
        &common::placeholders_only().prompts,
        &repo(&name),
        &ticket,
        &pull(),
        &PullTask::FixCi {
            failures: &failures,
            conflict: None,
        },
        None,
        std::path::Path::new(ATTACHMENTS),
    );

    assert_eq!(prompt, expected("fix-ci.txt"));
}

#[test]
fn update_is_filled_as_before() {
    let name = repo_name();
    let ticket_issue = issue(7, "Count the widgets", "Count them.");
    let ticket = Ticket::new(&ticket_issue, None, vec![], None);
    let conflict = Conflict {
        method: UpdateMethod::Merge,
        files: vec!["src/count.rs".into(), "Cargo.lock".into()],
    };
    let prompt = prompts::pull_work(
        &common::placeholders_only().prompts,
        &repo(&name),
        &ticket,
        &pull(),
        &PullTask::Update(&conflict),
        None,
        std::path::Path::new(ATTACHMENTS),
    );

    assert_eq!(prompt, expected("update.txt"));
}

#[test]
fn automation_is_filled_as_before() {
    let name = repo_name();
    let automation = linewright::automation::parse(
        &"weekly-bugs".parse().unwrap(),
        "---\non:\n  schedule: \"0 9 * * 1\"\nmax-issues: 2\n---\nLook at the bugs fixed in the past week.\n",
    )
    .unwrap();
    let prompt = prompts::automation(
        &common::placeholders_only().prompts,
        &repo(&name),
        &automation,
        std::path::Path::new(ATTACHMENTS),
    );

    assert_eq!(prompt, expected("automation.txt"));
}
