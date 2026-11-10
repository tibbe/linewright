//! What Linewright fills into each first-message prompt's placeholders, byte
//! for byte, against the text in `fixtures/prompts/`. The prompts are a layer
//! holding only their placeholders, so the shipped prompts' wording plays no
//! part (docs/specs/runs.md, "Contract"). Nor does the wording of a part with
//! no placeholders of its own in `briefs/parts/` or `briefs/outputs/`: the
//! fixtures name such a part, `<<briefs/parts/cut-issue.md>>`, rather than
//! copy it, and where several such parts hold the same text, they name each
//! of them, `<<briefs/outputs/fix-ci.md or briefs/outputs/rework.md>>`.

use std::borrow::Cow;

use chrono::{DateTime, Utc};

use crate::common;
use linewright::domain::*;
use linewright::git::{BranchStart, Conflict, UnfinishedCommits};
use linewright::prompts::{self, PullTask, Repo, SpecChangeAndDiff, Ticket};
use linewright::working_pull::Feedback;

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

/// The file at `path` from the repository root.
#[track_caller]
fn read(path: &str) -> String {
    let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Each text of the parts in `briefs/parts/` and `briefs/outputs/` with no
/// placeholders, as Linewright builds it in, with its end trimmed, and the
/// marker naming the parts that hold it, the longest text first.
#[track_caller]
fn placeholder_free_parts() -> Vec<(String, String)> {
    let mut by_text = std::collections::BTreeMap::<String, Vec<String>>::new();
    for dir in ["briefs/parts", "briefs/outputs"] {
        let entries = std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir))
            .unwrap_or_else(|e| panic!("{dir}: {e}"))
            .map(|entry| entry.unwrap().file_name().into_string().unwrap());
        for file in entries {
            let path = format!("{dir}/{file}");
            let text = read(&path);
            if !text.contains("{{") {
                by_text
                    .entry(text.trim_end().to_string())
                    .or_default()
                    .push(path);
            }
        }
    }
    let mut parts: Vec<_> = by_text
        .into_iter()
        .map(|(text, mut paths)| {
            paths.sort();
            (text, format!("<<{}>>", paths.join(" or ")))
        })
        .collect();
    parts.sort_by_key(|(text, _)| std::cmp::Reverse(text.len()));
    parts
}

/// Fails unless `prompt`, with each placeholder-free part's text replaced by
/// its marker, is the fixture `name` byte for byte.
#[track_caller]
fn assert_filled_as(prompt: &str, name: &str) {
    let mut named = prompt.to_string();
    for (text, marker) in placeholder_free_parts() {
        named = named.replace(&text, &marker);
    }
    let expected = read(&format!("tests/integration/fixtures/prompts/{name}"));
    if named != expected {
        let got: Vec<_> = named.split('\n').collect();
        let want: Vec<_> = expected.split('\n').collect();
        let line = (0..got.len().max(want.len()))
            .find(|&i| got.get(i) != want.get(i))
            .unwrap();
        let (got, want) = (got.get(line), want.get(line));
        panic!(
            "the prompt differs from {name} first on line {}:\n  \
             prompt:  {got:?}\n  fixture: {want:?}\n\n\
             the prompt, its placeholder-free parts named:\n{named}",
            line + 1,
        );
    }
}

fn unfinished() -> UnfinishedCommits {
    UnfinishedCommits {
        first: "abc1234".into(),
        rest: vec![],
    }
}

#[test_log::test]
fn triage_is_filled_as_before() {
    let name = repo_name();
    let prompt = prompts::triage(
        &common::placeholders_only().prompts,
        &repo(&name),
        &issue(7, "Count the widgets", "They are not counted."),
        &["bug".into(), "enhancement".into()],
        std::path::Path::new(ATTACHMENTS),
    );

    assert_filled_as(&prompt, "triage.txt");
}

#[test_log::test]
fn design_of_an_issue_is_filled_as_before() {
    let name = repo_name();
    let prompt = prompts::design_issue(
        &common::placeholders_only().prompts,
        &repo(&name),
        &issue(7, "Count the widgets", "They are not counted."),
        None,
        &[],
        std::path::Path::new(ATTACHMENTS),
    );

    assert_filled_as(&prompt, "design-issue.txt");
}

#[test_log::test]
fn design_of_a_spec_changes_issue_reads_the_spec_change_and_its_diff() {
    let name = repo_name();
    let spec_change = MergedPull {
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
    let its_issue = Issue {
        author: Login::new("factory"),
        labels: vec!["ready-for-agent".into()],
        filed: Some(Filed::SpecChange {
            spec_change: Number(5),
        }),
        ..issue(
            7,
            "Spec change: Spec the widget count",
            "Opened for the spec change in #5.",
        )
    };
    let gap = issue(9, "Which widgets count?", "Say which.");
    let prompt = prompts::design_issue(
        &common::placeholders_only().prompts,
        &repo(&name),
        &its_issue,
        Some(&SpecChangeAndDiff {
            spec_change,
            diff: "+Widgets are counted.\n".into(),
        }),
        &[&gap],
        std::path::Path::new(ATTACHMENTS),
    );

    assert_filled_as(&prompt, "design-spec-change-issue.txt");
}

#[test_log::test]
fn implement_is_filled_as_before() {
    let name = repo_name();
    let ticket_issue = issue(7, "Count the widgets", "Count them.");
    let parent = issue(4, "Widgets", "The design.");
    let ticket = common::ready(Ticket::read(
        &ticket_issue,
        Some(Cow::Borrowed(&parent)),
        vec![],
    ));
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

    assert_filled_as(&prompt, "implement.txt");
}

#[test_log::test]
fn rework_is_filled_as_before() {
    let name = repo_name();
    let ticket_issue = issue(7, "Count the widgets", "Count them.");
    let ticket = common::ready(Ticket::read(&ticket_issue, None, vec![]));
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
            history: prompts::History::Series,
        },
        Some(&unfinished()),
        std::path::Path::new(ATTACHMENTS),
    );

    assert_filled_as(&prompt, "rework.txt");
}

#[test_log::test]
fn fix_ci_is_filled_as_before() {
    let name = repo_name();
    let ticket_issue = issue(7, "Count the widgets", "Count them.");
    let ticket = common::ready(Ticket::read(&ticket_issue, None, vec![]));
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
            history: prompts::History::Series,
        },
        None,
        std::path::Path::new(ATTACHMENTS),
    );

    assert_filled_as(&prompt, "fix-ci.txt");
}

#[test_log::test]
fn update_is_filled_as_before() {
    let name = repo_name();
    let ticket_issue = issue(7, "Count the widgets", "Count them.");
    let ticket = common::ready(Ticket::read(&ticket_issue, None, vec![]));
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

    assert_filled_as(&prompt, "update.txt");
}

#[test_log::test]
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

    assert_filled_as(&prompt, "automation.txt");
}

#[test_log::test]
fn implement_of_a_ticket_cut_from_a_spec_changes_issue_reads_the_spec_change_and_its_diff() {
    let name = repo_name();
    let ticket_issue = issue(7, "Count the widgets", "Count them.");
    let tracking = Issue {
        filed: Some(Filed::SpecChange {
            spec_change: Number(5),
        }),
        ..issue(
            4,
            "Spec change: Count widgets",
            "Opened for the spec change in #5.",
        )
    };
    let ticket = common::with_spec_diff(
        Ticket::read(&ticket_issue, Some(Cow::Borrowed(&tracking)), vec![]),
        "+Widgets are counted.\n",
    );
    let prompt = prompts::implement(
        &common::placeholders_only().prompts,
        &repo(&name),
        &ticket,
        "7-count-the-widgets",
        &BranchStart::New,
        std::path::Path::new(ATTACHMENTS),
    );

    assert_filled_as(&prompt, "implement-spec-change-ticket.txt");
}

#[test_log::test]
fn implement_of_a_spec_changes_issue_built_as_it_is_reads_the_spec_change_and_its_diff() {
    let name = repo_name();
    let its_issue = Issue {
        filed: Some(Filed::SpecChange {
            spec_change: Number(5),
        }),
        ..issue(
            7,
            "Spec change: Count widgets",
            "Opened for the spec change in #5.",
        )
    };
    let ticket = common::with_spec_diff(
        Ticket::read(&its_issue, None, vec![]),
        "+Widgets are counted.\n",
    );
    let prompt = prompts::implement(
        &common::placeholders_only().prompts,
        &repo(&name),
        &ticket,
        "7-spec-change-count-widgets",
        &BranchStart::New,
        std::path::Path::new(ATTACHMENTS),
    );

    assert_filled_as(&prompt, "implement-spec-change-issue.txt");
}

#[test_log::test]
fn rework_of_a_ticket_with_a_parent_is_filled_as_before() {
    let name = repo_name();
    let ticket_issue = issue(7, "Count the widgets", "Count them.");
    let parent = Issue {
        comments: vec![Comment {
            author: Login::new("designer"),
            body: "Store them in one table.".into(),
            created_at: created(),
        }],
        ..issue(4, "Widgets", "All of them.")
    };
    let ticket = common::ready(Ticket::read(
        &ticket_issue,
        Some(Cow::Borrowed(&parent)),
        vec![],
    ));
    let comment = Comment {
        author: Login::new("maintainer"),
        body: "Name it `count`.".into(),
        created_at: created(),
    };
    let prompt = prompts::pull_work(
        &common::placeholders_only().prompts,
        &repo(&name),
        &ticket,
        &pull(),
        &PullTask::Rework {
            feedback: Feedback {
                comments: vec![&comment],
                reviews: vec![],
                threads: vec![],
            },
            conflict: None,
            history: prompts::History::OnTop,
        },
        None,
        std::path::Path::new(ATTACHMENTS),
    );

    assert_filled_as(&prompt, "rework-of-a-ticket-with-a-parent.txt");
}

#[test_log::test]
fn the_history_is_a_series_for_merge_and_rebase_and_on_top_for_squash() {
    for (method, history) in [
        (MergeMethod::Merge, prompts::History::Series),
        (MergeMethod::Rebase, prompts::History::Series),
        (MergeMethod::Squash, prompts::History::OnTop),
    ] {
        assert_eq!(prompts::History::for_method(method), history, "{method}");
    }
}
