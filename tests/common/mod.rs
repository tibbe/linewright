//! Fakes of the external systems for the end-to-end tests: the local store
//! stands in for GitHub, a scripted runtime for the agents, and a clock that
//! ticks a second per reading. The log the operator reads is captured too.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use linewright::briefs::Briefs;
use linewright::clock::Clock;
use linewright::domain::{CheckState, Issue, Login, MergeMethod, MergeMethods, Number, Timestamp};
use linewright::factory::{Factory, FactoryConfig};
use linewright::reconcile::WorkKind;
use linewright::runtime::{Purpose, Runtime, SessionError, SessionSpec, Turn};
use linewright::spec_glob::SpecGlob;
use linewright::store::local::{CheckRecord, LocalStore, Seed};
use linewright::webhook::{Address, Listener, Secret, Webhooks};
use serde_json::Value;

pub const MACHINE: &str = "factory";

pub struct FakeClock {
    now: Mutex<Timestamp>,
}

impl FakeClock {
    pub fn new() -> FakeClock {
        FakeClock {
            now: Mutex::new(
                DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
        }
    }

    pub fn advance(&self, by: TimeDelta) {
        *self.now.lock().unwrap() += by;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        let mut now = self.now.lock().unwrap();
        *now += TimeDelta::seconds(1);
        *now
    }
}

/// One turn of a scripted session: the session's spec and, when resumed, the
/// follow-up prompt.
pub struct Call<'a> {
    pub spec: &'a SessionSpec,
    pub followup: Option<&'a str>,
}

type Script = Arc<dyn Fn(&Call) -> Result<String, SessionError> + Send + Sync>;

/// Which script a session runs: one per kind of work, and one for every
/// workflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ScriptKey {
    Work(WorkKind),
    Workflow,
}

impl ScriptKey {
    fn of(spec: &SessionSpec) -> ScriptKey {
        match spec.purpose {
            Purpose::Work { kind, .. } => ScriptKey::Work(kind),
            Purpose::Workflow(_) => ScriptKey::Workflow,
        }
    }
}

/// An agent runtime whose sessions run a script per kind of work.
#[derive(Default)]
pub struct ScriptedRuntime {
    scripts: Mutex<HashMap<ScriptKey, Script>>,
    prompts: Mutex<Vec<(ScriptKey, String)>>,
}

impl ScriptedRuntime {
    pub fn on(
        &self,
        kind: WorkKind,
        script: impl Fn(&Call) -> Result<String, SessionError> + Send + Sync + 'static,
    ) {
        self.script(ScriptKey::Work(kind), script);
    }

    /// The script every workflow's sessions run.
    pub fn on_workflow(
        &self,
        script: impl Fn(&Call) -> Result<String, SessionError> + Send + Sync + 'static,
    ) {
        self.script(ScriptKey::Workflow, script);
    }

    fn script(
        &self,
        key: ScriptKey,
        script: impl Fn(&Call) -> Result<String, SessionError> + Send + Sync + 'static,
    ) {
        self.scripts.lock().unwrap().insert(key, Arc::new(script));
    }

    /// Every prompt sessions of `kind` were given, follow-ups included.
    pub fn prompts(&self, kind: WorkKind) -> Vec<String> {
        self.prompts_for(ScriptKey::Work(kind))
    }

    /// Every prompt workflow sessions were given, follow-ups included.
    pub fn workflow_prompts(&self) -> Vec<String> {
        self.prompts_for(ScriptKey::Workflow)
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

    fn turn(&self, spec: &SessionSpec, followup: Option<&str>) -> Result<Turn, SessionError> {
        let key = ScriptKey::of(spec);
        self.prompts
            .lock()
            .unwrap()
            .push((key, followup.unwrap_or(&spec.prompt).to_string()));
        std::fs::write(spec.record.join("trace.jsonl"), "{\"type\":\"scripted\"}\n").unwrap();
        let script = self.scripts.lock().unwrap().get(&key).cloned();
        let script = script.ok_or_else(|| SessionError::Failed {
            reason: format!("no script for {key:?}"),
            log_tail: String::new(),
        })?;
        Ok(Turn {
            session: format!("session-{key:?}"),
            message: script(&Call { spec, followup })?,
            log_tail: String::new(),
        })
    }
}

#[async_trait]
impl Runtime for ScriptedRuntime {
    async fn start(&self, spec: &SessionSpec) -> Result<Turn, SessionError> {
        self.turn(spec, None)
    }

    async fn resume(
        &self,
        spec: &SessionSpec,
        _session: &str,
        prompt: &str,
    ) -> Result<Turn, SessionError> {
        self.turn(spec, Some(prompt))
    }
}

/// Writes `files` in the session's clone and commits them, as an agent would.
pub fn commit_in(dir: &Path, files: &[(&str, &str)], message: &str) {
    for (path, contents) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    for args in [
        vec!["add", "--all"],
        vec!["commit", "--quiet", "-m", message],
    ] {
        let output = Command::new("git")
            .args(&args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The server's log, as the operator reads it, captured on this thread.
#[derive(Clone, Default)]
pub struct Log(Arc<Mutex<Vec<u8>>>);

impl Log {
    /// Captures what this thread logs until the guard is dropped. A
    /// `#[tokio::test]` runs every task on its one thread.
    pub fn capture() -> (Log, tracing::subscriber::DefaultGuard) {
        // While just one subscriber exists, tracing caches whether a log line
        // is wanted from the subscriber of the thread that first reaches it,
        // so another test's thread would turn this one's lines off. A second
        // subscriber, kept for the whole run, makes it ask each thread's own.
        static SECOND_SUBSCRIBER: OnceLock<tracing::Dispatch> = OnceLock::new();
        SECOND_SUBSCRIBER
            .get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
        let log = Log::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        (log, tracing::subscriber::set_default(subscriber))
    }

    /// The lines logged at `level`, such as `WARN`.
    pub fn lines(&self, level: &str) -> Vec<String> {
        String::from_utf8_lossy(&self.0.lock().unwrap())
            .lines()
            .filter(|line| line.split_whitespace().nth(1) == Some(level))
            .map(str::to_string)
            .collect()
    }
}

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn json(value: Value) -> Result<String, SessionError> {
    Ok(value.to_string())
}

pub struct World {
    pub store: Arc<LocalStore>,
    pub runtime: Arc<ScriptedRuntime>,
    pub clock: Arc<FakeClock>,
    pub factory: Factory,
    pub dir: tempfile::TempDir,
}

/// The repository settings a world's store reports.
pub struct Repository {
    pub linear_history: bool,
    pub allowed_merge_methods: MergeMethods,
}

impl Repository {
    /// A repository allowing only `methods`, without linear history.
    pub fn allowing(methods: impl IntoIterator<Item = MergeMethod>) -> Repository {
        Repository {
            allowed_merge_methods: methods.into_iter().collect(),
            ..Repository::default()
        }
    }
}

impl Default for Repository {
    fn default() -> Repository {
        Repository {
            linear_history: false,
            allowed_merge_methods: MergeMethods::all(),
        }
    }
}

pub fn config(work_dir: &Path) -> FactoryConfig {
    FactoryConfig {
        spec_glob: SpecGlob::default(),
        spec_lookback: TimeDelta::days(7),
        max_workers: 4,
        ci_grace: TimeDelta::minutes(5),
        ci_timeout: TimeDelta::hours(6),
        ci_attempts: 2,
        result_retries: 1,
        session_timeout: std::time::Duration::from_secs(30 * 60),
        update_method: None,
        auto_merge: false,
        merge_method: None,
        require_label: None,
        briefs: Briefs::shipped(),
        work_dir: work_dir.to_path_buf(),
        schedule_workflows: true,
    }
}

impl World {
    pub async fn new() -> World {
        World::with(|_| {}).await
    }

    pub async fn with(adjust: impl FnOnce(&mut FactoryConfig)) -> World {
        World::with_repository(Repository::default(), adjust).await
    }

    /// A world on `repository`.
    pub async fn with_repository(
        repository: Repository,
        adjust: impl FnOnce(&mut FactoryConfig),
    ) -> World {
        World::start(repository, adjust).await.unwrap()
    }

    /// Starts a factory on a store standing in for `repository`, as startup
    /// does, failing as it fails.
    pub async fn start(
        repository: Repository,
        adjust: impl FnOnce(&mut FactoryConfig),
    ) -> anyhow::Result<World> {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(FakeClock::new());
        let store = Arc::new(
            LocalStore::create(
                &dir.path().join("store"),
                Seed {
                    default_branch: "main".into(),
                    machine_user: Login::new(MACHINE),
                    issue_types: None,
                    linear_history: repository.linear_history,
                    allowed_merge_methods: repository.allowed_merge_methods,
                },
                clock.clone(),
            )
            .await
            .unwrap(),
        );
        store
            .commit(
                "maintainer",
                "main",
                &[
                    ("README.md", "# Widgets\n"),
                    ("docs/spec.md", "# Spec\n\nWidgets exist.\n"),
                    ("src/widgets.txt", "widget\n"),
                ],
                "Start",
            )
            .await
            .unwrap();
        let runtime = Arc::new(ScriptedRuntime::default());
        let mut config = config(&dir.path().join("work"));
        adjust(&mut config);
        let factory = Factory::start(store.clone(), runtime.clone(), clock.clone(), config).await?;
        Ok(World {
            store,
            runtime,
            clock,
            factory,
            dir,
        })
    }

    /// One poll, and every run it started run to its end.
    pub async fn poll(&mut self) {
        self.factory.cycle().await.unwrap();
        self.factory.settle().await;
    }

    pub fn issue(&self, number: Number) -> Issue {
        self.store.issue(number).unwrap()
    }
}

pub fn labels(issue: &Issue) -> Vec<&str> {
    let mut labels: Vec<&str> = issue.labels.iter().map(String::as_str).collect();
    labels.sort();
    labels
}

pub fn last_comment(issue: &Issue) -> &str {
    &issue.comments.last().expect("the issue has a comment").body
}

/// A file name unique to the session's subject, such as `issue-3`.
pub fn subject_file(call: &Call) -> String {
    linewright::run::purpose_name(&call.spec.purpose)
}

/// Implementing sessions commit a file named for their subject and finish.
pub fn implement(world: &World) {
    world.runtime.on(WorkKind::Implement, |call| {
        let file = format!("src/{}.txt", subject_file(call));
        commit_in(&call.spec.dir, &[(&file, "done\n")], "Do the ticket");
        json(serde_json::json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."}))
    });
}

impl World {
    /// A ticket for the factory and the draft pull request it implemented.
    pub async fn ticket_in_review(&mut self) -> (Number, Number) {
        implement(self);
        let ticket = self.store.open_issue(
            "maintainer",
            "Do the thing",
            "Please.",
            &["ready-for-agent"],
        );
        self.store.assign_as(ticket, "maintainer").unwrap();
        self.poll().await;
        let pr = *self
            .store
            .pulls()
            .last()
            .expect("a pull request was opened");
        (ticket, pr)
    }
}

/// A stand-in for the `claude` program, written to `dir`: every turn answers
/// `message`, appends the GitHub tokens it was given to `dir/tokens.log`, its
/// temporary directory to `dir/tmpdir.log`, the shell it was told to use to
/// `dir/shell.log` and its arguments, one per line and followed by a blank
/// line, to `dir/args.log`.
pub fn fake_claude(dir: &Path, message: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let event = serde_json::json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": message,
    });
    let program = dir.join("claude");
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then echo 'fake claude'; exit 0; fi\n\
         cat > /dev/null\n\
         printf '%s\\n' \"$@\" '' >> '{args}'\n\
         echo \"GH_TOKEN=${{GH_TOKEN-}} GITHUB_TOKEN=${{GITHUB_TOKEN-}}\" >> '{tokens}'\n\
         echo \"TMPDIR=${{TMPDIR-}}\" >> '{tmpdir}'\n\
         echo \"${{CLAUDE_CODE_SHELL-}}\" >> '{shell}'\n\
         cat <<'EOF'\n{event}\nEOF\n",
        tokens = dir.join("tokens.log").display(),
        tmpdir = dir.join("tmpdir.log").display(),
        shell = dir.join("shell.log").display(),
        args = dir.join("args.log").display(),
    );
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    program
}

/// The secret the tests' webhook and listener share.
pub const SECRET: &str = "It's a Secret to Everybody";

pub fn webhooks(address: &str) -> Webhooks {
    Webhooks {
        address: address.parse::<Address>().unwrap(),
        secret: Secret::new(SECRET.into()).unwrap(),
    }
}

/// The `X-Hub-Signature-256` GitHub sends with `body`.
pub fn signature(body: &str) -> String {
    use hmac::Mac;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(body.as_bytes());
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// A delivery as it reaches the listener, with each header GitHub sends it
/// with left out when `None`.
pub struct Request<'a> {
    pub event: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub signature: Option<String>,
    pub body: String,
}

/// The listener's response to a delivery.
pub struct Response {
    pub status: reqwest::StatusCode,
    pub body: String,
}

impl<'a> Request<'a> {
    /// A delivery of `body` as `application/json`, signed with the secret,
    /// with no event.
    pub fn signed_json(body: &str) -> Request<'a> {
        Request {
            event: None,
            content_type: Some("application/json"),
            signature: Some(signature(body)),
            body: body.to_string(),
        }
    }

    /// A delivery of the JSON `payload` as GitHub sends it with the content
    /// type `application/x-www-form-urlencoded`: the value of the form field
    /// `payload`. Signed with the secret, with no event.
    pub fn signed_form(payload: &str) -> Request<'a> {
        let body = form_urlencoded::Serializer::new(String::new())
            .append_pair("payload", payload)
            .finish();
        Request {
            event: None,
            content_type: Some("application/x-www-form-urlencoded"),
            signature: Some(signature(&body)),
            body,
        }
    }

    pub async fn send(&self, listener: &Listener) -> Response {
        let mut request = reqwest::Client::new()
            .post(format!("http://{}/", listener.local_address()))
            .header("X-GitHub-Delivery", "72d3162e-cc78-11e3-81ab-4c9367dc0958")
            .body(self.body.clone());
        if let Some(signature) = &self.signature {
            request = request.header("X-Hub-Signature-256", signature);
        }
        if let Some(event) = self.event {
            request = request.header("X-GitHub-Event", event);
        }
        if let Some(content_type) = self.content_type {
            request = request.header("Content-Type", content_type);
        }
        let response = request.send().await.unwrap();
        Response {
            status: response.status(),
            body: response.text().await.unwrap(),
        }
    }
}

/// Sends a delivery of `event` signed with the secret.
pub async fn signed(listener: &Listener, event: &str, body: &str) -> reqwest::StatusCode {
    let request = Request {
        event: Some(event),
        ..Request::signed_json(body)
    };
    request.send(listener).await.status
}

/// Sends a delivery of `event` with the JSON `payload` form-encoded, as
/// GitHub sends it with the webhook's content type at its default, signed
/// with the secret.
pub async fn signed_form(listener: &Listener, event: &str, payload: &str) -> reqwest::StatusCode {
    let request = Request {
        event: Some(event),
        ..Request::signed_form(payload)
    };
    request.send(listener).await.status
}

/// A check named `name` in `state`, which logged `log`, as CI reports it on
/// a pull request's head.
pub fn check(name: &str, state: CheckState, log: &str) -> CheckRecord {
    CheckRecord {
        name: name.into(),
        state,
        log: log.into(),
    }
}
