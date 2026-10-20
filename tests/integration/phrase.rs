use chrono::{TimeZone, Utc};

use linewright::domain::Number;
use linewright::phrase::Phrase;

#[test]
fn a_phrase_reads_as_the_commands_word_items_times_durations_and_labels() {
    let at = Utc.with_ymd_and_hms(2026, 3, 4, 5, 6, 7).unwrap();
    let phrase = Phrase::from("waiting for ")
        .item(Number(42))
        .text(", labeled ")
        .code("needs-info")
        .text(" since ")
        .time(at)
        .text(", for ")
        .duration(3_903, Some(at))
        .text(" and ")
        .duration(0, None);
    assert_eq!(
        phrase.to_string(),
        "waiting for #42, labeled `needs-info` since 2026-03-04 05:06:07 UTC, for 1h 5m 3s and 0s"
    );
}

#[test]
fn phrases_join_with_a_separator() {
    let joined = Phrase::join([Number(1), Number(2), Number(3)].map(Phrase::from), ", ");
    assert_eq!(joined.to_string(), "#1, #2, #3");
    assert_eq!(Phrase::join([], "; ").to_string(), "");
}

#[test]
fn a_list_puts_its_conjunction_before_its_last_phrase() {
    let list = |numbers: &[u64], conjunction| {
        Phrase::list(
            numbers.iter().map(|&n| Phrase::from(Number(n))),
            conjunction,
        )
        .to_string()
    };
    assert_eq!(list(&[], "and"), "");
    assert_eq!(list(&[1], "and"), "#1");
    assert_eq!(list(&[1, 2], "or"), "#1 or #2");
    assert_eq!(list(&[1, 2, 3], "and"), "#1, #2 and #3");
}
