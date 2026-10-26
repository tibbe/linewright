//! The child processes `serve` spawns.

use std::ffi::OsStr;

use tokio::process::Command;

/// A command for `program` run as a child of `serve`. It runs in a process
/// group of its own, out of the reach of the SIGINT a terminal sends
/// `serve`'s process group on Ctrl-C, which drains rather than cancelling
/// the runs in flight. It is killed if dropped before it ends.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.process_group(0).kill_on_drop(true);
    command
}
