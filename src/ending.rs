//! How `serve` ends, on the signals it is sent (`docs/specs/operation.md`,
//! "Stopping").

use tokio::signal::unix::SignalKind;

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

/// Where `serve` is in ending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
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
    pub fn on(self, signal: Signal) -> (Ending, Step) {
        match (self, signal) {
            (Ending::Serving, Signal::Quit) => (Ending::Draining, Step::Drain),
            (Ending::Serving | Ending::Draining, Signal::Interrupt | Signal::Terminate) => {
                (Ending::Stopping, Step::Stop)
            }
            (Ending::Draining | Ending::Stopping, Signal::Quit) => (self, Step::Nothing),
            // A process that has exited reads no more signals.
            (Ending::Stopping | Ending::Exited, Signal::Interrupt | Signal::Terminate)
            | (Ending::Exited, Signal::Quit) => (Ending::Exited, Step::Exit),
        }
    }
}
