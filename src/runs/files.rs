//! The file form of a run: its record, and its sessions' directories, under
//! a directory.

use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::{
    Begin, Ending, Keeping, Ran, Recorded, RunId, RunOf, RunRecord, Running, SessionEntry, Started,
    StartedBy, State, entries, off_runtime,
};
use crate::domain::Timestamp;
use crate::input::Input;
use crate::reconcile::{OLD_DESIGN, WorkKind};
use crate::records;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Record {
    #[serde(flatten)]
    of: RunOf,
    n: u32,
    /// Of an automation's run; absent from those recorded before it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_by: Option<StartedBy>,
    pub(super) started_at: Timestamp,
    pub(super) end: Option<End>,
    pub(super) sessions: Vec<RecordedSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) answered_until: Option<Timestamp>,
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

    /// Where an earlier version kept the runs of `of`, if anywhere else.
    fn old_place_of(&self, of: &RunOf) -> Option<OldPlace> {
        match of {
            RunOf::Work {
                stage: WorkKind::Design,
                ..
            } => Some(OldPlace {
                dir: self.dir_of(of),
                prefix: format!("{OLD_DESIGN}-"),
            }),
            RunOf::Work { .. } => None,
            RunOf::Automation { automation } => Some(OldPlace {
                dir: self
                    .runs_dir()
                    .join(OLD_AUTOMATIONS)
                    .join(automation.to_string()),
                prefix: file_prefix(of),
            }),
        }
    }

    /// Begins a run of `begin` here, which records no input.
    pub(crate) async fn begin(&self, begin: Begin, now: Timestamp) -> Result<Running> {
        self.begin_kept(begin, now, Keeping::Files).await
    }

    /// Begins a run of `begin`, kept as `keeping` says, numbered after every
    /// run of the same item and stage, or of the same automation, with no
    /// two runs given the same number, even by two processes at once.
    pub(super) async fn begin_kept(
        &self,
        begin: Begin,
        now: Timestamp,
        keeping: Keeping,
    ) -> Result<Running> {
        let files = self.clone();
        off_runtime(move || files.begin_now(begin, now, keeping)).await
    }

    fn begin_now(&self, begin: Begin, now: Timestamp, keeping: Keeping) -> Result<Running> {
        let (of, started_by) = begin.into_parts();
        let dir = self.dir_of(&of);
        std::fs::create_dir_all(&dir).context("creating the runs' directory")?;
        let prefix = file_prefix(&of);
        let old = self.old_place_of(&of);
        let mut n = numbers(&dir, &prefix)
            .chain(old.iter().flat_map(|old| numbers(&old.dir, &old.prefix)))
            .max()
            .unwrap_or(0)
            + 1;
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
        let running = Running {
            kept: RunRecord {
                id: RunId { of: of.clone(), n },
                path: lock_path.with_extension("json"),
                record: Arc::new(Mutex::new(Record {
                    of,
                    n,
                    started_by,
                    started_at: now,
                    end: None,
                    sessions: vec![],
                    answered_until: None,
                })),
            },
            keeping,
            lock_path,
            dir: self.dir.clone(),
            _lock: lock,
        };
        running.kept.save()?;
        Ok(running)
    }

    /// Writes `recorded` here, each session's directory kept relative to
    /// this directory where it is under it. A run with no end is written
    /// with none, and reads back as interrupted.
    pub fn write(&self, recorded: &Recorded) -> Result<()> {
        let Started { id, by } = recorded.started();
        let path = self.dir_of(&id.of).join(record_file(&id));
        std::fs::create_dir_all(path.parent().expect("a run's record is in a directory"))
            .context("creating the runs' directory")?;
        let record = Record {
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
                .map(|SessionEntry { dir, ran }| {
                    let dir = dir.strip_prefix(&self.dir).unwrap_or(dir).to_path_buf();
                    match ran {
                        Some(ran) => RecordedSession::Ran {
                            dir,
                            ran: ran.clone(),
                        },
                        None => RecordedSession::Dir(dir),
                    }
                })
                .collect(),
            answered_until: recorded.answered_until,
        };
        records::save(&path, &record, "a run's record")
    }

    fn path_of(&self, id: &RunId) -> PathBuf {
        let path = self.dir_of(&id.of).join(record_file(id));
        match self.old_place_of(&id.of) {
            Some(old) if !path.exists() && old.path(id.n).exists() => old.path(id.n),
            _ => path,
        }
    }

    /// The input run `id` recorded at its start, `None` when it recorded
    /// none.
    pub(super) fn read_input(&self, id: &RunId) -> Result<Option<Input>> {
        records::read(&input_path(&self.path_of(id)))
    }

    /// A record that does not read is logged and taken as missing.
    pub(super) fn read_run(&self, id: &RunId) -> Option<Recorded> {
        self.load(&self.path_of(id))
    }

    /// `None` if the run has no record; a record that does not read fails.
    pub(super) fn try_read_run(&self, id: &RunId) -> Result<Option<Recorded>> {
        let path = self.path_of(id);
        let record = records::read::<Record>(&path)?;
        record
            .map(|record| recorded(record, &path, &self.dir))
            .transpose()
            .map_err(anyhow::Error::msg)
    }

    /// A record that does not read is logged and taken as missing.
    fn load(&self, path: &Path) -> Option<Recorded> {
        let record = records::load::<Record>(path, "a run's record")?;
        recorded(record, path, &self.dir)
            .inspect_err(|e| warn!(path = %path.display(), error = e, "ignoring a run's record"))
            .ok()
    }

    /// Every run whose record is here, where this version keeps it or where
    /// an earlier one did, with a record that does not read logged and
    /// skipped.
    pub fn read_legacy(&self) -> Result<Vec<Recorded>> {
        let mut recorded = vec![];
        for kind in ["items", AUTOMATIONS, OLD_AUTOMATIONS] {
            for dir in entries(&self.runs_dir().join(kind))? {
                for path in entries(&dir)? {
                    if path.extension().is_some_and(|e| e == "json")
                        && !is_input(&path)
                        && let Some(record) = self.load(&path)
                    {
                        recorded.push(record);
                    }
                }
            }
        }
        Ok(recorded)
    }
}

const AUTOMATIONS: &str = "automations";

/// Where versions before automations were renamed from workflows kept
/// automations' runs: read beside [`AUTOMATIONS`], never written to.
const OLD_AUTOMATIONS: &str = "workflows";

/// Where an earlier version kept a run's records: their directory and the
/// prefix of their files.
struct OldPlace {
    dir: PathBuf,
    prefix: String,
}

impl OldPlace {
    fn path(&self, n: u32) -> PathBuf {
        self.dir.join(format!("{}{n}.json", self.prefix))
    }
}

fn record_file(id: &RunId) -> String {
    format!("{}{}.json", file_prefix(&id.of), id.n)
}

/// Where the input of the run whose record is at `record` is kept: beside
/// it, as `<stage>-<n>.input.json`.
pub(super) fn input_path(record: &Path) -> PathBuf {
    record.with_extension(INPUT_EXTENSION)
}

fn is_input(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(&format!(".{INPUT_EXTENSION}")))
}

const INPUT_EXTENSION: &str = "input.json";

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

/// The run whose record at `path` under `dir` is `record`.
fn recorded(record: Record, path: &Path, dir: &Path) -> Result<Recorded, String> {
    let state = match record.end {
        Some(End { at, ending }) => State::Ended { at, ending },
        None if is_locked(&path.with_extension("lock")) => State::InFlight,
        None => State::Interrupted,
    };
    let id = RunId {
        of: record.of,
        n: record.n,
    };
    Ok(Recorded::new(
        Started::new(id, record.started_by)?,
        record.started_at,
        state,
        record
            .sessions
            .into_iter()
            .map(|session| match session {
                RecordedSession::Ran { dir: session, ran } => SessionEntry {
                    dir: dir.join(session),
                    ran: Some(ran),
                },
                RecordedSession::Dir(session) => dir.join(session).into(),
            })
            .collect(),
        record.answered_until,
    ))
}

fn is_locked(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    matches!(file.try_lock(), Err(TryLockError::WouldBlock))
}
