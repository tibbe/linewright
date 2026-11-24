//! Running a task, through the system: its stage run on the task in
//! place of GitHub, and what it keeps in its output
//! (`docs/specs/benchmarks.md`, Trials, The output of a trial).

use std::path::Path;

use linewright_core::runs::RunId;
use linewright_core::runs::files::RunFiles;
use linewright_core::store::SESSION_TOKEN_VARIABLE;
use linewright_core::testing::*;
use serde_json::{Value, json};

use crate::programs::*;

/// Runs `lw` with `args`, `bin` as its path and `work` as its work
/// directory, from a directory of its own, with Linewright's token
/// `linewright-token` in `GH_TOKEN`, and neither `GITHUB_TOKEN`, the
/// separate token nor `repo` set, but as `env` sets them.
pub(crate) async fn lw_isolated(
    bin: &Path,
    work: &Path,
    args: &[&str],
    env: &[(&str, &str)],
) -> Output {
    let cwd = tempfile::tempdir().unwrap_or_else(|e| panic!("a directory: {e}"));
    let output = lw_command(bin, work, cwd.path())
        .args(args)
        .envs(env.iter().copied())
        .output()
        .await
        .unwrap_or_else(|e| panic!("running lw: {e}"));
    Output {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// `lw` with `bin` as its path and `work` as its work directory, run in
/// `cwd`, with Linewright's token `linewright-token` in `GH_TOKEN`, the
/// model `MODEL`, and neither `GITHUB_TOKEN`, the separate token nor `repo`
/// set.
pub(crate) fn lw_command(bin: &Path, work: &Path, cwd: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_lw"));
    command
        .arg("--work-dir")
        .arg(work)
        .current_dir(cwd)
        .env("PATH", bin)
        .env("XDG_RUNTIME_DIR", cwd)
        .env("GH_TOKEN", "linewright-token")
        .env("LINEWRIGHT_MODEL", MODEL)
        .env_remove("GITHUB_TOKEN")
        .env_remove(SESSION_TOKEN_VARIABLE)
        .env_remove("LINEWRIGHT_REPO");
    command
}

/// A directory to put on the path of `lw`, holding git, bash and a fake
/// `claude` whose every triage answers that the issue is a bug ready for an
/// agent, logging what it was given there.
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
async fn lw_trial_without_a_model_exits_saying_where_to_set_it() {
    let mut world = World::new().await;
    let (_, task) = triage_task(&mut world).await;
    let bin = bin_triaging();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    let ran = lw_command(bin.path(), work.path(), work.path())
        .args([
            "trial",
            task.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .env_remove("LINEWRIGHT_MODEL")
        .output()
        .await
        .unwrap();

    let stderr = String::from_utf8_lossy(&ran.stderr);
    assert_ne!(ran.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("`model` is required to start a session"),
        "{stderr}"
    );
    assert!(!out.exists(), "{stderr}");
}

#[test_log::test(tokio::test)]
async fn lw_trial_keeps_the_version_lw_prints_in_its_output() {
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
        &[],
    )
    .await;

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    let version = std::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(out.join("version")).unwrap(),
        String::from_utf8(version.stdout).unwrap()
    );
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
