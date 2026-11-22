//! Scoring a trial's output, through the system: the scorer's session on
//! what the trial left, and the score it keeps in the output
//! (`docs/specs/benchmarks.md`, Scoring).

use crate::common;
use crate::trial::{
    configuration, entries, gh_answer, lw_command, lw_isolated, ran_trial_with, triage_task,
    triaged,
};

use std::collections::BTreeMap;
use std::io::Write as _;
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::*;
use linewright::apply::hand_back::LogTail;
use linewright::cost::{Figures, Spend, Tokens, TurnSpend};
use linewright::factory::FactoryConfig;
use linewright::harness::SessionError;
use linewright::reconcile::WorkKind;
use linewright::runs::UnexpectedEnd;
use linewright::session::Ended;
use linewright::task::Task;
use linewright::trial::Under;
use linewright::trial::scorer::{self, Score, ScorerRecord, Unscored};
use linewright::work_settings::{Effort, Partial, Setup, SetupCommand, StageKey, StageTable};
use rstest::rstest;
use serde_json::{Value, json};

/// A scorer's result: the verdict `verdict`, and a leak found when `calls`
/// quotes any.
fn scored_as(verdict: &str, calls: &[&str]) -> String {
    json!({
        "outcome": "judged",
        "correctness": {"reasoning": format!("It is a {verdict}."), "verdict": verdict},
        "leak": {
            "reasoning": "I read every transcript.",
            "found": !calls.is_empty(),
            "calls": calls,
        },
    })
    .to_string()
}

/// A triage task of a new world, kept in `under/case`, and the output of a
/// trial of it whose run ended `outcome`, written in `under/out` as a
/// trial leaves it: the ID of the task's run and the outcome.
#[track_caller]
pub(crate) fn triage_output<'a>(
    under: &'a Path,
    outcome: &'a str,
) -> impl Future<Output = (PathBuf, PathBuf)> + 'a {
    let caller = Location::caller();
    async move {
        let mut world = World::new().await;
        let (_, captured) = triage_task(&mut world).await;
        let task = under.join("case");
        copy_dir(&captured, &task);
        let run = Task::open(&task)
            .await
            .unwrap_or_else(|e| panic!("opening the case at {caller}: {e}"))
            .run()
            .to_string();
        let out = under.join("out");
        std::fs::create_dir(&out).unwrap();
        std::fs::write(out.join("task"), format!("{run}\n")).unwrap();
        std::fs::write(out.join("outcome"), format!("{outcome}\n")).unwrap();
        (task, out)
    }
}

/// Scores the output `out` of a trial of the task in `task` with the
/// scorers `harness` scripts and the factory config `adjust` makes of the
/// tests' own, its work directory under `under`, keeping the score it
/// makes, and gives its summary, and waits for what it leaves to remove.
#[track_caller]
fn scoring<'a>(
    task: &'a Path,
    out: &'a Path,
    under: &'a Path,
    harness: Arc<ScriptedHarness>,
    adjust: impl FnOnce(&mut FactoryConfig) + 'a,
) -> impl Future<Output = Result<String, Unscored>> + 'a {
    let caller = Location::caller();
    async move {
        let task = Task::open(task)
            .await
            .unwrap_or_else(|e| panic!("opening the case at {caller}: {e}"));
        let scoreable = scorer::ScoreableTrial::open(out, &task)
            .await
            .unwrap_or_else(|e| panic!("opening the output at {caller}: {e}"));
        let record = ScorerRecord::make(&scoreable)
            .unwrap_or_else(|e| panic!("making the scorer's record at {caller}: {e}"));
        let mut config = config(&under.join("work").join(REPO)).await;
        adjust(&mut config);
        let winding_up = config.winding_up.clone();
        let scored = scorer::run(
            &task,
            &scoreable,
            record,
            harness,
            config,
            &Under::command(),
        )
        .await;
        let kept = scored.map(|score: Score| {
            let summary = score.summary();
            scorer::keep(&scoreable, score)
                .unwrap_or_else(|e| panic!("keeping the score at {caller}: {e}"));
            summary
        });
        winding_up.wait().await;
        kept
    }
}

/// The score kept in the output `out`.
#[track_caller]
fn score_in(out: &Path) -> Value {
    let text = std::fs::read_to_string(out.join("score.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The names in the scorers' directory of the output `out`.
#[track_caller]
fn scorer_records(out: &Path) -> Vec<String> {
    let dir = out.join("scorer");
    if !dir.exists() {
        return vec![];
    }
    entries(&dir)
}

/// What `lw --version` prints, without its newline.
#[track_caller]
fn lw_version() -> String {
    let version = std::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .arg("--version")
        .output()
        .unwrap();
    String::from_utf8(version.stdout).unwrap().trim_end().into()
}

#[test_log::test(tokio::test)]
async fn a_scorer_keeps_its_score_in_the_output_with_its_record() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.reports_model("claude-judge-1");
    harness.on_scorer(|_| Ok(scored_as("pass", &[])));
    let scorer_settings = |config: &mut FactoryConfig| {
        edit_work(config, |work| {
            work.stages.insert(
                StageKey::Scorer,
                StageTable {
                    settings: Partial {
                        model: Some("judge-model".into()),
                        effort: Some(Effort::High),
                        session_timeout: None,
                    },
                    ..StageTable::default()
                },
            );
        });
    };

    let scored = scoring(&task, &out, under.path(), harness.clone(), scorer_settings).await;

    assert_eq!(scored.unwrap(), "Verdict: pass\nLeak: none found\n");
    let kept = score_in(&out);
    assert_eq!(
        kept["correctness"],
        json!({"reasoning": "It is a pass.", "verdict": "pass"})
    );
    assert_eq!(
        kept["leak"],
        json!({"reasoning": "I read every transcript.", "found": false, "calls": []})
    );
    assert_eq!(kept["linewright"], json!(lw_version()));
    let record = kept["session"]["record"].as_str().unwrap().to_string();
    assert!(
        record.starts_with("scorer/") && record.ends_with("-scorer"),
        "{record}"
    );
    assert!(out.join(&record).join("brief.md").is_file(), "{record}");
    assert_eq!(kept["session"]["model"], json!("judge-model"));
    assert_eq!(kept["session"]["effort"], json!("high"));
    assert_eq!(kept["session"]["reported_model"], json!("claude-judge-1"));
    assert_eq!(
        kept["session"]["layers"]["settings"]["model"],
        json!("operator")
    );
    assert_eq!(harness.scorer_prompts().len(), 1);
    assert_eq!(
        scorer_records(&out),
        vec![record.strip_prefix("scorer/").unwrap().to_string()]
    );
    assert_eq!(
        entries(&out),
        vec!["outcome", "score.json", "scorer", "task"]
    );
}

#[test_log::test(tokio::test)]
async fn a_score_keeps_what_the_scorers_session_cost_apart_from_the_trials() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.reports_spend(TurnSpend::OfSessionSoFar(Figures(BTreeMap::from([(
        "claude-judge-1".to_string(),
        Spend {
            tokens: Some(Tokens {
                input: 1,
                output: 20,
                cache_write: 300,
                cache_read: 4000,
            }),
            usd: None,
        },
    )]))));
    harness.on_scorer(|_| Ok(scored_as("pass", &[])));

    scoring(&task, &out, under.path(), harness, |_| {})
        .await
        .unwrap();

    let mut cost = score_in(&out)["session"]["cost"].clone();
    let seconds = cost.as_object_mut().unwrap().remove("seconds");
    assert!(seconds.as_ref().is_some_and(Value::is_f64), "{seconds:?}");
    assert_eq!(
        cost,
        json!({"models": {"claude-judge-1": {
            "tokens": {"input": 1, "output": 20, "cache_write": 300, "cache_read": 4000},
        }}})
    );
    assert!(!out.join("cost.json").exists());
}

#[test_log::test(tokio::test)]
async fn a_scorer_that_makes_no_score_keeps_nothing_of_itself() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "handed-back").await;
    let harness = Arc::new(ScriptedHarness::default());

    harness.on_scorer(|_| {
        Ok(json!({"outcome": "gave-up", "reason": "The output is empty."}).to_string())
    });
    let gave_up = scoring(&task, &out, under.path(), harness.clone(), |_| {}).await;
    harness.on_scorer(|_| {
        Ok(json!({
            "outcome": "judged",
            "correctness": {"reasoning": "It passes.", "verdict": "pass"},
            "leak": {"reasoning": "It read the pull request.", "found": true},
        })
        .to_string())
    });
    let unmatched = scoring(&task, &out, under.path(), harness, |config| {
        config.result_retries = 1;
    })
    .await;

    let gave_up = gave_up.expect_err("a scorer that gives up makes no score");
    assert_eq!(
        gave_up.to_string(),
        "The scorer gave up: The output is empty."
    );
    let unmatched = unmatched.expect_err("a result never accepted is no score");
    let said = unmatched.to_string();
    assert!(
        said.starts_with(
            "The scorer's session ended without a score: its result was not accepted: "
        ) && said.contains("/leak"),
        "{said}"
    );
    assert_eq!(entries(&out), vec!["outcome", "task"]);
}

#[rstest]
#[case::the_usage_limit(
    Unscored::UsageLimit {
        resets_at: "2026-11-20T18:00:00Z".parse().unwrap(),
    },
    "The scorer's session reached the usage limit, which resets at 2026-11-20 18:00:00 UTC."
)]
#[case::giving_up(
    Unscored::GaveUp("The output is empty.".into()),
    "The scorer gave up: The output is empty."
)]
#[case::its_session_ending_with_its_log(
    Unscored::Ended(Ended {
        end: UnexpectedEnd::NoResult,
        log_tail: LogTail::session("Out of memory.\n"),
    }),
    "The scorer's session ended without a score: the harness stopped without a result\n\n\
     Out of memory."
)]
#[case::its_session_ending_with_no_log(
    Unscored::Ended(Ended {
        end: UnexpectedEnd::TimedOut {
            after: std::time::Duration::from_mins(30),
            setting: linewright::work_settings::TimeoutSetting::Pipeline,
        },
        log_tail: None,
    }),
    "The scorer's session ended without a score: it timed out after 30m"
)]
#[case::its_build_failing_with_its_log(
    Unscored::BuildFailed {
        log_tail: LogTail::maintenance("npm ci\nnpm ERR! no package.json\n"),
    },
    "The scorer's environment could not be built.\n\nnpm ci\nnpm ERR! no package.json"
)]
#[case::its_build_failing_with_no_log(
    Unscored::BuildFailed { log_tail: None },
    "The scorer's environment could not be built."
)]
#[case::its_score_not_kept(
    Unscored::NotKept(std::io::Error::from(std::io::ErrorKind::StorageFull)),
    "The score could not be kept: no storage space"
)]
#[case::any_other_cause(
    Unscored::Unexpected(anyhow::anyhow!("no such file").context("the task cannot be scored")),
    "The scorer stopped unexpectedly."
)]
fn a_scorer_that_makes_no_score_says_why(#[case] unscored: Unscored, #[case] why: &str) {
    assert_eq!(unscored.to_string(), why);
}

#[test_log::test]
fn the_cause_of_a_scorer_that_stopped_unexpectedly_is_logged_and_not_told() {
    let (log, _guard) = Log::capture();

    let told = Unscored::Unexpected(anyhow::anyhow!("no such file")).told();

    assert_eq!(told, "The scorer stopped unexpectedly.");
    let errors = log.lines("ERROR");
    assert!(
        errors.len() == 1 && errors[0].contains("no such file"),
        "{errors:?}"
    );
}

#[test_log::test(tokio::test)]
async fn a_scorer_whose_environment_cannot_be_built_says_so_with_the_builds_log() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let harness = Arc::new(ScriptedHarness::default());

    let scored = scoring(&task, &out, under.path(), harness.clone(), |config| {
        edit_work(config, |work| {
            work.pipeline_flags_and_env.setup.command = Some(Setup::Command(
                SetupCommand::new("echo the build broke; exit 3".into()).unwrap(),
            ));
        });
    })
    .await;

    let unscored = scored.expect_err("a scorer with no environment makes no score");
    assert!(
        matches!(unscored, Unscored::BuildFailed { .. }),
        "{unscored:?}"
    );
    assert_eq!(
        unscored.to_string(),
        "The scorer's environment could not be built.\n\nthe build broke"
    );
    assert_eq!(harness.scorer_prompts().len(), 0);
    assert_eq!(entries(&out), vec!["outcome", "task"]);
}

#[test_log::test(tokio::test)]
async fn a_scorers_session_that_the_harness_ends_with_an_error_is_its_typed_end() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.on_scorer(|_| {
        Err(SessionError::Errored {
            kind: "error_max_turns".into(),
            result: Some("stuck".into()),
            log_tail: "Reading the diff.".into(),
        })
    });

    let scored = scoring(&task, &out, under.path(), harness, |_| {}).await;

    let unscored = scored.expect_err("a session the harness ended makes no score");
    let Unscored::Ended(Ended { end, log_tail }) = &unscored else {
        panic!("expected an unexpected end, got {unscored:?}");
    };
    assert_eq!(
        (end, log_tail.as_ref().map(LogTail::text)),
        (
            &UnexpectedEnd::HarnessError {
                kind: "error_max_turns".into(),
            },
            Some("Reading the diff.")
        )
    );
    assert_eq!(
        unscored.to_string(),
        "The scorer's session ended without a score: the harness ended it with \
         `error_max_turns`\n\nReading the diff."
    );
}

#[test_log::test(tokio::test)]
async fn a_score_is_not_kept_over_one_the_output_came_to_hold_while_it_was_made() {
    let under = tempfile::tempdir().unwrap();
    let (task_dir, out) = triage_output(under.path(), "done").await;
    let task = Task::open(&task_dir).await.unwrap();
    let scoreable = scorer::ScoreableTrial::open(&out, &task).await.unwrap();
    let record = ScorerRecord::make(&scoreable).unwrap();
    let harness = Arc::new(ScriptedHarness::default());
    harness.on_scorer(|_| Ok(scored_as("pass", &[])));
    let config = config(&under.path().join("work").join(REPO)).await;
    let winding_up = config.winding_up.clone();
    let score = scorer::run(
        &task,
        &scoreable,
        record,
        harness,
        config,
        &Under::command(),
    )
    .await
    .unwrap();
    winding_up.wait().await;
    std::fs::write(out.join("score.json"), "{}\n").unwrap();

    let kept = scorer::keep(&scoreable, score);

    let refused = kept.expect_err("a score is never kept over another");
    assert!(
        matches!(&refused, Unscored::NotKept(e) if e.kind() == std::io::ErrorKind::AlreadyExists),
        "{refused:?}"
    );
    assert_eq!(
        std::fs::read_to_string(out.join("score.json")).unwrap(),
        "{}\n"
    );
    assert_eq!(entries(&out), vec!["outcome", "score.json", "task"]);
}

#[test_log::test(tokio::test)]
async fn a_scorers_session_that_fails_on_its_infrastructure_is_retried_once() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let harness = Arc::new(ScriptedHarness::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    harness.on_scorer(move |_| {
        if counted.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(SessionError::Infrastructure("the harness lost it".into()));
        }
        Ok(scored_as("pass", &[]))
    });

    let scored = scoring(&task, &out, under.path(), harness, |_| {}).await;

    assert!(scored.is_ok(), "{scored:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(scorer_records(&out).len(), 1);
}

#[test_log::test(tokio::test)]
async fn a_scorers_session_that_fails_on_its_infrastructure_again_on_the_retry_ends_without_a_score()
 {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.on_scorer(|_| Err(SessionError::Infrastructure("the harness lost it".into())));

    let scored = scoring(&task, &out, under.path(), harness, |_| {}).await;

    let unscored = scored.expect_err("a session that failed again on the retry makes no score");
    assert_eq!(
        unscored.to_string(),
        "The scorer's session ended without a score: it failed on its infrastructure: the \
         harness lost it"
    );
    assert_eq!(entries(&out), vec!["outcome", "task"]);
}

/// What a scorer saw of the trial in its copy, from the paths its prompt
/// names.
#[derive(Debug)]
struct Copied {
    root: PathBuf,
    entries: Vec<String>,
    outcome: String,
    brief: String,
    write: String,
    attachments: Vec<String>,
    attachment: Vec<u8>,
}

/// What the scorer called with `call` sees in its copy of the trial, which
/// it then overwrites the copy's first write of in place.
#[track_caller]
fn seen_then_overwritten(call: &Call) -> Copied {
    let prompt = &call.spec.prompt;
    let outcome = PathBuf::from(filled_in(prompt, "outcome"));
    let sessions = PathBuf::from(filled_in(prompt, "sessions"));
    let writes = PathBuf::from(filled_in(prompt, "writes"));
    let root = outcome.parent().unwrap().to_path_buf();
    assert_eq!(sessions, root.join("sessions"));
    assert_eq!(writes, root.join("writes"));
    let seen = Copied {
        entries: entries(&root),
        outcome: std::fs::read_to_string(&outcome).unwrap(),
        brief: std::fs::read_to_string(sessions.join("triage/1/brief.md")).unwrap(),
        write: std::fs::read_to_string(writes.join("0001-comment.json")).unwrap(),
        attachments: entries(&root.join("attachments")),
        attachment: std::fs::read(root.join("attachments/shot.png")).unwrap(),
        root,
    };
    std::fs::OpenOptions::new()
        .write(true)
        .open(writes.join("0001-comment.json"))
        .unwrap()
        .write_all(b"{\"overwritten\"")
        .unwrap();
    seen
}

#[test_log::test(tokio::test)]
async fn each_scorer_reads_a_copy_of_only_the_parts_it_scores_removed_once_it_is_done() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let write = r#"{"body": "See ![the export](../attachments/shot.png)."}"#;
    std::fs::create_dir_all(out.join("sessions/triage/1")).unwrap();
    std::fs::write(out.join("sessions/triage/1/brief.md"), "Triage it.\n").unwrap();
    std::fs::create_dir(out.join("writes")).unwrap();
    std::fs::write(out.join("writes/0001-comment.json"), write).unwrap();
    std::fs::create_dir(out.join("attachments")).unwrap();
    std::fs::write(out.join("attachments/shot.png"), b"PNG").unwrap();
    std::fs::write(out.join("version"), "lw 1.0.0\n").unwrap();
    std::fs::write(out.join("build.log"), "built\n").unwrap();
    let harness = Arc::new(ScriptedHarness::default());
    let seen = Arc::new(Mutex::new(vec![]));
    let scored = seen.clone();
    harness.on_scorer(move |call| {
        scored.lock().unwrap().push(seen_then_overwritten(call));
        Ok(scored_as("pass", &[]))
    });

    scoring(&task, &out, under.path(), harness, |_| {})
        .await
        .unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "{seen:?}");
    for copy in seen.iter() {
        assert!(!copy.root.starts_with(&out), "{}", copy.root.display());
        assert_eq!(
            copy.entries,
            vec!["attachments", "outcome", "sessions", "writes"]
        );
        assert_eq!(copy.outcome, "done\n");
        assert_eq!(copy.brief, "Triage it.\n");
        assert_eq!(copy.write, write);
        assert_eq!(copy.attachments, vec!["shot.png"]);
        assert_eq!(copy.attachment, b"PNG");
        assert!(!copy.root.exists(), "{}", copy.root.display());
    }
    assert_eq!(
        std::fs::read_to_string(out.join("writes/0001-comment.json")).unwrap(),
        write
    );
}

#[test_log::test(tokio::test)]
async fn an_output_is_refused_naming_its_outcome_or_its_task_part() {
    let under = tempfile::tempdir().unwrap();
    let (task_dir, done) = triage_output(under.path(), "done").await;
    let task = Task::open(&task_dir).await.unwrap();
    let run = task.run().to_string();
    let changed = |name: &str, change: &dyn Fn(&Path)| {
        let copy = under.path().join(name);
        copy_dir(&done, &copy);
        change(&copy);
        copy
    };
    let interrupted = changed("interrupted", &|copy| {
        std::fs::write(copy.join("outcome"), "interrupted\n").unwrap();
    });
    let failed = changed("failed", &|copy| {
        std::fs::write(copy.join("outcome"), "failed\n").unwrap();
    });
    let no_task = changed("no-case", &|copy| {
        std::fs::remove_file(copy.join("task")).unwrap();
    });
    let other = changed("other", &|copy| {
        std::fs::write(copy.join("task"), "99/triage-1\n").unwrap();
    });
    let handed_back = changed("handed-back", &|copy| {
        std::fs::write(copy.join("outcome"), "handed-back\n").unwrap();
    });
    let holds_one = changed("judged", &|copy| {
        std::fs::write(copy.join("score.json"), "{}\n").unwrap();
    });
    let scored_as_a_dir = changed("judged-as-a-dir", &|copy| {
        std::fs::create_dir(copy.join("score.json")).unwrap();
    });
    let dangling = changed("dangling", &|copy| {
        std::os::unix::fs::symlink(copy.join("nowhere"), copy.join("score.json")).unwrap();
    });
    let mut refusals = vec![];

    for scored in [
        &interrupted,
        &failed,
        &no_task,
        &other,
        &handed_back,
        &done,
        &holds_one,
        &scored_as_a_dir,
        &dangling,
    ] {
        let opened = scorer::ScoreableTrial::open(scored, &task).await;
        refusals.push(opened.err().map(|e| e.to_string()).unwrap_or_default());
    }

    assert!(refusals[0].contains("`interrupted`"), "{}", refusals[0]);
    assert!(refusals[1].contains("`failed`"), "{}", refusals[1]);
    assert!(refusals[2].contains("no `task` part"), "{}", refusals[2]);
    assert!(refusals[3].contains("99/triage-1"), "{}", refusals[3]);
    assert!(refusals[3].contains(&run), "{}", refusals[3]);
    assert_eq!(refusals[4], "");
    assert_eq!(refusals[5], "");
    for refusal in &refusals[6..] {
        assert_eq!(refusal, "the output already holds a score, `score.json`");
    }
}

/// The branch checked out in the clone `dir`.
#[track_caller]
fn checked_out(dir: &Path) -> String {
    git_in(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .trim()
        .to_string()
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn the_scorers_clone_is_at_the_default_branch_and_its_gh_is_refused_the_tasks_repository() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let harness = Arc::new(ScriptedHarness::default());
    let seen = Arc::new(Mutex::new(vec![]));
    let kept = seen.clone();
    harness.on_scorer(move |call| {
        kept.lock().unwrap().push((
            checked_out(&call.spec.dir),
            gh_answer(call.spec, &format!("/repos/{REPO}/issues")),
        ));
        Ok(scored_as("pass", &[]))
    });

    let scored = scoring(&task, &out, under.path(), harness, |_| {}).await;

    assert!(scored.is_ok(), "{scored:?}");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].0, "main");
    assert!(seen[0].1.starts_with("HTTP/1.1 403 "), "{}", seen[0].1);
}

/// Writes the push of `branch` as the output `out` keeps it, as its write
/// number `n`.
#[track_caller]
fn kept_push(out: &Path, n: u32, branch: &str) {
    let writes = out.join("writes");
    std::fs::create_dir_all(&writes).unwrap();
    std::fs::write(
        writes.join(format!("{n:04}-push.json")),
        json!({"branch": branch, "commit": "0000000", "lease": null}).to_string(),
    )
    .unwrap();
}

#[test_log::test(tokio::test)]
async fn the_scorers_clone_has_the_last_branch_the_trial_pushed_checked_out() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let commits = out.join("commits.git");
    git_in(
        under.path(),
        &[
            "clone",
            "--bare",
            "--quiet",
            task.join("repository.git").to_str().unwrap(),
            commits.to_str().unwrap(),
        ],
    );
    let work = under.path().join("pushing");
    git_in(
        under.path(),
        &[
            "clone",
            "--quiet",
            commits.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    git_in(&work, &["checkout", "--quiet", "-b", "1-count-the-widgets"]);
    std::fs::write(work.join("count.txt"), "7\n").unwrap();
    git_in(&work, &["add", "count.txt"]);
    git_in(
        &work,
        &[
            "-c",
            "user.name=Replay",
            "-c",
            "user.email=replay@example.com",
            "commit",
            "--quiet",
            "-m",
            "Count the widgets",
        ],
    );
    git_in(&work, &["push", "--quiet", "origin", "1-count-the-widgets"]);
    kept_push(&out, 9, "1-count-the-widgets");
    kept_push(&out, 10, "1-refused");
    let harness = Arc::new(ScriptedHarness::default());
    let seen = Arc::new(Mutex::new(vec![]));
    let kept = seen.clone();
    harness.on_scorer(move |call| {
        kept.lock().unwrap().push((
            checked_out(&call.spec.dir),
            std::fs::read_to_string(call.spec.dir.join("count.txt")).ok(),
        ));
        Ok(scored_as("pass", &[]))
    });

    let scored = scoring(&task, &out, under.path(), harness, |_| {}).await;

    assert!(scored.is_ok(), "{scored:?}");
    let seen = seen.lock().unwrap();
    assert_eq!(
        *seen,
        vec![("1-count-the-widgets".to_string(), Some("7\n".to_string()))]
    );
}

/// A directory to put on the path of `lw`, holding git and bash.
#[track_caller]
fn bin_with_git() -> tempfile::TempDir {
    let bin = tempfile::tempdir().unwrap();
    link_from_path(bin.path(), "git");
    link_from_path(bin.path(), "bash");
    bin
}

/// Runs `lw score` on the task `task` and the output `out`, with `bin` on
/// its path, from a work directory of its own.
#[track_caller]
fn lw_score<'a>(bin: &'a Path, task: &'a Path, out: &'a Path) -> impl Future<Output = Output> + 'a {
    let caller = Location::caller();
    async move {
        let work =
            tempfile::tempdir().unwrap_or_else(|e| panic!("a work directory at {caller}: {e}"));
        lw_isolated(
            bin,
            work.path(),
            &["score", task.to_str().unwrap(), out.to_str().unwrap()],
            &[],
        )
        .await
    }
}

#[test_log::test(tokio::test)]
async fn lw_score_without_repo_keeps_the_score_and_prints_it() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let bin = bin_with_git();
    fake_claude(
        bin.path(),
        &scored_as("fail", &["curl https://github.com/owner/widgets"]),
    );

    let ran = lw_score(bin.path(), &task, &out).await;

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    assert_eq!(ran.stdout, "Verdict: fail\nLeak: found\n");
    let kept = score_in(&out);
    assert_eq!(kept["correctness"]["verdict"], json!("fail"));
    assert_eq!(kept["leak"]["found"], json!(true));
}

#[test_log::test(tokio::test)]
async fn lw_score_refuses_an_output_whose_run_did_not_end_done_or_handed_back() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "failed").await;
    let bin = bin_with_git();
    fake_claude(bin.path(), &scored_as("pass", &[]));

    let ran = lw_score(bin.path(), &task, &out).await;

    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert!(
        ran.stderr.contains("the output cannot be scored: ") && ran.stderr.contains("`failed`"),
        "{}",
        ran.stderr
    );
    assert!(!bin.path().join("tokens.log").exists());
    assert_eq!(entries(&out), vec!["outcome", "task"]);
}

#[test_log::test(tokio::test)]
async fn lw_score_refuses_an_output_that_holds_a_score_and_leaves_it() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let record = out.join("scorer/20261117T120000Z");
    std::fs::create_dir_all(&record).unwrap();
    std::fs::write(record.join("brief.md"), "Judge it.\n").unwrap();
    let earlier = r#"{"session": {"record": "scorer/20261117T120000Z"}}"#;
    std::fs::write(out.join("score.json"), earlier).unwrap();
    let bin = bin_with_git();
    fake_claude(bin.path(), &scored_as("pass", &[]));

    let ran = lw_score(bin.path(), &task, &out).await;

    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert!(
        ran.stderr.contains(
            "the output cannot be scored: the output already holds a score, `score.json`"
        ),
        "{}",
        ran.stderr
    );
    assert_eq!(ran.stdout, "");
    assert!(!bin.path().join("tokens.log").exists());
    assert_eq!(
        std::fs::read_to_string(out.join("score.json")).unwrap(),
        earlier
    );
    assert_eq!(scorer_records(&out), vec!["20261117T120000Z"]);
    assert_eq!(entries(&record), vec!["brief.md"]);
    assert_eq!(
        entries(&out),
        vec!["outcome", "score.json", "scorer", "task"]
    );
}

#[test_log::test(tokio::test)]
async fn lw_score_whose_scorer_gives_up_exits_1_saying_why_and_keeps_nothing() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let bin = bin_with_git();
    fake_claude(
        bin.path(),
        &json!({"outcome": "gave-up", "reason": "The output is empty."}).to_string(),
    );

    let ran = lw_score(bin.path(), &task, &out).await;

    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert!(
        ran.stderr
            .contains("The scorer gave up: The output is empty."),
        "{}",
        ran.stderr
    );
    assert_eq!(ran.stdout, "");
    assert_eq!(entries(&out), vec!["outcome", "task"]);
}

#[test_log::test(tokio::test)]
async fn a_signal_ends_lw_score_at_once_saying_why_and_keeping_nothing_of_its_scorer() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let bin = bin_with_git();
    let started = bin.path().join("claude.started");
    fake_claude_running(
        bin.path(),
        &Scenario {
            turn: vec![Step::Touch(started.clone()), Step::Hang],
            ..Scenario::default()
        },
    );
    let work = tempfile::tempdir().unwrap();
    let stderr = work.path().join("stderr.log");
    let mut scorer = lw_command(bin.path(), work.path(), work.path())
        .arg("score")
        .arg(&task)
        .arg(&out)
        .stderr(std::fs::File::create(&stderr).unwrap())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    eventually(|| started.exists()).await;
    assert_eq!(scorer_records(&out).len(), 1);

    let sent = std::process::Command::new("kill")
        .args(["-TERM", &scorer.id().unwrap().to_string()])
        .status()
        .unwrap();

    assert!(sent.success());
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), scorer.wait())
        .await
        .expect("the scorer did not end at once")
        .unwrap();
    let stderr = std::fs::read_to_string(&stderr).unwrap();
    assert_eq!(status.code(), Some(128 + 15), "{status}: {stderr}");
    assert!(stderr.contains("A signal ended the scorer."), "{stderr}");
    assert_eq!(entries(&out), vec!["outcome", "task"]);
}

#[test_log::test(tokio::test)]
async fn lw_help_lists_score() {
    let bin = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();

    let ran = lw_isolated(bin.path(), work.path(), &["--help"], &[]).await;

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    assert!(
        ran.stdout
            .lines()
            .any(|line| line.trim_start().starts_with("score ")),
        "{}",
        ran.stdout
    );
}

#[test_log::test(tokio::test)]
async fn a_scorer_of_a_trial_with_a_configuration_runs_without_it() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let harness = Arc::new(ScriptedHarness::default());
    let models = Arc::new(Mutex::new(vec![]));
    let triage_models = models.clone();
    harness.on(WorkKind::Triage, move |call| {
        triage_models
            .lock()
            .unwrap()
            .push(("triage", call.spec.model.clone()));
        Ok(triaged())
    });
    let scorer_models = models.clone();
    harness.on_scorer(move |call| {
        scorer_models
            .lock()
            .unwrap()
            .push(("scorer", call.spec.model.clone()));
        Ok(scored_as("pass", &[]))
    });
    let under = tempfile::tempdir().unwrap();
    let configuration = configuration(
        &under.path().join("configuration"),
        &[("config.toml", "model = \"configuration-model\"\n")],
    );
    let scoreable = ran_trial_with(
        &task,
        under.path(),
        harness.clone(),
        |_| {},
        Some(&configuration),
    )
    .await;

    scoring(&task, &scoreable.out, under.path(), harness, |_| {})
        .await
        .unwrap();

    assert_eq!(
        *models.lock().unwrap(),
        vec![
            ("triage", Some("configuration-model".to_string())),
            ("scorer", None),
        ]
    );
    let score = score_in(&scoreable.out);
    assert_eq!(
        score["session"]["layers"]["settings"]["model"], "harness",
        "{score}"
    );
}
