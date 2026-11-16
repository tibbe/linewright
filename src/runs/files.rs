//! The file form of a run: its record, and its sessions' directories, under
//! a directory.

use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{
    Begin, Ending, Keeping, Ran, Recorded, RunId, RunOf, Running, SessionEntry, Started, StartedBy,
    State, entries, off_runtime,
};
use crate::domain::Timestamp;
use crate::records;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Record {
    #[serde(flatten)]
    pub(super) of: RunOf,
    pub(super) n: u32,
    /// Of an automation's run; absent from those recorded before it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) started_by: Option<StartedBy>,
    pub(super) started_at: Timestamp,
    pub(super) end: Option<End>,
    pub(super) sessions: Vec<RecordedSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) answered_until: Option<Timestamp>,
}

impl Record {
    pub(super) fn id(&self) -> RunId {
        RunId {
            of: self.of.clone(),
            n: self.n,
        }
    }

    /// The record of run `id` as it begins at `now`.
    pub(super) fn begun(id: RunId, started_by: Option<StartedBy>, now: Timestamp) -> Record {
        Record {
            of: id.of,
            n: id.n,
            started_by,
            started_at: now,
            end: None,
            sessions: vec![],
            answered_until: None,
        }
    }

    /// The record of `recorded`, each session's directory relative to `dir`
    /// where it is under it.
    pub(super) fn of(recorded: &Recorded, dir: &Path) -> Record {
        let Started { id, by } = recorded.started();
        Record {
            of: id.of,
            n: id.n,
            started_by: by,
            started_at: recorded.started_at,
            end: match recorded.state {
                State::Ended { at, ending } => Some(End { at, ending }),
                State::InFlight | State::Interrupted => None,
            },
            sessions: recorded
                .sessions
                .iter()
                .map(|session| RecordedSession::of(session, dir))
                .collect(),
            answered_until: recorded.answered_until,
        }
    }
}

/// A session as its run's record keeps it, its directory relative to the
/// directory of [`RunFiles`] that the record is under.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum RecordedSession {
    Ran {
        dir: PathBuf,
        #[serde(flatten)]
        ran: Ran,
    },
    /// As records kept before they kept what a session ran with.
    Dir(PathBuf),
}

impl RecordedSession {
    /// `session`, its directory relative to `dir` where it is under it.
    pub(super) fn of(session: &SessionEntry, dir: &Path) -> RecordedSession {
        let SessionEntry { dir: session, ran } = session;
        let session = session.strip_prefix(dir).unwrap_or(session).to_path_buf();
        match ran {
            Some(ran) => RecordedSession::Ran {
                dir: session,
                ran: ran.clone(),
            },
            None => RecordedSession::Dir(session),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) struct End {
    pub(super) at: Timestamp,
    pub(super) ending: Ending,
}

/// The file form of runs under a directory: each run's record at
/// `runs/items/N/<stage>-<n>.json`, or `runs/automations/NAME/<n>.json`,
/// and its sessions' directories under `sessions/`.
#[derive(Clone, Debug)]
pub struct RunFiles {
    dir: PathBuf,
}

impl RunFiles {
    pub fn new(dir: &Path) -> RunFiles {
        RunFiles {
            dir: dir.to_path_buf(),
        }
    }

    fn runs_dir(&self) -> PathBuf {
        self.dir.join("runs")
    }

    fn dir_of(&self, of: &RunOf) -> PathBuf {
        match of {
            RunOf::Work { item, .. } => self.runs_dir().join("items").join(item.0.to_string()),
            RunOf::Automation { automation } => self
                .runs_dir()
                .join(AUTOMATIONS)
                .join(automation.to_string()),
        }
    }

    /// Begins a run of `begin` here, which records no input, numbered after
    /// every run of the same item and stage, or of the same automation,
    /// with no two runs given the same number, even by two processes at
    /// once.
    pub(crate) async fn begin(&self, begin: Begin, now: Timestamp) -> Result<Running> {
        let files = self.clone();
        off_runtime(move || files.begin_now(begin, now)).await
    }

    fn begin_now(&self, begin: Begin, now: Timestamp) -> Result<Running> {
        let (of, started_by) = begin.into_parts();
        let dir = self.dir_of(&of);
        std::fs::create_dir_all(&dir).context("creating the runs' directory")?;
        let prefix = file_prefix(&of);
        let mut n = numbers(&dir, &prefix).max().unwrap_or(0) + 1;
        let (lock, lock_path) = loop {
            let path = dir.join(format!("{prefix}{n}.lock"));
            match File::options().write(true).create_new(true).open(&path) {
                Ok(file) => break (file, path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => {
                    return Err(e).with_context(|| format!("creating {}", path.display()));
                }
            }
        };
        lock.try_lock()
            .with_context(|| format!("locking {}", lock_path.display()))?;
        let record = Record::begun(RunId { of, n }, started_by, now);
        let path = lock_path.with_extension("json");
        records::save(&path, &record, "a run's record")?;
        Ok(Running::new(
            record,
            Keeping::Files { path },
            lock_path,
            self.dir.clone(),
            lock,
        ))
    }

    /// Writes `recorded` here, each session's directory kept relative to
    /// this directory where it is under it. A run with no end is written
    /// with none.
    pub fn write(&self, recorded: &Recorded) -> Result<()> {
        let path = self.record_path(&recorded.id);
        records::save(&path, &Record::of(recorded, &self.dir), "a run's record")
    }

    /// Where the record of run `id` is kept here.
    pub(super) fn record_path(&self, id: &RunId) -> PathBuf {
        self.dir_of(&id.of).join(record_file(id))
    }
}

const AUTOMATIONS: &str = "automations";

fn record_file(id: &RunId) -> String {
    format!("{}{}.json", file_prefix(&id.of), id.n)
}

fn file_prefix(of: &RunOf) -> String {
    match of {
        RunOf::Work { stage, .. } => format!("{}-", stage.name()),
        RunOf::Automation { .. } => String::new(),
    }
}

fn numbers(dir: &Path, prefix: &str) -> impl Iterator<Item = u32> {
    entries(dir)
        .unwrap_or_default()
        .into_iter()
        .filter_map(move |path| {
            let stem = path.file_stem()?.to_str()?;
            stem.strip_prefix(prefix)?.parse().ok()
        })
}
