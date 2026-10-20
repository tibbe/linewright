use std::path::Path;
use std::process::Command;

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
