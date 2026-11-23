//! Scoring a trial's output, through the system: the scorer's session on
//! what the trial left, and the score it keeps in the output
//! (`docs/specs/benchmarks.md`, Scoring).

use std::panic::Location;
use std::path::Path;

use linewright_core::testing::scores::*;
use linewright_core::testing::trial_runs::*;
use linewright_core::testing::*;
use serde_json::json;

use crate::programs::*;
use crate::trial::{lw_command, lw_isolated};

/// What `lw --version` prints, without its newline.
#[track_caller]
fn lw_version() -> String {
    let version = std::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .arg("--version")
        .output()
        .unwrap();
    String::from_utf8(version.stdout).unwrap().trim_end().into()
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
async fn lw_score_without_a_model_exits_saying_where_to_set_it_and_keeps_nothing() {
    let under = tempfile::tempdir().unwrap();
    let (task, out) = triage_output(under.path(), "done").await;
    let bin = bin_with_git();
    fake_claude(bin.path(), &scored_as("pass", &[]));
    let work = tempfile::tempdir().unwrap();

    let ran = lw_command(bin.path(), work.path(), work.path())
        .args(["score", task.to_str().unwrap(), out.to_str().unwrap()])
        .env_remove("LINEWRIGHT_MODEL")
        .output()
        .await
        .unwrap();

    let stderr = String::from_utf8_lossy(&ran.stderr);
    assert_eq!(ran.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("`model` is required to start a session"),
        "{stderr}"
    );
    assert_eq!(scorer_records(&out), Vec::<String>::new());
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
    assert_eq!(kept["linewright"], json!(lw_version()));
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
