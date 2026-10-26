use crate::common;

use common::{Log, at};
use linewright::ending::{Ending, Ends, Event, Signal, Step, StopBy};

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
        (Ending::Serving, terminate, draining, Step::Drain),
        (Ending::Serving, interrupt, draining, Step::Drain),
        (Ending::Serving, drain, draining, Step::Drain),
        (
            Ending::Serving,
            drain_by(one),
            draining_by(one),
            Step::Drain,
        ),
        (Ending::Serving, quit, stopping, Step::Stop),
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
        (draining, terminate, stopping, Step::Stop),
        (draining, interrupt, stopping, Step::Stop),
        (draining, quit, stopping, Step::Stop),
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
        (stopping, drain, stopping, Step::Nothing),
        (stopping, drain_by(one), stopping, Step::Nothing),
        (stopping, terminate, Ending::Exited, Step::Exit),
        (stopping, interrupt, Ending::Exited, Step::Exit),
        (stopping, quit, Ending::Exited, Step::Exit),
    ];

    for (state, event, next, step) in table {
        assert_eq!(state.on(event), (next, step), "{state:?} on {event:?}");
    }
}

#[test]
fn an_exit_at_once_has_status_128_plus_the_signals_number() {
    assert_eq!(Signal::Interrupt.exit_status(), 130);
    assert_eq!(Signal::Terminate.exit_status(), 143);
    assert_eq!(Signal::Quit.exit_status(), 131);
}

#[tokio::test]
async fn each_way_a_signal_takes_is_logged_with_the_signal_and_what_the_next_signal_does() {
    let (log, _guard) = Log::capture();
    let ends = Ends::new();

    ends.on(Event::Signal(Signal::Interrupt));
    ends.on(Event::Signal(Signal::Quit));
    ends.on(Event::Signal(Signal::Terminate));

    let lines = log.lines("INFO");
    let [drained, stopped, exited] = lines.as_slice() else {
        panic!("{lines:?}");
    };
    assert!(
        drained.contains(
            "draining: exiting once no run is in flight; the next signal stops instead by=\"SIGINT\""
        ),
        "{drained}"
    );
    assert!(
        stopped.contains(
            "stopping after the action in progress; the next signal exits at once by=\"SIGQUIT\""
        ),
        "{stopped}"
    );
    assert!(
        exited.contains("exiting at once, without waiting for the runs in flight by=\"SIGTERM\""),
        "{exited}"
    );
}
