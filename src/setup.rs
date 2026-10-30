//! Running a session's setup command in its clone, as the session starts
//! (`docs/specs/runs.md`, Setup): in a process group of its own, its output
//! on one pipe, redacted line by line into the session's record, and its
//! start and end in the session's transcript.

use std::ffi::OsString;
use std::io::Write as _;
use std::os::fd::OwnedFd;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use rustix::process::{Pid, Signal};
use tokio::io::AsyncReadExt as _;
use tokio::net::unix::pipe;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::child;
use crate::domain::Timestamp;
use crate::redaction::Redaction;
use crate::transcript;
use crate::work_settings::SetupCommand;

pub use crate::transcript::SetupStatus as Status;

/// How long what the command left running has to end on SIGTERM before it
/// is sent SIGKILL.
const GRACE: Duration = Duration::from_secs(2);

pub struct Request<'a> {
    pub command: &'a SetupCommand,
    /// The clone's root.
    pub dir: &'a Path,
    /// The whole environment the command runs in.
    pub environment: Vec<(OsString, OsString)>,
    /// The session's deadline: the command still running then is ended, as
    /// the session timing out.
    pub deadline: Instant,
    /// When it starts.
    pub started_at: Timestamp,
    /// The session's record directory.
    pub record: &'a Path,
}

/// How the command ran.
#[derive(Debug)]
pub struct Ran {
    pub status: Status,
    /// What the command wrote until it ended, redacted.
    pub output: String,
}

/// Runs `bash -c command` with stdin from `/dev/null` and stdout and stderr
/// on one pipe, in a process group of its own. `cancelled` ending first ends
/// the group at once, as the session's deadline passing does. Its start and
/// end are appended to [`transcript::OUTPUT`] in the record, and its output
/// to [`transcript::SETUP_OUTPUT`] as it arrives, all of it before its end.
/// `Err` is a failure to start the command or to keep its record.
pub async fn run(
    bash: &Path,
    request: Request<'_>,
    redaction: &Redaction,
    cancelled: impl Future<Output = ()>,
) -> Result<(Ran, Leftovers)> {
    let started = Instant::now();
    let redacted = redaction.apply(request.command.as_str());
    let mut transcript = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(request.record.join(transcript::OUTPUT))
        .context("creating the session's transcript")?;
    writeln!(
        transcript,
        "{}",
        transcript::setup_started(request.started_at, &redacted)
    )
    .context("writing the setup command's start")?;
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(request.record.join(transcript::SETUP_OUTPUT))
        .context("creating the setup command's log")?;
    let (sender, mut receiver) = pipe::pipe().context("making the setup command's pipe")?;
    let writer: OwnedFd = sender
        .into_blocking_fd()
        .context("making the setup command's pipe")?;
    // Ends the group from here on, even if this is dropped part way.
    let leftovers = Leftovers {
        group: Some(Group::start(bash)?),
        drained: CancellationToken::new(),
    };
    let group = leftovers.group.as_ref().expect("just started").id;
    let mut command = child::command(bash);
    command
        .arg("-c")
        .arg(request.command.as_str())
        .current_dir(request.dir)
        .env_clear()
        .envs(request.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            writer
                .try_clone()
                .context("making the setup command's pipe")?,
        ))
        .stderr(Stdio::from(writer))
        .process_group(group.as_raw_nonzero().get());
    let spawned = command.spawn();
    // The command holds the pipe's write end until it is dropped, and the
    // pipe ends only once every writer has closed it.
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
                break exit_status(exited.context("waiting for the setup command")?);
            }
            read = receiver.read(&mut buffer), if open => match read {
                Ok(0) => open = false,
                Ok(n) => output.take(&buffer[..n])?,
                Err(e) => return Err(e).context("reading the setup command's output"),
            },
        }
    };
    // What the command wrote before it ended is in the pipe.
    output.take_buffered(&receiver, &mut buffer)?;
    output.finish()?;
    let text = output.text;
    // A process left running can still write to the pipe: it is read and
    // dropped until the group ends, so that the process gets no `EPIPE`.
    let drained = leftovers.drained.clone();
    tokio::spawn(async move {
        tokio::select! {
            () = drained.cancelled() => {}
            () = async { while let Ok(1..) = receiver.read(&mut buffer).await {} } => {}
        }
    });
    let ran_for = started.elapsed();
    let ended_at =
        request.started_at + chrono::TimeDelta::from_std(ran_for).unwrap_or(chrono::TimeDelta::MAX);
    writeln!(
        transcript,
        "{}",
        transcript::setup_ended(ended_at, status, ran_for.as_secs_f64())
    )
    .context("writing the setup command's end")?;
    Ok((
        Ran {
            status,
            output: text,
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

    /// Takes what `receiver` holds now, but nothing written after. It reads
    /// the descriptor, which does not block, as the runtime may not have
    /// seen the output arrive yet.
    fn take_buffered(&mut self, receiver: &pipe::Receiver, buffer: &mut [u8]) -> Result<()> {
        let reading = "reading the setup command's output";
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
            .context("writing the setup command's log")?;
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

/// The command's process group. Its leader is a process of Linewright's
/// that waits for as long as the group is held, so that no other group
/// takes its id meanwhile, as one could once the command and every process
/// it left had exited.
#[derive(Debug)]
struct Group {
    leader: std::process::Child,
    id: Pid,
}

impl Group {
    fn start(bash: &Path) -> Result<Group> {
        // It waits for a line that never comes on a pipe Linewright holds
        // open.
        let leader = std::process::Command::new(bash)
            .args(["-c", "read -r _"])
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
    use crate::work_settings::Setup;

    /// The `bash` on the `PATH`.
    fn bash() -> std::path::PathBuf {
        std::env::var_os("PATH")
            .iter()
            .flat_map(std::env::split_paths)
            .map(|dir| dir.join("bash"))
            .find(|candidate| candidate.is_file())
            .expect("bash on the PATH")
    }

    #[tokio::test]
    async fn a_secret_in_the_command_or_its_output_is_redacted_where_it_is_kept() {
        let secret = "sk-setup-0123456789";
        let secrets = Secrets::of(
            &VariableNames::new("secrets", ["API_KEY".to_string()]).unwrap(),
            [("API_KEY".into(), secret.into())],
        )
        .unwrap();
        let Setup::Command(command) = Setup::from(format!("echo 'token {secret}'; exit 1")) else {
            panic!("a command");
        };
        let clone = tempfile::tempdir().unwrap();
        let record = tempfile::tempdir().unwrap();
        let request = Request {
            command: &command,
            dir: clone.path(),
            environment: vec![],
            deadline: Instant::now() + Duration::from_secs(30),
            started_at: "2026-10-01T12:00:00Z".parse().unwrap(),
            record: record.path(),
        };

        let (ran, _leftovers) = run(
            &bash(),
            request,
            secrets.redaction(),
            std::future::pending(),
        )
        .await
        .unwrap();

        assert_eq!(ran.output, "token [redacted]\n");
        let log = std::fs::read_to_string(record.path().join(transcript::SETUP_OUTPUT));
        assert_eq!(log.unwrap(), "token [redacted]\n");
        let transcript = std::fs::read_to_string(record.path().join(transcript::OUTPUT)).unwrap();
        assert!(
            transcript.contains("echo 'token [redacted]'; exit 1"),
            "{transcript}"
        );
        assert!(!transcript.contains(secret), "{transcript}");
    }
}
