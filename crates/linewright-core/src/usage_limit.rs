//! The waits for the usage limit under way (`docs/specs/runs.md`, Usage
//! limit): which sessions wait, the hold they make, and the pause of each
//! waiting session's timeout.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::domain::Timestamp;
use crate::session::lock_set_whole;
use crate::transcript;

/// The waits for the usage limit under way in one process, `serve`'s or
/// `lw`'s: its clones share them.
#[derive(Clone)]
pub struct UsageLimitWaits(Arc<Registry>);

struct Registry {
    live: Mutex<Live>,
    held: watch::Sender<Option<UsageLimitHeld>>,
}

/// The waits that live, each with the reset time it stated last, if any.
#[derive(Default)]
struct Live {
    /// How many begins and statements of waits there have been.
    events: u64,
    waits: BTreeMap<WaitId, Option<Stated>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct WaitId(u64);

/// A reset time a wait stated, and when, among the begins and statements of
/// waits.
#[derive(Clone, Copy)]
struct Stated {
    event: u64,
    resets_at: Timestamp,
}

impl Live {
    fn event(&mut self) -> u64 {
        let event = self.events;
        self.events += 1;
        event
    }

    fn stated(&mut self, resets_at: Timestamp) -> Stated {
        Stated {
            event: self.event(),
            resets_at,
        }
    }

    fn held(&self) -> Option<UsageLimitHeld> {
        (!self.waits.is_empty()).then(|| UsageLimitHeld {
            resets_at: self
                .waits
                .values()
                .flatten()
                .max_by_key(|stated| stated.event)
                .map(|stated| stated.resets_at),
        })
    }
}

/// The usage limit's hold (`docs/specs/runs.md`, Holds), in force while a
/// session waits for the usage limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UsageLimitHeld {
    /// The reset time stated last by a session that waits now, if any of
    /// them stated one.
    pub resets_at: Option<Timestamp>,
}

impl Default for UsageLimitWaits {
    fn default() -> UsageLimitWaits {
        UsageLimitWaits(Arc::new(Registry {
            live: Mutex::default(),
            held: watch::Sender::new(None),
        }))
    }
}

impl std::fmt::Debug for UsageLimitWaits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("UsageLimitWaits")
            .field(&*self.0.held.borrow())
            .finish()
    }
}

impl UsageLimitWaits {
    /// The waits of a session that starts at `start` with the session
    /// timeout `timeout`.
    pub fn session(&self, start: Instant, timeout: Duration) -> Waits {
        Waits {
            registry: self.clone(),
            deadline: Deadline::after(start, timeout),
        }
    }

    /// The hold, `Some` while any session waits, changing as waits begin,
    /// restate their reset time and end.
    pub fn held(&self) -> watch::Receiver<Option<UsageLimitHeld>> {
        self.0.held.subscribe()
    }

    /// Changes the waits with `change`, and the hold with them.
    fn change<T>(&self, change: impl FnOnce(&mut Live) -> T) -> T {
        let mut live = lock_set_whole(&self.0.live);
        let changed = change(&mut live);
        let held = live.held();
        self.0.held.send_if_modified(|was| {
            let differs = *was != held;
            *was = held;
            differs
        });
        changed
    }
}

/// A session's waits for the usage limit, and its deadline, which they
/// pause.
#[derive(Clone, Debug)]
pub struct Waits {
    registry: UsageLimitWaits,
    deadline: Deadline,
}

impl Waits {
    /// When the session times out.
    pub fn deadline(&self) -> &Deadline {
        &self.deadline
    }

    /// Begins a wait `at`, with the reset time `stated`, if one was: until the
    /// [`Waiting`] is dropped, the session's deadline is paused and the hold
    /// is in force. Gives the line to append to the session's transcript.
    pub fn begin(&self, at: Timestamp, stated: Option<Timestamp>) -> (Waiting, String) {
        let id = self.registry.change(|live| {
            let id = WaitId(live.event());
            let stated = stated.map(|resets_at| live.stated(resets_at));
            live.waits.insert(id, stated);
            id
        });
        self.deadline.pause(Instant::now());
        let waiting = Waiting {
            id,
            waits: self.clone(),
        };
        (waiting, transcript::usage_limit_waiting(at, stated))
    }
}

/// A wait for the usage limit under way, which ends when it is dropped.
#[derive(Debug)]
pub struct Waiting {
    id: WaitId,
    waits: Waits,
}

impl Waiting {
    /// The wait states `at` that the limit resets at `resets_at`. Gives the
    /// line to append to the session's transcript.
    pub fn restate(&mut self, at: Timestamp, resets_at: Timestamp) -> String {
        let id = self.id;
        self.waits.registry.change(|live| {
            let stated = live.stated(resets_at);
            live.waits.insert(id, Some(stated));
        });
        transcript::usage_limit_waiting(at, Some(resets_at))
    }

    /// Ends the wait `at`. Gives the line to append to the session's
    /// transcript.
    pub fn end(self, at: Timestamp) -> String {
        transcript::usage_limit_waited(at)
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let id = self.id;
        self.waits.registry.change(|live| live.waits.remove(&id));
        self.waits.deadline.resume(Instant::now());
    }
}

/// When a session times out: its session timeout counts from its start,
/// less its waits for the usage limit (`docs/specs/runs.md`, Limits, and
/// Usage limit, The session timeout). Its clones share it.
#[derive(Clone, Debug)]
pub struct Deadline(Arc<watch::Sender<Counting>>);

#[derive(Clone, Copy, Debug)]
struct Counting {
    start: Instant,
    timeout: Duration,
    /// How long the waits that ended paused it.
    paused: Duration,
    pause: Option<Pause>,
}

/// A pause of a deadline, while any of its session's waits lives.
#[derive(Clone, Copy, Debug)]
struct Pause {
    since: Instant,
    waits: usize,
}

impl Counting {
    fn counted(&self, now: Instant) -> Duration {
        let pausing = self.pause.map_or(Duration::ZERO, |pause| {
            now.saturating_duration_since(pause.since)
        });
        now.saturating_duration_since(self.start)
            .saturating_sub(self.paused.saturating_add(pausing))
    }

    /// When it is reached, unless it is paused or beyond what an instant
    /// holds.
    fn due(&self) -> Option<Instant> {
        match self.pause {
            Some(_) => None,
            None => self
                .start
                .checked_add(self.timeout.saturating_add(self.paused)),
        }
    }
}

impl Deadline {
    fn after(start: Instant, timeout: Duration) -> Deadline {
        Deadline(Arc::new(watch::Sender::new(Counting {
            start,
            timeout,
            paused: Duration::ZERO,
            pause: None,
        })))
    }

    /// Ends once the session timeout has counted the whole timeout.
    pub async fn reached(&self) {
        let mut changes = self.0.subscribe();
        loop {
            let due = changes.borrow_and_update().due();
            let paused_or_resumed = async {
                let _ = changes.changed().await;
            };
            match due {
                Some(due) => tokio::select! {
                    () = tokio::time::sleep_until(due) => return,
                    () = paused_or_resumed => {},
                },
                None => paused_or_resumed.await,
            }
        }
    }

    /// What the session timeout has counted by `now`.
    pub fn counted(&self, now: Instant) -> Duration {
        self.0.borrow().counted(now)
    }

    fn pause(&self, now: Instant) {
        self.0.send_modify(|counting| {
            counting.pause = Some(match counting.pause {
                Some(pause) => Pause {
                    waits: pause.waits + 1,
                    ..pause
                },
                None => Pause {
                    since: now,
                    waits: 1,
                },
            });
        });
    }

    fn resume(&self, now: Instant) {
        self.0.send_modify(|counting| match counting.pause {
            Some(Pause { since, waits: 1 }) => {
                counting.paused = counting
                    .paused
                    .saturating_add(now.saturating_duration_since(since));
                counting.pause = None;
            }
            Some(pause) => {
                counting.pause = Some(Pause {
                    waits: pause.waits - 1,
                    ..pause
                });
            }
            None => {}
        });
    }
}
