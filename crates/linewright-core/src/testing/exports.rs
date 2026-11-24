use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The paths of the files under `dir`, relative to it.
pub fn files_under(dir: &Path) -> BTreeSet<PathBuf> {
    let mut files = BTreeSet::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        for entry in std::fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(path.strip_prefix(dir).unwrap().to_path_buf());
            }
        }
    }
    files
}
