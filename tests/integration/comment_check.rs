//! `scripts/check comments`: every comment is one of the four kinds
//! `CODING_STANDARDS.md` allows.

use std::path::Path;
use std::process::Command;

/// What `scripts/check comments` reports on a repository that tracks
/// `files`, each a path and its content: one line per comment block or file
/// it fails.
#[track_caller]
fn reports(files: &[(&str, &str)]) -> Vec<String> {
    let tree = tempfile::tempdir().unwrap();
    let dir = tree.path();
    git(dir, &["init", "--quiet"]);
    for (path, content) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
    }
    git(dir, &["add", "--all"]);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    for script in [
        "scripts/check",
        "scripts/lib.sh",
        "scripts/check_comments.py",
    ] {
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join(script),
            dir.join(script),
        )
        .unwrap();
    }
    let output = Command::new("bash")
        .args([dir.join("scripts/check").as_os_str(), "comments".as_ref()])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let reports: Vec<String> = stdout
        .lines()
        .filter(|line| {
            !(line.starts_with("✗ ") || line.starts_with("failed: ") || line.starts_with("ok: "))
        })
        .map(str::to_string)
        .collect();
    assert_eq!(
        output.status.success(),
        reports.is_empty(),
        "the check's status disagrees with its output:\n{stdout}"
    );
    reports
}

#[track_caller]
fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

const NOT_ALLOWED: &str =
    "not a contract, a linked outside constraint, a directive or a license notice";

#[test_log::test]
fn a_comment_that_narrates_fails_at_the_first_line_of_its_block() {
    let source = "fn f() {\n    // Adds one,\n    // then two.\n    let x = 1; // and three\n}\n";

    assert_eq!(
        reports(&[("src/f.rs", source)]),
        [
            format!("src/f.rs:2: {NOT_ALLOWED}"),
            format!("src/f.rs:4: {NOT_ALLOWED}"),
        ]
    );
}

#[test_log::test]
fn a_comment_in_toml_or_yaml_fails() {
    assert_eq!(
        reports(&[
            ("Cargo.toml", "[package]\n# The name.\nname = \"x\"\n"),
            ("ci.yml", "on: push # on every push\n"),
        ]),
        [
            format!("Cargo.toml:2: {NOT_ALLOWED}"),
            format!("ci.yml:1: {NOT_ALLOWED}"),
        ]
    );
}

#[test_log::test]
fn a_python_comment_fails_and_a_docstring_or_string_does_not() {
    let python = "#!/usr/bin/env python3\n\
                  \"\"\"The module. # not a comment\"\"\"\n\
                  a = '# no' + \"# no\" + r'\\\\' # Narrates.\n\
                  # https://example.com\n";

    assert_eq!(
        reports(&[("scripts/x.py", python)]),
        [format!("scripts/x.py:3: {NOT_ALLOWED}")]
    );
}

#[test_log::test]
fn a_template_comment_passes_only_directly_above_a_macro() {
    let template = "{#- The layout. -#}\n<main></main>\n\
                    {#- A badge,\n    in its tone. -#}\n{%- macro badge(tone) -%}\n{%- endmacro -%}\n\
                    <!-- The footer. -->\n";

    assert_eq!(
        reports(&[("templates/page.html", template)]),
        [
            format!("templates/page.html:1: {NOT_ALLOWED}"),
            format!("templates/page.html:7: {NOT_ALLOWED}"),
        ]
    );
}

#[test_log::test]
fn a_shellcheck_directive_needs_a_reason() {
    let script = "#!/usr/bin/env bash\n\
                  # shellcheck disable=SC2086\n\
                  echo $a # shellcheck disable=SC2086 # one word\n";

    assert_eq!(
        reports(&[("run", script)]),
        ["run:2: a `# shellcheck disable=` needs a `# reason` after it"]
    );
}

#[test_log::test]
fn shellchecks_shell_and_source_directives_pass() {
    let script = "#!/usr/bin/env bash\n\
                  # shellcheck source=lib.sh\n\
                  . lib.sh\n";
    let lib = "# shellcheck shell=bash\n\
               # shellcheck disable=SC2034 # read by run\n\
               greeting=hi\n";

    assert_eq!(
        reports(&[("run", script), ("lib.sh", lib)]),
        Vec::<String>::new()
    );
}

#[test_log::test]
fn the_four_kinds_pass() {
    let rust = "//! The crate.\n\
                // SPDX-License-Identifier: MIT\n\
                /// One.\n\
                fn one() -> u8 { 1 }\n\
                /** Two. */\n\
                fn two() -> u8 { 2 } // https://example.com/two\n";
    let script = "#!/usr/bin/env bash\n# Greets `$1`.\ngreet() {\n  echo \"hi $1\"\n}\n";
    let dockerfile = "# syntax=docker/dockerfile:1\nFROM debian\n";

    assert_eq!(
        reports(&[
            ("src/lib.rs", rust),
            ("greet", script),
            ("Dockerfile", dockerfile)
        ]),
        Vec::<String>::new()
    );
}

#[test_log::test]
fn what_only_looks_like_a_comment_inside_a_string_passes() {
    let rust = "fn f() {\n\
                let a = \"// https://example.com and // more\";\n\
                let b = r#\"a \" // b\"#;\n\
                let c = '\"';\n\
                let d = \"/* not */\";\n\
                }\n";
    let js = "const r = /a\\/\\/b/;\nconst d = 4 / 2;\nconst t = `// ${d + \"//\"} //`;\n";
    let yaml =
        "steps:\n  - run: |\n      # shell, in a block scalar\n      echo hi\n  - k: \"a # b\"\n";
    let toml = "a = \"# no\"\nb = '''\n# no\n'''\n";
    let script = "#!/usr/bin/env bash\n\
                  echo \"# no\" $# ${#x} 'a # b' \"$(echo \"# no\")\"\n\
                  cat <<'END'\n# no\nEND\n";
    let css = "a { content: \"/* no */\"; }\n";

    assert_eq!(
        reports(&[
            ("src/f.rs", rust),
            ("assets/app.js", js),
            ("ci.yml", yaml),
            ("x.toml", toml),
            ("run", script),
            ("styles/app.css", css),
        ]),
        Vec::<String>::new()
    );
}

#[test_log::test]
fn the_model_vendored_files_and_files_without_comments_are_not_checked() {
    assert_eq!(
        reports(&[
            ("quint/x.qnt", "// Narrates.\n"),
            ("assets/htmx.min.js", "// Narrates.\n"),
            ("styles/daisyui.mjs", "// Narrates.\n"),
            ("README.md", "<!-- Narrates. -->\n"),
            ("notes.txt", "# Narrates.\n"),
        ]),
        Vec::<String>::new()
    );
}
