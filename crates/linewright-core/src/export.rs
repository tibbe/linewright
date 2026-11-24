//! `lw export ID DIR`: a run of the work directory's, written into a
//! directory in the form that the output of a trial gives its run
//! (`docs/specs/runs.md`, `lw export`).

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::runs::files::RunFiles;
use crate::runs::{OutcomeName, Recorded, RunId, Runs, SessionEntry, State, Unended};
use crate::transcript;
use crate::trial::{self, OutputRefusal};

/// Why a run was not exported.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("there is no run {0}")]
    NoSuchRun(RunId),
    #[error(transparent)]
    Refused(#[from] OutputRefusal),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// Writes run `id` of `runs`, or of none where there is no database, into
/// `dir` as it is now: its outcome, its record and its sessions'
/// directories, each at its path in the output of a trial. `dir` is made
/// when it is missing. Gives `dir` as an absolute path.
///
/// # Errors
///
/// When there is no run `id`, when `dir` holds anything or cannot be made,
/// or when the run cannot be read or written.
pub async fn run(runs: Option<&Runs>, id: &RunId, dir: &Path) -> Result<PathBuf, ExportError> {
    let runs = runs.ok_or_else(|| ExportError::NoSuchRun(id.clone()))?;
    snapshot(runs, id).await?.write(dir).await
}

/// A run as it was when its snapshot was taken: its record, read once, and
/// what its sessions' directories held then.
#[derive(Debug)]
struct Snapshot {
    /// With no sessions: they are in `sessions`.
    recorded: Recorded,
    sessions: Vec<SessionSnapshot>,
}

/// A session of a run, and what its directory held when the snapshot was
/// taken.
#[derive(Debug)]
struct SessionSnapshot {
    session: SessionEntry,
    /// The session's directory's path in the output, relative to it.
    in_output: PathBuf,
    /// Each relative to the session's directory, parents before what they
    /// hold; `None` when there was no directory.
    held: Option<Vec<Held>>,
}

/// What a session's directory held.
#[derive(Debug)]
enum Held {
    Dir(PathBuf),
    /// A file, and for its transcript, which only grows, its length: what
    /// it held is its first `len` bytes.
    File {
        path: PathBuf,
        len: Option<u64>,
    },
    Link {
        path: PathBuf,
        target: PathBuf,
    },
}

/// Run `id` of `runs` as it is now.
async fn snapshot(runs: &Runs, id: &RunId) -> Result<Snapshot, ExportError> {
    let mut recorded = runs
        .try_read_run(id)
        .await?
        .ok_or_else(|| ExportError::NoSuchRun(id.clone()))?;
    let work_dir = runs.database().work_dir().to_path_buf();
    let snapshot = tokio::task::spawn_blocking(move || {
        let sessions = std::mem::take(&mut recorded.sessions)
            .into_iter()
            .map(|session| {
                Ok(SessionSnapshot {
                    in_output: in_output(&work_dir, &session.dir)?,
                    held: held(&session.dir)?,
                    session,
                })
            })
            .collect::<Result<_>>()?;
        Ok::<_, anyhow::Error>(Snapshot { recorded, sessions })
    })
    .await
    .context("reading the run's sessions")??;
    Ok(snapshot)
}

/// The path in the output of the session's directory `dir`: its path under
/// `work_dir`, as a trial keeps its sessions under its output.
fn in_output(work_dir: &Path, dir: &Path) -> Result<PathBuf> {
    match dir.strip_prefix(work_dir) {
        Ok(relative)
            if relative.components().next().is_some()
                && relative
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))) =>
        {
            Ok(relative.to_path_buf())
        }
        _ => bail!(
            "the session's directory {} is not in the work directory",
            dir.display()
        ),
    }
}

/// What the directory `dir` holds, `None` when there is no `dir`. Only
/// directories, files and links are kept.
fn held(dir: &Path) -> Result<Option<Vec<Held>>> {
    let mut found = vec![];
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let at = dir.join(&relative);
        let listing = match std::fs::read_dir(&at) {
            Ok(listing) => listing,
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound && relative.as_os_str().is_empty() =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", at.display())),
        };
        for entry in listing {
            let entry = entry.with_context(|| format!("reading {}", at.display()))?;
            let path = relative.join(entry.file_name());
            let kind = entry
                .file_type()
                .with_context(|| format!("reading {}", entry.path().display()))?;
            if kind.is_dir() {
                found.push(Held::Dir(path.clone()));
                pending.push(path);
            } else if kind.is_symlink() {
                let target = std::fs::read_link(entry.path())
                    .with_context(|| format!("reading {}", entry.path().display()))?;
                found.push(Held::Link { path, target });
            } else if kind.is_file() {
                let len = if path == Path::new(transcript::OUTPUT) {
                    let metadata = entry
                        .metadata()
                        .with_context(|| format!("reading {}", entry.path().display()))?;
                    Some(metadata.len())
                } else {
                    None
                };
                found.push(Held::File { path, len });
            }
        }
    }
    Ok(Some(found))
}

impl Snapshot {
    /// Writes the run into `dir`, made when it is missing: its sessions'
    /// directories, its record, and then its outcome. Gives `dir` as an
    /// absolute path.
    async fn write(self, dir: &Path) -> Result<PathBuf, ExportError> {
        let dir = trial::make_empty_dir(dir)?;
        let into = dir.clone();
        tokio::task::spawn_blocking(move || self.write_blocking(&into))
            .await
            .context("writing the run")??;
        Ok(dir)
    }

    fn write_blocking(self, dir: &Path) -> Result<()> {
        let mut recorded = self.recorded;
        for snapshot in self.sessions {
            snapshot.copy_into(dir)?;
            recorded.sessions.push(SessionEntry {
                dir: dir.join(&snapshot.in_output),
                kept: snapshot.session.kept,
            });
        }
        RunFiles::new(dir).write(&recorded)?;
        trial::write_outcome(dir, outcome(recorded.state))
    }
}

impl SessionSnapshot {
    /// Copies what the session's directory held to its path under `dir`:
    /// its transcript as far as it went, and each other file that is still
    /// there as it is now, with its permissions.
    fn copy_into(&self, dir: &Path) -> Result<()> {
        let Some(everything) = &self.held else {
            return Ok(());
        };
        let (from, to) = (&self.session.dir, dir.join(&self.in_output));
        std::fs::create_dir_all(&to).with_context(|| format!("making {}", to.display()))?;
        for held in everything {
            match held {
                Held::Dir(path) => {
                    let path = to.join(path);
                    std::fs::create_dir(&path)
                        .with_context(|| format!("making {}", path.display()))?;
                }
                Held::File { path, len } => copy_file(&from.join(path), &to.join(path), *len)?,
                Held::Link { path, target } => {
                    let path = to.join(path);
                    std::os::unix::fs::symlink(target, &path)
                        .with_context(|| format!("making {}", path.display()))?;
                }
            }
        }
        Ok(())
    }
}

/// Copies the file `from` to `to`, cut to its first `len` bytes where
/// given. A file that is gone is left out.
fn copy_file(from: &Path, to: &Path, len: Option<u64>) -> Result<()> {
    let copying = || -> std::io::Result<()> {
        match std::fs::copy(from, to) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            copied => copied?,
        };
        if let Some(len) = len {
            std::fs::OpenOptions::new()
                .write(true)
                .open(to)?
                .set_len(len)?;
        }
        Ok(())
    };
    copying().with_context(|| format!("copying {} to {}", from.display(), to.display()))
}

/// The outcome of a run in `state`, as the output of a trial names it.
fn outcome(state: State) -> OutcomeName {
    match state {
        State::Ended { ending, .. } => OutcomeName::Ended(ending),
        State::InFlight => OutcomeName::Unended(Unended::InFlight),
        State::Interrupted => OutcomeName::Unended(Unended::Interrupted),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::database::Database;
    use crate::testing::{keep_record, lock_of};
    use crate::transcript::OUTPUT;

    /// The runs of a database made in `work_dir`.
    async fn runs_in(work_dir: &Path) -> Runs {
        let db = Database::open(work_dir)
            .await
            .unwrap_or_else(|e| panic!("opening the database: {e:#}"));
        Runs::new(db)
    }

    /// A session of a triage run in `dir`, as its run's record keeps it, with
    /// what it ran with.
    fn session(dir: &str) -> serde_json::Value {
        json!({
            "dir": dir,
            "model": "opus",
            "effort": null,
            "layers": {"settings": {}, "briefs": {}},
        })
    }

    /// Keeps the triage run 1/triage-1 in the database of `work_dir`, started
    /// at 09:00 and, if `ending` is given, ended so at 09:01, its first session's
    /// directory, `sessions/1/triage-1` under `work_dir`, holding a brief, an
    /// attachment and a transcript of one line, and then the sessions in the
    /// directories `others`.
    async fn triaging(work_dir: &Path, others: &[&str], ending: Option<&str>) {
        let session_dir = work_dir.join("sessions/1/triage-1");
        for (path, text) in [
            ("brief.md", "Triage #1.\n"),
            (OUTPUT, "{\"first\":1}\n"),
            ("attachments/shot.png", "PNG"),
        ] {
            let path = session_dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap())
                .and_then(|()| std::fs::write(&path, text))
                .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
        }
        triage_recorded(work_dir, others, ending).await;
    }

    /// Keeps the record of the run [`triaging`] keeps, with its sessions'
    /// directories as they are, in place of any record of it.
    async fn triage_recorded(work_dir: &Path, others: &[&str], ending: Option<&str>) {
        let sessions: Vec<serde_json::Value> = std::iter::once("sessions/1/triage-1")
            .chain(others.iter().copied())
            .map(session)
            .collect();
        let end = ending.map(|ending| json!({"at": "2026-10-15T09:01:00Z", "ending": ending}));
        keep_record(
            work_dir,
            &json!({
                "item": 1,
                "stage": "triage",
                "n": 1,
                "started_at": "2026-10-15T09:00:00Z",
                "end": end,
                "sessions": sessions,
            }),
        )
        .await;
    }

    /// Holds the lock of the run 1/triage-1 of `work_dir`, as its process does
    /// while it is in flight, until it is dropped.
    fn in_flight(work_dir: &Path) -> std::fs::File {
        lock_of(&work_dir.join("runs/items/1/triage-1.json"))
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test_log::test(tokio::test)]
    async fn an_ended_run_is_written_with_its_ending_its_record_and_its_sessions() {
        let work_dir = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let dir = out.path().join("export");
        let runs = runs_in(work_dir.path()).await;
        triaging(work_dir.path(), &[], Some("done")).await;
        let id: RunId = "1/triage-1".parse().unwrap();

        run(Some(&runs), &id, &dir).await.unwrap();

        assert_eq!(read(&dir.join("outcome")), "done\n");
        let session = dir.join("sessions/1/triage-1");
        assert_eq!(read(&session.join("brief.md")), "Triage #1.\n");
        assert_eq!(read(&session.join(OUTPUT)), "{\"first\":1}\n");
        assert_eq!(read(&session.join("attachments/shot.png")), "PNG");
        let record: serde_json::Value =
            serde_json::from_str(&read(&dir.join("runs/items/1/triage-1.json"))).unwrap();
        assert_eq!(record["sessions"][0]["dir"], "sessions/1/triage-1");
        assert_eq!(record["end"]["ending"], "done");
    }

    #[test_log::test(tokio::test)]
    async fn a_run_in_flight_is_written_as_it_was_when_it_was_taken() {
        let work_dir = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let dir = out.path().join("export");
        let runs = runs_in(work_dir.path()).await;
        triaging(work_dir.path(), &["sessions/1/triage-0"], None).await;
        let _lock = in_flight(work_dir.path());
        let session = work_dir.path().join("sessions/1/triage-1");
        let id: RunId = "1/triage-1".parse().unwrap();

        let snapshot = snapshot(&runs, &id).await.unwrap();
        let mut transcript = std::fs::OpenOptions::new()
            .append(true)
            .open(session.join(OUTPUT))
            .unwrap();
        std::io::Write::write_all(&mut transcript, b"{\"second\":2}\n").unwrap();
        std::fs::write(session.join("follow-up-1.md"), "Again.\n").unwrap();
        std::fs::remove_file(session.join("attachments/shot.png")).unwrap();
        triage_recorded(
            work_dir.path(),
            &["sessions/1/triage-0", "sessions/1/triage-2"],
            Some("done"),
        )
        .await;
        snapshot.write(&dir).await.unwrap();

        assert_eq!(read(&dir.join("outcome")), "in-flight\n");
        let copied = dir.join("sessions/1/triage-1");
        assert_eq!(read(&copied.join(OUTPUT)), "{\"first\":1}\n");
        assert!(!copied.join("follow-up-1.md").exists());
        assert!(copied.join("attachments").is_dir());
        assert!(!copied.join("attachments/shot.png").exists());
        assert!(!dir.join("sessions/1/triage-2").exists());
        let record: serde_json::Value =
            serde_json::from_str(&read(&dir.join("runs/items/1/triage-1.json"))).unwrap();
        assert_eq!(record["sessions"].as_array().unwrap().len(), 2);
        assert!(!dir.join("sessions/1/triage-0").exists());
        assert_eq!(record["end"], serde_json::Value::Null);
    }

    #[test_log::test(tokio::test)]
    async fn an_unknown_run_is_refused_and_makes_no_directory() {
        let work_dir = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let dir = out.path().join("export");
        let runs = runs_in(work_dir.path()).await;
        let id: RunId = "1/triage-1".parse().unwrap();

        let in_database = run(Some(&runs), &id, &dir).await;
        let without = run(None, &id, &dir).await;

        assert!(
            matches!(in_database, Err(ExportError::NoSuchRun(_))),
            "{in_database:?}"
        );
        assert!(
            matches!(without, Err(ExportError::NoSuchRun(_))),
            "{without:?}"
        );
        assert!(!dir.exists());
    }

    #[test_log::test(tokio::test)]
    async fn a_directory_that_holds_anything_is_refused() {
        let work_dir = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("kept"), "").unwrap();
        let runs = runs_in(work_dir.path()).await;
        triaging(work_dir.path(), &[], None).await;
        let _lock = in_flight(work_dir.path());
        let id: RunId = "1/triage-1".parse().unwrap();

        let refused = run(Some(&runs), &id, dir.path()).await;

        assert!(
            matches!(
                refused,
                Err(ExportError::Refused(OutputRefusal::NotEmpty(_)))
            ),
            "{refused:?}"
        );
        assert!(!dir.path().join("outcome").exists());
    }
}
