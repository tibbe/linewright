use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use linewright::domain::Timestamp;
use linewright::harness::{
    Harness, Purpose, Reported, SessionError, SessionSpec, Turn, UsageLimit,
};
use linewright::reconcile::WorkKind;
use linewright::session_env::Given;
use linewright::work_settings::Effort;
use serde_json::Value;

#[path = "../../../src/bin/fake-claude/scenario.rs"]
mod scenario;
pub use scenario::{Scenario, Step};

pub struct Call<'a> {
    pub spec: &'a SessionSpec,
    pub session: &'a str,
    pub followup: Option<&'a str>,
}

type Script = Arc<dyn Fn(&Call) -> Result<String, SessionError> + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ScriptKey {
    Work(WorkKind),
    Automation,
}

impl ScriptKey {
    fn of(spec: &SessionSpec) -> ScriptKey {
        match spec.purpose {
            Purpose::Work { kind, .. } => ScriptKey::Work(kind),
            Purpose::Automation(_) => ScriptKey::Automation,
        }
    }
}

#[derive(Default)]
pub struct ScriptedHarness {
    scripts: Mutex<HashMap<ScriptKey, Script>>,
    prompts: Mutex<Vec<(ScriptKey, String)>>,
    /// The update each session's clone was left stopped in as it started.
    stopped: Mutex<Vec<(ScriptKey, Stopped)>>,
    started: std::sync::atomic::AtomicUsize,
    turn_time: Mutex<std::time::Duration>,
    held: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<String>>>,
    /// The model each turn reports it runs, as the turn starts; `None`
    /// reports none.
    model: Mutex<Option<String>>,
}

/// The agent of a turn held open: it writes output each time it is told
/// to, and ends its turn when dropped.
pub struct HeldTurn(tokio::sync::mpsc::UnboundedSender<String>);

impl HeldTurn {
    pub fn writes(&self, output: &str) {
        self.0.send(output.to_string()).unwrap();
    }
}

impl ScriptedHarness {
    pub fn on(
        &self,
        kind: WorkKind,
        script: impl Fn(&Call) -> Result<String, SessionError> + Send + Sync + 'static,
    ) {
        self.script(ScriptKey::Work(kind), script);
    }

    pub fn on_automation(
        &self,
        script: impl Fn(&Call) -> Result<String, SessionError> + Send + Sync + 'static,
    ) {
        self.script(ScriptKey::Automation, script);
    }

    fn script(
        &self,
        key: ScriptKey,
        script: impl Fn(&Call) -> Result<String, SessionError> + Send + Sync + 'static,
    ) {
        self.scripts.lock().unwrap().insert(key, Arc::new(script));
    }

    /// Each turn from now on reports it runs `model`.
    pub fn reports_model(&self, model: &str) {
        *self.model.lock().unwrap() = Some(model.to_string());
    }

    pub fn takes(&self, time: std::time::Duration) {
        *self.turn_time.lock().unwrap() = time;
    }

    /// Holds the next turn open, its agent producing output as the turn
    /// returned tells it to, over time, until that is dropped.
    pub fn hold(&self) -> HeldTurn {
        let (says, said) = tokio::sync::mpsc::unbounded_channel();
        *self.held.lock().unwrap() = Some(said);
        HeldTurn(says)
    }

    pub fn prompts(&self, kind: WorkKind) -> Vec<String> {
        self.prompts_for(ScriptKey::Work(kind))
    }

    pub fn automation_prompts(&self) -> Vec<String> {
        self.prompts_for(ScriptKey::Automation)
    }

    fn prompts_for(&self, key: ScriptKey) -> Vec<String> {
        self.prompts
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| *k == key)
            .map(|(_, p)| p.clone())
            .collect()
    }

    /// The update each session of `kind` found its clone stopped in as it
    /// started, for those that found one.
    pub fn stopped(&self, kind: WorkKind) -> Vec<Stopped> {
        self.stopped
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| *k == ScriptKey::Work(kind))
            .map(|(_, stopped)| *stopped)
            .collect()
    }

    async fn turn(
        &self,
        spec: &SessionSpec,
        session: String,
        followup: Option<&str>,
    ) -> Result<Turn, SessionError> {
        let model = self.model.lock().unwrap().clone();
        if let Some(model) = model {
            spec.report.report(Reported { model });
        }
        let time = std::mem::take(&mut *self.turn_time.lock().unwrap());
        if !time.is_zero() {
            tokio::time::sleep(time).await;
        }
        let held = self.held.lock().unwrap().take();
        if let Some(mut said) = held {
            while let Some(output) = said.recv().await {
                let path = spec.record.join(linewright::transcript::OUTPUT);
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .unwrap();
                std::io::Write::write_all(&mut file, output.as_bytes()).unwrap();
            }
        }
        let key = ScriptKey::of(spec);
        if followup.is_none()
            && let Some(stopped) = stopped_in(&spec.dir)
        {
            self.stopped.lock().unwrap().push((key, stopped));
        }
        self.prompts
            .lock()
            .unwrap()
            .push((key, followup.unwrap_or(&spec.prompt).to_string()));
        std::fs::write(
            spec.record.join("transcript.jsonl"),
            "{\"type\":\"scripted\"}\n",
        )
        .unwrap();
        let script = self.scripts.lock().unwrap().get(&key).cloned();
        let script = script.ok_or_else(|| SessionError::Failed {
            reason: format!("no script for {key:?}"),
            log_tail: String::new(),
        })?;
        let call = Call {
            spec,
            session: &session,
            followup,
        };
        // A script may block, waiting for its test to let it go on. On a
        // multi-threaded runtime it gives up its worker meanwhile, else the
        // tasks queued on that worker wait with it.
        let flavor = tokio::runtime::Handle::current().runtime_flavor();
        let message = if flavor == tokio::runtime::RuntimeFlavor::MultiThread {
            tokio::task::block_in_place(|| script(&call))
        } else {
            script(&call)
        }?;
        Ok(Turn {
            session,
            message,
            log_tail: String::new(),
        })
    }
}

/// The environment a fake harness's session starts from: `given`, then
/// `scratch` as its temporary directory.
pub fn fake_environment(given: &Given, scratch: &Path) -> Vec<(OsString, OsString)> {
    given
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .chain([("TMPDIR".into(), scratch.into())])
        .collect()
}

#[async_trait]
impl Harness for ScriptedHarness {
    fn environment(
        &self,
        given: &Given,
        scratch: &Path,
        _: Option<Effort>,
    ) -> Vec<(OsString, OsString)> {
        fake_environment(given, scratch)
    }

    async fn start(&self, spec: &SessionSpec) -> Result<Turn, SessionError> {
        let n = self
            .started
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.turn(spec, format!("session-{}", n + 1), None).await
    }

    async fn resume(
        &self,
        spec: &SessionSpec,
        session: &str,
        prompt: &str,
    ) -> Result<Turn, SessionError> {
        self.turn(spec, session.to_string(), Some(prompt)).await
    }
}

/// An update of a clone that stopped on conflicts for its session to finish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    Rebase,
    Merge,
}

/// The update `dir`'s clone is stopped in, if any. One process says where
/// each of git's markers would be: `rev-parse` prints a line per
/// `--git-path`.
fn stopped_in(dir: &Path) -> Option<Stopped> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--path-format=absolute"])
        .args(["--git-path", "rebase-merge"])
        .args(["--git-path", "rebase-apply"])
        .args(["--git-path", "MERGE_HEAD"])
        .output()
        .unwrap();
    let paths = String::from_utf8(output.stdout).unwrap();
    let exists: Vec<bool> = paths.lines().map(|path| Path::new(path).exists()).collect();
    match exists[..] {
        [true, _, _] | [_, true, _] => Some(Stopped::Rebase),
        [false, false, true] => Some(Stopped::Merge),
        [false, false, false] => None,
        _ => panic!("git rev-parse printed {paths:?}"),
    }
}

pub fn usage_limit_resets_at() -> Timestamp {
    "2026-10-13T07:50:00Z".parse().unwrap()
}

pub fn refused_for_the_usage_limit() -> Result<String, SessionError> {
    refused_until(usage_limit_resets_at(), None)
}

/// A session that ended unexpectedly, for `reason`.
pub fn failed_session(reason: &str) -> Result<String, SessionError> {
    Err(SessionError::Failed {
        reason: reason.into(),
        log_tail: String::new(),
    })
}

pub fn refused_part_way(call: &Call) -> Result<String, SessionError> {
    refused_until(usage_limit_resets_at(), Some(call))
}

/// The end of a turn the harness refuses for the usage limit that resets at
/// `resets_at`: part way through `call`, or before the turn began.
pub fn refused_until(
    resets_at: Timestamp,
    part_way: Option<&Call>,
) -> Result<String, SessionError> {
    Err(SessionError::UsageLimit {
        limit: UsageLimit { resets_at },
        underway: part_way.map(|call| call.session.to_string()),
    })
}

pub fn subject_file(call: &Call) -> String {
    linewright::run::purpose_name(&call.spec.purpose)
}

/// A stand-in for the `claude` program, written to `dir`: every turn answers
/// `message`, appends the GitHub tokens it was given to `dir/tokens.log`, its
/// temporary directory to `dir/tmpdir.log`, the shell it was told to use to
/// `dir/shell.log` and its arguments, one per line and followed by a blank
/// line, to `dir/args.log`, and writes its environment over `dir/env.log`.
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
    fake_claude_running(
        dir,
        &Scenario {
            turn,
            ..Scenario::default()
        },
    )
}

/// The one fake `claude`, as `dir/claude`, carrying out `scenario`: a
/// symlink to it, which is not a new program for the OS to check on its
/// first run, and a scenario it finds beside it, as `dir/claude.json`.
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

/// Writes the program `path` with `script` in it. A child process writes
/// it, as Linux refuses to run a program that is open for writing
/// (`ETXTBSY`), and a process another test starts holds open each file the
/// tests' process has open until it starts its own program.
pub fn write_program(path: &Path, script: &str) {
    let source = path.with_extension("source");
    std::fs::write(&source, script).unwrap();
    let status = Command::new("install")
        .args(["-m", "755"])
        .arg(&source)
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "install {} failed", path.display());
}

pub fn rejected(reason: &str) -> String {
    format!(
        "To https://github.com/acme/widgets.git\n ! [remote rejected] HEAD -> branch ({reason})\n\
         error: failed to push some refs to 'https://github.com/acme/widgets.git'"
    )
}

/// An automation's answer that it found nothing to do.
pub fn nothing_needs_doing() -> String {
    serde_json::json!({"summary": "Nothing needs doing.", "issues": []}).to_string()
}

/// Asserts that `group`, a process group as `ps -o pgid=` prints it, is not
/// the tests' own.
pub fn assert_in_a_process_group_of_its_own(group: &str) {
    let output = Command::new("ps")
        .args(["-o", "pgid=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    let own = String::from_utf8(output.stdout).unwrap();
    assert_ne!(group.trim(), "");
    assert_ne!(group.trim(), own.trim());
}
