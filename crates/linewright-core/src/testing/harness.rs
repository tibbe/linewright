use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::cost::TurnSpend;
use crate::domain::Timestamp;
pub use crate::fake_claude::scenario::{Lift, Scenario, Step};

/// The variables of the scripted harness's program, in the namespaces the
/// table of sessions' variables passes.
pub const VARIABLES: Variables = Variables {
    credentials: &["CLAUDE_SCRIPTED_TOKEN"],
    headers: &["CLAUDE_SCRIPTED_HEADERS"],
    settings: &["CLAUDE_SCRIPTED_SHELL"],
};
use crate::harness::{
    GhReach, Harness, Purpose, Reported, SessionError, SessionSpec, Turn, UsageLimit, Variables,
};
use crate::reconcile::WorkKind;
use crate::session_env::Given;
use crate::work_settings::Effort;

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
    Scorer,
}

impl ScriptKey {
    fn of(spec: &SessionSpec) -> ScriptKey {
        match spec.purpose {
            Purpose::Work { kind, .. } => ScriptKey::Work(kind),
            Purpose::Automation(_) => ScriptKey::Automation,
            Purpose::Scorer => ScriptKey::Scorer,
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
    /// What each turn reports it spent, as it ends.
    spend: Mutex<Option<TurnSpend>>,
    /// The wait for the usage limit the next turn begins with, if any.
    wait: Mutex<Option<ScriptedWait>>,
}

/// A wait for the usage limit a turn begins with.
struct ScriptedWait {
    at: Timestamp,
    stated: Option<Timestamp>,
    lifted: tokio::sync::oneshot::Receiver<Timestamp>,
}

/// The usage limit a turn waits for in its turn: the wait ends, and the turn
/// carries on, once it lifts.
pub struct InTurnLimit(tokio::sync::oneshot::Sender<Timestamp>);

impl InTurnLimit {
    /// Lifts the limit `at`.
    pub fn lifts(self, at: Timestamp) {
        let _ = self.0.send(at);
    }
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

    pub fn on_scorer(
        &self,
        script: impl Fn(&Call) -> Result<String, SessionError> + Send + Sync + 'static,
    ) {
        self.script(ScriptKey::Scorer, script);
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

    /// Each turn from now on reports it spent `spend`, where until then it
    /// reports no figures.
    pub fn reports_spend(&self, spend: TurnSpend) {
        *self.spend.lock().unwrap() = Some(spend);
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

    /// Has the next turn wait for the usage limit in its turn from `at`, as
    /// a harness does that can wait, the reset time `stated` if any, until
    /// the limit returned lifts. The turn then carries on.
    pub fn waits_in_turn(&self, at: Timestamp, stated: Option<Timestamp>) -> InTurnLimit {
        let (lifts, lifted) = tokio::sync::oneshot::channel();
        *self.wait.lock().unwrap() = Some(ScriptedWait { at, stated, lifted });
        InTurnLimit(lifts)
    }

    pub fn prompts(&self, kind: WorkKind) -> Vec<String> {
        self.prompts_for(ScriptKey::Work(kind))
    }

    pub fn automation_prompts(&self) -> Vec<String> {
        self.prompts_for(ScriptKey::Automation)
    }

    pub fn scorer_prompts(&self) -> Vec<String> {
        self.prompts_for(ScriptKey::Scorer)
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

    /// A turn, its start and its end reported, unless it is dropped first.
    async fn turn(
        &self,
        spec: &SessionSpec,
        session: String,
        followup: Option<&str>,
    ) -> Result<Turn, SessionError> {
        spec.report.report(Reported::TurnStarted);
        let turn = self.unreported_turn(spec, session, followup).await;
        let spend = self.spend.lock().unwrap().clone();
        spec.report
            .report(Reported::TurnEnded(spend.unwrap_or(TurnSpend::None)));
        turn
    }

    async fn unreported_turn(
        &self,
        spec: &SessionSpec,
        session: String,
        followup: Option<&str>,
    ) -> Result<Turn, SessionError> {
        let model = self.model.lock().unwrap().clone();
        if let Some(model) = model {
            spec.report.report(Reported::Model(model));
        }
        let time = std::mem::take(&mut *self.turn_time.lock().unwrap());
        if !time.is_zero() {
            tokio::time::sleep(time).await;
        }
        let wait = self.wait.lock().unwrap().take();
        if let Some(wait) = wait {
            let (waiting, line) = spec.waits.begin(wait.at, wait.stated);
            append_output(spec, &format!("{line}\n"));
            if let Ok(at) = wait.lifted.await {
                append_output(spec, &format!("{}\n", waiting.end(at)));
            }
        }
        let held = self.held.lock().unwrap().take();
        if let Some(mut said) = held {
            while let Some(output) = said.recv().await {
                append_output(spec, &output);
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
        let script = script.ok_or_else(|| SessionError::NoResult {
            why: format!("no script for {key:?}"),
            log_tail: String::new(),
        })?;
        let call = Call {
            spec,
            session: &session,
            followup,
        };
        // A blocking script would stall the tasks queued on its worker
        // (<https://docs.rs/tokio/latest/tokio/task/fn.block_in_place.html>).
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

/// Appends `output` to the output of `spec`'s session.
fn append_output(spec: &SessionSpec, output: &str) {
    let path = spec.record.join(crate::transcript::OUTPUT);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    std::io::Write::write_all(&mut file, output.as_bytes()).unwrap();
}

/// The environment a fake harness's session starts from: `given`, then
/// `scratch` as its temporary directory and, when `gh` goes through a gate,
/// the gate's configuration as `GH_CONFIG_DIR`.
pub fn fake_environment(given: &Given, scratch: &Path, gh: &GhReach) -> Vec<(OsString, OsString)> {
    let gh_config = gh
        .config_dir()
        .map(|config| ("GH_CONFIG_DIR".into(), config.into()));
    given
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .chain([("TMPDIR".into(), scratch.into())])
        .chain(gh_config)
        .collect()
}

#[async_trait]
impl Harness for ScriptedHarness {
    fn environment(
        &self,
        given: &Given,
        scratch: &Path,
        _: Option<Effort>,
        gh: &GhReach,
    ) -> Vec<(OsString, OsString)> {
        fake_environment(given, scratch, gh)
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

/// The update `dir`'s clone is stopped in, if any.
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

/// The end of `call`, refused for the usage limit.
pub fn refused_for_the_usage_limit(call: &Call) -> Result<String, SessionError> {
    refused_until(usage_limit_resets_at(), call)
}

/// A session that stopped without a result, as `why` says.
pub fn failed_session(why: &str) -> Result<String, SessionError> {
    Err(SessionError::NoResult {
        why: why.into(),
        log_tail: String::new(),
    })
}

/// The end of `call`, refused for the usage limit that resets at
/// `resets_at`.
pub fn refused_until(resets_at: Timestamp, call: &Call) -> Result<String, SessionError> {
    Err(SessionError::UsageLimit {
        limit: UsageLimit { resets_at },
        session: call.session.to_string(),
    })
}

pub fn subject_file(call: &Call) -> String {
    crate::session::purpose_name(&call.spec.purpose)
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
