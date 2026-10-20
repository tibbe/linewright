use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use linewright::clock::Clock;
use linewright::domain::Timestamp;

/// The time the functional core is asked at.
pub const NOW: &str = "2026-10-01T12:00:30Z";

/// 2026-10-01 is a Thursday.
pub fn at(time: &str) -> Timestamp {
    DateTime::parse_from_rfc3339(time)
        .unwrap()
        .with_timezone(&Utc)
}

pub struct FakeClock {
    now: Mutex<Timestamp>,
    advanced: tokio::sync::Notify,
    sleeps: tokio::sync::mpsc::UnboundedSender<Timestamp>,
    slept: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<Timestamp>>,
}

impl FakeClock {
    pub fn new() -> FakeClock {
        let (sleeps, slept) = tokio::sync::mpsc::unbounded_channel();
        FakeClock {
            now: Mutex::new(
                DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            advanced: tokio::sync::Notify::new(),
            sleeps,
            slept: tokio::sync::Mutex::new(slept),
        }
    }

    pub fn advance(&self, by: TimeDelta) {
        *self.now.lock().unwrap() += by;
        self.advanced.notify_waiters();
    }

    pub async fn next_sleep(&self) -> Timestamp {
        // No timeout: under paused time it would fire while git runs.
        self.slept.lock().await.recv().await.unwrap()
    }
}

#[async_trait]
impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        let mut now = self.now.lock().unwrap();
        *now += TimeDelta::seconds(1);
        *now
    }

    async fn sleep_until(&self, at: Timestamp) {
        let _ = self.sleeps.send(at);
        loop {
            let advanced = self.advanced.notified();
            if *self.now.lock().unwrap() >= at {
                return;
            }
            advanced.await;
        }
    }
}
