use async_trait::async_trait;

use crate::domain::Timestamp;

#[async_trait]
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
    /// Returns once `at` has come.
    async fn sleep_until(&self, at: Timestamp);
}

pub struct SystemClock;

#[async_trait]
impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        chrono::Utc::now()
    }

    async fn sleep_until(&self, at: Timestamp) {
        // The wall clock may run ahead of or behind the timer, as across a
        // suspend, so it is read again on waking.
        loop {
            let Ok(left) = (at - self.now()).to_std() else {
                return;
            };
            if left.is_zero() {
                return;
            }
            tokio::time::sleep(left).await;
        }
    }
}
