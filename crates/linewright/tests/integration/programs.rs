//! The programs cargo builds for these tests, `lw` and the fake `claude`.
//!
//! - `Output`: the exit code, stdout and stderr of an `lw`.
//! - `lw_in`: runs `lw` on `REPO` with a work dir; `lw_asking`, asking a
//!   `serve`.
//! - `fake_claude`: a stand-in `claude` program whose turns answer a message.
//! - `fake_claude_streaming`: a stand-in `claude` program that streams events.
//! - `fake_claude_saying`: a stand-in `claude` that streams, keeps and writes errors.
//! - `fake_claude_running`: the one fake `claude`, carrying out a `Scenario`
//!   of `Step`s; `fake_claude_turning`, carrying out `Step`s on each turn.

use std::path::Path;

use linewright_core::testing::{REPO, Scenario, Step};
use serde_json::Value;

pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `lw` on `REPO` with `args` and the work dir `work_dir`, from a
/// directory of its own.
pub async fn lw_in(work_dir: &Path, args: &[&str]) -> Output {
    let runtime_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    lw_asking(work_dir, runtime_dir.path(), cwd.path(), args).await
}

/// Runs `lw` on `REPO` with `args` and the work dir `work_dir`, from the
/// directory `cwd`, asking the `serve` whose socket is in `runtime_dir`.
pub async fn lw_asking(work_dir: &Path, runtime_dir: &Path, cwd: &Path, args: &[&str]) -> Output {
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .args(["--repo", REPO, "--work-dir"])
        .arg(work_dir)
        .args(args)
        .current_dir(cwd)
        .env("XDG_RUNTIME_DIR", runtime_dir)
        .output()
        .await
        .unwrap();
    Output {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// A stand-in for the `claude` program, written to `dir`: every turn answers
/// `message`, appends the GitHub tokens it was given to `dir/tokens.log`, its
/// temporary directory to `dir/tmpdir.log`, the shell it was told to use to
/// `dir/shell.log` and its arguments, one per line and followed by a blank
/// line, to `dir/args.log`, and writes the environment it was spawned with
/// over `dir/env.log`.
/// Like Claude Code under its config directory
/// (<https://code.claude.com/docs/en/sessions>), it keeps each session's
/// transcript as `dir/projects/repo/<session>.jsonl`.
pub fn fake_claude(dir: &Path, message: &str) -> std::path::PathBuf {
    fake_claude_streaming(
        dir,
        &[serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": message,
        })],
    )
}

pub fn fake_claude_streaming(dir: &Path, events: &[Value]) -> std::path::PathBuf {
    fake_claude_saying(dir, events, &serde_json::json!({"type": "said"}), "")
}

/// A Claude Code that streams `events`, adds `kept` to the session's
/// transcript and writes `errors` to standard error.
pub fn fake_claude_saying(
    dir: &Path,
    events: &[Value],
    kept: &Value,
    errors: &str,
) -> std::path::PathBuf {
    let mut turn = vec![
        Step::Log { dir: dir.into() },
        Step::Keep {
            projects: dir.join("projects/repo"),
            entry: kept.clone(),
        },
    ];
    if !errors.is_empty() {
        turn.push(Step::Stderr(format!("{errors}\n")));
    }
    turn.push(Step::Stream(events.to_vec()));
    fake_claude_turning(dir, turn)
}

/// The one fake `claude`, as `dir/claude`, carrying out `turn` on each turn.
pub fn fake_claude_turning(dir: &Path, turn: Vec<Step>) -> std::path::PathBuf {
    fake_claude_running(
        dir,
        &Scenario {
            turn,
            ..Scenario::default()
        },
    )
}

/// The one fake `claude`, as `dir/claude`, carrying out `scenario`, which it
/// finds beside it, as `dir/claude.json`.
pub fn fake_claude_running(dir: &Path, scenario: &Scenario) -> std::path::PathBuf {
    let program = dir.join("claude");
    std::fs::write(
        dir.join("claude.json"),
        serde_json::to_string(scenario).unwrap(),
    )
    .unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_fake-claude"), &program).unwrap();
    program
}
