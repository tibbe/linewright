use std::path::Path;
use std::sync::Arc;

use chrono::TimeDelta;
use linewright::api::{Publish, Served};
use linewright::briefs::OperatorPrompts;
use linewright::domain::{
    AutoMerge, CheckState, Issue, Login, MergeMethod, MergeMethods, Number, Windows, label,
};
use linewright::factory::{Factory, FactoryConfig, ServeEnd, SessionToken};
use linewright::reconcile::{Subject, WorkKind};
use linewright::redaction::Redaction;
use linewright::run::subject_name;
use linewright::runs::{Ending, Runs, State};
use linewright::secrets::Secrets;
use linewright::spec_glob::SpecGlob;
use linewright::status::{FailedPolls, Polling, View};
use linewright::store::Store;
use linewright::store::local::{CheckRecord, LocalStore, Paused, Seed, Write};
use linewright::variable_names::VariableNames;
use linewright::webhook::Pending;
use linewright::work_settings::{Environments, Partial, Setup, WorkSettings};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::back_door::{ASKS_FOR_REWORK, RENAME, RENAMED, RENAMED_COMMENT};
use super::cli::REPO;
use super::clock::FakeClock;
use super::files::commit_in;
use super::harness::{ScriptedHarness, failed_session, subject_file};

pub const LINEWRIGHT: &str = "factory";

/// Opens an issue `author` filed, which Linewright then labeled `labels`, as
/// it labels an issue it triages or hands back: it has worked it.
pub fn open_issue_linewright_labeled(
    store: &LocalStore,
    author: &str,
    title: &str,
    body: &str,
    labels: &[&str],
) -> Number {
    let issue = store.open_issue(author, title, body, &[]);
    for label in labels {
        store.label_as(LINEWRIGHT, issue, label).unwrap();
    }
    issue
}

/// The label a factory requires when set to require one.
pub const REQUIRED: &str = "linewright";

/// Auto-merge on, by the repository's merge method.
pub const AUTO_MERGE: Option<AutoMerge> = Some(AutoMerge { merge_method: None });

/// The secrets of an environment holding `variables`, each named in
/// `secrets`.
pub fn secrets_of(variables: &[(&str, &str)]) -> Secrets {
    let names = VariableNames::new(
        "secrets",
        variables.iter().map(|(name, _)| name.to_string()),
    )
    .unwrap();
    Secrets::of(
        &names,
        variables
            .iter()
            .map(|(name, value)| (name.into(), value.into())),
    )
    .unwrap()
}

/// The secrets of an environment holding `variables`, with no `secrets`
/// setting.
pub fn secrets_unnamed(variables: &[(&str, &str)]) -> Secrets {
    Secrets::of(
        &VariableNames::default(),
        variables
            .iter()
            .map(|(name, value)| (name.into(), value.into())),
    )
    .unwrap()
}

/// The redaction of an environment holding `variables`.
pub fn redaction_of(variables: &[(&str, &str)]) -> Redaction {
    secrets_of(variables).redaction().clone()
}

pub struct World {
    pub store: Arc<LocalStore>,
    pub harness: Arc<ScriptedHarness>,
    pub clock: Arc<FakeClock>,
    pub factory: Factory,
    pub dir: tempfile::TempDir,
}

/// A tracking issue and its closed ticket.
pub struct Tracking {
    pub tracking: Number,
    pub closed: Number,
}

pub struct Repository {
    pub linear_history: bool,
    pub allowed_merge_methods: MergeMethods,
    /// GitHub tokens that cannot read the repository.
    pub unreadable_by: Vec<&'static str>,
    /// The labels the repository has before Linewright starts.
    pub labels: Vec<label::Label>,
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
            unreadable_by: vec![],
            labels: vec![],
        }
    }
}

pub fn config(work_dir: &Path) -> FactoryConfig {
    FactoryConfig {
        spec_glob: SpecGlob::default(),
        spec_lookback: TimeDelta::days(7),
        done_window: TimeDelta::days(7),
        max_runs: 4,
        ci_grace: TimeDelta::minutes(5),
        ci_timeout: TimeDelta::hours(6),
        ci_attempts: 2,
        result_retries: 1,
        bash: bash(),
        work: WorkSettings {
            file: Partial {
                session_timeout: Some(std::time::Duration::from_mins(30)),
                ..Partial::default()
            },
            ..WorkSettings::default()
        },
        environments: Environments::default(),
        update_method: None,
        auto_merge: None,
        require_label: None,
        prompts: OperatorPrompts::read(None).unwrap(),
        work_dir: work_dir.to_path_buf(),
        schedule_automations: true,
        waits_out_usage_limit: false,
        session_token: SessionToken::Withheld,
        secrets: Secrets::default(),
        ends: linewright::ending::Ends::new(),
    }
}

/// The `bash` on the path.
pub fn bash() -> std::path::PathBuf {
    std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join("bash"))
        .find(|candidate| candidate.is_file())
        .expect("`bash` is on the path")
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
                    repo: REPO.parse().unwrap(),
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
        for token in repository.unreadable_by {
            store.refuse_reads_by(token);
        }
        for label in &repository.labels {
            Store::create_label(&*store, label).await.unwrap();
        }
        let harness = Arc::new(ScriptedHarness::default());
        let mut config = config(&dir.path().join("work"));
        adjust(&mut config);
        let factory = Factory::start(store.clone(), harness.clone(), clock.clone(), config).await?;
        Ok(World {
            store,
            harness,
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
            self.harness.clone(),
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

/// How each of `world`'s runs ended, failing if one is not over.
pub fn ended_runs(world: &World) -> Vec<Ending> {
    Runs::new(&world.dir.path().join("work"))
        .read()
        .unwrap()
        .into_iter()
        .map(|recorded| match recorded.state {
            State::Ended { ending, .. } => ending,
            state @ (State::InFlight | State::Interrupted) => {
                panic!("a run is not over: {state:?}")
            }
        })
        .collect()
}

pub fn labels(issue: &Issue) -> Vec<&str> {
    let mut labels: Vec<&str> = issue.labels.iter().map(String::as_str).collect();
    labels.sort_unstable();
    labels
}

pub fn last_comment(issue: &Issue) -> &str {
    &issue.comments.last().expect("the issue has a comment").body
}

pub async fn last_pull_comment(world: &World, pr: Number) -> String {
    world
        .store
        .snapshot_pull(pr)
        .await
        .comments
        .pop()
        .expect("the pull request has a comment")
        .body
}

/// Who is assigned to the merged pull request `pr`, such as a spec change.
pub async fn spec_change_assignees(world: &World, pr: Number) -> Vec<Login> {
    world
        .store
        .merged_pull(pr, Windows::EVER)
        .await
        .unwrap()
        .expect("a merged pull request")
        .assignees
}

pub fn check(name: &str, state: CheckState, log: &str) -> CheckRecord {
    CheckRecord {
        name: name.into(),
        state,
        log: log.into(),
    }
}

/// Each run adds to what its ticket's branch holds.
pub fn implement(world: &World) {
    implement_describing(world, IMPLEMENTED_BODY.to_string());
}

/// [`implement`]'s, each pull request described as also closing `other`.
pub fn implement_closing(world: &World, other: Number) {
    implement_describing(world, also_closing(other));
}

/// How each run [`implement_closing`] scripts describes its pull request.
fn also_closing(other: Number) -> String {
    format!("{IMPLEMENTED_BODY}\n\nCloses {other}")
}

fn implement_describing(world: &World, body: String) {
    world.harness.on(WorkKind::Implement, move |call| {
        let file = implemented_file(&subject_file(call));
        let done = std::fs::read_to_string(call.spec.dir.join(&file)).unwrap_or_default();
        commit_in(
            &call.spec.dir,
            &[(&file, &format!("{done}{IMPLEMENTED}"))],
            IMPLEMENTED_MESSAGE,
        );
        Ok(serde_json::json!({
            "outcome": "done",
            "pr_title": IMPLEMENTED_TITLE,
            "pr_body": body,
        })
        .to_string())
    });
}

/// What each run [`implement`] scripts adds to its file, commits with, and
/// titles and describes its pull request with.
const IMPLEMENTED: &str = "done\n";
const IMPLEMENTED_MESSAGE: &str = "Do the ticket";
const IMPLEMENTED_TITLE: &str = "Do it";
const IMPLEMENTED_BODY: &str = "Does it.";

/// The file each implement run the tests script adds to, for the subject
/// named `subject`.
pub(super) fn implemented_file(subject: &str) -> String {
    format!("src/{subject}.txt")
}

/// What an implement session committed to its ticket's file and answered.
pub(super) struct Implementation<'a> {
    pub contents: &'a str,
    pub message: &'a str,
    pub title: &'a str,
    pub body: &'a str,
}

impl<'a> Implementation<'a> {
    /// What each run [`implement`] scripts commits and answers, describing
    /// its pull request with `body`.
    fn scripted(body: &'a str) -> Implementation<'a> {
        Implementation {
            contents: IMPLEMENTED,
            message: IMPLEMENTED_MESSAGE,
            title: IMPLEMENTED_TITLE,
            body,
        }
    }
}

/// Each design of an issue answers that it is designed, built as it is.
pub fn design_issues(world: &World) {
    world.harness.on(WorkKind::Design, |_| {
        Ok(serde_json::json!({"outcome": "designed", "design": BUILT_AS_ASKED}).to_string())
    });
}

/// The design each design [`design_issues`] scripts answers with.
pub(super) const BUILT_AS_ASKED: &str = "Built as asked.";

/// A session held open until released, from [`gate`] or [`gated`].
pub struct Gate {
    release: std::sync::mpsc::Sender<()>,
    begun: tokio::sync::mpsc::UnboundedReceiver<()>,
}

impl Gate {
    /// Returns once the session has begun.
    pub async fn begun(&mut self) {
        self.begun.recv().await.unwrap();
    }

    /// Lets the session go on; under [`gated`], it ends as failed.
    pub fn release(&self) {
        self.release.send(()).unwrap();
    }
}

/// The session's end of a [`Gate`], from [`gate`].
pub struct Gatepost {
    begin: tokio::sync::mpsc::UnboundedSender<()>,
    released: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl Gatepost {
    /// Says the session has begun, and waits until its [`Gate`] releases it.
    pub fn wait(&self) {
        let _ = self.begin.send(());
        self.released.lock().unwrap().recv().unwrap();
    }
}

/// A [`Gate`] and the [`Gatepost`] a session's script waits at for it.
pub fn gate() -> (Gate, Gatepost) {
    let (release, released) = std::sync::mpsc::channel();
    let (begin, begun) = tokio::sync::mpsc::unbounded_channel();
    let post = Gatepost {
        begin,
        released: std::sync::Mutex::new(released),
    };
    (Gate { release, begun }, post)
}

/// Holds each session of `kind` open until the returned [`Gate`] releases
/// it.
pub fn gated(world: &World, kind: WorkKind) -> Gate {
    let (gate, post) = gate();
    world.harness.on(kind, move |_| {
        post.wait();
        failed_session("released")
    });
    gate
}

/// Sets `setup` as the setup command of every stage.
pub fn setting_up_with(config: &mut FactoryConfig, setup: &str) {
    config.work.flags_and_env.setup = Some(Setup::from(setup.to_string()));
}

/// A world whose work dir is `REPO`'s, holding an issue that needs triage,
/// which each triage answers is ready for an agent.
pub async fn with_issue_for_triage() -> World {
    with_issue_for_triage_where(|_| {}).await
}

/// [`with_issue_for_triage`]'s, with `setup` the setup command of every
/// stage.
pub async fn with_issue_for_triage_setting_up_with(setup: &str) -> World {
    with_issue_for_triage_where(|config| setting_up_with(config, setup)).await
}

async fn with_issue_for_triage_where(adjust: impl FnOnce(&mut FactoryConfig)) -> World {
    let world = World::with(|config| {
        config.work_dir = config.work_dir.join(REPO);
        adjust(config);
    })
    .await;
    world.harness.on(WorkKind::Triage, |_| {
        Ok(serde_json::json!({
            "outcome": "ready-for-agent",
            "category": "bug",
            "comment": "## Agent Brief\n\n**Summary:** It crashes."
        })
        .to_string())
    });
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
    world
}

/// Opens an issue needing triage, which each triage answers is a bug ready
/// for an agent, briefed with `TRIAGE_BRIEF`, and pauses its triage once it
/// removes `needs-triage`.
pub fn triage_paused_after_removing_needs_triage(world: &World) -> (Number, Paused) {
    world.harness.on(WorkKind::Triage, |_| {
        Ok(serde_json::json!({
            "outcome": "ready-for-agent",
            "category": "bug",
            "comment": TRIAGE_BRIEF
        })
        .to_string())
    });
    let issue = world.store.open_issue(
        "reporter",
        "Crash on empty widget",
        "It crashes.",
        &["needs-triage"],
    );
    let paused = world.store.pause_after(Write::RemoveLabel {
        number: issue,
        label: "needs-triage".into(),
    });
    (issue, paused)
}

/// The comment `triage_paused_after_removing_needs_triage`'s triage posts.
pub const TRIAGE_BRIEF: &str = "## Agent Brief\n\n**Summary:** Empty widgets crash.";

/// A maintainer relabels `issue`, handed back, `ready-for-agent`.
pub fn relabel_ready_for_agent(world: &World, issue: Number) {
    world.store.unlabel_as(issue, "ready-for-human").unwrap();
    world
        .store
        .label_as("maintainer", issue, "ready-for-agent")
        .unwrap();
}

/// Opens a ticket titled `title`, ready for an agent and designed.
pub fn open_ticket(world: &World, title: &str) -> Number {
    world.store.open_issue(
        "maintainer",
        title,
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    )
}

/// A store under `dir` whose default branch, `main`, holds only `files`,
/// for a test of a reader of the repository that needs no factory.
pub async fn store_holding(dir: &Path, files: &[(&str, &str)]) -> LocalStore {
    let store = LocalStore::create(
        dir,
        Seed {
            repo: REPO.parse().unwrap(),
            default_branch: "main".into(),
            linewright: Login::new(LINEWRIGHT),
            issue_types: None,
            linear_history: false,
            allowed_merge_methods: MergeMethods::all(),
        },
        Arc::new(FakeClock::new()),
    )
    .await
    .unwrap();
    store
        .commit("maintainer", "main", files, "Start")
        .await
        .unwrap();
    store
}

/// An automation file that runs on the hour; the fake clock starts at 12:00.
pub const HOURLY: &str = "---\non:\n  schedule: \"0 * * * *\"\n---\nLook around.\n";

impl World {
    /// Opens a tracking issue cut into two tickets, ready for a person, and
    /// closes the first.
    pub async fn tracking_with_a_closed_ticket(&self) -> Tracking {
        let tracking = self.store.open_issue(
            "maintainer",
            "Big thing",
            "Cut it.",
            &["linewright:designed"],
        );
        let [closed, open] = ["Part one", "Part two"].map(|title| {
            self.store
                .open_issue(LINEWRIGHT, title, "Of it.", &["ready-for-human"])
        });
        for ticket in [closed, open] {
            self.store.add_sub_issue(tracking, ticket).await.unwrap();
        }
        self.store.close_issue_as_completed(closed).unwrap();
        Tracking { tracking, closed }
    }

    /// The hourly automation, run once on its schedule at 13:00 and once by
    /// hand at 13:05: `hourly/1` and `hourly/2`.
    pub async fn hourly_by_schedule_then_by_hand(&mut self) {
        self.store
            .commit(
                "maintainer",
                "main",
                &[(".linewright/automations/hourly.md", HOURLY)],
                "Add an automation",
            )
            .await
            .unwrap();
        self.harness
            .on_automation(|_| Ok(super::harness::nothing_needs_doing()));
        self.poll().await;
        self.clock.advance(TimeDelta::hours(1));
        self.poll().await;
        self.clock.advance(TimeDelta::minutes(5));
        self.factory
            .run_automation(&"hourly".parse().unwrap())
            .await
            .unwrap();
    }

    /// Runs the implement stage on a ticket, as [`implement`] scripts it,
    /// returning the ticket and the pull request it opened: what
    /// [`World::ticket_in_review`] writes, for a test of the stage itself.
    pub async fn implement_a_ticket(&mut self) -> (Number, Number) {
        implement(self);
        let ticket = open_ticket(self, "Do the thing");
        self.poll().await;
        let pr = *self
            .store
            .pulls()
            .last()
            .expect("a pull request was opened");
        (ticket, pr)
    }

    /// A ticket the implement stage implemented and opened its pull request
    /// for, written straight into the store, with each implement run scripted
    /// as [`implement`] scripts it.
    pub async fn ticket_in_review(&mut self) -> (Number, Number) {
        self.ticket_in_review_described(IMPLEMENTED_BODY.to_string())
            .await
    }

    /// [`World::ticket_in_review`]'s, its pull request also closing `other`,
    /// with each implement run scripted as [`implement_closing`] scripts it.
    pub async fn ticket_in_review_closing(&mut self, other: Number) -> (Number, Number) {
        self.ticket_in_review_described(also_closing(other)).await
    }

    /// [`World::ticket_in_review`]'s, each pull request described with
    /// `body`.
    async fn ticket_in_review_described(&mut self, body: String) -> (Number, Number) {
        implement_describing(self, body.clone());
        let ticket = open_ticket(self, "Do the thing");
        let pr = self
            .implemented(ticket, &Implementation::scripted(&body))
            .await;
        (ticket, pr)
    }

    /// The branch Linewright names for `ticket`, after its title.
    pub(super) fn ticket_branch(&self, ticket: Number) -> String {
        let title = self.issue(ticket).title;
        format!("{}-{}", ticket.0, title.to_lowercase().replace(' ', "-"))
    }

    /// What the implement stage leaves of `ticket`, open and unclaimed,
    /// once its session committed and answered as `implementation` says:
    /// the ticket claimed, its branch, and Linewright's draft pull request
    /// closing it, assigned to Linewright. Returns the pull request.
    pub(super) async fn implemented(
        &self,
        ticket: Number,
        implementation: &Implementation<'_>,
    ) -> Number {
        self.store
            .assign_as(LINEWRIGHT, ticket, LINEWRIGHT)
            .unwrap();
        let branch = self.ticket_branch(ticket);
        let file = implemented_file(&subject_name(Subject::Issue(ticket)));
        self.store
            .commit(
                LINEWRIGHT,
                &branch,
                &[(&file, implementation.contents)],
                implementation.message,
            )
            .await
            .unwrap();
        let pr = self.store.open_linewrights_pull(
            &branch,
            implementation.title,
            &format!("{}\n\nCloses {ticket}", implementation.body),
        );
        self.store.assign_as(LINEWRIGHT, pr, LINEWRIGHT).unwrap();
        pr
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
            .comment_as(ASKS_FOR_REWORK, pr, "Rename it.")
            .unwrap();
        self.harness.on(WorkKind::Rework, |call| {
            commit_in(&call.spec.dir, &[RENAMED], RENAME);
            Ok(serde_json::json!({"outcome": "done", "comment": RENAMED_COMMENT}).to_string())
        });
    }
}

pub struct Serving {
    stopping: CancellationToken,
    draining: CancellationToken,
    handle: JoinHandle<ServeEnd>,
    pub served: tokio::sync::watch::Receiver<Option<Served>>,
    pub polling: tokio::sync::watch::Receiver<Option<Polling>>,
}

impl Serving {
    pub fn start(
        mut factory: Factory,
        poll_interval: std::time::Duration,
        pending: Pending,
    ) -> Serving {
        let stopping = factory.stopping();
        let draining = factory.draining();
        let (publish, served, polling) = Publish::new();
        let handle = tokio::spawn(async move {
            let end = factory.serve(poll_interval, &pending, &publish).await;
            factory.stop().await;
            end
        });
        Serving {
            stopping,
            draining,
            handle,
            served,
            polling,
        }
    }

    pub fn drain(&self) {
        self.draining.cancel();
    }

    pub fn has_ended(&self) -> bool {
        self.handle.is_finished()
    }

    /// Waits for serving to end of its own accord.
    pub async fn ended(self) -> ServeEnd {
        tokio::time::timeout(std::time::Duration::from_secs(10), self.handle)
            .await
            .expect("serving ends")
            .unwrap()
    }

    pub async fn stop(self) -> ServeEnd {
        self.stopping.cancel();
        self.finished().await
    }

    /// Waits for serving to end, however long that takes.
    pub async fn finished(self) -> ServeEnd {
        self.handle.await.unwrap()
    }

    /// The polls failed since the last that succeeded, as last published.
    pub fn failed_polls(&self) -> Option<FailedPolls> {
        self.polling
            .borrow()
            .as_ref()
            .and_then(|polling| polling.failed.clone())
    }

    pub async fn kill(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

/// A `prompts` directory holding only the shipped prompt at `path`, changed
/// by `edit`.
pub fn prompts_with(path: &str, edit: impl FnOnce(String) -> String) -> tempfile::TempDir {
    let shipped = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("briefs/prompts")
        .join(path);
    let dir = tempfile::tempdir().unwrap();
    let prompt = dir.path().join(path);
    std::fs::create_dir_all(prompt.parent().unwrap()).unwrap();
    std::fs::write(prompt, edit(std::fs::read_to_string(shipped).unwrap())).unwrap();
    dir
}

/// Each first-message prompt and its placeholders, in the order of its
/// table in the spec.
const PLACEHOLDERS: &[(&str, &[&str])] = &[
    ("triage.md", &["facts", "categories", "issue", "output"]),
    (
        "design.md",
        &["facts", "kind", "item", "filed_gaps", "cutting", "output"],
    ),
    (
        "implement.md",
        &[
            "facts",
            "default_branch",
            "branch",
            "holding",
            "ticket",
            "output",
        ],
    ),
    (
        "rework.md",
        &[
            "facts",
            "branch",
            "pr",
            "pr_title",
            "description",
            "ticket",
            "unfinished",
            "conflict",
            "feedback",
            "conversation",
            "output",
        ],
    ),
    (
        "fix-ci.md",
        &[
            "facts",
            "branch",
            "pr",
            "pr_title",
            "description",
            "ticket",
            "unfinished",
            "conflict",
            "failures",
            "output",
        ],
    ),
    (
        "update.md",
        &[
            "facts",
            "branch",
            "pr",
            "pr_title",
            "description",
            "ticket",
            "unfinished",
            "conflict",
            "output",
        ],
    ),
    (
        "automation.md",
        &["facts", "name", "instructions", "max_issues", "output"],
    ),
];

/// The briefs with each first-message prompt replaced by its placeholders,
/// each under a heading naming it.
pub fn placeholders_only() -> std::sync::Arc<linewright::briefs::Briefs> {
    let dir = tempfile::tempdir().unwrap();
    for (file, names) in PLACEHOLDERS {
        let text: String = names
            .iter()
            .map(|name| format!("## {name}\n\n{{{{{name}}}}}\n\n"))
            .collect();
        std::fs::write(dir.path().join(file), text).unwrap();
    }
    linewright::briefs::OperatorPrompts::read(Some(dir.path()))
        .unwrap()
        .briefs()
}

/// The view `serve` holds once its first poll took its snapshot.
pub async fn view_served(world: World) -> View {
    let mut serving = Serving::start(
        world.factory,
        std::time::Duration::from_hours(1),
        Pending::default(),
    );
    let served = serving.served.wait_for(Option::is_some).await.unwrap();
    let view = served.as_ref().unwrap().view();
    drop(served);
    serving.stop().await;
    view
}
