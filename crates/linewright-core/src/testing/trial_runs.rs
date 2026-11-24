use std::path::Path;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::harness::UsageLimit;
use crate::session::Cancellation;
use crate::testing::write_files;
use crate::trial::{Configuration, Under};

/// What the benchmark runner runs a trial or a scorer under: a cancellation
/// that nothing else holds, and sessions that wait out the usage limit,
/// reporting it to the receiver it gives.
pub fn under_reporting() -> (Under, UnboundedReceiver<UsageLimit>) {
    let (report_refusal, reported) = tokio::sync::mpsc::unbounded_channel();
    let under = Under {
        cancel: Cancellation::default(),
        report_refusal,
    };
    (under, reported)
}

/// A usage limit's reset a second after the moment it is asked for, as the
/// clock of a trial of a task that starts at `start` tells it: the clock
/// runs from `start` once the trial begins, after this is made, so the
/// reset is always ahead of it, by about a second.
#[derive(Clone)]
pub struct SoonAfter {
    start: crate::domain::Timestamp,
    made: std::time::Instant,
    given: std::sync::Arc<std::sync::Mutex<Option<crate::domain::Timestamp>>>,
}

/// A [`SoonAfter`] made now, for a trial of a task that starts at `start`.
pub fn soon_after(start: crate::domain::Timestamp) -> SoonAfter {
    SoonAfter {
        start,
        made: std::time::Instant::now(),
        given: std::sync::Arc::default(),
    }
}

impl SoonAfter {
    /// The reset, as of now, which [`SoonAfter::given`] then gives.
    pub fn at(&self) -> crate::domain::Timestamp {
        let elapsed = chrono::TimeDelta::from_std(self.made.elapsed()).unwrap();
        let at = self.start + elapsed + chrono::TimeDelta::seconds(1);
        *self.given.lock().unwrap() = Some(at);
        at
    }

    /// The last reset [`SoonAfter::at`] gave, failing when it gave none.
    pub fn given(&self) -> crate::domain::Timestamp {
        self.given.lock().unwrap().expect("a reset was given")
    }
}

/// [`under_reporting`], whose reports nothing reads.
pub fn under_unread() -> Under {
    under_reporting().0
}

/// The configuration in `dir`, holding `files` by their paths in it.
pub fn configuration(dir: &Path, files: &[(&str, &str)]) -> Configuration {
    std::fs::create_dir_all(dir).unwrap();
    write_files(dir, files);
    Configuration::read(dir).unwrap_or_else(|e| panic!("{e}"))
}

/// The names of the entries in `dir`.
pub fn entries(dir: &Path) -> Vec<String> {
    let mut names = vec![];
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("listing {}: {e}", dir.display()))
    {
        let entry = entry.unwrap_or_else(|e| panic!("listing {}: {e}", dir.display()));
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    names
}
