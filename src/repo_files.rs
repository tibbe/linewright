//! Linewright's files in the repository: its settings for the work and its
//! prompts, read from the default branch as a run starts
//! (`docs/specs/repository.md`, Linewright's files).

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::automation;
use crate::briefs::{self, Briefs, OperatorPrompts, PromptLayer};
use crate::layer::Layer;
use crate::prompts::Readers;
use crate::store::{Blob, Store};
use crate::work_settings::{RepoSettings, RunKind};

/// The path of `under` in the directory of Linewright's files, as a literal.
macro_rules! linewright_path {
    ($under:literal) => {
        concat!(".linewright", $under)
    };
}
pub(crate) use linewright_path;

/// The directory of Linewright's files.
const DIR: &str = linewright_path!("");

/// The repository's file (`docs/specs/operation.md`, The repository's file).
pub const CONFIG: &str = linewright_path!("/config.toml");

/// The repository's layer of the prompts, laid out as `briefs/prompts/` is.
pub const PROMPTS: &str = linewright_path!("/prompts");

/// A file of the repository's Linewright cannot use, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileError {
    /// Its path in the repository.
    pub path: String,
    pub error: String,
}

/// A file of the repository's Linewright cannot use, and the runs it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unusable {
    pub file: FileError,
    pub holds: Readers,
}

/// The files of `unusable` that hold the runs of `run`.
pub fn holding<'a>(unusable: &'a [Unusable], run: &RunKind) -> Vec<&'a FileError> {
    unusable
        .iter()
        .filter(|unusable| unusable.holds.read_by(run))
        .map(|unusable| &unusable.file)
        .collect()
}

/// What the repository's files give the runs that start from one read of
/// them.
#[derive(Clone, Debug)]
pub struct RepoLayer {
    pub settings: RepoSettings,
    /// The briefs, each prompt from the repository's layer, the operator's
    /// or Linewright's own.
    pub briefs: Arc<Briefs>,
    pub unusable: Vec<Unusable>,
}

impl RepoLayer {
    /// The repository's layer as it is now on `branch`, over the operator's:
    /// every file under `.linewright/`, hidden ones aside, but the
    /// automation files, which [`automation::read`] reads.
    pub async fn read(
        store: &dyn Store,
        branch: &str,
        operator: &OperatorPrompts,
    ) -> Result<RepoLayer> {
        let listed = store.list_files(branch, DIR).await?;
        let files = store.read_files(branch, &RepoLayer::paths(listed)).await?;
        Ok(RepoLayer::parse(files, operator))
    }

    /// The path in the repository of each file of the layer among `listed`,
    /// the files under `.linewright/` by their paths under it.
    fn paths(listed: Vec<String>) -> Vec<String> {
        let automations = format!("{}/", automation::DIR);
        listed
            .into_iter()
            .filter(|path| !briefs::is_hidden(path))
            .map(|path| format!("{DIR}/{path}"))
            .filter(|path| !path.starts_with(&automations))
            .collect()
    }

    /// The layer the repository's `files`, by their paths in the repository,
    /// make over the operator's prompts. A file that cannot be used holds
    /// the runs it could change, and the next layer's stands in for every
    /// other run: a config every run, a prompt the runs it is sent to, and
    /// a file that is not one of Linewright's every run
    /// (`docs/specs/repository.md`, Linewright's files).
    pub fn parse(files: BTreeMap<String, Blob>, operator: &OperatorPrompts) -> RepoLayer {
        let mut unusable = Vec::new();
        let mut settings = RepoSettings::default();
        let mut prompts = Vec::new();
        for (path, blob) in files {
            if path == CONFIG {
                let parsed = blob.text().map_err(str::to_string).and_then(|text| {
                    RepoSettings::parse(&text).map_err(|error| format!("{error:#}"))
                });
                match parsed {
                    Ok(parsed) => settings = parsed,
                    Err(error) => unusable.push(Unusable {
                        file: FileError { path, error },
                        holds: Readers::Every,
                    }),
                }
            } else if let Some(under) = path
                .strip_prefix(PROMPTS)
                .and_then(|under| under.strip_prefix('/'))
            {
                prompts.push((under.to_string(), blob));
            } else {
                unusable.push(Unusable {
                    file: FileError {
                        path,
                        error: NOT_LINEWRIGHTS.into(),
                    },
                    holds: Readers::Every,
                });
            }
        }
        let (layer, parsed) = PromptLayer::parse(Layer::Repository, prompts);
        let briefs = if layer.files.is_empty() && parsed.is_empty() {
            operator.briefs()
        } else {
            let resolved = Briefs::resolve(&[&layer, operator.layer()]);
            unusable.extend(
                parsed
                    .into_iter()
                    .chain(resolved.unusable)
                    .map(|file| Unusable {
                        holds: file.holds,
                        file: FileError {
                            path: format!("{PROMPTS}/{}", file.path),
                            error: file.error,
                        },
                    }),
            );
            Arc::new(resolved.briefs)
        };
        unusable.sort_by(|a, b| a.file.path.cmp(&b.file.path));
        RepoLayer {
            settings,
            briefs,
            unusable,
        }
    }
}

const NOT_LINEWRIGHTS: &str = "not one of Linewright's files";
