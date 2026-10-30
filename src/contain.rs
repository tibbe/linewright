//! `lw contain -- PROGRAM ARGS…`: the wrapper [`crate::child::Contained`]
//! runs a program in, so that nothing the program starts outlives it. Once
//! the program exits, or `serve` closes the control socket, it ends every
//! process under it, those that left the program's process group and
//! session included.
//!
//! On Linux it is a child subreaper (`PR_SET_CHILD_SUBREAPER` in
//! <https://man7.org/linux/man-pages/man2/PR_SET_CHILD_SUBREAPER.2const.html>),
//! so a process whose parent exits is re-parented to it rather than to PID 1,
//! and stays within its reach. Elsewhere it reaches only the processes whose
//! parents are still running.

use std::ffi::{OsStr, OsString};
use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, WaitOptions, WaitStatus};

/// The variable that tells the wrapper where `serve` listens for it.
pub const CONTROL_VARIABLE: &str = "LINEWRIGHT_CONTAIN_CONTROL";

/// How long the processes left have to end on SIGTERM before they are sent
/// SIGKILL.
const GRACE: Duration = Duration::from_secs(2);

/// The variable macOS's CoreFoundation adds to the environment of the
/// process it loads into, here the wrapper's, when it is missing. The
/// wrapper does not pass it on, so that the program's environment is the one
/// `serve` gave; a program that loads CoreFoundation sets it again itself.
const CF_TEXT_ENCODING: &str = "__CF_USER_TEXT_ENCODING";

/// What the wrapper exits with when it cannot run the program, as `env`
/// does when it fails itself.
const CANNOT_CONTAIN: u8 = 125;

/// What `serve` tells the wrapper on the control socket once it has checked
/// that the wrapper is its peer, and before the wrapper starts the program.
/// The wrapper waits for it, so that it is still connected while it is
/// checked. `serve` sends nothing after it: the socket's end, however
/// `serve` closes it or ends, is the word to end.
pub const GO_AHEAD: &str = "go\n";

/// What the wrapper tells `serve` on the control socket, in one line, once
/// it has had the [`GO_AHEAD`] and tried to start the program.
#[derive(Debug, PartialEq, Eq)]
pub enum Report {
    Started,
    /// With the OS's error number, if it gave one.
    Failed(Option<i32>),
}

impl Report {
    pub fn line(&self) -> String {
        match self {
            Report::Started => "started\n".into(),
            Report::Failed(Some(errno)) => format!("failed {errno}\n"),
            Report::Failed(None) => "failed\n".into(),
        }
    }

    pub fn parse(line: &str) -> Option<Report> {
        match line.trim_end() {
            "started" => Some(Report::Started),
            "failed" => Some(Report::Failed(None)),
            line => match line.split_once(' ')? {
                ("failed", errno) => Some(Report::Failed(Some(errno.parse().ok()?))),
                _ => None,
            },
        }
    }
}

/// Runs `program` with `args`, which inherit the wrapper's standard streams,
/// working directory and environment, and reports on the control socket
/// whether it started. Returns, once every process under the wrapper has
/// ended, as the program did: with its exit code, or by the signal that
/// ended it.
pub fn main(program: &OsStr, args: &[OsString]) -> ExitCode {
    let Some(path) = std::env::var_os(CONTROL_VARIABLE) else {
        eprintln!("{}: `contain` is for Linewright's own use", crate::COMMAND);
        return ExitCode::from(CANNOT_CONTAIN);
    };
    let mut control = match reach_serve(&path) {
        Ok(control) => control,
        Err(e) => {
            eprintln!("{}: reaching serve: {e}", crate::COMMAND);
            return ExitCode::from(CANNOT_CONTAIN);
        }
    };
    let me = rustix::process::getpid();
    let started = become_subreaper(me)
        .map_err(std::io::Error::from)
        .and_then(|()| {
            let mut command = Command::new(program);
            command.args(args).env_remove(CONTROL_VARIABLE);
            if cfg!(target_os = "macos") {
                command.env_remove(CF_TEXT_ENCODING);
            }
            command.spawn()
        });
    let report = match &started {
        Ok(_) => Report::Started,
        Err(e) => Report::Failed(e.raw_os_error()),
    };
    let _ = control.write_all(report.line().as_bytes());
    let Ok(child) = started else {
        return ExitCode::from(CANNOT_CONTAIN);
    };
    let pid = Pid::from_child(&child);
    std::thread::spawn(move || {
        let _ = control.read_to_end(&mut Vec::new());
        end_all(me, Reaping::No);
    });
    let status = wait_for(pid);
    end_all(me, Reaping::Yes);
    match status {
        Ok(status) => exit_as(status),
        Err(e) => {
            eprintln!("{}: waiting for the program: {e}", crate::COMMAND);
            ExitCode::FAILURE
        }
    }
}

/// The control socket at `path`, once `serve` has given the [`GO_AHEAD`].
fn reach_serve(path: &OsStr) -> std::io::Result<UnixStream> {
    let mut control = UnixStream::connect(path)?;
    let mut word = [0; GO_AHEAD.len()];
    control.read_exact(&mut word)?;
    if word != GO_AHEAD.as_bytes() {
        return Err(std::io::Error::other(format!(
            "expected the go-ahead, got {:?}",
            String::from_utf8_lossy(&word)
        )));
    }
    Ok(control)
}

#[cfg(target_os = "linux")]
fn become_subreaper(me: Pid) -> rustix::io::Result<()> {
    rustix::process::set_child_subreaper(Some(me))
}

#[cfg(not(target_os = "linux"))]
fn become_subreaper(_: Pid) -> rustix::io::Result<()> {
    Ok(())
}

/// The program's status, reaping meanwhile the processes re-parented to the
/// wrapper as they end.
#[cfg(target_os = "linux")]
fn wait_for(program: Pid) -> rustix::io::Result<WaitStatus> {
    loop {
        if let Some((pid, status)) = rustix::process::wait(WaitOptions::empty())?
            && pid == program
        {
            return Ok(status);
        }
    }
}

/// The program's status. No other process is re-parented to the wrapper,
/// and the `ps` it runs is reaped by its own wait.
#[cfg(not(target_os = "linux"))]
fn wait_for(program: Pid) -> rustix::io::Result<WaitStatus> {
    loop {
        if let Some((_, status)) = rustix::process::waitpid(Some(program), WaitOptions::empty())? {
            return Ok(status);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reaping {
    /// Reaps the processes re-parented to the wrapper, and returns only
    /// once it has no child left.
    Yes,
    /// Leaves them to the wrapper's main thread, which waits for the
    /// program, and returns once every process under it has ended or been
    /// sent SIGKILL.
    No,
}

/// Sends every process under `me` SIGTERM, and SIGKILL to those left after
/// [`GRACE`], again and again, so that one started meanwhile ends too.
fn end_all(me: Pid, reaping: Reaping) {
    signal_all(&descendants(&running(), me), Signal::TERM);
    let deadline = Instant::now() + GRACE;
    loop {
        let children_left = reaping == Reaping::Yes && reap();
        let left = descendants(&running(), me);
        if left.is_empty() && !children_left {
            return;
        }
        if Instant::now() >= deadline {
            signal_all(&left, Signal::KILL);
            if reaping == Reaping::No {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn signal_all(processes: &[Pid], signal: Signal) {
    for &pid in processes {
        let _ = rustix::process::kill_process(pid, signal);
    }
}

/// Reaps the wrapper's children that have ended. Whether it has a child
/// left.
#[cfg(target_os = "linux")]
fn reap() -> bool {
    loop {
        match rustix::process::wait(WaitOptions::NOHANG) {
            Ok(Some(_)) => {}
            Ok(None) => return true,
            Err(_) => return false,
        }
    }
}

/// Only the program is the wrapper's child, and [`wait_for`] has reaped it.
/// A `wait` here could take the status of the `ps` another thread runs.
#[cfg(not(target_os = "linux"))]
fn reap() -> bool {
    false
}

/// The program's status, as the wrapper's own.
fn exit_as(status: WaitStatus) -> ExitCode {
    if let Some(code) = status.exit_status() {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    let signal = status.terminating_signal().unwrap_or(0);
    if let Some(raised) = Signal::from_named_raw(signal) {
        let _ = rustix::process::kill_process(rustix::process::getpid(), raised);
    }
    // A signal the wrapper ignores, such as SIGPIPE, which Rust ignores: a
    // shell's way of saying it.
    ExitCode::from(128_u8.saturating_add(u8::try_from(signal).unwrap_or(0)))
}

/// A running process: its ID and its parent's.
struct Process {
    pid: Pid,
    parent: Option<Pid>,
}

/// Of `processes`, every one under `me`.
fn descendants(processes: &[Process], me: Pid) -> Vec<Pid> {
    let mut found = vec![me];
    let mut at = 0;
    while at < found.len() {
        let parent = found[at];
        found.extend(
            processes
                .iter()
                .filter(|process| process.parent == Some(parent))
                .map(|process| process.pid),
        );
        at += 1;
    }
    found.remove(0);
    found
}

/// The running processes, from `/proc`. Ended ones not yet reaped are left
/// out.
#[cfg(target_os = "linux")]
fn running() -> Vec<Process> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let pid = Pid::from_raw(entry.ok()?.file_name().to_str()?.parse().ok()?)?;
            from_stat(
                pid,
                &std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?,
            )
        })
        .collect()
}

/// The process `pid`, from its `/proc/PID/stat`, unless it has ended:
/// <https://man7.org/linux/man-pages/man5/proc_pid_stat.5.html>.
#[cfg(target_os = "linux")]
fn from_stat(pid: Pid, stat: &str) -> Option<Process> {
    // The name, in parentheses, can hold anything, a `)` included.
    let mut fields = stat[stat.rfind(')')? + 1..].split_whitespace();
    let state = fields.next()?;
    let parent = Pid::from_raw(fields.next()?.parse().ok()?);
    (state != "Z" && state != "X").then_some(Process { pid, parent })
}

/// The running processes, from `ps`, leaving out the `ps` itself. Ended
/// ones not yet reaped are left out.
#[cfg(not(target_os = "linux"))]
fn running() -> Vec<Process> {
    let Ok(ps) = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,stat="])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let ps_pid = Pid::from_child(&ps);
    let Ok(output) = ps.wait_with_output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(from_ps_line)
        .filter(|process| process.pid != ps_pid)
        .collect()
}

/// The process on a line of `ps -o pid=,ppid=,stat=`, unless it has ended:
/// <https://man.freebsd.org/cgi/man.cgi?ps(1)>.
#[cfg(not(target_os = "linux"))]
fn from_ps_line(line: &str) -> Option<Process> {
    let mut fields = line.split_whitespace();
    let pid = Pid::from_raw(fields.next()?.parse().ok()?)?;
    let parent = Pid::from_raw(fields.next()?.parse().ok()?);
    let state = fields.next()?;
    (!state.starts_with('Z')).then_some(Process { pid, parent })
}
