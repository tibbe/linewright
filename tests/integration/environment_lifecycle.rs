//! The environment's lifecycle and what triggers a build
//! (`docs/specs/sessions.md`, Building).

use crate::common;

use std::time::Duration;

use common::at;
use linewright::environment::lifecycle::{
    Basis, Effect, Event, Fingerprint, State, Trigger, on, polled, trigger, unqueued,
};
use rstest::rstest;

const BUILDING: State = State::Building { queued: false };
const REBUILDING: State = State::Rebuilding { queued: false };

/// The spec's table, row by row.
#[rstest]
#[case::serve_starts_with_none(State::None, Event::Started, BUILDING, &[Effect::StartBuild])]
#[case::a_trigger_with_none(State::None, Event::Triggered, BUILDING, &[Effect::StartBuild])]
#[case::a_first_build_succeeds(BUILDING, Event::Succeeded, State::Ready, &[Effect::Replace])]
#[case::a_first_build_fails(BUILDING, Event::Failed, State::None, &[])]
#[case::a_trigger_when_ready(State::Ready, Event::Triggered, REBUILDING, &[Effect::StartBuild])]
#[case::a_rebuild_succeeds(REBUILDING, Event::Succeeded, State::Ready, &[Effect::Replace])]
#[case::a_rebuild_fails(REBUILDING, Event::Failed, State::Ready, &[Effect::KeepLast])]
#[case::a_trigger_while_building(
    BUILDING,
    Event::Triggered,
    State::Building { queued: true },
    &[]
)]
#[case::a_trigger_while_rebuilding(
    REBUILDING,
    Event::Triggered,
    State::Rebuilding { queued: true },
    &[]
)]
#[case::serve_exits_while_building(BUILDING, Event::Exiting, State::None, &[Effect::Abandon])]
#[case::serve_exits_while_rebuilding(REBUILDING, Event::Exiting, State::Ready, &[Effect::Abandon])]
#[test_log::test]
fn each_row_of_the_table(
    #[case] state: State,
    #[case] event: Event,
    #[case] next: State,
    #[case] effects: &[Effect],
) {
    assert_eq!(on(state, event), (next, effects.to_vec()));
}

#[rstest]
#[case::a_first_build_succeeds(
    State::Building { queued: true },
    Event::Succeeded,
    REBUILDING,
    &[Effect::Replace, Effect::StartBuild]
)]
#[case::a_first_build_fails(
    State::Building { queued: true },
    Event::Failed,
    BUILDING,
    &[Effect::StartBuild]
)]
#[case::a_rebuild_fails(
    State::Rebuilding { queued: true },
    Event::Failed,
    REBUILDING,
    &[Effect::KeepLast, Effect::StartBuild]
)]
#[case::serve_exits(State::Rebuilding { queued: true }, Event::Exiting, State::Ready, &[Effect::Abandon])]
#[test_log::test]
fn the_build_a_trigger_queued_starts_once_the_one_in_progress_ends(
    #[case] state: State,
    #[case] event: Event,
    #[case] next: State,
    #[case] effects: &[Effect],
) {
    assert_eq!(on(state, event), (next, effects.to_vec()));
}

#[test_log::test]
fn held_builds_drop_the_build_a_trigger_queued() {
    assert_eq!(unqueued(State::Building { queued: true }), BUILDING);
    assert_eq!(unqueued(State::Rebuilding { queued: true }), REBUILDING);
    assert_eq!(unqueued(State::Ready), State::Ready);
}

#[rstest]
#[case::serve_starting_when_ready(State::Ready, Event::Started)]
#[case::serve_starting_while_building(BUILDING, Event::Started)]
#[case::serve_exiting_when_ready(State::Ready, Event::Exiting)]
#[case::serve_exiting_with_none(State::None, Event::Exiting)]
#[test_log::test]
fn an_event_the_table_has_no_row_for_changes_nothing(#[case] state: State, #[case] event: Event) {
    assert_eq!(on(state, event), (state, vec![]));
}

#[track_caller]
fn fingerprint(command: &str) -> Fingerprint {
    Fingerprint::of(command, [("HOME", "/home/lw")])
}

#[track_caller]
fn basis(command: &str, time: &str) -> Basis {
    Basis {
        fingerprint: fingerprint(command),
        since: at(time),
    }
}

const DAY: Duration = Duration::from_hours(24);

#[test_log::test]
fn the_environment_as_it_was_built_triggers_nothing() {
    let built = basis("npm ci", "2026-10-01T12:00:00Z");

    assert_eq!(
        trigger(
            Some(&built),
            None,
            &fingerprint("npm ci"),
            DAY,
            at("2026-10-02T12:00:00Z")
        ),
        None
    );
}

#[test_log::test]
fn a_changed_setup_command_triggers_a_build() {
    let built = basis("npm ci", "2026-10-01T12:00:00Z");

    assert_eq!(
        trigger(
            Some(&built),
            None,
            &fingerprint("pnpm install"),
            DAY,
            at("2026-10-01T12:05:00Z")
        ),
        Some(Trigger::Changed)
    );
}

#[test_log::test]
fn a_changed_variable_triggers_a_build() {
    let built = basis("npm ci", "2026-10-01T12:00:00Z");
    let changed = Fingerprint::of("npm ci", [("HOME", "/home/other")]);

    assert_eq!(
        trigger(
            Some(&built),
            None,
            &changed,
            DAY,
            at("2026-10-01T12:05:00Z")
        ),
        Some(Trigger::Changed)
    );
}

#[test_log::test]
fn an_environment_older_than_max_age_triggers_a_build() {
    let built = basis("npm ci", "2026-10-01T12:00:00Z");

    assert_eq!(
        trigger(
            Some(&built),
            None,
            &fingerprint("npm ci"),
            DAY,
            at("2026-10-02T12:00:01Z")
        ),
        Some(Trigger::Aged)
    );
}

/// A build that failed is not tried again for the same command and
/// variables until they change, or `max-age` passes again.
#[test_log::test]
fn the_latest_build_tried_is_what_a_change_or_the_age_is_measured_from() {
    let built = basis("npm ci", "2026-10-01T12:00:00Z");
    let tried = basis("pnpm install", "2026-10-02T13:00:00Z");

    let deciding = |command: &str, time: &str| {
        trigger(
            Some(&built),
            Some(&tried),
            &fingerprint(command),
            DAY,
            at(time),
        )
    };

    assert_eq!(deciding("pnpm install", "2026-10-02T14:00:00Z"), None);
    assert_eq!(
        deciding("npm ci", "2026-10-02T14:00:00Z"),
        Some(Trigger::Changed)
    );
    assert_eq!(
        deciding("pnpm install", "2026-10-03T13:00:01Z"),
        Some(Trigger::Aged)
    );
}

#[test_log::test]
fn without_an_environment_only_a_change_from_the_build_tried_triggers_one() {
    let tried = basis("npm ci", "2026-10-01T12:00:00Z");

    assert_eq!(
        trigger(
            None,
            Some(&tried),
            &fingerprint("npm ci"),
            DAY,
            at("2026-10-05T12:00:00Z")
        ),
        None
    );
    assert_eq!(
        trigger(
            None,
            Some(&tried),
            &fingerprint("npm ci --force"),
            DAY,
            at("2026-10-01T12:01:00Z")
        ),
        Some(Trigger::Changed)
    );
}

#[test_log::test]
fn the_fingerprint_keeps_no_value_and_depends_on_neither_order_nor_tmpdir() {
    let secret = "sk-registry-0123456789";
    let one = Fingerprint::of(
        "npm ci",
        [("TOKEN", secret), ("HOME", "/h"), ("TMPDIR", "/a")],
    );
    let other = Fingerprint::of(
        "npm ci",
        [("HOME", "/h"), ("TMPDIR", "/b"), ("TOKEN", secret)],
    );

    assert_eq!(one, other);
    assert!(!serde_json::to_string(&one).unwrap().contains(secret));
    assert_ne!(one, Fingerprint::of("npm ci", [("HOME", "/h")]));
}

#[test_log::test]
fn serve_starting_with_no_environment_and_no_build_tried_starts_one() {
    let deciding = |state, tried: Option<&Basis>| {
        polled(
            state,
            None,
            tried,
            &fingerprint("npm ci"),
            DAY,
            at("2026-10-01T12:05:00Z"),
        )
    };
    let tried = basis("npm ci", "2026-10-01T12:00:00Z");

    assert_eq!(deciding(State::None, None), Some(Event::Started));
    assert_eq!(deciding(State::None, Some(&tried)), None);
    assert_eq!(deciding(BUILDING, Some(&tried)), None);
}

#[test_log::test]
fn a_poll_with_an_environment_brings_its_trigger() {
    let built = basis("npm ci", "2026-10-01T12:00:00Z");

    assert_eq!(
        polled(
            State::Ready,
            Some(&built),
            None,
            &fingerprint("pnpm install"),
            DAY,
            at("2026-10-01T12:05:00Z")
        ),
        Some(Event::Triggered)
    );
    assert_eq!(
        polled(
            State::Ready,
            Some(&built),
            None,
            &fingerprint("npm ci"),
            DAY,
            at("2026-10-01T12:05:00Z")
        ),
        None
    );
}
