//! Judging a replay's output, through the system: the judge's session on
//! what the replay left, and the judgement it keeps in the output
//! (`docs/specs/benchmarks.md`, Judging).

use crate::common;
use crate::replay::{entries, gh_answer, lw_command, lw_isolated, triage_case};

use std::io::Write as _;
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::*;
use linewright::case::Case;
use linewright::factory::FactoryConfig;
use linewright::harness::SessionError;
use linewright::replay::judge::{self, JudgeRecord, Judgement, Unjudged};
use linewright::work_settings::{Effort, Partial, StageKey, StageTable};
use serde_json::{Value, json};

/// A judge's result: the verdict `verdict`, and a leak found when `calls`
/// quotes any.
fn judged_as(verdict: &str, calls: &[&str]) -> String {
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

/// A triage case of a new world, kept in `under/case`, and the output of a
/// replay of it whose run ended `outcome`, written in `under/out` as a
/// replay leaves it: the ID of the case's run and the outcome.
#[track_caller]
pub(crate) fn triage_output<'a>(
    under: &'a Path,
    outcome: &'a str,
) -> impl Future<Output = (PathBuf, PathBuf)> + 'a {
    let caller = Location::caller();
    async move {
        let mut world = World::new().await;
        let (_, captured) = triage_case(&mut world).await;
        let case = under.join("case");
        copy_dir(&captured, &case);
        let run = Case::open(&case)
            .await
            .unwrap_or_else(|e| panic!("opening the case at {caller}: {e}"))
            .run()
            .to_string();
        let out = under.join("out");
        std::fs::create_dir(&out).unwrap();
        std::fs::write(out.join("case"), format!("{run}\n")).unwrap();
        std::fs::write(out.join("outcome"), format!("{outcome}\n")).unwrap();
        (case, out)
    }
}

/// Copies the directory `from` to `to`, as `cp -R` does.
#[track_caller]
fn copy_dir(from: &Path, to: &Path) {
    let copied = std::process::Command::new("cp")
        .arg("-R")
        .arg(from)
        .arg(to)
        .status()
        .unwrap();
    assert!(copied.success());
}

/// Judges the output `out` of a replay of the case in `case` with the
/// judges `harness` scripts and the factory config `adjust` makes of the
/// tests' own, its work directory under `under`, keeping the judgement it
/// makes, and gives its summary, and waits for what it leaves to remove.
#[track_caller]
fn judging<'a>(
    case: &'a Path,
    out: &'a Path,
    under: &'a Path,
    harness: Arc<ScriptedHarness>,
    adjust: impl FnOnce(&mut FactoryConfig) + 'a,
) -> impl Future<Output = Result<String, Unjudged>> + 'a {
    let caller = Location::caller();
    async move {
        let case = Case::open(case)
            .await
            .unwrap_or_else(|e| panic!("opening the case at {caller}: {e}"));
        let replayed = judge::Replayed::open(out, &case)
            .await
            .unwrap_or_else(|e| panic!("opening the output at {caller}: {e}"));
        let record = JudgeRecord::make(&replayed)
            .unwrap_or_else(|e| panic!("making the judge's record at {caller}: {e}"));
        let mut config = config(&under.join("work").join(REPO));
        adjust(&mut config);
        let winding_up = config.winding_up.clone();
        let judged = judge::run(&case, &replayed, record, harness, config).await;
        let kept = judged.map(|judgement: Judgement| {
            let summary = judgement.summary();
            judge::keep(&replayed, judgement)
                .unwrap_or_else(|e| panic!("keeping the judgement at {caller}: {e}"));
            summary
        });
        winding_up.wait().await;
        kept
    }
}

/// The judgement kept in the output `out`.
#[track_caller]
fn judgement_in(out: &Path) -> Value {
    let text = std::fs::read_to_string(out.join("judgement.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The names in the judges' directory of the output `out`.
#[track_caller]
fn judge_records(out: &Path) -> Vec<String> {
    let dir = out.join("judge");
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
async fn a_judge_keeps_its_judgement_in_the_output_and_a_new_one_replaces_it() {
    let under = tempfile::tempdir().unwrap();
    let (case, out) = triage_output(under.path(), "done").await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.reports_model("claude-judge-1");
    harness.on_judge(|_| Ok(judged_as("pass", &[])));
    let judge_settings = |config: &mut FactoryConfig| {
        config.work.stages.insert(
            StageKey::Judge,
            StageTable {
                settings: Partial {
                    model: Some("judge-model".into()),
                    effort: Some(Effort::High),
                    session_timeout: None,
                },
                ..StageTable::default()
            },
        );
    };

    let first = judging(&case, &out, under.path(), harness.clone(), judge_settings).await;

    assert_eq!(first.unwrap(), "Verdict: pass\nLeak: none found\n");
    let kept = judgement_in(&out);
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
    assert!(record.starts_with("judge/"), "{record}");
    assert!(out.join(&record).join("brief.md").is_file(), "{record}");
    assert_eq!(kept["session"]["model"], json!("judge-model"));
    assert_eq!(kept["session"]["effort"], json!("high"));
    assert_eq!(kept["session"]["reported_model"], json!("claude-judge-1"));
    assert_eq!(
        kept["session"]["layers"]["settings"]["model"],
        json!("operator")
    );
    assert_eq!(harness.judge_prompts().len(), 1);

    harness.on_judge(|_| {
        Ok(judged_as(
            "fail",
            &["curl https://github.com/owner/widgets"],
        ))
    });
    let second = judging(&case, &out, under.path(), harness, |_| {}).await;

    assert_eq!(second.unwrap(), "Verdict: fail\nLeak: found\n");
    let replaced = judgement_in(&out);
    assert_eq!(replaced["correctness"]["verdict"], json!("fail"));
    assert_eq!(
        replaced["leak"],
        json!({
            "reasoning": "I read every transcript.",
            "found": true,
            "calls": ["curl https://github.com/owner/widgets"],
        })
    );
    let new_record = replaced["session"]["record"].as_str().unwrap();
    assert_ne!(new_record, record);
    assert!(!out.join(&record).exists(), "{record}");
    assert_eq!(
        judge_records(&out),
        vec![new_record.strip_prefix("judge/").unwrap().to_string()]
    );
    assert_eq!(
        entries(&out),
        vec!["case", "judge", "judgement.json", "outcome"]
    );
}

#[test_log::test(tokio::test)]
async fn a_judge_that_makes_no_judgement_keeps_nothing_and_leaves_the_earlier_one() {
    let under = tempfile::tempdir().unwrap();
    let (case, out) = triage_output(under.path(), "handed-back").await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.on_judge(|_| Ok(judged_as("pass", &[])));
    judging(&case, &out, under.path(), harness.clone(), |_| {})
        .await
        .unwrap();
    let earlier = std::fs::read(out.join("judgement.json")).unwrap();
    let records = judge_records(&out);

    harness.on_judge(|_| {
        Ok(json!({"outcome": "gave-up", "reason": "The output is empty."}).to_string())
    });
    let gave_up = judging(&case, &out, under.path(), harness.clone(), |_| {}).await;
    harness.on_judge(|_| {
        Ok(json!({
            "outcome": "judged",
            "correctness": {"reasoning": "It passes.", "verdict": "pass"},
            "leak": {"reasoning": "It read the pull request.", "found": true},
        })
        .to_string())
    });
    let unmatched = judging(&case, &out, under.path(), harness, |config| {
        config.result_retries = 1;
    })
    .await;

    let gave_up = gave_up.expect_err("a judge that gives up makes no judgement");
    assert_eq!(
        gave_up.to_string(),
        "the judge gave up: The output is empty."
    );
    let unmatched = unmatched.expect_err("a result never accepted is no judgement");
    assert!(unmatched.to_string().contains("/leak"), "{unmatched}");
    assert_eq!(std::fs::read(out.join("judgement.json")).unwrap(), earlier);
    assert_eq!(judge_records(&out), records);
}

#[test_log::test(tokio::test)]
async fn a_judges_session_that_fails_on_its_infrastructure_is_retried_once() {
    let under = tempfile::tempdir().unwrap();
    let (case, out) = triage_output(under.path(), "done").await;
    let harness = Arc::new(ScriptedHarness::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    harness.on_judge(move |_| {
        if counted.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(SessionError::Infrastructure("the harness lost it".into()));
        }
        Ok(judged_as("pass", &[]))
    });

    let judged = judging(&case, &out, under.path(), harness, |_| {}).await;

    assert!(judged.is_ok(), "{judged:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(judge_records(&out).len(), 1);
}

/// The path the judge's prompt `prompt` gives after `label`.
#[track_caller]
fn named_in(prompt: &str, label: &str) -> PathBuf {
    let line = prompt
        .lines()
        .find_map(|line| line.strip_prefix(&format!("- {label}: `")))
        .unwrap_or_else(|| panic!("the prompt names no {label}:\n{prompt}"));
    let path = line
        .strip_suffix('`')
        .unwrap_or_else(|| panic!("the {label} is not quoted whole:\n{prompt}"));
    PathBuf::from(path)
}

/// What a judge saw of the replay in its copy, from the paths its prompt
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

/// What the judge called with `call` sees in its copy of the replay, which
/// it then overwrites the copy's first write of in place.
#[track_caller]
fn seen_then_overwritten(call: &Call) -> Copied {
    let prompt = &call.spec.prompt;
    let outcome = named_in(prompt, "How its run ended");
    let sessions = named_in(
        prompt,
        "Each session's record, with its brief and its transcript",
    );
    let writes = named_in(
        prompt,
        "What Linewright would have written to GitHub, in order",
    );
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
async fn each_judge_reads_a_copy_of_only_the_parts_it_judges_removed_once_it_is_done() {
    let under = tempfile::tempdir().unwrap();
    let (case, out) = triage_output(under.path(), "done").await;
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
    let first = seen.clone();
    harness.on_judge(move |call| {
        first.lock().unwrap().push(seen_then_overwritten(call));
        Ok(judged_as("pass", &[]))
    });
    judging(&case, &out, under.path(), harness.clone(), |_| {})
        .await
        .unwrap();
    let second = seen.clone();
    harness.on_judge(move |call| {
        second.lock().unwrap().push(seen_then_overwritten(call));
        Ok(json!({"outcome": "gave-up", "reason": "The output is empty."}).to_string())
    });

    let regave = judging(&case, &out, under.path(), harness, |_| {}).await;

    assert!(regave.is_err(), "{regave:?}");
    assert!(out.join("judgement.json").is_file());
    assert_eq!(judge_records(&out).len(), 1);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "{seen:?}");
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
async fn an_output_is_refused_naming_its_outcome_or_its_case_part() {
    let under = tempfile::tempdir().unwrap();
    let (case_dir, done) = triage_output(under.path(), "done").await;
    let case = Case::open(&case_dir).await.unwrap();
    let run = case.run().to_string();
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
    let no_case = changed("no-case", &|copy| {
        std::fs::remove_file(copy.join("case")).unwrap();
    });
    let other = changed("other", &|copy| {
        std::fs::write(copy.join("case"), "99/triage-1\n").unwrap();
    });
    let handed_back = changed("handed-back", &|copy| {
        std::fs::write(copy.join("outcome"), "handed-back\n").unwrap();
    });
    let mut refusals = vec![];

    for judged in [&interrupted, &failed, &no_case, &other, &handed_back, &done] {
        let opened = judge::Replayed::open(judged, &case).await;
        refusals.push(opened.err().map(|e| e.to_string()).unwrap_or_default());
    }

    assert!(refusals[0].contains("`interrupted`"), "{}", refusals[0]);
    assert!(refusals[1].contains("`failed`"), "{}", refusals[1]);
    assert!(refusals[2].contains("no `case` part"), "{}", refusals[2]);
    assert!(refusals[3].contains("99/triage-1"), "{}", refusals[3]);
    assert!(refusals[3].contains(&run), "{}", refusals[3]);
    assert_eq!(refusals[4], "");
    assert_eq!(refusals[5], "");
}

/// The branch checked out in the clone `dir`.
#[track_caller]
fn checked_out(dir: &Path) -> String {
    git_in(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .trim()
        .to_string()
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn the_judges_clone_is_at_the_default_branch_and_its_gh_is_refused_the_cases_repository() {
    let under = tempfile::tempdir().unwrap();
    let (case, out) = triage_output(under.path(), "done").await;
    let harness = Arc::new(ScriptedHarness::default());
    let seen = Arc::new(Mutex::new(vec![]));
    let kept = seen.clone();
    harness.on_judge(move |call| {
        kept.lock().unwrap().push((
            checked_out(&call.spec.dir),
            gh_answer(call.spec, &format!("/repos/{REPO}/issues")),
        ));
        Ok(judged_as("pass", &[]))
    });

    let judged = judging(&case, &out, under.path(), harness, |_| {}).await;

    assert!(judged.is_ok(), "{judged:?}");
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
async fn the_judges_clone_has_the_last_branch_the_replay_pushed_checked_out() {
    let under = tempfile::tempdir().unwrap();
    let (case, out) = triage_output(under.path(), "done").await;
    let commits = out.join("commits.git");
    git_in(
        under.path(),
        &[
            "clone",
            "--bare",
            "--quiet",
            case.join("repository.git").to_str().unwrap(),
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
    harness.on_judge(move |call| {
        kept.lock().unwrap().push((
            checked_out(&call.spec.dir),
            std::fs::read_to_string(call.spec.dir.join("count.txt")).ok(),
        ));
        Ok(judged_as("pass", &[]))
    });

    let judged = judging(&case, &out, under.path(), harness, |_| {}).await;

    assert!(judged.is_ok(), "{judged:?}");
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

/// Runs `lw judge` on the case `case` and the output `out`, with `bin` on
/// its path, from a work directory of its own.
#[track_caller]
fn lw_judge<'a>(bin: &'a Path, case: &'a Path, out: &'a Path) -> impl Future<Output = Output> + 'a {
    let caller = Location::caller();
    async move {
        let work =
            tempfile::tempdir().unwrap_or_else(|e| panic!("a work directory at {caller}: {e}"));
        lw_isolated(
            bin,
            work.path(),
            &["judge", case.to_str().unwrap(), out.to_str().unwrap()],
            &[],
        )
        .await
    }
}

#[test_log::test(tokio::test)]
async fn lw_judge_without_repo_keeps_the_judgement_and_prints_it() {
    let under = tempfile::tempdir().unwrap();
    let (case, out) = triage_output(under.path(), "done").await;
    let bin = bin_with_git();
    fake_claude(
        bin.path(),
        &judged_as("fail", &["curl https://github.com/owner/widgets"]),
    );

    let ran = lw_judge(bin.path(), &case, &out).await;

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    assert_eq!(ran.stdout, "Verdict: fail\nLeak: found\n");
    let kept = judgement_in(&out);
    assert_eq!(kept["correctness"]["verdict"], json!("fail"));
    assert_eq!(kept["leak"]["found"], json!(true));
}

#[test_log::test(tokio::test)]
async fn lw_judge_refuses_an_output_whose_run_did_not_end_done_or_handed_back() {
    let under = tempfile::tempdir().unwrap();
    let (case, out) = triage_output(under.path(), "failed").await;
    let bin = bin_with_git();
    fake_claude(bin.path(), &judged_as("pass", &[]));

    let ran = lw_judge(bin.path(), &case, &out).await;

    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert!(
        ran.stderr.contains("the output cannot be judged: ") && ran.stderr.contains("`failed`"),
        "{}",
        ran.stderr
    );
    assert!(!bin.path().join("tokens.log").exists());
    assert_eq!(entries(&out), vec!["case", "outcome"]);
}

#[test_log::test(tokio::test)]
async fn lw_judge_whose_judge_gives_up_exits_1_saying_why_and_keeps_nothing() {
    let under = tempfile::tempdir().unwrap();
    let (case, out) = triage_output(under.path(), "done").await;
    let bin = bin_with_git();
    fake_claude(
        bin.path(),
        &json!({"outcome": "gave-up", "reason": "The output is empty."}).to_string(),
    );

    let ran = lw_judge(bin.path(), &case, &out).await;

    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert!(
        ran.stderr
            .contains("the judge gave up: The output is empty."),
        "{}",
        ran.stderr
    );
    assert_eq!(ran.stdout, "");
    assert_eq!(entries(&out), vec!["case", "outcome"]);
}

#[test_log::test(tokio::test)]
async fn a_signal_ends_lw_judge_at_once_saying_why_and_keeping_nothing_of_its_judge() {
    let under = tempfile::tempdir().unwrap();
    let (case, out) = triage_output(under.path(), "done").await;
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
    let mut judge = lw_command(bin.path(), work.path(), work.path())
        .arg("judge")
        .arg(&case)
        .arg(&out)
        .stderr(std::fs::File::create(&stderr).unwrap())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    eventually(|| started.exists()).await;
    assert_eq!(judge_records(&out).len(), 1);

    let sent = std::process::Command::new("kill")
        .args(["-TERM", &judge.id().unwrap().to_string()])
        .status()
        .unwrap();

    assert!(sent.success());
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), judge.wait())
        .await
        .expect("the judge did not end at once")
        .unwrap();
    let stderr = std::fs::read_to_string(&stderr).unwrap();
    assert_eq!(status.code(), Some(128 + 15), "{status}: {stderr}");
    assert!(
        stderr.contains("the judge made no judgement: a signal ended the command"),
        "{stderr}"
    );
    assert_eq!(entries(&out), vec!["case", "outcome"]);
}

#[test_log::test(tokio::test)]
async fn lw_help_lists_judge() {
    let bin = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();

    let ran = lw_isolated(bin.path(), work.path(), &["--help"], &[]).await;

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    assert!(
        ran.stdout
            .lines()
            .any(|line| line.trim_start().starts_with("judge ")),
        "{}",
        ran.stdout
    );
}
