//! Linewright's files in the repository: its settings for the work and its
//! prompts, read from the default branch as a run starts
//! (`docs/specs/repository.md`, Linewright's files).

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::briefs::{Briefs, OperatorPrompts, PromptLayer};
use crate::layer::Layer;
use crate::prompts::{self, Prompts, Readers};
use crate::store::{Blob, Store};
use crate::work_settings::{RepoSettings, RunKind};

/// The repository's file (`docs/specs/operation.md`, The repository's file).
pub const CONFIG: &str = ".linewright/config.toml";

/// The root of the repository's layer of the prompts, laid out as
/// `briefs/prompts/` is.
pub const PROMPTS: &str = ".linewright/briefs/prompts/";

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
    /// The repository's layer as it is now on `branch`, over the operator's.
    pub async fn read(
        store: &dyn Store,
        branch: &str,
        operator: &OperatorPrompts,
    ) -> Result<RepoLayer> {
        let files = store.read_files(branch, &paths()).await?;
        Ok(RepoLayer::parse(files, operator))
    }

    /// The layer the repository's `files`, by their paths in it, make over
    /// the operator's prompts. A file that cannot be used holds the runs that
    /// read it, and the next layer's stands in for every other run.
    pub fn parse(files: BTreeMap<String, Blob>, operator: &OperatorPrompts) -> RepoLayer {
        let mut unusable = Vec::new();
        let mut settings = RepoSettings::default();
        let mut layer_files = Vec::new();
        for (path, blob) in files {
            let text = match blob {
                Blob::Text(text) => Ok(text),
                Blob::Binary => Err("it is not UTF-8 text".to_string()),
                Blob::TooLarge => Err("it is too large to read".to_string()),
            };
            if path == CONFIG {
                match text.and_then(|text| {
                    RepoSettings::parse(&text).map_err(|error| format!("{error:#}"))
                }) {
                    Ok(parsed) => settings = parsed,
                    Err(error) => unusable.push(Unusable {
                        file: FileError { path, error },
                        holds: Readers::Every,
                    }),
                }
                continue;
            }
            let Some(prompt) = path.strip_prefix(PROMPTS) else {
                continue;
            };
            match text {
                Ok(text) => layer_files.push((prompt.to_string(), text.into_bytes())),
                Err(error) => unusable.push(Unusable {
                    holds: prompts::readers(prompt).unwrap_or(Readers::Every),
                    file: FileError { path, error },
                }),
            }
        }
        let (layer, parsed) = PromptLayer::parse(Layer::Repository, layer_files);
        let briefs = if layer.files.is_empty() && parsed.is_empty() {
            operator.briefs()
        } else {
            let resolved = Briefs::resolve(&[&layer, operator.layer()]);
            unusable.extend(
                parsed
                    .into_iter()
                    .chain(resolved.unusable)
                    .map(|file| Unusable {
                        holds: prompts::readers(&file.path).unwrap_or(Readers::Every),
                        file: FileError {
                            path: format!("{PROMPTS}{}", file.path),
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

/// The path in the repository of each of Linewright's files that runs read:
/// its config and every one of Linewright's prompts.
fn paths() -> Vec<String> {
    std::iter::once(CONFIG.to_string())
        .chain(
            Prompts::paths()
                .into_iter()
                .map(|path| format!("{PROMPTS}{path}")),
        )
        .collect()
}
