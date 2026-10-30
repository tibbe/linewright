//! The child processes `serve` spawns.

use std::ffi::OsStr;
use std::io;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::process::ExitStatus;

use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use crate::contain::{CONTROL_VARIABLE, Report};

/// A command for `program` run as a child of `serve`. It runs in a process
/// group of its own, out of the reach of the SIGINT a terminal sends
/// `serve`'s process group on Ctrl-C, which drains rather than cancelling
/// the runs in flight. It is killed if dropped before it ends.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.process_group(0).kill_on_drop(true);
    command
}

/// A command for `program` run inside a wrapper, `container contain`, that
/// ends everything `program` starts once `program` ends or is ended (see
/// [`crate::contain`]). Callers add `program`'s arguments, environment,
/// working directory and standard streams to it as they would to
/// `program`'s own command, and start it with [`ContainedCommand::spawn`].
///
/// The wrapper runs in a process group of its own, as [`command`]'s
/// programs do, but is not killed on drop: [`Contained`] ends it.
pub fn contained(container: &Path, program: impl AsRef<OsStr>) -> ContainedCommand {
    let mut command = Command::new(container);
    command
        .args(["contain", "--"])
        .arg(program)
        .process_group(0);
    ContainedCommand(command)
}

/// A command made by [`contained`], so that only a wrapper is ever waited
/// on to report on its control socket.
pub struct ContainedCommand(Command);

impl Deref for ContainedCommand {
    type Target = Command;

    fn deref(&self) -> &Command {
        &self.0
    }
}

impl DerefMut for ContainedCommand {
    fn deref_mut(&mut self) -> &mut Command {
        &mut self.0
    }
}

/// A program running inside its wrapper, and the control socket that holds
/// the wrapper back from ending it. The socket closes when this is dropped,
/// or `serve` ends however it ends, and the wrapper then ends the program
/// and everything it started.
pub struct Contained {
    wrapper: Child,
    control: UnixStream,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}

impl ContainedCommand {
    /// Starts the wrapper and its program. `Err` when the wrapper or the
    /// program could not be started, with the program's error as
    /// `Command::spawn` gives it.
    pub async fn spawn(self) -> io::Result<Contained> {
        let ContainedCommand(mut command) = self;
        // A directory of its own, which only Linewright's user can enter.
        let dir = tempfile::Builder::new().prefix("lw-contain-").tempdir()?;
        let path = dir.path().join("control");
        let listener = UnixListener::bind(&path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("listening for the wrapper on {}: {e}", path.display()),
            )
        })?;
        // Set after any `env_clear` the caller made.
        command.env(CONTROL_VARIABLE, &path);
        let mut wrapper = command.spawn()?;
        // Taken first, as `wait` closes stdin.
        let (stdin, stdout, stderr) = (
            wrapper.stdin.take(),
            wrapper.stdout.take(),
            wrapper.stderr.take(),
        );
        let wrapper_pid = wrapper.id().and_then(|id| i32::try_from(id).ok());
        let mut control = loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (control, _) = accepted?;
                    // Another process of the same user may connect too.
                    if control.peer_cred()?.pid() == wrapper_pid {
                        break control;
                    }
                }
                exited = wrapper.wait() => {
                    return Err(io::Error::other(format!(
                        "the wrapper exited ({}) before it started the program",
                        exited?
                    )));
                }
            }
        };
        drop(listener);
        drop(dir);
        let mut report = String::new();
        BufReader::new(&mut control).read_line(&mut report).await?;
        let report = Report::parse(&report);
        if report == Some(Report::Started) {
            return Ok(Contained {
                wrapper,
                control,
                stdin,
                stdout,
                stderr,
            });
        }
        let _ = wrapper.wait().await;
        Err(match report {
            Some(Report::Failed(Some(errno))) => io::Error::from_raw_os_error(errno),
            Some(Report::Failed(None)) => {
                io::Error::other("the wrapper could not start the program")
            }
            Some(Report::Started) | None => {
                io::Error::other("the wrapper ended before it started the program")
            }
        })
    }
}

impl Contained {
    /// The program's status, once the program and everything it started
    /// have ended.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.wrapper.wait().await
    }

    /// Ends the program and everything it started, and returns once they
    /// have ended.
    pub async fn end(self) {
        let Contained {
            mut wrapper,
            control,
            ..
        } = self;
        drop(control);
        let _ = wrapper.wait().await;
    }
}
