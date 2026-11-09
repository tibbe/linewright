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
    /// JSON string once or twice, by `serde_json` or by Claude Code. A secret
    /// that holds another is redacted whole.
    pub fn of(secrets: &[Secret]) -> Redaction {
        let mut patterns = Vec::new();
        for secret in secrets {
            let value = secret.value().to_string();
            let in_json = json_escaped(&value);
            let mut forms = vec![json_escaped(&in_json)];
            for case in [Hex::Lower, Hex::Upper] {
                let in_claude = claude_escaped(&value, case);
                forms.extend([
                    claude_escaped(&in_claude, case),
                    claude_escaped(&in_json, case),
                    json_escaped(&in_claude),
                    in_claude,
                ]);
            }
            forms.extend([in_json, value]);
            forms.sort_unstable();
            forms.dedup();
            patterns.extend(forms);
        }
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

/// The case of the hex digits in a `\uXXXX` escape.
#[derive(Clone, Copy)]
enum Hex {
    Lower,
    Upper,
}

/// `text` as Claude Code writes it inside a JSON string: as `serde_json`
/// does, but with DEL, the C1 controls, the line and paragraph separators
/// and the format characters as `\uXXXX`, outside the Basic
/// Multilingual Plane as a UTF-16 surrogate pair.
fn claude_escaped(text: &str, case: Hex) -> String {
    let mut escaped = String::new();
    for c in text.chars() {
        if !claude_escapes(c) {
            escaped.push_str(&json_escaped(c.encode_utf8(&mut [0; 4])));
            continue;
        }
        for unit in c.encode_utf16(&mut [0; 2]) {
            match case {
                Hex::Lower => escaped.push_str(&format!("\\u{unit:04x}")),
                Hex::Upper => escaped.push_str(&format!("\\u{unit:04X}")),
            }
        }
    }
    escaped
}

/// Whether Claude Code writes `c` as `\uXXXX` where `serde_json` writes it
/// as it is. Claude Code documents no such list; this is what 2.1.311
/// wrote: <https://github.com/tibbe/linewright/issues/1107>.
fn claude_escapes(c: char) -> bool {
    matches!(
        c,
        '\u{7f}'..='\u{9f}'
            | '\u{ad}'
            | '\u{600}'..='\u{605}'
            | '\u{61c}'
            | '\u{6dd}'
            | '\u{70f}'
            | '\u{890}'..='\u{891}'
            | '\u{8e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

impl std::fmt::Debug for Redaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Redaction(..)")
    }
}
