use std::path::Path;

use super::world::World;

pub const REPO: &str = "owner/widgets";

/// A world whose work dir is `REPO`'s in a temporary directory, the one
/// `lw_in` is given.
pub async fn world_in_work_dir() -> (World, tempfile::TempDir) {
    let work_dir = tempfile::tempdir().unwrap();
    let world = World::with(|config| config.work_dir = work_dir.path().join(REPO)).await;
    (world, work_dir)
}

/// The objects of JSON `lines`, one per line, as `logs --json` prints them.
pub fn json_objects(lines: &str) -> Vec<serde_json::Value> {
    lines
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `lw` on `REPO` with `args` and the work dir `work_dir`, from a
/// directory of its own.
pub async fn lw_in(work_dir: &Path, args: &[&str]) -> Output {
    let runtime_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_lw"))
        .args(["--repo", REPO, "--work-dir"])
        .arg(work_dir)
        .args(args)
        .current_dir(cwd.path())
        .env("XDG_RUNTIME_DIR", runtime_dir.path())
        .output()
        .await
        .unwrap();
    Output {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}
