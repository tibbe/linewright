//! What the fake `claude` does, read from `<the fake's path>.json`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The variable that names the stage a session is in, for the fake to take
/// that stage's steps from [`Scenario::stages`].
pub const STAGE_VARIABLE: &str = "FAKE_CLAUDE_STAGE";

/// The steps the fake carries out, in order, on each invocation; running out
/// of steps exits 0.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scenario {
    /// On `claude --version`.
    pub version: Vec<Step>,
    /// On any other invocation, once it has read standard input to its end.
    pub turn: Vec<Step>,
    /// In place of `turn`, the steps for the stage [`STAGE_VARIABLE`] names.
    #[serde(default)]
    pub stages: BTreeMap<String, Vec<Step>>,
    /// When given, each event streamed with no `timestamp` gets one, as
    /// Claude Code stamps its events: the time it is streamed, this many
    /// milliseconds on, so that it tells the time of a clock ahead of the
    /// system's.
    #[serde(default)]
    pub stamped_ahead_millis: Option<i64>,
}

impl Default for Scenario {
    /// Answers `--version` with `fake claude`, and does nothing on a turn.
    fn default() -> Self {
        Scenario {
            version: vec![Step::Say("fake claude".into())],
            turn: Vec::new(),
            stages: BTreeMap::new(),
            stamped_ahead_millis: None,
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
    /// Sleeps this many milliseconds.
    Sleep(u64),
    /// Writes `contents` to `path`, relative to the directory the fake runs
    /// in, and commits it there with `message`.
    Commit {
        path: PathBuf,
        contents: String,
        message: String,
    },
    /// Sleeps until killed, for at most 300 s.
    Hang,
    /// Exits with the status.
    Exit(i32),
}
