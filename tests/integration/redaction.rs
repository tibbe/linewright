//! Redacting a secret in each form a kept text may hold it in
//! (`docs/specs/operation.md`, Secrets).

use crate::common;

use common::*;

/// A secret holding a format character (U+200B), DEL, a C1 control
/// (U+0085), a quote and a format character beyond the Basic Multilingual
/// Plane (U+E0041). Claude Code writes the first three and the last as
/// `\uXXXX` in its JSON; `serde_json` writes them as they are.
const SECRET: &str = "tok\u{200b}en\u{7f}-\u{85}\"9\u{e0041}";

/// `SECRET` as Claude Code 2.1.311 writes it inside a JSON string.
const IN_CLAUDE_CODE: &str = "tok\\u200ben\\u007f-\\u0085\\\"9\\udb40\\udc41";

/// `SECRET` as Claude Code writes it, with uppercase hex.
const IN_CLAUDE_CODE_UPPERCASE: &str = "tok\\u200Ben\\u007F-\\u0085\\\"9\\uDB40\\uDC41";

fn redacted(text: &str) -> String {
    redaction_of(&[("GH_TOKEN", SECRET)]).apply(text)
}

/// As `claude -p --output-format stream-json` 2.1.311 writes it.
#[test_log::test]
fn a_secret_claude_code_writes_with_unicode_escapes_is_redacted() {
    assert_eq!(
        redacted(&format!("\"GH_TOKEN={IN_CLAUDE_CODE}\"")),
        "\"GH_TOKEN=[redacted]\""
    );
}

#[test_log::test]
fn a_secret_written_with_uppercase_unicode_escapes_is_redacted() {
    assert_eq!(
        redacted(&format!("\"GH_TOKEN={IN_CLAUDE_CODE_UPPERCASE}\"")),
        "\"GH_TOKEN=[redacted]\""
    );
}

/// JSON Claude Code wrote, inside a string of JSON it wrote, as when a
/// tool's result is its own stream-json.
#[test_log::test]
fn a_secret_claude_code_escapes_twice_is_redacted() {
    for written in [
        "tok\\\\u200ben\\\\u007f-\\\\u0085\\\\\\\"9\\\\udb40\\\\udc41",
        "tok\\\\u200Ben\\\\u007F-\\\\u0085\\\\\\\"9\\\\uDB40\\\\uDC41",
    ] {
        assert_eq!(
            redacted(&format!("\"{{\\\"token\\\":\\\"{written}\\\"}}\"")),
            "\"{\\\"token\\\":\\\"[redacted]\\\"}\"",
            "{written}"
        );
    }
}

/// JSON another tool wrote with `serde_json`, leaving the format characters
/// as they are, inside a string of JSON Claude Code wrote.
#[test_log::test]
fn a_secret_another_tool_escaped_inside_claude_code_s_json_is_redacted() {
    assert_eq!(
        redacted("\"{\\\"token\\\":\\\"tok\\u200ben\\u007f-\\u0085\\\\\\\"9\\udb40\\udc41\\\"}\""),
        "\"{\\\"token\\\":\\\"[redacted]\\\"}\""
    );
}

#[test_log::test]
fn a_redacted_stream_json_line_still_parses() {
    let line = format!(
        "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"GH_TOKEN={IN_CLAUDE_CODE} and {IN_CLAUDE_CODE_UPPERCASE}\"}}]}}}}"
    );

    let parsed: serde_json::Value = serde_json::from_str(&redacted(&line)).unwrap();

    assert_eq!(
        parsed["message"]["content"][0]["text"],
        "GH_TOKEN=[redacted] and [redacted]"
    );
}

#[test_log::test]
fn a_secret_of_ascii_is_redacted_as_it_is_and_escaped_once_and_twice() {
    let redaction = redaction_of(&[("GH_TOKEN", "ghp_\"s3cr3t")]);
    assert_eq!(
        redaction.apply("ghp_\"s3cr3t ghp_\\\"s3cr3t ghp_\\\\\\\"s3cr3t ghp_s3cr3t"),
        "[redacted] [redacted] [redacted] ghp_s3cr3t"
    );
}
