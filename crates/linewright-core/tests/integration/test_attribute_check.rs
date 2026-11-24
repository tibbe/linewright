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
#[track_caller]
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
#[track_caller]
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
#[track_caller]
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

const RSTEST_TESTS: &str = r#"#[rstest]
#[case(1)]
fn plain(#[case] n: u8) {}

#[rstest::rstest] // A comment after it.
#[case::one(1)]
fn by_path(#[case] n: u8) {}

#[rstest]
#[case::spanning(
    "a ] that does not close it", // nor this (
    [1, 2],
    '[', 'é',
)]
// A comment in the run.

#[case::another("x", [3, 4], ']', 'e')]
fn multi_line(#[case] s: &str, #[case] v: [u8; 2], #[case] a: char, #[case] b: char) {}

mod m {
    mod n {
        #[rstest]
        #[case(1)]
        pub(crate) async fn nested(#[case] n: u8) {}
    }
}

#[test_log::test]
#[rstest]
#[case(1)]
fn test_log_above(#[case] n: u8) {}
"#;

#[test_log::test]
fn an_rstest_test_without_test_log_after_it_is_reported_at_its_rstest_line() {
    let tree = tree(&[("crates/a/tests/t.rs", RSTEST_TESTS)]);

    assert_eq!(
        reports(tree.path()),
        [1, 5, 9, 22, 29].map(|line| format!("crates/a/tests/t.rs:{line}: {USE_TEST_LOG}"))
    );
}

#[test_log::test]
fn the_fix_puts_test_log_just_before_an_rstest_tests_fn() {
    let tree = tree(&[("crates/a/tests/t.rs", RSTEST_TESTS)]);

    assert_eq!(
        after_fix(tree.path(), "crates/a/tests/t.rs"),
        r#"#[rstest]
#[case(1)]
#[test_log::test]
fn plain(#[case] n: u8) {}

#[rstest::rstest] // A comment after it.
#[case::one(1)]
#[test_log::test]
fn by_path(#[case] n: u8) {}

#[rstest]
#[case::spanning(
    "a ] that does not close it", // nor this (
    [1, 2],
    '[', 'é',
)]
// A comment in the run.

#[case::another("x", [3, 4], ']', 'e')]
#[test_log::test]
fn multi_line(#[case] s: &str, #[case] v: [u8; 2], #[case] a: char, #[case] b: char) {}

mod m {
    mod n {
        #[rstest]
        #[case(1)]
        #[test_log::test]
        pub(crate) async fn nested(#[case] n: u8) {}
    }
}

#[test_log::test]
#[rstest]
#[case(1)]
#[test_log::test]
fn test_log_above(#[case] n: u8) {}
"#
    );
    assert_eq!(reports(tree.path()), Vec::<String>::new());
}

#[test_log::test]
fn an_rstest_test_with_a_bare_tokio_test_is_reported_and_fixed_once() {
    let source = "#[rstest]\n#[case(1)]\n#[tokio::test]\nasync fn a(#[case] n: u8) {}\n\n#[tokio::test]\n#[rstest]\n#[case(1)]\nasync fn b(#[case] n: u8) {}\n";
    let tree = tree(&[("crates/a/tests/t.rs", source)]);

    assert_eq!(
        reports(tree.path()),
        [3, 6].map(|line| format!("crates/a/tests/t.rs:{line}: {USE_TEST_LOG}"))
    );
    assert_eq!(
        after_fix(tree.path(), "crates/a/tests/t.rs"),
        "#[rstest]\n#[case(1)]\n#[test_log::test(tokio::test)]\nasync fn a(#[case] n: u8) {}\n\n#[test_log::test(tokio::test)]\n#[rstest]\n#[case(1)]\nasync fn b(#[case] n: u8) {}\n"
    );
}

#[test_log::test]
fn an_rstest_test_with_test_log_a_fixture_or_an_rstest_in_a_comment_or_string_is_not_reported() {
    let source = r##"#[rstest]
#[case(1)]
#[test_log::test]
fn has_it(#[case] n: u8) {}

#[rstest]
#[case(1)]
#[test_log::test(tokio::test)]
async fn has_it_async(#[case] n: u8) {}

#[fixture]
fn a_fixture() -> u8 {
    1
}

// #[rstest]
fn not_a_test() {}

/*
#[rstest]
fn in_a_block_comment() {}
*/

/// ```
/// #[rstest]
/// fn in_a_doc() {}
/// ```
fn documented() {}

const S: &str = r#"
#[rstest]
fn in_a_string() {}
"#;
"##;
    let tree = tree(&[("crates/a/tests/t.rs", source)]);

    assert_eq!(reports(tree.path()), Vec::<String>::new());
    assert_eq!(after_fix(tree.path(), "crates/a/tests/t.rs"), source);
}
