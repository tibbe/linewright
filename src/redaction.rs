//! Keeping Linewright's credentials out of what it keeps, logs or posts
//! (`docs/specs/architecture.md`, the redaction decision).

use aho_corasick::{AhoCorasick, MatchKind};

use crate::credentials::Credential;

pub const REDACTED: &str = "[redacted]";

/// Replaces the credentials' values with [`REDACTED`]. The default redacts
/// nothing.
#[derive(Clone, Default)]
pub struct Redaction {
    /// `None` when no credential is set, as a matcher needs a pattern.
    matcher: Option<AhoCorasick>,
}

impl Redaction {
    pub fn of(credentials: &[Credential]) -> Redaction {
        let mut patterns = Vec::new();
        for credential in credentials {
            let value = credential.value().to_string();
            // The harness's events and transcript write text in JSON strings,
            // and a session's result is often JSON itself, so a credential
            // can be escaped twice.
            let once = json_escaped(&value);
            let twice = json_escaped(&once);
            patterns.extend([value, once, twice]);
        }
        patterns.dedup();
        if patterns.is_empty() {
            return Redaction::default();
        }
        // Leftmost-longest, so a credential that holds another is redacted
        // whole.
        let matcher = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(patterns)
            .expect("the credentials make a matcher");
        Redaction {
            matcher: Some(matcher),
        }
    }

    pub fn apply(&self, text: &str) -> String {
        match &self.matcher {
            Some(matcher) => matcher.replace_all(text, &vec![REDACTED; matcher.patterns_len()]),
            None => text.to_string(),
        }
    }
}

/// `text` as it is written inside a JSON string.
fn json_escaped(text: &str) -> String {
    let quoted = serde_json::to_string(text).expect("a string serializes");
    quoted[1..quoted.len() - 1].to_string()
}

impl std::fmt::Debug for Redaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Redaction(..)")
    }
}
