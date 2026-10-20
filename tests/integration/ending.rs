use crate::common;

use common::at;
use linewright::ending::{Ending, Event, Signal, Step, StopBy};

/// `docs/specs/operation.md`, "Stopping".
#[test]
fn the_signals_drain_and_its_deadline_take_serve_from_serving_to_draining_stopping_and_exited() {
    let quit = Event::Signal(Signal::Quit);
    let interrupt = Event::Signal(Signal::Interrupt);
    let terminate = Event::Signal(Signal::Terminate);
    let one = at("2026-10-01T14:00:00Z");
    let other = at("2026-10-01T15:00:00Z");
    let drain = Event::Drain { deadline: None };
    let drain_by = |at| Event::Drain { deadline: Some(at) };
    let draining = Ending::Draining { deadline: None };
    let draining_by = |at| Ending::Draining { deadline: Some(at) };
    let stopping = Ending::Stopping { by: StopBy::Signal };
    let stopping_at_deadline = Ending::Stopping {
        by: StopBy::Deadline,
    };
    let table = [
        (Ending::Serving, quit, draining, Step::Drain),
        (Ending::Serving, drain, draining, Step::Drain),
        (
            Ending::Serving,
            drain_by(one),
            draining_by(one),
            Step::Drain,
        ),
        (Ending::Serving, interrupt, stopping, Step::Stop),
        (Ending::Serving, terminate, stopping, Step::Stop),
        (draining, quit, draining, Step::Nothing),
        (draining_by(one), quit, draining_by(one), Step::Nothing),
        // The latest `drain` sets the deadline.
        (draining, drain, draining, Step::SetDeadline),
        (draining, drain_by(one), draining_by(one), Step::SetDeadline),
        (
            draining_by(one),
            drain_by(other),
            draining_by(other),
            Step::SetDeadline,
        ),
        (draining_by(one), drain, draining, Step::SetDeadline),
        (draining, interrupt, stopping, Step::Stop),
        (draining, terminate, stopping, Step::Stop),
        (draining_by(one), terminate, stopping, Step::Stop),
        (
            draining_by(one),
            Event::Deadline(one),
            stopping_at_deadline,
            Step::Stop,
        ),
        // A deadline replaced before it passed, or passed once stopping.
        (
            draining_by(other),
            Event::Deadline(one),
            draining_by(other),
            Step::PassedUnheeded,
        ),
        (
            draining,
            Event::Deadline(one),
            draining,
            Step::PassedUnheeded,
        ),
        (
            stopping,
            Event::Deadline(one),
            stopping,
            Step::PassedUnheeded,
        ),
        (stopping, quit, stopping, Step::Nothing),
        (stopping, drain, stopping, Step::Nothing),
        (stopping, drain_by(one), stopping, Step::Nothing),
        (stopping, interrupt, Ending::Exited, Step::Exit),
        (stopping, terminate, Ending::Exited, Step::Exit),
    ];

    for (state, event, next, step) in table {
        assert_eq!(state.on(event), (next, step), "{state:?} on {event:?}");
    }
}

#[test]
fn an_exit_at_once_has_status_128_plus_the_signals_number() {
    assert_eq!(Signal::Interrupt.exit_status(), 130);
    assert_eq!(Signal::Terminate.exit_status(), 143);
}
