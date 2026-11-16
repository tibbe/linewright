//! A judge's copy of the parts of a replay's output that it judges
//! (`docs/specs/benchmarks.md`, Judging, "What the judge reads").

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::Replayed;
use crate::replay::journal::in_safe_name;
use crate::replay::{ATTACHMENTS, OUTCOME, SESSIONS, WRITES, attachment_link};
use crate::tree;

/// A copy of the outcome, the sessions' records, the writes and the
/// attachments the writes refer to of a replay's output, each at its path
/// in the output, in a directory apart from it, and nothing else of the
/// output. Its directory is its maker's to remove.
#[derive(Debug)]
pub struct ReplayCopy {
    root: PathBuf,
}

impl ReplayCopy {
    /// Copies the parts of `replayed`'s output that a judge reads into
    /// `into`, an empty directory apart from the output. A part the output
    /// lacks is left out, but for the outcome, which every [`Replayed`]
    /// has.
    ///
    /// # Errors
    ///
    /// When a part cannot be read or copied.
    pub async fn make(replayed: &Replayed, into: &Path) -> Result<ReplayCopy> {
        let from = replayed.dir();
        let into = std::path::absolute(into)
            .with_context(|| format!("making {} absolute", into.display()))?;
        copy_file(&from.join(OUTCOME), &into.join(OUTCOME)).await?;
        for dir in [SESSIONS, WRITES] {
            if exists(&from.join(dir)).await? {
                tree::copy(&from.join(dir), &into.join(dir))
                    .await
                    .with_context(|| format!("copying the output's `{dir}`"))?;
            }
        }
        let attachments = names(&from.join(ATTACHMENTS)).await?;
        let mut writes = vec![];
        let writes_dir = from.join(WRITES);
        for name in names(&writes_dir).await? {
            let path = writes_dir.join(&name);
            let text = tokio::fs::read(&path)
                .await
                .with_context(|| format!("reading {}", path.display()))?;
            writes.push(String::from_utf8_lossy(&text).into_owned());
        }
        let referred = referred(&writes, &attachments);
        if !referred.is_empty() {
            tokio::fs::create_dir(into.join(ATTACHMENTS))
                .await
                .with_context(|| format!("making `{ATTACHMENTS}` in the judge's copy"))?;
        }
        for name in referred {
            let path = Path::new(ATTACHMENTS).join(name);
            copy_file(&from.join(&path), &into.join(&path)).await?;
        }
        Ok(ReplayCopy { root: into })
    }

    /// Where the copy's outcome is: an absolute path.
    pub fn outcome(&self) -> PathBuf {
        self.root.join(OUTCOME)
    }

    /// Where the copy's sessions' records are, if the output has any: an
    /// absolute path.
    pub fn sessions(&self) -> PathBuf {
        self.root.join(SESSIONS)
    }

    /// Where the copy's writes are, if the output has any: an absolute
    /// path.
    pub fn writes(&self) -> PathBuf {
        self.root.join(WRITES)
    }
}

/// The names of `attachments` that any of the texts of the writes `writes`
/// links to: its link, not followed by a character a name can hold.
fn referred<'a>(writes: &[String], attachments: &'a [String]) -> BTreeSet<&'a str> {
    attachments
        .iter()
        .filter(|name| {
            let link = attachment_link(name);
            writes.iter().any(|write| {
                write
                    .match_indices(&link)
                    .any(|(at, _)| !write[at + link.len()..].starts_with(in_safe_name))
            })
        })
        .map(String::as_str)
        .collect()
}

async fn copy_file(from: &Path, to: &Path) -> Result<()> {
    tokio::fs::copy(from, to)
        .await
        .with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
    Ok(())
}

async fn exists(path: &Path) -> Result<bool> {
    tokio::fs::try_exists(path)
        .await
        .with_context(|| format!("reading {}", path.display()))
}

/// The names in the directory `dir`, or none when there is no `dir`.
async fn names(dir: &Path) -> Result<Vec<String>> {
    let reading = |e| anyhow::Error::new(e).context(format!("reading {}", dir.display()));
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(reading(e)),
    };
    let mut names = vec![];
    while let Some(entry) = entries.next_entry().await.map_err(reading)? {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_log::test]
    fn an_attachment_is_picked_when_a_write_refers_to_it_and_not_otherwise() {
        let writes = vec![
            r#"{"body": "See ![the crash](../attachments/crash.png)."}"#.to_string(),
            r#"{"body": "Nothing attached; crash-2.png was not sent."}"#.to_string(),
            r#"{"body": "[The log](../attachments/log-2)"}"#.to_string(),
        ];
        let attachments = vec![
            "crash.png".to_string(),
            "crash-2.png".to_string(),
            "trace.webm".to_string(),
            "log".to_string(),
            "log-2".to_string(),
        ];

        let picked = referred(&writes, &attachments);

        assert_eq!(picked, BTreeSet::from(["crash.png", "log-2"]));
    }
}
