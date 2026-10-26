use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Default)]
pub struct Log(Arc<Mutex<Vec<u8>>>);

impl Log {
    /// Captures what this thread logs until the guard is dropped. A
    /// `#[tokio::test]` runs every task on its one thread.
    pub fn capture() -> (Log, tracing::subscriber::DefaultGuard) {
        // While just one subscriber exists, tracing caches whether a log line
        // is wanted from the subscriber of the thread that first reaches it,
        // so another test's thread would turn this one's lines off. A second
        // subscriber, kept for the whole run, makes it ask each thread's own.
        static SECOND_SUBSCRIBER: OnceLock<tracing::Dispatch> = OnceLock::new();
        SECOND_SUBSCRIBER
            .get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
        let log = Log::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
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

/// The IDs `runs` lists, leaving out the lines under each run of its
/// sessions.
pub fn run_ids(listed: &str) -> Vec<&str> {
    listed
        .lines()
        .filter(|line| !line.starts_with(' '))
        .map(|line| line.split_once(':').unwrap().0)
        .collect()
}

pub async fn eventually(done: impl Fn() -> bool) {
    eventually_async(|| std::future::ready(done())).await;
}

pub async fn eventually_async<F: Future<Output = bool>>(done: impl Fn() -> F) {
    let waited = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !done().await {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "timed out");
}
