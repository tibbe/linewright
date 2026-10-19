//! How `serve` ends, on the signals it is sent and the drains `lw drain`
//! asks for (`docs/specs/operation.md`, "Stopping").

use std::sync::{Arc, Mutex};

use tokio::signal::unix::SignalKind;
use tokio_util::sync::CancellationToken;
use tracing::info;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Quit,
    Interrupt,
    Terminate,
}

impl Signal {
    pub fn name(self) -> &'static str {
        match self {
            Signal::Quit => "SIGQUIT",
            Signal::Interrupt => "SIGINT",
            Signal::Terminate => "SIGTERM",
        }
    }

    pub fn kind(self) -> SignalKind {
        match self {
            Signal::Quit => SignalKind::quit(),
            Signal::Interrupt => SignalKind::interrupt(),
            Signal::Terminate => SignalKind::terminate(),
        }
    }

    /// 128 plus the signal's number, as a shell reports it
    /// (<https://www.gnu.org/software/bash/manual/html_node/Exit-Status.html>).
    pub fn exit_status(self) -> i32 {
        128 + self.kind().as_raw_value()
    }
}

/// What asks `serve` to end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Signal(Signal),
    /// A drain asked for through the local API.
    Drain,
}

impl Event {
    fn name(self) -> &'static str {
        match self {
            Event::Signal(signal) => signal.name(),
            Event::Drain => "`drain`",
        }
    }
}

/// Where `serve` is in ending.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ending {
    #[default]
    Serving,
    Draining,
    Stopping,
    Exited,
}

/// What a signal has `serve` do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Drain,
    Stop,
    Nothing,
    Exit,
}

impl Ending {
    pub fn on(self, event: Event) -> (Ending, Step) {
        use Signal::{Interrupt, Quit, Terminate};
        match (self, event) {
            (Ending::Serving, Event::Signal(Quit) | Event::Drain) => {
                (Ending::Draining, Step::Drain)
            }
            (Ending::Serving | Ending::Draining, Event::Signal(Interrupt | Terminate)) => {
                (Ending::Stopping, Step::Stop)
            }
            (Ending::Draining | Ending::Stopping, Event::Signal(Quit) | Event::Drain) => {
                (self, Step::Nothing)
            }
            // A process that has exited reads no more signals.
            (Ending::Stopping, Event::Signal(Interrupt | Terminate)) | (Ending::Exited, _) => {
                (Ending::Exited, Step::Exit)
            }
        }
    }
}

/// Where `serve` is in ending, shared by what asks it to end: it takes each
/// event as [`Ending::on`] has it, logging what it does.
#[derive(Clone, Debug, Default)]
pub struct Ends {
    state: Arc<Mutex<Ending>>,
    draining: CancellationToken,
    stopping: CancellationToken,
}

impl Ends {
    pub fn new() -> Ends {
        Ends::default()
    }

    /// Cancelled once a drain begins.
    pub fn draining(&self) -> CancellationToken {
        self.draining.clone()
    }

    /// Cancelled once a stop begins.
    pub fn stopping(&self) -> CancellationToken {
        self.stopping.clone()
    }

    /// Returns where `serve` was in ending, and the step `event` takes, which
    /// is left to the caller only if it is [`Step::Exit`].
    pub fn on(&self, event: Event) -> (Ending, Step) {
        let mut state = self.state.lock().expect("no holder of the lock panics");
        let was = *state;
        let step;
        (*state, step) = was.on(event);
        drop(state);
        let by = event.name();
        match step {
            Step::Drain => {
                info!(
                    by,
                    "draining: exiting once no run is in flight; \
                     SIGINT or SIGTERM stops instead"
                );
                self.draining.cancel();
            }
            Step::Stop => {
                info!(
                    by,
                    "stopping after the action in progress; \
                     a second SIGINT or SIGTERM exits at once"
                );
                self.stopping.cancel();
            }
            Step::Nothing => info!(by, "already ending; nothing more"),
            Step::Exit => {}
        }
        (was, step)
    }

    /// Waits until a drain or a stop begins.
    pub async fn drain_or_stop_begun(&self) {
        tokio::select! {
            () = self.draining.cancelled() => {}
            () = self.stopping.cancelled() => {}
        }
    }
}
