//! The stand-in for Claude Code's `claude` that the integration tests run.
//! A test symlinks a path of its own to it and writes the [`Scenario`] it is
//! to carry out beside that path, as `<path>.json`: the harness runs it with
//! an environment and arguments of its own, but by the path the test chose.

mod scenario;

use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context as _, bail};
use scenario::{Scenario, Step};

/// The first argument the fake starts itself again with, to stand in for a
/// process left in a session of its own; the second names the file it
/// writes its PID to once it is in that session.
const OWN_SESSION: &str = "--fake-claude-own-session";

/// The first argument the fake starts itself again with, to stand in for a
/// process left behind that only sleeps.
const SLEEP: &str = "--fake-claude-sleep";

/// How long a process the fake leaves, or the fake hanging, sleeps.
const LEFT_FOR: Duration = Duration::from_secs(300);

/// How often the fake looks for a file it waits for.
const POLL: Duration = Duration::from_millis(10);

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os();
    let invoked = args.next().context("no argv[0]")?;
    let args: Vec<OsString> = args.collect();
    match args.first().and_then(|arg| arg.to_str()) {
        Some(OWN_SESSION) => {
            let pid = args.get(1).context("no PID file")?;
            return in_own_session(Path::new(pid));
        }
        Some(SLEEP) => {
            std::thread::sleep(LEFT_FOR);
            return Ok(());
        }
        _ => {}
    }
    let scenario = scenario(&invoked)?;
    let steps = if args.first().is_some_and(|arg| arg == "--version") {
        scenario.version
    } else {
        std::io::copy(&mut std::io::stdin(), &mut std::io::sink())?;
        scenario.turn
    };
    for step in steps {
        if let Some(status) = carry_out(step, &args)? {
            std::process::exit(status);
        }
    }
    Ok(())
}

/// The scenario beside the path the fake was invoked by: `invoked` itself
/// when it holds a `/`, and otherwise the first file of that name on `PATH`,
/// as the caller found it.
fn scenario(invoked: &OsStr) -> anyhow::Result<Scenario> {
    let path = if invoked.as_bytes().contains(&b'/') {
        PathBuf::from(invoked)
    } else {
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path)
            .map(|dir| dir.join(invoked))
            .find(|candidate| candidate.is_file())
            .with_context(|| format!("no {} on PATH", invoked.display()))?
    };
    let mut json = path.into_os_string();
    json.push(".json");
    let json = PathBuf::from(json);
    let text = std::fs::read_to_string(&json)
        .with_context(|| format!("reading the scenario {}", json.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing the scenario {}", json.display()))
}

/// Carries out `step`, the fake having been given `args`, returning the
/// status to exit with if the step ends the fake.
fn carry_out(step: Step, args: &[OsString]) -> anyhow::Result<Option<i32>> {
    match step {
        Step::Say(line) => say(&line)?,
        Step::Stream(events) => {
            for event in events {
                say(&event.to_string())?;
            }
        }
        Step::Stderr(text) => {
            let mut stderr = std::io::stderr();
            stderr.write_all(text.as_bytes())?;
            stderr.flush()?;
        }
        Step::Log { dir } => log(&dir, args)?,
        Step::Keep { projects, entry } => {
            let session = args
                .windows(2)
                .filter(|pair| pair[0] == "--session-id" || pair[0] == "--resume")
                .map(|pair| &pair[1])
                .next_back()
                .context("no --session-id or --resume")?;
            std::fs::create_dir_all(&projects)?;
            let mut name = session.clone();
            name.push(".jsonl");
            append(&projects.join(name), format!("{entry}\n").as_bytes())?;
        }
        Step::WaitFor(path) => {
            while !path.exists() {
                std::thread::sleep(POLL);
            }
        }
        Step::Touch(path) => append(&path, b"")?,
        Step::WriteProcessGroup(path) => {
            let group = rustix::process::getpgrp().as_raw_nonzero();
            std::fs::write(&path, format!("{group}\n"))?;
        }
        Step::LeaveInOwnSession { pid } => leave_in_own_session(&pid)?,
        Step::LeaveHoldingOutput => {
            Command::new(std::env::current_exe()?)
                .arg(SLEEP)
                .stdin(Stdio::null())
                .spawn()?;
        }
        Step::Hang => std::thread::sleep(LEFT_FOR),
        Step::Exit(status) => return Ok(Some(status)),
    }
    Ok(None)
}

/// Writes `line` to standard output, flushed, so that whoever reads it sees
/// it while the fake runs on.
fn say(line: &str) -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}")?;
    stdout.flush()
}

/// Logs what the fake was given, as [`Step::Log`] says.
fn log(dir: &Path, args: &[OsString]) -> anyhow::Result<()> {
    let var = |name| std::env::var_os(name).unwrap_or_default();
    let mut logged = Vec::new();
    for arg in args {
        logged.extend_from_slice(&line(&[arg.as_bytes()]));
    }
    logged.push(b'\n');
    append(&dir.join("args.log"), &logged)?;
    let tokens = line(&[
        b"GH_TOKEN=",
        var("GH_TOKEN").as_bytes(),
        b" GITHUB_TOKEN=",
        var("GITHUB_TOKEN").as_bytes(),
    ]);
    append(&dir.join("tokens.log"), &tokens)?;
    append(
        &dir.join("tmpdir.log"),
        &line(&[b"TMPDIR=", var("TMPDIR").as_bytes()]),
    )?;
    append(
        &dir.join("shell.log"),
        &line(&[var("CLAUDE_CODE_SHELL").as_bytes()]),
    )?;
    let env: Vec<u8> = std::env::vars_os()
        .flat_map(|(name, value)| line(&[name.as_bytes(), b"=", value.as_bytes()]))
        .collect();
    std::fs::write(dir.join("env.log"), env)?;
    Ok(())
}

/// `pieces` joined, as one line.
fn line(pieces: &[&[u8]]) -> Vec<u8> {
    [pieces, &[b"\n"]].concat().concat()
}

/// Appends `bytes` to the file `path`, creating it if missing.
fn append(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .with_context(|| format!("appending to {}", path.display()))
}

/// Starts the fake again in a session of its own, sharing its standard
/// output and error, and waits until that has written its PID to `pid`.
fn leave_in_own_session(pid: &Path) -> anyhow::Result<()> {
    let mut left = Command::new(std::env::current_exe()?)
        .arg(OWN_SESSION)
        .arg(pid)
        .stdin(Stdio::null())
        .spawn()?;
    while !pid.exists() {
        if let Some(status) = left.try_wait()? {
            bail!("the process left in its own session exited first, {status}");
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}

/// Moves into a session of its own, writes its PID to `pid` and sleeps. The
/// PID is written whole, under another name first, so the fake that waits
/// for it never reads part of it.
fn in_own_session(pid: &Path) -> anyhow::Result<()> {
    rustix::process::setsid()?;
    let mut writing = pid.as_os_str().to_owned();
    writing.push(".writing");
    std::fs::write(&writing, format!("{}\n", std::process::id()))?;
    std::fs::rename(&writing, pid)?;
    std::thread::sleep(LEFT_FOR);
    Ok(())
}
