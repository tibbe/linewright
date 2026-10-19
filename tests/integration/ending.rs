use linewright::ending::{Ending, Signal, Step};

/// `docs/specs/operation.md`, "Stopping".
#[test]
fn the_signals_take_serve_from_serving_to_draining_stopping_and_exited() {
    let table = [
        (Ending::Serving, Signal::Quit, Ending::Draining, Step::Drain),
        (
            Ending::Serving,
            Signal::Interrupt,
            Ending::Stopping,
            Step::Stop,
        ),
        (
            Ending::Serving,
            Signal::Terminate,
            Ending::Stopping,
            Step::Stop,
        ),
        (
            Ending::Draining,
            Signal::Quit,
            Ending::Draining,
            Step::Nothing,
        ),
        (
            Ending::Draining,
            Signal::Interrupt,
            Ending::Stopping,
            Step::Stop,
        ),
        (
            Ending::Draining,
            Signal::Terminate,
            Ending::Stopping,
            Step::Stop,
        ),
        (
            Ending::Stopping,
            Signal::Quit,
            Ending::Stopping,
            Step::Nothing,
        ),
        (
            Ending::Stopping,
            Signal::Interrupt,
            Ending::Exited,
            Step::Exit,
        ),
        (
            Ending::Stopping,
            Signal::Terminate,
            Ending::Exited,
            Step::Exit,
        ),
    ];

    for (state, signal, next, step) in table {
        assert_eq!(state.on(signal), (next, step), "{state:?} on {signal:?}");
    }
}

#[test]
fn an_exit_at_once_has_status_128_plus_the_signals_number() {
    assert_eq!(Signal::Interrupt.exit_status(), 130);
    assert_eq!(Signal::Terminate.exit_status(), 143);
}
