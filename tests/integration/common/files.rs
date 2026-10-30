use std::path::Path;
use std::process::Command;

/// Writes each of `files`, a path under `dir` and what it holds.
pub fn write_files(dir: &Path, files: &[(&str, &str)]) {
    for (path, contents) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
}

pub fn commit_in(dir: &Path, files: &[(&str, &str)], message: &str) {
    write_files(dir, files);
    git_in(dir, &["add", "--all"]);
    git_in(dir, &["commit", "--quiet", "-m", message]);
}

/// Runs git in `dir`, failing the test if git fails, and returns what it
/// printed.
pub fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Runs git in `dir`, failing the test only if git does not start, and
/// returns how it ended.
pub fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
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
