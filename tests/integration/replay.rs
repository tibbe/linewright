//! Replaying a case, through the system: its stage run on the case in
//! place of GitHub, and what it keeps in its output
//! (`docs/specs/benchmarks.md`, Replaying, The output of a replay).

use crate::common;

use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::*;
use linewright::case::{self, Case, CaseName, Criteria};
use linewright::domain::Number;
use linewright::factory::FactoryConfig;
use linewright::harness::{GhReach, SessionSpec};
use linewright::reconcile::WorkKind;
use linewright::replay::{self, OutputRefusal};
use linewright::runs::{Ending, Runs};
use linewright::store::github::SESSION_TOKEN_VARIABLE;
use linewright::work_settings::{RunKind, Setup, SetupCommand};
use serde_json::{Value, json};

/// What each triage answers: a bug, ready for an agent.
fn triaged() -> String {
    json!({
        "outcome": "ready-for-agent",
        "category": "bug",
        "comment": "## Agent Brief\n\n**Summary:** It crashes."
    })
    .to_string()
}

/// Captures `world`'s run `run` as a case, and gives its directory.
#[track_caller]
fn captured<'a>(world: &'a World, run: &'a str) -> impl Future<Output = PathBuf> + 'a {
    let caller = Location::caller();
    case_at(caller, world, run)
}

/// A triage run of `world` on an issue, captured as a case: the issue and
/// the case's directory.
#[track_caller]
fn triage_case(world: &mut World) -> impl Future<Output = (Number, PathBuf)> + '_ {
    let caller = Location::caller();
    async move {
        world.harness.on(WorkKind::Triage, |_| Ok(triaged()));
        let issue =
            world
                .store
                .open_issue("reporter", "Export fails", "It crashes.", &["needs-triage"]);
        world.poll().await;
        let run = format!("{}/triage-1", issue.0);
        let case = case_at(caller, world, &run).await;
        (issue, case)
    }
}

/// [`captured`], failing at `caller`.
async fn case_at(caller: &Location<'_>, world: &World, run: &str) -> PathBuf {
    let work = world.dir.path().join("work");
    let id = run
        .parse()
        .unwrap_or_else(|e| panic!("the run {run} at {caller}: {e}"));
    let capturable = case::capturable(
        &Runs::new(&work),
        &work.join(case::CASES),
        &id,
        &CaseName::suggested(&id),
    )
    .unwrap_or_else(|e| panic!("capturing {run} at {caller}: {e}"));
    let criteria = Criteria::parse("It triages the issue.\n")
        .unwrap_or_else(|| panic!("blank criteria at {caller}"));
    case::capture(&*world.store, capturable, &criteria)
        .await
        .unwrap_or_else(|e| panic!("capturing {run} at {caller}: {e}"))
}

/// A replay's output, and how its run ended or why it did not run.
struct Replayed {
    out: PathBuf,
    ended: anyhow::Result<Ending>,
}

/// Replays the case in `dir` with the sessions `harness` scripts and the
/// factory config `adjust` makes of the tests' own, its work directory
/// `REPO`'s and its output under `under`, and waits for what it leaves to
/// remove.
#[track_caller]
fn replayed<'a>(
    dir: &'a Path,
    under: &'a Path,
    harness: Arc<ScriptedHarness>,
    adjust: impl FnOnce(&mut FactoryConfig) + 'a,
) -> impl Future<Output = Replayed> + 'a {
    let caller = Location::caller();
    async move {
        let case = Case::open(dir)
            .await
            .unwrap_or_else(|e| panic!("opening the case at {caller}: {e}"));
        let out = under.join("out");
        let output =
            replay::Output::make(&out).unwrap_or_else(|e| panic!("the output at {caller}: {e}"));
        let mut config = config(&under.join("work").join(REPO));
        adjust(&mut config);
        let winding_up = config.winding_up.clone();
        let ended = replay::run(&case, &output, harness, config).await;
        winding_up.wait().await;
        Replayed { out, ended }
    }
}

impl Replayed {
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
fn entries(dir: &Path) -> Vec<String> {
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
async fn a_triage_case_is_replayed_into_its_output() {
    let mut world = World::new().await;
    let (issue, case) = triage_case(&mut world).await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, |_| Ok(triaged()));
    let under = tempfile::tempdir().unwrap();

    let replayed = replayed(&case, under.path(), harness.clone(), |_| {}).await;

    assert_eq!(replayed.ended.as_ref().unwrap(), &Ending::Done);
    assert_eq!(replayed.read("outcome"), "done\n");
    let version = std::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        replayed.read("version"),
        String::from_utf8(version.stdout).unwrap()
    );
    assert_eq!(replayed.read("case"), format!("{}/triage-1\n", issue.0));
    assert_eq!(harness.prompts(WorkKind::Triage).len(), 1);
    let replays = under.path().join("work").join(REPO).join("replays");
    assert_eq!(entries(&replays.join("clones")), Vec::<String>::new());
    assert_eq!(
        entries(&replayed.out.join(format!("runs/items/{}", issue.0))),
        vec!["triage-1.json"]
    );
    assert_eq!(
        files_named(&replayed.out.join("sessions"), "output.jsonl").len(),
        1
    );
    let n = issue.0;
    let label = |kind: &str, label: &str| (kind.to_string(), json!({"number": n, "label": label}));
    assert_eq!(
        replayed.writes(),
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
        replayed
            .read("writes/0001-comment.md")
            .contains("**Summary:** It crashes."),
        "{}",
        replayed.read("writes/0001-comment.md")
    );
}

#[test_log::test(tokio::test)]
async fn a_design_case_is_replayed_numbering_its_tickets_above_the_cases_items() {
    let mut world = World::new().await;
    tickets_script(&world);
    let issue = open_undesigned_gadgets(&world);
    world.poll().await;
    let case = captured(&world, &format!("{}/design-1", issue.0)).await;
    for _ in 0..3 {
        open_ticket(&world, "Opened since");
    }
    let harness = world.harness.clone();
    let under = tempfile::tempdir().unwrap();

    let replayed = replayed(&case, under.path(), harness, |_| {}).await;

    assert_eq!(replayed.ended.as_ref().unwrap(), &Ending::Done);
    assert_eq!(replayed.read("outcome"), "done\n");
    let created: Vec<Value> = replayed
        .writes_of("create-issue")
        .into_iter()
        .map(|fields| fields["title"].clone())
        .collect();
    assert_eq!(created, vec![json!("Store gadgets"), json!("List gadgets")]);
    assert_eq!(
        replayed.writes_of("add-sub-issue"),
        vec![
            json!({"parent": issue.0, "child": issue.0 + 1}),
            json!({"parent": issue.0, "child": issue.0 + 2}),
        ]
    );
}

#[test_log::test(tokio::test)]
async fn an_implement_case_is_replayed_pushing_its_branch_into_the_output() {
    let mut world = World::new().await;
    implement_gadgets(&world);
    let ticket = open_designed_gadgets(&world);
    world.poll().await;
    let case = captured(&world, &format!("{}/implement-1", ticket.0)).await;
    let harness = world.harness.clone();
    let under = tempfile::tempdir().unwrap();

    let replayed = replayed(&case, under.path(), harness, |config| {
        config.work.pipeline_flags_and_env.setup.command = Some(Setup::Command(
            SetupCommand::new("echo built the environment".into()).unwrap(),
        ));
    })
    .await;

    assert_eq!(replayed.ended.as_ref().unwrap(), &Ending::Done);
    assert_eq!(replayed.read("outcome"), "done\n");
    assert!(
        replayed.read("build.log").contains("built the environment"),
        "{}",
        replayed.read("build.log")
    );
    let pushed: Vec<String> = replayed
        .writes_of("push")
        .into_iter()
        .map(|fields| fields["branch"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(pushed.len(), 1, "{pushed:?}");
    let file = git_in(
        &replayed.out.join("commits.git"),
        &["show", &format!("{}:src/issue-{}.txt", pushed[0], ticket.0)],
    );
    assert_eq!(file, "gadget\n");
    assert_eq!(
        replayed.writes_of("open-pull"),
        vec![json!({"head": pushed[0], "title": "Add gadgets"})]
    );
}

/// What the socket that the configuration of `gh` in the environment of
/// `spec`'s session names answers a `GET` of `target`, as `gh` sends it:
/// the whole answer, or why there is none.
#[track_caller]
fn gh_answer(spec: &SessionSpec, target: &str) -> String {
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
async fn a_replays_sessions_gh_is_refused_the_cases_repository() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
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

    let replayed = replayed(&case, under.path(), harness, |_| {}).await;

    assert_eq!(replayed.ended.as_ref().unwrap(), &Ending::Done);
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
async fn a_later_replay_from_the_same_head_uses_the_environment_an_earlier_one_built() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, |_| Ok(triaged()));
    let under = tempfile::tempdir().unwrap();
    let setup = |config: &mut FactoryConfig| {
        config.work.pipeline_flags_and_env.setup.command = Some(Setup::Command(
            SetupCommand::new("echo built the environment".into()).unwrap(),
        ));
    };
    let first = replayed(&case, under.path(), harness.clone(), setup).await;
    assert_eq!(first.ended.as_ref().unwrap(), &Ending::Done);
    assert!(first.out.join("build.log").is_file());
    std::fs::remove_dir_all(&first.out).unwrap();

    let second = replayed(&case, under.path(), harness, setup).await;

    assert_eq!(second.ended.as_ref().unwrap(), &Ending::Done);
    assert!(!second.out.join("build.log").exists());
}

#[test_log::test(tokio::test)]
async fn a_session_refused_for_the_usage_limit_ends_the_replays_run_for_it() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, refused_for_the_usage_limit);
    let under = tempfile::tempdir().unwrap();

    let replayed = replayed(&case, under.path(), harness, |config| {
        config.waits_out_usage_limit = true;
    })
    .await;

    assert_eq!(replayed.ended.as_ref().unwrap(), &Ending::UsageLimit);
    assert_eq!(replayed.read("outcome"), "usage-limit\n");
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn a_replays_run_does_not_count_toward_max_runs() {
    let mut world = World::with(|config| config.max_runs = 1).await;
    let (_, case) = triage_case(&mut world).await;
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
    let replaying = {
        let under = under.path().to_path_buf();
        tokio::spawn(async move {
            replayed(&case, &under, harness, |config| config.work_dir = work)
                .await
                .ended
                .map_err(|e| format!("{e:#}"))
        })
    };
    gate.begun().await;

    world.poll().await;

    assert_eq!(ended_runs(&world), vec![Ending::Done, Ending::Done]);
    gate.release();
    assert_eq!(replaying.await.unwrap(), Ok(Ending::HandedBack));
}

/// Commits `files` to the default branch of the case in `case`.
#[track_caller]
fn commit_to_case(case: &Path, files: &[(&str, &str)]) {
    let checkout = tempfile::tempdir().unwrap();
    let repository = case.join("repository.git");
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
async fn a_case_whose_default_branch_holds_a_file_that_would_hold_the_run_is_refused_naming_it() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
    commit_to_case(&case, &[(".linewright/config.toml", "model = [\n")]);
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, |_| Ok(triaged()));
    let under = tempfile::tempdir().unwrap();

    let replayed = replayed(&case, under.path(), harness.clone(), |_| {}).await;

    let error = format!("{:#}", replayed.ended.unwrap_err());
    assert!(
        error.contains(".linewright/config.toml") && error.contains("would hold the run"),
        "{error}"
    );
    assert_eq!(harness.prompts(WorkKind::Triage).len(), 0);
    assert!(!replayed.out.join("outcome").exists());
}

#[test_log::test(tokio::test)]
async fn a_replays_setting_refuses_a_file_that_would_hold_the_kind_of_run_it_is_prepared_for() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
    commit_to_case(
        &case,
        &[(".linewright/prompts/implement.md", "Do {{nope}}\n")],
    );
    let case = Case::open(&case).await.unwrap();
    let under = tempfile::tempdir().unwrap();
    let config = config(&under.path().join("work").join(REPO));
    let prepare = |name: &'static str, kind: WorkKind| {
        let case = &case;
        let config = &config;
        let dir = under.path().join(name);
        async move {
            replay::Setting::prepare(
                case,
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
        error.contains(".linewright/prompts/implement.md") && error.contains("would hold the run"),
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

    let refused = replay::Output::make(&held);
    let made = replay::Output::make(&missing);

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
fn lw_isolated<'a>(
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
fn lw_command(bin: &Path, work: &Path, cwd: &Path) -> tokio::process::Command {
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
async fn without_linewrights_token_lw_replay_exits_1_naming_its_variables_and_runs_no_session() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "replay",
            case.to_str().unwrap(),
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
async fn lw_replay_refuses_a_case_without_criteria_naming_them() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
    std::fs::remove_file(case.join("criteria.md")).unwrap();
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "replay",
            case.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        &[],
    )
    .await;

    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert!(
        ran.stderr
            .contains("the case cannot be replayed: the case's criteria: "),
        "{}",
        ran.stderr
    );
    assert!(!bin.path().join("tokens.log").exists());
}

#[test_log::test(tokio::test)]
async fn lw_replay_gives_sessions_linewrights_token_with_repo_set_or_unset() {
    let mut world = World::new().await;
    let (issue, case) = triage_case(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let case_arg = case.to_str().unwrap();
    let unset = work.path().join("unset");
    let other = work.path().join("other");

    let unset_ran = lw_isolated(
        bin.path(),
        work.path(),
        &["replay", case_arg, "--out", unset.to_str().unwrap()],
        &[],
    )
    .await;
    let other_ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "--repo",
            "someone/else",
            "replay",
            case_arg,
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
    assert!(work.path().join(REPO).join("replays").is_dir());
    assert!(!work.path().join("someone").exists());
}

#[test_log::test(tokio::test)]
async fn lw_replay_warns_that_sessions_given_linewrights_token_can_write_to_github() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "--log-format",
            "json",
            "replay",
            case.to_str().unwrap(),
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
async fn lw_replay_gives_sessions_the_separate_token_when_it_is_set() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "replay",
            case.to_str().unwrap(),
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
async fn lw_replay_does_not_count_gh_config_dir_among_the_variables_sessions_are_not_given() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "--log-format",
            "json",
            "replay",
            case.to_str().unwrap(),
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
async fn lw_replay_refuses_a_case_that_lacks_a_part_with_status_1_naming_the_part() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
    std::fs::remove_file(case.join("properties.json")).unwrap();
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "replay",
            case.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        &[],
    )
    .await;

    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert!(
        ran.stderr.contains("the case's properties"),
        "{}",
        ran.stderr
    );
    assert!(!bin.path().join("tokens.log").exists());
}

#[test_log::test(tokio::test)]
async fn lw_replay_refuses_an_output_that_holds_anything_with_status_1() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");
    std::fs::create_dir(&out).unwrap();
    std::fs::write(out.join("notes.md"), "Mine.\n").unwrap();

    let ran = lw_isolated(
        bin.path(),
        work.path(),
        &[
            "replay",
            case.to_str().unwrap(),
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
async fn a_signal_ends_lw_replay_at_once_leaving_its_output_interrupted() {
    let mut world = World::new().await;
    let (issue, case) = triage_case(&mut world).await;
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
    let mut replay = lw_command(bin.path(), work.path(), work.path())
        .arg("replay")
        .arg(&case)
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

    let pid = replay.id().unwrap().to_string();
    let sent = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap();

    assert!(sent.success());
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), replay.wait())
        .await
        .expect("the replay did not end at once")
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
}

#[test_log::test(tokio::test)]
async fn a_replay_removes_the_directory_of_one_a_signal_ended() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
    let under = tempfile::tempdir().unwrap();
    let left = under
        .path()
        .join("work")
        .join(REPO)
        .join("replays/clones/replay-left");
    std::fs::create_dir_all(left.join("store")).unwrap();
    let harness = Arc::new(ScriptedHarness::default());
    harness.on(WorkKind::Triage, |_| Ok(triaged()));

    let replayed = replayed(&case, under.path(), harness, |_| {}).await;

    assert!(replayed.ended.is_ok(), "{:?}", replayed.ended);
    assert!(!left.exists());
}

#[test_log::test(tokio::test)]
async fn lw_help_lists_replay() {
    let bin = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();

    let ran = lw_isolated(bin.path(), work.path(), &["--help"], &[]).await;

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    assert!(
        ran.stdout
            .lines()
            .any(|line| line.trim_start().starts_with("replay ")),
        "{}",
        ran.stdout
    );
}

#[test_log::test(tokio::test)]
async fn a_signal_during_the_build_ends_the_build_with_lw_replay() {
    let mut world = World::new().await;
    let (_, case) = triage_case(&mut world).await;
    let bin = bin_triaging();
    link_from_path(bin.path(), "sleep");
    let work = tempfile::tempdir().unwrap();
    let pid = work.path().join("build.pid");
    let setup = format!("echo $$ > {}; exec sleep 300", pid.display());
    let mut replay = lw_command(bin.path(), work.path(), work.path())
        .arg("replay")
        .arg(&case)
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
        .args(["-TERM", &replay.id().unwrap().to_string()])
        .status()
        .unwrap();

    assert!(sent.success());
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), replay.wait())
        .await
        .expect("the replay did not end at once")
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
