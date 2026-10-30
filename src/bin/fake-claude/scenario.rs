//! What a test has the fake `claude` do, which the test writes as
//! `<the fake's path>.json`. The fake includes this file as a module, and so
//! do the tests (`tests/integration/common/harness.rs`), so the two agree on
//! it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The steps the fake carries out, in order, on each invocation; running out
/// of steps exits 0.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scenario {
    /// On `claude --version`.
    pub version: Vec<Step>,
    /// On any other invocation, once it has read standard input to its end.
    pub turn: Vec<Step>,
}

impl Default for Scenario {
    /// Answers `--version` with `fake claude`, and does nothing on a turn.
    fn default() -> Self {
        Scenario {
            version: vec![Step::Say("fake claude".into())],
            turn: Vec::new(),
        }
    }
}

/// One thing the fake does.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Step {
    /// One line on standard output.
    Say(String),
    /// Each event as one JSON line on standard output.
    Stream(Vec<Value>),
    /// The text on standard error.
    Stderr(String),
    /// Appends its arguments, one per line and then a blank line, to
    /// `dir/args.log`, `GH_TOKEN=… GITHUB_TOKEN=…` to `dir/tokens.log`,
    /// `TMPDIR=…` to `dir/tmpdir.log` and `$CLAUDE_CODE_SHELL` to
    /// `dir/shell.log`, and writes its environment as `NAME=value` lines over
    /// `dir/env.log`.
    Log { dir: PathBuf },
    /// Appends `entry` as a line to `projects/<session>.jsonl`, the session
    /// being the argument after `--session-id` or `--resume`.
    Keep { projects: PathBuf, entry: Value },
    /// Waits until the path exists.
    WaitFor(PathBuf),
    /// Creates the file.
    Touch(PathBuf),
    /// Writes its process group's ID to the file.
    WriteProcessGroup(PathBuf),
    /// Starts a process in a session of its own that sleeps 300 s, and
    /// writes that process's PID to `pid`.
    LeaveInOwnSession { pid: PathBuf },
    /// Starts a process in the background that shares its standard output
    /// and sleeps 300 s.
    LeaveHoldingOutput,
    /// Sleeps until killed, for at most 300 s.
    Hang,
    /// Exits with the status.
    Exit(i32),
}
