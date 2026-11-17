use std::time::Duration;

use tokio::time::Instant;

use crate::domain::Timestamp;

/// When polls start: each one interval after the previous poll started, and
/// at once after a poll that took longer than the interval, never alongside
/// it.
pub struct Cadence {
    interval: Duration,
    due: Instant,
}

impl Cadence {
    /// A cadence whose first poll is due at once.
    pub fn new(interval: Duration) -> Cadence {
        Cadence {
            interval,
            due: Instant::now(),
        }
    }

    /// Starts a poll at `now`, and returns the time the next poll is due at:
    /// one interval later.
    pub fn start(&mut self, now: Timestamp) -> Timestamp {
        self.due = Instant::now() + self.interval;
        now + self.interval
    }

    /// Returns once the next poll is due, at once when it is overdue.
    pub async fn due(&self) {
        tokio::time::sleep_until(self.due).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn millis(offsets: &[u64]) -> Vec<Duration> {
        offsets.iter().copied().map(Duration::from_millis).collect()
    }

    #[track_caller]
    fn at(time: &str) -> Timestamp {
        time.parse().unwrap()
    }

    /// When each poll started, as offsets from the first, for polls that take
    /// `durations` and a select woken every `wake_every` between them.
    async fn starts(durations: &[Duration], wake_every: Duration) -> Vec<Duration> {
        let mut cadence = Cadence::new(Duration::from_millis(100));
        let mut wakes = tokio::time::interval(wake_every);
        let first = Instant::now();
        let mut starts = vec![];
        for duration in durations {
            loop {
                tokio::select! {
                    biased;
                    () = cadence.due() => break,
                    _ = wakes.tick() => {}
                }
            }
            starts.push(Instant::now() - first);
            cadence.start(at("2026-10-01T12:00:00Z"));
            tokio::time::sleep(*duration).await;
        }
        starts
    }

    #[test_log::test(tokio::test(start_paused = true))]
    async fn a_poll_starts_one_interval_after_the_previous_poll_started() {
        let polls = millis(&[10, 250, 30, 30, 180, 10]);

        let starts = starts(&polls, Duration::from_hours(1)).await;

        assert_eq!(starts, millis(&[0, 100, 350, 450, 550, 730]));
    }

    #[test_log::test(tokio::test(start_paused = true))]
    async fn wakes_between_polls_do_not_move_the_next_poll() {
        let polls = millis(&[10, 250, 30, 30, 180, 10]);

        let starts = starts(&polls, Duration::from_millis(30)).await;

        assert_eq!(starts, millis(&[0, 100, 350, 450, 550, 730]));
    }
}
