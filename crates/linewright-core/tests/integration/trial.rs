//! Running a task, through the system: its stage run on the task in
//! place of GitHub, and what it keeps in its output
//! (`docs/specs/benchmarks.md`, Trials, The output of a trial).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use linewright_core::cost::{Figures, Spend, Tokens, TurnSpend, Usd};
use linewright_core::domain::Timestamp;
use linewright_core::factory::FactoryConfig;
use linewright_core::harness::{GhReach, SessionSpec, UsageLimit};
use linewright_core::logs::{KeptRuns, RunRecords, Which};
use linewright_core::reconcile::WorkKind;
use linewright_core::runs::files::RunFiles;
use linewright_core::runs::{Ending, Ran, RunId, State};
use linewright_core::task::Task;
use linewright_core::testing::trial_runs::{self, *};
use linewright_core::testing::*;
use linewright_core::transcript;
use linewright_core::trial::{self, Configuration, ConfigurationRefusal};
use linewright_core::work_settings::{Effort, RunKind, Setup, SetupCommand};
use serde_json::{Value, json};

/// A trial's output, how its run ended or why it did not run, and each
/// usage limit its sessions reported.
pub(crate) struct RanTrial {
    pub(crate) out: PathBuf,
    ended: Result<Ending, trial::Unrun>,
    usage_limits: Vec<UsageLimit>,
}

/// Runs the task in `dir` as the benchmark runner does, with the sessions
/// `harness` scripts and the factory config `adjust` makes of the tests'
/// own, its work directory `REPO`'s and its output under `under`, and waits
/// for what it leaves to remove.
async fn ran_trial(
    dir: &Path,
    under: &Path,
    harness: Arc<ScriptedHarness>,
    adjust: impl FnOnce(&mut FactoryConfig),
) -> RanTrial {
    ran_trial_with(dir, under, harness, adjust, None).await
}

/// [`ran_trial`], with `configuration` if one is given.
pub(crate) async fn ran_trial_with(
    dir: &Path,
    under: &Path,
    harness: Arc<ScriptedHarness>,
    adjust: impl FnOnce(&mut FactoryConfig),
    configuration: Option<&Configuration>,
) -> RanTrial {
    let task = Task::open(dir)
        .await
        .unwrap_or_else(|e| panic!("opening the case: {e}"));
    let out = under.join("out");
    let output = trial::Output::make(&out).unwrap_or_else(|e| panic!("the output: {e}"));
    let mut config = config(&under.join("work").join(REPO)).await;
    adjust(&mut config);
    let winding_up = config.winding_up.clone();
    let (runner_under, mut reported) = trial_runs::under_reporting();
    let ended = Box::pin(trial::run(
        &task,
        &output,
        harness,
        config,
        configuration,
        &runner_under,
    ))
    .await;
    winding_up.wait().await;
    let mut usage_limits = vec![];
    while let Ok(limit) = reported.try_recv() {
        usage_limits.push(limit);
    }
    RanTrial {
        out,
        ended,
        usage_limits,
    }
}

impl RanTrial {
    fn read(&self, name: &str) -> String {
        let path = self.out.join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    }

    /// Each write, in order: its kind, and its fields.
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
    fn writes_of(&self, kind: &str) -> Vec<Value> {
        self.writes()
            .into_iter()
            .filter(|(written, _)| written == kind)
            .map(|(_, fields)| fields)
            .collect()
    }
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
    assert_eq!(
        ran_trial.read("version"),
        format!("lw {} ({COMMIT})\n", env!("CARGO_PKG_VERSION"))
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
fn assert_seconds(part: &str) {
    let seconds = part.strip_suffix('\n').map(str::parse::<f64>);
    assert!(
        matches!(seconds, Some(Ok(seconds)) if seconds >= 0.0),
        "{part:?} is not a number of seconds"
    );
}

/// The cost a trial's output keeps, its time checked and left out.
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
        edit_work(config, |work| {
            work.pipeline_flags_and_env.setup.command = Some(Setup::Command(
                SetupCommand::new("echo the build broke; exit 3".into()).unwrap(),
            ));
        });
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
    let task = task(&world, &format!("{}/design-1", issue.0)).await;
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
    let task = task(&world, &format!("{}/implement-1", ticket.0)).await;
    let harness = world.harness.clone();
    let under = tempfile::tempdir().unwrap();

    let ran_trial = ran_trial(&task, under.path(), harness, |config| {
        edit_work(config, |work| {
            work.pipeline_flags_and_env.setup.command = Some(Setup::Command(
                SetupCommand::new("echo built the environment".into()).unwrap(),
            ));
        });
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
        edit_work(config, |work| {
            work.pipeline_flags_and_env.setup.command = Some(Setup::Command(
                SetupCommand::new("echo built the environment".into()).unwrap(),
            ));
        });
    };
    let first = ran_trial(&task, under.path(), harness.clone(), setup).await;
    assert_eq!(first.ended.as_ref().unwrap(), &Ending::Done);
    assert!(first.out.join("build.log").is_file());
    std::fs::remove_dir_all(&first.out).unwrap();

    let second = ran_trial(&task, under.path(), harness, setup).await;

    assert_eq!(second.ended.as_ref().unwrap(), &Ending::Done);
    assert!(!second.out.join("build.log").exists());
}

/// `docs/specs/benchmarks.md`, Benchmark runs, Holds: the usage limit
/// applies to a trial as to a run of the pipeline (`docs/specs/runs.md`,
/// Usage limit).
#[test_log::test(tokio::test)]
async fn a_session_refused_for_the_usage_limit_reports_it_and_resumes_once_it_resets() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let reset = soon_after(Task::open(&task).await.unwrap().start());
    let harness = Arc::new(ScriptedHarness::default());
    let followups = Arc::new(Mutex::new(vec![]));
    harness.on(WorkKind::Triage, {
        let (followups, reset) = (followups.clone(), reset.clone());
        move |call| {
            let mut followups = followups.lock().unwrap();
            followups.push(call.followup.map(str::to_string));
            if followups.len() == 1 {
                refused_until(reset.at(), call)
            } else {
                Ok(triaged())
            }
        }
    });
    let under = tempfile::tempdir().unwrap();

    let ran_trial = ran_trial(&task, under.path(), harness, |_| {}).await;

    assert_eq!(ran_trial.ended.as_ref().unwrap(), &Ending::Done);
    assert_eq!(ran_trial.read("outcome"), "done\n");
    let resets_at = reset.given();
    assert_eq!(ran_trial.usage_limits, [UsageLimit { resets_at }]);
    let follow_up = marker("prompts/follow-ups/usage-limit-reset.md", &[]);
    assert_eq!(*followups.lock().unwrap(), [None, Some(follow_up)]);
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

/// Linewright's own text of the file at `path` under `briefs/`, below a
/// line `marker`.
fn marked(path: &str, marker: &str) -> String {
    let own = linewright_core::testing::repository()
        .join("briefs")
        .join(path);
    let own =
        std::fs::read_to_string(&own).unwrap_or_else(|e| panic!("reading {}: {e}", own.display()));
    format!("{marker}\n{own}")
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
        |config| {
            edit_work(config, |work| {
                work.flags_and_env.model = Some(model("flag-model"));
            });
        },
        Some(&configuration),
    )
    .await;

    assert_eq!(ran_trial.ended.as_ref().unwrap(), &Ending::Done);
    assert_eq!(
        *ran_with.lock().unwrap(),
        vec![(model("configuration-model"), Effort::Low)]
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
