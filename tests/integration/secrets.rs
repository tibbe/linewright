//! The secrets Linewright finds in its environment with no `secrets`
//! setting: the credentials the table of sessions' variables passes,
//! "Credentials passed" in docs/specs/operation.md.

use crate::common;

use common::*;
use linewright::git::Addition;

/// What the secrets of an environment holding `variables` leave of `text`.
fn redacted(variables: &[(&str, &str)], text: &str) -> String {
    secrets_unnamed(variables).redaction().apply(text)
}

/// The variable `secrets_unnamed(variables)` says a commit adding `text`
/// holds the secret of, if any.
fn held_in(variables: &[(&str, &str)], text: &str) -> Option<String> {
    let addition = Addition {
        commit: "abc1234".to_string(),
        file: "notes.txt".to_string(),
        text: text.to_string(),
    };
    secrets_unnamed(variables)
        .held(&[addition])
        .map(|held| held.variable)
}

#[test]
fn the_harness_s_credentials_are_secrets_without_being_named() {
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
        let env = [(name, "harness-s3cr3t")];
        assert_eq!(
            redacted(&env, "token: harness-s3cr3t"),
            "token: [redacted]",
            "{name}"
        );
        assert_eq!(held_in(&env, "harness-s3cr3t").as_deref(), Some(name));
    }
}

#[test]
fn the_harness_s_other_variables_are_not_secrets() {
    let env = [
        ("ANTHROPIC_BASE_URL", "https://gateway.example.com"),
        ("CLAUDE_CODE_CLIENT_KEY", "/etc/claude/client-key.pem"),
        ("CLAUDE_CODE_USE_BEDROCK", "1"),
    ];
    let text = "https://gateway.example.com /etc/claude/client-key.pem";
    assert_eq!(redacted(&env, text), text);
}

#[test]
fn a_credential_only_session_env_passes_is_not_a_secret_unless_named() {
    let env = [("AWS_BEARER_TOKEN_BEDROCK", "bedrock-s3cr3t")];
    assert_eq!(redacted(&env, "bedrock-s3cr3t"), "bedrock-s3cr3t");
    assert_eq!(held_in(&env, "bedrock-s3cr3t"), None);
    assert_eq!(
        secrets_of(&env).redaction().apply("bedrock-s3cr3t"),
        "[redacted]"
    );
}

#[test]
fn a_proxy_password_is_a_secret_as_written_and_percent_decoded() {
    let env = [("HTTPS_PROXY", "http://bot:p%40ss-w0rd@proxy:3128")];
    assert_eq!(
        redacted(&env, "http://bot:p%40ss-w0rd@proxy:3128 and p@ss-w0rd"),
        "http://bot:[redacted]@proxy:3128 and [redacted]"
    );
    assert_eq!(held_in(&env, "p%40ss-w0rd").as_deref(), Some("HTTPS_PROXY"));
    assert_eq!(held_in(&env, "p@ss-w0rd").as_deref(), Some("HTTPS_PROXY"));
    assert_eq!(held_in(&env, "bot proxy:3128"), None);
}

#[test]
fn a_proxy_password_is_found_in_every_form_the_url_takes() {
    for (name, url, password) in [
        ("HTTP_PROXY", "bot:hunter2-long@proxy:3128", "hunter2-long"),
        (
            "https_proxy",
            "http://bot:hunter2-long@proxy:3128",
            "hunter2-long",
        ),
        (
            "http_proxy",
            "http://bot:hunter2-long@proxy:3128/",
            "hunter2-long",
        ),
        (
            "ALL_PROXY",
            "socks5h://bot:hunter2-long@proxy:1080",
            "hunter2-long",
        ),
        (
            "all_proxy",
            "socks5://bot:hunter2-long@proxy:1080",
            "hunter2-long",
        ),
        ("HTTPS_PROXY", "http://bot:hun@ter2@proxy:3128", "hun@ter2"),
        (
            "HTTPS_PROXY",
            "http://bot:pass:word@proxy:3128",
            "pass:word",
        ),
    ] {
        let env = [(name, url)];
        assert_eq!(
            redacted(&env, &format!("<{password}>")),
            "<[redacted]>",
            "{name}={url}"
        );
        assert_eq!(held_in(&env, password).as_deref(), Some(name), "{url}");
        assert_eq!(held_in(&env, "bot proxy"), None, "{url}");
    }
}

#[test]
fn a_proxy_url_without_a_password_gives_no_secret() {
    for url in [
        "http://proxy:3128",
        "http://bot@proxy:3128",
        "http://bot:@proxy:3128",
        "proxy:3128",
        "http://proxy:3128/path@thing",
        "",
    ] {
        let secrets = secrets_unnamed(&[("HTTPS_PROXY", url)]);
        assert_eq!(secrets.short(), [] as [String; 0], "{url}");
        assert_eq!(
            secrets
                .redaction()
                .apply("http://bot:x@proxy:3128/path@thing"),
            "http://bot:x@proxy:3128/path@thing",
            "{url}"
        );
    }
}

#[test]
fn no_proxy_gives_no_secret() {
    let env = [
        ("NO_PROXY", "bot:hunter2-long@internal.example.com"),
        ("no_proxy", "bot:hunter2-long@internal.example.com"),
    ];
    assert_eq!(redacted(&env, "hunter2-long"), "hunter2-long");
}

#[test]
fn a_percent_decoded_proxy_password_that_is_not_utf_8_is_skipped() {
    let env = [("HTTPS_PROXY", "http://bot:s3cr3t%FF@proxy:3128")];
    assert_eq!(redacted(&env, "s3cr3t%FF"), "[redacted]");
    assert_eq!(redacted(&env, "s3cr3t\u{fffd}"), "s3cr3t\u{fffd}");
}

#[test]
fn a_proxy_password_of_1_to_5_characters_is_set_aside_by_its_variable() {
    let env = [
        ("HTTPS_PROXY", "http://bot:p%40s@proxy:3128"),
        ("http_proxy", "http://bot:hunter2-long@proxy:3128"),
    ];
    let secrets = secrets_unnamed(&env);
    assert_eq!(secrets.short(), ["HTTPS_PROXY"]);
    assert_eq!(secrets.redaction().apply("p@s p%40s"), "p@s p%40s");
}
