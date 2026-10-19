//! How `serve` ends, on the signals it is sent and the drains `lw drain`
//! asks for (`docs/specs/operation.md`, "Stopping").

use serde::{Deserialize, Serialize};
use tokio::signal::unix::SignalKind;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::clock::Clock;
use crate::domain::Timestamp;

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
    /// A drain asked for through the local API, which sets the drain's
    /// deadline, or leaves it with none.
    Drain {
        deadline: Option<Timestamp>,
    },
    /// The deadline of a drain has passed.
    Deadline(Timestamp),
}

impl Event {
    fn name(self) -> &'static str {
        match self {
            Event::Signal(signal) => signal.name(),
            Event::Drain { .. } => "`drain`",
            Event::Deadline(_) => "the drain's deadline",
        }
    }
}

/// Where `serve` is in ending.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ending {
    #[default]
    Serving,
    /// Becomes a stop once `deadline`, if any, passes.
    Draining {
        deadline: Option<Timestamp>,
    },
    Stopping {
        by: StopBy,
    },
    Exited,
}

/// What began a stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopBy {
    Signal,
    Deadline,
}

/// What an event has `serve` do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Drain,
    /// The drain under way takes the latest `drain`'s deadline, or none.
    SetDeadline,
    Stop,
    Nothing,
    /// A deadline passed that a later `drain` replaced, or that came after
    /// the stop began.
    PassedUnheeded,
    Exit,
}

impl Ending {
    pub fn on(self, event: Event) -> (Ending, Step) {
        use Signal::{Interrupt, Quit, Terminate};
        match (self, event) {
            (Ending::Serving, Event::Signal(Quit)) => {
                (Ending::Draining { deadline: None }, Step::Drain)
            }
            (Ending::Serving, Event::Drain { deadline }) => {
                (Ending::Draining { deadline }, Step::Drain)
            }
            (Ending::Serving | Ending::Draining { .. }, Event::Signal(Interrupt | Terminate)) => {
                (Ending::Stopping { by: StopBy::Signal }, Step::Stop)
            }
            // The latest `drain` sets the deadline.
            (Ending::Draining { .. }, Event::Drain { deadline }) => {
                (Ending::Draining { deadline }, Step::SetDeadline)
            }
            (Ending::Draining { deadline }, Event::Deadline(passed))
                if deadline == Some(passed) =>
            {
                (
                    Ending::Stopping {
                        by: StopBy::Deadline,
                    },
                    Step::Stop,
                )
            }
            (
                Ending::Serving | Ending::Draining { .. } | Ending::Stopping { .. },
                Event::Deadline(_),
            ) => (self, Step::PassedUnheeded),
            (Ending::Draining { .. } | Ending::Stopping { .. }, Event::Signal(Quit))
            | (Ending::Stopping { .. }, Event::Drain { .. }) => (self, Step::Nothing),
            // A process that has exited reads no more signals.
            (Ending::Stopping { .. }, Event::Signal(Interrupt | Terminate))
            | (Ending::Exited, _) => (Ending::Exited, Step::Exit),
        }
    }

    /// The deadline of the drain under way, if it has one.
    pub fn deadline(self) -> Option<Timestamp> {
        match self {
            Ending::Draining { deadline } => deadline,
            Ending::Serving | Ending::Stopping { .. } | Ending::Exited => None,
        }
    }
}

/// Where `serve` is in ending, shared by what asks it to end: it takes each
/// event as [`Ending::on`] has it, logging what it does.
#[derive(Clone, Debug)]
pub struct Ends {
    state: watch::Sender<Ending>,
    draining: CancellationToken,
    stopping: CancellationToken,
}

impl Default for Ends {
    fn default() -> Ends {
        Ends {
            state: watch::Sender::new(Ending::default()),
            draining: CancellationToken::new(),
            stopping: CancellationToken::new(),
        }
    }
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

    /// The deadline of the drain under way, if it has one.
    pub fn deadline(&self) -> Option<Timestamp> {
        self.state.borrow().deadline()
    }

    /// Returns where `serve` was in ending, and the step `event` takes, which
    /// is left to the caller only if it is [`Step::Exit`].
    pub fn on(&self, event: Event) -> (Ending, Step) {
        let mut was = Ending::default();
        let mut now = Ending::default();
        let mut step = Step::Nothing;
        self.state.send_if_modified(|state| {
            was = *state;
            (now, step) = was.on(event);
            *state = now;
            now != was
        });
        let by = event.name();
        let deadline = now.deadline().map(|at| at.to_rfc3339());
        match step {
            Step::Drain => {
                info!(
                    by,
                    deadline,
                    "draining: exiting once no run is in flight; \
                     SIGINT or SIGTERM stops instead"
                );
                self.draining.cancel();
            }
            Step::SetDeadline => info!(
                by,
                deadline,
                replaced = was.deadline().map(|at| at.to_rfc3339()),
                "already draining; the deadline is the latest `drain`'s"
            ),
            Step::Stop => {
                info!(
                    by,
                    "stopping after the action in progress; \
                     a second SIGINT or SIGTERM exits at once"
                );
                self.stopping.cancel();
            }
            Step::Nothing => info!(by, "already ending; nothing more"),
            Step::PassedUnheeded | Step::Exit => {}
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

    /// Waits until a stop begins, and says what began it: `None` once
    /// `serve` is exiting at once.
    pub async fn stop_begun(&self) -> Option<StopBy> {
        self.stopping.cancelled().await;
        match *self.state.borrow() {
            Ending::Stopping { by } => Some(by),
            Ending::Serving | Ending::Draining { .. } | Ending::Exited => None,
        }
    }

    /// Waits on `clock` until the deadline of the drain under way passes,
    /// whichever `drain` set it, and takes it, which turns the drain into a
    /// stop.
    pub async fn deadline_passed(&self, clock: &dyn Clock) {
        let mut state = self.state.subscribe();
        loop {
            let set = state.borrow_and_update().deadline();
            let changed = state.changed();
            match set {
                Some(at) => tokio::select! {
                    () = clock.sleep_until(at) => {
                        self.on(Event::Deadline(at));
                        return;
                    }
                    _ = changed => {}
                },
                None => {
                    // The sender lives as long as `self`.
                    let _ = changed.await;
                }
            }
        }
    }
}
