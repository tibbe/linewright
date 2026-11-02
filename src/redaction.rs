//! Keeping Linewright's secrets out of what it keeps, logs or posts
//! (`docs/specs/operation.md`, Secrets).

use aho_corasick::{AhoCorasick, MatchKind};

use crate::secrets::Secret;

pub const REDACTED: &str = "[redacted]";

/// Replaces the secrets' values with [`REDACTED`]. The default redacts
/// nothing.
#[derive(Clone, Default)]
pub struct Redaction {
    matcher: Option<AhoCorasick>,
}

impl Redaction {
    /// Redacts each secret's value as it is and as it is written inside a
    /// JSON string once or twice. A secret that holds another is redacted
    /// whole.
    pub fn of(secrets: &[Secret]) -> Redaction {
        let mut patterns = Vec::new();
        for secret in secrets {
            let value = secret.value().to_string();
            let in_json = json_escaped(&value);
            let in_json_in_json = json_escaped(&in_json);
            patterns.extend([value, in_json, in_json_in_json]);
        }
        patterns.dedup();
        if patterns.is_empty() {
            return Redaction::default();
        }
        let matcher = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(patterns)
            .expect("the secrets make a matcher");
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
