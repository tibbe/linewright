//! What `serve-fake` starts from: the repository, its issues, and what the
//! fake agent does in each stage.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use linewright::benchmarks::{
    Experiments, JudgementFigures, ReplayFigures, SuiteName, Suites, TrialChange, TrialRef,
};
use linewright::case::{self, Criteria, ReplayStage, SuiteChoice};
use linewright::cost::Usd;
use linewright::database::Database;
use linewright::domain::{Login, MergeMethods, Number, Timestamp, label};
use linewright::input::{self, Wanted};
use linewright::replay::Configuration;
use linewright::results::Verdict;
use linewright::runs::{Ending, OutcomeName, RunId, Runs};
use linewright::store::local::{LocalStore, Seed};
use serde_json::{Value, json};

use crate::scenario::{Scenario, Step};

pub const REPO: &str = "acme/widgets";
const DEFAULT_BRANCH: &str = "main";
const LINEWRIGHT: &str = "linewright-bot";
const MAINTAINER: &str = "maintainer";
const REPORTER: &str = "reporter";
const DESIGNER: &str = "designer";

/// How long the fake agent takes over each step of a triage or a design.
const BRISK_MILLIS: u64 = 1000;

/// How long it takes over each step of an implement session, which keeps it
/// in flight the longest of the seed's sessions.
const STEADY_MILLIS: u64 = 3000;

/// The repository's settings: a setup command.
const SETTINGS: &str = "[setup]\ncommand = \"echo ready\"\n";

/// An automation that runs on Mondays at 06:00, which the clock starts
/// after.
const WEEKLY: &str = "---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook around.\n";

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
            &[
                ("README.md", "# Widgets\n"),
                (".linewright/config.toml", SETTINGS),
                (".linewright/automations/weekly.md", WEEKLY),
                (
                    ".linewright/briefs/plugin/skills/triage/SKILL.md",
                    "# Triage\n\nTriage the widgets' issues.\n",
                ),
            ],
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
    let done = store.open_issue(
        MAINTAINER,
        "Name the board's columns",
        "Each column should say which stage it holds.",
        &[],
    );
    store.label_as(LINEWRIGHT, done, label::READY_FOR_AGENT)?;
    let tracking = store.open_issue(
        MAINTAINER,
        "Let the board be read on a phone",
        "The board should fit a phone's screen.",
        &[],
    );
    store.label_as(LINEWRIGHT, tracking, label::DESIGNED)?;
    store.assign_as(MAINTAINER, tracking, DESIGNER)?;
    for title in ["Stack the board's lanes", "Shorten the cards' lines"] {
        let ticket = store.open_issue(LINEWRIGHT, title, "", &[label::NEEDS_INFO]);
        store.add_sub_issue_as(LINEWRIGHT, tracking, ticket)?;
    }
    let taken_back = store.open_issue(
        REPORTER,
        "Keep the board's filter across reloads",
        "The board forgets who I narrowed it to.",
        &[label::READY_FOR_AGENT],
    );
    store.assign_as(LINEWRIGHT, taken_back, LINEWRIGHT)?;
    store.unassign_as(MAINTAINER, taken_back, LINEWRIGHT)?;
    store.unlabel_as(taken_back, label::READY_FOR_AGENT)?;
    store.label_as(MAINTAINER, taken_back, label::READY_FOR_HUMAN)?;
    Ok(())
}

/// The issue whose runs the seed's suite holds cases of: the one closed as
/// done.
const CASES_ISSUE: Number = Number(3);

/// Closes as done the issue the seed's cases are captured from. Its runs'
/// input is recorded as it was before, open, which a replay needs.
///
/// # Errors
///
/// When the store refuses the write.
pub fn close_done(store: &LocalStore) -> Result<()> {
    store.close_issue_as_completed(CASES_ISSUE)
}

/// The suite of design cases.
const BOARD: &str = "board";

/// A case of [`BOARD`], and the design run of [`CASES_ISSUE`] it is
/// captured from.
struct SeedCase {
    name: &'static str,
    /// The run's number.
    n: u32,
    /// When the run started, the day before the clock starts.
    started_at: &'static str,
    /// How the run ended, twenty minutes after it started.
    ending: &'static str,
    criteria: &'static str,
}

const BOARD_CASES: [SeedCase; 2] = [
    SeedCase {
        name: "columns-first-try",
        n: 1,
        started_at: "2026-03-01T10:00:00Z",
        ending: "handed-back",
        criteria: "A good design names each column of the board after its stage.\n\nIt keeps the names short enough for a narrow column.\n",
    },
    SeedCase {
        name: "columns-named",
        n: 2,
        started_at: "2026-03-01T14:00:00Z",
        ending: "done",
        criteria: "A good design cuts the work into one ticket, with a test that reads the names off the board.\n",
    },
];

/// A suite with no cases yet.
const EMPTY_SUITE: &str = "triage-basics";

/// Writes into the work directory `work_dir` the suites the web UI shows: a
/// suite of design cases captured from #3's runs, which the seed records
/// as ended, with an experiment on it that has ended, and an empty suite of
/// triage. A work directory that holds the
/// first of those runs already is left as it is.
///
/// # Errors
///
/// When the database, the store or a capture fails.
pub async fn suites(store: &LocalStore, work_dir: &Path) -> Result<()> {
    let db = Database::open(work_dir).await?;
    let runs = Runs::new(db.clone());
    let suites = Suites::new(db);
    let first = run_of(&BOARD_CASES[0])?;
    if runs.try_read_run(&first).await?.is_some() {
        return Ok(());
    }
    let wanted = Wanted {
        items: BTreeSet::from([CASES_ISSUE]),
        branches: vec![DEFAULT_BRANCH.into()],
    };
    for case in &BOARD_CASES {
        let start: Timestamp = case.started_at.parse()?;
        let input = input::record(store, &wanted, start).await?;
        record_run(work_dir, case, start, &serde_json::to_string(&input)?).await?;
    }
    let board: SuiteName = BOARD.parse().map_err(anyhow::Error::msg)?;
    suites.make(&board, ReplayStage::Design).await?;
    for case in &BOARD_CASES {
        let capturable = case::capturable(
            &runs,
            &suites,
            &run_of(case)?,
            &case.name.parse().map_err(anyhow::Error::msg)?,
            &SuiteChoice::Named(board.clone()),
        )
        .await
        .map_err(|e| anyhow::anyhow!(e.why()))?;
        let criteria = Criteria::parse(case.criteria).context("the seed's criteria are blank")?;
        case::capture(store, capturable, &criteria, &suites)
            .await
            .map_err(|e| anyhow::anyhow!(e.why()))?;
    }
    let empty: SuiteName = EMPTY_SUITE.parse().map_err(anyhow::Error::msg)?;
    suites.make(&empty, ReplayStage::Triage).await?;
    experiment(&Experiments::new(runs.database().clone()), &board).await
}

/// How each trial of the seed's experiment ended, in start order: by
/// repetition, then case, then configuration.
const TRIALS: [SeedTrial; 8] = [
    SeedTrial::Judged {
        ending: Ending::Done,
        money: 0.42,
        seconds: 95,
        verdict: Verdict::Pass,
        leak: false,
    },
    SeedTrial::Judged {
        ending: Ending::Done,
        money: 0.21,
        seconds: 61,
        verdict: Verdict::Fail,
        leak: true,
    },
    SeedTrial::Unjudged {
        ending: Ending::Failed,
        money: 0.05,
        seconds: 12,
    },
    SeedTrial::Judged {
        ending: Ending::HandedBack,
        money: 0.18,
        seconds: 48,
        verdict: Verdict::Pass,
        leak: false,
    },
    SeedTrial::Judged {
        ending: Ending::Done,
        money: 0.39,
        seconds: 88,
        verdict: Verdict::Fail,
        leak: false,
    },
    SeedTrial::JudgeMadeNone {
        money: 0.24,
        seconds: 70,
    },
    SeedTrial::Judged {
        ending: Ending::Done,
        money: 0.47,
        seconds: 102,
        verdict: Verdict::Pass,
        leak: false,
    },
    SeedTrial::Judged {
        ending: Ending::Done,
        money: 0.2,
        seconds: 55,
        verdict: Verdict::Pass,
        leak: false,
    },
];

/// How a trial of the seed's experiment ended.
enum SeedTrial {
    Judged {
        ending: Ending,
        money: f64,
        seconds: u64,
        verdict: Verdict,
        leak: bool,
    },
    /// Its run neither done nor handed back.
    Unjudged {
        ending: Ending,
        money: f64,
        seconds: u64,
    },
    /// Its run done, and its judge made no judgement.
    JudgeMadeNone { money: f64, seconds: u64 },
}

/// Launches on `board` an experiment the day before the clock starts, under
/// `as-is` and `terse`, which sets a low effort, twice, and ends each of its
/// trials as [`TRIALS`] says, twenty minutes after its launch: with passes,
/// failures, a leak and replays without a judgement.
async fn experiment(experiments: &Experiments, board: &SuiteName) -> Result<()> {
    let terse = Configuration::parse(BTreeMap::from([(
        "config.toml".to_string(),
        b"effort = \"low\"\n".to_vec(),
    )]))
    .map_err(|errors| anyhow::anyhow!("the seed's configuration: {errors:?}"))?;
    let launched_at: Timestamp = "2026-03-01T16:00:00Z".parse()?;
    let launched = experiments
        .launch(
            board,
            vec![("terse".parse().map_err(anyhow::Error::msg)?, terse)],
            NonZeroU32::new(2).context("two repetitions")?,
            launched_at,
        )
        .await?;
    let record = experiments
        .show(launched.id)
        .await?
        .context("the seed's experiment is kept")?;
    let at = launched_at + chrono::TimeDelta::minutes(20);
    let usd = |money: f64| Usd::new(money).context("a cost is a number of dollars");
    for (key, seeded) in record.trials.keys().zip(&TRIALS) {
        let trial = TrialRef {
            experiment: launched.id,
            key: *key,
        };
        let (ending, money, seconds) = match *seeded {
            SeedTrial::Judged {
                ending,
                money,
                seconds,
                ..
            }
            | SeedTrial::Unjudged {
                ending,
                money,
                seconds,
            } => (ending, money, seconds),
            SeedTrial::JudgeMadeNone { money, seconds } => (Ending::Done, money, seconds),
        };
        experiments
            .step(&trial, TrialChange::ReplayStarts, at)
            .await?;
        experiments
            .step(
                &trial,
                TrialChange::ReplayEnds(ReplayFigures {
                    outcome: OutcomeName::Ended(ending),
                    money: Some(usd(money)?),
                    time: Duration::from_secs(seconds),
                }),
                at,
            )
            .await?;
        let judged = match *seeded {
            SeedTrial::Unjudged { .. } => continue,
            SeedTrial::Judged { verdict, leak, .. } => TrialChange::Judged(JudgementFigures {
                correctness: verdict,
                leak,
                judge_money: Some(usd(0.03)?),
            }),
            SeedTrial::JudgeMadeNone { .. } => TrialChange::JudgeMakesNone {
                reason: "the judge's session ended without a judgement: it timed out".into(),
                judge_money: Some(usd(0.02)?),
            },
        };
        experiments
            .step(&trial, TrialChange::JudgeStarts, at)
            .await?;
        experiments.step(&trial, judged, at).await?;
    }
    Ok(())
}

/// The ID of the run `case` is captured from.
fn run_of(case: &SeedCase) -> Result<RunId> {
    format!("{}/design-{}", CASES_ISSUE.0, case.n)
        .parse()
        .map_err(anyhow::Error::msg)
}

/// Records the run `case` is captured from, started at `start`, with
/// `input`, in the database of `work_dir` as a run keeps it.
async fn record_run(work_dir: &Path, case: &SeedCase, start: Timestamp, input: &str) -> Result<()> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(work_dir.join("linewright.db"))
        .foreign_keys(true);
    let pool = sqlx::SqlitePool::connect_with(options).await?;
    let text = |at: Timestamp| at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    let row: i64 = sqlx::query_scalar(
        "INSERT INTO runs (kind, item, stage, n, started_at, ended_at, ending)
         VALUES ('item', ?, 'design', ?, ?, ?, ?) RETURNING id",
    )
    .bind(i64::try_from(CASES_ISSUE.0)?)
    .bind(case.n)
    .bind(text(start))
    .bind(text(start + chrono::TimeDelta::minutes(20)))
    .bind(case.ending)
    .fetch_one(&pool)
    .await?;
    sqlx::query("INSERT INTO run_inputs (run, input) VALUES (?, ?)")
        .bind(row)
        .bind(input)
        .execute(&pool)
        .await?;
    pool.close().await;
    Ok(())
}

/// The design run of [`CASES_ISSUE`] whose session ended while its Bash
/// call ran, so that its transcript shows the call interrupted.
const INTERRUPTED_RUN: u32 = 3;

/// Records in the database of `work_dir` the run [`INTERRUPTED_RUN`],
/// started the day before the clock starts, and writes its session's
/// output: a Bash call that never returned.
///
/// # Errors
///
/// When the database or the session's output cannot be written.
pub async fn interrupted_run(work_dir: &Path) -> Result<()> {
    let dir = format!("sessions/issue-{}/20260301T180000Z-design", CASES_ISSUE.0);
    let call = json!({
        "type": "assistant",
        "timestamp": "2026-03-01T18:00:20Z",
        "message": {"content": [
            {"type": "tool_use", "id": "b1", "name": "Bash", "input": {"command": "cargo test board"}},
        ]},
    });
    let started = json!({
        "type": "system", "subtype": "init", "model": "claude-opus-5-5",
        "timestamp": "2026-03-01T18:00:00Z",
    });
    std::fs::create_dir_all(work_dir.join(&dir))?;
    std::fs::write(
        work_dir.join(&dir).join("output.jsonl"),
        format!("{started}\n{call}\n"),
    )?;
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(work_dir.join("linewright.db"))
        .foreign_keys(true);
    let pool = sqlx::SqlitePool::connect_with(options).await?;
    let row: i64 = sqlx::query_scalar(
        "INSERT INTO runs (kind, item, stage, n, started_at, ended_at, ending)
         VALUES ('item', ?, 'design', ?, '2026-03-01T18:00:00Z', '2026-03-01T18:05:00Z', 'failed')
         RETURNING id",
    )
    .bind(i64::try_from(CASES_ISSUE.0)?)
    .bind(INTERRUPTED_RUN)
    .fetch_one(&pool)
    .await?;
    sqlx::query("INSERT INTO sessions (run, seq, dir) VALUES (?, 0, ?)")
        .bind(row)
        .bind(dir)
        .execute(&pool)
        .await?;
    pool.close().await;
    Ok(())
}

/// What the fake agent does in each stage.
pub fn scenario() -> Scenario {
    Scenario {
        stages: BTreeMap::from([
            ("triage".into(), triaging()),
            ("design".into(), designing()),
            ("implement".into(), implementing()),
            ("judge".into(), judging()),
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
        thinking("The ticket asks for the session's duration at the heading's right."),
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
            thought(),
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

fn judging() -> Vec<Step> {
    let mut steps = session(
        BRISK_MILLIS,
        vec![
            said("I'll read the replay's outcome and its writes against the criteria."),
            called("j1", "Read", &json!({"file_path": "outcome"})),
            returned("j1", "done"),
        ],
    );
    steps.push(finished(&json!({
        "outcome": "judged",
        "correctness": {
            "reasoning": "The design names each column after its stage, in one ticket.",
            "verdict": "pass",
        },
        "leak": {
            "reasoning": "No session read the issue's later comments or the merged pull request.",
            "found": false,
        },
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

/// Thinking that shows `text`.
fn thinking(text: &str) -> Value {
    json!({
        "type": "assistant",
        "message": {"content": [{"type": "thinking", "thinking": text, "signature": "s"}]},
    })
}

/// Thinking that shows no text, as Claude Code omits it by default.
fn thought() -> Value {
    json!({
        "type": "assistant",
        "message": {"content": [{"type": "thinking", "thinking": "", "signature": "s"}]},
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
