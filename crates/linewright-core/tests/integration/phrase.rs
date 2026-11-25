use chrono::{TimeZone, Utc};
use linewright_core::domain::{ItemKind, Number};
use linewright_core::phrase::{Phrase, Segment};

#[test_log::test]
fn a_phrase_reads_as_the_commands_word_items_times_durations_and_labels() {
    let at = Utc.with_ymd_and_hms(2026, 3, 4, 5, 6, 7).unwrap();
    let phrase = Phrase::from("waiting for ")
        .issue(Number(42))
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
        "waiting for #42, labeled `needs-info` since 2026-03-04 05:06:07 UTC, for 1h 5m and 0s"
    );
}

#[test_log::test]
fn an_item_is_an_issue_or_a_pull_request_and_reads_as_its_number_alike() {
    let phrase = Phrase::default()
        .issue(Number(7))
        .text(" closed in ")
        .pull_request(Number(8));
    assert_eq!(
        phrase.segments(),
        [
            Segment::Item {
                number: Number(7),
                kind: ItemKind::Issue
            },
            Segment::Text(" closed in ".to_string()),
            Segment::Item {
                number: Number(8),
                kind: ItemKind::PullRequest
            },
        ]
    );
    assert_eq!(phrase.to_string(), "#7 closed in #8");
}

/// `docs/specs/ui_patterns.md`, Time, Durations.
#[test_log::test]
fn a_duration_reads_as_its_two_largest_units_that_are_not_zero() {
    let shown = [45, 103, 10_929, 86_700]
        .map(|seconds| Phrase::default().duration(seconds, None).to_string());
    assert_eq!(shown, ["45s", "1m 43s", "3h 2m", "1d 5m"]);
}

#[test_log::test]
fn phrases_join_with_a_separator() {
    let joined = Phrase::join([1, 2, 3].map(|n| Phrase::default().issue(Number(n))), ", ");
    assert_eq!(joined.to_string(), "#1, #2, #3");
    assert_eq!(Phrase::join([], "; ").to_string(), "");
}

#[test_log::test]
fn a_list_puts_its_conjunction_before_its_last_phrase() {
    let list = |numbers: &[u64], conjunction| {
        Phrase::list(
            numbers.iter().map(|&n| Phrase::default().issue(Number(n))),
            conjunction,
        )
        .to_string()
    };
    assert_eq!(list(&[], "and"), "");
    assert_eq!(list(&[1], "and"), "#1");
    assert_eq!(list(&[1, 2], "or"), "#1 or #2");
    assert_eq!(list(&[1, 2, 3], "and"), "#1, #2 and #3");
}
