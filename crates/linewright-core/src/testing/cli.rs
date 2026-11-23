use std::path::Path;

use super::world::World;

pub const REPO: &str = "owner/widgets";

/// The directory of `serve`'s socket with `runtime_dir` as
/// `XDG_RUNTIME_DIR`, where `lw` looks for it.
pub fn socket_dir_in(runtime_dir: &Path) -> std::path::PathBuf {
    crate::api::socket_dir(Some(runtime_dir.to_path_buf()), Path::new("/unused"), 0)
}

/// A world whose work dir is `REPO`'s in a temporary directory, and that
/// directory.
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

/// Puts the `tool` on the tests' path in `bin`.
#[track_caller]
pub fn link_from_path(bin: &Path, tool: &str) {
    let found = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join(tool))
        .find(|path| path.is_file())
        .unwrap();
    std::os::unix::fs::symlink(found, bin.join(tool)).unwrap();
}
