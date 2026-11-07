//! What `serve-fake` starts from: the repository, its issues, and what the
//! fake agent does in each stage.

use std::collections::BTreeMap;

use anyhow::Result;
use linewright::domain::{Login, MergeMethods, Timestamp, label};
use linewright::store::local::{LocalStore, Seed};
use serde_json::{Value, json};

use crate::scenario::{Scenario, Step};

pub const REPO: &str = "acme/widgets";
const DEFAULT_BRANCH: &str = "main";
const LINEWRIGHT: &str = "linewright-bot";
const MAINTAINER: &str = "maintainer";
const REPORTER: &str = "reporter";

/// How long the fake agent takes over each step of a triage or a design.
const BRISK_MILLIS: u64 = 1000;

/// How long it takes over each step of an implement session, which keeps it
/// in flight the longest of the seed's sessions.
const STEADY_MILLIS: u64 = 3000;

/// When the clock starts, so that the pages show the same times on every
/// run.
pub fn started_at() -> Timestamp {
    "2026-03-02T09:00:00Z"
        .parse()
        .expect("the seed's start is a timestamp")
}

pub fn repository() -> Result<Seed> {
    Ok(Seed {
        repo: REPO.parse()?,
        default_branch: DEFAULT_BRANCH.into(),
        linewright: Login::new(LINEWRIGHT),
        issue_types: None,
        linear_history: false,
        requires_up_to_date: false,
        allowed_merge_methods: MergeMethods::all(),
    })
}

/// # Errors
///
/// When the store refuses a write.
pub async fn seed(store: &LocalStore) -> Result<()> {
    store
        .commit(
            MAINTAINER,
            DEFAULT_BRANCH,
            &[("README.md", "# Widgets\n")],
            "Start",
        )
        .await?;
    store.open_issue(
        MAINTAINER,
        "Show a session's duration in its heading",
        "A session's heading should say how long it took.",
        &[label::READY_FOR_AGENT],
    );
    store.open_issue(
        REPORTER,
        "The board's lanes don't fit a phone's screen",
        "On my phone the board scrolls sideways.",
        &[label::NEEDS_TRIAGE],
    );
    Ok(())
}

/// What the fake agent does in each stage.
pub fn scenario() -> Scenario {
    Scenario {
        stages: BTreeMap::from([
            ("triage".into(), triaging()),
            ("design".into(), designing()),
            ("implement".into(), implementing()),
        ]),
        ..Scenario::default()
    }
}

fn triaging() -> Vec<Step> {
    let mut steps = session(
        BRISK_MILLIS,
        vec![
            said("I'll see whether the board can lay out its lanes on a narrow screen."),
            called(
                "t1",
                "Grep",
                &json!({"pattern": "lane", "path": "templates"}),
            ),
            returned("t1", "templates/board.html: <section class=\"lane\">"),
        ],
    );
    steps.push(finished(&json!({
        "outcome": "needs-info",
        "category": label::BUG,
        "comment": "Which phone, and which browser? The lanes stack below 640 px wide.",
        "ask": [REPORTER],
    })));
    steps
}

fn designing() -> Vec<Step> {
    let mut steps = session(
        BRISK_MILLIS,
        vec![
            said("I'll read the issue and where a session's heading is drawn."),
            called("d1", "Grep", &json!({"pattern": "heading", "path": "src"})),
            returned("d1", "src/run_page.rs: fn heading"),
            said("One ticket covers it."),
        ],
    );
    steps.push(finished(&json!({
        "outcome": "designed",
        "design": "The heading ends with the session's duration.",
        "tickets": [{
            "id": "duration",
            "title": "End a session's heading with its duration",
            "body": "Show how long the session took at the right of its heading.",
            "blocked_by": [],
        }],
        "comment": "One ticket: the change is in one place.",
    })));
    steps
}

fn implementing() -> Vec<Step> {
    let reading = [
        said("I'll read the ticket and the code the heading comes from."),
        called("i1", "Bash", &json!({"command": "git log --oneline -5"})),
        returned("i1", "c0ffee1 Start"),
        said("The heading is drawn by the run page. I'll add the duration at its right."),
        called("i2", "Read", &json!({"file_path": "src/run_page.rs"})),
        returned("i2", "fn heading(session: &Session) -> Markup { … }"),
    ];
    let testing = (1..=8).flat_map(|round| {
        let id = format!("test-{round}");
        [
            said(&format!("Round {round}: running the run page's tests.")),
            called(&id, "Bash", &json!({"command": "cargo test run_page"})),
            returned(
                &id,
                &format!("test result: ok. {} passed; 0 failed", 10 + round),
            ),
        ]
    });
    let mut steps = session(STEADY_MILLIS, reading.into_iter().chain(testing).collect());
    steps.push(Step::Commit {
        path: "src/run_page.rs".into(),
        contents: "fn heading(session: &Session) -> Markup { duration(session) }\n".into(),
        message: "End a session's heading with its duration".into(),
    });
    steps.push(finished(&json!({
        "outcome": "done",
        "pr_title": "End a session's heading with its duration",
        "pr_body": "The heading now ends with how long the session took.",
    })));
    steps
}

/// A session's start, then each of `work`'s events `pace_millis` apart.
fn session(pace_millis: u64, work: Vec<Value>) -> Vec<Step> {
    let mut steps = vec![Step::Stream(vec![json!({
        "type": "system", "subtype": "init", "model": "claude-opus-5-5",
    })])];
    for event in work {
        steps.push(Step::Sleep(pace_millis));
        steps.push(Step::Stream(vec![event]));
    }
    steps
}

/// The session's end, answering with `result`.
fn finished(result: &Value) -> Step {
    Step::Stream(vec![json!({
        "type": "result", "subtype": "success", "is_error": false,
        "result": result.to_string(),
    })])
}

fn said(text: &str) -> Value {
    json!({
        "type": "assistant",
        "message": {"content": [{"type": "text", "text": text}]},
    })
}

fn called(id: &str, tool: &str, input: &Value) -> Value {
    json!({
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "id": id, "name": tool, "input": input}]},
    })
}

fn returned(id: &str, output: &str) -> Value {
    json!({
        "type": "user",
        "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": output}]},
    })
}
