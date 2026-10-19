use linewright::ending::{Ending, Event, Signal, Step};

/// `docs/specs/operation.md`, "Stopping".
#[test]
fn the_signals_and_drain_take_serve_from_serving_to_draining_stopping_and_exited() {
    let quit = Event::Signal(Signal::Quit);
    let interrupt = Event::Signal(Signal::Interrupt);
    let terminate = Event::Signal(Signal::Terminate);
    let table = [
        (Ending::Serving, quit, Ending::Draining, Step::Drain),
        (Ending::Serving, Event::Drain, Ending::Draining, Step::Drain),
        (Ending::Serving, interrupt, Ending::Stopping, Step::Stop),
        (Ending::Serving, terminate, Ending::Stopping, Step::Stop),
        (Ending::Draining, quit, Ending::Draining, Step::Nothing),
        (
            Ending::Draining,
            Event::Drain,
            Ending::Draining,
            Step::Nothing,
        ),
        (Ending::Draining, interrupt, Ending::Stopping, Step::Stop),
        (Ending::Draining, terminate, Ending::Stopping, Step::Stop),
        (Ending::Stopping, quit, Ending::Stopping, Step::Nothing),
        (
            Ending::Stopping,
            Event::Drain,
            Ending::Stopping,
            Step::Nothing,
        ),
        (Ending::Stopping, interrupt, Ending::Exited, Step::Exit),
        (Ending::Stopping, terminate, Ending::Exited, Step::Exit),
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
