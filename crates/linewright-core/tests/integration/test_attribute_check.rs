//! `scripts/test_attributes.py`: every test is `#[test_log::test]`, around
//! `#[tokio::test]` where it is async.

use std::path::Path;
use std::process::Command;

const USE_TEST_LOG: &str =
    "use `#[test_log::test]`, around `#[tokio::test]` where async: run `scripts/fix test`";

/// A repository holding `files`, each a path and its content, and the scanner.
fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let tree = tempfile::tempdir().unwrap();
    linewright_core::testing::write_files(tree.path(), files);
    std::fs::create_dir_all(tree.path().join("scripts")).unwrap();
    std::fs::copy(
        linewright_core::testing::repository().join("scripts/test_attributes.py"),
        tree.path().join("scripts/test_attributes.py"),
    )
    .unwrap();
    tree
}

/// What the scanner prints with `--check` in `dir`, one line per test it
/// reports, checking that its status agrees.
fn reports(dir: &Path) -> Vec<String> {
    let output = scanner(dir, &["--check"]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let reports: Vec<String> = stdout.lines().map(str::to_string).collect();
    assert_eq!(
        output.status.code(),
        Some(i32::from(!reports.is_empty())),
        "the check's status disagrees with its output:\n{stdout}"
    );
    reports
}

/// Runs the scanner without `--check` in `dir`, and returns the file at `path`.
fn after_fix(dir: &Path, path: &str) -> String {
    let output = scanner(dir, &[]);
    assert!(
        output.status.success(),
        "the fix failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read_to_string(dir.join(path)).unwrap()
}

/// Runs the scanner in `dir` with `args`.
fn scanner(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("python3")
        .args(["-I", "-B", "scripts/test_attributes.py"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

const BARE_TESTS: &str = "#[test]\nfn a() {}\n\nmod m {\n    #[tokio::test]\n    async fn b() {}\n\n    #[tokio::test(flavor = \"multi_thread\")]  \n    async fn c() {}\n}\n";

#[test_log::test]
fn a_bare_test_is_reported_at_its_attribute_in_path_then_line_order() {
    let tree = tree(&[
        ("crates/b/tests/t.rs", BARE_TESTS),
        ("crates/a/src/lib.rs", "fn f() {}\n\n#[test]\nfn g() {}\n"),
    ]);

    assert_eq!(
        reports(tree.path()),
        [
            format!("crates/a/src/lib.rs:3: {USE_TEST_LOG}"),
            format!("crates/b/tests/t.rs:1: {USE_TEST_LOG}"),
            format!("crates/b/tests/t.rs:5: {USE_TEST_LOG}"),
            format!("crates/b/tests/t.rs:8: {USE_TEST_LOG}"),
        ]
    );
}

#[test_log::test]
fn the_fix_makes_a_bare_test_test_log_and_touches_nothing_else() {
    let tree = tree(&[("crates/b/tests/t.rs", BARE_TESTS)]);

    assert_eq!(
        after_fix(tree.path(), "crates/b/tests/t.rs"),
        "#[test_log::test]\nfn a() {}\n\nmod m {\n    #[test_log::test(tokio::test)]\n    async fn b() {}\n\n    #[test_log::test(tokio::test(flavor = \"multi_thread\"))]\n    async fn c() {}\n}\n"
    );
    assert_eq!(reports(tree.path()), Vec::<String>::new());
}

#[test_log::test]
fn a_test_log_test_or_an_attribute_sharing_its_line_is_not_reported() {
    let source = "#[test_log::test]\nfn a() {}\n\n#[test_log::test(tokio::test)]\nasync fn b() {}\n\n// #[test]\nconst S: &str = \"#[test] fn c() {}\";\n";
    let tree = tree(&[("crates/a/tests/t.rs", source)]);

    assert_eq!(reports(tree.path()), Vec::<String>::new());
    assert_eq!(after_fix(tree.path(), "crates/a/tests/t.rs"), source);
}

#[test_log::test]
fn only_an_rs_file_that_is_not_a_symlink_is_scanned() {
    let tree = tree(&[
        ("crates/a/notes.md", "#[test]\n"),
        ("outside.rs", "#[test]\nfn a() {}\n"),
    ]);
    std::os::unix::fs::symlink(
        tree.path().join("outside.rs"),
        tree.path().join("crates/a/linked.rs"),
    )
    .unwrap();

    assert_eq!(reports(tree.path()), Vec::<String>::new());
    assert_eq!(after_fix(tree.path(), "outside.rs"), "#[test]\nfn a() {}\n");
}
