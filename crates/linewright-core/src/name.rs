//! What a person calls a benchmark, a task or a configuration
//! (`docs/specs/benchmarks.md`, Names and IDs).

use std::fmt;

use serde::Serialize;

mod freeform;

/// One line of the characters that the `FreeformClass` of PRECIS (RFC 8264)
/// allows, with no run of spaces in it, no space at its start or its end,
/// and something in it. Two names are the same only when they hold the same
/// characters, letter case included.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, schemars::JsonSchema)]
pub struct Name(String);

/// A name that holds nothing once it is clean.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("it holds nothing once it is clean, which is no name")]
pub struct NoName;

impl Name {
    /// `text`, clean: each white-space character that a name does not hold
    /// turned into a space, each other character that a name does not hold
    /// removed, each run of spaces turned into one space, and the space at
    /// each end removed. Cleaning a name gives it back as it is.
    pub fn parse(text: &str) -> Result<Name, NoName> {
        let spaced = text
            .chars()
            .map(|c| {
                if freeform::is_disallowed_white_space(c) {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        let mut name = String::with_capacity(text.len());
        for c in freeform::allowed(spaced) {
            if !freeform::is_space(c) {
                name.push(c);
            } else if !name.is_empty() && !name.ends_with(' ') {
                name.push(' ');
            }
        }
        if name.ends_with(' ') {
            name.pop();
        }
        if name.is_empty() {
            return Err(NoName);
        }
        Ok(Name(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for Name {
    type Err = NoName;

    fn from_str(text: &str) -> Result<Name, NoName> {
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

/// A name as it serializes, clean ([`Name::parse`]).
impl<'de> serde::Deserialize<'de> for Name {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Name, D::Error> {
        let text = String::deserialize(deserializer)?;
        Name::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Sorts `items` by the names `key` gives, in the order of the Unicode
/// Collation Algorithm's root collation, and by their characters where it
/// finds two names equal.
pub fn collated<T>(items: &mut [T], key: impl Fn(&T) -> &Name) {
    collated_by(items, |item| key(item).as_str());
}

/// Sorts `items` by the text `key` gives, as [`collated`] sorts by names.
pub fn collated_by<T>(items: &mut [T], key: impl Fn(&T) -> &str) {
    let collator = icu_collator::Collator::try_new(
        icu_collator::CollatorPreferences::default(),
        icu_collator::options::CollatorOptions::default(),
    )
    .expect("the root collation is compiled in");
    items.sort_by(|a, b| {
        let (a, b) = (key(a), key(b));
        collator.compare(a, b).then_with(|| a.cmp(b))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> Name {
        Name::parse(text).unwrap()
    }

    #[test_log::test]
    fn a_clean_name_is_kept_as_it_is() {
        for text in [
            "Design an HTTP caching solution for images",
            "gadgets",
            "Ünïcödé, punctuation: & <script>!",
            "7",
            "Crab 🦀 and melting face 🫠",
            "日本語の名前",
            "col·lecció",
            "ﬁne",
        ] {
            assert_eq!(name(text).as_str(), text);
        }
    }

    #[test_log::test]
    fn the_white_space_at_either_end_is_removed() {
        assert_eq!(name("  Design\t").as_str(), "Design");
        assert_eq!(name(" Design "), name("Design"));
        assert_eq!(name("\u{a0}Two words\u{2003}").as_str(), "Two words");
    }

    #[test_log::test]
    fn each_line_break_tab_and_run_of_spaces_inside_becomes_one_space() {
        for (text, clean) in [
            ("two\nlines", "two lines"),
            ("two\r\nlines", "two lines"),
            ("two\u{2028}lines", "two lines"),
            ("two\u{2029}lines", "two lines"),
            ("two\u{85}lines", "two lines"),
            ("a\rb", "a b"),
            ("tab\tbed", "tab bed"),
            ("Two  words", "Two words"),
            ("Two \u{a0}\u{3000} words", "Two words"),
            ("a \u{b}\u{c} b", "a b"),
        ] {
            assert_eq!(name(text).as_str(), clean, "{text:?}");
        }
    }

    #[test_log::test]
    fn each_other_character_a_name_does_not_hold_is_removed() {
        for (text, clean) in [
            ("bell\u{7}", "bell"),
            ("es\u{1b}cape", "escape"),
            ("zero\u{200b}width", "zerowidth"),
            ("soft\u{ad}hyphen", "softhyphen"),
            ("private\u{e000}use", "privateuse"),
            ("un\u{378}assigned", "unassigned"),
            ("non\u{fffe}character", "noncharacter"),
            ("old\u{1100}jamo", "oldjamo"),
            ("tat\u{640}weel", "tatweel"),
            ("a·b", "ab"),
            ("a\u{375}b", "ab"),
            ("\u{5f3}\u{5f3}x", "x"),
            ("x\u{200d}y", "xy"),
        ] {
            assert_eq!(name(text).as_str(), clean, "{text:?}");
        }
    }

    #[test_log::test]
    fn a_character_kept_by_its_context_is_kept() {
        for text in [
            "l·l",
            "\u{375}α",
            "א\u{5f3}",
            "ア\u{30fb}イ",
            "\u{915}\u{94d}\u{200d}\u{937}",
            "\u{915}\u{94d}\u{200c}\u{937}",
            "\u{628}\u{200c}\u{628}",
            "\u{663}\u{664}",
        ] {
            assert_eq!(name(text).as_str(), text, "{text:?}");
        }
    }

    #[test_log::test]
    fn a_name_that_holds_nothing_once_clean_is_no_name() {
        for text in [
            "",
            " ",
            "\t\n ",
            "\u{a0}\u{2003}",
            "\u{200b}",
            "\u{7}\u{ad}\u{e000}",
            "\u{663}\u{6f3}",
        ] {
            assert_eq!(Name::parse(text), Err(NoName), "{text:?}");
        }
    }

    proptest::proptest! {
        #[test_log::test]
        fn cleaning_a_clean_name_leaves_it_as_it_is(text in "\\PC*|.*|[\\s\\p{Cf}\\p{Cc}a·\u{5f3}\u{375}\u{200c}\u{200d}\u{94d}\u{915}α\u{663}\u{6f3}]*") {
            if let Ok(clean) = Name::parse(&text) {
                proptest::prop_assert_eq!(Name::parse(clean.as_str()), Ok(clean));
            }
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
