//! Embeds the commit Linewright is built from as `LINEWRIGHT_COMMIT`: the
//! checkout's `HEAD`, else the `LINEWRIGHT_COMMIT` the build is given (the
//! Docker build has no `.git`), else `unknown`
//! (`docs/specs/operation.md`).

use std::path::{Path, PathBuf};
use std::process::Command;

const COMMIT_VARIABLE: &str = "LINEWRIGHT_COMMIT";

/// As many hex digits as a commit is named by.
const COMMIT_DIGITS: usize = 12;

fn main() {
    println!("cargo::rerun-if-env-changed={COMMIT_VARIABLE}");
    for path in head_paths() {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    let commit = commit(
        git(&["rev-parse", "HEAD"]),
        std::env::var(COMMIT_VARIABLE).ok(),
    );
    println!("cargo::rustc-env={COMMIT_VARIABLE}={commit}");
}

/// The commit to embed, abbreviated: `head`, the commit the checkout's `HEAD`
/// names, else `given`, the one the build is given, else `unknown`.
fn commit(head: Option<String>, given: Option<String>) -> String {
    head.or(given)
        .map(|commit| {
            commit
                .trim()
                .chars()
                .take(COMMIT_DIGITS)
                .collect::<String>()
        })
        .filter(|commit| !commit.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// The paths that change when `HEAD` moves: `HEAD` itself and the branch it
/// is on, loose or packed. A branch with no loose ref yet gets one in its
/// directory when it moves, so that directory stands in for it. Only paths
/// that exist, as Cargo reruns the script on every build for one that
/// doesn't.
fn head_paths() -> Vec<PathBuf> {
    let mut paths = git_path("HEAD").into_iter().collect::<Vec<_>>();
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        paths.extend(git_path(&branch).map(|loose| match loose.exists() {
            true => loose,
            false => loose.parent().map(Path::to_path_buf).unwrap_or(loose),
        }));
        paths.extend(git_path("packed-refs"));
    }
    paths.retain(|path| path.exists());
    paths
}

/// Where the repository keeps `name`, such as `HEAD`.
fn git_path(name: &str) -> Option<PathBuf> {
    git(&["rev-parse", "--git-path", name]).map(PathBuf::from)
}

/// What `git args` prints, trimmed, if it succeeds.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let out = String::from_utf8(output.stdout).ok()?;
    Some(out.trim().to_owned())
}
