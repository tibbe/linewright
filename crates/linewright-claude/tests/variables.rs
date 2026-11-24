//! Claude Code's variables that Linewright's settings answer to
//! (`docs/specs/sessions.md`, Environment variables), by the names Claude
//! Code reads: <https://code.claude.com/docs/en/env-vars>.

use linewright_claude::PROGRAM;
use linewright_core::secrets::Secrets;
use linewright_core::session_env::SetEnv;
use linewright_core::variable_names::VariableNames;

/// The secrets of an environment holding `name` as `value`, with no
/// `secrets` setting.
fn secrets_of(name: &str, value: &str) -> Secrets {
    Secrets::of(
        &VariableNames::default(),
        [(name.into(), value.into())],
        &PROGRAM.variables,
    )
    .unwrap()
}

#[test_log::test]
fn claude_codes_credentials_are_secrets_without_being_named() {
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_AWS_API_KEY",
        "ANTHROPIC_FOUNDRY_API_KEY",
        "ANTHROPIC_FOUNDRY_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
        "CLAUDE_CODE_CLIENT_KEY_PASSPHRASE",
    ] {
        let secrets = secrets_of(name, "harness-s3cr3t");

        assert_eq!(
            secrets.redaction().apply("token: harness-s3cr3t"),
            "token: [redacted]",
            "{name}"
        );
        assert_eq!(secrets.held_in(b"harness-s3cr3t"), Some(name));
    }
}

#[test_log::test]
fn a_value_of_claude_codes_custom_headers_is_a_secret_whole_and_without_its_scheme() {
    let secrets = secrets_of(
        "ANTHROPIC_CUSTOM_HEADERS",
        "x-litellm-api-key: Bearer sk-abc123",
    );

    assert_eq!(
        secrets.redaction().apply("<Bearer sk-abc123> <sk-abc123>"),
        "<[redacted]> <[redacted]>"
    );
}

#[test_log::test]
fn session_env_setting_a_setting_of_claude_codes_that_linewright_sets_is_an_error() {
    for name in [
        "CLAUDE_CODE_SHELL",
        "DISABLE_UPDATES",
        "CLAUDE_CODE_EFFORT_LEVEL",
        "CLAUDE_CODE_STARTUP_FAILURE_RESULTS",
    ] {
        let error =
            SetEnv::new([(name.to_string(), "x".to_string())], &PROGRAM.variables).unwrap_err();

        assert!(error.contains(&format!("`{name}`")), "{error}");
        assert!(error.contains("Linewright sets"), "{error}");
    }
}
