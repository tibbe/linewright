//! What a trial changes: files of the brief and settings of the sessions
//! of its run, over every other layer (`docs/specs/benchmarks.md`,
//! Configurations).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::briefs::{self, BriefLayer, Briefs, OperatorBriefs};
use crate::config;
use crate::layer::Layer;
use crate::prompts::OwnTemplates;
use crate::repo_files::{FileError, RepoLayer};
use crate::run::RunWith;
use crate::store::Blob;
use crate::work_settings::Partial;

use super::write_whole;

/// The configuration's settings file, by its path in CONFIG.
const SETTINGS: &str = "config.toml";

/// The configuration's layer of the brief, by its path in CONFIG.
const BRIEFS: &str = "briefs/";

/// A trial's configuration, each of whose files Linewright can use.
#[derive(Debug)]
pub struct Configuration {
    settings: Partial,
    briefs: BriefLayer,
    /// Each file read, as it was read, by its path under CONFIG.
    files: BTreeMap<String, Vec<u8>>,
}

/// Why a configuration cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigurationRefusal {
    #[error("the configuration {} cannot be used: {error:#}", dir.display())]
    Unreadable { dir: PathBuf, error: anyhow::Error },
    /// Each file with an error, by its path under CONFIG.
    #[error("the configuration {} cannot be used:{}", dir.display(), listed(errors))]
    Errors {
        dir: PathBuf,
        errors: Vec<FileError>,
    },
}

/// `errors`, each on a line of its own.
fn listed(errors: &[FileError]) -> String {
    errors
        .iter()
        .map(|file| format!("\n- {}: {}", file.path, file.error))
        .collect()
}

/// Whether `path` names a file under the directory it is relative to: its
/// every part a name, none empty, `.` or `..`.
fn within(path: &str) -> bool {
    path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

const NOT_HELD: &str = "not a file a configuration may hold: it holds `config.toml` and `briefs/`";

impl Configuration {
    /// The configuration in the directory `dir`, hidden entries aside: its
    /// `config.toml` and the files of its `briefs/`.
    ///
    /// # Errors
    ///
    /// When `dir` cannot be read, and, naming each, when a file cannot be
    /// read or [`Configuration::parse`] refuses it.
    pub fn read(dir: &Path) -> Result<Configuration, ConfigurationRefusal> {
        let unreadable = |error| ConfigurationRefusal::Unreadable {
            dir: dir.to_path_buf(),
            error,
        };
        let metadata = std::fs::metadata(dir)
            .with_context(|| format!("reading {}", dir.display()))
            .map_err(unreadable)?;
        if !metadata.is_dir() {
            return Err(unreadable(anyhow::anyhow!("it is not a directory")));
        }
        let paths = briefs::files_under(dir, "configuration").map_err(unreadable)?;
        let mut files = BTreeMap::new();
        let mut unread = Vec::new();
        for path in paths {
            match std::fs::read(dir.join(&path)) {
                Ok(bytes) => {
                    files.insert(path, bytes);
                }
                Err(e) => unread.push(FileError {
                    path,
                    error: format!("it cannot be read: {e}"),
                }),
            }
        }
        match Configuration::parse(files) {
            Ok(configuration) if unread.is_empty() => Ok(configuration),
            parsed => {
                let mut errors = unread;
                errors.extend(parsed.err().into_iter().flatten());
                errors.sort();
                Err(ConfigurationRefusal::Errors {
                    dir: dir.to_path_buf(),
                    errors,
                })
            }
        }
    }

    /// The configuration of `files`, by their paths under CONFIG.
    ///
    /// # Errors
    ///
    /// Each file with an error, in order: a `config.toml` that does not
    /// parse or holds a key other than `model`, `effort` and
    /// `session-timeout`, a file of `briefs/` that a layer of the brief may
    /// not hold or that Linewright refuses, and any other file.
    pub fn parse(files: BTreeMap<String, Vec<u8>>) -> Result<Configuration, Vec<FileError>> {
        let mut errors = Vec::new();
        let mut refuse = |path: &str, error: String| {
            errors.push(FileError {
                path: path.to_string(),
                error,
            });
        };
        let mut settings = Partial::default();
        let mut brief_files = Vec::new();
        for (path, bytes) in &files {
            if path == SETTINGS {
                let parsed = Blob::from_bytes(bytes.clone())
                    .text()
                    .map_err(str::to_string)
                    .and_then(|text| {
                        config::configuration_settings(&text).map_err(|e| format!("{e:#}"))
                    });
                match parsed {
                    Ok(parsed) => settings = parsed,
                    Err(error) => refuse(path, error),
                }
            } else if let Some(under) = path.strip_prefix(BRIEFS) {
                if !within(under) {
                    refuse(path, NOT_HELD.into());
                    continue;
                }
                brief_files.push((under.to_string(), Blob::from_bytes(bytes.clone())));
            } else {
                refuse(path, NOT_HELD.into());
            }
        }
        let (briefs, mut unusable) = BriefLayer::parse(Layer::Configuration, brief_files);
        unusable.extend(Briefs::resolve(&OwnTemplates::shipped(), &[&briefs]).unusable);
        for file in unusable {
            refuse(&format!("{BRIEFS}{}", file.path), file.error);
        }
        if !errors.is_empty() {
            errors.sort();
            return Err(errors);
        }
        Ok(Configuration {
            settings,
            briefs,
            files,
        })
    }

    /// `with`, a run's resolution over `repo` and `operator`, with each
    /// setting and file of the brief this holds in place of the one it
    /// resolved.
    pub(crate) fn over(
        &self,
        with: RunWith,
        repo: &RepoLayer,
        operator: &OperatorBriefs,
    ) -> RunWith {
        let resolved = operator.with_layers(&[&self.briefs, repo.brief_layer()]);
        RunWith {
            briefs: Arc::new(resolved.briefs),
            settings: with.settings.overridden(&self.settings),
        }
    }

    /// Writes a copy of the configuration into `dir`, each file as it was
    /// read at its path under CONFIG, so that a reader sees each file
    /// whole or not at all.
    ///
    /// # Errors
    ///
    /// When a file cannot be written.
    pub fn keep(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir).with_context(|| format!("making {}", dir.display()))?;
        for (path, bytes) in &self.files {
            let file = dir.join(path);
            let parent = file.parent().expect("a file sits in a directory");
            let name = file
                .file_name()
                .expect("a file has a name")
                .to_string_lossy();
            std::fs::create_dir_all(parent)
                .with_context(|| format!("making {}", parent.display()))?;
            write_whole(&file, &parent.join(format!(".{name}")), bytes)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_log::test]
    fn a_file_of_the_brief_outside_briefs_is_not_held() {
        for path in [
            "briefs/../../escaped",
            "briefs//etc/passwd",
            "briefs/./prompts/implement.md",
        ] {
            let files = BTreeMap::from([(path.to_string(), b"text".to_vec())]);

            let errors = Configuration::parse(files).unwrap_err();

            assert_eq!(
                errors,
                [FileError {
                    path: path.to_string(),
                    error: NOT_HELD.to_string(),
                }],
                "{path}"
            );
        }
    }
}
