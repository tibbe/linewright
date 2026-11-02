//! Running a command with `bash`: in a process group of its own, until a
//! deadline or a cancel, its output on one pipe, redacted line by line into
//! a log.

use std::ffi::OsString;
use std::io::Write as _;
use std::os::fd::OwnedFd;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use rustix::process::{Pid, Signal};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt as _;
use tokio::net::unix::pipe;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::child;
use crate::redaction::Redaction;

/// How a command ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// It exited with this status.
    Exited(i32),
    /// A signal ended it, this one.
    Signalled(i32),
    /// Its deadline passed while it ran.
    TimedOut,
    Cancelled,
}

/// How long what the command left running has to end on SIGTERM before it
/// is sent SIGKILL.
const GRACE: Duration = Duration::from_secs(2);

pub struct Request<'a> {
    pub command: &'a str,
    /// The directory it runs in.
    pub dir: &'a Path,
    /// The whole environment the command runs in.
    pub environment: Vec<(OsString, OsString)>,
    /// The command still running then is ended, as [`Status::TimedOut`].
    pub deadline: Instant,
    /// The file its output is appended to, created if missing.
    pub log: &'a Path,
}

/// How the command ran.
#[derive(Debug)]
pub struct Ran {
    pub status: Status,
    /// What the command wrote until it ended, redacted.
    pub output: String,
    /// How long it ran, until its status was known.
    pub duration: Duration,
}

/// Runs `bash -c command` with stdin from `/dev/null` and stdout and stderr
/// on one pipe, in a process group of its own. `cancelled` ending first ends
/// the group at once, as the deadline passing does. Its output is appended
/// to the request's log as it arrives, all of it before this returns. `Err`
/// is a failure to start the command or to keep its log.
pub async fn run(
    bash: &Path,
    request: Request<'_>,
    redaction: &Redaction,
    cancelled: impl Future<Output = ()>,
) -> Result<(Ran, Leftovers)> {
    let started = Instant::now();
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(request.log)
        .context("creating the command's log")?;
    let (sender, mut receiver) = pipe::pipe().context("making the command's pipe")?;
    let writer: OwnedFd = sender
        .into_blocking_fd()
        .context("making the command's pipe")?;
    let leftovers = Leftovers {
        group: Some(Group::start(bash)?),
        drained: CancellationToken::new(),
    };
    let group = leftovers.group.as_ref().expect("just started").id;
    let mut command = child::command(bash);
    command
        .arg("-c")
        .arg(request.command)
        .current_dir(request.dir)
        .env_clear()
        .envs(request.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            writer.try_clone().context("making the command's pipe")?,
        ))
        .stderr(Stdio::from(writer))
        .process_group(group.as_raw_nonzero().get());
    let spawned = command.spawn();
    // A pipe's reader sees end-of-file only once every write end is closed:
    // https://man7.org/linux/man-pages/man7/pipe.7.html
    drop(command);
    let mut child = spawned.with_context(|| format!("starting {}", bash.display()))?;
    let mut output = Output {
        redaction,
        pending: Vec::new(),
        text: String::new(),
        log: &mut log,
    };
    let mut cancelled = std::pin::pin!(cancelled);
    let mut buffer = vec![0; 8192];
    let mut open = true;
    let status = loop {
        tokio::select! {
            biased;
            () = &mut cancelled => {
                end_at_once(group, &mut child).await;
                break Status::Cancelled;
            }
            () = tokio::time::sleep_until(request.deadline) => {
                end_at_once(group, &mut child).await;
                break Status::TimedOut;
            }
            exited = child.wait() => {
                break exit_status(exited.context("waiting for the command")?);
            }
            read = receiver.read(&mut buffer), if open => match read {
                Ok(0) => open = false,
                Ok(n) => output.take(&buffer[..n])?,
                Err(e) => return Err(e).context("reading the command's output"),
            },
        }
    };
    output.take_buffered(&receiver, &mut buffer)?;
    output.finish()?;
    let text = output.text;
    // A write to a pipe with no reader fails with `EPIPE`:
    // https://man7.org/linux/man-pages/man7/pipe.7.html
    let drained = leftovers.drained.clone();
    tokio::spawn(async move {
        tokio::select! {
            () = drained.cancelled() => {}
            () = async { while let Ok(1..) = receiver.read(&mut buffer).await {} } => {}
        }
    });
    Ok((
        Ran {
            status,
            output: text,
            duration: started.elapsed(),
        },
        leftovers,
    ))
}

fn exit_status(status: ExitStatus) -> Status {
    match (status.code(), status.signal()) {
        (Some(code), _) => Status::Exited(code),
        (None, Some(signal)) => Status::Signalled(signal),
        (None, None) => unreachable!("a process that ended exited or was signalled"),
    }
}

/// Ends `group` with SIGKILL, and reaps `child`, the command in it.
async fn end_at_once(group: Pid, child: &mut tokio::process::Child) {
    let _ = rustix::process::kill_process_group(group, Signal::KILL);
    let _ = child.wait().await;
}

/// The command's output as it arrives, kept redacted line by line.
struct Output<'a> {
    redaction: &'a Redaction,
    /// The start of a line not yet ended.
    pending: Vec<u8>,
    text: String,
    log: &'a mut std::fs::File,
}

impl Output<'_> {
    fn take(&mut self, bytes: &[u8]) -> Result<()> {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            self.keep(&line)?;
        }
        Ok(())
    }

    /// Takes what `receiver` holds now, but nothing written after.
    fn take_buffered(&mut self, receiver: &pipe::Receiver, buffer: &mut [u8]) -> Result<()> {
        let reading = "reading the command's output";
        let held = rustix::io::ioctl_fionread(receiver).context(reading)?;
        let mut left = usize::try_from(held).unwrap_or(usize::MAX);
        while left > 0 {
            let room = left.min(buffer.len());
            match rustix::io::read(receiver, &mut buffer[..room]) {
                Ok(0) | Err(rustix::io::Errno::AGAIN) => break,
                Ok(n) => {
                    self.take(&buffer[..n])?;
                    left -= n;
                }
                Err(e) => return Err(e).context(reading),
            }
        }
        Ok(())
    }

    /// Keeps the last line, which no newline ended.
    fn finish(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let mut line = std::mem::take(&mut self.pending);
        line.push(b'\n');
        self.keep(&line)
    }

    fn keep(&mut self, line: &[u8]) -> Result<()> {
        let line = self.redaction.apply(&String::from_utf8_lossy(line));
        self.log
            .write_all(line.as_bytes())
            .context("writing the command's log")?;
        self.text.push_str(&line);
        Ok(())
    }
}

/// What the command left running, in its process group. Best effort: a
/// process that left the group is beyond it.
#[derive(Debug)]
pub struct Leftovers {
    /// `None` once ended.
    group: Option<Group>,
    /// Ends the reading of what the group writes.
    drained: CancellationToken,
}

impl Leftovers {
    /// Ends the group: SIGTERM, then SIGKILL after a short grace. It blocks
    /// meanwhile.
    pub fn end(mut self) {
        if let Some(group) = self.group.take() {
            group.end(GRACE);
        }
    }
}

/// Dropped without [`Leftovers::end`], it ends the group at once, with
/// SIGKILL.
impl Drop for Leftovers {
    fn drop(&mut self) {
        if let Some(group) = self.group.take() {
            group.end(Duration::ZERO);
        }
        self.drained.cancel();
    }
}

/// The command's process group, whose id no other group takes for as long
/// as it is held.
#[derive(Debug)]
struct Group {
    leader: std::process::Child,
    id: Pid,
}

const WAIT_UNTIL_STDIN_CLOSES: &str = "read -r _";

impl Group {
    fn start(bash: &Path) -> Result<Group> {
        let leader = std::process::Command::new(bash)
            .args(["-c", WAIT_UNTIL_STDIN_CLOSES])
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .with_context(|| format!("starting {}", bash.display()))?;
        let id = Pid::from_child(&leader);
        Ok(Group { leader, id })
    }

    /// Sends the group SIGTERM, then SIGKILL to what is left of it after
    /// `grace`; SIGKILL at once without a grace.
    fn end(mut self, grace: Duration) {
        let signal = if grace.is_zero() {
            Signal::KILL
        } else {
            Signal::TERM
        };
        let _ = rustix::process::kill_process_group(self.id, signal);
        let _ = self.leader.wait();
        let deadline = std::time::Instant::now() + grace;
        while self.has_members() {
            if std::time::Instant::now() >= deadline {
                let _ = rustix::process::kill_process_group(self.id, Signal::KILL);
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn has_members(&self) -> bool {
        rustix::process::test_kill_process_group(self.id).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::Secrets;
    use crate::variable_names::VariableNames;

    /// The `bash` on the `PATH`.
    fn bash() -> std::path::PathBuf {
        std::env::var_os("PATH")
            .iter()
            .flat_map(std::env::split_paths)
            .map(|dir| dir.join("bash"))
            .find(|candidate| candidate.is_file())
            .expect("bash on the PATH")
    }

    #[test_log::test(tokio::test)]
    async fn a_secret_in_the_output_is_redacted_where_it_is_kept() {
        let secret = "sk-output-0123456789";
        let secrets = Secrets::of(
            &VariableNames::new("secrets", ["API_KEY".to_string()]).unwrap(),
            [("API_KEY".into(), secret.into())],
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let logs = tempfile::tempdir().unwrap();
        let log = logs.path().join("output.log");
        let request = Request {
            command: &format!("echo 'token {secret}'; exit 1"),
            dir: dir.path(),
            environment: vec![],
            deadline: Instant::now() + Duration::from_secs(30),
            log: &log,
        };

        let (ran, _leftovers) = run(
            &bash(),
            request,
            secrets.redaction(),
            std::future::pending(),
        )
        .await
        .unwrap();

        assert_eq!(ran.status, Status::Exited(1));
        assert_eq!(ran.output, "token [redacted]\n");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "token [redacted]\n");
    }
}
