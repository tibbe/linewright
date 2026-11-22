//! Running a task, through the system: its stage run on the task in
//! place of GitHub, and what it keeps in its output
//! (`docs/specs/benchmarks.md`, Trials, The output of a trial).

use crate::common;

use std::collections::BTreeMap;
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::*;
use linewright::benchmarks::Benchmarks;
use linewright::cost::{Figures, Spend, Tokens, TurnSpend, Usd};
use linewright::domain::Number;
use linewright::domain::Timestamp;
use linewright::factory::FactoryConfig;
use linewright::harness::{GhReach, SessionSpec};
use linewright::logs::{KeptRuns, RunRecords, Which};
use linewright::reconcile::WorkKind;
use linewright::runs::files::RunFiles;
use linewright::runs::{Ending, Ran, RunId, State};
use linewright::store::github::SESSION_TOKEN_VARIABLE;
use linewright::task::{self, BenchmarkChoice, Criteria, Task, TaskName};
use linewright::transcript;
use linewright::trial::{self, Configuration, ConfigurationRefusal, OutputRefusal};
use linewright::work_settings::{Effort, RunKind, Setup, SetupCommand};
use serde_json::{Value, json};

/// What each triage answers: a bug, ready for an agent.
pub(crate) fn triaged() -> String {
    json!({
        "outcome": "ready-for-agent",
        "category": "bug",
        "comment": "## Agent Brief\n\n**Summary:** It crashes."
    })
    .to_string()
}

/// Captures `world`'s run `run` as a task, and gives its directory.
#[track_caller]
fn captured<'a>(world: &'a World, run: &'a str) -> impl Future<Output = PathBuf> + 'a {
    let caller = Location::caller();
    task_at(caller, world, run)
}

/// A triage run of `world` on an issue, captured as a task: the issue and
/// the task's directory.
#[track_caller]
pub(crate) fn triage_task(world: &mut World) -> impl Future<Output = (Number, PathBuf)> + '_ {
    let caller = Location::caller();
    async move {
        world.harness.on(WorkKind::Triage, |_| Ok(triaged()));
        let issue =
            world
                .store
                .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
        world.poll().await;
        let run = format!("{}/triage-1", issue.0);
        let task = task_at(caller, world, &run).await;
        (issue, task)
    }
}

/// [`captured`], failing at `caller`.
async fn task_at(caller: &Location<'_>, world: &World, run: &str) -> PathBuf {
    let work = world.dir.path().join("work");
    let id = run
        .parse()
        .unwrap_or_else(|e| panic!("the run {run} at {caller}: {e}"));
    let runs = runs_in(&work).await;
    let benchmarks = Benchmarks::new(runs.database().clone());
    let benchmark = BenchmarkChoice::New(TaskName::suggested(&id).as_str().parse().unwrap());
    let capturable = task::capturable(
        &runs,
        &benchmarks,
        &id,
        &TaskName::suggested(&id),
        &benchmark,
    )
    .await
    .unwrap_or_else(|e| panic!("capturing {run} at {caller}: {e}"));
    let criteria = Criteria::parse("It triages the issue.\n")
        .unwrap_or_else(|| panic!("blank criteria at {caller}"));
    task::capture(&*world.store, capturable, &criteria, &benchmarks)
        .await
        .unwrap_or_else(|e| panic!("capturing {run} at {caller}: {e}"))
        .dir
}

/// A trial's output, and how its run ended or why it did not run.
pub(crate) struct RanTrial {
    pub(crate) out: PathBuf,
    ended: Result<Ending, trial::Unrun>,
}

/// Runs the task in `dir` with the sessions `harness` scripts and the
/// factory config `adjust` makes of the tests' own, its work directory
/// `REPO`'s and its output under `under`, and waits for what it leaves to
/// remove.
#[track_caller]
fn ran_trial<'a>(
    dir: &'a Path,
    under: &'a Path,
    harness: Arc<ScriptedHarness>,
    adjust: impl FnOnce(&mut FactoryConfig) + 'a,
) -> impl Future<Output = RanTrial> + 'a {
    ran_trial_with(dir, under, harness, adjust, None)
}

/// [`ran_trial`], with `configuration` if one is given.
#[track_caller]
pub(crate) fn ran_trial_with<'a>(
    dir: &'a Path,
    under: &'a Path,
    harness: Arc<ScriptedHarness>,
    adjust: impl FnOnce(&mut FactoryConfig) + 'a,
    configuration: Option<&'a Configuration>,
) -> impl Future<Output = RanTrial> + 'a {
    let caller = Location::caller();
    async move {
        let task = Task::open(dir)
            .await
            .unwrap_or_else(|e| panic!("opening the case at {caller}: {e}"));
        let out = under.join("out");
        let output =
            trial::Output::make(&out).unwrap_or_else(|e| panic!("the output at {caller}: {e}"));
        let mut config = config(&under.join("work").join(REPO)).await;
        adjust(&mut config);
        let winding_up = config.winding_up.clone();
        let ended = Box::pin(trial::run(
            &task,
            &output,
            harness,
            config,
            configuration,
            &trial::Under::command(),
        ))
        .await;
        winding_up.wait().await;
        RanTrial { out, ended }
    }
}

impl RanTrial {
    #[track_caller]
    fn read(&self, name: &str) -> String {
        let path = self.out.join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    }

    /// Each write, in order: its kind, and its fields.
    #[track_caller]
    fn writes(&self) -> Vec<(String, Value)> {
        let mut names = vec![];
        for entry in std::fs::read_dir(self.out.join("writes"))
            .unwrap_or_else(|e| panic!("listing the writes: {e}"))
        {
            let entry = entry.unwrap_or_else(|e| panic!("listing the writes: {e}"));
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        let mut writes = vec![];
        for name in names {
            let Some((_, kind)) = name
                .strip_suffix(".json")
                .and_then(|stem| stem.split_once('-'))
            else {
                continue;
            };
            let text = self.read(&format!("writes/{name}"));
            let fields =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("reading {name}: {e}"));
            writes.push((kind.to_string(), fields));
        }
        writes
    }

    /// The fields of each write of `kind`, in order.
    #[track_caller]
    fn writes_of(&self, kind: &str) -> Vec<Value> {
        self.writes()
            .into_iter()
            .filter(|(written, _)| written == kind)
            .map(|(_, fields)| fields)
            .collect()
    }
}

/// The names of the entries in `dir`.
#[track_caller]
pub(crate) fn entries(dir: &Path) -> Vec<String> {
    let mut names = vec![];
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("listing {}: {e}", dir.display()))
    {
        let entry = entry.unwrap_or_else(|e| panic!("listing {}: {e}", dir.display()));
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    names
}

#[test_log::test(tokio::test)]
async fn a_triage_task_is_run_into_its_output() {
    let mut world = World::new().await;
    let (issue, task) = triage_task(&mut world).await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, |_| Ok(triaged()));
    let under = tempfile::tempdir().unwrap();

    let ran_trial = ran_trial(&task, under.path(), harness.clone(), |_| {}).await;

    assert_eq!(ran_trial.ended.as_ref().unwrap(), &Ending::Done);
    assert_eq!(ran_trial.read("outcome"), "done\n");
    let version = std::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        ran_trial.read("version"),
        String::from_utf8(version.stdout).unwrap()
    );
    assert_eq!(ran_trial.read("task"), format!("{}/triage-1\n", issue.0));
    assert_eq!(harness.prompts(WorkKind::Triage).len(), 1);
    let trials = under.path().join("work").join(REPO).join("trials");
    assert_eq!(entries(&trials.join("clones")), Vec::<String>::new());
    assert_eq!(
        entries(&ran_trial.out.join(format!("runs/items/{}", issue.0))),
        vec!["triage-1.json"]
    );
    assert_eq!(
        files_named(&ran_trial.out.join("sessions"), "output.jsonl").len(),
        1
    );
    let n = issue.0;
    let label = |kind: &str, label: &str| (kind.to_string(), json!({"number": n, "label": label}));
    assert_eq!(
        ran_trial.writes(),
        vec![
            ("comment".to_string(), json!({"number": n})),
            label("add-label", "bug"),
            label("add-label", "ready-for-agent"),
            label("remove-label", "needs-triage"),
            label("remove-label", "needs-info"),
            label("remove-label", "ready-for-human"),
        ]
    );
    assert!(
        ran_trial
            .read("writes/0001-comment.md")
            .contains("**Summary:** It crashes."),
        "{}",
        ran_trial.read("writes/0001-comment.md")
    );
}

#[test_log::test(tokio::test)]
async fn a_trials_run_reads_back_from_its_output_as_the_trial_wrote_it() {
    let mut world = World::new().await;
    let (issue, task) = triage_task(&mut world).await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, |_| Ok(triaged()));
    let under = tempfile::tempdir().unwrap();
    let ran_trial = ran_trial(&task, under.path(), harness, |_| {}).await;
    let id: RunId = format!("{}/triage-1", issue.0).parse().unwrap();

    let mut records = RunRecords::start(
        KeptRuns::Files(RunFiles::new(&ran_trial.out)),
        &Which::Run {
            id: id.clone(),
            item: None,
        },
    )
    .await
    .unwrap();
    records.read().await.unwrap();
    let of_item = RunRecords::start(
        KeptRuns::Files(RunFiles::new(&ran_trial.out)),
        &Which::Item(issue),
    )
    .await
    .unwrap();

    let written: Value =
        serde_json::from_str(&ran_trial.read(&format!("runs/items/{}/triage-1.json", issue.0)))
            .unwrap();
    assert_eq!(records.run(), &id);
    assert_eq!(of_item.run(), &id);
    assert!(records.ended());
    let [written_session] = written["sessions"].as_array().unwrap().as_slice() else {
        panic!("one session written: {written}");
    };
    let [session] = records.sessions() else {
        panic!("one session read: {:?}", records.sessions());
    };
    let dir = ran_trial.out.join(written_session["dir"].as_str().unwrap());
    assert_eq!(
        session.output,
        std::fs::read_to_string(dir.join(transcript::OUTPUT)).unwrap()
    );
    assert_eq!(
        session.brief.as_deref(),
        Some(
            std::fs::read_to_string(dir.join(transcript::BRIEF))
                .unwrap()
                .as_str()
        )
    );
    assert_eq!(
        session.ran,
        Some(serde_json::from_value::<Ran>(written_session.clone()).unwrap())
    );
    let read = RunFiles::new(&ran_trial.out)
        .read(&id)
        .unwrap()
        .expect("the trial's run is read");
    assert_eq!(
        read.state,
        State::Ended {
            at: serde_json::from_value(written["end"]["at"].clone()).unwrap(),
            ending: Ending::Done,
        }
    );
    assert_eq!(
        read.started_at,
        serde_json::from_value::<Timestamp>(written["started_at"].clone()).unwrap()
    );
}

/// Asserts that `part` is a number of seconds on a line of its own.
#[track_caller]
fn assert_seconds(part: &str) {
    let seconds = part.strip_suffix('\n').map(str::parse::<f64>);
    assert!(
        matches!(seconds, Some(Ok(seconds)) if seconds >= 0.0),
        "{part:?} is not a number of seconds"
    );
}

/// The cost a trial's output keeps, its time checked and left out.
#[track_caller]
fn cost_without_time(ran_trial: &RanTrial) -> Value {
    let mut cost: Value = serde_json::from_str(&ran_trial.read("cost.json")).unwrap();
    let seconds = cost.as_object_mut().unwrap().remove("seconds");
    assert!(
        seconds
            .as_ref()
            .and_then(Value::as_f64)
            .is_some_and(|s| s >= 0.0),
        "the trial's time: {seconds:?}"
    );
    cost
}

#[test_log::test(tokio::test)]
async fn a_trial_keeps_its_time_and_what_its_run_cost() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, |_| Ok(triaged()));
    harness.reports_model("claude-opus-5-5");
    harness.reports_spend(TurnSpend::OfSessionSoFar(Figures(BTreeMap::from([(
        "claude-opus-5-5".to_string(),
        Spend {
            tokens: Some(Tokens {
                input: 10,
                output: 200,
                cache_write: 3000,
                cache_read: 40000,
            }),
            usd: Usd::new(1.25),
        },
    )]))));
    let under = tempfile::tempdir().unwrap();

    let ran_trial = ran_trial(&task, under.path(), harness, |_| {}).await;

    assert_eq!(ran_trial.ended.as_ref().unwrap(), &Ending::Done);
    let mut cost = cost_without_time(&ran_trial);
    let run_seconds = cost["run"].as_object_mut().unwrap().remove("seconds");
    assert!(
        run_seconds.as_ref().and_then(Value::as_f64).is_some(),
        "the run's time: {run_seconds:?}"
    );
    let spent = json!({
        "tokens": {"input": 10, "output": 200, "cache_write": 3000, "cache_read": 40000},
        "usd": 1.25,
    });
    assert_eq!(
        cost,
        json!({"run": {"models": {"claude-opus-5-5": spent}, "all": spent}})
    );
    assert_eq!(
        trial::kept_spent(&ran_trial.out).map(|kept| kept.money),
        Some(Usd::new(1.25))
    );
}

#[test_log::test(tokio::test)]
async fn a_trial_whose_build_fails_keeps_the_builds_output_and_time_and_its_own_time() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, |_| Ok(triaged()));
    let under = tempfile::tempdir().unwrap();

    let ran_trial = ran_trial(&task, under.path(), harness.clone(), |config| {
        config.work.pipeline_flags_and_env.setup.command = Some(Setup::Command(
            SetupCommand::new("echo the build broke; exit 3".into()).unwrap(),
        ));
    })
    .await;

    assert!(
        matches!(ran_trial.ended, Err(trial::Unrun::BuildFailed(_))),
        "{:?}",
        ran_trial.ended
    );
    assert_eq!(ran_trial.read("build.log"), "the build broke\n");
    assert_seconds(&ran_trial.read("build-seconds"));
    assert_eq!(cost_without_time(&ran_trial), json!({}));
    assert_eq!(harness.prompts(WorkKind::Triage).len(), 0);
}

#[test_log::test(tokio::test)]
async fn a_design_task_is_run_numbering_its_tickets_above_the_tasks_items() {
    let mut world = World::new().await;
    tickets_script(&world);
    let issue = open_undesigned_gadgets(&world);
    world.poll().await;
    let task = captured(&world, &format!("{}/design-1", issue.0)).await;
    for _ in 0..3 {
        open_ticket(&world, "Opened since");
    }
    let harness = world.harness.clone();
    let under = tempfile::tempdir().unwrap();

    let ran_trial = ran_trial(&task, under.path(), harness, |_| {}).await;

    assert_eq!(ran_trial.ended.as_ref().unwrap(), &Ending::Done);
    assert_eq!(ran_trial.read("outcome"), "done\n");
    let created: Vec<Value> = ran_trial
        .writes_of("create-issue")
        .into_iter()
        .map(|fields| fields["title"].clone())
        .collect();
    assert_eq!(created, vec![json!("Store gadgets"), json!("List gadgets")]);
    assert_eq!(
        ran_trial.writes_of("add-sub-issue"),
        vec![
            json!({"parent": issue.0, "child": issue.0 + 1}),
            json!({"parent": issue.0, "child": issue.0 + 2}),
        ]
    );
}

#[test_log::test(tokio::test)]
async fn an_implement_task_is_run_pushing_its_branch_into_the_output() {
    let mut world = World::new().await;
    implement_gadgets(&world);
    let ticket = open_designed_gadgets(&world);
    world.poll().await;
    let task = captured(&world, &format!("{}/implement-1", ticket.0)).await;
    let harness = world.harness.clone();
    let under = tempfile::tempdir().unwrap();

    let ran_trial = ran_trial(&task, under.path(), harness, |config| {
        config.work.pipeline_flags_and_env.setup.command = Some(Setup::Command(
            SetupCommand::new("echo built the environment".into()).unwrap(),
        ));
    })
    .await;

    assert_eq!(ran_trial.ended.as_ref().unwrap(), &Ending::Done);
    assert_eq!(ran_trial.read("outcome"), "done\n");
    assert!(
        ran_trial
            .read("build.log")
            .contains("built the environment"),
        "{}",
        ran_trial.read("build.log")
    );
    assert_seconds(&ran_trial.read("build-seconds"));
    let pushed: Vec<String> = ran_trial
        .writes_of("push")
        .into_iter()
        .map(|fields| fields["branch"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(pushed.len(), 1, "{pushed:?}");
    let file = git_in(
        &ran_trial.out.join("commits.git"),
        &["show", &format!("{}:src/issue-{}.txt", pushed[0], ticket.0)],
    );
    assert_eq!(file, "gadget\n");
    assert_eq!(
        ran_trial.writes_of("open-pull"),
        vec![json!({"head": pushed[0], "title": "Add gadgets"})]
    );
}

/// What the socket that the configuration of `gh` in the environment of
/// `spec`'s session names answers a `GET` of `target`, as `gh` sends it:
/// the whole answer, or why there is none.
#[track_caller]
pub(crate) fn gh_answer(spec: &SessionSpec, target: &str) -> String {
    use std::io::{Read as _, Write as _};

    let environment = fake_environment(&spec.environment, &spec.scratch, &spec.gh);
    let Some((_, config)) = environment
        .iter()
        .rev()
        .find(|(name, _)| name == "GH_CONFIG_DIR")
    else {
        return "no GH_CONFIG_DIR".into();
    };
    let yaml = std::fs::read_to_string(Path::new(config).join("config.yml")).unwrap();
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&yaml).unwrap();
    let socket = yaml["http_unix_socket"].as_str().unwrap();
    let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: api.github.com\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    answer
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn a_trials_sessions_gh_is_refused_the_tasks_repository() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let harness = Arc::new(ScriptedHarness::default());
    let answers = Arc::new(Mutex::new(vec![]));
    let kept = answers.clone();
    harness.on(WorkKind::Triage, move |call| {
        kept.lock()
            .unwrap()
            .push(gh_answer(call.spec, &format!("/repos/{REPO}/issues")));
        Ok(triaged())
    });
    let under = tempfile::tempdir().unwrap();

    let ran_trial = ran_trial(&task, under.path(), harness, |_| {}).await;

    assert_eq!(ran_trial.ended.as_ref().unwrap(), &Ending::Done);
    let answers = answers.lock().unwrap();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert!(answers[0].starts_with("HTTP/1.1 403 "), "{}", answers[0]);
}

#[test_log::test(tokio::test)]
async fn a_served_runs_sessions_gh_reaches_github_as_configured() {
    let mut world = World::new().await;
    let reached = Arc::new(Mutex::new(vec![]));
    let kept = reached.clone();
    world.harness.on(WorkKind::Triage, move |call| {
        kept.lock().unwrap().push(call.spec.gh.clone());
        Ok(triaged())
    });
    world
        .store
        .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);

    world.poll().await;

    assert_eq!(*reached.lock().unwrap(), vec![GhReach::Direct]);
}

#[test_log::test(tokio::test)]
async fn a_later_trial_from_the_same_head_uses_the_environment_an_earlier_one_built() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, |_| Ok(triaged()));
    let under = tempfile::tempdir().unwrap();
    let setup = |config: &mut FactoryConfig| {
        config.work.pipeline_flags_and_env.setup.command = Some(Setup::Command(
            SetupCommand::new("echo built the environment".into()).unwrap(),
        ));
    };
    let first = ran_trial(&task, under.path(), harness.clone(), setup).await;
    assert_eq!(first.ended.as_ref().unwrap(), &Ending::Done);
    assert!(first.out.join("build.log").is_file());
    std::fs::remove_dir_all(&first.out).unwrap();

    let second = ran_trial(&task, under.path(), harness, setup).await;

    assert_eq!(second.ended.as_ref().unwrap(), &Ending::Done);
    assert!(!second.out.join("build.log").exists());
}

#[test_log::test(tokio::test)]
async fn a_session_refused_for_the_usage_limit_ends_the_trials_run_for_it() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, refused_for_the_usage_limit);
    let under = tempfile::tempdir().unwrap();

    let ran_trial = ran_trial(&task, under.path(), harness, |config| {
        config.waits_out_usage_limit = true;
    })
    .await;

    assert_eq!(ran_trial.ended.as_ref().unwrap(), &Ending::UsageLimit);
    assert_eq!(ran_trial.read("outcome"), "usage-limit\n");
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn a_trials_run_does_not_count_toward_max_runs() {
    let mut world = World::with(|config| config.max_runs = 1).await;
    let (_, task) = triage_task(&mut world).await;
    let (mut gate, post) = gate();
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, move |_| {
        post.wait();
        failed_session("released")
    });
    let under = tempfile::tempdir().unwrap();
    let work = world.dir.path().join("work");
    world
        .store
        .open_issue("reporter", "Import fails", "It crashes.", &["needs-triage"]);
    let running = {
        let under = under.path().to_path_buf();
        tokio::spawn(async move {
            Box::pin(ran_trial(&task, &under, harness, |config| {
                config.work_dir = work;
            }))
            .await
            .ended
            .map_err(|e| format!("{e:#}"))
        })
    };
    gate.begun().await;

    world.poll().await;

    assert_eq!(ended_runs(&world).await, vec![Ending::Done, Ending::Done]);
    gate.release();
    assert_eq!(running.await.unwrap(), Ok(Ending::HandedBack));
}

/// Commits `files` to the default branch of the task in `task`.
#[track_caller]
fn commit_to_task(task: &Path, files: &[(&str, &str)]) {
    let checkout = tempfile::tempdir().unwrap();
    let repository = task.join("repository.git");
    git_in(
        checkout.path(),
        &[
            "clone",
            "--quiet",
            "--branch",
            "main",
            repository.to_str().unwrap(),
            ".",
        ],
    );
    git_in(checkout.path(), &["config", "user.name", "Maintainer"]);
    git_in(
        checkout.path(),
        &["config", "user.email", "maintainer@example.com"],
    );
    commit_in(checkout.path(), files, "Change Linewright's files");
    git_in(checkout.path(), &["push", "--quiet", "origin", "HEAD:main"]);
}

#[test_log::test(tokio::test)]
async fn a_task_whose_default_branch_holds_a_file_that_would_hold_the_run_is_refused_naming_it() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    commit_to_task(&task, &[(".linewright/config.toml", "model = [\n")]);
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, |_| Ok(triaged()));
    let under = tempfile::tempdir().unwrap();

    let ran_trial = ran_trial(&task, under.path(), harness.clone(), |_| {}).await;

    let error = format!("{:#}", ran_trial.ended.unwrap_err());
    assert!(
        error.contains(".linewright/config.toml") && error.contains("would hold the run"),
        "{error}"
    );
    assert_eq!(harness.prompts(WorkKind::Triage).len(), 0);
    assert!(!ran_trial.out.join("outcome").exists());
}

#[test_log::test(tokio::test)]
async fn a_trials_setting_refuses_a_file_that_would_hold_the_kind_of_run_it_is_prepared_for() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    commit_to_task(
        &task,
        &[(".linewright/briefs/prompts/implement.md", "Do {{nope}}\n")],
    );
    let task = Task::open(&task).await.unwrap();
    let under = tempfile::tempdir().unwrap();
    let config = config(&under.path().join("work").join(REPO)).await;
    let prepare = |name: &'static str, kind: WorkKind| {
        let task = &task;
        let config = &config;
        let dir = under.path().join(name);
        async move {
            trial::Setting::prepare(
                task,
                &RunKind::Stage(kind),
                config,
                Arc::new(ScriptedHarness::default()),
                &dir.join("store"),
                &dir.join("commits.git"),
                |_| Ok(()),
            )
            .await
        }
    };

    let triage = prepare("triage", WorkKind::Triage).await;
    let implement = prepare("implement", WorkKind::Implement).await;

    assert!(triage.is_ok(), "{:#}", triage.err().unwrap());
    let error = format!("{:#}", implement.err().unwrap());
    assert!(
        error.contains(".linewright/briefs/prompts/implement.md")
            && error.contains("would hold the run"),
        "{error}"
    );
    drop(triage);
    config.winding_up.wait().await;
}

#[test_log::test]
fn an_output_that_holds_anything_is_refused_and_a_missing_one_is_made() {
    let dir = tempfile::tempdir().unwrap();
    let held = dir.path().join("held");
    std::fs::create_dir(&held).unwrap();
    std::fs::write(held.join("notes.md"), "Mine.\n").unwrap();
    let missing = dir.path().join("missing");

    let refused = trial::Output::make(&held);
    let made = trial::Output::make(&missing);

    assert!(
        matches!(refused, Err(OutputRefusal::NotEmpty(_))),
        "{refused:?}"
    );
    assert_eq!(entries(&held), vec!["notes.md"]);
    assert!(made.is_ok(), "{made:?}");
    assert_eq!(entries(&missing), Vec::<String>::new());
}

/// Runs `lw` with `args`, `bin` as its path and `work` as its work
/// directory, from a directory of its own, with Linewright's token
/// `linewright-token` in `GH_TOKEN`, and neither `GITHUB_TOKEN`, the
/// separate token nor `repo` set, but as `env` sets them.
#[track_caller]
pub(crate) fn lw_isolated<'a>(
    bin: &'a Path,
    work: &'a Path,
    args: &'a [&str],
    env: &'a [(&str, &str)],
) -> impl Future<Output = Output> + 'a {
    let caller = Location::caller();
    async move {
        let cwd = tempfile::tempdir().unwrap_or_else(|e| panic!("a directory at {caller}: {e}"));
        let output = lw_command(bin, work, cwd.path())
            .args(args)
            .envs(env.iter().copied())
            .output()
            .await
            .unwrap_or_else(|e| panic!("running lw at {caller}: {e}"));
        Output {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

/// `lw` with `bin` as its path and `work` as its work directory, run in
/// `cwd`, with Linewright's token `linewright-token` in `GH_TOKEN`, and
/// neither `GITHUB_TOKEN`, the separate token nor `repo` set.
pub(crate) fn lw_command(bin: &Path, work: &Path, cwd: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_lw"));
    command
        .arg("--work-dir")
        .arg(work)
        .current_dir(cwd)
        .env("PATH", bin)
        .env("XDG_RUNTIME_DIR", cwd)
        .env("GH_TOKEN", "linewright-token")
        .env_remove("GITHUB_TOKEN")
        .env_remove(SESSION_TOKEN_VARIABLE)
        .env_remove("LINEWRIGHT_REPO");
    command
}

/// A directory to put on the path of `lw`, holding git, bash and a fake
/// `claude` whose every triage answers that the issue is a bug ready for an
/// agent, logging what it was given there.
#[track_caller]
fn bin_triaging() -> tempfile::TempDir {
    let bin = tempfile::tempdir().unwrap();
    link_from_path(bin.path(), "git");
    link_from_path(bin.path(), "bash");
    fake_claude(bin.path(), &triaged());
    bin
}

#[test_log::test(tokio::test)]
async fn without_linewrights_token_lw_trial_exits_1_naming_its_variables_and_runs_no_session() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "trial",
            task.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        &[("GH_TOKEN", ""), (SESSION_TOKEN_VARIABLE, "sessions-token")],
    )
    .await;

    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert_eq!(ran.stderr.lines().count(), 1, "{}", ran.stderr);
    assert!(ran.stderr.starts_with("lw: "), "{}", ran.stderr);
    assert!(ran.stderr.contains("GH_TOKEN"), "{}", ran.stderr);
    assert!(ran.stderr.contains("GITHUB_TOKEN"), "{}", ran.stderr);
    assert!(!bin.path().join("tokens.log").exists());
    assert!(!out.exists());
}

#[test_log::test(tokio::test)]
async fn lw_trial_refuses_a_task_without_criteria_naming_them() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    std::fs::remove_file(task.join("criteria.md")).unwrap();
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "trial",
            task.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        &[],
    )
    .await;

    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert!(
        ran.stderr
            .contains("the task cannot be run: the task's criteria: "),
        "{}",
        ran.stderr
    );
    assert!(!bin.path().join("tokens.log").exists());
}

#[test_log::test(tokio::test)]
async fn lw_trial_gives_sessions_linewrights_token_with_repo_set_or_unset() {
    let mut world = World::new().await;
    let (issue, task) = triage_task(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let task_arg = task.to_str().unwrap();
    let unset = work.path().join("unset");
    let other = work.path().join("other");

    let unset_ran = lw_isolated(
        bin.path(),
        work.path(),
        &["trial", task_arg, "--out", unset.to_str().unwrap()],
        &[],
    )
    .await;
    let other_ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "--repo",
            "someone/else",
            "trial",
            task_arg,
            "--out",
            other.to_str().unwrap(),
        ],
        &[("GH_TOKEN", ""), ("GITHUB_TOKEN", "linewright-token")],
    )
    .await;

    for (ran, out) in [(unset_ran, &unset), (other_ran, &other)] {
        assert_eq!(ran.code, Some(0), "{}", ran.stderr);
        assert_eq!(
            std::fs::read_to_string(out.join("outcome")).unwrap(),
            "done\n"
        );
        let record = out.join(format!("runs/items/{}/triage-1.json", issue.0));
        assert!(record.is_file(), "{}", ran.stderr);
    }
    let tokens = std::fs::read_to_string(bin.path().join("tokens.log")).unwrap();
    assert_eq!(
        tokens,
        "GH_TOKEN=linewright-token GITHUB_TOKEN=linewright-token\n".repeat(2)
    );
    assert!(work.path().join(REPO).join("trials").is_dir());
    assert!(!work.path().join("someone").exists());
}

#[test_log::test(tokio::test)]
async fn lw_trial_warns_that_sessions_given_linewrights_token_can_write_to_github() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "--log-format",
            "json",
            "trial",
            task.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        &[],
    )
    .await;

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    let warned = json_messages(&ran.stderr, "WARN")
        .into_iter()
        .any(|message| {
            message.contains("sessions can write to GitHub")
                && message.contains("LINEWRIGHT_SESSION_GITHUB_TOKEN")
        });
    assert!(warned, "{}", ran.stderr);
}

#[test_log::test(tokio::test)]
async fn lw_trial_gives_sessions_the_separate_token_when_it_is_set() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "trial",
            task.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        &[(SESSION_TOKEN_VARIABLE, "sessions-token")],
    )
    .await;

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    assert!(
        !ran.stderr.contains("sessions can write to GitHub"),
        "{}",
        ran.stderr
    );
    let tokens = std::fs::read_to_string(bin.path().join("tokens.log")).unwrap();
    assert_eq!(
        tokens,
        "GH_TOKEN=sessions-token GITHUB_TOKEN=sessions-token\n"
    );
}

#[test_log::test(tokio::test)]
async fn lw_trial_does_not_count_gh_config_dir_among_the_variables_sessions_are_not_given() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "--log-format",
            "json",
            "trial",
            task.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        &[("GH_CONFIG_DIR", "/etc/gh")],
    )
    .await;

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    let withheld = ran
        .stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|line| line["fields"]["variables"].as_str().map(String::from))
        .unwrap_or_else(|| panic!("no variables logged: {}", ran.stderr));
    assert!(
        !withheld.split(' ').any(|name| name == "GH_CONFIG_DIR"),
        "{withheld}"
    );
}

#[test_log::test(tokio::test)]
async fn lw_trial_refuses_a_task_that_lacks_a_part_with_status_1_naming_the_part() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    std::fs::remove_file(task.join("properties.json")).unwrap();
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "trial",
            task.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        &[],
    )
    .await;

    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert!(
        ran.stderr.contains("the task's properties"),
        "{}",
        ran.stderr
    );
    assert!(!bin.path().join("tokens.log").exists());
}

#[test_log::test(tokio::test)]
async fn lw_trial_refuses_an_output_that_holds_anything_with_status_1() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");
    std::fs::create_dir(&out).unwrap();
    std::fs::write(out.join("notes.md"), "Mine.\n").unwrap();

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "trial",
            task.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        &[],
    )
    .await;

    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert!(ran.stderr.contains("holds something"), "{}", ran.stderr);
    assert!(!bin.path().join("tokens.log").exists());
}

#[test_log::test(tokio::test)]
async fn a_signal_ends_lw_trial_at_once_leaving_its_output_interrupted() {
    let mut world = World::new().await;
    let (issue, task) = triage_task(&mut world).await;
    let bin = tempfile::tempdir().unwrap();
    link_from_path(bin.path(), "git");
    link_from_path(bin.path(), "bash");
    let started = bin.path().join("claude.started");
    fake_claude_running(
        bin.path(),
        &Scenario {
            turn: vec![Step::Touch(started.clone()), Step::Hang],
            ..Scenario::default()
        },
    );
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");
    let mut trial = lw_command(bin.path(), work.path(), work.path())
        .arg("trial")
        .arg(&task)
        .arg("--out")
        .arg(&out)
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    eventually(|| started.exists()).await;
    assert_eq!(
        std::fs::read_to_string(out.join("outcome")).unwrap(),
        "in-flight\n"
    );
    let run: RunId = format!("{}/triage-1", issue.0).parse().unwrap();
    eventually(|| {
        RunFiles::new(&out)
            .cost(&run)
            .is_ok_and(|cost| cost.spend.is_none())
    })
    .await;

    let pid = trial.id().unwrap().to_string();
    let sent = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap();

    assert!(sent.success());
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), trial.wait())
        .await
        .expect("the trial did not end at once")
        .unwrap();
    assert_eq!(status.code(), Some(128 + 15), "{status}");
    assert_eq!(
        std::fs::read_to_string(out.join("outcome")).unwrap(),
        "interrupted\n"
    );
    assert!(
        out.join(format!("runs/items/{}/triage-1.json", issue.0))
            .is_file()
    );
    let cost: Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("cost.json")).unwrap()).unwrap();
    assert!(cost["seconds"].is_f64(), "{cost}");
    assert_eq!(cost["run"], json!({}));
}

#[test_log::test(tokio::test)]
async fn a_trial_removes_the_directory_of_one_a_signal_ended() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let under = tempfile::tempdir().unwrap();
    let left = under
        .path()
        .join("work")
        .join(REPO)
        .join("trials/clones/trial-left");
    std::fs::create_dir_all(left.join("store")).unwrap();
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, |_| Ok(triaged()));

    let ran_trial = ran_trial(&task, under.path(), harness, |_| {}).await;

    assert!(ran_trial.ended.is_ok(), "{:?}", ran_trial.ended);
    assert!(!left.exists());
}

#[test_log::test(tokio::test)]
async fn lw_help_lists_trial() {
    let bin = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();

    let ran = lw_isolated(bin.path(), work.path(), &["--help"], &[]).await;

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    assert!(
        ran.stdout
            .lines()
            .any(|line| line.trim_start().starts_with("trial ")),
        "{}",
        ran.stdout
    );
}

#[test_log::test(tokio::test)]
async fn a_signal_during_the_build_ends_the_build_with_lw_trial() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let bin = bin_triaging();
    link_from_path(bin.path(), "sleep");
    let work = tempfile::tempdir().unwrap();
    let pid = work.path().join("build.pid");
    let setup = format!("echo $$ > {}; exec sleep 300", pid.display());
    let mut trial = lw_command(bin.path(), work.path(), work.path())
        .arg("trial")
        .arg(&task)
        .arg("--out")
        .arg(work.path().join("out"))
        .env("LINEWRIGHT_SETUP_COMMAND", &setup)
        .stderr(std::fs::File::create(work.path().join("stderr.log")).unwrap())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    eventually(|| std::fs::read_to_string(&pid).is_ok_and(|text| text.ends_with('\n'))).await;
    let build = std::fs::read_to_string(&pid).unwrap().trim().to_string();

    let sent = std::process::Command::new("kill")
        .args(["-TERM", &trial.id().unwrap().to_string()])
        .status()
        .unwrap();

    assert!(sent.success());
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), trial.wait())
        .await
        .expect("the trial did not end at once")
        .unwrap();
    let stderr = std::fs::read_to_string(work.path().join("stderr.log")).unwrap();
    assert_eq!(status.code(), Some(128 + 15), "{status}: {stderr}");
    eventually(|| {
        !std::process::Command::new("kill")
            .args(["-0", &build])
            .status()
            .unwrap()
            .success()
    })
    .await;
}

/// Linewright's own text of the file at `path` under `briefs/`, below a
/// line `marker`.
#[track_caller]
fn marked(path: &str, marker: &str) -> String {
    let own = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("briefs")
        .join(path);
    let own =
        std::fs::read_to_string(&own).unwrap_or_else(|e| panic!("reading {}: {e}", own.display()));
    format!("{marker}\n{own}")
}

/// Writes `files`, by their paths in it, into the directory `dir`.
#[track_caller]
fn write_files(dir: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
}

/// The configuration in `dir`, holding `files` by their paths in it.
#[track_caller]
pub(crate) fn configuration(dir: &Path, files: &[(&str, &str)]) -> Configuration {
    std::fs::create_dir_all(dir).unwrap();
    write_files(dir, files);
    Configuration::read(dir).unwrap_or_else(|e| panic!("{e}"))
}

#[test_log::test(tokio::test)]
async fn a_configuration_wins_over_the_tasks_files_and_the_flags_leaving_the_rest_to_them() {
    let mut world = World::new().await;
    let (issue, task) = triage_task(&mut world).await;
    let repository_triage = marked("prompts/triage.md", "THE REPOSITORY'S TRIAGE");
    commit_to_task(
        &task,
        &[
            (
                ".linewright/config.toml",
                "[stage.triage]\nmodel = \"repository-model\"\neffort = \"low\"\n",
            ),
            (".linewright/briefs/prompts/triage.md", &repository_triage),
            (
                ".linewright/briefs/prompts/implement.md",
                &marked("prompts/implement.md", "THE REPOSITORY'S IMPLEMENT"),
            ),
        ],
    );
    let harness = Arc::new(ScriptedHarness::default());
    let ran_with = Arc::new(Mutex::new(vec![]));
    let seen = ran_with.clone();
    harness.on(WorkKind::Triage, move |call| {
        seen.lock()
            .unwrap()
            .push((call.spec.model.clone(), call.spec.effort));
        Ok(triaged())
    });
    let under = tempfile::tempdir().unwrap();
    let settings = "model = \"configuration-model\"\n";
    let configuration_triage = marked("prompts/triage.md", "THE CONFIGURATION'S TRIAGE");
    let configuration = configuration(
        &under.path().join("configuration"),
        &[
            ("config.toml", settings),
            ("briefs/prompts/triage.md", &configuration_triage),
        ],
    );

    let ran_trial = ran_trial_with(
        &task,
        under.path(),
        harness.clone(),
        |config| config.work.flags_and_env.model = Some("flag-model".into()),
        Some(&configuration),
    )
    .await;

    assert_eq!(ran_trial.ended.as_ref().unwrap(), &Ending::Done);
    assert_eq!(
        *ran_with.lock().unwrap(),
        vec![(Some("configuration-model".to_string()), Some(Effort::Low))]
    );
    let [prompt] = harness.prompts(WorkKind::Triage).try_into().unwrap();
    assert!(prompt.contains("THE CONFIGURATION'S TRIAGE"), "{prompt}");
    assert!(!prompt.contains("THE REPOSITORY'S TRIAGE"), "{prompt}");
    let record: Value =
        serde_json::from_str(&ran_trial.read(&format!("runs/items/{}/triage-1.json", issue.0)))
            .unwrap();
    let layers = &record["sessions"][0]["layers"];
    assert_eq!(layers["settings"]["model"], "configuration");
    assert_eq!(layers["settings"]["effort"], "repository");
    assert_eq!(layers["briefs"]["prompts/triage.md"], "configuration");
    assert_eq!(layers["briefs"]["prompts/implement.md"], "repository");
    assert_eq!(ran_trial.read("configuration/config.toml"), settings);
    assert_eq!(
        ran_trial.read("configuration/briefs/prompts/triage.md"),
        configuration_triage
    );
}

/// The path of each file the refusal `refused` names, and its error.
#[track_caller]
fn refused_files(refused: Result<Configuration, ConfigurationRefusal>) -> Vec<(String, String)> {
    match refused {
        Err(ConfigurationRefusal::Errors { errors, .. }) => errors
            .into_iter()
            .map(|file| (file.path, file.error))
            .collect(),
        other => panic!("not a refusal naming files: {other:?}"),
    }
}

#[test_log::test]
fn a_configuration_is_refused_naming_each_file_with_an_error_hidden_files_aside() {
    let dir = tempfile::tempdir().unwrap();
    write_files(
        dir.path(),
        &[
            ("config.toml", "model = \"opus\"\ntemperature = 1\n"),
            ("briefs/prompts/triage.md", "Triage {{nope}}.\n"),
            ("briefs/results/triage.schema.json", "{}\n"),
            ("notes.md", "Mine.\n"),
            (".notes.md", "Mine.\n"),
            ("briefs/.DS_Store", "\0"),
        ],
    );

    let refused = refused_files(Configuration::read(dir.path()));

    let paths: Vec<&str> = refused.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "briefs/prompts/triage.md",
            "briefs/results/triage.schema.json",
            "config.toml",
            "notes.md",
        ],
        "{refused:?}"
    );
    assert!(refused[0].1.contains("nope"), "{refused:?}");
    assert!(
        refused[1].1.contains("not a file a layer may hold"),
        "{refused:?}"
    );
    assert!(refused[2].1.contains("`temperature`"), "{refused:?}");
}

#[test_log::test]
fn a_configurations_settings_with_a_table_are_refused_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    write_files(
        dir.path(),
        &[("config.toml", "[stage.triage]\nmodel = \"opus\"\n")],
    );

    let refused = refused_files(Configuration::read(dir.path()));

    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0].0, "config.toml");
    assert!(refused[0].1.contains("`stage`"), "{refused:?}");
}

#[test_log::test]
fn a_missing_configuration_is_refused_and_an_empty_one_is_read() {
    let dir = tempfile::tempdir().unwrap();

    let missing = Configuration::read(&dir.path().join("missing"));
    let empty = Configuration::read(dir.path());

    assert!(
        matches!(missing, Err(ConfigurationRefusal::Unreadable { .. })),
        "{missing:?}"
    );
    assert!(empty.is_ok(), "{empty:?}");
}

#[test_log::test(tokio::test)]
async fn lw_trial_refuses_a_configuration_with_an_error_naming_the_file_before_it_starts() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");
    let configuration = work.path().join("configuration");
    write_files(&configuration, &[("config.toml", "temperature = 1\n")]);

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "trial",
            task.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--configuration",
            configuration.to_str().unwrap(),
        ],
        &[],
    )
    .await;

    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert!(
        ran.stderr.contains("cannot be used:") && ran.stderr.contains("- config.toml: "),
        "{}",
        ran.stderr
    );
    assert!(!out.exists());
    assert!(!bin.path().join("tokens.log").exists());
}
