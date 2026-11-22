//! What a person calls a benchmark, a task or a configuration
//! (`docs/specs/benchmarks.md`, Names and IDs).

use std::fmt;

use serde::Serialize;

/// One line of any text, with no white space at its start or its end and
/// something other than white space in it. Two names are the same only when
/// they hold the same characters, letter case included.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(String);

/// Why `text` is no name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NameRefusal {
    #[error("it holds only white space, which is no name")]
    Blank,
    #[error("it holds a line break, and a name is one line")]
    LineBreak,
}

impl Name {
    /// `text` without the white space at its start and its end, refused when
    /// that leaves nothing or holds a line break.
    pub fn parse(text: &str) -> Result<Name, NameRefusal> {
        let name = text.trim();
        if name.is_empty() {
            return Err(NameRefusal::Blank);
        }
        if name.chars().any(breaks_a_line) {
            return Err(NameRefusal::LineBreak);
        }
        Ok(Name(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether `c` ends a line, as Unicode's mandatory breaks do: the line
/// breaking classes BK, CR, LF and NL
/// (<https://www.unicode.org/reports/tr14/#Table1>).
fn breaks_a_line(c: char) -> bool {
    matches!(
        c,
        '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

impl std::str::FromStr for Name {
    type Err = NameRefusal;

    fn from_str(text: &str) -> Result<Name, NameRefusal> {
        Name::parse(text)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Name {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// Sorts `items` by the names `key` gives, in the order of the Unicode
/// Collation Algorithm's root collation, and by their characters where it
/// finds two names equal.
pub fn collated<T>(items: &mut [T], key: impl Fn(&T) -> &Name) {
    let collator = icu_collator::Collator::try_new(
        icu_collator::CollatorPreferences::default(),
        icu_collator::options::CollatorOptions::default(),
    )
    .expect("the root collation is compiled in");
    items.sort_by(|a, b| {
        let (a, b) = (key(a).as_str(), key(b).as_str());
        collator.compare(a, b).then_with(|| a.cmp(b))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn name(text: &str) -> Name {
        Name::parse(text).unwrap()
    }

    #[test_log::test]
    fn a_name_is_one_line_of_any_text() {
        for text in [
            "Design an HTTP caching solution for images",
            "gadgets",
            "Ünïcödé, punctuation: & <script>!",
            "7",
        ] {
            assert_eq!(name(text).as_str(), text);
        }
    }

    #[test_log::test]
    fn the_white_space_at_either_end_is_removed() {
        assert_eq!(name("  Design\t").as_str(), "Design");
        assert_eq!(name(" Design "), name("Design"));
        assert_eq!(name("\u{a0}Two  words\u{2003}").as_str(), "Two  words");
    }

    #[test_log::test]
    fn a_blank_name_is_refused() {
        for text in ["", " ", "\t\n ", "\u{a0}\u{2003}"] {
            assert_eq!(Name::parse(text), Err(NameRefusal::Blank), "{text:?}");
        }
    }

    #[test_log::test]
    fn a_name_with_a_line_break_is_refused() {
        for text in ["two\nlines", "two\r\nlines", "two\u{2028}lines", "a\rb"] {
            assert_eq!(Name::parse(text), Err(NameRefusal::LineBreak), "{text:?}");
        }
    }

    #[test_log::test]
    fn names_differing_in_letter_case_are_two_names() {
        assert_ne!(name("Design"), name("design"));
    }

    #[test_log::test]
    fn names_are_collated_by_the_unicode_collation_algorithm() {
        let mut names = vec![name("Éclair"), name("Banana"), name("apple")];

        collated(&mut names, |n| n);

        assert_eq!(
            names.iter().map(Name::as_str).collect::<Vec<_>>(),
            ["apple", "Banana", "Éclair"]
        );
    }
}
