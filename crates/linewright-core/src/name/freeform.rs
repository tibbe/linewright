//! The `FreeformClass` of PRECIS, derived from the Unicode properties of the
//! version ICU4X compiles in (RFC 8264, sections 8 and 9), with the context
//! rules of RFC 5892, Appendix A.
//! <https://www.rfc-editor.org/rfc/rfc8264#section-8>
//! <https://www.rfc-editor.org/rfc/rfc5892#appendix-A>

use std::ops::RangeInclusive;

use icu_normalizer::ComposingNormalizerBorrowed;
use icu_properties::props::{
    CanonicalCombiningClass, DefaultIgnorableCodePoint, GeneralCategory, HangulSyllableType,
    JoinControl, JoiningType, NoncharacterCodePoint, Script, WhiteSpace,
};
use icu_properties::{CodePointMapData, CodePointSetData};

/// ASCII7 (RFC 8264, section 9.11).
const ASCII7: RangeInclusive<char> = '\u{21}'..='\u{7e}';
const ARABIC_INDIC_DIGITS: RangeInclusive<char> = '\u{660}'..='\u{669}';
const EXTENDED_ARABIC_INDIC_DIGITS: RangeInclusive<char> = '\u{6f0}'..='\u{6f9}';

/// What the `FreeformClass` makes of a character on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Allowed,
    /// Allowed only where its rule in RFC 5892, Appendix A, holds.
    Contextual,
    Disallowed,
}

/// Whether `c` is white space that the `FreeformClass` disallows, such as a
/// tab or a line break.
pub(super) fn is_disallowed_white_space(c: char) -> bool {
    CodePointSetData::new::<WhiteSpace>().contains(c) && class(c) == Class::Disallowed
}

/// Whether `c` is a space: `General_Category` Zs.
pub(super) fn is_space(c: char) -> bool {
    CodePointMapData::<GeneralCategory>::new().get(c) == GeneralCategory::SpaceSeparator
}

/// `chars` without each character the `FreeformClass` disallows, nor each
/// contextual one whose rule fails, removed until every rule that is left
/// holds: a removal can make the rule of another fail.
pub(super) fn allowed(mut chars: Vec<char>) -> Vec<char> {
    chars.retain(|c| class(*c) != Class::Disallowed);
    let mut text = Text::new(chars);
    let mut candidates: Vec<usize> = (0..text.chars.len())
        .filter(|at| class(text.chars[*at]) == Class::Contextual)
        .collect();
    while !candidates.is_empty() {
        candidates.sort_unstable();
        candidates.dedup();
        let failing: Vec<usize> = candidates
            .into_iter()
            .filter(|at| text.live[*at] && !text.context_holds(*at))
            .collect();
        candidates = text.remove(&failing);
    }
    text.into_chars()
}

/// The derived property value of `c` (RFC 8264, section 8), with
/// `FREE_PVAL` and `PVALID` both allowed.
fn class(c: char) -> Class {
    use GeneralCategory as Gc;
    if let Some(class) = exception(c) {
        return class;
    }
    let category = CodePointMapData::<GeneralCategory>::new().get(c);
    let noncharacter = CodePointSetData::new::<NoncharacterCodePoint>().contains(c);
    if category == Gc::Unassigned && !noncharacter {
        return Class::Disallowed;
    }
    if ASCII7.contains(&c) {
        return Class::Allowed;
    }
    if CodePointSetData::new::<JoinControl>().contains(c) {
        return Class::Contextual;
    }
    let jamo = CodePointMapData::<HangulSyllableType>::new().get(c);
    if matches!(
        jamo,
        HangulSyllableType::LeadingJamo
            | HangulSyllableType::VowelJamo
            | HangulSyllableType::TrailingJamo
    ) {
        return Class::Disallowed;
    }
    if noncharacter
        || CodePointSetData::new::<DefaultIgnorableCodePoint>().contains(c)
        || category == Gc::Control
    {
        return Class::Disallowed;
    }
    if has_compat(c) {
        return Class::Allowed;
    }
    match category {
        Gc::LowercaseLetter
        | Gc::UppercaseLetter
        | Gc::OtherLetter
        | Gc::DecimalNumber
        | Gc::ModifierLetter
        | Gc::NonspacingMark
        | Gc::SpacingMark
        | Gc::TitlecaseLetter
        | Gc::LetterNumber
        | Gc::OtherNumber
        | Gc::EnclosingMark
        | Gc::SpaceSeparator
        | Gc::MathSymbol
        | Gc::CurrencySymbol
        | Gc::ModifierSymbol
        | Gc::OtherSymbol
        | Gc::ConnectorPunctuation
        | Gc::DashPunctuation
        | Gc::OpenPunctuation
        | Gc::ClosePunctuation
        | Gc::InitialPunctuation
        | Gc::FinalPunctuation
        | Gc::OtherPunctuation => Class::Allowed,
        Gc::Unassigned
        | Gc::LineSeparator
        | Gc::ParagraphSeparator
        | Gc::Control
        | Gc::Format
        | Gc::PrivateUse
        | Gc::Surrogate => Class::Disallowed,
    }
}

/// The Exceptions of RFC 5892, section 2.6.
/// <https://www.rfc-editor.org/rfc/rfc5892#section-2.6>
fn exception(c: char) -> Option<Class> {
    match c {
        '\u{df}' | '\u{3c2}' | '\u{6fd}' | '\u{6fe}' | '\u{f0b}' | '\u{3007}' => {
            Some(Class::Allowed)
        }
        '\u{b7}' | '\u{375}' | '\u{5f3}' | '\u{5f4}' | '\u{30fb}' => Some(Class::Contextual),
        c if ARABIC_INDIC_DIGITS.contains(&c) || EXTENDED_ARABIC_INDIC_DIGITS.contains(&c) => {
            Some(Class::Contextual)
        }
        '\u{640}' | '\u{7fa}' | '\u{302e}' | '\u{302f}' | '\u{3031}'..='\u{3035}' | '\u{303b}' => {
            Some(Class::Disallowed)
        }
        _ => None,
    }
}

/// `HasCompat`: whether NFKC changes `c` (RFC 8264, section 9.17).
fn has_compat(c: char) -> bool {
    let mut text = [0; 4];
    !ComposingNormalizerBorrowed::new_nfkc().is_normalized(c.encode_utf8(&mut text))
}

/// Which way from a character.
#[derive(Clone, Copy)]
enum Side {
    Before,
    After,
}

/// The characters of a name as their context rules see them, each linked to
/// the characters left on either side of it, so that removing one leaves
/// the rules of only its neighbours to check again.
struct Text {
    chars: Vec<char>,
    before: Vec<Option<usize>>,
    after: Vec<Option<usize>>,
    live: Vec<bool>,
    kana_or_han: bool,
    arabic_indic: usize,
    extended_arabic_indic: usize,
}

impl Text {
    fn new(chars: Vec<char>) -> Text {
        let script = CodePointMapData::<Script>::new();
        Text {
            before: (0..chars.len()).map(|at| at.checked_sub(1)).collect(),
            after: (0..chars.len())
                .map(|at| Some(at + 1).filter(|next| *next < chars.len()))
                .collect(),
            live: vec![true; chars.len()],
            kana_or_han: chars.iter().any(|c| {
                matches!(
                    script.get(*c),
                    Script::Hiragana | Script::Katakana | Script::Han
                )
            }),
            arabic_indic: chars
                .iter()
                .filter(|c| ARABIC_INDIC_DIGITS.contains(*c))
                .count(),
            extended_arabic_indic: chars
                .iter()
                .filter(|c| EXTENDED_ARABIC_INDIC_DIGITS.contains(*c))
                .count(),
            chars,
        }
    }

    fn into_chars(self) -> Vec<char> {
        self.chars
            .into_iter()
            .zip(self.live)
            .filter_map(|(c, live)| live.then_some(c))
            .collect()
    }

    fn next(&self, at: usize, side: Side) -> Option<usize> {
        match side {
            Side::Before => self.before[at],
            Side::After => self.after[at],
        }
    }

    /// Whether the rule of the contextual character at `at` holds (RFC 5892,
    /// Appendix A).
    fn context_holds(&self, at: usize) -> bool {
        let before = self.before[at].map(|b| self.chars[b]);
        let after = self.after[at].map(|a| self.chars[a]);
        let script = |c: Option<char>| c.map(|c| CodePointMapData::<Script>::new().get(c));
        match self.chars[at] {
            '\u{200c}' => after_virama(before) || self.joins_both_sides(at),
            '\u{200d}' => after_virama(before),
            '\u{b7}' => before == Some('l') && after == Some('l'),
            '\u{375}' => script(after) == Some(Script::Greek),
            '\u{5f3}' | '\u{5f4}' => script(before) == Some(Script::Hebrew),
            '\u{30fb}' => self.kana_or_han,
            c if ARABIC_INDIC_DIGITS.contains(&c) => self.extended_arabic_indic == 0,
            c if EXTENDED_ARABIC_INDIC_DIGITS.contains(&c) => self.arabic_indic == 0,
            _ => false,
        }
    }

    /// Whether ZERO WIDTH NON-JOINER at `at` sits between a character that
    /// joins to its left and one that joins to its right, past transparent
    /// ones.
    fn joins_both_sides(&self, at: usize) -> bool {
        let joining = |side| {
            self.next(at, side)
                .and_then(|next| self.past_transparent(next, side))
                .map(|beyond| CodePointMapData::<JoiningType>::new().get(self.chars[beyond]))
        };
        matches!(
            joining(Side::Before),
            Some(JoiningType::LeftJoining | JoiningType::DualJoining)
        ) && matches!(
            joining(Side::After),
            Some(JoiningType::RightJoining | JoiningType::DualJoining)
        )
    }

    /// The first character from `at` towards `side`, `at` included, that is
    /// not transparent.
    fn past_transparent(&self, mut at: usize, side: Side) -> Option<usize> {
        let joining = CodePointMapData::<JoiningType>::new();
        while joining.get(self.chars[at]) == JoiningType::Transparent {
            at = self.next(at, side)?;
        }
        Some(at)
    }

    /// Removes the characters at `failing`, and gives the contextual
    /// characters whose rules the removals may have made fail.
    fn remove(&mut self, failing: &[usize]) -> Vec<usize> {
        let mut neighbours = vec![];
        let mut digits_removed = false;
        for &at in failing {
            let (before, after) = (self.before[at], self.after[at]);
            if let Some(b) = before {
                self.after[b] = after;
            }
            if let Some(a) = after {
                self.before[a] = before;
            }
            self.live[at] = false;
            let c = self.chars[at];
            if ARABIC_INDIC_DIGITS.contains(&c) {
                self.arabic_indic -= 1;
                digits_removed = true;
            }
            if EXTENDED_ARABIC_INDIC_DIGITS.contains(&c) {
                self.extended_arabic_indic -= 1;
                digits_removed = true;
            }
            neighbours.extend(before.map(|b| (b, Side::Before)));
            neighbours.extend(after.map(|a| (a, Side::After)));
        }
        let mut affected = vec![];
        for (at, side) in neighbours {
            if self.live[at] {
                affected.push(at);
                affected.extend(self.past_transparent(at, side));
            }
        }
        if digits_removed {
            affected.extend((0..self.chars.len()).filter(|at| {
                self.live[*at]
                    && (ARABIC_INDIC_DIGITS.contains(&self.chars[*at])
                        || EXTENDED_ARABIC_INDIC_DIGITS.contains(&self.chars[*at]))
            }));
        }
        affected.retain(|at| class(self.chars[*at]) == Class::Contextual);
        affected
    }
}

fn after_virama(before: Option<char>) -> bool {
    before.is_some_and(|c| {
        CodePointMapData::<CanonicalCombiningClass>::new().get(c) == CanonicalCombiningClass::Virama
    })
}
