//! What `serve-fake` starts from: the repository, its issues, and what the
//! fake agent does in each stage.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use linewright::access::Role;
use linewright::access::people::{Claims, Identity, People};
use linewright::access::providers::Provider;
use linewright::access::providers::fake::FakeAccount;
use linewright::apply::hand_back::LogTail;
use linewright::benchmarks::{
    BenchmarkId, BenchmarkRuns, Benchmarks, RunTask, ScoreFigures, TrialChange, TrialFigures,
    TrialRef,
};
use linewright::cost::Usd;
use linewright::database::Database;
use linewright::domain::{Login, MergeMethods, Number, Timestamp, label};
use linewright::input::{self, Wanted};
use linewright::name::Name;
use linewright::results::Verdict;
use linewright::runs::{Ending, OutcomeName, RunId, Runs};
use linewright::store::local::{LocalStore, Seed};
use linewright::task::{self, BenchmarkChoice, Criteria, TaskStage};
use linewright::trial::Configuration;
use linewright::trial::scorer::Unscored;
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

/// An automation that runs when CI fails on the default branch.
const FIX_CI: &str = "---\non:\n  workflow_run:\n    workflows: [CI]\n    branches: [main]\n    conclusions: [failure]\n---\nFix what failed.\n";

/// An automation file that does not parse: its schedule is not a cron
/// expression.
const NIGHTLY: &str = "---\non:\n  schedule: \"every night\"\n---\nTidy up.\n";

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
                (".linewright/automations/team/weekly.md", WEEKLY),
                (".linewright/automations/nightly.md", NIGHTLY),
                (".linewright/automations/fix-ci.md", FIX_CI),
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

/// The issue whose runs the seed's benchmark holds tasks of: the one closed as
/// done.
const TASKS_ISSUE: Number = Number(3);

/// Closes as done the issue the seed's tasks are captured from. Its runs'
/// input is recorded as it was before, open, which a trial needs.
///
/// # Errors
///
/// When the store refuses the write.
pub fn close_done(store: &LocalStore) -> Result<()> {
    store.close_issue_as_completed(TASKS_ISSUE)
}

/// The benchmark of design tasks.
const BOARD: &str = "board";

/// A task of [`BOARD`], and the design run of [`TASKS_ISSUE`] it is
/// captured from.
struct SeedTask {
    name: &'static str,
    /// The run's number.
    n: u32,
    /// When the run started, the day before the clock starts.
    started_at: &'static str,
    /// How the run ended, twenty minutes after it started.
    ending: &'static str,
    criteria: &'static str,
}

/// In the order they are captured, which is not the order of their names.
const BOARD_TASKS: [SeedTask; 2] = [
    SeedTask {
        name: "columns-named",
        n: 2,
        started_at: "2026-03-01T14:00:00Z",
        ending: "done",
        criteria: "A good design cuts the work into one ticket, with a test that reads the names off the board.\n",
    },
    SeedTask {
        name: "columns-first-try",
        n: 1,
        started_at: "2026-03-01T10:00:00Z",
        ending: "handed-back",
        criteria: "A good design names each column of the board after its stage.\n\nIt keeps the names short enough for a narrow column.\n",
    },
];

/// A benchmark with no tasks yet.
const EMPTY_BENCHMARK: &str = "triage-basics";

/// Writes into the work directory `work_dir` the benchmarks the web UI shows: a
/// benchmark of design tasks captured from #3's runs, which the seed records as
/// ended, with a benchmark run on it that has ended and one cancelled before
/// any score, and an empty benchmark of triage. A work directory that holds
/// the first of those runs already is left as it is.
///
/// # Errors
///
/// When the database, the store or a capture fails.
pub async fn benchmarks(store: &LocalStore, work_dir: &Path) -> Result<()> {
    let db = Database::open(work_dir).await?;
    let runs = Runs::new(db.clone());
    let benchmarks = Benchmarks::new(db);
    let first = run_of(&BOARD_TASKS[0])?;
    if runs.try_read_run(&first).await?.is_some() {
        return Ok(());
    }
    let wanted = Wanted {
        items: BTreeSet::from([TASKS_ISSUE]),
        branches: vec![DEFAULT_BRANCH.into()],
    };
    for task in &BOARD_TASKS {
        let start: Timestamp = task.started_at.parse()?;
        let input = input::record(store, &wanted, start).await?;
        record_run(work_dir, task, start, &serde_json::to_string(&input)?).await?;
    }
    let board = Name::parse(BOARD)?;
    let board = benchmarks.make(&board, TaskStage::Design).await?.id;
    for task in &BOARD_TASKS {
        let capturable = task::capturable(
            &runs,
            &benchmarks,
            &run_of(task)?,
            &Name::parse(task.name)?,
            &BenchmarkChoice::Existing(board),
        )
        .await
        .map_err(|e| anyhow::anyhow!(e.why()))?;
        let criteria = Criteria::parse(task.criteria).context("the seed's criteria are blank")?;
        task::capture(store, capturable, &criteria, &benchmarks)
            .await
            .map_err(|e| anyhow::anyhow!(e.why()))?;
    }
    let empty = Name::parse(EMPTY_BENCHMARK)?;
    benchmarks.make(&empty, TaskStage::Triage).await?;
    let benchmark_runs = BenchmarkRuns::new(runs.database().clone());
    benchmark_run(&benchmark_runs, board).await?;
    cancelled_benchmark_run(&benchmark_runs, board).await
}

/// How each trial of the seed's benchmark run ended, in start order: by
/// repetition, then task, then configuration.
const TRIALS: [SeedTrial; 8] = [
    SeedTrial::Scored {
        ending: Ending::Done,
        money: 0.42,
        seconds: 95,
        verdict: Verdict::Pass,
        leak: false,
    },
    SeedTrial::Scored {
        ending: Ending::Done,
        money: 0.21,
        seconds: 61,
        verdict: Verdict::Pass,
        leak: true,
    },
    SeedTrial::Unscored {
        ending: Ending::Failed,
        money: 0.05,
        seconds: 12,
    },
    SeedTrial::Scored {
        ending: Ending::HandedBack,
        money: 0.18,
        seconds: 48,
        verdict: Verdict::Pass,
        leak: false,
    },
    SeedTrial::Scored {
        ending: Ending::Done,
        money: 0.39,
        seconds: 88,
        verdict: Verdict::Fail,
        leak: false,
    },
    SeedTrial::ScorerMadeNone {
        money: 0.24,
        seconds: 70,
        why: ScorerWhy::GaveUp,
    },
    SeedTrial::ScorerMadeNone {
        money: 0.47,
        seconds: 102,
        why: ScorerWhy::BuildFailed,
    },
    SeedTrial::BuildFailed { seconds: 9 },
];

/// How a trial of the seed's benchmark run ended.
enum SeedTrial {
    Scored {
        ending: Ending,
        money: f64,
        seconds: u64,
        verdict: Verdict,
        leak: bool,
    },
    /// Its run neither done nor handed back.
    Unscored {
        ending: Ending,
        money: f64,
        seconds: u64,
    },
    /// Its run done, and its scorer made no score.
    ScorerMadeNone {
        money: f64,
        seconds: u64,
        why: ScorerWhy,
    },
    /// The build of its environment failed, its output begun.
    BuildFailed { seconds: u64 },
}

/// Why a seeded scorer made no score.
#[derive(Clone, Copy)]
enum ScorerWhy {
    /// It gave up.
    GaveUp,
    /// The build of its environment failed, with a log.
    BuildFailed,
}

impl ScorerWhy {
    /// Why, as the trial keeps it.
    fn why(self) -> String {
        match self {
            ScorerWhy::GaveUp => {
                Unscored::GaveUp("the criteria name no column to check".into()).to_string()
            }
            ScorerWhy::BuildFailed => Unscored::BuildFailed {
                log_tail: LogTail::maintenance(
                    "$ npm ci\nnpm ERR! code ENOTFOUND\nnpm ERR! network request to \
                     https://registry.npmjs.org/tailwindcss failed\n",
                ),
            }
            .to_string(),
        }
    }
}

/// Launches on `board` a benchmark run the day before the clock starts, under
/// `as-is` and `terse`, which sets a low effort, twice, and ends each of its
/// trials as [`TRIALS`] says, twenty minutes after its launch: with passes,
/// failures, a leak and trials without a score. The first trial's
/// output holds what its run and its scorer left, as [`trial_output`] says,
/// the output of a trial with a leak its score, as [`leak_output`] says, and
/// that of a trial whose build failed the task it began on.
async fn benchmark_run(benchmark_runs: &BenchmarkRuns, board: BenchmarkId) -> Result<()> {
    let terse = Configuration::parse(BTreeMap::from([(
        "config.toml".to_string(),
        b"effort = \"low\"\n".to_vec(),
    )]))
    .map_err(|errors| anyhow::anyhow!("the seed's configuration: {errors:?}"))?;
    let launched_at: Timestamp = "2026-03-01T16:00:00Z".parse()?;
    let launched = benchmark_runs
        .launch(
            board,
            vec![(Name::parse("terse")?, terse)],
            NonZeroU32::new(2).context("two repetitions")?,
            launched_at,
        )
        .await?;
    let record = benchmark_runs
        .show(launched.id)
        .await?
        .context("the seed's benchmark run is kept")?;
    let at = launched_at + chrono::TimeDelta::minutes(20);
    for (key, seeded) in record.trials.keys().zip(&TRIALS) {
        let trial = TrialRef {
            benchmark_run: launched.id,
            key: *key,
        };
        let output = benchmark_runs
            .trial_output(&record, key)
            .context("the trial is of the benchmark run")?;
        benchmark_runs
            .step(&trial, TrialChange::RunStarts, at)
            .await?;
        let (ending, money, seconds) = match *seeded {
            SeedTrial::Scored {
                ending,
                money,
                seconds,
                ..
            }
            | SeedTrial::Unscored {
                ending,
                money,
                seconds,
            } => (ending, money, seconds),
            SeedTrial::ScorerMadeNone { money, seconds, .. } => (Ending::Done, money, seconds),
            SeedTrial::BuildFailed { seconds } => {
                write_files(
                    &output,
                    [(
                        linewright::trial::TASK.to_string(),
                        format!("{}\n", run_of(seed_task(&record.tasks[key.task])?)?),
                    )],
                )?;
                let time = Duration::from_secs(seconds);
                benchmark_runs
                    .step(&trial, TrialChange::BuildFails { time }, at)
                    .await?;
                continue;
            }
        };
        benchmark_runs
            .step(
                &trial,
                TrialChange::RunEnds(TrialFigures {
                    outcome: OutcomeName::Ended(ending),
                    money: Some(usd(money)?),
                    time: Duration::from_secs(seconds),
                }),
                at,
            )
            .await?;
        let scored = match *seeded {
            SeedTrial::Unscored { .. } | SeedTrial::BuildFailed { .. } => continue,
            SeedTrial::Scored { verdict, leak, .. } => {
                if leak {
                    leak_output(&output, verdict)?;
                }
                TrialChange::Scored(ScoreFigures {
                    correctness: verdict,
                    leak,
                    scorer_money: Some(usd(0.03)?),
                })
            }
            SeedTrial::ScorerMadeNone { why, .. } => TrialChange::ScorerMakesNone {
                why: why.why(),
                scorer_money: Some(usd(0.02)?),
            },
        };
        benchmark_runs
            .step(&trial, TrialChange::ScorerStarts, at)
            .await?;
        benchmark_runs.step(&trial, scored, at).await?;
    }
    let first = record
        .trials
        .keys()
        .next()
        .context("the benchmark run has trials")?;
    let task_run = seed_task(&record.tasks[first.task])?;
    trial_output(
        &benchmark_runs
            .trial_output(&record, first)
            .context("the trial is of the benchmark run")?,
        task_run,
    )
}

/// Launches on `board` a benchmark run under `as-is` alone, twice, after the
/// seed's other one, and cancels it as its first repetition's trials are in
/// flight: the run of the first has ended and its scorer has not started, so
/// that trial ends cancelled with its run; the cancel stops the run of the
/// second, which keeps what it cost until then; and the second repetition's
/// trials have not started.
async fn cancelled_benchmark_run(benchmark_runs: &BenchmarkRuns, board: BenchmarkId) -> Result<()> {
    let launched_at: Timestamp = "2026-03-01T18:00:00Z".parse()?;
    let launched = benchmark_runs
        .launch(
            board,
            Vec::new(),
            NonZeroU32::new(2).context("two repetitions")?,
            launched_at,
        )
        .await?;
    let at = launched_at + chrono::TimeDelta::minutes(1);
    let record = benchmark_runs
        .show(launched.id)
        .await?
        .context("the seed's cancelled benchmark run is kept")?;
    let mut keys = record.trials.keys();
    let mut next = || {
        anyhow::Ok(TrialRef {
            benchmark_run: launched.id,
            key: *keys.next().context("the benchmark run has two trials")?,
        })
    };
    let (ran, stopped) = (next()?, next()?);
    for trial in [&ran, &stopped] {
        benchmark_runs
            .step(trial, TrialChange::RunStarts, at)
            .await?;
    }
    benchmark_runs
        .step(
            &ran,
            TrialChange::RunEnds(TrialFigures {
                outcome: OutcomeName::Ended(Ending::Done),
                money: Some(usd(0.31)?),
                time: Duration::from_secs(52),
            }),
            at,
        )
        .await?;
    benchmark_runs
        .cancel(launched.id, at)
        .await
        .context("cancelling the seed's benchmark run")?;
    benchmark_runs
        .step(&ran, TrialChange::Cancelled { stopped: None }, at)
        .await?;
    benchmark_runs
        .step(
            &stopped,
            TrialChange::Cancelled {
                stopped: Some(TrialFigures {
                    outcome: OutcomeName::Ended(STOPPED),
                    money: Some(usd(0.12)?),
                    time: Duration::from_secs(41),
                }),
            },
            at,
        )
        .await?;
    run_output(
        &benchmark_runs
            .trial_output(&record, &stopped.key)
            .context("the trial is of the benchmark run")?,
        seed_task(&record.tasks[stopped.key.task])?,
        STOPPED,
        "The board's columns are half named when the run stops.",
    )
}

/// How the run of the trial of the seed's cancelled benchmark run that the
/// cancel stopped ended.
const STOPPED: Ending = Ending::Cancelled;

/// The seed's task `task`.
fn seed_task(task: &RunTask) -> Result<&'static SeedTask> {
    BOARD_TASKS
        .iter()
        .find(|seeded| seeded.name == task.name.as_str())
        .context("the trial's task is the seed's")
}

/// Writes in `output` what the first trial of the seed's benchmark run left
/// there as it ran `task` and its scorer scored it: the task it
/// ran, its run's record and session, its score and its scorer's
/// session.
fn trial_output(output: &Path, task: &SeedTask) -> Result<()> {
    run_output(
        output,
        task,
        Ending::Done,
        "The board's columns now carry their names.",
    )?;
    let score = score(
        Verdict::Pass,
        "The board names its columns as the criteria ask, and the tests pass.",
        &json!({
            "reasoning": "No call read the task's pull request or its branch.",
            "found": false,
            "calls": [],
        }),
    )?;
    let files = [
        (
            linewright::trial::scorer::SCORE.to_string(),
            score.to_string(),
        ),
        (
            format!("{SCORER}/output.jsonl"),
            transcript(
                "2026-03-01T16:10:00Z",
                "The trial passes, and it read nothing it should not.",
            ),
        ),
        (
            format!("{SCORER}/brief.md"),
            "# Your task\n\nScore the trial.\n".to_string(),
        ),
    ];
    write_files(output, files)
}

/// Writes in `output` what a trial of a seed's benchmark run left there as it
/// ran `task` until its run ended as `ending`, its agent last saying `said`:
/// the task it ran, and its run's record and session.
fn run_output(output: &Path, task: &SeedTask, ending: Ending, said: &str) -> Result<()> {
    let run = run_of(task)?;
    let session = format!("sessions/issue-{}/20260301T160005Z-design", TASKS_ISSUE.0);
    let mut session_entry = json!({"dir": session});
    extend(&mut session_entry, &ran())?;
    let record = json!({
        "item": TASKS_ISSUE.0,
        "stage": "design",
        "n": task.n,
        "started_at": "2026-03-01T16:00:05Z",
        "end": {"at": "2026-03-01T16:09:40Z", "ending": ending},
        "sessions": [session_entry],
    });
    let files = [
        (linewright::trial::TASK.to_string(), format!("{run}\n")),
        (
            format!("runs/items/{}/design-{}.json", TASKS_ISSUE.0, task.n),
            record.to_string(),
        ),
        (
            format!("{session}/output.jsonl"),
            transcript("2026-03-01T16:00:05Z", said),
        ),
        (
            format!("{session}/brief.md"),
            "# Your task\n\nDesign the ticket below.\n".to_string(),
        ),
    ];
    write_files(output, files)
}

/// A session's transcript in which the agent, started at `at`, says `text`.
fn transcript(at: &str, text: &str) -> String {
    let started = json!({
        "type": "system", "subtype": "init", "model": "claude-opus-5-5", "timestamp": at,
    });
    let said = json!({
        "type": "assistant", "timestamp": at,
        "message": {"content": [{"type": "text", "text": text}]},
    });
    format!("{started}\n{said}\n")
}

/// Writes each file of `files`, a path under `output` and its text.
fn write_files(output: &Path, files: impl IntoIterator<Item = (String, String)>) -> Result<()> {
    for (path, text) in files {
        let path = output.join(path);
        std::fs::create_dir_all(path.parent().context("a file has a directory")?)?;
        std::fs::write(path, text)?;
    }
    Ok(())
}

/// Writes in `output` the score of a trial of the seed's benchmark run whose
/// correctness is `verdict`, with a leak that two tool calls show.
fn leak_output(output: &Path, verdict: Verdict) -> Result<()> {
    let score = score(
        verdict,
        "The board names its columns, and its test reads them back.",
        &json!({
            "reasoning": "The session read the pull request that closed the task's issue.",
            "found": true,
            "calls": ["gh pr view 4 --comments", "git log origin/3-name-the-columns"],
        }),
    )?;
    std::fs::create_dir_all(output)?;
    std::fs::write(
        output.join(linewright::trial::scorer::SCORE),
        score.to_string(),
    )?;
    Ok(())
}

/// The record of the scorer's session of a trial of the seed's benchmark run,
/// under its output.
const SCORER: &str = "scorer/20260301T161000Z-scorer";

/// What a session of the seed's benchmark run ran with: the operator's
/// model and Linewright's own effort.
fn ran() -> Value {
    json!({
        "model": "claude-opus-5-5",
        "effort": "medium",
        "layers": {
            "settings": {"model": "operator", "effort": "linewright"},
            "prompts": {},
        },
    })
}

/// A score as a trial's output keeps it, of correctness `verdict` for
/// `reasoning`, and of the leak `leak`, scored in [`SCORER`]'s session.
fn score(verdict: Verdict, reasoning: &str, leak: &Value) -> Result<Value> {
    let mut session = json!({"record": SCORER, "cost": {}});
    extend(&mut session, &ran())?;
    Ok(json!({
        "correctness": {"reasoning": reasoning, "verdict": verdict},
        "leak": leak,
        "session": session,
        "linewright": env!("CARGO_PKG_VERSION"),
    }))
}

/// `money` dollars.
fn usd(money: f64) -> Result<Usd> {
    Usd::new(money).context("a cost is a number of dollars")
}

/// Adds the fields of the JSON object `more` to the JSON object `object`.
fn extend(object: &mut Value, more: &Value) -> Result<()> {
    object
        .as_object_mut()
        .context("a JSON object")?
        .extend(more.as_object().context("a JSON object")?.clone());
    Ok(())
}

/// The ID of the run `task` is captured from.
fn run_of(task: &SeedTask) -> Result<RunId> {
    format!("{}/design-{}", TASKS_ISSUE.0, task.n)
        .parse()
        .map_err(anyhow::Error::msg)
}

/// Records the run `task` is captured from, started at `start`, with
/// `input`, in the database of `work_dir` as a run keeps it.
async fn record_run(work_dir: &Path, task: &SeedTask, start: Timestamp, input: &str) -> Result<()> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(work_dir.join("linewright.db"))
        .foreign_keys(true);
    let pool = sqlx::SqlitePool::connect_with(options).await?;
    let text = |at: Timestamp| at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    let row: i64 = sqlx::query_scalar(
        "INSERT INTO runs (kind, item, stage, n, started_at, ended_at, ending)
         VALUES ('item', ?, 'design', ?, ?, ?, ?) RETURNING id",
    )
    .bind(i64::try_from(TASKS_ISSUE.0)?)
    .bind(task.n)
    .bind(text(start))
    .bind(text(start + chrono::TimeDelta::minutes(20)))
    .bind(task.ending)
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

/// The design run of [`TASKS_ISSUE`] whose session ended while its Bash
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
    let dir = format!("sessions/issue-{}/20260301T180000Z-design", TASKS_ISSUE.0);
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
    .bind(i64::try_from(TASKS_ISSUE.0)?)
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

/// How many earlier runs [`earlier_runs`] records, enough for Runs to show
/// more than one page (`docs/specs/design_system.md`, Tables, Pages).
const EARLIER_RUNS: i64 = 60;

/// Records the earlier triage runs of the tasks' issue, each done in a minute,
/// an hour apart up to the day before the clock starts.
///
/// # Errors
///
/// When the database cannot be written.
pub async fn earlier_runs(work_dir: &Path) -> Result<()> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(work_dir.join("linewright.db"))
        .foreign_keys(true);
    let pool = sqlx::SqlitePool::connect_with(options).await?;
    for n in 1..=EARLIER_RUNS {
        let (day, hour) = (25 + n / 24, n % 24);
        let started = format!("2026-02-{day:02}T{hour:02}:00:00Z");
        let ended = format!("2026-02-{day:02}T{hour:02}:01:00Z");
        sqlx::query(
            "INSERT INTO runs (kind, item, stage, n, started_at, ended_at, ending)
             VALUES ('item', ?, 'triage', ?, ?, ?, 'done')",
        )
        .bind(i64::try_from(TASKS_ISSUE.0)?)
        .bind(n)
        .bind(started)
        .bind(ended)
        .execute(&pool)
        .await?;
    }
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
            ("scorer".into(), scoring()),
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
        called(
            "i3",
            "Agent",
            &json!({
                "description": "Find the heading's tests",
                "prompt": "Find the tests of the run page's session heading.",
            }),
        ),
        of_sub_agent(
            "i3",
            called(
                "i3a",
                "Grep",
                &json!({"pattern": "heading", "path": "tests"}),
            ),
        ),
        of_sub_agent(
            "i3",
            returned("i3a", "tests/run_page.rs: fn a_heading_shows_its_duration"),
        ),
        returned("i3", "The heading's tests are in tests/run_page.rs."),
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

fn scoring() -> Vec<Step> {
    let mut steps = session(
        BRISK_MILLIS,
        vec![
            said("I'll read the trial's outcome and its writes against the criteria."),
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

/// `event`, as the sub-agent that the call `call` started streams it.
fn of_sub_agent(call: &str, mut event: Value) -> Value {
    event["parent_tool_use_id"] = json!(call);
    event
}

fn returned(id: &str, output: &str) -> Value {
    json!({
        "type": "user",
        "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": output}]},
    })
}

/// The person the `manager` setting names.
pub const MANAGER: &str = "ada@example.com";

/// The domain whose verified emails meet Google's sign-up rule.
pub const SIGNUP_DOMAIN: &str = "example.com";

/// The accounts the fake Google signs in: the manager; a contributor and a
/// viewer, whom [`people`] adds; someone of the sign-up rule's domain, whom
/// a first sign-in adds as a viewer; and two that Linewright refuses.
pub fn google_accounts() -> Vec<FakeAccount> {
    let account = |subject: &str, email: &str, verified: bool, name: Option<&str>| FakeAccount {
        subject: subject.into(),
        email: email.into(),
        email_verified: verified,
        name: name.map(Into::into),
        picture: None,
    };
    vec![
        account("1001", MANAGER, true, Some("Ada Lovelace")),
        account("1002", "grace@example.com", true, Some("Grace Hopper")),
        account("1003", "vera@example.com", true, Some("Vera Viewer")),
        account("1004", "new@example.com", true, None),
        account("1005", "mallory@elsewhere.example", true, Some("Mallory")),
        account(
            "1006",
            "unverified@example.com",
            false,
            Some("Una Verified"),
        ),
    ]
}

/// Adds the manager, the contributor and the viewer of
/// [`google_accounts`], as their first sign-ins would have, but those the
/// database already holds.
pub async fn people(database: &Database) -> Result<()> {
    let people = People::new(database.clone());
    for (account, role) in
        google_accounts()
            .into_iter()
            .zip([Role::Manager, Role::Contributor, Role::Viewer])
    {
        let claims = Claims {
            identity: Identity {
                provider: Provider::Google,
                subject: account.subject,
            },
            shown_as: account.email,
            name: account.name,
            avatar: None,
            email_verified: account.email_verified,
        };
        if people.find_by_identity(&claims.identity).await?.is_none() {
            people.add(&claims, role, started_at()).await?;
        }
    }
    Ok(())
}
