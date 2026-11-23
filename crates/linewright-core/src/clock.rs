use async_trait::async_trait;

use crate::domain::Timestamp;

#[async_trait]
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
    /// Returns once [`Clock::now`] is at or past `at`.
    async fn sleep_until(&self, at: Timestamp);
}

pub struct SystemClock;

#[async_trait]
impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        chrono::Utc::now()
    }

    async fn sleep_until(&self, at: Timestamp) {
        sleep_until(self, at).await;
    }
}

/// The time as the system's clock would tell it had it read `from` when this
/// was made: it runs on from there at the system's pace.
pub struct RunningFrom {
    from: Timestamp,
    made: std::time::Instant,
}

impl RunningFrom {
    pub fn new(from: Timestamp) -> RunningFrom {
        RunningFrom {
            from,
            made: std::time::Instant::now(),
        }
    }
}

#[async_trait]
impl Clock for RunningFrom {
    fn now(&self) -> Timestamp {
        let elapsed = chrono::TimeDelta::from_std(self.made.elapsed())
            .expect("a running clock is younger than chrono's limit");
        self.from + elapsed
    }

    async fn sleep_until(&self, at: Timestamp) {
        sleep_until(self, at).await;
    }
}

/// Returns once `clock` is at or past `at`, for a clock that keeps the
/// system's pace.
async fn sleep_until(clock: &dyn Clock, at: Timestamp) {
    loop {
        let Ok(left) = (at - clock.now()).to_std() else {
            return;
        };
        if left.is_zero() {
            return;
        }
        tokio::time::sleep(left).await;
    }
}
