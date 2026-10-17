#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use linewright::api::Served;
use linewright::briefs::Briefs;
use linewright::clock::Clock;
use linewright::domain::{CheckState, Issue, Login, MergeMethod, MergeMethods, Number, Timestamp};
use linewright::factory::{Factory, FactoryConfig};
use linewright::reconcile::WorkKind;
use linewright::runtime::{Purpose, Runtime, SessionError, SessionSpec, Turn, UsageLimit};
use linewright::spec_glob::SpecGlob;
use linewright::store::local::{CheckRecord, LocalStore, Seed};
use linewright::webhook::{Address, Listener, Pending, Secret, Webhooks};
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub const LINEWRIGHT: &str = "factory";

pub struct FakeClock {
    now: Mutex<Timestamp>,
    advanced: tokio::sync::Notify,
    sleeps: tokio::sync::mpsc::UnboundedSender<Timestamp>,
    slept: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<Timestamp>>,
}

impl FakeClock {
    pub fn new() -> FakeClock {
        let (sleeps, slept) = tokio::sync::mpsc::unbounded_channel();
        FakeClock {
            now: Mutex::new(
                DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            advanced: tokio::sync::Notify::new(),
            sleeps,
            slept: tokio::sync::Mutex::new(slept),
        }
    }

    pub fn advance(&self, by: TimeDelta) {
        *self.now.lock().unwrap() += by;
        self.advanced.notify_waiters();
    }

    pub async fn next_sleep(&self) -> Timestamp {
        // No timeout: under paused time it would fire while git runs.
        self.slept.lock().await.recv().await.unwrap()
    }
}

#[async_trait]
impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        let mut now = self.now.lock().unwrap();
        *now += TimeDelta::seconds(1);
        *now
    }

    async fn sleep_until(&self, at: Timestamp) {
        let _ = self.sleeps.send(at);
        loop {
            let advanced = self.advanced.notified();
            if *self.now.lock().unwrap() >= at {
                return;
            }
            advanced.await;
        }
    }
}

pub struct Call<'a> {
    pub spec: &'a SessionSpec,
    pub session: &'a str,
    pub followup: Option<&'a str>,
}

type Script = Arc<dyn Fn(&Call) -> Result<String, SessionError> + Send + Sync>;

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

#[derive(Default)]
pub struct ScriptedRuntime {
    scripts: Mutex<HashMap<ScriptKey, Script>>,
    prompts: Mutex<Vec<(ScriptKey, String)>>,
    started: std::sync::atomic::AtomicUsize,
    turn_time: Mutex<std::time::Duration>,
    held: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Option<String>>>>,
}

/// The agent of a turn held open: it says something each time it is told
/// to, and ends its turn when dropped.
pub struct HeldTurn(tokio::sync::mpsc::UnboundedSender<Option<String>>);

impl HeldTurn {
    pub fn says(&self) {
        self.0.send(None).unwrap();
    }

    pub fn writes(&self, output: &str) {
        self.0.send(Some(output.to_string())).unwrap();
    }
}

impl ScriptedRuntime {
    pub fn on(
        &self,
        kind: WorkKind,
        script: impl Fn(&Call) -> Result<String, SessionError> + Send + Sync + 'static,
    ) {
        self.script(ScriptKey::Work(kind), script);
    }

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

    async fn turn(
        &self,
        spec: &SessionSpec,
        session: String,
        followup: Option<&str>,
    ) -> Result<Turn, SessionError> {
        let time = std::mem::take(&mut *self.turn_time.lock().unwrap());
        if !time.is_zero() {
            tokio::time::sleep(time).await;
        }
        let held = self.held.lock().unwrap().take();
        if let Some(mut said) = held {
            while let Some(output) = said.recv().await {
                spec.output.heard();
                if let Some(output) = output {
                    let path = spec.record.join(linewright::transcript::OUTPUT);
                    let mut file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .unwrap();
                    std::io::Write::write_all(&mut file, output.as_bytes()).unwrap();
                }
            }
        }
        let key = ScriptKey::of(spec);
        self.prompts
            .lock()
            .unwrap()
            .push((key, followup.unwrap_or(&spec.prompt).to_string()));
        std::fs::write(
            spec.record.join("transcript.jsonl"),
            "{\"type\":\"scripted\"}\n",
        )
        .unwrap();
        spec.output.heard();
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
        let message = match tokio::runtime::Handle::current().runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| script(&call))
            }
            _ => script(&call),
        }?;
        Ok(Turn {
            session,
            message,
            log_tail: String::new(),
        })
    }
}

#[async_trait]
impl Runtime for ScriptedRuntime {
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

pub fn commit_in(dir: &Path, files: &[(&str, &str)], message: &str) {
    for (path, contents) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    git_in(dir, &["add", "--all"]);
    git_in(dir, &["commit", "--quiet", "-m", message]);
}

pub fn git_in(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
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

pub fn usage_limit_resets_at() -> Timestamp {
    "2026-10-13T07:50:00Z".parse().unwrap()
}

pub fn refused_for_the_usage_limit() -> Result<String, SessionError> {
    refused_until(usage_limit_resets_at(), None)
}

pub fn refused_part_way(call: &Call) -> Result<String, SessionError> {
    refused_until(usage_limit_resets_at(), Some(call))
}

/// The end of a turn the runtime refuses for the usage limit that resets at
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

impl Log {
    pub fn usage_limit_warnings(&self) -> Vec<String> {
        self.lines("WARN")
            .into_iter()
            .filter(|line| line.contains("usage limit") && line.contains("2026-10-13 07:50:00"))
            .collect()
    }
}

pub fn files_named(dir: &Path, name: &str) -> Vec<std::path::PathBuf> {
    let mut found = vec![];
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_named(&path, name));
        } else if path.ends_with(name) {
            found.push(path);
        }
    }
    found
}

pub fn run_ids(listed: &str) -> Vec<&str> {
    listed
        .lines()
        .map(|line| line.split_once(':').unwrap().0)
        .collect()
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

pub struct Repository {
    pub linear_history: bool,
    pub allowed_merge_methods: MergeMethods,
}

impl Repository {
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
        max_runs: 4,
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
        waits_out_usage_limit: false,
    }
}

impl World {
    pub async fn new() -> World {
        World::with(|_| {}).await
    }

    pub async fn serving() -> World {
        World::with(|config| config.waits_out_usage_limit = true).await
    }

    pub async fn with(adjust: impl FnOnce(&mut FactoryConfig)) -> World {
        World::with_repository(Repository::default(), adjust).await
    }

    pub async fn with_repository(
        repository: Repository,
        adjust: impl FnOnce(&mut FactoryConfig),
    ) -> World {
        World::start(repository, adjust).await.unwrap()
    }

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
                    linewright: Login::new(LINEWRIGHT),
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

    pub async fn restart(&mut self, adjust: impl FnOnce(&mut FactoryConfig)) {
        let mut config = config(&self.dir.path().join("work"));
        adjust(&mut config);
        self.factory = Factory::start(
            self.store.clone(),
            self.runtime.clone(),
            self.clock.clone(),
            config,
        )
        .await
        .unwrap();
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

pub fn subject_file(call: &Call) -> String {
    linewright::run::purpose_name(&call.spec.purpose)
}

/// Each run adds to what its ticket's branch holds.
pub fn implement(world: &World) {
    world.runtime.on(WorkKind::Implement, |call| {
        let file = format!("src/{}.txt", subject_file(call));
        let done = std::fs::read_to_string(call.spec.dir.join(&file)).unwrap_or_default();
        commit_in(
            &call.spec.dir,
            &[(&file, &format!("{done}done\n"))],
            "Do the ticket",
        );
        json(serde_json::json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."}))
    });
}

impl World {
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

    pub async fn persons_pull_closing(&self, issue: Number) -> Number {
        self.store
            .open_pull_as(
                "alice",
                &format!("alice-closes-{}", issue.0),
                "Do the thing",
                &format!("Closes {issue}"),
                &[("src/thing.txt", "thing\n")],
            )
            .await
            .unwrap()
    }

    pub fn rework_asked(&self, pr: Number) {
        self.store
            .comment_as("maintainer", pr, "Rename it.")
            .unwrap();
        self.runtime.on(WorkKind::Rework, |call| {
            commit_in(
                &call.spec.dir,
                &[("src/renamed.txt", "renamed\n")],
                "Rename",
            );
            json(serde_json::json!({"outcome": "done", "comment": "Renamed."}))
        });
    }
}

/// A stand-in for the `claude` program, written to `dir`: every turn answers
/// `message`, appends the GitHub tokens it was given to `dir/tokens.log`, its
/// temporary directory to `dir/tmpdir.log`, the shell it was told to use to
/// `dir/shell.log` and its arguments, one per line and followed by a blank
/// line, to `dir/args.log`. Like Claude Code under its config directory
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
    let stream: Vec<String> = events.iter().map(Value::to_string).collect();
    let stream = stream.join("\n");
    let program = dir.join("claude");
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then echo 'fake claude'; exit 0; fi\n\
         cat > /dev/null\n\
         printf '%s\\n' \"$@\" '' >> '{args}'\n\
         echo \"GH_TOKEN=${{GH_TOKEN-}} GITHUB_TOKEN=${{GITHUB_TOKEN-}}\" >> '{tokens}'\n\
         echo \"TMPDIR=${{TMPDIR-}}\" >> '{tmpdir}'\n\
         echo \"${{CLAUDE_CODE_SHELL-}}\" >> '{shell}'\n\
         take=0\n\
         for arg in \"$@\"; do\n\
           if [ $take = 1 ]; then session=$arg; fi\n\
           take=0\n\
           case $arg in --session-id|--resume) take=1;; esac\n\
         done\n\
         mkdir -p '{projects}'\n\
         echo '{{\"type\":\"said\"}}' >> \"{projects}/$session.jsonl\"\n\
         cat <<'EOF'\n{stream}\nEOF\n",
        tokens = dir.join("tokens.log").display(),
        tmpdir = dir.join("tmpdir.log").display(),
        shell = dir.join("shell.log").display(),
        args = dir.join("args.log").display(),
        projects = dir.join("projects/repo").display(),
    );
    write_program(&program, &script);
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

pub const SECRET: &str = "It's a Secret to Everybody";

pub fn webhooks(address: &str) -> Webhooks {
    Webhooks {
        address: address.parse::<Address>().unwrap(),
        secret: Secret::new(SECRET.into()).unwrap(),
    }
}

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

pub struct Response {
    pub status: reqwest::StatusCode,
    pub body: String,
}

impl<'a> Request<'a> {
    pub fn signed_json(body: &str) -> Request<'a> {
        Request {
            event: None,
            content_type: Some("application/json"),
            signature: Some(signature(body)),
            body: body.to_string(),
        }
    }

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

pub async fn signed(listener: &Listener, event: &str, body: &str) -> reqwest::StatusCode {
    let request = Request {
        event: Some(event),
        ..Request::signed_json(body)
    };
    request.send(listener).await.status
}

pub async fn signed_form(listener: &Listener, event: &str, payload: &str) -> reqwest::StatusCode {
    let request = Request {
        event: Some(event),
        ..Request::signed_form(payload)
    };
    request.send(listener).await.status
}

pub fn check(name: &str, state: CheckState, log: &str) -> CheckRecord {
    CheckRecord {
        name: name.into(),
        state,
        log: log.into(),
    }
}

pub async fn eventually(done: impl Fn() -> bool) {
    eventually_async(|| std::future::ready(done())).await;
}

pub async fn eventually_async<F: Future<Output = bool>>(done: impl Fn() -> F) {
    let waited = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !done().await {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "timed out");
}

pub struct Serving {
    stopping: CancellationToken,
    handle: JoinHandle<()>,
    pub served: tokio::sync::watch::Receiver<Option<Served>>,
}

impl Serving {
    pub fn start(
        mut factory: Factory,
        poll_interval: std::time::Duration,
        pending: Pending,
    ) -> Serving {
        let stopping = factory.stopping();
        let (sender, served) = tokio::sync::watch::channel(None);
        let handle = tokio::spawn(async move {
            factory.serve(poll_interval, &pending, &sender).await;
            factory.stop().await;
        });
        Serving {
            stopping,
            handle,
            served,
        }
    }

    pub async fn stop(self) {
        self.stopping.cancel();
        self.handle.await.unwrap();
    }

    pub async fn kill(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

pub fn rejected(reason: &str) -> String {
    format!(
        "To https://github.com/acme/widgets.git\n ! [remote rejected] HEAD -> branch ({reason})\n\
         error: failed to push some refs to 'https://github.com/acme/widgets.git'"
    )
}
