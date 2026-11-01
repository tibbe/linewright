use std::future::Future;
use std::panic::Location;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::TestWriter;
use tracing_subscriber::fmt::writer::MakeWriterExt;

#[derive(Clone, Default)]
pub struct Log(Arc<Mutex<Vec<u8>>>);

impl Log {
    /// Captures what this thread logs until the guard is dropped, as well as
    /// writing it to the test's output. A `#[tokio::test]` runs every task on
    /// its one thread.
    ///
    /// While just one subscriber exists, tracing caches whether a log line is
    /// wanted from the subscriber of the thread that first reaches it, so
    /// another test's thread would turn this one's lines off. The global one
    /// each test's `test_log` attribute installs makes it ask each thread's
    /// own.
    pub fn capture() -> (Log, tracing::subscriber::DefaultGuard) {
        let log = Log::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer((move || writer.clone()).and(TestWriter::with_stderr))
            .with_ansi(false)
            .finish();
        (log, tracing::subscriber::set_default(subscriber))
    }

    /// All that was logged.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    /// The lines logged at `level`, such as `WARN`.
    pub fn lines(&self, level: &str) -> Vec<String> {
        String::from_utf8_lossy(&self.0.lock().unwrap())
            .lines()
            .filter(|line| line.split_whitespace().nth(1) == Some(level))
            .map(str::to_string)
            .collect()
    }
}

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Log {
    pub fn usage_limit_warnings(&self) -> Vec<String> {
        self.lines("WARN")
            .into_iter()
            .filter(|line| line.contains("usage limit") && line.contains("2026-10-13 07:50:00"))
            .collect()
    }
}

/// Waits until `done` holds, failing the test at the line that waited if it
/// doesn't within 10s.
#[track_caller]
pub fn eventually(done: impl Fn() -> bool) -> impl Future<Output = ()> {
    eventually_async(move || std::future::ready(done()))
}

/// [`eventually`], for a condition that is async.
#[track_caller]
pub fn eventually_async<F: Future<Output = bool>>(
    done: impl Fn() -> F,
) -> impl Future<Output = ()> {
    let caller = Location::caller();
    async move {
        let waited = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !done().await {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(waited.is_ok(), "timed out waiting at {caller}");
    }
}
